//! The `render` subcommand: one patch synthesized offline to a WAV file.

use crate::patch_param::{patch_with_params, ParamAssignment, PatchParamError};
use crate::synth;
use fundsp::wave::Wave;
use sonify_health_lib::Patch;
use std::num::ParseFloatError;
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::info;

#[derive(Clone, Debug, clap::Args)]
pub struct RenderArgs {
  /// WAV file to write: stereo, 32-bit float.
  #[arg(long)]
  pub output: PathBuf,

  /// Length of audio to render, in seconds.  Defaults to the note's own
  /// length, through its release and echo tail.
  #[arg(long, value_parser = render_seconds)]
  pub seconds: Option<f64>,

  /// Sample rate in Hz.
  #[arg(
    long,
    default_value_t = 44_100,
    value_parser = clap::value_parser!(u32).range(1..)
  )]
  pub sample_rate: u32,

  /// Set a patch parameter over the default patch, as NAME=VALUE.  May be
  /// repeated; a value past the parameter's hard limits is held at them.
  #[arg(long = "param", value_name = "NAME=VALUE")]
  pub params: Vec<ParamAssignment>,
}

#[derive(Debug, Error)]
pub enum RenderSecondsParseError {
  #[error("{argument:?} is not a number of seconds: {source}")]
  NotANumber {
    argument: String,
    #[source]
    source: ParseFloatError,
  },

  #[error(
    "the length must be a finite number of seconds above zero, got {seconds}"
  )]
  NotPositive { seconds: f64 },
}

fn render_seconds(argument: &str) -> Result<f64, RenderSecondsParseError> {
  argument
    .parse::<f64>()
    .map_err(|source| RenderSecondsParseError::NotANumber {
      argument: argument.to_string(),
      source,
    })
    .and_then(|seconds| {
      (seconds.is_finite() && seconds > 0.0)
        .then_some(seconds)
        .ok_or(RenderSecondsParseError::NotPositive { seconds })
    })
}

#[derive(Debug, Error)]
pub enum RenderError {
  #[error("A --param assignment could not be applied: {0}")]
  PatchParam(#[from] PatchParamError),

  #[error("The rendered audio could not be encoded as WAV: {source}")]
  WavEncode {
    #[source]
    source: std::io::Error,
  },

  #[error("The rendered audio could not be written to {path:?}: {source}")]
  OutputWrite {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },
}

pub fn run(args: &RenderArgs) -> Result<(), RenderError> {
  let wave = patch_with_params(Patch::default(), &args.params)
    .map(|patch| note_wave(patch, args.seconds, f64::from(args.sample_rate)))?;
  write_wav(&wave, &args.output)?;
  info!(
    output = %args.output.display(),
    frames = wave.len(),
    sample_rate = args.sample_rate,
    peak = wave.amplitude(),
    "Rendered patch"
  );
  Ok(())
}

/// One note of `patch` as a stereo wave `seconds` long, or as long as the note
/// itself when no length is given.
fn note_wave(patch: Patch, seconds: Option<f64>, sample_rate: f64) -> Wave {
  let frames = (seconds.unwrap_or_else(|| synth::note_seconds(&patch))
    * sample_rate)
    .round() as usize;
  synth::note_frames(patch, sample_rate).take(frames).fold(
    Wave::with_capacity(2, sample_rate, frames),
    |mut wave, frame| {
      wave.push(frame);
      wave
    },
  )
}

fn write_wav(wave: &Wave, path: &Path) -> Result<(), RenderError> {
  // The WAV is encoded into memory and written with `fs::write` rather than
  // saved directly via `Wave::save_wav32`.  `Wave::save_wav32` could report
  // success for a truncated file.  It writes through a `BufWriter` it never
  // flushes, and the flush that runs on drop discards any I/O error.  This was
  // discovered by an LLM agent reviewing the fundsp code.
  let mut encoded = Vec::new();
  wave
    .write_wav32(&mut encoded)
    .map_err(|source| RenderError::WavEncode { source })?;
  std::fs::write(path, encoded).map_err(|source| RenderError::OutputWrite {
    path: path.to_path_buf(),
    source,
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn note_wave_is_stereo_at_the_requested_length() {
    let wave = note_wave(Patch::default(), Some(0.25), 48_000.0);
    assert_eq!(wave.channels(), 2);
    assert_eq!(wave.len(), 12_000);
    assert_eq!(wave.sample_rate(), 48_000.0);
  }

  #[test]
  fn note_wave_defaults_to_the_note_length() {
    let patch = Patch::default();
    let expected = (synth::note_seconds(&patch) * 44_100.0).round() as usize;
    assert_eq!(note_wave(patch, None, 44_100.0).len(), expected);
  }

  #[test]
  fn render_seconds_accepts_a_positive_length() {
    assert_eq!(render_seconds("1.5").unwrap(), 1.5);
  }

  #[test]
  fn render_seconds_rejects_lengths_that_render_nothing() {
    for argument in ["0", "-1", "nan", "inf"] {
      assert!(
        matches!(
          render_seconds(argument),
          Err(RenderSecondsParseError::NotPositive { .. })
        ),
        "{argument:?} should be rejected as not positive"
      );
    }
  }

  #[test]
  fn render_seconds_rejects_text() {
    assert!(matches!(
      render_seconds("long"),
      Err(RenderSecondsParseError::NotANumber { .. })
    ));
  }
}
