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

//! End-to-end test that step 4's outbound connector mirrors a
//! remote sonify-health instance's state into a Remote Source on
//! the local `PreviewState`.
//!
//! Spins up two `PreviewState` instances in-process: instance A
//! plays the role of the speakerless remote (it has a heartbeat
//! config and a metric value), and instance B has a Remote Source
//! pointing at A's WebSocket address.  After the connector task
//! starts, B's mirror should populate with A's library and
//! heartbeat shape and pick up live metric updates.

use sonify_health_cli::{
  config::SliderRanges,
  metrics::Metrics,
  preview_state::{ConnectionStatus, PreviewState, SourceKind},
  remote_source,
  web_base::{test_router, AppState},
};
use sonify_health_lib::{
  builtin_library, heartbeat_config::default_crossfade_ms, HeartbeatConfig,
  NoteConfig, Playback, ResultMode, Transition,
};
use std::collections::HashMap;
use std::sync::{atomic::AtomicBool, Arc};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;

/// Build a "remote" PreviewState with one heartbeat ready for
/// mirroring, and start its WebSocket server.  Returns the
/// listening address and the Arc<PreviewState> so the test can
/// poke metric values directly.
async fn start_remote_instance() -> (std::net::SocketAddr, Arc<PreviewState>) {
  let library = builtin_library();
  let heartbeat = HeartbeatConfig::new(
    "remote-disk".to_string(),
    "echo 0".to_string(),
    ResultMode::ExitCode,
    vec![NoteConfig {
      transition: Transition::Discrete {
        states: vec![sonify_health_lib::transition::DiscreteState {
          threshold: 1.01,
          patch: "sine".to_string(),
        }],
      },
      volume: 0.3,
      offset: 0.0,
    }],
    Playback::Clock,
    0.0,
    1.0,
    5.0,
    10.0,
    0.0,
    default_crossfade_ms(),
    vec![],
  );
  let muted = Arc::new(AtomicBool::new(false));
  let running = Arc::new(AtomicBool::new(true));

  let preview = Arc::new(PreviewState::new(
    library,
    HashMap::new(),
    vec![heartbeat],
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

/// Build a "subscriber" PreviewState with a Remote Source pointing
/// at `remote_addr`, and spawn its connector task.
fn start_subscriber(remote_addr: std::net::SocketAddr) -> Arc<PreviewState> {
  let subscriber = Arc::new(PreviewState::new(
    builtin_library(),
    HashMap::new(),
    vec![],
    Arc::new(AtomicBool::new(false)),
    Arc::new(AtomicBool::new(true)),
    Metrics::new().expect("Metrics::new in test"),
    SliderRanges::default(),
    None,
    false,
    false,
  ));
  let url = format!("ws://{remote_addr}/ws");
  let name = "remote-instance".to_string();
  subscriber.add_remote_source(name.clone(), url).unwrap();

  let connector_preview = Arc::clone(&subscriber);
  tokio::spawn(async move {
    remote_source::run_connector(connector_preview, name).await;
  });
  subscriber
}

/// Poll a closure until it returns true or `timeout` elapses.  Used
/// to wait for the connector to finish populating the mirror.
async fn wait_for(
  timeout: Duration,
  description: &str,
  mut check: impl FnMut() -> bool,
) {
  let deadline = Instant::now() + timeout;
  while Instant::now() < deadline {
    if check() {
      return;
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
  }
  panic!("Timed out waiting for: {description}");
}

#[tokio::test]
async fn remote_mirror_populates_from_state_snapshot() {
  let (addr, _remote) = start_remote_instance().await;
  let subscriber = start_subscriber(addr);
  let remote = subscriber.source_by_name("remote-instance").unwrap();

  wait_for(
    Duration::from_secs(2),
    "remote heartbeat config to be mirrored",
    || !remote.heartbeat_configs.read().is_empty(),
  )
  .await;

  let configs = remote.heartbeat_configs.read();
  assert_eq!(configs.len(), 1);
  assert_eq!(configs[0].name, "remote-disk");
  assert_eq!(configs[0].command, "echo 0");
  drop(configs);

  let lib = remote.library.read();
  assert!(
    lib.contains_key("sine"),
    "mirrored library should include the remote's patches"
  );

  // Connection status should have advanced past Connecting.
  if let SourceKind::Remote { status, .. } = &remote.kind {
    assert!(matches!(*status.read(), ConnectionStatus::Connected));
  } else {
    panic!("expected Remote kind");
  }
}

#[tokio::test]
async fn remote_mirror_propagates_metric_updates() {
  let (addr, remote_instance) = start_remote_instance().await;
  let subscriber = start_subscriber(addr);
  let remote = subscriber.source_by_name("remote-instance").unwrap();

  // Wait for the initial state snapshot.
  wait_for(Duration::from_secs(2), "initial mirror to populate", || {
    !remote.heartbeats.read().is_empty()
  })
  .await;

  // Drive a metric_changed broadcast on the remote instance, the
  // way the daemon's poll thread would.
  let remote_local = remote_instance.local();
  remote_local.heartbeats.read()[0].metric.set_value(0.7);
  let _ = remote_instance.broadcast_tx.send(
    serde_json::json!({
      "type": "metric_changed",
      "index": 0,
      "value": 0.7,
    })
    .to_string(),
  );

  // The subscriber's mirror should pick that up incrementally
  // (without re-fetching the full snapshot).
  wait_for(Duration::from_secs(2), "metric to propagate to subscriber", || {
    let hbs = remote.heartbeats.read();
    hbs
      .first()
      .map(|h| (h.metric.value() - 0.7).abs() < 1e-3)
      .unwrap_or(false)
  })
  .await;
}
