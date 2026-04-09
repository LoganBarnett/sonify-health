use prometheus::{IntCounter, IntGauge, Registry};
use std::sync::Arc;

/// Holds all Prometheus collectors for the daemon, registered
/// against a single registry.  All prometheus types are
/// `Arc`-wrapped internally, so this struct is cheap to clone.
#[derive(Clone)]
pub struct Metrics {
  pub registry: Arc<Registry>,
  pub heartbeats_played: IntCounter,
  pub muted: IntGauge,
}

impl Default for Metrics {
  fn default() -> Self {
    Self::new()
  }
}

impl Metrics {
  /// Build a fresh registry and register every collector.
  pub fn new() -> Self {
    let registry = Registry::new();

    let heartbeats_played = IntCounter::new(
      "sonify_health_heartbeats_played_total",
      "Total heartbeat audio sequences played.",
    )
    .expect("Failed to create heartbeats_played counter");

    let muted = IntGauge::new(
      "sonify_health_muted",
      "Whether audio output is currently muted (1=muted, 0=unmuted).",
    )
    .expect("Failed to create muted gauge");

    registry
      .register(Box::new(heartbeats_played.clone()))
      .expect("Failed to register heartbeats_played");
    registry
      .register(Box::new(muted.clone()))
      .expect("Failed to register muted");

    Self {
      registry: Arc::new(registry),
      heartbeats_played,
      muted,
    }
  }
}
