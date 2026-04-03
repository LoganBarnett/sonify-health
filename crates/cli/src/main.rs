mod config;
mod daemon;
mod logging;

use clap::{Parser, Subcommand, ValueEnum};
use config::{Config, ConfigError};
use daemon::DaemonError;
use fundsp::prelude32::shared;
use logging::init_logging;
use sonify_health_lib::{
  audio::{AudioError, AudioOutput},
  drone, heartbeat, DroneRegister, Severity, Voice,
};
use std::time::Duration;
use thiserror::Error;
use tracing::info;

#[derive(Debug, Error)]
enum ApplicationError {
  #[error("Failed to load configuration: {0}")]
  ConfigurationLoad(#[from] ConfigError),

  #[error("Invalid severity input: {0}")]
  InvalidSeverity(String),

  #[error("Invalid drone metric: {0}")]
  InvalidDroneMetric(String),

  #[error("Audio playback failed: {0}")]
  AudioPlayback(#[from] AudioError),

  #[error("Daemon failed: {0}")]
  Daemon(#[from] DaemonError),
}

#[derive(Parser)]
#[command(
  name = "sonify-health",
  author,
  version,
  about = "Infrastructure sonification daemon and CLI"
)]
struct Cli {
  #[arg(long, env = "LOG_LEVEL")]
  log_level: Option<String>,

  #[arg(long, env = "LOG_FORMAT")]
  log_format: Option<String>,

  #[arg(short, long, env = "CONFIG_FILE")]
  config: Option<std::path::PathBuf>,

  #[command(subcommand)]
  command: Command,
}

#[derive(Clone, Debug, ValueEnum)]
enum SoundType {
  Health,
  Drone,
}

#[derive(Subcommand)]
enum Command {
  /// Preview a sound layer (heartbeat or drone).
  Preview {
    /// Sound type to preview.
    #[arg(long, value_enum, default_value_t = SoundType::Health)]
    sound_type: SoundType,

    /// Sugar: equivalent to --sound-type health.
    #[arg(long, conflicts_with_all = ["sound_type", "drone"])]
    health: bool,

    /// Sugar: equivalent to --sound-type drone.
    #[arg(long, conflicts_with_all = ["sound_type", "health"])]
    drone: bool,

    /// Drone register (low/mid/high). Only used with drone mode.
    #[arg(long, value_enum, default_value_t = DroneRegister::Mid)]
    register: DroneRegister,

    /// Playback duration in seconds for drone preview.
    #[arg(long, default_value_t = 5.0)]
    duration: f64,

    /// Positional values: 3 severities for health, 1 metric for drone.
    values: Vec<String>,
  },

  /// Display the machine's voice parameters.
  Voice {
    /// Preview another machine's voice by hostname.
    #[arg(long)]
    hostname: Option<String>,
  },

  /// Run as a long-lived daemon producing heartbeat audio.
  Daemon,
}

fn main() -> Result<(), ApplicationError> {
  let cli = Cli::parse();
  let config = Config::from_args(
    cli.log_level.as_deref(),
    cli.log_format.as_deref(),
    cli.config.as_deref(),
  )?;

  init_logging(config.log_level, config.log_format);

  match cli.command {
    Command::Preview {
      sound_type,
      health,
      drone,
      register,
      duration,
      values,
    } => {
      let effective = if drone {
        SoundType::Drone
      } else if health {
        SoundType::Health
      } else {
        sound_type
      };
      match effective {
        SoundType::Health => run_health_preview(&config, &values),
        SoundType::Drone => {
          run_drone_preview(&config, register, duration, &values)
        }
      }
    }
    Command::Voice { hostname } => {
      run_voice(hostname.as_deref());
      Ok(())
    }
    Command::Daemon => {
      let voice = config.voice();
      daemon::run_daemon(&config.daemon, &voice)?;
      Ok(())
    }
  }
}

fn run_health_preview(
  config: &Config,
  values: &[String],
) -> Result<(), ApplicationError> {
  if values.len() != 3 {
    return Err(ApplicationError::InvalidSeverity(format!(
      "expected exactly 3 severity values, got {}",
      values.len()
    )));
  }

  let parse_severity = |s: &str| -> Result<Severity, ApplicationError> {
    s.parse::<u8>()
      .map_err(|e| ApplicationError::InvalidSeverity(e.to_string()))
      .and_then(|v| {
        Severity::try_from(v)
          .map_err(|e| ApplicationError::InvalidSeverity(e.to_string()))
      })
  };

  let severities: [Severity; 3] = [
    parse_severity(&values[0])?,
    parse_severity(&values[1])?,
    parse_severity(&values[2])?,
  ];

  let voice = config.voice();
  info!(base_freq = voice.base_freq, "Playing heartbeat preview");

  let durations = heartbeat::boop_durations(&voice);
  let gap = Duration::from_millis(100);

  for (i, &severity) in severities.iter().enumerate() {
    let dur = durations[i];
    let graph = heartbeat::boop_graph(&voice, severity, dur);
    AudioOutput::play_for(graph, Duration::from_secs_f64(dur + 0.05))?;
    if i < 2 {
      std::thread::sleep(gap);
    }
  }

  Ok(())
}

fn run_drone_preview(
  config: &Config,
  register: DroneRegister,
  duration: f64,
  values: &[String],
) -> Result<(), ApplicationError> {
  if values.len() != 1 {
    return Err(ApplicationError::InvalidDroneMetric(format!(
      "expected exactly 1 metric value, got {}",
      values.len()
    )));
  }

  let metric: f64 =
    values[0].parse().map_err(|e: std::num::ParseFloatError| {
      ApplicationError::InvalidDroneMetric(e.to_string())
    })?;

  if !(0.0..=1.0).contains(&metric) {
    return Err(ApplicationError::InvalidDroneMetric(format!(
      "metric must be between 0.0 and 1.0, got {}",
      metric
    )));
  }

  let voice = config.voice();
  info!(
    base_freq = voice.base_freq,
    ?register,
    metric,
    duration,
    "Playing drone preview"
  );

  let metric_shared = shared(metric as f32);
  let graph = drone::drone_graph(&voice, register, &metric_shared);
  AudioOutput::play_for(graph, Duration::from_secs_f64(duration))?;

  Ok(())
}

fn run_voice(hostname: Option<&str>) {
  let voice = hostname
    .map(Voice::from_hostname)
    .unwrap_or_else(Voice::from_current_host);

  let label = hostname.unwrap_or("(current host)");
  println!("Voice for {}:\n{}", label, voice);
}
