use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use fundsp::prelude32::*;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AudioError {
  #[error("No audio output device available")]
  NoOutputDevice,

  #[error("Cannot determine default audio output format: {0}")]
  OutputConfigUnavailable(#[source] cpal::DefaultStreamConfigError),

  #[error("Failed to build audio output stream: {0}")]
  StreamBuildFailed(#[source] cpal::BuildStreamError),

  #[error("Failed to start audio playback: {0}")]
  PlaybackStartFailed(#[source] cpal::PlayStreamError),
}

/// Holds a live cpal stream.  Audio plays as long as this value
/// exists; dropping it stops playback.
pub struct AudioOutput {
  _stream: cpal::Stream,
}

impl AudioOutput {
  /// Open the default audio device and play the given graph.
  pub fn play(mut graph: Box<dyn AudioUnit>) -> Result<Self, AudioError> {
    let host = cpal::default_host();
    let device = host
      .default_output_device()
      .ok_or(AudioError::NoOutputDevice)?;
    let supported = device
      .default_output_config()
      .map_err(AudioError::OutputConfigUnavailable)?;

    let sample_rate = supported.sample_rate().0 as f64;
    let channels = supported.channels() as usize;
    let stream_config: cpal::StreamConfig = supported.into();

    graph.set_sample_rate(sample_rate);
    graph.allocate();

    let stream = device
      .build_output_stream(
        &stream_config,
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
          for frame in data.chunks_mut(channels) {
            let (l, r) = graph.get_stereo();
            frame[0] = l;
            if channels > 1 {
              frame[1] = r;
            }
          }
        },
        |err| tracing::error!("Audio stream error: {}", err),
        None,
      )
      .map_err(AudioError::StreamBuildFailed)?;

    stream.play().map_err(AudioError::PlaybackStartFailed)?;

    Ok(AudioOutput { _stream: stream })
  }

  /// Play a graph for the given duration, then stop.
  pub fn play_for(
    graph: Box<dyn AudioUnit>,
    duration: std::time::Duration,
  ) -> Result<(), AudioError> {
    let _output = Self::play(graph)?;
    std::thread::sleep(duration);
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn graph_produces_samples() {
    let mut graph: Box<dyn AudioUnit> = Box::new(sine_hz(440.0) * 0.5);
    graph.set_sample_rate(44100.0);
    graph.allocate();

    let mut has_nonzero = false;
    for _ in 0..1000 {
      let (l, _r) = graph.get_stereo();
      if l.abs() > 0.001 {
        has_nonzero = true;
        break;
      }
    }
    assert!(has_nonzero, "Sine graph should produce non-zero samples");
  }
}
