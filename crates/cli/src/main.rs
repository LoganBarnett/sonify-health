mod logging;
mod print;
mod systemd;

use sonify_health_cli::{
  auth, command, config, daemon, metrics, patch_args, preview_state, web_base,
};

use axum::{middleware, routing::get, Router};
use clap::Parser;
use command::{Command, PrintFormat};
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

/// CLI args parser — alias for the macro-generated `CliRaw`.  The
/// `Config::from_cli_and_file` entry point consumes one of these.
type Cli = <Config as rust_template_foundation::CliApp>::CliArgs;

#[derive(Debug, Error)]
enum ApplicationError {
  // ConfigError carries large embedded errors (toml::de::Error,
  // serde_json::Error) so it pushes the enum size into clippy's
  // result_large_err territory.  Box the inner error to keep
  // the discriminant-plus-pointer small.  The manual From impl
  // below preserves `?`-conversion ergonomics — callers still
  // write `cfg_load()?` against `Result<T, ConfigError>`.
  #[error("Failed to load configuration: {0}")]
  ConfigurationLoad(#[source] Box<ConfigError>),

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

  #[error("Failed to initialize metrics: {0}")]
  MetricsInit(#[from] metrics::MetricsInitError),

  /// Failed to install a synchronous signal handler at startup
  /// (ctrlc backend).  The `signal` field carries the human
  /// signal name so logs identify which install tripped.
  #[error("Failed to install {signal} handler: {source}")]
  SignalHandlerInstallFailed {
    signal: &'static str,
    #[source]
    source: ctrlc::Error,
  },

  /// The daemon `spawn_blocking` task itself panicked.  Carries
  /// the `JoinError` so the panic payload can be inspected /
  /// re-raised by the caller if desired.  See
  /// `std::panic::resume_unwind` for the explicit re-raise path.
  #[error("Daemon task panicked or was cancelled: {0}")]
  DaemonTaskJoin(#[source] tokio::task::JoinError),
}

/// Manual `From<ConfigError>` so `?` keeps converting cleanly
/// even though the variant boxes the inner error to satisfy
/// `clippy::result_large_err`.
impl From<ConfigError> for ApplicationError {
  fn from(e: ConfigError) -> Self {
    Self::ConfigurationLoad(Box::new(e))
  }
}

#[tokio::main]
async fn main() -> Result<(), ApplicationError> {
  // rustls 0.23 requires a process-level CryptoProvider to be
  // selected before the first TLS handshake; without one the
  // first outbound `wss://` connection panics with "Could not
  // automatically determine the process-level CryptoProvider".
  // We register ring (already in the dep tree via reqwest's
  // rustls 0.21 backend).  `install_default` returns Err if a
  // provider is already registered — benign, since whichever
  // provider is in place will satisfy the panic check, so we
  // log and carry on rather than abort.
  if rustls::crypto::ring::default_provider()
    .install_default()
    .is_err()
  {
    tracing::debug!(
      "rustls crypto provider already installed; using the existing one"
    );
  }

  let cli = Cli::parse();
  let config = Config::from_cli_and_file(cli)?;

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

  match &config.command {
    Command::Preview { continuous, patch } => {
      run_preview(&config, patch, *continuous)
    }
    Command::Print { format, patch } => {
      run_print(&config, patch, format.clone());
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

// -- Daemon ------------------------------------------------------------------

async fn run_daemon(config: &Config) -> Result<(), ApplicationError> {
  let muted = Arc::new(AtomicBool::new(false));
  let running = Arc::new(AtomicBool::new(true));
  let metrics = metrics::Metrics::new()?;

  let config_writable = config.config_path_resolved.as_ref().is_some_and(|p| {
    std::fs::metadata(p).is_ok_and(|m| !m.permissions().readonly())
  });

  let preview = Arc::new(preview_state::PreviewState::new(
    config.library.clone(),
    config.overrides.clone(),
    config.heartbeats.clone(),
    Arc::clone(&muted),
    Arc::clone(&running),
    metrics.clone(),
    config.slider_ranges.clone(),
    config.config_path_resolved.clone(),
    config_writable,
    config.headless,
  ));
  // Declare each Remote Source up-front so the connector spawn
  // loop below picks them up, and apply the user's
  // playback_enabled choice from the config / CLI.
  for rs in &config.remote_sources {
    preview
      .add_remote_source(rs.name.clone(), rs.url.clone())
      .map_err(|e| {
        ApplicationError::from(ConfigError::Validation(format!("{e}")))
      })?;
    if let Some(source) = preview.source_by_name(&rs.name) {
      if let preview_state::SourceKind::Remote {
        playback_enabled, ..
      } = &source.kind
      {
        playback_enabled.store(rs.playback_enabled, Ordering::Relaxed);
      }
    }
  }

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
  // Iterates the snapshot of names so the connector tasks identify
  // their target by name (stable across runtime add/remove) rather
  // than by index.
  for source in preview.sources_snapshot() {
    if source.kind.is_remote() {
      let connector_preview = Arc::clone(&preview);
      let name = source.name.clone();
      tokio::spawn(async move {
        sonify_health_cli::remote_source::run_connector(
          connector_preview,
          name,
        )
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
        .map_err(ApplicationError::DaemonTaskJoin)?
        .map_err(ApplicationError::Daemon)?;
    }
    result = &mut daemon_handle => {
      running.store(false, Ordering::Relaxed);
      result
        .map_err(ApplicationError::DaemonTaskJoin)?
        .map_err(ApplicationError::Daemon)?;
    }
  }

  info!("Shutdown complete");
  Ok(())
}

/// axum's `with_graceful_shutdown` requires a future producing
/// `()`, so we can't propagate signal-install errors out of this
/// function.  When a subscription fails we log the error and fall
/// back to `pending::<()>()` for that branch — the binary loses
/// graceful-shutdown coverage for the signal that failed but
/// continues to run, with the failure visible in the log.
async fn shutdown_signal(running: Arc<AtomicBool>) {
  let ctrl_c = async {
    match signal::ctrl_c().await {
      Ok(()) => {}
      Err(e) => {
        tracing::error!(
          error = %e,
          "Failed to subscribe to Ctrl+C; \
           graceful shutdown via Ctrl+C disabled",
        );
        std::future::pending::<()>().await
      }
    }
  };

  #[cfg(unix)]
  let terminate = async {
    match signal::unix::signal(signal::unix::SignalKind::terminate()) {
      Ok(mut s) => {
        s.recv().await;
      }
      Err(e) => {
        tracing::error!(
          error = %e,
          "Failed to subscribe to SIGTERM; \
           graceful shutdown via SIGTERM disabled",
        );
        std::future::pending::<()>().await
      }
    }
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
