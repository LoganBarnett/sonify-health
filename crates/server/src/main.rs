//! sonify-health-server — daemon entry point.
//!
//! `#[foundation_main]` generates the real `fn main()` with CLI
//! parsing, config resolution, logging init, tokio runtime, and
//! `BaseServerState::init` (OIDC discovery, metrics registry,
//! request counter, frontend path).  This file owns only the
//! application-specific glue: building `PreviewState`, registering
//! sonify's audio metrics against the foundation registry, spawning
//! the audio daemon thread + the remote-source connector tasks, and
//! racing `Server::listen()` against the daemon so either exiting
//! tears the other down.

use rust_template_foundation::main as foundation_main;
use rust_template_foundation::{Server, ServerError};
use sonify_health_lib::config::ConfigError as LibConfigError;
use sonify_health_server::config::{Config, ConfigError};
use sonify_health_server::daemon::{self, DaemonError};
use sonify_health_server::metrics::{self, Metrics};
use sonify_health_server::preview_state::{self, PreviewState};
use sonify_health_server::remote_source;
use sonify_health_server::web_base::{self, AppState};
use sonify_health_server::websocket;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use thiserror::Error;
use tracing::{error, info};

#[derive(Debug, Error)]
pub enum ApplicationError {
  // ConfigError carries large embedded errors (toml::de::Error,
  // serde_json::Error) so it pushes the enum size into clippy's
  // result_large_err territory.  Box the inner error to keep the
  // discriminant-plus-pointer small.
  #[error("Failed to load configuration: {0}")]
  ConfigurationLoad(#[source] Box<ConfigError>),

  #[error("Daemon failed: {0}")]
  Daemon(#[from] DaemonError),

  #[error("Server runtime: {0}")]
  Server(#[from] ServerError),

  #[error("Failed to initialize metrics: {0}")]
  MetricsInit(#[from] metrics::MetricsInitError),

  /// The daemon `spawn_blocking` task itself panicked.  Carries the
  /// `JoinError` so the panic payload can be inspected / re-raised
  /// by the caller if desired.
  #[error("Daemon task panicked or was cancelled: {0}")]
  DaemonTaskJoin(#[source] tokio::task::JoinError),
}

impl From<ConfigError> for ApplicationError {
  fn from(e: ConfigError) -> Self {
    Self::ConfigurationLoad(Box::new(e))
  }
}

#[foundation_main]
pub async fn main(
  config: Config,
  server: Server,
) -> Result<ExitCode, ApplicationError> {
  // rustls 0.23 requires a process-level CryptoProvider to be
  // selected before the first TLS handshake; without one the first
  // outbound `wss://` connection panics with "Could not
  // automatically determine the process-level CryptoProvider".  We
  // register ring (already in the dep tree via reqwest's rustls
  // backend).  `install_default` returns Err if a provider is
  // already registered — benign, since whichever provider is in
  // place will satisfy the panic check, so we log and carry on
  // rather than abort.
  if rustls::crypto::ring::default_provider()
    .install_default()
    .is_err()
  {
    tracing::debug!(
      "rustls crypto provider already installed; using the existing one"
    );
  }

  let muted = Arc::new(AtomicBool::new(false));
  let running = Arc::new(AtomicBool::new(true));

  // Sonify's audio/probe metrics register on foundation's
  // registry so a single `/metrics` endpoint exposes both
  // foundation infra metrics (`http_requests_total`, …) and
  // sonify's audio metrics.
  let base = server.base_state();
  let metrics = Metrics::new(&base.metrics_registry)?;

  let config_writable = config.config_path_resolved.as_ref().is_some_and(|p| {
    std::fs::metadata(p).is_ok_and(|m| !m.permissions().readonly())
  });

  let preview = Arc::new(PreviewState::new(
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

  // Declare each Remote Source up-front so the connector spawn loop
  // below picks them up, and apply the user's `playback_enabled`
  // choice from the config / CLI.
  for rs in &config.remote_sources {
    preview
      .add_remote_source(rs.name.clone(), rs.url.clone())
      .map_err(|e| {
        ApplicationError::from(ConfigError::Extra(LibConfigError::Validation(
          format!("{e}"),
        )))
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

  // Spawn one outbound WebSocket connector per Remote Source.
  for source in preview.sources_snapshot() {
    if source.kind.is_remote() {
      let connector_preview = Arc::clone(&preview);
      let name = source.name.clone();
      tokio::spawn(async move {
        remote_source::run_connector(connector_preview, name).await;
      });
    }
  }

  // Compose the server: swap in our AppState, merge sonify routes,
  // add the WebSocket route (which can't be an aide-documented
  // ApiRouter — axum's WS types don't implement OperationOutput).
  let app_state_preview = Arc::clone(&preview);
  let app_state_metrics = metrics.clone();
  let app_state_muted = Arc::clone(&muted);
  let ws_routes = aide::axum::ApiRouter::<AppState>::new()
    .route("/ws", axum::routing::get(websocket::ws_handler));
  let server = server
    .with_state(move |base| AppState {
      base,
      preview: app_state_preview,
      muted: app_state_muted,
      metrics: app_state_metrics,
    })
    .merge(web_base::mute_api())
    .merge(ws_routes);

  // Spawn the blocking daemon loop in a separate thread.
  let audio_device = config.audio_device.clone();
  let daemon_preview = Arc::clone(&preview);
  let daemon_headless = config.headless;
  let mut daemon_handle = tokio::task::spawn_blocking(move || {
    daemon::run_daemon(daemon::DaemonContext {
      audio_device: audio_device.as_deref(),
      headless: daemon_headless,
      preview: daemon_preview,
    })
  });

  // Race the server against the daemon: if either exits, the other
  // is torn down.  Dropping `server.listen()` on cancellation drops
  // the inner axum::serve future and its listener.  Flipping
  // `running` to false tells the daemon's main loop to exit
  // gracefully when the server-arm wins.
  tokio::select! {
    result = server.listen() => {
      running.store(false, Ordering::Relaxed);
      result?;
      info!("Web server shut down, waiting for daemon loop");
      daemon_handle
        .await
        .map_err(ApplicationError::DaemonTaskJoin)??;
    }
    result = &mut daemon_handle => {
      running.store(false, Ordering::Relaxed);
      result.map_err(ApplicationError::DaemonTaskJoin)??;
      error!("Daemon exited before web server; shutting down");
    }
  }

  Ok(ExitCode::SUCCESS)
}
