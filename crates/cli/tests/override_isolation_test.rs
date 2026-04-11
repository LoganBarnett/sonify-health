use sonify_health_cli::{
  config::SliderRanges, daemon::spawn_poll_thread, metrics::Metrics,
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

/// Overriding heartbeat 1 must not affect heartbeat 0's metric.
#[test]
fn override_does_not_leak_to_other_heartbeats() {
  let heartbeats = vec![healthy_heartbeat("alpha"), healthy_heartbeat("beta")];
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

  let h0 = spawn_poll_thread(&preview, 0);
  let h1 = spawn_poll_thread(&preview, 1);

  // Let both heartbeats poll a few times at 0.1s interval.
  std::thread::sleep(Duration::from_millis(350));

  // Both should be healthy (0.0).
  let m0 = preview.heartbeats.read().unwrap()[0].metric.value();
  let m1 = preview.heartbeats.read().unwrap()[1].metric.value();
  assert!(m0.abs() < 0.001, "alpha should be 0.0 before override, got {m0}");
  assert!(m1.abs() < 0.001, "beta should be 0.0 before override, got {m1}");

  // Override heartbeat 1 to 1.0.
  {
    let hbs = preview.heartbeats.read().unwrap();
    *hbs[1].override_value.write().unwrap() = Some(1.0);
  }

  // Wait for several more poll cycles.
  std::thread::sleep(Duration::from_millis(500));

  // Heartbeat 0 must still be 0.0.
  let m0 = preview.heartbeats.read().unwrap()[0].metric.value();
  let m1 = preview.heartbeats.read().unwrap()[1].metric.value();
  assert!(
    m0.abs() < 0.001,
    "alpha should remain 0.0 after overriding beta, got {m0}"
  );
  assert!(
    (m1 - 1.0).abs() < 0.001,
    "beta should be 1.0 (overridden), got {m1}"
  );

  running.store(false, Ordering::Relaxed);
  h0.join().unwrap();
  h1.join().unwrap();
}
