use sonify_health_cli::{
  config::SliderRanges,
  daemon::{extract_panic_message, spawn_poll_thread},
  metrics::Metrics,
  preview_state::PreviewState,
};
use sonify_health_lib::{builtin_library, HeartbeatConfig};
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

/// Poison the heartbeat_configs RwLock, then verify the poll thread
/// continues running by recovering from the poison.
#[test]
fn poll_thread_survives_poisoned_lock() {
  let heartbeats = vec![healthy_heartbeat("alpha")];
  let running = Arc::new(AtomicBool::new(true));
  let preview = Arc::new(PreviewState::new(
    builtin_library(),
    HashMap::new(),
    heartbeats,
    Arc::new(AtomicBool::new(false)),
    Arc::clone(&running),
    Metrics::new(),
    SliderRanges::default(),
    None,
    false,
  ));

  // Spawn the poll thread.
  let h = spawn_poll_thread(&preview, 0);

  // Let it run a few cycles.
  std::thread::sleep(Duration::from_millis(350));

  // Poison the heartbeat_configs lock by panicking while holding a
  // write guard.
  {
    let lock = &preview.heartbeat_configs;
    let lock2 = lock as *const _ as usize;
    let _ = std::thread::spawn(move || {
      // SAFETY: We're in a test and the lock lives on the Arc which
      // outlives this thread.
      let lock_ref =
        unsafe { &*(lock2 as *const std::sync::RwLock<Vec<HeartbeatConfig>>) };
      let _guard = lock_ref.write().unwrap();
      panic!("intentional poison for test");
    })
    .join();

    // Verify the lock is actually poisoned.
    assert!(lock.read().is_err(), "lock should be poisoned");
  }

  // Let the poll thread run a few more cycles — it should recover.
  std::thread::sleep(Duration::from_millis(500));

  assert!(
    !h.is_finished(),
    "poll thread should still be alive after lock poisoning"
  );

  running.store(false, Ordering::Relaxed);
  h.join().unwrap();
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
    Metrics::new(),
    SliderRanges::default(),
    None,
    false,
  ));

  let h = spawn_poll_thread(&preview, 0);

  // Let one cycle complete, then remove all configs so the next
  // iteration panics on out-of-bounds access.
  std::thread::sleep(Duration::from_millis(200));
  {
    let mut configs = preview
      .heartbeat_configs
      .write()
      .unwrap_or_else(|e| e.into_inner());
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
