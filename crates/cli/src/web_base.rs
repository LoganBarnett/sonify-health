use aide::{
  axum::{routing::get_with, ApiRouter},
  openapi::OpenApi,
  scalar::Scalar,
  transform::TransformOperation,
};
use axum::{
  extract::State,
  http::StatusCode,
  response::{IntoResponse, Response},
  routing::get,
  Json, Router,
};
use prometheus::{Encoder, IntCounter, Registry, TextEncoder};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;
use std::sync::{
  atomic::{AtomicBool, Ordering},
  Arc,
};

// ── AppState ──────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AppState {
  pub registry: Arc<Registry>,
  pub request_counter: IntCounter,
  pub muted: Arc<AtomicBool>,
}

impl AppState {
  /// Construct `AppState` with a Prometheus registry and shared mute flag.
  pub fn init(muted: Arc<AtomicBool>) -> Self {
    let registry = Registry::new();
    let request_counter =
      IntCounter::new("http_requests_total", "Total HTTP requests")
        .expect("Failed to create counter");
    registry
      .register(Box::new(request_counter.clone()))
      .expect("Failed to register counter");

    Self {
      registry: Arc::new(registry),
      request_counter,
      muted,
    }
  }
}

// ── Health ────────────────────────────────────────────────────────────────────

#[derive(Serialize, JsonSchema)]
pub struct HealthResponse {
  status: String,
}

async fn healthz() -> Json<HealthResponse> {
  Json(HealthResponse {
    status: "healthy".to_string(),
  })
}

// ── Mute ──────────────────────────────────────────────────────────────────────

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
  Json(MuteResponse { muted: true })
}

async fn delete_mute(State(state): State<AppState>) -> Json<MuteResponse> {
  state.muted.store(false, Ordering::Relaxed);
  Json(MuteResponse { muted: false })
}

// ── Metrics ───────────────────────────────────────────────────────────────────

async fn metrics_endpoint(State(state): State<AppState>) -> Response {
  let encoder = TextEncoder::new();
  let metric_families = state.registry.gather();
  let mut buffer = Vec::new();

  match encoder.encode(&metric_families, &mut buffer) {
    Ok(_) => {
      (StatusCode::OK, [("content-type", encoder.format_type())], buffer)
        .into_response()
    }
    Err(e) => (
      StatusCode::INTERNAL_SERVER_ERROR,
      Json(json!({
          "error": format!("Failed to encode metrics: {}", e)
      })),
    )
      .into_response(),
  }
}

// ── Router ────────────────────────────────────────────────────────────────────

pub fn base_router(state: AppState) -> Router {
  aide::generate::extract_schemas(true);
  let mut api = OpenApi::default();

  let app_router = ApiRouter::new()
    .api_route(
      "/healthz",
      get_with(healthz, |op: TransformOperation| {
        op.description("Health check.")
      }),
    )
    .api_route(
      "/metrics",
      get_with(metrics_endpoint, |op: TransformOperation| {
        op.description("Prometheus metrics in text/plain format.")
      }),
    )
    .api_route(
      "/api/mute",
      aide::axum::routing::get_with(get_mute, |op: TransformOperation| {
        op.description("Current mute state.")
      })
      .put_with(put_mute, |op: TransformOperation| {
        op.description("Mute audio output.")
      })
      .delete_with(delete_mute, |op: TransformOperation| {
        op.description("Unmute audio output.")
      }),
    )
    .with_state(state)
    .finish_api_with(&mut api, |a| a.title("sonify-health"));

  let api = Arc::new(api);

  Router::new()
    .merge(app_router)
    .route(
      "/api-docs/openapi.json",
      get({
        let api = api.clone();
        move || async move { Json((*api).clone()) }
      }),
    )
    .route(
      "/scalar",
      get(
        Scalar::new("/api-docs/openapi.json")
          .with_title("sonify-health")
          .axum_handler(),
      ),
    )
}
