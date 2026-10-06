//! Recorded audio, loaded as the mono samples a measurement works on.

use fundsp::wave::Wave;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RecordingError {
  #[error("The audio file {path:?} could not be read or decoded: {source}")]
  Load {
    path: PathBuf,
    #[source]
    source: fundsp::read::WaveError,
  },

  #[error("The audio file {path:?} holds no audio frames")]
  Empty { path: PathBuf },
}

pub struct Recording {
  pub samples: Vec<f64>,
  pub sample_rate: f64,
}

/// Loads a WAV or MP3 file and mixes its channels down to one.
pub fn load(path: &Path) -> Result<Recording, RecordingError> {
  Wave::load(path)
    .map_err(|source| RecordingError::Load {
      path: path.to_path_buf(),
      source,
    })
    .and_then(|wave| {
      (!wave.is_empty() && wave.channels() > 0)
        .then(|| Recording {
          samples: mono(&wave),
          sample_rate: wave.sample_rate(),
        })
        .ok_or_else(|| RecordingError::Empty {
          path: path.to_path_buf(),
        })
    })
}

fn mono(wave: &Wave) -> Vec<f64> {
  let channels = wave.channels();
  (0..wave.len())
    .map(|frame| {
      (0..channels)
        .map(|channel| f64::from(wave.at(channel, frame)))
        .sum::<f64>()
        / channels as f64
    })
    .collect()
}
