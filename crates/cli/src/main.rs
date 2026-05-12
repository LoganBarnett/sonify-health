mod logging;
mod print;

use sonify_health_cli::{command, config, patch_args};

use clap::Parser;
use command::{Command, PrintFormat};
use config::{Config, ConfigError};
use logging::init_logging;
use patch_args::CliPatchOverrides;
use sonify_health_lib::{
  audio::{AudioError, AudioMixer, AudioOutput},
  heartbeat, ResolvedNote,
};
use std::process::ExitCode;
use std::sync::{
  atomic::{AtomicBool, Ordering},
  Arc,
};
use thiserror::Error;
use tracing::{debug, info};

/// CLI args parser — alias for the macro-generated `CliRaw`.
type Cli = <Config as rust_template_foundation::CliApp>::CliArgs;

#[derive(Debug, Error)]
enum ApplicationError {
  #[error("Failed to load configuration: {0}")]
  ConfigurationLoad(#[source] Box<ConfigError>),

  #[error("Unknown patch name: {0}")]
  UnknownPatch(String),

  #[error("Audio playback failed: {0}")]
  AudioPlayback(#[from] AudioError),

  #[error("Failed to install {signal} handler: {source}")]
  SignalHandlerInstallFailed {
    signal: &'static str,
    #[source]
    source: ctrlc::Error,
  },
}

impl From<ConfigError> for ApplicationError {
  fn from(e: ConfigError) -> Self {
    Self::ConfigurationLoad(Box::new(e))
  }
}

fn main() -> ExitCode {
  let cli = Cli::parse();
  let config = match Config::from_cli_and_file(cli) {
    Ok(c) => c,
    Err(e) => {
      eprintln!("Configuration error: {e}");
      return ExitCode::FAILURE;
    }
  };

  init_logging(config.log_level, config.log_format);

  debug!(
    log_level = ?config.log_level,
    log_format = ?config.log_format,
    audio_device = ?config.audio_device,
    "Resolved configuration"
  );

  let result = match &config.command {
    Command::Preview { continuous, patch } => {
      run_preview(&config, patch, *continuous)
    }
    Command::Print { format, patch } => {
      run_print(&config, patch, format.clone());
      Ok(())
    }
  };

  match result {
    Ok(()) => ExitCode::SUCCESS,
    Err(e) => {
      tracing::error!("Application error: {e}");
      ExitCode::FAILURE
    }
  }
}

// -- Preview -----------------------------------------------------------------

fn run_preview(
  config: &Config,
  patch_args: &CliPatchOverrides,
  continuous: bool,
) -> Result<(), ApplicationError> {
  if !config.library.contains_key(&patch_args.patch_name) {
    return Err(ApplicationError::UnknownPatch(patch_args.patch_name.clone()));
  }

  let patch = patch_args.resolve_patch(&config.library);
  debug!(?patch, "Resolved patch");
  info!(
    patch_name = %patch_args.patch_name,
    freq = patch.freq,
    "Playing preview"
  );

  if continuous {
    run_continuous_preview(patch, config.audio_device.as_deref())
  } else {
    let notes = [ResolvedNote {
      patch,
      volume: 1.0,
      offset: 0.0,
    }];
    let graph = heartbeat::heartbeat_graph_with_notes(&notes, None);
    let dur = heartbeat::heartbeat_notes_duration(&notes);
    AudioOutput::play_for(graph, dur, config.audio_device.as_deref())
      .map_err(ApplicationError::AudioPlayback)
  }
}

fn run_continuous_preview(
  patch: sonify_health_lib::Patch,
  audio_device: Option<&str>,
) -> Result<(), ApplicationError> {
  let mixer = AudioMixer::new(audio_device)?;
  let run = Arc::new(AtomicBool::new(true));
  let (tx, rx) = std::sync::mpsc::channel();
  ctrlc::set_handler(move || {
    let _ = tx.send(());
  })
  .map_err(|source| ApplicationError::SignalHandlerInstallFailed {
    signal: "Ctrl-C",
    source,
  })?;

  info!("Playing continuously, press Ctrl-C to stop");
  let play_run = Arc::clone(&run);
  let handle = mixer.handle();
  let play_handle = std::thread::spawn(move || {
    while play_run.load(Ordering::Relaxed) {
      let notes = [ResolvedNote {
        patch: patch.clone(),
        volume: 1.0,
        offset: 0.0,
      }];
      let graph = heartbeat::heartbeat_graph_with_notes(&notes, None);
      let dur = heartbeat::heartbeat_notes_duration(&notes);
      let slot = handle.add(graph);
      std::thread::sleep(dur);
      handle.remove(slot);
    }
  });

  rx.recv().ok();
  run.store(false, Ordering::Relaxed);
  let _ = play_handle.join();
  Ok(())
}

// -- Print -------------------------------------------------------------------

fn run_print(
  config: &Config,
  patch_args: &CliPatchOverrides,
  format: PrintFormat,
) {
  let output = match format {
    PrintFormat::Toml => print::format_toml(&config.library),
    PrintFormat::Nix => print::format_nix(&config.library),
    PrintFormat::Cli => {
      print::format_cli(&patch_args.resolve_patch(&config.library))
    }
  };
  println!("{output}");
}
