use fundsp::prelude32::shared;
use fundsp::shared::Shared;

/// Shared state for check metric values (0.0..=1.0).
///
/// Check threads write metrics; the audio thread reads via
/// `fundsp::var()`.  The `Shared` primitive is lock-free.
pub struct CheckState {
  pub metrics: Vec<Shared>,
}

impl CheckState {
  /// Create state for the given number of checks, all initialized
  /// to zero.
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
  fn check_state_round_trip() {
    let state = CheckState::new(3);
    state.set(0, 0.5);
    assert!((state.metrics[0].value() - 0.5).abs() < f32::EPSILON);
  }

  #[test]
  fn check_state_clamps() {
    let state = CheckState::new(2);
    state.set(0, 1.5);
    assert_eq!(state.metrics[0].value(), 1.0);
  }

  #[test]
  fn out_of_bounds_set_is_safe() {
    let state = CheckState::new(3);
    state.set(5, 1.0);
  }

  #[test]
  fn zero_checks_is_valid() {
    let state = CheckState::new(0);
    assert!(state.metrics.is_empty());
    state.set(0, 1.0);
  }
}
