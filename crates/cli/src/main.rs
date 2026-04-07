mod config;
mod daemon;
mod logging;
mod metrics;
mod systemd;
mod web_base;

use clap::{Parser, Subcommand, ValueEnum};
use config::{Config, ConfigError};
use daemon::DaemonError;
use fundsp::prelude32::shared;
use logging::init_logging;
use sonify_health_lib::{
  audio::{AudioError, AudioOutput},
  drone, heartbeat, DroneRegister, Severity, Voice,
};
use std::sync::{
  atomic::{AtomicBool, Ordering},
  Arc,
};
use std::time::Duration;
use thiserror::Error;
use tokio::signal;
use tower_http::trace::TraceLayer;
use tracing::info;
use web_base::AppState;

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

  #[error("Failed to bind listener to {address}: {source}")]
  ListenerBind {
    address: String,
    source: std::io::Error,
  },

  #[error("Server encountered a runtime error: {0}")]
  ServerRuntime(#[source] std::io::Error),
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

  #[arg(long, env = "LISTEN")]
  listen: Option<String>,

  #[arg(short, long, env = "CONFIG_FILE")]
  config: Option<std::path::PathBuf>,

  #[command(subcommand)]
  command: Command,
}

#[derive(Clone, Debug, ValueEnum)]
enum SoundType {
  Heartbeat,
  Drone,
}

#[derive(Subcommand)]
enum Command {
  /// Preview a sound layer (heartbeat or drone).
  Preview {
    /// Sound type to preview.
    #[arg(long, value_enum, default_value_t = SoundType::Heartbeat)]
    sound_type: SoundType,

    /// Sugar: equivalent to --sound-type heartbeat.
    #[arg(long, conflicts_with_all = ["sound_type", "drone"])]
    heartbeat: bool,

    /// Sugar: equivalent to --sound-type drone.
    #[arg(long, conflicts_with_all = ["sound_type", "heartbeat"])]
    drone: bool,

    /// Drone register (low/mid/high). Only used with drone mode.
    #[arg(long, value_enum, default_value_t = DroneRegister::Mid)]
    register: DroneRegister,

    /// Playback duration in seconds for drone preview.
    #[arg(long, default_value_t = 5.0)]
    duration: f64,

    /// Positional values: 1 or more severities for heartbeat,
    /// 1 metric for drone.
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

#[tokio::main]
async fn main() -> Result<(), ApplicationError> {
  let cli = Cli::parse();
  let config = Config::from_args(
    cli.log_level.as_deref(),
    cli.log_format.as_deref(),
    cli.listen.as_deref(),
    cli.config.as_deref(),
  )?;

  init_logging(config.log_level, config.log_format);

  match cli.command {
    Command::Preview {
      sound_type,
      heartbeat,
      drone,
      register,
      duration,
      values,
    } => {
      let effective = if drone {
        SoundType::Drone
      } else if heartbeat {
        SoundType::Heartbeat
      } else {
        sound_type
      };
      match effective {
        SoundType::Heartbeat => run_heartbeat_preview(&config, &values),
        SoundType::Drone => {
          run_drone_preview(&config, register, duration, &values)
        }
      }
    }
    Command::Voice { hostname } => {
      run_voice(hostname.as_deref());
      Ok(())
    }
    Command::Daemon => run_daemon(&config).await,
  }
}

async fn run_daemon(config: &Config) -> Result<(), ApplicationError> {
  let muted = Arc::new(AtomicBool::new(false));
  let running = Arc::new(AtomicBool::new(true));
  let metrics = metrics::Metrics::new();

  let state = AppState::init(Arc::clone(&muted), metrics.clone());
  let app = web_base::base_router(state).layer(TraceLayer::new_for_http());

  info!("Binding to {}", config.listen_address);

  // Remove stale Unix socket file from a previous run so we can
  // re-bind.  This is safe because launchd/systemd guarantees only
  // one instance is running at a time.
  let addr_str = config.listen_address.to_string();
  if addr_str.starts_with('/') {
    let path = std::path::Path::new(&addr_str);
    if path.exists() {
      info!("Removing stale socket {}", addr_str);
      std::fs::remove_file(path).ok();
    }
  }

  let listener = tokio_listener::Listener::bind(
    &config.listen_address,
    &tokio_listener::SystemOptions::default(),
    &tokio_listener::UserOptions::default(),
  )
  .await
  .map_err(|source| ApplicationError::ListenerBind {
    address: config.listen_address.to_string(),
    source,
  })?;

  info!("Server listening on {}", config.listen_address);

  systemd::notify_ready();
  systemd::spawn_watchdog();

  // Spawn the blocking daemon loop in a separate thread.
  let daemon_config = config.daemon.clone();
  let voice = config.voice();
  let scale = config.scale();
  let audio_device = config.audio_device.clone();
  let daemon_muted = Arc::clone(&muted);
  let daemon_running = Arc::clone(&running);
  let daemon_handle = tokio::task::spawn_blocking(move || {
    daemon::run_daemon(
      &daemon_config,
      &voice,
      &scale,
      audio_device.as_deref(),
      daemon_muted,
      daemon_running,
      metrics,
    )
  });

  let running_signal = Arc::clone(&running);
  axum::serve(listener, app.into_make_service())
    .with_graceful_shutdown(shutdown_signal(running_signal))
    .await
    .map_err(ApplicationError::ServerRuntime)?;

  info!("Web server shut down, waiting for daemon loop");
  daemon_handle
    .await
    .expect("Daemon task panicked")
    .map_err(ApplicationError::Daemon)?;

  info!("Shutdown complete");
  Ok(())
}

async fn shutdown_signal(running: Arc<AtomicBool>) {
  let ctrl_c = async {
    signal::ctrl_c()
      .await
      .expect("Failed to install Ctrl+C handler");
  };

  #[cfg(unix)]
  let terminate = async {
    signal::unix::signal(signal::unix::SignalKind::terminate())
      .expect("Failed to install SIGTERM handler")
      .recv()
      .await;
  };

  #[cfg(not(unix))]
  let terminate = std::future::pending::<()>();

  tokio::select! {
    _ = ctrl_c => {
      info!("Received Ctrl+C, shutting down gracefully");
    },
    _ = terminate => {
      info!("Received SIGTERM, shutting down gracefully");
    },
  }

  running.store(false, Ordering::Relaxed);
}

fn run_heartbeat_preview(
  config: &Config,
  values: &[String],
) -> Result<(), ApplicationError> {
  if values.is_empty() {
    return Err(ApplicationError::InvalidSeverity(
      "expected 1 or more severity values, got 0".to_string(),
    ));
  }

  let parse_severity = |s: &str| -> Result<Severity, ApplicationError> {
    s.parse::<u8>()
      .map_err(|e| ApplicationError::InvalidSeverity(e.to_string()))
      .and_then(|v| {
        Severity::try_from(v)
          .map_err(|e| ApplicationError::InvalidSeverity(e.to_string()))
      })
  };

  let severities: Vec<Severity> = values
    .iter()
    .map(|v| parse_severity(v))
    .collect::<Result<_, _>>()?;

  let voice = config.voice();
  let scale = config.scale();
  let specs =
    voice.boop_specs(&scale, severities.len(), heartbeat::TOTAL_BOOP_TIME);
  info!(
    base_freq = voice.base_freq,
    boops = severities.len(),
    "Playing heartbeat preview"
  );

  let graph = heartbeat::heartbeat_graph(&voice, &severities, &specs);
  AudioOutput::play_for(
    graph,
    heartbeat::heartbeat_duration(&specs),
    config.audio_device.as_deref(),
  )
  .map_err(ApplicationError::AudioPlayback)
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
  AudioOutput::play_for(
    graph,
    Duration::from_secs_f64(duration),
    config.audio_device.as_deref(),
  )?;

  Ok(())
}

fn run_voice(hostname: Option<&str>) {
  let hn = hostname.map(String::from).unwrap_or_else(|| {
    gethostname::gethostname().to_string_lossy().to_string()
  });
  let voice = Voice::from_hostname(&hn)
    .with_scale(&sonify_health_lib::scale::domain_from_hostname(&hn));

  let label = hostname.unwrap_or("(current host)");
  println!("Voice for {}:\n{}", label, voice);
}
