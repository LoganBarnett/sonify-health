use crate::library::PatchLibrary;
use crate::patch::Patch;
use serde::{Deserialize, Serialize};

fn default_intensity() -> f64 {
  2.0
}

fn default_step_intensity() -> f64 {
  0.5
}

/// Describes how to interpolate between two adjacent patches
/// within a gradient transition segment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "strategy", rename_all = "kebab-case")]
pub enum LerpStrategy {
  Linear {
    #[serde(default = "default_intensity")]
    intensity: f64,
  },
  EaseIn {
    #[serde(default = "default_intensity")]
    intensity: f64,
  },
  EaseOut {
    #[serde(default = "default_intensity")]
    intensity: f64,
  },
  EaseInOut {
    #[serde(default = "default_intensity")]
    intensity: f64,
  },
  Step {
    #[serde(default = "default_step_intensity")]
    intensity: f64,
  },
}

impl Default for LerpStrategy {
  fn default() -> Self {
    LerpStrategy::Linear {
      intensity: default_intensity(),
    }
  }
}

impl LerpStrategy {
  /// Apply the strategy to a segment-local parameter `t` in 0.0..1.0,
  /// returning the shaped value in 0.0..1.0.
  pub fn apply(&self, t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    match self {
      LerpStrategy::Linear { .. } => t,
      LerpStrategy::EaseIn { intensity } => t.powf(*intensity),
      LerpStrategy::EaseOut { intensity } => 1.0 - (1.0 - t).powf(*intensity),
      LerpStrategy::EaseInOut { intensity } => {
        if t < 0.5 {
          0.5 * (2.0 * t).powf(*intensity)
        } else {
          1.0 - 0.5 * (2.0 - 2.0 * t).powf(*intensity)
        }
      }
      LerpStrategy::Step { intensity } => {
        if t < *intensity {
          0.0
        } else {
          1.0
        }
      }
    }
  }
}

/// Describes how a probe metric (0.0–1.0) maps to patches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Transition {
  /// Interpolate between adjacent keyframe patches.  Each segment
  /// between consecutive patches can use a different lerp strategy.
  /// An empty `segments` vec defaults to linear for all segments.
  Gradient {
    patches: Vec<String>,
    #[serde(default)]
    segments: Vec<LerpStrategy>,
  },
  /// Select the first state whose threshold exceeds the metric.
  Discrete { states: Vec<DiscreteState> },
}

/// A single threshold/patch pair in a discrete transition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
      Transition::Gradient { patches, segments } => {
        if patches.is_empty() {
          return None;
        }
        if patches.len() == 1 {
          return library.get(&patches[0]).cloned();
        }
        let n = patches.len() - 1;
        let scaled = metric * n as f64;
        let lo_idx = (scaled.floor() as usize).min(n - 1);
        let hi_idx = lo_idx + 1;
        let local_t = scaled - lo_idx as f64;
        let strategy = segments.get(lo_idx).cloned().unwrap_or_default();
        let shaped_t = strategy.apply(local_t);
        let lo = library.get(&patches[lo_idx])?;
        let hi = library.get(&patches[hi_idx])?;
        Some(Patch::lerp(lo, hi, shaped_t))
      }
      Transition::Discrete { states } => {
        for state in states {
          if metric < state.threshold {
            return library.get(&state.patch).cloned();
          }
        }
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
      segments: vec![],
    };
    let result = t.resolve(0.0, &lib).unwrap();
    assert_eq!(result.freq, 200.0);
  }

  #[test]
  fn gradient_at_one_returns_last_patch() {
    let lib = test_library();
    let t = Transition::Gradient {
      patches: vec!["low".into(), "mid".into(), "high".into()],
      segments: vec![],
    };
    let result = t.resolve(1.0, &lib).unwrap();
    assert_eq!(result.freq, 800.0);
  }

  #[test]
  fn gradient_at_half_interpolates() {
    let lib = test_library();
    let t = Transition::Gradient {
      patches: vec!["low".into(), "high".into()],
      segments: vec![],
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
      segments: vec![],
    };
    assert!(t.resolve(0.5, &lib).is_none());
  }

  #[test]
  fn lerp_linear_is_identity() {
    let strat = LerpStrategy::Linear { intensity: 2.0 };
    assert!((strat.apply(0.0)).abs() < 1e-10);
    assert!((strat.apply(0.5) - 0.5).abs() < 1e-10);
    assert!((strat.apply(1.0) - 1.0).abs() < 1e-10);
  }

  #[test]
  fn lerp_ease_in_power_curve() {
    let strat = LerpStrategy::EaseIn { intensity: 2.0 };
    assert!((strat.apply(0.0)).abs() < 1e-10);
    assert!((strat.apply(0.5) - 0.25).abs() < 1e-10);
    assert!((strat.apply(1.0) - 1.0).abs() < 1e-10);
  }

  #[test]
  fn lerp_ease_out_power_curve() {
    let strat = LerpStrategy::EaseOut { intensity: 2.0 };
    assert!((strat.apply(0.0)).abs() < 1e-10);
    assert!((strat.apply(0.5) - 0.75).abs() < 1e-10);
    assert!((strat.apply(1.0) - 1.0).abs() < 1e-10);
  }

  #[test]
  fn lerp_ease_in_out_symmetric() {
    let strat = LerpStrategy::EaseInOut { intensity: 2.0 };
    assert!((strat.apply(0.0)).abs() < 1e-10);
    assert!((strat.apply(0.5) - 0.5).abs() < 1e-10);
    assert!((strat.apply(1.0) - 1.0).abs() < 1e-10);
    // First half: 0.5 * (2*0.25)^2 = 0.5 * 0.25 = 0.125
    assert!((strat.apply(0.25) - 0.125).abs() < 1e-10);
    // Second half: 1 - 0.5*(2 - 2*0.75)^2 = 1 - 0.5*0.25 = 0.875
    assert!((strat.apply(0.75) - 0.875).abs() < 1e-10);
  }

  #[test]
  fn lerp_step_threshold() {
    let strat = LerpStrategy::Step { intensity: 0.5 };
    assert!((strat.apply(0.0)).abs() < 1e-10);
    assert!((strat.apply(0.49)).abs() < 1e-10);
    assert!((strat.apply(0.5) - 1.0).abs() < 1e-10);
    assert!((strat.apply(1.0) - 1.0).abs() < 1e-10);
  }

  #[test]
  fn lerp_strategy_serde_roundtrip() {
    let strategies = vec![
      LerpStrategy::Linear { intensity: 2.0 },
      LerpStrategy::EaseIn { intensity: 3.0 },
      LerpStrategy::EaseOut { intensity: 1.5 },
      LerpStrategy::EaseInOut { intensity: 2.5 },
      LerpStrategy::Step { intensity: 0.7 },
    ];
    for strat in strategies {
      let json = serde_json::to_string(&strat).unwrap();
      let back: LerpStrategy = serde_json::from_str(&json).unwrap();
      // Verify the roundtrip produces same apply results.
      for i in 0..=10 {
        let t = i as f64 / 10.0;
        assert!(
          (strat.apply(t) - back.apply(t)).abs() < 1e-10,
          "roundtrip mismatch for {json} at t={t}"
        );
      }
    }
  }

  #[test]
  fn gradient_serde_roundtrip() {
    let trans = Transition::Gradient {
      patches: vec!["low".into(), "mid".into(), "high".into()],
      segments: vec![
        LerpStrategy::EaseIn { intensity: 2.0 },
        LerpStrategy::Step { intensity: 0.3 },
      ],
    };
    let json = serde_json::to_string(&trans).unwrap();
    let back: Transition = serde_json::from_str(&json).unwrap();
    let lib = test_library();
    for i in 0..=20 {
      let m = i as f64 / 20.0;
      let a = trans.resolve(m, &lib).unwrap();
      let b = back.resolve(m, &lib).unwrap();
      assert!(
        (a.freq - b.freq).abs() < 1e-10,
        "gradient roundtrip mismatch at m={m}"
      );
    }
  }

  #[test]
  fn backward_compat_old_curve_field_ignored() {
    // Old configs with a `curve` field should parse with empty
    // segments (serde ignores unknown fields on enum variants).
    let json = r#"{"type":"gradient","patches":["low","high"],"curve":2.0}"#;
    let trans: Transition = serde_json::from_str(json).unwrap();
    match &trans {
      Transition::Gradient { segments, .. } => {
        assert!(segments.is_empty());
      }
      _ => panic!("expected Gradient"),
    }
  }

  #[test]
  fn multi_segment_gradient() {
    let lib = test_library();
    let trans = Transition::Gradient {
      patches: vec!["low".into(), "mid".into(), "high".into()],
      segments: vec![
        LerpStrategy::EaseIn { intensity: 2.0 },
        LerpStrategy::EaseOut { intensity: 2.0 },
      ],
    };
    // At metric 0.25: segment 0, local t = 0.5, ease-in => 0.25
    // freq = lerp(200, 500, 0.25) = 275
    let r = trans.resolve(0.25, &lib).unwrap();
    assert!((r.freq - 275.0).abs() < 1e-10);
    // At metric 0.75: segment 1, local t = 0.5, ease-out => 0.75
    // freq = lerp(500, 800, 0.75) = 725
    let r = trans.resolve(0.75, &lib).unwrap();
    assert!((r.freq - 725.0).abs() < 1e-10);
  }
}
