//! Shared test helpers for the server crate's integration tests.
//!
//! Lives at `tests/common/mod.rs` so each integration test can
//! `mod common;` it.  Helpers are exempted from the unwrap/expect
//! bans because they sit outside `#[test]` functions which clippy's
//! `allow-*-in-tests` doesn't reach.  `dead_code` is also waived:
//! rust treats `mod common;` as a fresh compilation per integration
//! test, so any helper not used by one test reads as dead for that
//! test's view of the module.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, dead_code)]

use openidconnect::core::CoreClient;
use prometheus::{IntCounterVec, Opts, Registry};
use rust_template_foundation::server::health::HealthRegistry;
use rust_template_foundation::server::runner::BaseServerState;
use sonify_health_server::metrics::Metrics;
use sonify_health_server::preview_state::PreviewState;
use sonify_health_server::web_base::AppState;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Construct a fresh `Metrics` on a private registry for use in
/// tests that don't share the registry with anything else.
pub fn test_metrics() -> Metrics {
  let registry = Registry::new();
  Metrics::new(&registry).unwrap()
}

/// Construct a `BaseServerState` with empty registries and no OIDC
/// for use in integration tests.  Mirrors what foundation builds in
/// `BaseServerState::init` but skips the async OIDC discovery
/// (tests don't run an OIDC provider).
pub fn stub_base() -> BaseServerState {
  let registry = Registry::new();
  let request_counter = IntCounterVec::new(
    Opts::new("http_requests_total", "test"),
    &["method", "status"],
  )
  .unwrap();
  registry
    .register(Box::new(request_counter.clone()))
    .unwrap();
  BaseServerState {
    health_registry: HealthRegistry::default(),
    metrics_registry: Arc::new(registry),
    request_counter,
    oidc_client: None::<Arc<CoreClient>>,
    frontend_path: None,
  }
}

/// Bundle the sonify runtime state into an `AppState` over a stub
/// `BaseServerState`.
pub fn test_app_state(
  preview: Arc<PreviewState>,
  muted: Arc<AtomicBool>,
  metrics: Metrics,
) -> AppState {
  AppState {
    base: stub_base(),
    preview,
    muted,
    metrics,
  }
}

/// Build a minimal `axum::Router` with just the WebSocket route, no
/// auth middleware or SPA fallback.  Mirrors what the previous
/// `cli::web_base::test_router` used to do.
pub fn test_router(state: AppState) -> axum::Router {
  use axum::routing::get;
  axum::Router::new()
    .route("/ws", get(sonify_health_server::websocket::ws_handler))
    .with_state(state)
}
