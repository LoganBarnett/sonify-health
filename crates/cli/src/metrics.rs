use prometheus::{GaugeVec, IntCounterVec, IntGauge, Opts, Registry};
use std::sync::Arc;

/// Holds all Prometheus collectors for the daemon, registered
/// against a single registry.  All prometheus types are
/// `Arc`-wrapped internally, so this struct is cheap to clone.
#[derive(Clone)]
pub struct Metrics {
  pub registry: Arc<Registry>,
  pub heartbeats_played: IntCounterVec,
  pub probes_completed: IntCounterVec,
  pub probe_value: GaugeVec,
  pub muted: IntGauge,
  pub audio_lock_failures: IntGauge,
  pub audio_nan_frames: IntGauge,
  pub audio_peak_callback_us: IntGauge,
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

    let probe_value = GaugeVec::new(
      Opts::new(
        "sonify_health_probe_value",
        "Most recent probe metric value per heartbeat (0.0–1.0).",
      ),
      &["heartbeat"],
    )
    .expect("Failed to create probe_value gauge");

    let muted = IntGauge::new(
      "sonify_health_muted",
      "Whether audio output is currently muted (1=muted, 0=unmuted).",
    )
    .expect("Failed to create muted gauge");

    let audio_lock_failures = IntGauge::new(
      "sonify_health_audio_lock_failures_total",
      "Number of audio callbacks where the mixer slot lock was contended.",
    )
    .expect("Failed to create audio_lock_failures gauge");

    let audio_nan_frames = IntGauge::new(
      "sonify_health_audio_nan_frames_total",
      "Number of audio callbacks where a graph produced NaN/Inf samples.",
    )
    .expect("Failed to create audio_nan_frames gauge");

    let audio_peak_callback_us = IntGauge::new(
      "sonify_health_audio_peak_callback_us",
      "Peak audio callback duration in microseconds since last reset.",
    )
    .expect("Failed to create audio_peak_callback_us gauge");

    registry
      .register(Box::new(heartbeats_played.clone()))
      .expect("Failed to register heartbeats_played");
    registry
      .register(Box::new(probes_completed.clone()))
      .expect("Failed to register probes_completed");
    registry
      .register(Box::new(probe_value.clone()))
      .expect("Failed to register probe_value");
    registry
      .register(Box::new(muted.clone()))
      .expect("Failed to register muted");
    registry
      .register(Box::new(audio_lock_failures.clone()))
      .expect("Failed to register audio_lock_failures");
    registry
      .register(Box::new(audio_nan_frames.clone()))
      .expect("Failed to register audio_nan_frames");
    registry
      .register(Box::new(audio_peak_callback_us.clone()))
      .expect("Failed to register audio_peak_callback_us");

    Self {
      registry: Arc::new(registry),
      heartbeats_played,
      probes_completed,
      probe_value,
      muted,
      audio_lock_failures,
      audio_nan_frames,
      audio_peak_callback_us,
    }
  }
}
