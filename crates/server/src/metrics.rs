use prometheus::{
  core::Collector, Gauge, GaugeVec, IntCounterVec, IntGauge, Opts, Registry,
};
use thiserror::Error;

/// Errors raised while constructing or registering Prometheus
/// collectors during `Metrics::new`.  The metric `name` field
/// pinpoints which one tripped — operationally useful when a
/// future edit introduces a duplicate registration or an invalid
/// label set.
#[derive(Debug, Error)]
pub enum MetricsInitError {
  #[error("Failed to construct Prometheus collector {name:?}: {source}")]
  CollectorConstructionFailed {
    name: &'static str,
    #[source]
    source: prometheus::Error,
  },

  #[error("Failed to register Prometheus collector {name:?}: {source}")]
  CollectorRegistrationFailed {
    name: &'static str,
    #[source]
    source: prometheus::Error,
  },
}

/// Holds every Prometheus collector for the daemon, registered
/// against the foundation-owned registry so a single `/metrics`
/// endpoint exposes both infra metrics (`http_requests_total`,
/// etc.) and sonify's audio/probe metrics.  All prometheus types
/// are `Arc`-wrapped internally, so this struct is cheap to clone.
#[derive(Clone)]
pub struct Metrics {
  pub heartbeats_played: IntCounterVec,
  pub probes_completed: IntCounterVec,
  pub probe_value: GaugeVec,
  pub muted: IntGauge,
  pub audio_lock_failures: IntGauge,
  pub audio_nan_frames: IntGauge,
  pub audio_peak_callback_us: IntGauge,
  pub audio_stream_errors: IntGauge,
  pub audio_stream_failed: IntGauge,
  pub audio_recovery_attempts: IntGauge,
  pub audio_output_peak_amplitude: Gauge,
  pub audio_slot_peak_amplitude: GaugeVec,
  pub audio_slot_rms_amplitude: GaugeVec,
  pub audio_callback_buffer_frames_min: IntGauge,
  pub audio_callback_buffer_frames_max: IntGauge,
}

/// Wrap a constructor `Result` with a semantic
/// `CollectorConstructionFailed { name }` so the failure points at
/// the specific metric.  Lifted out so the call sites stay tight.
fn construct<T>(
  name: &'static str,
  result: prometheus::Result<T>,
) -> Result<T, MetricsInitError> {
  result.map_err(|source| MetricsInitError::CollectorConstructionFailed {
    name,
    source,
  })
}

/// Register `collector` against `registry`, attaching the metric
/// `name` to any failure for diagnosability.
fn register<C: Collector + Clone + 'static>(
  registry: &Registry,
  name: &'static str,
  collector: &C,
) -> Result<(), MetricsInitError> {
  registry
    .register(Box::new(collector.clone()))
    .map_err(|source| MetricsInitError::CollectorRegistrationFailed {
      name,
      source,
    })
}

impl Metrics {
  /// Construct every audio/probe collector and register it on the
  /// caller-supplied registry.  Returns `Err(MetricsInitError)` if
  /// prometheus rejects a collector — that only happens for invalid
  /// metric names or duplicate registrations, both of which are
  /// programmer errors, but a typed error keeps the failure visible
  /// to `main`'s startup error path rather than aborting the
  /// process from inside a constructor.
  pub fn new(registry: &Registry) -> Result<Self, MetricsInitError> {
    let heartbeats_played = construct(
      "sonify_health_heartbeats_played_total",
      IntCounterVec::new(
        Opts::new(
          "sonify_health_heartbeats_played_total",
          "Total heartbeat audio sequences played.",
        ),
        &["heartbeat"],
      ),
    )?;

    let probes_completed = construct(
      "sonify_health_probes_completed_total",
      IntCounterVec::new(
        Opts::new(
          "sonify_health_probes_completed_total",
          "Total probe executions completed per heartbeat.",
        ),
        &["heartbeat"],
      ),
    )?;

    let probe_value = construct(
      "sonify_health_probe_value",
      GaugeVec::new(
        Opts::new(
          "sonify_health_probe_value",
          "Most recent probe metric value per heartbeat (0.0–1.0).",
        ),
        &["heartbeat"],
      ),
    )?;

    let muted = construct(
      "sonify_health_muted",
      IntGauge::new(
        "sonify_health_muted",
        "Whether audio output is currently muted (1=muted, 0=unmuted).",
      ),
    )?;

    let audio_lock_failures = construct(
      "sonify_health_audio_lock_failures_total",
      IntGauge::new(
        "sonify_health_audio_lock_failures_total",
        "Number of audio callbacks where the mixer slot lock was contended.",
      ),
    )?;

    let audio_nan_frames = construct(
      "sonify_health_audio_nan_frames_total",
      IntGauge::new(
        "sonify_health_audio_nan_frames_total",
        "Number of audio callbacks where a graph produced NaN/Inf samples.",
      ),
    )?;

    let audio_peak_callback_us = construct(
      "sonify_health_audio_peak_callback_us",
      IntGauge::new(
        "sonify_health_audio_peak_callback_us",
        "Peak audio callback duration in microseconds since last reset.",
      ),
    )?;

    let audio_stream_errors = construct(
      "sonify_health_audio_stream_errors_total",
      IntGauge::new(
        "sonify_health_audio_stream_errors_total",
        "Cumulative stream errors from the cpal error callback.",
      ),
    )?;

    let audio_stream_failed = construct(
      "sonify_health_audio_stream_failed",
      IntGauge::new(
        "sonify_health_audio_stream_failed",
        "Whether the audio stream has failed (1=failed, 0=ok).",
      ),
    )?;

    let audio_recovery_attempts = construct(
      "sonify_health_audio_recovery_attempts_total",
      IntGauge::new(
        "sonify_health_audio_recovery_attempts_total",
        "Total number of audio stream recovery attempts.",
      ),
    )?;

    let audio_output_peak_amplitude = construct(
      "sonify_health_audio_output_peak_amplitude",
      Gauge::new(
        "sonify_health_audio_output_peak_amplitude",
        "Peak abs amplitude of mixed output per window.",
      ),
    )?;

    let audio_slot_peak_amplitude = construct(
      "sonify_health_audio_slot_peak_amplitude",
      GaugeVec::new(
        Opts::new(
          "sonify_health_audio_slot_peak_amplitude",
          "Per-slot peak abs amplitude per window.",
        ),
        &["slot"],
      ),
    )?;

    let audio_slot_rms_amplitude = construct(
      "sonify_health_audio_slot_rms_amplitude",
      GaugeVec::new(
        Opts::new(
          "sonify_health_audio_slot_rms_amplitude",
          "Per-slot peak RMS amplitude per window.",
        ),
        &["slot"],
      ),
    )?;

    let audio_callback_buffer_frames_min = construct(
      "sonify_health_audio_callback_buffer_frames_min",
      IntGauge::new(
        "sonify_health_audio_callback_buffer_frames_min",
        "Smallest callback buffer size (frames) in window.",
      ),
    )?;

    let audio_callback_buffer_frames_max = construct(
      "sonify_health_audio_callback_buffer_frames_max",
      IntGauge::new(
        "sonify_health_audio_callback_buffer_frames_max",
        "Largest callback buffer size (frames) in window.",
      ),
    )?;

    register(
      registry,
      "sonify_health_heartbeats_played_total",
      &heartbeats_played,
    )?;
    register(
      registry,
      "sonify_health_probes_completed_total",
      &probes_completed,
    )?;
    register(registry, "sonify_health_probe_value", &probe_value)?;
    register(registry, "sonify_health_muted", &muted)?;
    register(
      registry,
      "sonify_health_audio_lock_failures_total",
      &audio_lock_failures,
    )?;
    register(
      registry,
      "sonify_health_audio_nan_frames_total",
      &audio_nan_frames,
    )?;
    register(
      registry,
      "sonify_health_audio_peak_callback_us",
      &audio_peak_callback_us,
    )?;
    register(
      registry,
      "sonify_health_audio_stream_errors_total",
      &audio_stream_errors,
    )?;
    register(
      registry,
      "sonify_health_audio_stream_failed",
      &audio_stream_failed,
    )?;
    register(
      registry,
      "sonify_health_audio_recovery_attempts_total",
      &audio_recovery_attempts,
    )?;
    register(
      registry,
      "sonify_health_audio_output_peak_amplitude",
      &audio_output_peak_amplitude,
    )?;
    register(
      registry,
      "sonify_health_audio_slot_peak_amplitude",
      &audio_slot_peak_amplitude,
    )?;
    register(
      registry,
      "sonify_health_audio_slot_rms_amplitude",
      &audio_slot_rms_amplitude,
    )?;
    register(
      registry,
      "sonify_health_audio_callback_buffer_frames_min",
      &audio_callback_buffer_frames_min,
    )?;
    register(
      registry,
      "sonify_health_audio_callback_buffer_frames_max",
      &audio_callback_buffer_frames_max,
    )?;

    Ok(Self {
      heartbeats_played,
      probes_completed,
      probe_value,
      muted,
      audio_lock_failures,
      audio_nan_frames,
      audio_peak_callback_us,
      audio_stream_errors,
      audio_stream_failed,
      audio_recovery_attempts,
      audio_output_peak_amplitude,
      audio_slot_peak_amplitude,
      audio_slot_rms_amplitude,
      audio_callback_buffer_frames_min,
      audio_callback_buffer_frames_max,
    })
  }
}
