use crate::auth;
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
use openidconnect::core::CoreClient;
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
  pub oidc_client: Option<Arc<CoreClient>>,
}

impl AppState {
  pub fn auth_enabled(&self) -> bool {
    self.oidc_client.is_some()
  }

  /// Construct `AppState` with pre-built metrics, shared mute flag,
  /// the path to the compiled frontend assets directory, the preview
  /// state backing the real-time control surface, and an optional
  /// OIDC client for authentication.
  pub fn init(
    muted: Arc<AtomicBool>,
    metrics: Metrics,
    frontend_path: PathBuf,
    preview: Arc<PreviewState>,
    oidc_client: Option<Arc<CoreClient>>,
  ) -> Self {
    Self {
      metrics,
      muted,
      frontend_path,
      preview,
      oidc_client,
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

// -- Me ----------------------------------------------------------------------

#[derive(Serialize, JsonSchema)]
pub struct MeResponse {
  name: String,
  auth_enabled: bool,
}

async fn me_handler(
  State(state): State<AppState>,
  session: tower_sessions::Session,
) -> Json<MeResponse> {
  if !state.auth_enabled() {
    return Json(MeResponse {
      name: "admin".to_string(),
      auth_enabled: false,
    });
  }

  let name = auth::current_user(&session)
    .await
    .map(|u| u.name)
    .unwrap_or_else(|| "anonymous".to_string());

  Json(MeResponse {
    name,
    auth_enabled: true,
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
  state.preview.update_all_effective_volumes();
  let _ = state
    .preview
    .broadcast_tx
    .send(json!({"type": "mute_changed", "muted": true}).to_string());
  Json(MuteResponse { muted: true })
}

async fn delete_mute(State(state): State<AppState>) -> Json<MuteResponse> {
  state.muted.store(false, Ordering::Relaxed);
  state.metrics.muted.set(0);
  state.preview.update_all_effective_volumes();
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

/// Routes that remain public regardless of OIDC configuration:
/// health check, Prometheus metrics, and the `/me` endpoint.
pub fn public_router(state: AppState) -> Router {
  aide::generate::extract_schemas(true);
  let me_state = state.clone();
  let mut api = OpenApi::default();

  let public = ApiRouter::new()
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
    .with_state(state)
    .finish_api_with(&mut api, |a| a.title("sonify-health"));

  // Stash the OpenAPI spec in an Arc so it can be shared with the
  // JSON endpoint built by the caller.
  let api = Arc::new(api);
  Router::new()
    .merge(public)
    .route("/me", get(me_handler).with_state(me_state))
    .route(
      "/api-docs/openapi.json",
      get({
        let api = api.clone();
        move || async move { Json((*api).clone()) }
      }),
    )
}

/// Build a minimal router with just the WebSocket endpoint, no
/// auth middleware.  Used by integration tests.
pub fn test_router(state: AppState) -> Router {
  Router::new().route("/ws", get(websocket::ws_handler).with_state(state))
}

/// Routes that are protected when OIDC is enabled: mute API,
/// WebSocket, Scalar docs, and the SPA fallback.
pub fn protected_router(state: AppState) -> Router {
  let frontend_path = state.frontend_path.clone();

  let mute_api = ApiRouter::new()
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
    .with_state(state.clone());

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
    .merge(mute_api)
    .route("/ws", get(websocket::ws_handler).with_state(state))
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
