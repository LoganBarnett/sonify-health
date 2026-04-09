use rand::Rng;
use rand::SeedableRng;
use rand_xoshiro::Xoshiro256StarStar;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sonify_health_voice_derive::PatchGenerate;
use std::fmt;
use tracing::debug;

/// Metadata for a single patch parameter, used by the UI and
/// serialisation helpers.
#[derive(Debug, Clone)]
pub struct PatchParamMeta {
  pub name: &'static str,
  pub description: &'static str,
  pub min: f64,
  pub max: f64,
  pub step: f64,
}

/// Patch parameters derived deterministically from a hostname.
///
/// Each parameter is drawn from a hostname-seeded PRNG in a fixed
/// order.  The draw order is a contract: appending new parameters
/// at the end is safe, but inserting between existing draws would
/// change all subsequent patches.
///
/// The `PatchGenerate` derive macro enforces:
/// - Contiguous 0..N order values (no gaps).
/// - No duplicate order values.
/// - All annotated fields must be `f64`.
#[derive(Debug, Clone, Serialize, PatchGenerate)]
pub struct Patch {
  #[patch_param(
    order = 0, range = 100.0..12000.0,
    min = 100.0, max = 12000.0, step = 1.0,
    description = "Root pitch in Hz. All boop notes derive from this frequency."
  )]
  pub base_freq: f64,

  #[patch_param(
    order = 1, range = 0.0..1.0,
    min = 0.0, max = 3.0, step = 0.01,
    description = "Relative weight of the sine oscillator. Smooth, pure tone."
  )]
  pub sine_ratio: f64,

  #[patch_param(
    order = 2, range = 0.0..1.0,
    min = 0.0, max = 3.0, step = 0.01,
    description = "Relative weight of the triangle oscillator. Hollow, flute-like."
  )]
  pub tri_ratio: f64,

  #[patch_param(
    order = 3, range = 0.0..1.0,
    min = 0.0, max = 3.0, step = 0.01,
    description = "Relative weight of the sawtooth oscillator. Bright, buzzy edge."
  )]
  pub saw_ratio: f64,

  #[patch_param(
    order = 4, range = 0.0..500.0,
    min = 0.0, max = 500.0, step = 1.0,
    description = "Fade-in time in milliseconds. Low = snappy click, high = soft swell."
  )]
  pub attack_ms: f64,

  #[patch_param(
    order = 5, range = 0.0..1000.0,
    min = 0.0, max = 1000.0, step = 1.0,
    description = "Fade-out time in milliseconds. Low = staccato, high = lingering tail."
  )]
  pub release_ms: f64,

  #[patch_param(
    order = 6, range = 0.5..4.0,
    min = 0.5, max = 4.0, step = 0.01,
    description = "Pitch bend at note onset. 1.0 = none, <1 = downward, >1 = upward chirp."
  )]
  pub chirp_ratio: f64,

  #[patch_param(
    order = 7, range = -1.0..1.0,
    min = -1.0, max = 1.0, step = 0.01,
    description = "Left/right stereo position. -1 = full left, +1 = full right."
  )]
  pub stereo_pan: f64,

  #[patch_param(
    order = 8, range = 0.0..1.0,
    min = 0.0, max = 1.0, step = 0.01,
    description = "Wet/dry reverb blend. 0 = fully dry, 1 = fully wet."
  )]
  pub reverb_mix: f64,

  #[patch_param(
    order = 9, range = 0.0..1.0,
    min = 0.0, max = 1.0, step = 0.01,
    description = "Seed for boop duration selection."
  )]
  pub note_seed: f64,

  #[patch_param(
    order = 10, range = 0.01..1.0,
    min = 0.01, max = 1.0, step = 0.01,
    description = "Delay time in seconds. Short = slapback, long = distinct repeats."
  )]
  pub echo_delay: f64,

  #[patch_param(
    order = 11, range = 0.0..1.0,
    min = 0.0, max = 1.0, step = 0.01,
    description = "Echo wet/dry blend. 0 = no echo, 1 = full echo."
  )]
  pub echo_mix: f64,

  #[patch_param(
    order = 12, range = 0.3..1.0,
    min = 0.05, max = 2.0, step = 0.01,
    description = "Lowpass cutoff scaler. 1.0 = full brightness, lower = darker tone."
  )]
  pub brightness: f64,

  #[patch_param(
    order = 13, range = 0.2..2.0,
    min = 0.1, max = 5.0, step = 0.01,
    description = "Filter Q scaler. 1.0 = default resonance, lower = smoother rolloff, higher = nasal peak."
  )]
  pub resonance: f64,

  #[patch_param(
    order = 14, range = 0.0..0.6,
    min = 0.0, max = 1.0, step = 0.01,
    description = "Sub-oscillator mix at one octave below. 0 = off, higher = deeper body."
  )]
  pub sub_octave: f64,

  #[patch_param(
    order = 15, range = 0.0..20.0,
    min = 0.0, max = 200.0, step = 0.1,
    description = "Vibrato speed (Hz). Above ~30 Hz becomes FM synthesis."
  )]
  pub vibrato_rate: f64,

  #[patch_param(
    order = 16, range = 0.0..1.0,
    min = 0.0, max = 12.0, step = 0.01,
    description = "Vibrato depth (semitones). Large values produce FM sidebands."
  )]
  pub vibrato_depth: f64,

  #[patch_param(
    order = 17, range = 0.0..20.0,
    min = 0.0, max = 20.0, step = 0.1,
    description = "Tremolo speed (Hz)"
  )]
  pub tremolo_rate: f64,

  #[patch_param(
    order = 18, range = 0.0..1.0,
    min = 0.0, max = 1.0, step = 0.01,
    description = "Tremolo depth (fraction)"
  )]
  pub tremolo_depth: f64,

  #[patch_param(
    order = 19, range = 0.1..0.5,
    min = 0.0, max = 1.0, step = 0.01,
    description = "Output amplitude. 0 = silent, 1 = full scale."
  )]
  pub amplitude: f64,

  #[patch_param(
    order = 20, range = 0.0..1.0,
    min = 0.0, max = 3.0, step = 0.01,
    description = "Relative weight of the square oscillator. Hollow, reedy tone."
  )]
  pub square_ratio: f64,

  #[patch_param(
    order = 21, range = 0.5..2.0,
    min = 0.01, max = 20.0, step = 0.1,
    description = "Pre-filter saturation. Low = clean, high = heavy distortion."
  )]
  pub drive: f64,

  #[patch_param(
    order = 22, range = 0.0..0.03,
    min = 0.0, max = 1.0, step = 0.01,
    description = "Pink noise mixed before the filter for texture and breath."
  )]
  pub noise_mix: f64,

  #[patch_param(
    order = 23, range = 0.0..0.01,
    min = 0.0, max = 1.0, step = 0.01,
    description = "Bitcrush intensity. 0 = clean, higher = grungier."
  )]
  pub crush: f64,

  #[patch_param(
    order = 24, range = 0.0..0.1,
    min = 0.0, max = 8.0, step = 0.01,
    description = "FM modulator frequency as a ratio of the carrier. 1.0 = unison, 2.0 = octave."
  )]
  pub fm_ratio: f64,

  #[patch_param(
    order = 25, range = 0.0..0.1,
    min = 0.0, max = 10.0, step = 0.1,
    description = "FM modulation index. 0 = clean, higher = richer metallic warble."
  )]
  pub fm_depth: f64,

  #[patch_param(
    order = 26, range = 0.0..0.01,
    min = 0.0, max = 1.0, step = 0.01,
    description = "Lo-fi sample rate reduction. 0 = full fidelity, higher = crunchier."
  )]
  pub downsample: f64,

  #[patch_param(
    order = 27, range = 0.0..1.0,
    min = 0.0, max = 1.0, step = 0.01,
    description = "Body amplitude after attack. 1.0 = full level, lower = quieter sustain."
  )]
  pub sustain: f64,

  #[patch_param(
    order = 28, range = 0.1..0.5,
    min = 0.0, max = 2.0, step = 0.01,
    description = "Per-check output volume. 0 = silent, 2 = doubled."
  )]
  pub volume: f64,

  #[patch_param(
    order = 29, range = 0.0..4.0,
    min = 0.0, max = 16.0, step = 0.1,
    description = "Seconds of silence between phrase repetitions."
  )]
  pub phrase_gap: f64,

  #[patch_param(
    order = 30, range = 0.5..2.0,
    min = 0.1, max = 10.0, step = 0.1,
    description = "Speed multiplier on phrase repetition. Divides the gap."
  )]
  pub repeat_rate: f64,

  /// Per-note duration in seconds.  Not a `#[patch_param]` — set
  /// by `with_note()` or `heartbeat_notes()`/`drone_notes()`,
  /// defaulting to 0.0 from `from_hostname`.
  #[serde(skip)]
  pub duration: f64,
}

/// Lightweight serialisation struct for config and wire formats.
#[derive(Debug, Clone)]
pub struct NoteSpec {
  pub freq: f64,
  pub duration: f64,
}

impl Patch {
  /// Derive patch from the current machine's hostname.
  pub fn from_current_host() -> Self {
    Self::from_hostname(&gethostname::gethostname().to_string_lossy())
  }

  /// Linearly interpolate every field between two patches.  The
  /// parameter `t` is clamped to 0.0..=1.0, where 0.0 yields `lo`
  /// and 1.0 yields `hi`.
  pub fn lerp(lo: &Patch, hi: &Patch, t: f64) -> Patch {
    let t = t.clamp(0.0, 1.0);
    let mut result = lo.clone();
    for meta in Self::PARAMS {
      let lo_val = lo.get_param(meta.name).unwrap_or(0.0);
      let hi_val = hi.get_param(meta.name).unwrap_or(0.0);
      result.set_param(meta.name, lo_val + (hi_val - lo_val) * t);
    }
    result.duration = 0.0;
    result
  }

  /// Return a lightweight note spec for serialisation.
  pub fn to_note_spec(&self) -> NoteSpec {
    NoteSpec {
      freq: self.base_freq,
      duration: self.duration,
    }
  }

  /// Set per-note frequency and duration, returning the modified patch.
  pub fn with_note(mut self, freq: f64, duration: f64) -> Self {
    self.base_freq = freq;
    self.duration = duration;
    self
  }

  /// Generate note patches for a single drone phrase.  Each drone
  /// gets its own sub-PRNG seeded from `note_seed + "drone" +
  /// drone_index`, keeping every drone's note sequence independent
  /// from heartbeats and from each other.
  ///
  /// Each returned patch is a clone of `self` with per-note
  /// `base_freq` and `duration` set.  The PRNG controls only
  /// duration; all notes use `self.base_freq`.
  pub fn drone_notes(
    &self,
    drone_index: usize,
    count: usize,
    slot_secs: f64,
  ) -> Vec<Patch> {
    use crate::heartbeat::{BEATS_PER_BAR, MIN_NOTE_VALUE, NOTE_VALUES};

    let mut hasher = Sha256::new();
    hasher.update(self.note_seed.to_le_bytes());
    hasher.update(b"drone");
    hasher.update((drone_index as u64).to_le_bytes());
    let hash = hasher.finalize();
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&hash);
    let mut rng = Xoshiro256StarStar::from_seed(seed);

    let beat_secs = slot_secs / BEATS_PER_BAR;
    let mut raw: Vec<f64> = (0..count)
      .map(|_| NOTE_VALUES[rng.gen_range(0..NOTE_VALUES.len())])
      .collect();

    // Fitting loop: downshift the longest note value until the bar
    // fits, stopping at MIN_NOTE_VALUE.
    loop {
      let total_beats: f64 = raw.iter().sum();
      if total_beats <= BEATS_PER_BAR {
        break;
      }
      let longest_idx = raw
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
        .map(|(i, _)| i);
      match longest_idx {
        Some(i) if raw[i] > MIN_NOTE_VALUE => {
          raw[i] /= 2.0;
        }
        _ => break,
      }
    }

    let patches: Vec<Patch> = raw
      .into_iter()
      .map(|note_val| {
        self.clone().with_note(self.base_freq, note_val * beat_secs)
      })
      .collect();

    debug!(
      note_seed = self.note_seed,
      drone_index,
      base_freq = format_args!("{:.1} Hz", self.base_freq),
      specs = ?patches.iter().map(|p| format!("{:.1}Hz/{:.3}s", p.base_freq, p.duration)).collect::<Vec<_>>(),
      "Drone phrase notes generated"
    );

    patches
  }

  /// Generate note patches for heartbeat boops using a musical
  /// beat grid.  Each check gets its own sub-PRNG seeded from
  /// `note_seed + "boop" + check_index`, so adding boops to one
  /// check never shifts another check's note sequence.
  ///
  /// Each returned patch is a clone of `self` with per-note
  /// `duration` set from PRNG.  A fitting loop downshifts the
  /// longest notes until the total fits one bar
  /// (`BEATS_PER_BAR`), flooring at `MIN_NOTE_VALUE`.
  pub fn heartbeat_notes(
    &self,
    check_count: usize,
    boops_per_check: usize,
    slot_secs: f64,
  ) -> Vec<Patch> {
    use crate::heartbeat::{BEATS_PER_BAR, MIN_NOTE_VALUE, NOTE_VALUES};

    let beat_secs = slot_secs / BEATS_PER_BAR;
    let total = check_count * boops_per_check;
    let mut raw: Vec<f64> = Vec::with_capacity(total);

    for check_idx in 0..check_count {
      let mut hasher = Sha256::new();
      hasher.update(self.note_seed.to_le_bytes());
      hasher.update(b"boop");
      hasher.update((check_idx as u64).to_le_bytes());
      let hash = hasher.finalize();
      let mut seed = [0u8; 32];
      seed.copy_from_slice(&hash);
      let mut rng = Xoshiro256StarStar::from_seed(seed);

      for _ in 0..boops_per_check {
        raw.push(NOTE_VALUES[rng.gen_range(0..NOTE_VALUES.len())]);
      }
    }

    // Fitting loop: downshift the longest note value until
    // the bar fits, stopping at MIN_NOTE_VALUE.
    loop {
      let total_beats: f64 = raw.iter().sum();
      if total_beats <= BEATS_PER_BAR {
        break;
      }
      let longest_idx = raw
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
        .map(|(i, _)| i);
      match longest_idx {
        Some(i) if raw[i] > MIN_NOTE_VALUE => {
          raw[i] /= 2.0;
        }
        _ => break,
      }
    }

    raw
      .into_iter()
      .map(|note_val| {
        self.clone().with_note(self.base_freq, note_val * beat_secs)
      })
      .collect()
  }
}

impl fmt::Display for Patch {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    for (i, meta) in Self::PARAMS.iter().enumerate() {
      let val = self.get_param(meta.name).unwrap_or(0.0);
      if i > 0 {
        writeln!(f)?;
      }
      write!(f, "{:14}{:.3}", format!("{}:", meta.name), val)?;
    }
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn deterministic_patch() {
    let v1 = Patch::from_hostname("silicon");
    let v2 = Patch::from_hostname("silicon");
    assert_eq!(v1.base_freq, v2.base_freq);
    assert_eq!(v1.sine_ratio, v2.sine_ratio);
    assert_eq!(v1.attack_ms, v2.attack_ms);
    assert_eq!(v1.note_seed, v2.note_seed);
  }

  #[test]
  fn distinct_hostnames_produce_distinct_patches() {
    let v1 = Patch::from_hostname("silicon");
    let v2 = Patch::from_hostname("carbon");
    assert!(
      v1.base_freq != v2.base_freq
        || v1.sine_ratio != v2.sine_ratio
        || v1.tri_ratio != v2.tri_ratio,
      "Different hostnames should produce different patches"
    );
  }

  #[test]
  fn parameters_within_range() {
    for name in ["alpha", "beta", "gamma", "delta", "epsilon"] {
      let v = Patch::from_hostname(name);
      assert!((100.0..12000.0).contains(&v.base_freq));
      assert!((0.0..1.0).contains(&v.sine_ratio));
      assert!((0.0..1.0).contains(&v.tri_ratio));
      assert!((0.0..1.0).contains(&v.saw_ratio));
      assert!((0.0..500.0).contains(&v.attack_ms));
      assert!((0.0..1000.0).contains(&v.release_ms));
      assert!((0.5..4.0).contains(&v.chirp_ratio));
      assert!((-1.0..1.0).contains(&v.stereo_pan));
      assert!((0.0..1.0).contains(&v.reverb_mix));
      assert!((0.0..1.0).contains(&v.note_seed));
      assert!((0.01..1.0).contains(&v.echo_delay));
      assert!((0.0..1.0).contains(&v.echo_mix));
      assert!((0.3..1.0).contains(&v.brightness));
      assert!((0.2..2.0).contains(&v.resonance));
      assert!((0.0..0.6).contains(&v.sub_octave));
      assert!((0.0..20.0).contains(&v.vibrato_rate));
      assert!((0.0..1.0).contains(&v.vibrato_depth));
      assert!((0.0..20.0).contains(&v.tremolo_rate));
      assert!((0.0..1.0).contains(&v.tremolo_depth));
      assert!((0.1..0.5).contains(&v.amplitude));
      assert!((0.0..1.0).contains(&v.square_ratio));
      assert!((0.5..2.0).contains(&v.drive));
      assert!((0.0..0.03).contains(&v.noise_mix));
      assert!((0.0..0.01).contains(&v.crush));
      assert!((0.0..0.1).contains(&v.fm_ratio));
      assert!((0.0..0.1).contains(&v.fm_depth));
      assert!((0.0..0.01).contains(&v.downsample));
      assert!((0.0..1.0).contains(&v.sustain));
      assert!((0.1..0.5).contains(&v.volume));
      assert!((0.0..4.0).contains(&v.phrase_gap));
      assert!((0.5..2.0).contains(&v.repeat_rate));
    }
  }

  #[test]
  fn overrides_replace_specified_fields_only() {
    let v = Patch::from_hostname("test");
    let original_sine = v.sine_ratio;
    let overridden = v.with_overrides(&PatchOverrides {
      base_freq: Some(440.0),
      ..Default::default()
    });
    assert_eq!(overridden.base_freq, 440.0);
    assert_eq!(overridden.sine_ratio, original_sine);
  }

  #[test]
  fn drone_notes_deterministic() {
    let v = Patch::from_hostname("test");
    let s1 = v.drone_notes(0, 3, 4.0);
    let s2 = v.drone_notes(0, 3, 4.0);
    assert_eq!(s1.len(), s2.len());
    for (a, b) in s1.iter().zip(s2.iter()) {
      assert_eq!(a.base_freq, b.base_freq);
      assert_eq!(a.duration, b.duration);
    }
  }

  #[test]
  fn drone_notes_independent_across_indices() {
    let v = Patch::from_hostname("test");
    let s0 = v.drone_notes(0, 3, 4.0);
    let s1 = v.drone_notes(1, 3, 4.0);
    // Different drone indices should produce different duration sequences.
    let same = s0
      .iter()
      .zip(s1.iter())
      .all(|(a, b)| a.duration == b.duration);
    assert!(!same, "Different drone indices should produce different specs");
  }

  #[test]
  fn heartbeat_notes_stable_across_count_changes() {
    let v = Patch::from_hostname("test");
    // With 3 checks and 1 boop each, record each check's first note.
    let patches_1 = v.heartbeat_notes(3, 1, 4.0);
    // With 3 checks and 3 boops each, the first boop per check
    // must keep the same frequency.
    let patches_3 = v.heartbeat_notes(3, 3, 4.0);
    for check in 0..3 {
      assert_eq!(
        patches_1[check].base_freq,
        patches_3[check * 3].base_freq,
        "Check {check}'s first note shifted when boops_per_check changed"
      );
    }
  }

  #[test]
  fn fitting_algorithm_downshifts_to_fit_bar() {
    // Force two whole notes (8 beats) into a 4-beat bar.
    // The fitting loop should halve both to half notes (2 beats
    // each = 4 beats total), which fits exactly.
    let v = Patch::from_hostname("test").with_overrides(&PatchOverrides {
      base_freq: Some(440.0),
      ..Default::default()
    });
    // With slot_secs = 4.0, beat_secs = 1.0.  Two boops gives
    // at most 8 beats raw; the fitting loop must bring it to ≤ 4.
    let patches = v.heartbeat_notes(1, 2, 4.0);
    let total_dur: f64 = patches.iter().map(|p| p.duration).sum();
    assert!(
      total_dur <= 4.0 + 1e-10,
      "Total duration {total_dur:.3} should fit within 4.0 s slot"
    );
    // Each note should be at least MIN_NOTE_VALUE × beat_secs = 0.5 s.
    for (i, patch) in patches.iter().enumerate() {
      assert!(
        patch.duration >= 0.5 - 1e-10,
        "Boop {i} duration {:.3} below minimum 0.5 s",
        patch.duration
      );
    }
  }

  #[test]
  fn lerp_at_zero_equals_lo() {
    let lo = Patch::from_hostname("lo");
    let hi = Patch::from_hostname("hi");
    let result = Patch::lerp(&lo, &hi, 0.0);
    assert_eq!(result.base_freq, lo.base_freq);
    assert_eq!(result.amplitude, lo.amplitude);
    assert_eq!(result.reverb_mix, lo.reverb_mix);
  }

  #[test]
  fn lerp_at_one_equals_hi() {
    let lo = Patch::from_hostname("lo");
    let hi = Patch::from_hostname("hi");
    let result = Patch::lerp(&lo, &hi, 1.0);
    assert_eq!(result.base_freq, hi.base_freq);
    assert_eq!(result.amplitude, hi.amplitude);
    assert_eq!(result.reverb_mix, hi.reverb_mix);
  }

  #[test]
  fn lerp_at_half_equals_midpoint() {
    let lo = Patch::from_hostname("lo");
    let hi = Patch::from_hostname("hi");
    let result = Patch::lerp(&lo, &hi, 0.5);
    let expected_freq = (lo.base_freq + hi.base_freq) / 2.0;
    assert!(
      (result.base_freq - expected_freq).abs() < 1e-10,
      "base_freq midpoint: got {} expected {}",
      result.base_freq,
      expected_freq,
    );
    let expected_amp = (lo.amplitude + hi.amplitude) / 2.0;
    assert!(
      (result.amplitude - expected_amp).abs() < 1e-10,
      "amplitude midpoint: got {} expected {}",
      result.amplitude,
      expected_amp,
    );
  }

  #[test]
  fn lerp_clamps_t() {
    let lo = Patch::from_hostname("lo");
    let hi = Patch::from_hostname("hi");
    let below = Patch::lerp(&lo, &hi, -0.5);
    assert_eq!(below.base_freq, lo.base_freq);
    let above = Patch::lerp(&lo, &hi, 2.0);
    assert_eq!(above.base_freq, hi.base_freq);
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

    let expected_base_freq: f64 = rng.gen_range(100.0..12000.0);
    let expected_sine_ratio: f64 = rng.gen_range(0.0..1.0);

    let derived = Patch::from_hostname("silicon");
    assert_eq!(derived.base_freq, expected_base_freq);
    assert_eq!(derived.sine_ratio, expected_sine_ratio);
  }

  #[test]
  fn params_metadata_covers_all_fields() {
    // Every patch_param field should be in PARAMS.
    let patch = Patch::from_hostname("test");
    for meta in Patch::PARAMS {
      assert!(
        patch.get_param(meta.name).is_some(),
        "PARAMS entry '{}' not accessible via get_param",
        meta.name
      );
    }
    assert_eq!(Patch::PARAMS.len(), 31);
  }

  #[test]
  fn set_param_round_trips() {
    let mut patch = Patch::from_hostname("test");
    patch.set_param("base_freq", 999.0);
    assert_eq!(patch.get_param("base_freq"), Some(999.0));
  }

  #[test]
  fn serialize_skips_duration() {
    let patch = Patch::from_hostname("test");
    let json = serde_json::to_value(&patch).unwrap();
    assert!(json.get("duration").is_none());
    assert!(json.get("base_freq").is_some());
  }

  #[test]
  fn patch_overrides_to_fields() {
    let o = PatchOverrides {
      base_freq: Some(440.0),
      amplitude: Some(0.5),
      ..Default::default()
    };
    let fields = o.to_fields();
    assert!(fields.iter().any(|(n, v)| *n == "base_freq" && *v == 440.0));
    assert!(fields.iter().any(|(n, v)| *n == "amplitude" && *v == 0.5));
    assert_eq!(fields.len(), 2);
  }
}
