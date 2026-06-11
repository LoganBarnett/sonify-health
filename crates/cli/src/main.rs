mod print;

use sonify_health_cli::{command, config, patch_args};

use command::{Command, PrintFormat};
use config::Config;
use patch_args::CliPatchOverrides;
use rust_template_foundation::main as foundation_main;
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

#[derive(Debug, Error)]
enum ApplicationError {
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

// The `#[foundation_main]` macro generates the real `fn main()`: it parses the
// CLI, resolves the `Config` (config-file load plus CLI merge), initializes CLI
// logging from the resolved log settings, and maps the returned `Result` to an
// `ExitCode`, logging the error on the `Err` path.  This function holds only the
// command dispatch.
#[foundation_main]
pub fn main(config: Config) -> Result<ExitCode, ApplicationError> {
  debug!(
    log_level = ?config.log_level,
    log_format = ?config.log_format,
    audio_device = ?config.audio_device,
    "Resolved configuration"
  );

  match &config.command {
    Command::Preview { continuous, patch } => {
      run_preview(&config, patch, *continuous)?
    }
    Command::Print { format, patch } => {
      run_print(&config, patch, format.clone())
    }
  }

  Ok(ExitCode::SUCCESS)
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
