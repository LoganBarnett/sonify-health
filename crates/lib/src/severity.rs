use std::fmt;
use std::str::FromStr;
use thiserror::Error;

/// Health severity level for a single check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
  Healthy = 0,
  Degraded = 1,
  Down = 2,
}

/// Timbre profile for a single boop, driven by severity.
#[derive(Debug, Clone, Copy)]
pub struct BoopProfile {
  /// Detuning in cents applied alternately +/- to successive boops.
  /// Creates beating and lost harmonization at higher values.
  pub detune_cents: f64,
  /// Amplitude scaling (0.08 gentle background to 0.18 noticeable).
  pub amplitude: f64,
  /// Saw-wave bleed-in weight (0.0 pure voice blend to 0.6 buzzy).
  pub harshness: f64,
  /// Lowpass cutoff in Hz (4000 open/bright to 1800 narrow/nasal).
  pub filter_cutoff: f64,
  /// Lowpass resonance Q (0.5 flat to 2.0 honky resonant peak).
  pub filter_q: f64,
}

impl Severity {
  /// Severity-driven timbre profile replacing the former pitch-only
  /// model.  Healthy sounds warm and chipper; degraded adds nasal
  /// edge and beating; down is harsh, dissonant, and attention-
  /// grabbing without becoming a klaxon.
  pub fn profile(self) -> BoopProfile {
    match self {
      Severity::Healthy => BoopProfile {
        detune_cents: 0.0,
        amplitude: 0.08,
        harshness: 0.0,
        filter_cutoff: 4000.0,
        filter_q: 0.5,
      },
      Severity::Degraded => BoopProfile {
        detune_cents: 12.0,
        amplitude: 0.14,
        harshness: 0.25,
        filter_cutoff: 2800.0,
        filter_q: 1.2,
      },
      Severity::Down => BoopProfile {
        detune_cents: 25.0,
        amplitude: 0.18,
        harshness: 0.6,
        filter_cutoff: 1800.0,
        filter_q: 2.0,
      },
    }
  }
}

#[derive(Debug, Error)]
#[error(
  "Invalid severity value {0}: must be 0 (healthy), \
   1 (degraded), or 2 (down)"
)]
pub struct SeverityParseError(String);

impl TryFrom<u8> for Severity {
  type Error = SeverityParseError;

  fn try_from(value: u8) -> Result<Self, Self::Error> {
    match value {
      0 => Ok(Severity::Healthy),
      1 => Ok(Severity::Degraded),
      2 => Ok(Severity::Down),
      other => Err(SeverityParseError(other.to_string())),
    }
  }
}

impl FromStr for Severity {
  type Err = SeverityParseError;

  fn from_str(s: &str) -> Result<Self, Self::Err> {
    match s {
      "0" | "healthy" => Ok(Severity::Healthy),
      "1" | "degraded" => Ok(Severity::Degraded),
      "2" | "down" => Ok(Severity::Down),
      other => Err(SeverityParseError(other.to_string())),
    }
  }
}

impl fmt::Display for Severity {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Severity::Healthy => write!(f, "healthy"),
      Severity::Degraded => write!(f, "degraded"),
      Severity::Down => write!(f, "down"),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn round_trip_from_u8() {
    for v in 0..=2u8 {
      let severity = Severity::try_from(v).unwrap();
      assert_eq!(severity as u8, v);
    }
  }

  #[test]
  fn invalid_u8_rejected() {
    assert!(Severity::try_from(3).is_err());
    assert!(Severity::try_from(255).is_err());
  }

  #[test]
  fn round_trip_from_str() {
    for (s, expected) in [
      ("0", Severity::Healthy),
      ("1", Severity::Degraded),
      ("2", Severity::Down),
    ] {
      assert_eq!(s.parse::<Severity>().unwrap(), expected);
    }
  }

  #[test]
  fn named_from_str() {
    assert_eq!("healthy".parse::<Severity>().unwrap(), Severity::Healthy);
    assert_eq!("degraded".parse::<Severity>().unwrap(), Severity::Degraded);
    assert_eq!("down".parse::<Severity>().unwrap(), Severity::Down);
  }

  #[test]
  fn invalid_str_rejected() {
    assert!("3".parse::<Severity>().is_err());
    assert!("bad".parse::<Severity>().is_err());
  }

  #[test]
  fn profile_amplitude_rises_with_severity() {
    let h = Severity::Healthy.profile();
    let d = Severity::Degraded.profile();
    let w = Severity::Down.profile();
    assert!(h.amplitude < d.amplitude);
    assert!(d.amplitude < w.amplitude);
  }

  #[test]
  fn profile_harshness_rises_with_severity() {
    let h = Severity::Healthy.profile();
    let d = Severity::Degraded.profile();
    let w = Severity::Down.profile();
    assert!(h.harshness < d.harshness);
    assert!(d.harshness < w.harshness);
  }

  #[test]
  fn profile_detune_rises_with_severity() {
    let h = Severity::Healthy.profile();
    let d = Severity::Degraded.profile();
    let w = Severity::Down.profile();
    assert!(h.detune_cents < d.detune_cents);
    assert!(d.detune_cents < w.detune_cents);
  }

  #[test]
  fn profile_cutoff_drops_with_severity() {
    let h = Severity::Healthy.profile();
    let d = Severity::Degraded.profile();
    let w = Severity::Down.profile();
    assert!(h.filter_cutoff > d.filter_cutoff);
    assert!(d.filter_cutoff > w.filter_cutoff);
  }

  #[test]
  fn profile_q_rises_with_severity() {
    let h = Severity::Healthy.profile();
    let d = Severity::Degraded.profile();
    let w = Severity::Down.profile();
    assert!(h.filter_q < d.filter_q);
    assert!(d.filter_q < w.filter_q);
  }
}
