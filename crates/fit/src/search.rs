//! Random search followed by hill climbing over a patch parameter space.

use crate::patch_param::ParamAssignment;
use crate::spectrum::Features;
use crate::synth;
use sonify_health_lib::Patch;
use std::thread::{self, ScopedJoinHandle};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SearchError {
  #[error(
    "A search worker thread panicked while scoring candidate patches, so the \
     search has no trustworthy result"
  )]
  WorkerPanicked,
}

/// How a dimension's unit coordinate maps onto its range.
#[derive(Debug, Clone, Copy)]
pub enum Scale {
  Linear,
  Logarithmic,
}

/// One searchable patch parameter.
#[derive(Debug, Clone, Copy)]
pub struct Dimension {
  pub name: &'static str,
  pub low: f64,
  pub high: f64,
  pub scale: Scale,
}

impl Dimension {
  const fn linear(name: &'static str, low: f64, high: f64) -> Self {
    Self {
      name,
      low,
      high,
      scale: Scale::Linear,
    }
  }

  const fn logarithmic(name: &'static str, low: f64, high: f64) -> Self {
    Self {
      name,
      low,
      high,
      scale: Scale::Logarithmic,
    }
  }

  fn value(&self, unit: f64) -> f64 {
    let unit = unit.clamp(0.0, 1.0);
    match self.scale {
      Scale::Linear => self.low + (self.high - self.low) * unit,
      Scale::Logarithmic => self.low * (self.high / self.low).powf(unit),
    }
  }
}

/// The parameters a search varies.
///
/// A frequency-modulation depth above 1 drives the oscillator's instantaneous
/// frequency negative, where the synth clamps it, so the note drifts sharp of
/// the fundamental the measurement assumes.  `fm_*` and `vibrato_*` therefore
/// stay out of the space; pin them to fit around a fixed setting.
///
/// Crush runs after the amplitude envelope, so a coarse setting sounds
/// different at every level a gradient passes through.  Its range stops where
/// that becomes audible at the nominal amplitude.
pub const SPACE: &[Dimension] = &[
  Dimension::linear("sine_ratio", 0.0, 1.0),
  Dimension::linear("tri_ratio", 0.0, 1.0),
  Dimension::linear("saw_ratio", 0.0, 1.0),
  Dimension::linear("square_ratio", 0.0, 1.0),
  Dimension::linear("sub_octave", 0.0, 1.0),
  // Put this back in when the sub_phase is added.
  // Dimension::linear("sub_phase", 0.0, 1.0),
  Dimension::logarithmic("drive", 0.3, 10.0),
  Dimension::logarithmic("brightness", 0.08, 2.0),
  Dimension::linear("resonance", 0.3, 5.0),
  Dimension::linear("crush", 0.0, 0.45),
  Dimension::linear("downsample", 0.0, 0.9),
  Dimension::logarithmic("lowpass", 200.0, 8000.0),
];

/// Amplitude every candidate is rendered at.
const NOMINAL_AMPLITUDE: f64 = 0.3;

/// A candidate rendered at the nominal amplitude that exceeds either of these
/// has a resonance feeding on itself rather than a louder timbre, and is not a
/// patch anyone could play.
const RUNAWAY_RMS: f64 = 0.35;
const RUNAWAY_PEAK: f64 = 0.9;

/// Seconds of rendered audio discarded before measuring, which lets the
/// attack and the filters' start-up transients die away.
const SETTLE_SECONDS: f64 = 0.6;

/// Share of coordinates a climbing step perturbs.
const MUTATION_RATE: f64 = 0.35;

/// Width of a climbing step's perturbation, as a share of each dimension's
/// range, at the start and at the end of the climb.
const STEP_START: f64 = 0.20;
const STEP_END: f64 = 0.02;

pub struct Settings<'a> {
  /// Parameters held at fixed values; these win over the search space.
  pub pinned: &'a [ParamAssignment],
  pub sample_rate: f64,
  pub fundamental_hz: f64,
  pub harmonics: usize,
  /// Frames of each candidate that are measured, after the settling time.
  pub measured_frames: usize,
  pub random_samples: usize,
  pub climb_steps: usize,
  pub seed: u64,
}

/// A candidate patch with its measurements and its distance from the target.
pub struct Scored {
  pub distance: f64,
  pub patch: Patch,
  pub features: Features,
}

/// Patch for a point in the search space.  The envelope is forced to a long
/// flat sustain so the measured frames hold a steady tone.
fn candidate(coords: &[f64], pinned: &[ParamAssignment]) -> Patch {
  let steady = [
    ("duration", 4.0),
    ("sustain", 1.0),
    ("decay_ms", 0.0),
    ("amplitude", NOMINAL_AMPLITUDE),
  ];
  steady
    .into_iter()
    .chain(
      SPACE
        .iter()
        .zip(coords)
        .map(|(d, unit)| (d.name, d.value(*unit))),
    )
    .chain(pinned.iter().map(|pin| (pin.name.as_str(), pin.value)))
    .fold(Patch::default(), |mut patch, (name, value)| {
      patch.set_param(name, value);
      patch
    })
}

/// Measures a patch the way the search does.  `None` means it rendered
/// silence.
fn measure(patch: &Patch, settings: &Settings) -> Option<Features> {
  let settle = (SETTLE_SECONDS * settings.sample_rate) as usize;
  let samples = synth::settled_mono(
    patch,
    settings.sample_rate,
    settle,
    settings.measured_frames,
  );
  Features::measure(
    &samples,
    settings.sample_rate,
    settings.fundamental_hz,
    settings.harmonics,
  )
}

fn score(coords: &[f64], target: &Features, settings: &Settings) -> f64 {
  measure(&candidate(coords, settings.pinned), settings)
    .filter(|features| {
      features.rms <= RUNAWAY_RMS && features.peak <= RUNAWAY_PEAK
    })
    .map_or(f64::INFINITY, |features| target.distance(&features))
}

/// Xorshift generator.  Reproducible from its seed, which lets a fit be
/// re-run exactly.
struct Rng(u64);

impl Rng {
  /// Xorshift sticks at an all-zero state, and adjacent seeds would start
  /// workers on correlated sequences, so the seed and stream are mixed before
  /// use.
  fn new(seed: u64, stream: u64) -> Self {
    let mixed = (seed ^ stream.wrapping_mul(0x9E37_79B9_7F4A_7C15))
      .wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    Self((mixed ^ (mixed >> 31)).max(1))
  }

  fn unit(&mut self) -> f64 {
    self.0 ^= self.0 << 13;
    self.0 ^= self.0 >> 7;
    self.0 ^= self.0 << 17;
    (self.0 >> 11) as f64 / (1u64 << 53) as f64
  }

  /// Roughly normal, from the sum of six uniform draws.
  fn bell(&mut self) -> f64 {
    (0..6).map(|_| self.unit()).sum::<f64>() - 3.0
  }
}

type Point = (f64, Vec<f64>);

fn join_all<T>(
  handles: Vec<ScopedJoinHandle<'_, T>>,
) -> Result<Vec<T>, SearchError> {
  handles
    .into_iter()
    .map(|handle| handle.join().map_err(|_| SearchError::WorkerPanicked))
    .collect()
}

fn scatter(
  target: &Features,
  settings: &Settings,
  workers: usize,
) -> Result<Vec<Point>, SearchError> {
  let per_worker = settings.random_samples.div_ceil(workers);
  thread::scope(|scope| {
    join_all(
      (0..workers)
        .map(|worker| {
          scope.spawn(move || {
            let mut rng = Rng::new(settings.seed, worker as u64);
            (0..per_worker)
              .map(|_| SPACE.iter().map(|_| rng.unit()).collect::<Vec<f64>>())
              .map(|coords| (score(&coords, target, settings), coords))
              .collect::<Vec<Point>>()
          })
        })
        .collect(),
    )
  })
  .map(|batches| batches.into_iter().flatten().collect())
}

fn climb(
  start: Point,
  stream: u64,
  target: &Features,
  settings: &Settings,
) -> Point {
  let mut rng = Rng::new(settings.seed, stream);
  let steps = settings.climb_steps;
  (0..steps).fold(start, |(best, coords), step| {
    let progress = step as f64 / steps.max(1) as f64;
    let width = STEP_START + (STEP_END - STEP_START) * progress;
    let trial: Vec<f64> = coords
      .iter()
      .map(|c| {
        if rng.unit() < MUTATION_RATE {
          (c + width * rng.bell()).clamp(0.0, 1.0)
        } else {
          *c
        }
      })
      .collect();
    let distance = score(&trial, target, settings);
    if distance < best {
      (distance, trial)
    } else {
      (best, coords)
    }
  })
}

/// Searches `SPACE` for the patches closest to `target`, best first.  One
/// result is returned per worker thread; they are independent climbs, so
/// agreement between them is evidence the match is not a fluke.
pub fn search(
  target: &Features,
  settings: &Settings,
) -> Result<Vec<Scored>, SearchError> {
  let workers = thread::available_parallelism().map_or(1, |n| n.get());
  let mut seeds = scatter(target, settings, workers)?;
  seeds.sort_by(|a, b| a.0.total_cmp(&b.0));
  seeds.truncate(workers);

  let climbed = thread::scope(|scope| {
    join_all(
      seeds
        .into_iter()
        .enumerate()
        .map(|(index, start)| {
          let stream = (workers + index) as u64;
          scope.spawn(move || climb(start, stream, target, settings))
        })
        .collect(),
    )
  })?;

  let mut scored: Vec<Scored> = climbed
    .into_iter()
    .filter(|(distance, _)| distance.is_finite())
    .filter_map(|(distance, coords)| {
      let patch = candidate(&coords, settings.pinned);
      measure(&patch, settings).map(|features| Scored {
        distance,
        patch,
        features,
      })
    })
    .collect();
  scored.sort_by(|a, b| a.distance.total_cmp(&b.distance));
  Ok(scored)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn logarithmic_dimension_spans_its_range_geometrically() {
    let d = Dimension::logarithmic("drive", 1.0, 100.0);
    assert!((d.value(0.0) - 1.0).abs() < 1e-9);
    assert!((d.value(0.5) - 10.0).abs() < 1e-9);
    assert!((d.value(1.0) - 100.0).abs() < 1e-9);
  }

  #[test]
  fn pinned_parameters_win_over_the_space() {
    let coords = vec![0.5; SPACE.len()];
    let pin = |name: &str, value| ParamAssignment {
      name: name.to_string(),
      value,
    };
    let pinned = [pin("drive", 7.0), pin("freq", 66.0)];
    let patch = candidate(&coords, &pinned);
    assert_eq!(patch.drive, 7.0);
    assert_eq!(patch.freq, 66.0);
  }

  #[test]
  fn every_dimension_names_a_real_parameter() {
    let mut patch = Patch::default();
    for d in SPACE {
      assert!(patch.set_param(d.name, d.low), "unknown: {}", d.name);
    }
  }

  #[test]
  fn generator_is_reproducible_and_streams_differ() {
    let draw = |seed, stream| {
      let mut rng = Rng::new(seed, stream);
      (0..4).map(|_| rng.unit()).collect::<Vec<f64>>()
    };
    assert_eq!(draw(1, 0), draw(1, 0));
    assert_ne!(draw(1, 0), draw(1, 1));
    assert!(draw(0, 0).iter().all(|u| (0.0..1.0).contains(u)));
  }
}
