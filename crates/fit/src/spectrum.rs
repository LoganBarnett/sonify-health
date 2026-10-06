//! Spectral measurements of a steady tone, and the distance between two of
//! them.

use std::f64::consts::TAU;
use std::num::ParseFloatError;
use thiserror::Error;

/// Most harmonics a measurement follows.  Beyond this a harmonic series is
/// denser than the windows that tell its members apart.
const MAX_HARMONICS: f64 = 128.0;

#[derive(Debug, Error)]
pub enum FundamentalParseError {
  #[error("{argument:?} is not a frequency in Hz: {source}")]
  NotANumber {
    argument: String,
    #[source]
    source: ParseFloatError,
  },

  #[error("a fundamental must be a finite frequency above zero, got {hz} Hz")]
  NotPositive { hz: f64 },
}

/// Parses a fundamental frequency given on the command line.
pub fn fundamental_hz(argument: &str) -> Result<f64, FundamentalParseError> {
  argument
    .parse::<f64>()
    .map_err(|source| FundamentalParseError::NotANumber {
      argument: argument.to_string(),
      source,
    })
    .and_then(|hz| {
      (hz.is_finite() && hz > 0.0)
        .then_some(hz)
        .ok_or(FundamentalParseError::NotPositive { hz })
    })
}

/// How many harmonics two recordings can both be measured on: those lying
/// under the Nyquist limit of each, at each one's own fundamental.
pub fn shared_harmonics(sides: &[(f64, f64)]) -> usize {
  sides
    .iter()
    .map(|(sample_rate, fundamental)| {
      (0.49 * sample_rate / fundamental).floor()
    })
    .fold(MAX_HARMONICS, f64::min)
    .max(1.0) as usize
}

/// Edges, in Hz, of the octave bands the power distribution is measured in.
pub const BAND_EDGES_HZ: [f64; 9] = [
  10.0, 80.0, 160.0, 320.0, 640.0, 1280.0, 2560.0, 5120.0, 20000.0,
];

/// Levels below this, in dB, are indistinguishable from a measurement's noise
/// floor, so harmonic levels are clamped to it before they are compared.
const HARMONIC_FLOOR_DB: f64 = -42.0;

/// The same clamp for a band's share of the total power.
const BAND_FLOOR_DB: f64 = -32.0;

/// The same clamp for the share of power lying between the harmonics.
const OFF_HARMONIC_FLOOR_DB: f64 = -30.0;

/// Harmonics above this number sit closer together than the windows that
/// separate them from their neighbours, so the off-harmonic share is measured
/// below it.
const OFF_HARMONIC_LIMIT: usize = 40;

/// How strongly a mismatch in crest factor counts against a candidate, in
/// squared-dB of harmonic error per unit of squared crest-factor error.
const CREST_WEIGHT: f64 = 2.0;

/// How strongly surplus off-harmonic power counts against a candidate.
const OFF_HARMONIC_WEIGHT: f64 = 0.5;

/// What a steady tone sounds like, reduced to numbers that can be compared.
#[derive(Debug, Clone)]
pub struct Features {
  /// Level of each harmonic of the fundamental, in dB relative to the summed
  /// power of all of them.
  pub harmonic_db: Vec<f64>,
  /// Share of the total power in each band of `BAND_EDGES_HZ`, in dB.
  pub band_db: Vec<f64>,
  /// Share of the low-frequency power that lies between the harmonics, in dB.
  pub off_harmonic_db: f64,
  /// Peak over RMS: how pulse-like the waveform is.
  pub crest: f64,
  pub peak: f64,
  pub rms: f64,
}

impl Features {
  /// Measures `samples` against the harmonic series of `fundamental_hz`.
  /// Returns `None` for silence, which has no spectrum to describe.
  pub fn measure(
    samples: &[f64],
    sample_rate: f64,
    fundamental_hz: f64,
    harmonics: usize,
  ) -> Option<Self> {
    let mean = samples.iter().sum::<f64>() / samples.len().max(1) as f64;
    let centred: Vec<f64> = samples.iter().map(|s| s - mean).collect();
    let power = power_spectrum(&centred);
    let bin_hz = sample_rate / (2 * power.len()) as f64;
    let bin =
      |hz: f64| ((hz / bin_hz).round().max(0.0) as usize).min(power.len() - 1);
    let window =
      |centre: f64, half: f64| &power[bin(centre - half)..=bin(centre + half)];

    let harmonic_power: Vec<f64> = (1..=harmonics)
      .map(|k| k as f64 * fundamental_hz)
      .map(|centre| {
        window(centre, (centre * 0.012).max(1.2))
          .iter()
          .fold(0.0f64, |peak, p| peak.max(*p))
      })
      .collect();
    let harmonic_total: f64 = harmonic_power.iter().sum();

    let on_harmonic: f64 = (1..=OFF_HARMONIC_LIMIT.min(harmonics))
      .map(|k| k as f64 * fundamental_hz)
      .map(|centre| {
        window(centre, (centre * 0.007).clamp(1.5, 8.0))
          .iter()
          .sum::<f64>()
      })
      .sum();
    let low_total: f64 = power[bin(BAND_EDGES_HZ[0])
      ..bin((OFF_HARMONIC_LIMIT + 1) as f64 * fundamental_hz)]
      .iter()
      .sum();

    let total: f64 = power[bin(BAND_EDGES_HZ[0])..].iter().sum();
    let rms = (centred.iter().map(|s| s * s).sum::<f64>()
      / centred.len().max(1) as f64)
      .sqrt();
    let peak = centred.iter().fold(0.0f64, |m, s| m.max(s.abs()));

    (harmonic_total > 0.0 && low_total > 0.0 && rms > 1e-6).then(|| Self {
      harmonic_db: harmonic_power
        .iter()
        .map(|p| decibels(p / harmonic_total).max(HARMONIC_FLOOR_DB))
        .collect(),
      band_db: BAND_EDGES_HZ
        .windows(2)
        .map(|edge| (bin(edge[0]), bin(edge[1])))
        .map(|(low, high)| power.get(low..high).map_or(0.0, |b| b.iter().sum()))
        .map(|p: f64| decibels(p / total).max(BAND_FLOOR_DB))
        .collect(),
      off_harmonic_db: decibels(1.0 - on_harmonic / low_total)
        .max(OFF_HARMONIC_FLOOR_DB),
      crest: peak / rms,
      peak,
      rms,
    })
  }

  /// How far `candidate` is from `self`, the target: zero for a perfect match
  /// and roughly the mean squared error, in dB, of the audible partials.
  pub fn distance(&self, candidate: &Self) -> f64 {
    let loudest = self.harmonic_db.iter().fold(f64::MIN, |m, l| m.max(*l));
    // Unweighted, the many partials near the noise floor outvote the few that
    // carry the sound, so each term is weighted by the target's own level.
    let (error, weight) = self
      .harmonic_db
      .iter()
      .zip(&candidate.harmonic_db)
      .map(|(t, c)| (10f64.powf((t - loudest) / 20.0), t - c))
      .fold((0.0, 0.0), |(error, weight), (w, diff)| {
        (error + w * diff * diff, weight + w)
      });
    let bands = self
      .band_db
      .iter()
      .zip(&candidate.band_db)
      .map(|(t, c)| (t - c).powi(2))
      .sum::<f64>()
      / self.band_db.len().max(1) as f64;
    let surplus_off_harmonic =
      (candidate.off_harmonic_db - self.off_harmonic_db).max(0.0);

    error / weight.max(f64::MIN_POSITIVE)
      + bands
      + CREST_WEIGHT * (self.crest - candidate.crest).powi(2)
      + OFF_HARMONIC_WEIGHT * surplus_off_harmonic.powi(2)
  }
}

fn decibels(power_ratio: f64) -> f64 {
  10.0 * power_ratio.max(1e-12).log10()
}

/// Hann-windowed power spectrum of `samples`, zero-padded to a power of two.
/// The result holds the bins from DC up to, but not including, Nyquist.
fn power_spectrum(samples: &[f64]) -> Vec<f64> {
  let size = samples.len().next_power_of_two().max(2);
  let span = samples.len().saturating_sub(1).max(1) as f64;
  let mut re: Vec<f64> = samples
    .iter()
    .enumerate()
    .map(|(i, s)| s * (0.5 - 0.5 * (TAU * i as f64 / span).cos()))
    .chain(std::iter::repeat(0.0))
    .take(size)
    .collect();
  let mut im = vec![0.0; size];
  fft(&mut re, &mut im);
  re.iter()
    .zip(&im)
    .take(size / 2)
    .map(|(r, i)| r * r + i * i)
    .collect()
}

/// In-place radix-2 FFT.  Both slices must share a power-of-two length.
fn fft(re: &mut [f64], im: &mut [f64]) {
  let n = re.len();
  let mut j = 0;
  for i in 1..n.saturating_sub(1) {
    let mut bit = n >> 1;
    while j & bit != 0 {
      j ^= bit;
      bit >>= 1;
    }
    j |= bit;
    if i < j {
      re.swap(i, j);
      im.swap(i, j);
    }
  }
  let mut len = 2;
  while len <= n {
    let (step_im, step_re) = (-TAU / len as f64).sin_cos();
    for start in (0..n).step_by(len) {
      let (mut w_re, mut w_im) = (1.0, 0.0);
      for k in 0..len / 2 {
        let (a, b) = (start + k, start + k + len / 2);
        let (t_re, t_im) =
          (re[b] * w_re - im[b] * w_im, re[b] * w_im + im[b] * w_re);
        (re[b], im[b]) = (re[a] - t_re, im[a] - t_im);
        (re[a], im[a]) = (re[a] + t_re, im[a] + t_im);
        (w_re, w_im) =
          (w_re * step_re - w_im * step_im, w_re * step_im + w_im * step_re);
      }
    }
    len <<= 1;
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn tone(partials: &[(f64, f64)], rate: f64, seconds: f64) -> Vec<f64> {
    (0..(rate * seconds) as usize)
      .map(|i| i as f64 / rate)
      .map(|t| {
        partials
          .iter()
          .map(|(hz, a)| a * (TAU * hz * t).sin())
          .sum()
      })
      .collect()
  }

  #[test]
  fn harmonic_levels_follow_partial_amplitudes() {
    let samples = tone(&[(100.0, 1.0), (200.0, 0.1)], 8000.0, 1.0);
    let features = Features::measure(&samples, 8000.0, 100.0, 4).unwrap();
    let gap = features.harmonic_db[0] - features.harmonic_db[1];
    // A partial that falls between two bins reads up to a decibel low.
    assert!((gap - 20.0).abs() < 1.0, "expected a 20 dB gap, got {gap}");
  }

  #[test]
  fn silence_has_no_features() {
    assert!(Features::measure(&[0.0; 4096], 8000.0, 100.0, 4).is_none());
  }

  #[test]
  fn power_between_harmonics_is_reported_as_off_harmonic() {
    let clean = tone(&[(100.0, 1.0)], 8000.0, 1.0);
    let dirty = tone(&[(100.0, 1.0), (150.0, 1.0)], 8000.0, 1.0);
    let clean = Features::measure(&clean, 8000.0, 100.0, 8).unwrap();
    let dirty = Features::measure(&dirty, 8000.0, 100.0, 8).unwrap();
    assert!(clean.off_harmonic_db < -20.0, "{}", clean.off_harmonic_db);
    assert!(dirty.off_harmonic_db > -6.0, "{}", dirty.off_harmonic_db);
  }

  #[test]
  fn a_tone_is_closer_to_itself_than_to_another() {
    let a = tone(&[(100.0, 0.2), (200.0, 0.1)], 8000.0, 1.0);
    let b = tone(&[(100.0, 0.04), (300.0, 0.2)], 8000.0, 1.0);
    let a = Features::measure(&a, 8000.0, 100.0, 8).unwrap();
    let b = Features::measure(&b, 8000.0, 100.0, 8).unwrap();
    assert!(a.distance(&a) < 1e-9);
    assert!(a.distance(&b) > 10.0);
  }
}
