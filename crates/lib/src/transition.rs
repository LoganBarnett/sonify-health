use crate::library::PatchLibrary;
use crate::patch::Patch;
use serde::{Deserialize, Serialize};

fn default_curve() -> f64 {
  1.0
}

/// Describes how a probe metric (0.0–1.0) maps to patches.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Transition {
  /// Interpolate between adjacent keyframe patches using
  /// `Patch::lerp`.  The metric is raised to `curve` before
  /// interpolation (power-curve shaping).
  Gradient {
    patches: Vec<String>,
    #[serde(default = "default_curve")]
    curve: f64,
  },
  /// Select the first state whose threshold exceeds the metric.
  Discrete { states: Vec<DiscreteState> },
}

/// A single threshold/patch pair in a discrete transition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscreteState {
  pub threshold: f64,
  pub patch: String,
}

impl Transition {
  /// Resolve a metric to a concrete patch by looking up names in
  /// the library.  Returns `None` if any referenced patch name is
  /// missing from the library.
  pub fn resolve(&self, metric: f64, library: &PatchLibrary) -> Option<Patch> {
    let metric = metric.clamp(0.0, 1.0);
    match self {
      Transition::Gradient { patches, curve } => {
        if patches.is_empty() {
          return None;
        }
        if patches.len() == 1 {
          return library.get(&patches[0]).cloned();
        }
        let shaped = metric.powf(*curve);
        let n = patches.len() - 1;
        let scaled = shaped * n as f64;
        let lo_idx = (scaled.floor() as usize).min(n - 1);
        let hi_idx = lo_idx + 1;
        let t = scaled - lo_idx as f64;
        let lo = library.get(&patches[lo_idx])?;
        let hi = library.get(&patches[hi_idx])?;
        Some(Patch::lerp(lo, hi, t))
      }
      Transition::Discrete { states } => {
        // Select the first state whose threshold exceeds the metric.
        for state in states {
          if metric < state.threshold {
            return library.get(&state.patch).cloned();
          }
        }
        // If metric exceeds all thresholds, use the last state.
        states.last().and_then(|s| library.get(&s.patch).cloned())
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn test_library() -> PatchLibrary {
    let mut lib = PatchLibrary::new();
    lib.insert(
      "low".to_string(),
      Patch {
        freq: 200.0,
        ..Default::default()
      },
    );
    lib.insert(
      "mid".to_string(),
      Patch {
        freq: 500.0,
        ..Default::default()
      },
    );
    lib.insert(
      "high".to_string(),
      Patch {
        freq: 800.0,
        ..Default::default()
      },
    );
    lib
  }

  #[test]
  fn gradient_at_zero_returns_first_patch() {
    let lib = test_library();
    let t = Transition::Gradient {
      patches: vec!["low".into(), "mid".into(), "high".into()],
      curve: 1.0,
    };
    let result = t.resolve(0.0, &lib).unwrap();
    assert_eq!(result.freq, 200.0);
  }

  #[test]
  fn gradient_at_one_returns_last_patch() {
    let lib = test_library();
    let t = Transition::Gradient {
      patches: vec!["low".into(), "mid".into(), "high".into()],
      curve: 1.0,
    };
    let result = t.resolve(1.0, &lib).unwrap();
    assert_eq!(result.freq, 800.0);
  }

  #[test]
  fn gradient_at_half_interpolates() {
    let lib = test_library();
    let t = Transition::Gradient {
      patches: vec!["low".into(), "high".into()],
      curve: 1.0,
    };
    let result = t.resolve(0.5, &lib).unwrap();
    assert!((result.freq - 500.0).abs() < 1e-10);
  }

  #[test]
  fn discrete_selects_by_threshold() {
    let lib = test_library();
    let t = Transition::Discrete {
      states: vec![
        DiscreteState {
          threshold: 0.33,
          patch: "low".into(),
        },
        DiscreteState {
          threshold: 0.66,
          patch: "mid".into(),
        },
        DiscreteState {
          threshold: 1.01,
          patch: "high".into(),
        },
      ],
    };
    let r0 = t.resolve(0.0, &lib).unwrap();
    assert_eq!(r0.freq, 200.0);
    let r5 = t.resolve(0.5, &lib).unwrap();
    assert_eq!(r5.freq, 500.0);
    let r9 = t.resolve(0.9, &lib).unwrap();
    assert_eq!(r9.freq, 800.0);
  }

  #[test]
  fn discrete_above_all_thresholds_returns_last() {
    let lib = test_library();
    let t = Transition::Discrete {
      states: vec![DiscreteState {
        threshold: 0.5,
        patch: "low".into(),
      }],
    };
    let result = t.resolve(0.8, &lib).unwrap();
    assert_eq!(result.freq, 200.0);
  }

  #[test]
  fn missing_patch_returns_none() {
    let lib = test_library();
    let t = Transition::Gradient {
      patches: vec!["nonexistent".into()],
      curve: 1.0,
    };
    assert!(t.resolve(0.5, &lib).is_none());
  }
}
