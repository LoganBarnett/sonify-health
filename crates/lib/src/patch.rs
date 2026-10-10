use serde::{Deserialize, Serialize};
use sonify_health_voice_derive::PatchGenerate;
use std::fmt;

/// Metadata for a single patch parameter, used by the UI and
/// serialisation helpers.
///
/// A parameter has two tiers of bounds.  `min` and `max` are the dashboard
/// slider's range, chosen to taste; a value past them is honoured.  The hard
/// limits (`limit_min`, `limit_max`, `positive`) mark where a value stops
/// meaning anything, and `limit` holds a value to them.
#[derive(Debug, Clone)]
pub struct PatchParamMeta {
  pub name: &'static str,
  pub description: &'static str,
  /// Slider minimum.
  pub min: f64,
  /// Slider maximum.
  pub max: f64,
  pub step: f64,
  /// The UI slider maps position to value logarithmically, and `Patch::lerp`
  /// interpolates geometrically.
  pub logarithmic: bool,
  /// Value of a freshly built patch, and what a meaningless value reads as.
  pub default: f64,
  /// Hard lower limit, inclusive; `None` is unbounded.
  pub limit_min: Option<f64>,
  /// Hard upper limit, inclusive; `None` is unbounded.
  pub limit_max: Option<f64>,
  /// Only strictly positive values work.  Zero has no nearest valid value,
  /// so a value at or below zero reads as `default`.
  pub positive: bool,
}

/// How a value broke its parameter's hard limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitKind {
  /// Not a finite number once narrowed to the synth's f32.
  NonFinite,
  /// At or below zero for a parameter that only works above it.
  NotPositive,
  BelowLimit,
  AboveLimit,
}

/// A parameter value that broke a hard limit, and what replaced it.
#[derive(Debug, Clone, PartialEq)]
pub struct LimitViolation {
  pub param: &'static str,
  pub value: f64,
  pub replacement: f64,
  pub kind: LimitKind,
}

impl fmt::Display for LimitViolation {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let LimitViolation {
      param,
      value,
      replacement,
      kind,
    } = self;
    match kind {
      LimitKind::NonFinite => write!(
        f,
        "{param} = {value} is not a finite number; using its default \
         {replacement}"
      ),
      LimitKind::NotPositive => write!(
        f,
        "{param} = {value} must be above zero; using its default \
         {replacement}"
      ),
      LimitKind::BelowLimit => write!(
        f,
        "{param} = {value} is below its hard limit; using {replacement}"
      ),
      LimitKind::AboveLimit => write!(
        f,
        "{param} = {value} is above its hard limit; using {replacement}"
      ),
    }
  }
}

impl PatchParamMeta {
  /// `value` held to this parameter's hard limits, and the violation when
  /// it broke one.  The slider range plays no part.
  pub fn limit(&self, value: f64) -> (f64, Option<LimitViolation>) {
    let breach = if !(value as f32).is_finite() {
      Some((self.default, LimitKind::NonFinite))
    } else if self.positive && value <= 0.0 {
      Some((self.default, LimitKind::NotPositive))
    } else {
      self
        .limit_min
        .filter(|lo| value < *lo)
        .map(|lo| (lo, LimitKind::BelowLimit))
        .or_else(|| {
          self
            .limit_max
            .filter(|hi| value > *hi)
            .map(|hi| (hi, LimitKind::AboveLimit))
        })
    };
    breach.map_or((value, None), |(replacement, kind)| {
      (
        replacement,
        Some(LimitViolation {
          param: self.name,
          value,
          replacement,
          kind,
        }),
      )
    })
  }
}

/// A named, static sound definition.
///
/// Patches define every parameter of a single synthesised note.
/// They are stored in a `PatchLibrary` (built-in presets plus user
/// overrides).  `Transition::resolve()` interpolates or selects
/// patches based on a probe metric.
///
/// The slider range is for taste.  A hard limit marks only a value that is
/// meaningless or a blatant error, and each one below carries a comment stating
/// the fact that forces it; a parameter without one accepts any finite value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, PatchGenerate)]
#[serde(default)]
pub struct Patch {
  // A negative pitch drives the ladder's cutoff negative, and the ladder
  // diverges to NaN.
  #[patch_param(
    min = 20.0,
    max = 12000.0,
    step = 1.0,
    default = 440.0,
    limit_min = 0.0,
    description = "Pitch in Hz."
  )]
  pub freq: f64,

  // A negative time ends the note before its own envelope does.
  #[patch_param(
    min = 0.01,
    max = 5.0,
    step = 0.01,
    default = 0.5,
    limit_min = 0.0,
    description = "Length of the sustain phase in seconds. The note holds at \
                   `sustain` level for this long, between the `attack_ms` + \
                   `decay_ms` ramp-up and the `release_ms` tail. Total audible \
                   note length = attack + decay + duration + release."
  )]
  pub duration: f64,

  // A negative weight lets the waveform weights sum toward zero, where their
  // normalisation divides by it.
  #[patch_param(
    min = 0.0,
    max = 3.0,
    step = 0.01,
    default = 1.0,
    limit_min = 0.0,
    description = "Relative weight of the sine oscillator. Smooth, pure tone."
  )]
  pub sine_ratio: f64,

  // A negative weight lets the waveform weights sum toward zero, where their
  // normalisation divides by it.
  #[patch_param(
    min = 0.0,
    max = 3.0,
    step = 0.01,
    default = 0.0,
    limit_min = 0.0,
    description = "Relative weight of the triangle oscillator. Hollow, \
                   flute-like."
  )]
  pub tri_ratio: f64,

  // A negative weight lets the waveform weights sum toward zero, where their
  // normalisation divides by it.
  #[patch_param(
    min = 0.0,
    max = 3.0,
    step = 0.01,
    default = 0.0,
    limit_min = 0.0,
    description = "Relative weight of the sawtooth oscillator. Bright, buzzy \
                   edge."
  )]
  pub saw_ratio: f64,

  // A negative weight lets the waveform weights sum toward zero, where their
  // normalisation divides by it.
  #[patch_param(
    min = 0.0,
    max = 3.0,
    step = 0.01,
    default = 0.0,
    limit_min = 0.0,
    description = "Relative weight of the square oscillator. Hollow, reedy \
                   tone."
  )]
  pub square_ratio: f64,

  // A negative time ends the note before its own envelope does.
  #[patch_param(
    min = 0.0,
    max = 500.0,
    step = 1.0,
    default = 20.0,
    limit_min = 0.0,
    description = "Ramp-up time from silence to full amplitude, in \
                   milliseconds. First phase of the ADSR envelope; `decay_ms`, \
                   `duration`, and `release_ms` follow. Low = snappy click, \
                   high = soft swell."
  )]
  pub attack_ms: f64,

  // A negative time ends the note before its own envelope does.
  #[patch_param(
    min = 0.0,
    max = 2000.0,
    step = 1.0,
    default = 0.0,
    limit_min = 0.0,
    description = "Ramp-down time in milliseconds, from the attack peak down \
                   to the `sustain` level. Runs between `attack_ms` and \
                   `duration`."
  )]
  pub decay_ms: f64,

  // A negative time ends the note before its own envelope does.
  #[patch_param(
    min = 0.0,
    max = 1000.0,
    step = 1.0,
    default = 150.0,
    limit_min = 0.0,
    description = "Ramp-down time in milliseconds, from the `sustain` level to \
                   silence. Final phase of the envelope, after `attack_ms` + \
                   `decay_ms` + `duration`. Low = staccato, high = lingering \
                   tail."
  )]
  pub release_ms: f64,

  // A negative level inverts the note's body.
  #[patch_param(
    min = 0.0,
    max = 1.0,
    step = 0.01,
    default = 1.0,
    limit_min = 0.0,
    description = "Body amplitude (0-1) held during the `duration` phase, \
                   between the end of `decay_ms` and the start of \
                   `release_ms`. 1 = full level (flat envelope top), lower = \
                   quieter sustained tone."
  )]
  pub sustain: f64,

  // A negative exponent makes `powf(0, curve)` infinite at the ends of the
  // ramps; zero is the step-envelope limit.
  #[patch_param(
    min = 0.25,
    max = 4.0,
    step = 0.01,
    logarithmic,
    default = 1.0,
    limit_min = 0.0,
    description = "Bend of the attack, decay, and release ramps. 1 = straight \
                   lines; above 1 the attack lingers near silence before \
                   swelling and decay and release fall fast then trail off; \
                   below 1 the reverse. No effect in continuous playback, \
                   which has no envelope."
  )]
  pub envelope_curve: f64,

  // A negative ratio starts the glide at a negative pitch.
  #[patch_param(
    min = 0.5,
    max = 4.0,
    step = 0.01,
    default = 1.0,
    limit_min = 0.0,
    description = "Pitch bend at note onset. 1.0 = none, <1 = downward, >1 = \
                   upward chirp."
  )]
  pub chirp_ratio: f64,

  // fundsp's pan clamps its position to +-1, so a value past it plays as the
  // edge.
  #[patch_param(
    min = -1.0,
    max = 1.0,
    step = 0.01,
    default = 0.0,
    limit_min = -1.0,
    limit_max = 1.0,
    description = "Left/right stereo position. -1 = full left, +1 = full right."
  )]
  pub stereo_pan: f64,

  // Past 1 the dry weight `1 - mix` goes negative and inverts the dry path.
  #[patch_param(
    min = 0.0,
    max = 1.0,
    step = 0.01,
    default = 0.2,
    limit_min = 0.0,
    limit_max = 1.0,
    description = "Wet/dry reverb blend. 0 = fully dry, 1 = fully wet. Runs \
                   through a fixed medium hall; 0.1-0.3 is a typical room."
  )]
  pub reverb_mix: f64,

  // fundsp's delay asserts a non-negative time when the graph is built.
  #[patch_param(
    min = 0.01,
    max = 1.0,
    step = 0.01,
    default = 0.25,
    limit_min = 0.0,
    description = "Delay time in seconds. Short = slapback, long = distinct \
                   repeats."
  )]
  pub echo_delay: f64,

  // A negative mix inverts the echo.
  #[patch_param(
    min = 0.0,
    max = 1.0,
    step = 0.01,
    default = 0.0,
    limit_min = 0.0,
    description = "Echo wet/dry blend. 0 = no echo, 1 = full echo."
  )]
  pub echo_mix: f64,

  // A negative scale drives the ladder's cutoff negative, and the ladder
  // diverges to NaN.
  #[patch_param(
    min = 0.05,
    max = 2.0,
    step = 0.01,
    default = 1.0,
    limit_min = 0.0,
    description = "Pitch-tracking ladder filter cutoff scaler. 1.0 = full \
                   brightness, lower = darker tone."
  )]
  pub brightness: f64,

  // A negative Q turns the ladder's feedback positive.
  #[patch_param(
    min = 0.1,
    max = 5.0,
    step = 0.01,
    default = 1.0,
    limit_min = 0.0,
    description = "Filter Q scaler. 1.0 = default resonance, lower = smoother \
                   rolloff, higher = nasal peak."
  )]
  pub resonance: f64,

  // Zero or below is off.  The upper limit depends on the device's sample
  // rate, so heartbeat.rs applies it when the graph is built.
  #[patch_param(
    min = 0.0,
    max = 2000.0,
    step = 1.0,
    default = 0.0,
    limit_min = 0.0,
    description = "Highpass filter cutoff in Hz. 0 = off, higher = cuts more \
                   low frequencies."
  )]
  pub highpass: f64,

  // A negative cutoff puts the filter's poles in the right half-plane.  The
  // attribute only accepts literals, so the slider's top, which is "off",
  // repeats `MAX_CUTOFF` from heartbeat.rs; a test asserts the sync.
  #[patch_param(
    min = 20.0,
    max = 18000.0,
    step = 1.0,
    logarithmic,
    default = 18000.0,
    limit_min = 0.0,
    description = "Lowpass filter cutoff in Hz. 18000 = off (fully open), \
                   lower = cuts more high frequencies."
  )]
  pub lowpass: f64,

  // A negative centre makes the band unstable; zero is an exact passthrough.
  // The upper limit depends on the device's sample rate, so heartbeat.rs
  // applies it when the graph is built.
  #[patch_param(
    min = 20.0,
    max = 18000.0,
    step = 1.0,
    logarithmic,
    default = 1000.0,
    limit_min = 0.0,
    description = "Centre frequency of a single parametric EQ band; `eq_db` \
                   sets the boost or cut and `eq_q` the width."
  )]
  pub eq_hz: f64,

  // The band's linear gain, 10^(dB/20), leaves f32's normal range past
  // +-758 dB.
  #[patch_param(
    min = -24.0,
    max = 24.0,
    step = 0.1,
    default = 0.0,
    limit_min = -750.0,
    limit_max = 750.0,
    description = "Gain of the EQ band in dB. 0 = off, positive = a resonant \
                   bump, negative = a notch. A large boost on a loud patch \
                   exceeds full scale, so lower `amplitude` to compensate."
  )]
  pub eq_db: f64,

  // The band's damping is 1 / (Q * sqrt(gain)), which divides by zero at
  // Q = 0, and a negative Q makes the band unstable.
  #[patch_param(
    min = 0.1,
    max = 10.0,
    step = 0.01,
    logarithmic,
    default = 1.0,
    positive,
    description = "Width of the EQ band. Q 1.4 spans an octave; lower = \
                   broader, higher = narrower."
  )]
  pub eq_q: f64,

  // A negative mix inverts the sub-octave.
  #[patch_param(
    min = 0.0,
    max = 1.0,
    step = 0.01,
    default = 0.0,
    limit_min = 0.0,
    description = "Sub-oscillator mix at one octave below. 0 = off, higher = \
                   deeper body."
  )]
  pub sub_octave: f64,

  #[patch_param(
    min = 0.0,
    max = 1.0,
    step = 0.01,
    default = 0.0,
    description = "Where in its cycle the sub-octave starts relative to the \
                   main oscillators, in turns. With drive above 1 this shifts \
                   which partials the saturation emphasises; 0 is the neutral \
                   alignment."
  )]
  pub sub_phase: f64,

  // A negative rate is the same vibrato half a cycle later.
  #[patch_param(
    min = 0.0,
    max = 200.0,
    step = 0.1,
    default = 0.0,
    limit_min = 0.0,
    description = "Vibrato speed (Hz). Above ~30 Hz becomes FM synthesis."
  )]
  pub vibrato_rate: f64,

  // A negative depth only flips the vibrato's phase.
  #[patch_param(
    min = 0.0,
    max = 12.0,
    step = 0.01,
    default = 0.0,
    limit_min = 0.0,
    description = "Vibrato depth (semitones). Large values produce FM \
                   sidebands."
  )]
  pub vibrato_depth: f64,

  // A negative rate is the same tremolo half a cycle later.
  #[patch_param(
    min = 0.0,
    max = 20.0,
    step = 0.1,
    default = 0.0,
    limit_min = 0.0,
    description = "Tremolo speed (Hz)."
  )]
  pub tremolo_rate: f64,

  // A negative depth inverts the tremolo's shape.  Past 1 the gain swings
  // negative, which is ring modulation and is kept.
  #[patch_param(
    min = 0.0,
    max = 1.0,
    step = 0.01,
    default = 0.0,
    limit_min = 0.0,
    description = "Tremolo depth (fraction)."
  )]
  pub tremolo_depth: f64,

  // A negative amplitude inverts the note.
  #[patch_param(
    min = 0.0,
    max = 1.0,
    step = 0.01,
    default = 0.3,
    limit_min = 0.0,
    description = "Output amplitude. 0 = silent, 1 = full scale."
  )]
  pub amplitude: f64,

  // The shaper divides by tanh(drive), which is zero at zero; at 0.01 it
  // already matches the clean signal within -90 dB.
  #[patch_param(
    min = 0.01,
    max = 20.0,
    step = 0.1,
    default = 1.0,
    limit_min = 0.01,
    description = "Pre-filter saturation. Low = clean, high = heavy distortion."
  )]
  pub drive: f64,

  // A negative mix inverts the noise.
  #[patch_param(
    min = 0.0,
    max = 1.0,
    step = 0.01,
    default = 0.0,
    limit_min = 0.0,
    description = "Pink noise mixed before the filter for texture and breath."
  )]
  pub noise_mix: f64,

  // A negative level inverts the hiss.
  #[patch_param(
    min = 0.0,
    max = 1.0,
    step = 0.01,
    default = 0.0,
    limit_min = 0.0,
    description = "White noise added after the ladder filter, so it stays \
                   bright however dark the tone; `lowpass` still caps it."
  )]
  pub hiss: f64,

  // The quantiser keeps 2^(16 - 15 * crush) levels per unit, so past 16/15
  // one step exceeds full scale; below 0 it only grows finer.
  #[patch_param(
    min = 0.0,
    max = 1.0,
    step = 0.01,
    default = 0.0,
    limit_min = 0.0,
    limit_max = 1.0666666666666667,
    description = "Bitcrush intensity. 0 = clean, higher = grungier."
  )]
  pub crush: f64,

  // The modulation is even in the ratio, so a negative ratio only mirrors a
  // positive one.
  #[patch_param(
    min = 0.0,
    max = 8.0,
    step = 0.01,
    default = 0.0,
    limit_min = 0.0,
    description = "FM modulator frequency as a ratio of the carrier. 1.0 = \
                   unison, 2.0 = octave."
  )]
  pub fm_ratio: f64,

  // A negative depth only flips the modulator's phase.
  #[patch_param(
    min = 0.0,
    max = 10.0,
    step = 0.1,
    default = 0.0,
    limit_min = 0.0,
    description = "FM modulation index. 0 = clean, higher = richer metallic \
                   warble. Above 1 the modulator swings the carrier through \
                   zero, where it clamps, and the modulator is drawn at \
                   control rate (about 500 Hz), so high settings give a gritty \
                   lo-fi grain rather than clean FM sidebands."
  )]
  pub fm_depth: f64,

  // Below 0 the hold rate rises past the device rate, the same passthrough as
  // 0, until it overflows f32.
  #[patch_param(
    min = 0.0,
    max = 1.0,
    step = 0.01,
    default = 0.0,
    limit_min = 0.0,
    description = "Lo-fi sample rate reduction. 0 = full fidelity, higher = \
                   crunchier."
  )]
  pub downsample: f64,

  #[patch_param(
    min = -5.0,
    max = 5.0,
    step = 0.01,
    default = 0.0,
    description = "Seconds added to content duration for loop repeat timing. \
                   Positive = silence between repetitions, negative = \
                   overlapping re-triggers via crossfade."
  )]
  pub gap: f64,

  #[patch_param(
    min = -100.0,
    max = 100.0,
    step = 1.0,
    default = 0.0,
    description = "Pitch offset in cents applied to all oscillators. Creates \
                   chorus-like thickness."
  )]
  pub detune: f64,

  // A negative spread swaps the two identical voices.
  #[patch_param(
    min = 0.0,
    max = 100.0,
    step = 0.1,
    default = 0.0,
    limit_min = 0.0,
    description = "Cents between two copies of the main oscillators, detuned \
                   symmetrically around the pitch. 5-20 thickens, higher \
                   beats audibly. The sub-octave is not spread."
  )]
  pub spread: f64,

  // At +-1 a whole unit of weight has moved between sine and saw; past it the
  // saw weight grows beyond the normalised total.
  #[patch_param(
    min = -1.0,
    max = 1.0,
    step = 0.01,
    default = 0.0,
    limit_min = -1.0,
    limit_max = 1.0,
    description = "Offset added to waveform harshness. Positive shifts sine \
                   toward saw, negative does the reverse."
  )]
  pub harshness_offset: f64,
}

impl Patch {
  /// This patch with every parameter held to its hard limits, and the
  /// violations that required it.  A value past the slider range but inside
  /// the hard limits is kept as written.
  pub fn limited(&self) -> (Self, Vec<LimitViolation>) {
    let violations = Self::PARAMS
      .iter()
      .zip(self.values())
      .filter_map(|(meta, value)| meta.limit(value).1)
      .collect();
    (self.map_params(|meta, value| meta.limit(value).0), violations)
  }

  /// Interpolate every field between two patches.  The parameter `t` is clamped
  /// to 0.0..=1.0, where 0.0 yields `lo` and 1.0 yields `hi`.  Log-scaled
  /// parameters interpolate geometrically so a sweep is perceptually even;
  /// everything else is linear.  Results are held to the hard limits, not the
  /// slider range, so a gradient keeps an endpoint written past the slider.
  pub fn lerp(lo: &Patch, hi: &Patch, t: f64) -> Patch {
    let t = t.clamp(0.0, 1.0);
    lo.zip_params(hi, |meta, lo_val, hi_val| {
      // Geometric interpolation needs positive endpoints, and a log-scaled
      // parameter's hard limit admits zero.
      let val = if meta.logarithmic && lo_val > 0.0 && hi_val > 0.0 {
        lo_val * (hi_val / lo_val).powf(t)
      } else {
        lo_val + (hi_val - lo_val) * t
      };
      meta.limit(val).0
    })
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

  fn meta(name: &str) -> &'static PatchParamMeta {
    Patch::PARAMS
      .iter()
      .find(|m| m.name == name)
      .unwrap_or_else(|| panic!("missing PARAMS entry '{name}'"))
  }

  /// A default patch with `name` set to `value` directly, the way a config
  /// file sets it, bypassing every limit.
  fn with_raw(name: &str, value: f64) -> Patch {
    let mut table = toml::Value::try_from(Patch::default()).unwrap();
    table
      .as_table_mut()
      .unwrap()
      .insert(name.to_string(), toml::Value::Float(value));
    table.try_into().unwrap()
  }

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
  fn default_matches_every_declared_default() {
    let patch = Patch::default();
    for (meta, value) in Patch::PARAMS.iter().zip(patch.values()) {
      assert_eq!(value, meta.default, "{}", meta.name);
    }
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
  fn lerp_keeps_an_endpoint_past_the_slider() {
    let lo = Patch {
      attack_ms: 2000.0,
      freq: 15.0,
      ..Default::default()
    };
    let hi = Patch::default();
    let result = Patch::lerp(&lo, &hi, 0.0);
    assert_eq!(result.attack_ms, 2000.0);
    assert_eq!(result.freq, 15.0);
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
    assert_eq!(Patch::PARAMS.len(), 41);
  }

  #[test]
  fn set_param_round_trips() {
    let mut patch = Patch::default();
    patch.set_param("freq", 999.0);
    assert_eq!(patch.get_param("freq"), Some(999.0));
  }

  #[test]
  fn set_param_honours_a_value_past_the_slider() {
    let mut patch = Patch::default();
    patch.set_param("attack_ms", 3000.0);
    assert_eq!(patch.attack_ms, 3000.0);
    patch.set_param("highpass", 5000.0);
    assert_eq!(patch.highpass, 5000.0);
  }

  #[test]
  fn set_param_holds_to_the_hard_limits() {
    let mut patch = Patch::default();
    patch.set_param("attack_ms", -5.0);
    assert_eq!(patch.attack_ms, 0.0);
    patch.set_param("reverb_mix", 1.5);
    assert_eq!(patch.reverb_mix, 1.0);
    patch.set_param("eq_q", 0.0);
    assert_eq!(patch.eq_q, meta("eq_q").default);
  }

  #[test]
  fn set_param_rejects_non_finite() {
    let mut patch = Patch::default();
    assert!(!patch.set_param("lowpass", f64::NAN));
    assert!(!patch.set_param("lowpass", f64::INFINITY));
    assert_eq!(patch.get_param("lowpass"), Some(18000.0));
  }

  #[test]
  fn limited_holds_every_parameter_to_its_hard_limits() {
    for meta in Patch::PARAMS {
      let raw = |value| {
        let (patch, violations) = with_raw(meta.name, value).limited();
        (patch.get_param(meta.name), violations.len())
      };
      if let Some(lo) = meta.limit_min {
        assert_eq!(raw(lo - 1.0), (Some(lo), 1), "{} below", meta.name);
      }
      if let Some(hi) = meta.limit_max {
        assert_eq!(raw(hi + 1.0), (Some(hi), 1), "{} above", meta.name);
      }
      assert_eq!(
        raw(f64::NAN),
        (Some(meta.default), 1),
        "{} non-finite",
        meta.name
      );
      let past_slider = meta.max * 3.0 + 1.0;
      assert_eq!(
        raw(past_slider).0,
        Some(meta.limit_max.map_or(past_slider, |hi| past_slider.min(hi))),
        "{} past the slider",
        meta.name
      );
    }
  }

  #[test]
  fn limited_reads_a_non_positive_value_as_the_default() {
    let (patch, violations) = with_raw("eq_q", 0.0).limited();
    assert_eq!(patch.eq_q, meta("eq_q").default);
    assert_eq!(violations.len(), 1);
    assert_eq!(violations[0].kind, LimitKind::NotPositive);
  }

  #[test]
  fn limited_treats_an_f32_overflow_as_non_finite() {
    let (patch, violations) = with_raw("attack_ms", 1e300).limited();
    assert_eq!(patch.attack_ms, meta("attack_ms").default);
    assert_eq!(violations[0].kind, LimitKind::NonFinite);
  }

  #[test]
  fn limited_leaves_a_patch_inside_its_limits_untouched() {
    let patch = Patch {
      attack_ms: 2000.0,
      freq: 15.0,
      ..Default::default()
    };
    assert_eq!(patch.limited(), (patch.clone(), vec![]));
  }

  #[test]
  fn violation_names_the_parameter_and_the_replacement() {
    let (_, violations) = with_raw("reverb_mix", 1.5).limited();
    assert_eq!(
      violations[0].to_string(),
      "reverb_mix = 1.5 is above its hard limit; using 1"
    );
  }

  #[test]
  fn lowpass_slider_top_is_the_heartbeat_off_point() {
    let lowpass = meta("lowpass");
    assert_eq!(lowpass.max, f64::from(crate::heartbeat::MAX_CUTOFF));
    assert!(lowpass.logarithmic);
  }

  #[test]
  fn lerp_interpolates_log_params_geometrically() {
    let lo = Patch {
      lowpass: 200.0,
      ..Default::default()
    };
    let hi = Patch {
      lowpass: 18000.0,
      ..Default::default()
    };
    let mid = Patch::lerp(&lo, &hi, 0.5);
    let expected = (200.0f64 * 18000.0).sqrt();
    assert!(
      (mid.lowpass - expected).abs() < 1.0,
      "geometric midpoint of 200..18000 should be ~{expected:.0}, \
       got {:.0}",
      mid.lowpass
    );
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
