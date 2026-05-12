// Helpers in this file sit outside `#[test]` functions, so
// clippy.toml's `allow-{unwrap,expect,panic}-in-tests` does not
// reach them.  Opt the whole file in explicitly.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use sonify_health_lib::config::SliderRanges;
use sonify_health_lib::{builtin_library, HeartbeatConfig};
use sonify_health_server::{
  audio_engine::{extract_panic_message, spawn_poll_thread},
  preview_state::PreviewState,
};
use std::collections::HashMap;
use std::sync::{
  atomic::{AtomicBool, Ordering},
  Arc,
};
use std::time::Duration;

fn healthy_heartbeat(name: &str) -> HeartbeatConfig {
  let mut hb = HeartbeatConfig::test_default();
  hb.name = name.to_string();
  hb.command = "true".to_string();
  hb.result_mode = sonify_health_lib::probe::ResultMode::ExitCode;
  hb.poll_interval_secs = 0.1;
  hb
}

/// Spawn a thread that panics, join it, and verify the panic
/// payload is extractable via `extract_panic_message`.  This
/// validates the foundation the supervisor relies on.
#[test]
fn panicking_thread_is_joinable() {
  let h = std::thread::spawn(|| {
    panic!("intentional test panic");
  });

  let result = h.join();
  assert!(result.is_err(), "thread should have panicked");
  let msg = extract_panic_message(&result.unwrap_err());
  assert_eq!(msg, "intentional test panic");
}

/// Spawn a poll thread whose heartbeat config is swapped to an
/// empty vec after spawning, causing it to panic on the next
/// iteration when it indexes the (now-empty) configs.  Verify the
/// join handle captures the panic.
#[test]
fn poll_thread_panic_is_capturable() {
  let heartbeats = vec![healthy_heartbeat("alpha")];
  let running = Arc::new(AtomicBool::new(true));
  let preview = Arc::new(PreviewState::new(
    builtin_library(),
    HashMap::new(),
    heartbeats,
    Arc::new(AtomicBool::new(false)),
    Arc::clone(&running),
    common::test_metrics(),
    SliderRanges::default(),
    None,
    false,
    false,
  ));

  let local = preview.local();
  let h = spawn_poll_thread(&preview, &local, 0);

  // Let one cycle complete, then remove all configs so the next
  // iteration panics on out-of-bounds access.
  std::thread::sleep(Duration::from_millis(200));
  {
    let local = preview.local();
    let mut configs = local.heartbeat_configs.write();
    configs.clear();
  }

  // Wait for it to crash.
  std::thread::sleep(Duration::from_millis(500));
  assert!(h.is_finished(), "thread should have panicked by now");

  let result = h.join();
  assert!(result.is_err(), "thread should have panicked");
  let msg = extract_panic_message(&result.unwrap_err());
  assert!(!msg.is_empty(), "panic message should be non-empty, got: {msg}");

  running.store(false, Ordering::Relaxed);
}
