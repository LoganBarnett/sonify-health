use clap::ValueEnum;
use serde::Deserialize;

/// Pitch register for a drone voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum DroneRegister {
  Low,
  Mid,
  High,
}

impl DroneRegister {
  /// Frequency multiplier relative to the voice's base.
  pub fn multiplier(self) -> f64 {
    match self {
      DroneRegister::Low => 0.5,
      DroneRegister::Mid => 1.0,
      DroneRegister::High => 2.0,
    }
  }
}

/// Maximum gap between drone phrases at metric 0.0.
const MAX_GAP_SECS: f64 = 8.0;

/// Minimum gap between drone phrases at metric 1.0.
const MIN_GAP_SECS: f64 = 0.3;

/// Compute the gap between drone phrases from a metric value.
/// Metric 0.0 produces a long gap (~8 s), metric 1.0 produces a
/// short gap (~0.3 s).  The mapping is linear.
pub fn drone_gap_secs(metric: f32) -> f64 {
  let m = (metric as f64).clamp(0.0, 1.0);
  MAX_GAP_SECS - m * (MAX_GAP_SECS - MIN_GAP_SECS)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn gap_at_zero_metric_is_max() {
    let gap = drone_gap_secs(0.0);
    assert!(
      (gap - MAX_GAP_SECS).abs() < 1e-10,
      "Expected max gap at metric 0.0, got {gap}"
    );
  }

  #[test]
  fn gap_at_full_metric_is_min() {
    let gap = drone_gap_secs(1.0);
    assert!(
      (gap - MIN_GAP_SECS).abs() < 1e-10,
      "Expected min gap at metric 1.0, got {gap}"
    );
  }

  #[test]
  fn gap_monotonically_decreases() {
    let g1 = drone_gap_secs(0.2);
    let g2 = drone_gap_secs(0.5);
    let g3 = drone_gap_secs(0.8);
    assert!(g1 > g2 && g2 > g3);
  }

  #[test]
  fn gap_clamps_out_of_range() {
    assert!((drone_gap_secs(-0.5) - MAX_GAP_SECS).abs() < 1e-10);
    assert!((drone_gap_secs(1.5) - MIN_GAP_SECS).abs() < 1e-10);
  }
}
