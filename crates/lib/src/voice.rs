use crate::scale::PentatonicScale;
use rand::Rng;
use rand::SeedableRng;
use rand_xoshiro::Xoshiro256StarStar;
use sha2::{Digest, Sha256};
use sonify_health_voice_derive::VoiceGenerate;
use std::fmt;
use tracing::debug;

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
  #[voice_param(order = 0, range = 100.0..12000.0)]
  pub base_freq: f64,

  #[voice_param(order = 1, range = 0.0..1.0)]
  pub sine_ratio: f64,

  #[voice_param(order = 2, range = 0.0..1.0)]
  pub tri_ratio: f64,

  #[voice_param(order = 3, range = 0.0..1.0)]
  pub saw_ratio: f64,

  #[voice_param(order = 4, range = 1.0..500.0)]
  pub attack_ms: f64,

  #[voice_param(order = 5, range = 10.0..1000.0)]
  pub release_ms: f64,

  #[voice_param(order = 6, range = 0.5..4.0)]
  pub chirp_ratio: f64,

  #[voice_param(order = 7, range = -1.0..1.0)]
  pub stereo_pan: f64,

  #[voice_param(order = 8, range = 0.0..1.0)]
  pub reverb_mix: f64,

  #[voice_param(order = 9, range = 0.0..1.0)]
  pub note_seed: f64,

  #[voice_param(order = 10, range = 0.01..1.0)]
  pub echo_delay: f64,

  #[voice_param(order = 11, range = 0.0..1.0)]
  pub echo_mix: f64,

  #[voice_param(order = 12, range = 0.3..1.0)]
  pub brightness: f64,

  #[voice_param(order = 13, range = 0.2..2.0)]
  pub resonance: f64,

  #[voice_param(order = 14, range = 0.0..0.6)]
  pub sub_octave: f64,

  #[voice_param(order = 15, range = 0.0..1.0)]
  pub note_spread: f64,

  #[voice_param(order = 16, range = 0.0..20.0)]
  pub vibrato_rate: f64,

  #[voice_param(order = 17, range = 0.0..1.0)]
  pub vibrato_depth: f64,

  #[voice_param(order = 18, range = 0.0..20.0)]
  pub tremolo_rate: f64,

  #[voice_param(order = 19, range = 0.0..1.0)]
  pub tremolo_depth: f64,

  #[voice_param(order = 20, range = 0.1..0.5)]
  pub amplitude: f64,

  #[voice_param(order = 21, range = 0.0..1.0)]
  pub square_ratio: f64,

  #[voice_param(order = 22, range = 0.5..2.0)]
  pub drive: f64,

  #[voice_param(order = 23, range = 0.0..0.03)]
  pub noise_mix: f64,

  #[voice_param(order = 24, range = 0.0..0.01)]
  pub crush: f64,

  #[voice_param(order = 25, range = 0.0..0.1)]
  pub fm_ratio: f64,

  #[voice_param(order = 26, range = 0.0..0.1)]
  pub fm_depth: f64,

  #[voice_param(order = 27, range = 0.0..0.01)]
  pub downsample: f64,
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
    if let Some(v) = o.echo_delay {
      self.echo_delay = v;
    }
    if let Some(v) = o.echo_mix {
      self.echo_mix = v;
    }
    if let Some(v) = o.brightness {
      self.brightness = v;
    }
    if let Some(v) = o.resonance {
      self.resonance = v;
    }
    if let Some(v) = o.sub_octave {
      self.sub_octave = v;
    }
    if let Some(v) = o.note_spread {
      self.note_spread = v;
    }
    if let Some(v) = o.vibrato_rate {
      self.vibrato_rate = v;
    }
    if let Some(v) = o.vibrato_depth {
      self.vibrato_depth = v;
    }
    if let Some(v) = o.tremolo_rate {
      self.tremolo_rate = v;
    }
    if let Some(v) = o.tremolo_depth {
      self.tremolo_depth = v;
    }
    if let Some(v) = o.amplitude {
      self.amplitude = v;
    }
    if let Some(v) = o.square_ratio {
      self.square_ratio = v;
    }
    if let Some(v) = o.drive {
      self.drive = v;
    }
    if let Some(v) = o.noise_mix {
      self.noise_mix = v;
    }
    if let Some(v) = o.crush {
      self.crush = v;
    }
    if let Some(v) = o.fm_ratio {
      self.fm_ratio = v;
    }
    if let Some(v) = o.fm_depth {
      self.fm_depth = v;
    }
    if let Some(v) = o.downsample {
      self.downsample = v;
    }
    self
  }

  /// Generate per-boop specs for a single drone phrase.  Each drone
  /// gets its own sub-PRNG seeded from `note_seed + "drone" +
  /// drone_index`, keeping every drone's note sequence independent
  /// from heartbeats and from each other.
  ///
  /// `base_freq` is the effective drone frequency (after register
  /// and any per-drone override).  Notes are narrowed to within
  /// `note_spread` octaves of `base_freq`.
  pub fn drone_specs(
    &self,
    scale: &PentatonicScale,
    drone_index: usize,
    count: usize,
    base_freq: f64,
    slot_secs: f64,
  ) -> Vec<BoopSpec> {
    use crate::heartbeat::{BEATS_PER_BAR, MIN_NOTE_VALUE, NOTE_VALUES};

    let lo = base_freq / 2_f64.powf(self.note_spread);
    let hi = base_freq * 2_f64.powf(self.note_spread);
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

    let mut hasher = Sha256::new();
    hasher.update(self.note_seed.to_le_bytes());
    hasher.update(b"drone");
    hasher.update((drone_index as u64).to_le_bytes());
    let hash = hasher.finalize();
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&hash);
    let mut rng = Xoshiro256StarStar::from_seed(seed);

    let beat_secs = slot_secs / BEATS_PER_BAR;
    let mut raw: Vec<(f64, f64)> = (0..count)
      .map(|_| {
        let note_idx = rng.gen_range(0..notes.len());
        let note_val = NOTE_VALUES[rng.gen_range(0..NOTE_VALUES.len())];
        (notes[note_idx], note_val)
      })
      .collect();

    // Fitting loop: downshift the longest note value until the bar
    // fits, stopping at MIN_NOTE_VALUE.
    loop {
      let total_beats: f64 = raw.iter().map(|(_, v)| v).sum();
      if total_beats <= BEATS_PER_BAR {
        break;
      }
      let longest_idx = raw
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.1.partial_cmp(&b.1).unwrap())
        .map(|(i, _)| i);
      match longest_idx {
        Some(i) if raw[i].1 > MIN_NOTE_VALUE => {
          raw[i].1 /= 2.0;
        }
        _ => break,
      }
    }

    let specs: Vec<BoopSpec> = raw
      .into_iter()
      .map(|(freq, note_val)| BoopSpec {
        freq,
        duration: note_val * beat_secs,
      })
      .collect();

    debug!(
      note_seed = self.note_seed,
      drone_index,
      base_freq = format_args!("{:.1} Hz", base_freq),
      candidate_notes = nearby.len(),
      specs = ?specs.iter().map(|s| format!("{:.1}Hz/{:.3}s", s.freq, s.duration)).collect::<Vec<_>>(),
      "Drone phrase specs generated"
    );

    specs
  }

  /// Generate per-boop note and duration specs using a musical
  /// beat grid.  Each check gets its own sub-PRNG seeded from
  /// `note_seed + "boop" + check_index`, so adding boops to one
  /// check never shifts another check's note sequence.
  ///
  /// The PRNG assigns each boop a note value from
  /// `NOTE_VALUES` (whole/half/quarter/eighth).  A fitting loop
  /// then downshifts the longest notes until the total fits one
  /// bar (`BEATS_PER_BAR`), flooring at `MIN_NOTE_VALUE`.
  pub fn boop_specs(
    &self,
    scale: &PentatonicScale,
    check_count: usize,
    boops_per_check: usize,
    slot_secs: f64,
  ) -> Vec<BoopSpec> {
    use crate::heartbeat::{BEATS_PER_BAR, MIN_NOTE_VALUE, NOTE_VALUES};

    // Narrow the full scale to notes within note_spread octaves
    // of base_freq so boops sound melodically related rather than
    // scattered across 4+ octaves.
    let lo = self.base_freq / 2_f64.powf(self.note_spread);
    let hi = self.base_freq * 2_f64.powf(self.note_spread);
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

    let beat_secs = slot_secs / BEATS_PER_BAR;
    let total = check_count * boops_per_check;
    let mut raw: Vec<(f64, f64)> = Vec::with_capacity(total);

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
        let note_idx = rng.gen_range(0..notes.len());
        let note_val = NOTE_VALUES[rng.gen_range(0..NOTE_VALUES.len())];
        raw.push((notes[note_idx], note_val));
      }
    }

    // Fitting loop: downshift the longest note value until
    // the bar fits, stopping at MIN_NOTE_VALUE.
    loop {
      let total_beats: f64 = raw.iter().map(|(_, v)| v).sum();
      if total_beats <= BEATS_PER_BAR {
        break;
      }
      let longest_idx = raw
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.1.partial_cmp(&b.1).unwrap())
        .map(|(i, _)| i);
      match longest_idx {
        Some(i) if raw[i].1 > MIN_NOTE_VALUE => {
          raw[i].1 /= 2.0;
        }
        _ => break,
      }
    }

    raw
      .into_iter()
      .map(|(freq, note_val)| BoopSpec {
        freq,
        duration: note_val * beat_secs,
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
  pub chirp_ratio: Option<f64>,
  pub stereo_pan: Option<f64>,
  pub reverb_mix: Option<f64>,
  pub note_seed: Option<f64>,
  pub echo_delay: Option<f64>,
  pub echo_mix: Option<f64>,
  pub brightness: Option<f64>,
  pub resonance: Option<f64>,
  pub sub_octave: Option<f64>,
  pub note_spread: Option<f64>,
  pub vibrato_rate: Option<f64>,
  pub vibrato_depth: Option<f64>,
  pub tremolo_rate: Option<f64>,
  pub tremolo_depth: Option<f64>,
  pub amplitude: Option<f64>,
  pub square_ratio: Option<f64>,
  pub drive: Option<f64>,
  pub noise_mix: Option<f64>,
  pub crush: Option<f64>,
  pub fm_ratio: Option<f64>,
  pub fm_depth: Option<f64>,
  pub downsample: Option<f64>,
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
    writeln!(f, "reverb_mix:   {:.3}", self.reverb_mix)?;
    writeln!(f, "echo_delay:   {:.3} s", self.echo_delay)?;
    writeln!(f, "echo_mix:     {:.3}", self.echo_mix)?;
    writeln!(f, "brightness:   {:.3}", self.brightness)?;
    writeln!(f, "resonance:    {:.3}", self.resonance)?;
    writeln!(f, "sub_octave:   {:.3}", self.sub_octave)?;
    writeln!(f, "note_spread:  {:.3}", self.note_spread)?;
    writeln!(f, "vibrato_rate: {:.3} Hz", self.vibrato_rate)?;
    writeln!(f, "vibrato_depth:{:.3} st", self.vibrato_depth)?;
    writeln!(f, "tremolo_rate: {:.3} Hz", self.tremolo_rate)?;
    writeln!(f, "tremolo_depth:{:.3}", self.tremolo_depth)?;
    writeln!(f, "amplitude:    {:.3}", self.amplitude)?;
    writeln!(f, "square_ratio: {:.3}", self.square_ratio)?;
    writeln!(f, "drive:        {:.3}", self.drive)?;
    writeln!(f, "noise_mix:    {:.3}", self.noise_mix)?;
    writeln!(f, "crush:        {:.3}", self.crush)?;
    writeln!(f, "fm_ratio:     {:.3}", self.fm_ratio)?;
    writeln!(f, "fm_depth:     {:.3}", self.fm_depth)?;
    write!(f, "downsample:   {:.3}", self.downsample)
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
      assert!((100.0..12000.0).contains(&v.base_freq));
      assert!((0.0..1.0).contains(&v.sine_ratio));
      assert!((0.0..1.0).contains(&v.tri_ratio));
      assert!((0.0..1.0).contains(&v.saw_ratio));
      assert!((1.0..500.0).contains(&v.attack_ms));
      assert!((10.0..1000.0).contains(&v.release_ms));
      assert!((0.5..4.0).contains(&v.chirp_ratio));
      assert!((-1.0..1.0).contains(&v.stereo_pan));
      assert!((0.0..1.0).contains(&v.reverb_mix));
      assert!((0.0..1.0).contains(&v.note_seed));
      assert!((0.01..1.0).contains(&v.echo_delay));
      assert!((0.0..1.0).contains(&v.echo_mix));
      assert!((0.3..1.0).contains(&v.brightness));
      assert!((0.2..2.0).contains(&v.resonance));
      assert!((0.0..0.6).contains(&v.sub_octave));
      assert!((0.0..1.0).contains(&v.note_spread));
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

  #[test]
  fn drone_specs_deterministic() {
    let v = Voice::from_hostname("test");
    let scale = PentatonicScale::from_key("local");
    let s1 = v.drone_specs(&scale, 0, 3, 400.0, 4.0);
    let s2 = v.drone_specs(&scale, 0, 3, 400.0, 4.0);
    assert_eq!(s1.len(), s2.len());
    for (a, b) in s1.iter().zip(s2.iter()) {
      assert_eq!(a.freq, b.freq);
      assert_eq!(a.duration, b.duration);
    }
  }

  #[test]
  fn drone_specs_independent_across_indices() {
    let v = Voice::from_hostname("test");
    let scale = PentatonicScale::from_key("local");
    let s0 = v.drone_specs(&scale, 0, 3, 400.0, 4.0);
    let s1 = v.drone_specs(&scale, 1, 3, 400.0, 4.0);
    // Different drone indices should produce different note sequences.
    let same = s0
      .iter()
      .zip(s1.iter())
      .all(|(a, b)| a.freq == b.freq && a.duration == b.duration);
    assert!(!same, "Different drone indices should produce different specs");
  }

  #[test]
  fn boop_notes_stable_across_count_changes() {
    let v = Voice::from_hostname("test");
    let scale = PentatonicScale::from_key("local");
    // With 3 checks and 1 boop each, record each check's first note.
    let specs_1 = v.boop_specs(&scale, 3, 1, 4.0);
    // With 3 checks and 3 boops each, the first boop per check
    // must keep the same frequency.
    let specs_3 = v.boop_specs(&scale, 3, 3, 4.0);
    for check in 0..3 {
      assert_eq!(
        specs_1[check].freq,
        specs_3[check * 3].freq,
        "Check {check}'s first note shifted when boops_per_check changed"
      );
    }
  }

  #[test]
  fn fitting_algorithm_downshifts_to_fit_bar() {
    // Force two whole notes (8 beats) into a 4-beat bar.
    // The fitting loop should halve both to half notes (2 beats
    // each = 4 beats total), which fits exactly.
    let v = Voice::from_hostname("test").with_overrides(&VoiceOverrides {
      base_freq: Some(440.0),
      ..Default::default()
    });
    let scale = PentatonicScale::from_key("local");
    // With slot_secs = 4.0, beat_secs = 1.0.  Two boops gives
    // at most 8 beats raw; the fitting loop must bring it to ≤ 4.
    let specs = v.boop_specs(&scale, 1, 2, 4.0);
    let total_dur: f64 = specs.iter().map(|s| s.duration).sum();
    assert!(
      total_dur <= 4.0 + 1e-10,
      "Total duration {total_dur:.3} should fit within 4.0 s slot"
    );
    // Each note should be at least MIN_NOTE_VALUE × beat_secs = 0.5 s.
    for (i, spec) in specs.iter().enumerate() {
      assert!(
        spec.duration >= 0.5 - 1e-10,
        "Boop {i} duration {:.3} below minimum 0.5 s",
        spec.duration
      );
    }
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

    let derived = Voice::from_hostname("silicon");
    assert_eq!(derived.base_freq, expected_base_freq);
    assert_eq!(derived.sine_ratio, expected_sine_ratio);
  }
}
