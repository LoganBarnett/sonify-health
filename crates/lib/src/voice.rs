use crate::scale::PentatonicScale;
use rand::Rng;
use rand::SeedableRng;
use rand_xoshiro::Xoshiro256StarStar;
use sha2::{Digest, Sha256};
use sonify_health_voice_derive::VoiceGenerate;
use std::fmt;

/// Voice parameters derived deterministically from a hostname.
///
/// Each parameter is drawn from a hostname-seeded PRNG in a fixed
/// order.  The draw order is a contract: appending new parameters
/// at the end is safe, but inserting between existing draws would
/// change all subsequent voices.
///
/// The `VoiceGenerate` derive macro enforces:
/// - Contiguous 0..N order values (no gaps).
/// - No duplicate order values.
/// - All annotated fields must be `f64`.
#[derive(Debug, Clone, VoiceGenerate)]
pub struct Voice {
  #[voice_param(order = 0, range = 100.0..600.0)]
  pub base_freq: f64,

  #[voice_param(order = 1, range = 0.5..1.0)]
  pub sine_ratio: f64,

  #[voice_param(order = 2, range = 0.0..0.3)]
  pub tri_ratio: f64,

  #[voice_param(order = 3, range = 0.0..0.15)]
  pub saw_ratio: f64,

  #[voice_param(order = 4, range = 20.0..80.0)]
  pub attack_ms: f64,

  #[voice_param(order = 5, range = 80.0..250.0)]
  pub release_ms: f64,

  #[voice_param(order = 6, range = 0.4..0.6)]
  pub boop1_ratio: f64,

  #[voice_param(order = 7, range = 0.2..0.3)]
  pub boop2_ratio: f64,

  #[voice_param(order = 8, range = 1.0..1.5)]
  pub chirp_ratio: f64,

  #[voice_param(order = 9, range = -0.3..0.3)]
  pub stereo_pan: f64,

  #[voice_param(order = 10, range = 0.3..0.6)]
  pub reverb_mix: f64,

  #[voice_param(order = 11, range = 0.0..1.0)]
  pub note_seed: f64,
}

/// Per-boop specification: frequency and duration.
#[derive(Debug, Clone)]
pub struct BoopSpec {
  pub freq: f64,
  pub duration: f64,
}

impl Voice {
  /// Derive voice from the current machine's hostname.
  pub fn from_current_host() -> Self {
    Self::from_hostname(&gethostname::gethostname().to_string_lossy())
  }

  /// Snap base_freq to the nearest note in the pentatonic scale
  /// derived from the given key.  Applied after PRNG generation and
  /// overrides so it does not disturb draw order.
  pub fn with_scale(mut self, scale_key: &str) -> Self {
    let scale = crate::scale::PentatonicScale::from_key(scale_key);
    self.base_freq = scale.snap(self.base_freq);
    self
  }

  /// Apply overrides, replacing only the specified fields.
  pub fn with_overrides(mut self, o: &VoiceOverrides) -> Self {
    if let Some(v) = o.base_freq {
      self.base_freq = v;
    }
    if let Some(v) = o.sine_ratio {
      self.sine_ratio = v;
    }
    if let Some(v) = o.tri_ratio {
      self.tri_ratio = v;
    }
    if let Some(v) = o.saw_ratio {
      self.saw_ratio = v;
    }
    if let Some(v) = o.attack_ms {
      self.attack_ms = v;
    }
    if let Some(v) = o.release_ms {
      self.release_ms = v;
    }
    if let Some(v) = o.boop1_ratio {
      self.boop1_ratio = v;
    }
    if let Some(v) = o.boop2_ratio {
      self.boop2_ratio = v;
    }
    if let Some(v) = o.chirp_ratio {
      self.chirp_ratio = v;
    }
    if let Some(v) = o.stereo_pan {
      self.stereo_pan = v;
    }
    if let Some(v) = o.reverb_mix {
      self.reverb_mix = v;
    }
    if let Some(v) = o.note_seed {
      self.note_seed = v;
    }
    self
  }

  /// Generate per-boop note and duration specs from a sub-PRNG
  /// seeded by `note_seed`.  The sub-PRNG always draws a drone
  /// note first so heartbeat draws are stable regardless of drone
  /// configuration.
  pub fn boop_specs(
    &self,
    scale: &PentatonicScale,
    count: usize,
    total_boop_time: f64,
  ) -> Vec<BoopSpec> {
    let hash = Sha256::digest(self.note_seed.to_le_bytes());
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&hash);
    let mut rng = Xoshiro256StarStar::from_seed(seed);

    // Narrow the full scale to notes within one octave of
    // base_freq so boops sound melodically related rather than
    // scattered across 4+ octaves.
    let lo = self.base_freq / 2.0;
    let hi = self.base_freq * 2.0;
    let nearby: Vec<f64> = scale
      .notes()
      .iter()
      .copied()
      .filter(|&n| n >= lo && n <= hi)
      .collect();
    let notes = if nearby.is_empty() {
      scale.notes()
    } else {
      &nearby
    };

    // Draw 0: drone note index (always consumed, discarded).
    let _drone_idx: usize = rng.gen_range(0..notes.len());

    let duration_weights: [f64; 3] = [1.0, 2.0, 4.0];
    let mut raw: Vec<(f64, f64)> = Vec::with_capacity(count);
    let mut total_weight = 0.0;

    for _ in 0..count {
      let note_idx = rng.gen_range(0..notes.len());
      let weight = duration_weights[rng.gen_range(0..3usize)];
      total_weight += weight;
      raw.push((notes[note_idx], weight));
    }

    raw
      .into_iter()
      .map(|(freq, weight)| BoopSpec {
        freq,
        duration: weight / total_weight * total_boop_time,
      })
      .collect()
  }
}

/// Optional overrides for voice parameters from configuration.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct VoiceOverrides {
  pub scale_key: Option<String>,
  pub base_freq: Option<f64>,
  pub sine_ratio: Option<f64>,
  pub tri_ratio: Option<f64>,
  pub saw_ratio: Option<f64>,
  pub attack_ms: Option<f64>,
  pub release_ms: Option<f64>,
  pub boop1_ratio: Option<f64>,
  pub boop2_ratio: Option<f64>,
  pub chirp_ratio: Option<f64>,
  pub stereo_pan: Option<f64>,
  pub reverb_mix: Option<f64>,
  pub note_seed: Option<f64>,
}

impl fmt::Display for Voice {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    writeln!(f, "base_freq:    {:.1} Hz", self.base_freq)?;
    writeln!(f, "sine_ratio:   {:.3}", self.sine_ratio)?;
    writeln!(f, "tri_ratio:    {:.3}", self.tri_ratio)?;
    writeln!(f, "saw_ratio:    {:.3}", self.saw_ratio)?;
    writeln!(f, "attack_ms:    {:.1} ms", self.attack_ms)?;
    writeln!(f, "release_ms:   {:.1} ms", self.release_ms)?;
    writeln!(f, "chirp_ratio:  {:.3}", self.chirp_ratio)?;
    writeln!(f, "stereo_pan:   {:.3}", self.stereo_pan)?;
    write!(f, "reverb_mix:   {:.3}", self.reverb_mix)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn deterministic_voice() {
    let v1 = Voice::from_hostname("silicon");
    let v2 = Voice::from_hostname("silicon");
    assert_eq!(v1.base_freq, v2.base_freq);
    assert_eq!(v1.sine_ratio, v2.sine_ratio);
    assert_eq!(v1.attack_ms, v2.attack_ms);
    assert_eq!(v1.note_seed, v2.note_seed);
  }

  #[test]
  fn distinct_hostnames_produce_distinct_voices() {
    let v1 = Voice::from_hostname("silicon");
    let v2 = Voice::from_hostname("carbon");
    assert!(
      v1.base_freq != v2.base_freq
        || v1.sine_ratio != v2.sine_ratio
        || v1.tri_ratio != v2.tri_ratio,
      "Different hostnames should produce different voices"
    );
  }

  #[test]
  fn parameters_within_range() {
    for name in ["alpha", "beta", "gamma", "delta", "epsilon"] {
      let v = Voice::from_hostname(name);
      assert!((100.0..600.0).contains(&v.base_freq));
      assert!((0.5..1.0).contains(&v.sine_ratio));
      assert!((0.0..0.3).contains(&v.tri_ratio));
      assert!((0.0..0.15).contains(&v.saw_ratio));
      assert!((20.0..80.0).contains(&v.attack_ms));
      assert!((80.0..250.0).contains(&v.release_ms));
      assert!((0.4..0.6).contains(&v.boop1_ratio));
      assert!((0.2..0.3).contains(&v.boop2_ratio));
      assert!((1.0..1.5).contains(&v.chirp_ratio));
      assert!((-0.3..0.3).contains(&v.stereo_pan));
      assert!((0.3..0.6).contains(&v.reverb_mix));
      assert!((0.0..1.0).contains(&v.note_seed));
    }
  }

  #[test]
  fn overrides_replace_specified_fields_only() {
    let v = Voice::from_hostname("test");
    let original_sine = v.sine_ratio;
    let overridden = v.with_overrides(&VoiceOverrides {
      base_freq: Some(440.0),
      ..Default::default()
    });
    assert_eq!(overridden.base_freq, 440.0);
    assert_eq!(overridden.sine_ratio, original_sine);
  }

  /// Golden test: the derive macro must produce the same values
  /// as the original manual implementation for known hostnames.
  #[test]
  fn golden_values_for_silicon() {
    use rand::Rng;
    use rand::SeedableRng;
    use rand_xoshiro::Xoshiro256StarStar;
    use sha2::{Digest, Sha256};

    // Manual reference implementation.
    let hash = Sha256::digest(b"silicon");
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&hash);
    let mut rng = Xoshiro256StarStar::from_seed(seed);

    let expected_base_freq: f64 = rng.gen_range(100.0..600.0);
    let expected_sine_ratio: f64 = rng.gen_range(0.5..1.0);

    let derived = Voice::from_hostname("silicon");
    assert_eq!(derived.base_freq, expected_base_freq);
    assert_eq!(derived.sine_ratio, expected_sine_ratio);
  }
}
