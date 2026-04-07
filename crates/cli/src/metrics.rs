use prometheus::{
  GaugeVec, IntCounter, IntCounterVec, IntGauge, IntGaugeVec, Opts, Registry,
};
use std::sync::Arc;

/// Holds all Prometheus collectors for the daemon, registered
/// against a single registry.  All prometheus types are
/// `Arc`-wrapped internally, so this struct is cheap to clone.
#[derive(Clone)]
pub struct Metrics {
  pub registry: Arc<Registry>,
  pub check_severity: IntGaugeVec,
  pub check_runs: IntCounterVec,
  pub heartbeats_played: IntCounter,
  pub muted: IntGauge,
  pub drone_metric_value: GaugeVec,
  pub drone_polls: IntCounterVec,
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

    let check_severity = IntGaugeVec::new(
            Opts::new(
                "sonify_health_check_severity",
                "Current severity level for each check (0=healthy, 1=degraded, 2=down).",
            ),
            &["check"],
        )
        .expect("Failed to create check_severity gauge");

    let check_runs = IntCounterVec::new(
      Opts::new(
        "sonify_health_check_runs_total",
        "Total check executions partitioned by outcome.",
      ),
      &["check", "result"],
    )
    .expect("Failed to create check_runs counter");

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

    let drone_metric_value = GaugeVec::new(
      Opts::new(
        "sonify_health_drone_metric_value",
        "Current normalized value (0.0..1.0) for each drone metric.",
      ),
      &["metric"],
    )
    .expect("Failed to create drone_metric_value gauge");

    let drone_polls = IntCounterVec::new(
      Opts::new(
        "sonify_health_drone_polls_total",
        "Total drone poll executions partitioned by outcome.",
      ),
      &["metric", "result"],
    )
    .expect("Failed to create drone_polls counter");

    registry
      .register(Box::new(check_severity.clone()))
      .expect("Failed to register check_severity");
    registry
      .register(Box::new(check_runs.clone()))
      .expect("Failed to register check_runs");
    registry
      .register(Box::new(heartbeats_played.clone()))
      .expect("Failed to register heartbeats_played");
    registry
      .register(Box::new(muted.clone()))
      .expect("Failed to register muted");
    registry
      .register(Box::new(drone_metric_value.clone()))
      .expect("Failed to register drone_metric_value");
    registry
      .register(Box::new(drone_polls.clone()))
      .expect("Failed to register drone_polls");

    Self {
      registry: Arc::new(registry),
      check_severity,
      check_runs,
      heartbeats_played,
      muted,
      drone_metric_value,
      drone_polls,
    }
  }
}
