use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use fundsp::audiounit::BigBlockAdapter;
use fundsp::prelude32::*;
use thiserror::Error;

/// Preferred buffer size in frames.  Larger buffers give the CPU
/// more headroom for expensive graphs (the 32-channel FDN reverb
/// in particular) at the cost of a few milliseconds of latency,
/// which is irrelevant for ambient sonification.
const BUFFER_FRAMES: u32 = 1024;

#[derive(Debug, Error)]
pub enum AudioError {
  #[error("No audio output device available")]
  NoOutputDevice,

  #[error("Audio device not found: {0}")]
  DeviceNotFound(String),

  #[error("Failed to enumerate audio output devices: {0}")]
  DeviceEnumeration(#[source] cpal::DevicesError),

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
  /// Open an audio device and play the given graph.
  ///
  /// When `device_name` is `Some`, the named output device is used
  /// (substring match, case-insensitive).  When `None`, the system
  /// default output device is used.
  pub fn play(
    mut graph: Box<dyn AudioUnit>,
    device_name: Option<&str>,
  ) -> Result<Self, AudioError> {
    let host = cpal::default_host();
    let device = match device_name {
      Some(name) => {
        let lower = name.to_lowercase();
        let devices: Vec<_> = host
          .output_devices()
          .map_err(AudioError::DeviceEnumeration)?
          .collect();
        let matched = devices.into_iter().find(|d| {
          d.name()
            .map(|n| n.to_lowercase().contains(&lower))
            .unwrap_or(false)
        });
        match matched {
          Some(d) => d,
          None => {
            let available: Vec<String> = host
              .output_devices()
              .map_err(AudioError::DeviceEnumeration)?
              .filter_map(|d| d.name().ok())
              .collect();
            tracing::warn!(
              requested = name,
              ?available,
              "Audio device not found"
            );
            return Err(AudioError::DeviceNotFound(name.to_string()));
          }
        }
      }
      None => host
        .default_output_device()
        .ok_or(AudioError::NoOutputDevice)?,
    };
    let supported = device
      .default_output_config()
      .map_err(AudioError::OutputConfigUnavailable)?;

    let sample_rate = supported.sample_rate().0 as f64;
    let channels = supported.channels() as usize;
    let mut stream_config: cpal::StreamConfig = supported.into();
    stream_config.buffer_size = cpal::BufferSize::Fixed(BUFFER_FRAMES);

    graph.set_sample_rate(sample_rate);
    graph.allocate();

    // Block adapter lets fundsp vectorize its inner loops
    // instead of computing one sample at a time.
    let mut adapter = BigBlockAdapter::new(graph);
    let mut left_buf: Vec<f32> = Vec::with_capacity(BUFFER_FRAMES as usize);
    let mut right_buf: Vec<f32> = Vec::with_capacity(BUFFER_FRAMES as usize);

    let stream = device
      .build_output_stream(
        &stream_config,
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
          let frames = data.len() / channels;
          left_buf.resize(frames, 0.0);
          right_buf.resize(frames, 0.0);
          adapter.process_big(
            frames,
            &[],
            &mut [&mut left_buf, &mut right_buf],
          );
          for (i, frame) in data.chunks_mut(channels).enumerate() {
            frame[0] = left_buf[i];
            if channels > 1 {
              frame[1] = right_buf[i];
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

  /// Play a graph on the given device for the given duration, then stop.
  pub fn play_for(
    graph: Box<dyn AudioUnit>,
    duration: std::time::Duration,
    device_name: Option<&str>,
  ) -> Result<(), AudioError> {
    let _output = Self::play(graph, device_name)?;
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
