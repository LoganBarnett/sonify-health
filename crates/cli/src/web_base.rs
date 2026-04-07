use crate::metrics::Metrics;
use crate::preview_state::PreviewState;
use crate::websocket;
use aide::{
  axum::{routing::get_with, ApiRouter},
  openapi::OpenApi,
  scalar::Scalar,
  transform::TransformOperation,
};
use axum::{
  extract::State,
  http::{header, HeaderValue, StatusCode},
  response::{IntoResponse, Response},
  routing::get,
  Json, Router,
};
use prometheus::{Encoder, TextEncoder};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;
use std::path::PathBuf;
use std::sync::{
  atomic::{AtomicBool, Ordering},
  Arc,
};
use tower::ServiceBuilder;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

// -- AppState ----------------------------------------------------------------

#[derive(Clone)]
pub struct AppState {
  pub metrics: Metrics,
  pub muted: Arc<AtomicBool>,
  pub frontend_path: PathBuf,
  pub preview: Arc<PreviewState>,
}

impl AppState {
  /// Construct `AppState` with pre-built metrics, shared mute flag,
  /// the path to the compiled frontend assets directory, and the
  /// preview state backing the real-time control surface.
  pub fn init(
    muted: Arc<AtomicBool>,
    metrics: Metrics,
    frontend_path: PathBuf,
    preview: Arc<PreviewState>,
  ) -> Self {
    Self {
      metrics,
      muted,
      frontend_path,
      preview,
    }
  }
}

// -- Health ------------------------------------------------------------------

#[derive(Serialize, JsonSchema)]
pub struct HealthResponse {
  status: String,
}

async fn healthz() -> Json<HealthResponse> {
  Json(HealthResponse {
    status: "healthy".to_string(),
  })
}

// -- Mute --------------------------------------------------------------------

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
  state.preview.update_all_combined_volumes();
  let _ = state
    .preview
    .broadcast_tx
    .send(json!({"type": "mute_changed", "muted": true}).to_string());
  Json(MuteResponse { muted: true })
}

async fn delete_mute(State(state): State<AppState>) -> Json<MuteResponse> {
  state.muted.store(false, Ordering::Relaxed);
  state.metrics.muted.set(0);
  state.preview.update_all_combined_volumes();
  let _ = state
    .preview
    .broadcast_tx
    .send(json!({"type": "mute_changed", "muted": false}).to_string());
  Json(MuteResponse { muted: false })
}

// -- Metrics -----------------------------------------------------------------

async fn metrics_endpoint(State(state): State<AppState>) -> Response {
  let encoder = TextEncoder::new();
  let metric_families = state.metrics.registry.gather();
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

// -- Router ------------------------------------------------------------------

pub fn base_router(state: AppState) -> Router {
  aide::generate::extract_schemas(true);
  let mut api = OpenApi::default();

  let frontend_path = state.frontend_path.clone();

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
    .with_state(state.clone())
    .finish_api_with(&mut api, |a| a.title("sonify-health"));

  let api = Arc::new(api);

  // Serve static frontend assets with SPA fallback to index.html.
  // Cache-Control: no-store ensures the browser always fetches fresh
  // assets; cache-busting is handled via Nix store hash in the URL.
  let index = frontend_path.join("index.html");
  let spa_fallback = ServiceBuilder::new()
    .layer(SetResponseHeaderLayer::overriding(
      header::CACHE_CONTROL,
      HeaderValue::from_static("no-store"),
    ))
    .service(ServeDir::new(&frontend_path).fallback(ServeFile::new(index)));

  Router::new()
    .merge(app_router)
    .route("/ws", get(websocket::ws_handler).with_state(state))
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
    .fallback_service(spa_fallback)
}
