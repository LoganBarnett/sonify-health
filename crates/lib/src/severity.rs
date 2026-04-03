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

impl Severity {
  /// Pitch multiplier relative to the voice's base frequency.
  /// Uses just intonation ratios for harmonic compatibility.
  pub fn pitch_ratio(self) -> f64 {
    match self {
      Severity::Healthy => 1.0,
      Severity::Degraded => 6.0 / 5.0, // minor third
      Severity::Down => 45.0 / 32.0,   // tritone
    }
  }

  /// Amplitude scaling — distress raises energy.
  pub fn amplitude(self) -> f64 {
    match self {
      Severity::Healthy => 0.3,
      Severity::Degraded => 0.5,
      Severity::Down => 0.8,
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
  fn severity_ordering() {
    assert!(Severity::Healthy.amplitude() < Severity::Degraded.amplitude());
    assert!(Severity::Degraded.amplitude() < Severity::Down.amplitude());
    assert!(Severity::Degraded.pitch_ratio() > Severity::Healthy.pitch_ratio());
    assert!(Severity::Down.pitch_ratio() > Severity::Degraded.pitch_ratio());
  }
}
