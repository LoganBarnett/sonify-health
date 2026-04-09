mod auth;
mod config;
mod daemon;
mod logging;
mod metrics;
mod patch_args;
mod preview_state;
mod print;
mod systemd;
mod web_base;
mod websocket;

use axum::{middleware, routing::get, Router};
use clap::{Parser, Subcommand, ValueEnum};
use config::{Config, ConfigError};
use daemon::DaemonError;
use logging::init_logging;
use patch_args::CliPatchOverrides;
use sonify_health_lib::{
  audio::{AudioError, AudioOutput},
  drone, heartbeat, Patch, Severity,
};
use std::sync::{
  atomic::{AtomicBool, Ordering},
  Arc,
};
use std::time::Duration;
use thiserror::Error;
use tokio::signal;
use tower_http::trace::TraceLayer;
use tower_sessions::{cookie::SameSite, MemoryStore, SessionManagerLayer};
use tracing::{debug, info};
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

  #[error("Invalid OIDC issuer URL: {0}")]
  OidcInvalidIssuer(String),

  #[error("OIDC provider discovery failed: {0}")]
  OidcDiscovery(String),

  #[error("Invalid OIDC redirect URI: {0}")]
  OidcInvalidRedirectUri(String),

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

  #[arg(long, env = "FRONTEND_PATH")]
  frontend_path: Option<std::path::PathBuf>,

  #[arg(short, long, env = "CONFIG_FILE")]
  config: Option<std::path::PathBuf>,

  /// Base URL of this service (e.g. https://sonify.example.com), used
  /// to construct the OIDC redirect URI.
  #[arg(long, env = "BASE_URL")]
  base_url: Option<String>,

  /// OIDC issuer URL for provider discovery.
  #[arg(long, env = "OIDC_ISSUER")]
  oidc_issuer: Option<String>,

  /// OIDC client ID.
  #[arg(long, env = "OIDC_CLIENT_ID")]
  oidc_client_id: Option<String>,

  /// Path to a file containing the OIDC client secret.
  #[arg(long, env = "OIDC_CLIENT_SECRET_FILE")]
  oidc_client_secret_file: Option<std::path::PathBuf>,

  #[command(subcommand)]
  command: Command,
}

#[derive(Clone, Debug, ValueEnum)]
enum SoundType {
  Heartbeat,
  Drone,
}

#[derive(Clone, Debug, ValueEnum)]
enum PrintFormat {
  Toml,
  Nix,
  Cli,
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

    /// Number of boops per drone phrase.
    #[arg(long, default_value_t = 1)]
    boops: usize,

    /// Playback duration in seconds for drone preview.
    #[arg(long, default_value_t = 5.0)]
    duration: f64,

    /// Play continuously until interrupted (Ctrl-C).  Overrides
    /// --duration for drone previews.
    #[arg(long)]
    continuous: bool,

    #[command(flatten)]
    patch: CliPatchOverrides,

    /// Positional values: 1 or more severities for heartbeat,
    /// 1 metric for drone.
    values: Vec<String>,
  },

  /// Print the fully-resolved patch configuration in a paste-ready
  /// format (TOML, Nix, or CLI flags).
  Print {
    /// Output format.
    #[arg(long, value_enum, default_value_t = PrintFormat::Toml)]
    format: PrintFormat,

    #[command(flatten)]
    patch: CliPatchOverrides,
  },

  /// Display the machine's patch parameters.
  Patch {
    /// Preview another machine's patch by hostname.
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
    cli.frontend_path.as_deref(),
    cli.config.as_deref(),
    cli.base_url.as_deref(),
    cli.oidc_issuer.as_deref(),
    cli.oidc_client_id.as_deref(),
    cli.oidc_client_secret_file.as_deref(),
  )?;

  init_logging(config.log_level, config.log_format);

  debug!(
    log_level = ?config.log_level,
    log_format = ?config.log_format,
    listen = %config.listen_address,
    audio_device = ?config.audio_device,
    frontend_path = ?config.frontend_path,
    "Resolved configuration"
  );

  match cli.command {
    Command::Preview {
      sound_type,
      heartbeat,
      drone,
      boops,
      duration,
      continuous,
      patch,
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
        SoundType::Heartbeat => run_heartbeat_preview(&config, &patch, &values),
        SoundType::Drone => run_drone_preview(
          &config, &patch, boops, duration, continuous, &values,
        ),
      }
    }
    Command::Print { format, patch } => {
      run_print(&config, &patch, format);
      Ok(())
    }
    Command::Patch { hostname } => {
      run_patch(hostname.as_deref());
      Ok(())
    }
    Command::Daemon => run_daemon(&config).await,
  }
}

async fn run_daemon(config: &Config) -> Result<(), ApplicationError> {
  let muted = Arc::new(AtomicBool::new(false));
  let running = Arc::new(AtomicBool::new(true));
  let metrics = metrics::Metrics::new();

  let mut patch = config.patch();
  if let Some(hb_overrides) = &config.daemon.heartbeat_patch_overrides {
    patch = patch.with_overrides(hb_overrides);
  }

  let preview = Arc::new(preview_state::PreviewState::new(
    patch.clone(),
    Arc::clone(&muted),
    &config.daemon.heartbeat_checks,
    &config.daemon.drone_metrics,
    &config.daemon.drone_profile_overrides,
    config.daemon.timing.slot_duration_secs,
    &config.daemon.heartbeat_notes,
    &config.daemon.drone_notes,
  ));

  // Perform OIDC provider discovery when configured.
  let oidc_client = match &config.oidc {
    Some(oidc) => {
      let issuer = openidconnect::IssuerUrl::new(oidc.issuer.clone())
        .map_err(|e| ApplicationError::OidcInvalidIssuer(e.to_string()))?;

      let provider = openidconnect::core::CoreProviderMetadata::discover_async(
        issuer,
        openidconnect::reqwest::async_http_client,
      )
      .await
      .map_err(|e| ApplicationError::OidcDiscovery(format!("{e:?}")))?;

      info!(issuer = %oidc.issuer, "OIDC provider discovery complete");

      let redirect = openidconnect::RedirectUrl::new(format!(
        "{}/auth/callback",
        oidc.base_url.trim_end_matches('/')
      ))
      .map_err(|e| ApplicationError::OidcInvalidRedirectUri(e.to_string()))?;

      // RequestBody sends client credentials in the POST body
      // (client_secret_post).  Some providers (e.g. Authelia) require
      // this instead of the HTTP Basic Auth default.
      let client = openidconnect::core::CoreClient::from_provider_metadata(
        provider,
        openidconnect::ClientId::new(oidc.client_id.clone()),
        Some(openidconnect::ClientSecret::new(oidc.client_secret.clone())),
      )
      .set_redirect_uri(redirect)
      .set_auth_type(openidconnect::AuthType::RequestBody);

      Some(Arc::new(client))
    }
    None => None,
  };

  let state = AppState::init(
    Arc::clone(&muted),
    metrics.clone(),
    config.frontend_path.clone(),
    Arc::clone(&preview),
    oidc_client,
  );
  let app = create_app(state, config);

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
  let audio_device = config.audio_device.clone();
  let daemon_muted = Arc::clone(&muted);
  let daemon_running = Arc::clone(&running);
  let daemon_preview = Arc::clone(&preview);
  let daemon_handle = tokio::task::spawn_blocking(move || {
    daemon::run_daemon(daemon::DaemonContext {
      config: &daemon_config,
      audio_device: audio_device.as_deref(),
      muted: daemon_muted,
      running: daemon_running,
      metrics,
      preview: daemon_preview,
    })
  });

  let running_signal = Arc::clone(&running);
  let server = axum::serve(listener, app.into_make_service())
    .with_graceful_shutdown(shutdown_signal(running_signal));

  // Race the web server against the daemon.  If the daemon exits
  // early (e.g., audio device failure), shut the program down
  // immediately rather than hanging with a headless web server.
  let mut daemon_handle = daemon_handle;
  tokio::select! {
    result = server => {
      result.map_err(ApplicationError::ServerRuntime)?;
      info!("Web server shut down, waiting for daemon loop");
      daemon_handle
        .await
        .expect("Daemon task panicked")
        .map_err(ApplicationError::Daemon)?;
    }
    result = &mut daemon_handle => {
      running.store(false, Ordering::Relaxed);
      result
        .expect("Daemon task panicked")
        .map_err(ApplicationError::Daemon)?;
    }
  }

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

fn create_app(state: AppState, config: &Config) -> Router {
  let session_store = MemoryStore::default();
  let secure = config
    .oidc
    .as_ref()
    .is_some_and(|o| o.base_url.starts_with("https://"));
  // SameSite::Lax is required: Strict suppresses the session cookie on
  // the cross-site redirect back from the OIDC provider.
  let session_layer = SessionManagerLayer::new(session_store)
    .with_secure(secure)
    .with_same_site(SameSite::Lax);

  let protected = web_base::protected_router(state.clone()).route_layer(
    middleware::from_fn_with_state(state.clone(), auth::require_auth),
  );

  let public = web_base::public_router(state.clone());

  let mut app = Router::new().merge(protected).merge(public);

  // Only expose auth routes when OIDC is configured.
  if state.oidc_client.is_some() {
    let auth_router = Router::new()
      .route("/auth/login", get(auth::login_handler))
      .route("/auth/callback", get(auth::callback_handler))
      .route("/auth/logout", get(auth::logout_handler))
      .with_state(state);
    app = app.merge(auth_router);
  }

  app.layer(session_layer).layer(TraceLayer::new_for_http())
}

fn run_heartbeat_preview(
  config: &Config,
  patch_args: &CliPatchOverrides,
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

  let patch = patch_args.resolve_patch(config);
  debug!(?patch, "Resolved patch");
  let slot_secs = config.daemon.timing.slot_duration_secs;
  let patches = patch.heartbeat_notes(severities.len(), 1, slot_secs);
  for (i, p) in patches.iter().enumerate() {
    debug!(
      boop = i,
      freq = format_args!("{:.1} Hz", p.base_freq),
      duration = format_args!("{:.3}s", p.duration),
      severity = %severities[i],
      "Note spec"
    );
  }
  info!(
    base_freq = patch.base_freq,
    boops = severities.len(),
    "Playing heartbeat preview"
  );

  let graph = heartbeat::heartbeat_graph(&patches, &severities);
  AudioOutput::play_for(
    graph,
    heartbeat::heartbeat_duration(&patches),
    config.audio_device.as_deref(),
  )
  .map_err(ApplicationError::AudioPlayback)
}

fn run_drone_preview(
  config: &Config,
  patch_args: &CliPatchOverrides,
  boops: usize,
  duration: f64,
  continuous: bool,
  values: &[String],
) -> Result<(), ApplicationError> {
  use sonify_health_lib::audio::AudioMixer;

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

  let patch = patch_args.resolve_patch(config);
  let slot_secs = config.daemon.timing.slot_duration_secs;
  debug!(?patch, "Resolved patch");
  info!(
    base_freq = patch.base_freq,
    boops, metric, duration, "Playing drone preview"
  );

  let mixer = AudioMixer::new(config.audio_device.as_deref())?;

  if continuous {
    let run = Arc::new(AtomicBool::new(true));
    let (tx, rx) = std::sync::mpsc::channel();
    ctrlc::set_handler(move || {
      let _ = tx.send(());
    })
    .expect("Failed to install Ctrl-C handler");

    info!("Playing continuously, press Ctrl-C to stop");
    let play_run = Arc::clone(&run);
    let handle = mixer.handle();
    let play_handle = std::thread::spawn(move || {
      while play_run.load(Ordering::Relaxed) {
        let patches = patch.drone_notes(0, boops, slot_secs);
        let severities: Vec<Severity> =
          (0..patches.len()).map(|_| Severity::Healthy).collect();
        let graph = heartbeat::heartbeat_graph(&patches, &severities);
        let phrase_dur = heartbeat::heartbeat_duration(&patches);
        let slot = handle.add(graph);
        std::thread::sleep(phrase_dur);
        handle.remove(slot);

        if !play_run.load(Ordering::Relaxed) {
          break;
        }

        let gap = drone::phrase_gap_secs(4.0, metric as f32, 1.0, 1.0);
        let deadline = std::time::Instant::now() + Duration::from_secs_f64(gap);
        while std::time::Instant::now() < deadline
          && play_run.load(Ordering::Relaxed)
        {
          std::thread::sleep(Duration::from_millis(100));
        }
      }
    });

    rx.recv().ok();
    run.store(false, Ordering::Relaxed);
    let _ = play_handle.join();
  } else {
    let deadline =
      std::time::Instant::now() + Duration::from_secs_f64(duration);
    while std::time::Instant::now() < deadline {
      let patches = patch.drone_notes(0, boops, slot_secs);
      let severities: Vec<Severity> =
        (0..patches.len()).map(|_| Severity::Healthy).collect();
      let graph = heartbeat::heartbeat_graph(&patches, &severities);
      let phrase_dur = heartbeat::heartbeat_duration(&patches);
      let slot = mixer.add(graph);
      std::thread::sleep(phrase_dur);
      mixer.remove(slot);

      if std::time::Instant::now() >= deadline {
        break;
      }

      let gap = drone::phrase_gap_secs(4.0, metric as f32, 1.0, 1.0);
      let gap_deadline =
        std::time::Instant::now() + Duration::from_secs_f64(gap);
      while std::time::Instant::now() < gap_deadline
        && std::time::Instant::now() < deadline
      {
        std::thread::sleep(Duration::from_millis(100));
      }
    }
  }

  Ok(())
}

fn run_print(
  config: &Config,
  patch_args: &CliPatchOverrides,
  format: PrintFormat,
) {
  let patch = patch_args.resolve_patch(config);
  let output = match format {
    PrintFormat::Toml => print::format_toml(&patch, &[], &[], &[]),
    PrintFormat::Nix => print::format_nix(&patch, &[], &[], &[]),
    PrintFormat::Cli => print::format_cli(&patch),
  };
  println!("{output}");
}

fn run_patch(hostname: Option<&str>) {
  let hn = hostname.map(String::from).unwrap_or_else(|| {
    gethostname::gethostname().to_string_lossy().to_string()
  });
  let patch = Patch::from_hostname(&hn);

  debug!(
    hostname = %hn,
    note_seed = patch.note_seed,
    "Patch seed derivation"
  );

  let label = hostname.unwrap_or("(current host)");
  println!("Patch for {}:\n{}", label, patch);
}
