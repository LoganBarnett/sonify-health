use crate::severity::Severity;
use fundsp::prelude32::shared;
use fundsp::shared::Shared;

/// Shared state for three heartbeat severity values.
///
/// Check threads write severity; the audio thread reads via
/// `fundsp::var()`.  The `Shared` primitive is lock-free.
pub struct HeartbeatState {
  /// Encoded as 0.0, 1.0, or 2.0 for fundsp consumption.
  pub boops: [Shared; 3],
}

impl Default for HeartbeatState {
  fn default() -> Self {
    Self {
      boops: [shared(0.0), shared(0.0), shared(0.0)],
    }
  }
}

impl HeartbeatState {
  /// Update the severity for a single boop slot.
  pub fn set(&self, index: usize, severity: Severity) {
    if index < 3 {
      self.boops[index].set_value(severity as u8 as f32);
    }
  }
}

/// Shared state for drone metric values (0.0..=1.0).
pub struct DroneState {
  pub metrics: Vec<Shared>,
}

impl DroneState {
  /// Create state for the given number of metrics, all
  /// initialized to zero.
  pub fn new(count: usize) -> Self {
    Self {
      metrics: (0..count).map(|_| shared(0.0)).collect(),
    }
  }

  /// Update a metric value.  Clamped to 0.0..=1.0.
  pub fn set(&self, index: usize, value: f32) {
    if let Some(m) = self.metrics.get(index) {
      m.set_value(value.clamp(0.0, 1.0));
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn heartbeat_state_round_trip() {
    let state = HeartbeatState::default();
    state.set(0, Severity::Degraded);
    assert_eq!(state.boops[0].value(), 1.0);
  }

  #[test]
  fn drone_state_clamps() {
    let state = DroneState::new(2);
    state.set(0, 1.5);
    assert_eq!(state.metrics[0].value(), 1.0);
  }

  #[test]
  fn out_of_bounds_set_is_safe() {
    let state = HeartbeatState::default();
    state.set(5, Severity::Down);
  }
}
