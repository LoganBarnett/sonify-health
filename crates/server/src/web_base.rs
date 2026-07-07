//! Application state for the daemon, plus the mute REST API.
//!
//! `AppState` wraps foundation's `BaseServerState` (health registry,
//! metrics registry, OIDC client) with the
//! sonify-specific runtime state the WebSocket handler and the mute
//! API need.  `impl_server_state!` generates the `FromRef` impls
//! foundation's built-in handlers (health, metrics, auth, /me)
//! depend on.

use aide::{
  axum::{routing::get_with, ApiRouter},
  transform::TransformOperation,
};
use axum::{extract::State, Json};
use rust_template_foundation::{
  impl_server_state, server::runner::BaseServerState,
};
use schemars::JsonSchema;
use serde::Serialize;
use std::sync::{
  atomic::{AtomicBool, Ordering},
  Arc,
};

use crate::metrics::Metrics;
use crate::preview_state::PreviewState;

#[derive(Clone)]
pub struct AppState {
  pub base: BaseServerState,
  pub preview: Arc<PreviewState>,
  pub muted: Arc<AtomicBool>,
  pub metrics: Metrics,
}

impl_server_state!(AppState, base);

// ── /api/mute ──────────────────────────────────────────────────────

#[derive(Serialize, JsonSchema)]
pub struct MuteResponse {
  muted: bool,
}

async fn get_mute(State(state): State<AppState>) -> Json<MuteResponse> {
  Json(MuteResponse {
    muted: state.muted.load(Ordering::Relaxed),
  })
}

async fn put_mute(State(state): State<AppState>) -> Json<MuteResponse> {
  state.muted.store(true, Ordering::Relaxed);
  state.metrics.muted.set(1);
  state.preview.update_all_effective_volumes();
  Json(MuteResponse { muted: true })
}

async fn delete_mute(State(state): State<AppState>) -> Json<MuteResponse> {
  state.muted.store(false, Ordering::Relaxed);
  state.metrics.muted.set(0);
  state.preview.update_all_effective_volumes();
  Json(MuteResponse { muted: false })
}

/// Build the sonify-specific API surface (currently the mute
/// endpoint).  Merged into the foundation `Server` via
/// `Server::merge`.  The WebSocket route is added separately because
/// it is not an `aide`-documented `ApiRouter` (axum's WS types don't
/// derive `OperationOutput`).
pub fn mute_api() -> ApiRouter<AppState> {
  ApiRouter::new().api_route(
    "/api/mute",
    get_with(get_mute, |op: TransformOperation| {
      op.description("Current mute state.")
    })
    .put_with(put_mute, |op: TransformOperation| {
      op.description("Mute audio output.")
    })
    .delete_with(delete_mute, |op: TransformOperation| {
      op.description("Unmute audio output.")
    }),
  )
}
