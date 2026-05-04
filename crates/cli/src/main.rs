mod logging;
mod patch_args;
mod print;
mod systemd;

use sonify_health_cli::{
  auth, config, daemon, metrics, preview_state, web_base,
};

use axum::{middleware, routing::get, Router};
use clap::{Parser, Subcommand, ValueEnum};
use config::{Config, ConfigError};
use daemon::DaemonError;
use logging::init_logging;
use patch_args::CliPatchOverrides;
use sonify_health_lib::{
  audio::{AudioError, AudioMixer, AudioOutput},
  heartbeat, ResolvedNote,
};
use std::sync::{
  atomic::{AtomicBool, Ordering},
  Arc,
};
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

  #[error("Unknown patch name: {0}")]
  UnknownPatch(String),

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

  /// Path to a TOML file of patch definitions.  May be repeated;
  /// last-in wins for overlapping patch names.  The main config
  /// file always wins over CLI-supplied patch libraries.
  #[arg(long)]
  patch_library: Vec<std::path::PathBuf>,

  /// Base URL of this service (e.g. https://sonify.example.com),
  /// used to construct the OIDC redirect URI.
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

  /// Run without opening an audio device.  Polls heartbeats and
  /// serves state over the WebSocket as usual, but spawns no play
  /// threads.  Intended for speakerless servers whose state will
  /// be rendered by another instance subscribed to this one.
  #[arg(long, env = "HEADLESS")]
  headless: bool,

  #[command(subcommand)]
  command: Command,
}

#[derive(Clone, Debug, ValueEnum)]
enum PrintFormat {
  Toml,
  Nix,
  Cli,
}

#[derive(Subcommand)]
enum Command {
  /// Preview a named patch from the library.
  Preview {
    /// Play continuously until interrupted (Ctrl-C).
    #[arg(long)]
    continuous: bool,

    #[command(flatten)]
    patch: CliPatchOverrides,
  },

  /// Print the patch library in a paste-ready format (TOML, Nix, or
  /// CLI flags).
  Print {
    /// Output format.
    #[arg(long, value_enum, default_value_t = PrintFormat::Toml)]
    format: PrintFormat,

    #[command(flatten)]
    patch: CliPatchOverrides,
  },

  /// Run as a long-lived daemon producing heartbeat audio.
  Daemon,
}

#[tokio::main]
async fn main() -> Result<(), ApplicationError> {
  let cli = Cli::parse();
  // Only forward the CLI flag when explicitly set; clap's `bool`
  // can't distinguish "absent" from "false", so we use `Some(true)`
  // when present and let the config file or default decide otherwise.
  let cli_headless = if cli.headless { Some(true) } else { None };
  let config = Config::from_args(
    cli.log_level.as_deref(),
    cli.log_format.as_deref(),
    cli.listen.as_deref(),
    cli.frontend_path.as_deref(),
    cli.config.as_deref(),
    &cli.patch_library,
    cli.base_url.as_deref(),
    cli.oidc_issuer.as_deref(),
    cli.oidc_client_id.as_deref(),
    cli.oidc_client_secret_file.as_deref(),
    cli_headless,
  )?;

  init_logging(config.log_level, config.log_format);

  debug!(
    log_level = ?config.log_level,
    log_format = ?config.log_format,
    listen = %config.listen_address,
    audio_device = ?config.audio_device,
    headless = config.headless,
    frontend_path = ?config.frontend_path,
    "Resolved configuration"
  );

  match cli.command {
    Command::Preview { continuous, patch } => {
      run_preview(&config, &patch, continuous)
    }
    Command::Print { format, patch } => {
      run_print(&config, &patch, format);
      Ok(())
    }
    Command::Daemon => run_daemon(&config).await,
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
  .expect("Failed to install Ctrl-C handler");

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

// -- Daemon ------------------------------------------------------------------

async fn run_daemon(config: &Config) -> Result<(), ApplicationError> {
  let muted = Arc::new(AtomicBool::new(false));
  let running = Arc::new(AtomicBool::new(true));
  let metrics = metrics::Metrics::new();

  let config_writable = config.config_path.as_ref().map_or(false, |p| {
    std::fs::metadata(p).map_or(false, |m| !m.permissions().readonly())
  });

  let preview = Arc::new(preview_state::PreviewState::new(
    config.library.clone(),
    config.overrides.clone(),
    config.heartbeats.clone(),
    Arc::clone(&muted),
    Arc::clone(&running),
    metrics.clone(),
    config.slider_ranges.clone(),
    config.config_path.clone(),
    config_writable,
    config.headless,
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
      // (client_secret_post).  Some providers (e.g. Authelia)
      // require this instead of the HTTP Basic Auth default.
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

  // Spawn one outbound WebSocket connector per Remote Source.
  // Today no path constructs Remote Sources at startup (config
  // plumbing lands in step 5), so this loop is a no-op for the
  // default config — but the integration is wired so a Remote
  // Source added via test fixtures or future config code starts
  // mirroring as soon as the runtime is up.
  for (idx, source) in preview.sources.iter().enumerate() {
    if matches!(source.kind, preview_state::SourceKind::Remote { .. }) {
      let connector_preview = Arc::clone(&preview);
      tokio::spawn(async move {
        sonify_health_cli::remote_source::run_connector(connector_preview, idx)
          .await;
      });
    }
  }

  // Spawn the blocking daemon loop in a separate thread.
  let audio_device = config.audio_device.clone();
  let daemon_preview = Arc::clone(&preview);
  let daemon_headless = config.headless;
  let daemon_handle = tokio::task::spawn_blocking(move || {
    daemon::run_daemon(daemon::DaemonContext {
      audio_device: audio_device.as_deref(),
      headless: daemon_headless,
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
  // SameSite::Lax is required: Strict suppresses the session cookie
  // on the cross-site redirect back from the OIDC provider.
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
