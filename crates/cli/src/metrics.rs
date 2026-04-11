use prometheus::{IntCounterVec, IntGauge, Opts, Registry};
use std::sync::Arc;

/// Holds all Prometheus collectors for the daemon, registered
/// against a single registry.  All prometheus types are
/// `Arc`-wrapped internally, so this struct is cheap to clone.
#[derive(Clone)]
pub struct Metrics {
  pub registry: Arc<Registry>,
  pub heartbeats_played: IntCounterVec,
  pub probes_completed: IntCounterVec,
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

    let heartbeats_played = IntCounterVec::new(
      Opts::new(
        "sonify_health_heartbeats_played_total",
        "Total heartbeat audio sequences played.",
      ),
      &["heartbeat"],
    )
    .expect("Failed to create heartbeats_played counter");

    let probes_completed = IntCounterVec::new(
      Opts::new(
        "sonify_health_probes_completed_total",
        "Total probe executions completed per heartbeat.",
      ),
      &["heartbeat"],
    )
    .expect("Failed to create probes_completed counter");

    let muted = IntGauge::new(
      "sonify_health_muted",
      "Whether audio output is currently muted (1=muted, 0=unmuted).",
    )
    .expect("Failed to create muted gauge");

    registry
      .register(Box::new(heartbeats_played.clone()))
      .expect("Failed to register heartbeats_played");
    registry
      .register(Box::new(probes_completed.clone()))
      .expect("Failed to register probes_completed");
    registry
      .register(Box::new(muted.clone()))
      .expect("Failed to register muted");

    Self {
      registry: Arc::new(registry),
      heartbeats_played,
      probes_completed,
      muted,
    }
  }
}
