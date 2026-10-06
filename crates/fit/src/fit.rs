//! The `fit` subcommand: search for the patch that best matches a recording.

use crate::patch_param::{patch_with_params, ParamAssignment, PatchParamError};
use crate::recording::{self, RecordingError};
use crate::search::{self, Scored, SearchError, Settings, SPACE};
use crate::spectrum::{fundamental_hz, shared_harmonics, Features};
use sonify_health_lib::Patch;
use std::collections::BTreeSet;
use std::path::PathBuf;
use thiserror::Error;
use tracing::info;

#[derive(Clone, Debug, clap::Args)]
pub struct FitArgs {
  /// Recording of the steady tone to match: WAV or MP3.
  #[arg(long)]
  pub target: PathBuf,

  /// Fundamental of the harmonic series to match, in Hz.  For a patch that
  /// mixes in its sub-octave this is half the patch's `freq`.
  #[arg(long, value_parser = fundamental_hz)]
  pub fundamental: f64,

  /// Fundamental of the recording, in Hz, when it sits slightly off the
  /// patch's.  Defaults to --fundamental.
  #[arg(long, value_parser = fundamental_hz)]
  pub target_fundamental: Option<f64>,

  /// Pin a patch parameter: hold it at a fixed value instead of searching
  /// it, as NAME=VALUE.  May be repeated.  Pin `freq` to set the pitch, or
  /// `fm_ratio` and `fm_depth` to fit the rest of a patch around a known
  /// modulation.
  #[arg(long = "param", value_name = "NAME=VALUE")]
  pub params: Vec<ParamAssignment>,

  /// Sample rate candidates are rendered at, in Hz.  Aliasing differs per
  /// rate, so use the output device's own.
  #[arg(
    long,
    default_value_t = 44_100,
    value_parser = clap::value_parser!(u32).range(1..)
  )]
  pub sample_rate: u32,

  /// Frames of each candidate that are measured.  Fewer is faster and resolves
  /// closely spaced harmonics less well; a power of two needs no padding.
  #[arg(
    long,
    default_value_t = 65_536,
    value_parser = clap::value_parser!(u32).range(256..)
  )]
  pub measured_frames: u32,

  /// Random candidates scored before the climbs start.
  #[arg(long, default_value_t = 8000)]
  pub random_samples: usize,

  /// Refinement steps each climb takes.
  #[arg(long, default_value_t = 1200)]
  pub climb_steps: usize,

  /// Seed for the search.  The same seed repeats the same fit.
  #[arg(long, default_value_t = 1)]
  pub seed: u64,

  /// Number of matches to print, best first.
  #[arg(long, default_value_t = 3)]
  pub results: usize,
}

#[derive(Debug, Error)]
pub enum FitError {
  #[error("A --param assignment could not be applied: {0}")]
  PatchParam(#[from] PatchParamError),

  #[error("The target recording could not be loaded: {0}")]
  Target(#[from] RecordingError),

  #[error(
    "The target recording {path:?} is silent, so there is no tone to fit"
  )]
  SilentTarget { path: PathBuf },

  #[error("The search for a matching patch failed: {0}")]
  Search(#[from] SearchError),

  #[error(
    "Every candidate patch rendered silence, so none could be compared with \
     the target; check the pinned parameters"
  )]
  NoAudibleCandidate,
}

pub fn run(args: &FitArgs) -> Result<(), FitError> {
  // A bad pin would otherwise surface only after the search has spent its
  // budget, so the pins are applied to a throwaway patch first.
  patch_with_params(Patch::default(), &args.params)?;
  let recording = recording::load(&args.target)?;
  let sample_rate = f64::from(args.sample_rate);
  let target_fundamental = args.target_fundamental.unwrap_or(args.fundamental);
  let harmonics = shared_harmonics(&[
    (recording.sample_rate, target_fundamental),
    (sample_rate, args.fundamental),
  ]);
  let target = Features::measure(
    &recording.samples,
    recording.sample_rate,
    target_fundamental,
    harmonics,
  )
  .ok_or_else(|| FitError::SilentTarget {
    path: args.target.clone(),
  })?;
  info!(
    target = %args.target.display(),
    harmonics,
    random_samples = args.random_samples,
    climb_steps = args.climb_steps,
    "Searching for a matching patch"
  );

  let results = search::search(
    &target,
    &Settings {
      pinned: &args.params,
      sample_rate,
      fundamental_hz: args.fundamental,
      harmonics,
      measured_frames: args.measured_frames as usize,
      random_samples: args.random_samples,
      climb_steps: args.climb_steps,
      seed: args.seed,
    },
  )?;
  results
    .iter()
    .take(args.results)
    .for_each(|scored| println!("{}", fragment(scored, &args.params)));
  (!results.is_empty())
    .then_some(())
    .ok_or(FitError::NoAudibleCandidate)
}

/// A match as lines that paste into a `[patches.<name>]` table: its searched
/// and pinned parameters, under a comment saying how close it came.
fn fragment(scored: &Scored, pinned: &[ParamAssignment]) -> String {
  let names: BTreeSet<&str> = SPACE
    .iter()
    .map(|dimension| dimension.name)
    .chain(pinned.iter().map(|pin| pin.name.as_str()))
    .collect();
  std::iter::once(format!(
    "# distance {:.2}, crest {:.2}, off-harmonic {:.1} dB, peak {:.2}, rms \
     {:.3}",
    scored.distance,
    scored.features.crest,
    scored.features.off_harmonic_db,
    scored.features.peak,
    scored.features.rms,
  ))
  .chain(names.into_iter().filter_map(|name| {
    scored
      .patch
      .get_param(name)
      .map(|value| format!("{name} = {value:.3}"))
  }))
  .chain(std::iter::once(String::new()))
  .collect::<Vec<_>>()
  .join("\n")
}
