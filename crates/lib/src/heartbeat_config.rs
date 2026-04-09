use crate::probe::ResultMode;
use crate::transition::Transition;
use serde::{Deserialize, Serialize};

fn default_volume() -> f64 {
  0.3
}

fn default_repeat_rate() -> f64 {
  1.0
}

fn default_poll_interval() -> f64 {
  10.0
}

fn default_cycle_secs() -> f64 {
  14.0
}

/// A heartbeat joins a probe command with a transition that maps
/// the probe's metric to patches from the library.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeartbeatConfig {
  pub name: String,
  pub command: String,
  pub result_mode: ResultMode,
  pub transition: Transition,

  /// Whether this heartbeat plays continuously (drone-style) or
  /// as a one-shot at each cycle.
  #[serde(default)]
  pub continuous: bool,

  /// Output volume for this heartbeat (0.0–1.0).
  #[serde(default = "default_volume")]
  pub volume: f64,

  /// Seconds of silence between phrase repetitions (continuous
  /// mode).
  #[serde(default)]
  pub phrase_gap: f64,

  /// Speed multiplier on phrase repetition.  Divides the gap.
  #[serde(default = "default_repeat_rate")]
  pub repeat_rate: f64,

  /// Seconds between probe command executions.
  #[serde(default = "default_poll_interval")]
  pub poll_interval_secs: f64,

  /// Seconds between plays for one-shot heartbeats.
  #[serde(default = "default_cycle_secs")]
  pub cycle_secs: f64,
}
