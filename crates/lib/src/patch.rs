use serde::{Deserialize, Serialize};
use sonify_health_voice_derive::PatchGenerate;
use std::fmt;

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

/// A named, static sound definition.
///
/// Patches define every parameter of a single synthesised note.
/// They are stored in a `PatchLibrary` (built-in presets plus user
/// overrides).  `Transition::resolve()` interpolates or selects
/// patches based on a probe metric.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, PatchGenerate)]
#[serde(default)]
pub struct Patch {
  #[patch_param(
    min = 20.0,
    max = 12000.0,
    step = 1.0,
    description = "Pitch in Hz."
  )]
  pub freq: f64,

  #[patch_param(
    min = 0.01,
    max = 5.0,
    step = 0.01,
    description = "Note duration in seconds."
  )]
  pub duration: f64,

  #[patch_param(
    min = 0.0,
    max = 3.0,
    step = 0.01,
    description = "Relative weight of the sine oscillator. Smooth, pure tone."
  )]
  pub sine_ratio: f64,

  #[patch_param(
    min = 0.0,
    max = 3.0,
    step = 0.01,
    description = "Relative weight of the triangle oscillator. Hollow, flute-like."
  )]
  pub tri_ratio: f64,

  #[patch_param(
    min = 0.0,
    max = 3.0,
    step = 0.01,
    description = "Relative weight of the sawtooth oscillator. Bright, buzzy edge."
  )]
  pub saw_ratio: f64,

  #[patch_param(
    min = 0.0,
    max = 3.0,
    step = 0.01,
    description = "Relative weight of the square oscillator. Hollow, reedy tone."
  )]
  pub square_ratio: f64,

  #[patch_param(
    min = 0.0,
    max = 500.0,
    step = 1.0,
    description = "Fade-in time in milliseconds. Low = snappy click, high = soft swell."
  )]
  pub attack_ms: f64,

  #[patch_param(
    min = 0.0,
    max = 2000.0,
    step = 1.0,
    description = "Decay time in milliseconds. Slopes from attack peak to sustain level."
  )]
  pub decay_ms: f64,

  #[patch_param(
    min = 0.0,
    max = 1000.0,
    step = 1.0,
    description = "Fade-out time in milliseconds. Low = staccato, high = lingering tail."
  )]
  pub release_ms: f64,

  #[patch_param(
    min = 0.0,
    max = 1.0,
    step = 0.01,
    description = "Body amplitude after attack. 1.0 = full level, lower = quieter sustain."
  )]
  pub sustain: f64,

  #[patch_param(
    min = 0.5,
    max = 4.0,
    step = 0.01,
    description = "Pitch bend at note onset. 1.0 = none, <1 = downward, >1 = upward chirp."
  )]
  pub chirp_ratio: f64,

  #[patch_param(
    min = -1.0, max = 1.0, step = 0.01,
    description = "Left/right stereo position. -1 = full left, +1 = full right."
  )]
  pub stereo_pan: f64,

  #[patch_param(
    min = 0.0,
    max = 1.0,
    step = 0.01,
    description = "Wet/dry reverb blend. 0 = fully dry, 1 = fully wet."
  )]
  pub reverb_mix: f64,

  #[patch_param(
    min = 0.01,
    max = 1.0,
    step = 0.01,
    description = "Delay time in seconds. Short = slapback, long = distinct repeats."
  )]
  pub echo_delay: f64,

  #[patch_param(
    min = 0.0,
    max = 1.0,
    step = 0.01,
    description = "Echo wet/dry blend. 0 = no echo, 1 = full echo."
  )]
  pub echo_mix: f64,

  #[patch_param(
    min = 0.05,
    max = 2.0,
    step = 0.01,
    description = "Lowpass cutoff scaler. 1.0 = full brightness, lower = darker tone."
  )]
  pub brightness: f64,

  #[patch_param(
    min = 0.1,
    max = 5.0,
    step = 0.01,
    description = "Filter Q scaler. 1.0 = default resonance, lower = smoother rolloff, higher = nasal peak."
  )]
  pub resonance: f64,

  #[patch_param(
    min = 0.0,
    max = 2000.0,
    step = 1.0,
    description = "Highpass filter cutoff in Hz. 0 = off, higher = cuts more low frequencies."
  )]
  pub highpass: f64,

  #[patch_param(
    min = 0.0,
    max = 1.0,
    step = 0.01,
    description = "Sub-oscillator mix at one octave below. 0 = off, higher = deeper body."
  )]
  pub sub_octave: f64,

  #[patch_param(
    min = 0.0,
    max = 200.0,
    step = 0.1,
    description = "Vibrato speed (Hz). Above ~30 Hz becomes FM synthesis."
  )]
  pub vibrato_rate: f64,

  #[patch_param(
    min = 0.0,
    max = 12.0,
    step = 0.01,
    description = "Vibrato depth (semitones). Large values produce FM sidebands."
  )]
  pub vibrato_depth: f64,

  #[patch_param(
    min = 0.0,
    max = 20.0,
    step = 0.1,
    description = "Tremolo speed (Hz)."
  )]
  pub tremolo_rate: f64,

  #[patch_param(
    min = 0.0,
    max = 1.0,
    step = 0.01,
    description = "Tremolo depth (fraction)."
  )]
  pub tremolo_depth: f64,

  #[patch_param(
    min = 0.0,
    max = 1.0,
    step = 0.01,
    description = "Output amplitude. 0 = silent, 1 = full scale."
  )]
  pub amplitude: f64,

  #[patch_param(
    min = 0.01,
    max = 20.0,
    step = 0.1,
    description = "Pre-filter saturation. Low = clean, high = heavy distortion."
  )]
  pub drive: f64,

  #[patch_param(
    min = 0.0,
    max = 1.0,
    step = 0.01,
    description = "Pink noise mixed before the filter for texture and breath."
  )]
  pub noise_mix: f64,

  #[patch_param(
    min = 0.0,
    max = 1.0,
    step = 0.01,
    description = "Bitcrush intensity. 0 = clean, higher = grungier."
  )]
  pub crush: f64,

  #[patch_param(
    min = 0.0,
    max = 8.0,
    step = 0.01,
    description = "FM modulator frequency as a ratio of the carrier. 1.0 = unison, 2.0 = octave."
  )]
  pub fm_ratio: f64,

  #[patch_param(
    min = 0.0,
    max = 10.0,
    step = 0.1,
    description = "FM modulation index. 0 = clean, higher = richer metallic warble."
  )]
  pub fm_depth: f64,

  #[patch_param(
    min = 0.0,
    max = 1.0,
    step = 0.01,
    description = "Lo-fi sample rate reduction. 0 = full fidelity, higher = crunchier."
  )]
  pub downsample: f64,

  #[patch_param(
    min = -5.0,
    max = 5.0,
    step = 0.01,
    description = "Seconds added to content duration for loop repeat timing. Positive = silence between repetitions, negative = overlapping re-triggers via crossfade."
  )]
  pub gap: f64,

  #[patch_param(
    min = -100.0,
    max = 100.0,
    step = 1.0,
    description = "Pitch offset in cents applied to all oscillators. Creates chorus-like thickness."
  )]
  pub detune: f64,

  #[patch_param(
    min = -1.0,
    max = 1.0,
    step = 0.01,
    description = "Offset added to waveform harshness. Positive shifts sine toward saw, negative does the reverse."
  )]
  pub harshness_offset: f64,
}

impl Default for Patch {
  fn default() -> Self {
    Self {
      freq: 440.0,
      duration: 0.5,
      sine_ratio: 1.0,
      tri_ratio: 0.0,
      saw_ratio: 0.0,
      square_ratio: 0.0,
      attack_ms: 20.0,
      decay_ms: 0.0,
      release_ms: 150.0,
      sustain: 1.0,
      chirp_ratio: 1.0,
      stereo_pan: 0.0,
      reverb_mix: 0.2,
      echo_delay: 0.25,
      echo_mix: 0.0,
      brightness: 1.0,
      resonance: 1.0,
      highpass: 0.0,
      sub_octave: 0.0,
      vibrato_rate: 0.0,
      vibrato_depth: 0.0,
      tremolo_rate: 0.0,
      tremolo_depth: 0.0,
      amplitude: 0.3,
      drive: 1.0,
      noise_mix: 0.0,
      crush: 0.0,
      fm_ratio: 0.0,
      fm_depth: 0.0,
      downsample: 0.0,
      gap: 0.0,
      detune: 0.0,
      harshness_offset: 0.0,
    }
  }
}

impl Patch {
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
    result
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
  fn default_patch_has_expected_values() {
    let p = Patch::default();
    assert_eq!(p.freq, 440.0);
    assert_eq!(p.duration, 0.5);
    assert_eq!(p.sine_ratio, 1.0);
    assert_eq!(p.amplitude, 0.3);
    assert_eq!(p.reverb_mix, 0.2);
  }

  #[test]
  fn overrides_replace_specified_fields_only() {
    let p = Patch::default();
    let overridden = p.with_overrides(&PatchOverrides {
      freq: Some(880.0),
      ..Default::default()
    });
    assert_eq!(overridden.freq, 880.0);
    assert_eq!(overridden.sine_ratio, 1.0);
  }

  #[test]
  fn lerp_at_zero_equals_lo() {
    let lo = Patch {
      freq: 200.0,
      amplitude: 0.2,
      ..Default::default()
    };
    let hi = Patch {
      freq: 800.0,
      amplitude: 0.8,
      ..Default::default()
    };
    let result = Patch::lerp(&lo, &hi, 0.0);
    assert_eq!(result.freq, 200.0);
    assert_eq!(result.amplitude, 0.2);
  }

  #[test]
  fn lerp_at_one_equals_hi() {
    let lo = Patch {
      freq: 200.0,
      amplitude: 0.2,
      ..Default::default()
    };
    let hi = Patch {
      freq: 800.0,
      amplitude: 0.8,
      ..Default::default()
    };
    let result = Patch::lerp(&lo, &hi, 1.0);
    assert_eq!(result.freq, 800.0);
    assert_eq!(result.amplitude, 0.8);
  }

  #[test]
  fn lerp_at_half_equals_midpoint() {
    let lo = Patch {
      freq: 200.0,
      ..Default::default()
    };
    let hi = Patch {
      freq: 800.0,
      ..Default::default()
    };
    let result = Patch::lerp(&lo, &hi, 0.5);
    assert!(
      (result.freq - 500.0).abs() < 1e-10,
      "freq midpoint: got {} expected 500.0",
      result.freq,
    );
  }

  #[test]
  fn lerp_clamps_t() {
    let lo = Patch {
      freq: 200.0,
      ..Default::default()
    };
    let hi = Patch {
      freq: 800.0,
      ..Default::default()
    };
    let below = Patch::lerp(&lo, &hi, -0.5);
    assert_eq!(below.freq, 200.0);
    let above = Patch::lerp(&lo, &hi, 2.0);
    assert_eq!(above.freq, 800.0);
  }

  #[test]
  fn params_metadata_covers_all_fields() {
    let patch = Patch::default();
    for meta in Patch::PARAMS {
      assert!(
        patch.get_param(meta.name).is_some(),
        "PARAMS entry '{}' not accessible via get_param",
        meta.name
      );
    }
    assert_eq!(Patch::PARAMS.len(), 33);
  }

  #[test]
  fn set_param_round_trips() {
    let mut patch = Patch::default();
    patch.set_param("freq", 999.0);
    assert_eq!(patch.get_param("freq"), Some(999.0));
  }

  #[test]
  fn deserialize_sparse_uses_defaults() {
    let toml = "freq = 880.0\nsaw_ratio = 0.5\n";
    let patch: Patch = toml::from_str(toml).unwrap();
    assert_eq!(patch.freq, 880.0);
    assert_eq!(patch.saw_ratio, 0.5);
    assert_eq!(patch.sine_ratio, 1.0);
    assert_eq!(patch.amplitude, 0.3);
  }

  #[test]
  fn serialize_round_trips() {
    let patch = Patch::default();
    let json = serde_json::to_value(&patch).unwrap();
    assert!(json.get("freq").is_some());
    assert!(json.get("duration").is_some());
  }

  #[test]
  fn patch_overrides_to_fields() {
    let o = PatchOverrides {
      freq: Some(440.0),
      amplitude: Some(0.5),
      ..Default::default()
    };
    let fields = o.to_fields();
    assert!(fields.iter().any(|(n, v)| *n == "freq" && *v == 440.0));
    assert!(fields.iter().any(|(n, v)| *n == "amplitude" && *v == 0.5));
    assert_eq!(fields.len(), 2);
  }
}
