// Tests-only exemption from the workspace's no-unwrap policy.
// See workspace `[lints.clippy]` in the root Cargo.toml.
#![allow(
  clippy::unwrap_used,
  clippy::expect_used,
  clippy::panic,
  clippy::unreachable,
  clippy::todo,
  clippy::unimplemented
)]

use futures::{SinkExt, StreamExt};
use serde_json::json;
use sonify_health_cli::{
  config::SliderRanges,
  metrics::Metrics,
  preview_state::PreviewState,
  web_base::{test_router, AppState},
};
use sonify_health_lib::{builtin_library, Playback};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{atomic::AtomicBool, Arc};
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// Spin up a test server with one default heartbeat and return
/// the bound address plus the `PreviewState`.
async fn start_test_server() -> (SocketAddr, Arc<PreviewState>) {
  let library = builtin_library();
  let heartbeats = vec![sonify_health_lib::HeartbeatConfig::test_default()];
  let muted = Arc::new(AtomicBool::new(false));

  let running = Arc::new(AtomicBool::new(true));
  let preview = Arc::new(PreviewState::new(
    library,
    HashMap::new(),
    heartbeats,
    muted.clone(),
    running,
    Metrics::new().expect("Metrics::new in test"),
    SliderRanges::default(),
    None,
    false,
    false,
  ));

  let state = AppState::init(
    muted,
    Metrics::new().expect("Metrics::new in test"),
    std::path::PathBuf::from("frontend/public"),
    Arc::clone(&preview),
    None,
  );

  let app = test_router(state);
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let addr = listener.local_addr().unwrap();

  tokio::spawn(async move {
    axum::serve(listener, app.into_make_service())
      .await
      .unwrap();
  });

  (addr, preview)
}

/// Connect a ws client and consume the initial state snapshot.
async fn connect_ws(
  addr: SocketAddr,
) -> tokio_tungstenite::WebSocketStream<
  tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
> {
  let url = format!("ws://{addr}/ws");
  let (ws, _) = connect_async(url).await.unwrap();
  ws
}

/// Read messages until we find one whose "type" matches `msg_type`,
/// or time out after 2 seconds.
async fn read_until_type(
  ws: &mut (impl StreamExt<
    Item = Result<Message, tokio_tungstenite::tungstenite::Error>,
  > + Unpin),
  msg_type: &str,
) -> Option<serde_json::Value> {
  let timeout = tokio::time::sleep(std::time::Duration::from_secs(2));
  tokio::pin!(timeout);

  loop {
    tokio::select! {
      msg = ws.next() => {
        let msg = msg?.ok()?;
        if let Message::Text(text) = msg {
          let s: &str = &text;
          if let Ok(val) = serde_json::from_str::<serde_json::Value>(s) {
            if val.get("type").and_then(|v| v.as_str()) == Some(msg_type) {
              return Some(val);
            }
          }
        }
      }
      _ = &mut timeout => {
        return None;
      }
    }
  }
}

/// Assert that no message of the given type arrives within a short
/// timeout.
async fn assert_no_message(
  ws: &mut (impl StreamExt<
    Item = Result<Message, tokio_tungstenite::tungstenite::Error>,
  > + Unpin),
  msg_type: &str,
) {
  let timeout = tokio::time::sleep(std::time::Duration::from_millis(300));
  tokio::pin!(timeout);

  loop {
    tokio::select! {
      msg = ws.next() => {
        if let Some(Ok(Message::Text(text))) = msg {
          let s: &str = &text;
          if let Ok(val) = serde_json::from_str::<serde_json::Value>(s) {
            assert_ne!(
              val.get("type").and_then(|v| v.as_str()),
              Some(msg_type),
              "Unexpected {msg_type} message received"
            );
          }
        }
      }
      _ = &mut timeout => {
        return;
      }
    }
  }
}

#[tokio::test]
async fn set_playback_round_trip() {
  let (addr, preview) = start_test_server().await;
  let mut ws = connect_ws(addr).await;

  // Consume the initial state snapshot.
  let _ = read_until_type(&mut ws, "state").await;

  ws.send(Message::Text(
    json!({
      "type": "set_playback",
      "index": 0,
      "value": "loop",
    })
    .to_string()
    .into(),
  ))
  .await
  .unwrap();

  let msg = read_until_type(&mut ws, "playback_changed").await;
  let msg = msg.expect("Expected playback_changed message");
  assert_eq!(msg["index"], 0);
  assert_eq!(msg["value"], "loop");

  let local = preview.local();
  let configs = local.heartbeat_configs.read();
  assert_eq!(configs[0].playback, Playback::Loop);
}

#[tokio::test]
async fn set_playback_all_modes() {
  let (addr, _preview) = start_test_server().await;
  let mut ws = connect_ws(addr).await;
  let _ = read_until_type(&mut ws, "state").await;

  for mode in ["clock", "loop", "continuous"] {
    ws.send(Message::Text(
      json!({
        "type": "set_playback",
        "index": 0,
        "value": mode,
      })
      .to_string()
      .into(),
    ))
    .await
    .unwrap();

    let msg = read_until_type(&mut ws, "playback_changed").await;
    let msg =
      msg.unwrap_or_else(|| panic!("Expected playback_changed for {mode}"));
    assert_eq!(msg["value"], mode);
  }
}

#[tokio::test]
async fn set_playback_invalid_value() {
  let (addr, _preview) = start_test_server().await;
  let mut ws = connect_ws(addr).await;
  let _ = read_until_type(&mut ws, "state").await;

  ws.send(Message::Text(
    json!({
      "type": "set_playback",
      "index": 0,
      "value": "bogus",
    })
    .to_string()
    .into(),
  ))
  .await
  .unwrap();

  assert_no_message(&mut ws, "playback_changed").await;
}

#[tokio::test]
async fn set_playback_out_of_bounds() {
  let (addr, _preview) = start_test_server().await;
  let mut ws = connect_ws(addr).await;
  let _ = read_until_type(&mut ws, "state").await;

  ws.send(Message::Text(
    json!({
      "type": "set_playback",
      "index": 99,
      "value": "loop",
    })
    .to_string()
    .into(),
  ))
  .await
  .unwrap();

  assert_no_message(&mut ws, "playback_changed").await;
}

#[tokio::test]
async fn state_snapshot_includes_playback() {
  let (addr, _preview) = start_test_server().await;
  let mut ws = connect_ws(addr).await;

  let state = read_until_type(&mut ws, "state").await;
  let state = state.expect("Expected state message");
  let hb = &state["heartbeats"][0];
  assert_eq!(hb["playback"], "clock");
}
