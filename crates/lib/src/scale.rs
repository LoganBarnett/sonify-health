use sha2::{Digest, Sha256};

/// Just intonation pentatonic ratios relative to root.
const PENTATONIC_RATIOS: [f64; 5] =
  [1.0, 9.0 / 8.0, 5.0 / 4.0, 3.0 / 2.0, 5.0 / 3.0];

/// Frequency bounds for generated scale notes.
const FREQ_LO: f64 = 60.0;
const FREQ_HI: f64 = 1200.0;

/// A2 reference pitch used as the base for chromatic root calculation.
const A2: f64 = 110.0;

/// Pentatonic scale derived from a string key, spanning 60–1200 Hz.
#[derive(Debug, Clone)]
pub struct PentatonicScale {
  notes: Vec<f64>,
}

impl PentatonicScale {
  /// Build a pentatonic scale from an arbitrary string key.
  ///
  /// SHA-256 hashes the key, uses the first byte mod 12 as a
  /// chromatic offset from A2, then fills the frequency range with
  /// just-intonation pentatonic intervals.
  pub fn from_key(key: &str) -> Self {
    let hash = Sha256::digest(key.as_bytes());
    let offset = (hash[0] % 12) as f64;
    let root = A2 * 2_f64.powf(offset / 12.0);

    let mut notes = Vec::new();
    for &ratio in &PENTATONIC_RATIOS {
      let base = root * ratio;
      // Walk octaves down to find the lowest instance in range.
      let mut f = base;
      while f > FREQ_LO {
        f /= 2.0;
      }
      // Walk octaves up, collecting every instance inside the range.
      while f < FREQ_HI {
        f *= 2.0;
        if f >= FREQ_LO && f <= FREQ_HI {
          notes.push(f);
        }
      }
    }

    notes.sort_by(|a, b| a.partial_cmp(b).unwrap());
    notes.dedup_by(|a, b| (*a - *b).abs() < 0.01);

    Self { notes }
  }

  /// Access the raw frequency table.
  pub fn notes(&self) -> &[f64] {
    &self.notes
  }

  /// Snap a frequency to the nearest pentatonic note.
  pub fn snap(&self, freq: f64) -> f64 {
    let idx = self
      .notes
      .binary_search_by(|n| n.partial_cmp(&freq).unwrap());
    match idx {
      Ok(i) => self.notes[i],
      Err(i) => {
        let below = if i > 0 { Some(self.notes[i - 1]) } else { None };
        let above = if i < self.notes.len() {
          Some(self.notes[i])
        } else {
          None
        };
        match (below, above) {
          (Some(b), Some(a)) => {
            if (freq - b).abs() <= (a - freq).abs() {
              b
            } else {
              a
            }
          }
          (Some(b), None) => b,
          (None, Some(a)) => a,
          (None, None) => freq,
        }
      }
    }
  }
}

/// Extract the shared domain from a hostname by stripping the first
/// dot-separated label.  Hostnames without dots fall back to "local".
pub fn domain_from_hostname(hostname: &str) -> String {
  hostname
    .find('.')
    .map(|i| &hostname[i + 1..])
    .filter(|d| !d.is_empty())
    .unwrap_or("local")
    .to_string()
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn snap_returns_exact_note() {
    let scale = PentatonicScale::from_key("test");
    let note = scale.notes[2];
    assert!(
      (scale.snap(note) - note).abs() < f64::EPSILON,
      "Snapping an exact note should return that note unchanged."
    );
  }

  #[test]
  fn snap_returns_nearest() {
    let scale = PentatonicScale::from_key("test");
    let mid = (scale.notes[0] + scale.notes[1]) / 2.0;
    let snapped = scale.snap(mid);
    assert!(
      snapped == scale.notes[0] || snapped == scale.notes[1],
      "Off-scale frequency should snap to one of its two neighbours."
    );
  }

  #[test]
  fn same_domain_same_scale() {
    let s1 = PentatonicScale::from_key("lab.example.com");
    let s2 = PentatonicScale::from_key("lab.example.com");
    assert_eq!(s1.notes, s2.notes);
  }

  #[test]
  fn domain_extraction() {
    assert_eq!(
      domain_from_hostname("gpu-1.lab.example.com"),
      "lab.example.com"
    );
    assert_eq!(domain_from_hostname("foo.local"), "local");
    assert_eq!(domain_from_hostname("silicon"), "local");
  }

  #[test]
  fn different_keys_different_roots() {
    let s1 = PentatonicScale::from_key("lab.example.com");
    let s2 = PentatonicScale::from_key("prod.acme.io");
    assert_ne!(
      s1.notes, s2.notes,
      "Distinct domains should produce distinct scales."
    );
  }
}
