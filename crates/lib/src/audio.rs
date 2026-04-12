use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use fundsp::audiounit::BigBlockAdapter;
use fundsp::prelude32::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use thiserror::Error;

/// Preferred buffer size in frames for a single-graph stream.
/// Larger buffers give the CPU more headroom for expensive graphs
/// (the 32-channel FDN reverb in particular) at the cost of a few
/// milliseconds of latency, which is irrelevant for ambient
/// sonification.
const BUFFER_FRAMES: u32 = 2048;

/// Buffer size for the mixer stream.  The mixer processes multiple
/// graphs sequentially in one callback, so it needs a proportionally
/// larger buffer.  At 44.1 kHz this gives ~93 ms of deadline
/// headroom — enough for two 32-channel FDN reverbs even in an
/// unoptimized debug build.
const MIXER_BUFFER_FRAMES: u32 = 4096;

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

/// Resolve the cpal device and stream config for the given device
/// name.  Shared by `AudioOutput` and `AudioMixer`.
fn resolve_device(
  device_name: Option<&str>,
) -> Result<(cpal::Device, cpal::StreamConfig), AudioError> {
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
        Some(d) => {
          tracing::info!(
            requested = name,
            selected = d.name().unwrap_or_default(),
            "Audio device selected"
          );
          d
        }
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
  tracing::info!(
    sample_rate = supported.sample_rate(),
    channels = supported.channels(),
    sample_format = %supported.sample_format(),
    "Audio device config"
  );
  let mut stream_config: cpal::StreamConfig = supported.into();

  // macOS CoreAudio defaults to tiny buffers that underrun with
  // the 32-channel FDN reverb.  ALSA/PipeWire defaults are already
  // large enough and may reject arbitrary fixed sizes.
  #[cfg(target_os = "macos")]
  {
    stream_config.buffer_size = cpal::BufferSize::Fixed(BUFFER_FRAMES);
  }

  Ok((device, stream_config))
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
    let (device, stream_config) = resolve_device(device_name)?;
    let sample_rate = stream_config.sample_rate as f64;
    let channels = stream_config.channels as usize;

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

// ---------------------------------------------------------------------------
// AudioMixer — single cpal stream that mixes N fundsp graphs.
// ---------------------------------------------------------------------------

/// Residual state from the previous graph during a crossfade.
struct CrossfadeState {
  adapter: BigBlockAdapter,
  left: Vec<f32>,
  right: Vec<f32>,
  remaining_frames: usize,
  total_frames: usize,
}

struct MixerSlot {
  adapter: BigBlockAdapter,
  left: Vec<f32>,
  right: Vec<f32>,
  /// Previous graph being crossfaded out after a `replace()`.
  prev: Option<CrossfadeState>,
}

struct MixerInner {
  slots: Mutex<Vec<Option<MixerSlot>>>,
  /// Number of callbacks where `try_lock` failed.
  lock_failures: AtomicU64,
  /// Number of callbacks where a slot produced non-finite samples.
  nan_frames: AtomicU64,
  /// Peak callback duration in microseconds.
  peak_callback_us: AtomicU64,
}

/// A single cpal output stream that mixes multiple fundsp graphs
/// through software summation.  Each graph occupies a numbered
/// "slot"; slots can be added, removed, or replaced from the main
/// thread while the audio callback keeps running.
pub struct AudioMixer {
  _stream: cpal::Stream,
  inner: Arc<MixerInner>,
  sample_rate: f64,
}

/// Thread-safe handle to the mixer's slot table.  Unlike
/// `AudioMixer`, this does not own the cpal stream and is `Send +
/// Sync`, so it can be shared across threads.
#[derive(Clone)]
pub struct MixerHandle {
  inner: Arc<MixerInner>,
  sample_rate: f64,
}

/// Build the mixer audio callback for the given shared state and
/// channel count.
fn mixer_callback(
  inner: Arc<MixerInner>,
  channels: usize,
) -> impl FnMut(&mut [f32], &cpal::OutputCallbackInfo) {
  move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
    let start = std::time::Instant::now();

    // Zero the output buffer.
    for sample in data.iter_mut() {
      *sample = 0.0;
    }

    // If the main thread holds the lock (graph add/remove),
    // output silence for this buffer rather than blocking the
    // audio thread.
    let Ok(mut slots) = inner.slots.try_lock() else {
      inner.lock_failures.fetch_add(1, Ordering::Relaxed);
      return;
    };

    let frames = data.len() / channels;
    for slot in slots.iter_mut().flatten() {
      slot.left.resize(frames, 0.0);
      slot.right.resize(frames, 0.0);
      slot.adapter.process_big(
        frames,
        &[],
        &mut [&mut slot.left, &mut slot.right],
      );

      // Guard against NaN/Inf from any graph — a single
      // non-finite sample would corrupt the entire summed
      // output and macOS renders NaN as silence.
      let finite = slot
        .left
        .iter()
        .chain(slot.right.iter())
        .all(|s| s.is_finite());
      if !finite {
        inner.nan_frames.fetch_add(1, Ordering::Relaxed);
        continue;
      }

      // Crossfade: blend the previous graph out while the new
      // graph fades in over CROSSFADE_FRAMES.
      if let Some(prev) = slot.prev.as_mut() {
        prev.left.resize(frames, 0.0);
        prev.right.resize(frames, 0.0);
        prev.adapter.process_big(
          frames,
          &[],
          &mut [&mut prev.left, &mut prev.right],
        );

        let prev_finite = prev
          .left
          .iter()
          .chain(prev.right.iter())
          .all(|s| s.is_finite());

        for (i, frame) in data.chunks_mut(channels).enumerate() {
          // fade ramps from 1→0 over the crossfade window;
          // once remaining_frames hits 0 we output 100% new.
          let fade = if prev.remaining_frames > 0 {
            prev.remaining_frames -= 1;
            prev.remaining_frames as f32 / prev.total_frames as f32
          } else {
            0.0
          };
          let new_gain = 1.0 - fade;
          let old_l = if prev_finite { prev.left[i] } else { 0.0 };
          let old_r = if prev_finite { prev.right[i] } else { 0.0 };
          frame[0] += slot.left[i] * new_gain + old_l * fade;
          if channels > 1 {
            frame[1] += slot.right[i] * new_gain + old_r * fade;
          }
        }

        if prev.remaining_frames == 0 {
          slot.prev = None;
        }
      } else {
        for (i, frame) in data.chunks_mut(channels).enumerate() {
          frame[0] += slot.left[i];
          if channels > 1 {
            frame[1] += slot.right[i];
          }
        }
      }
    }

    let elapsed = start.elapsed().as_micros() as u64;
    inner.peak_callback_us.fetch_max(elapsed, Ordering::Relaxed);
  }
}

impl AudioMixer {
  /// Open one cpal output stream on the given device.  All graphs
  /// added via `add` are mixed together in the audio callback.
  pub fn new(device_name: Option<&str>) -> Result<Self, AudioError> {
    let (device, mut stream_config) = resolve_device(device_name)?;

    // Override the single-graph buffer size with the larger mixer
    // budget so the callback has enough time for multiple graphs.
    #[cfg(target_os = "macos")]
    {
      stream_config.buffer_size = cpal::BufferSize::Fixed(MIXER_BUFFER_FRAMES);
    }
    let sample_rate = stream_config.sample_rate as f64;
    let channels = stream_config.channels as usize;

    let inner = Arc::new(MixerInner {
      slots: Mutex::new(Vec::new()),
      lock_failures: AtomicU64::new(0),
      nan_frames: AtomicU64::new(0),
      peak_callback_us: AtomicU64::new(0),
    });

    let err_cb = |err| tracing::error!("Audio mixer stream error: {}", err);

    // Try fixed buffer size first; fall back to the device default
    // if the hardware rejects it.
    let stream = device
      .build_output_stream(
        &stream_config,
        mixer_callback(Arc::clone(&inner), channels),
        err_cb,
        None,
      )
      .or_else(|e| {
        tracing::warn!(
          error = %e,
          buffer_frames = MIXER_BUFFER_FRAMES,
          "Fixed buffer size rejected, falling back to device default"
        );
        stream_config.buffer_size = cpal::BufferSize::Default;
        device.build_output_stream(
          &stream_config,
          mixer_callback(Arc::clone(&inner), channels),
          |err| tracing::error!("Audio mixer stream error: {}", err),
          None,
        )
      })
      .map_err(AudioError::StreamBuildFailed)?;

    stream.play().map_err(AudioError::PlaybackStartFailed)?;

    Ok(AudioMixer {
      _stream: stream,
      inner,
      sample_rate,
    })
  }

  /// Add a graph to the mixer and return its slot ID.
  pub fn add(&self, mut graph: Box<dyn AudioUnit>) -> usize {
    graph.set_sample_rate(self.sample_rate);
    graph.allocate();
    let slot = MixerSlot {
      adapter: BigBlockAdapter::new(graph),
      left: Vec::with_capacity(MIXER_BUFFER_FRAMES as usize),
      right: Vec::with_capacity(MIXER_BUFFER_FRAMES as usize),
      prev: None,
    };

    let mut slots = self.inner.slots.lock().unwrap();

    // Reuse the first empty position.
    let empty = slots.iter().position(Option::is_none);
    let slot_id = match empty {
      Some(i) => {
        slots[i] = Some(slot);
        i
      }
      None => {
        slots.push(Some(slot));
        slots.len() - 1
      }
    };

    let active = slots.iter().filter(|s| s.is_some()).count();
    tracing::info!(
      slot_id,
      active,
      lock_failures = self.inner.lock_failures.load(Ordering::Relaxed),
      nan_frames = self.inner.nan_frames.load(Ordering::Relaxed),
      peak_callback_us = self.inner.peak_callback_us.load(Ordering::Relaxed),
      "Mixer: slot added"
    );
    slot_id
  }

  /// Remove the graph at the given slot, silencing it.
  pub fn remove(&self, id: usize) {
    let mut slots = self.inner.slots.lock().unwrap();
    if let Some(entry) = slots.get_mut(id) {
      *entry = None;
    }
    let active = slots.iter().filter(|s| s.is_some()).count();
    tracing::info!(
      id,
      active,
      lock_failures = self.inner.lock_failures.load(Ordering::Relaxed),
      nan_frames = self.inner.nan_frames.load(Ordering::Relaxed),
      peak_callback_us = self.inner.peak_callback_us.load(Ordering::Relaxed),
      "Mixer: slot removed"
    );
  }

  /// Replace the graph at the given slot in-place, crossfading
  /// from the old graph to the new one over `crossfade_frames`.
  pub fn replace(
    &self,
    id: usize,
    mut graph: Box<dyn AudioUnit>,
    crossfade_frames: usize,
  ) {
    graph.set_sample_rate(self.sample_rate);
    graph.allocate();
    let new_adapter = BigBlockAdapter::new(graph);
    let frames = Ord::max(crossfade_frames, 1);

    let mut slots = self.inner.slots.lock().unwrap();
    if id < slots.len() {
      if let Some(old_slot) = slots[id].take() {
        slots[id] = Some(MixerSlot {
          adapter: new_adapter,
          left: Vec::with_capacity(MIXER_BUFFER_FRAMES as usize),
          right: Vec::with_capacity(MIXER_BUFFER_FRAMES as usize),
          prev: Some(CrossfadeState {
            adapter: old_slot.adapter,
            left: old_slot.left,
            right: old_slot.right,
            remaining_frames: frames,
            total_frames: frames,
          }),
        });
      } else {
        slots[id] = Some(MixerSlot {
          adapter: new_adapter,
          left: Vec::with_capacity(MIXER_BUFFER_FRAMES as usize),
          right: Vec::with_capacity(MIXER_BUFFER_FRAMES as usize),
          prev: None,
        });
      }
    } else {
      slots.resize_with(id + 1, || None);
      slots[id] = Some(MixerSlot {
        adapter: new_adapter,
        left: Vec::with_capacity(MIXER_BUFFER_FRAMES as usize),
        right: Vec::with_capacity(MIXER_BUFFER_FRAMES as usize),
        prev: None,
      });
    }
  }

  /// Remove all graphs from the mixer.
  pub fn clear(&self) {
    let mut slots = self.inner.slots.lock().unwrap();
    slots.clear();
  }

  /// Obtain a lightweight, thread-safe handle for adding and
  /// removing slots from other threads.
  pub fn handle(&self) -> MixerHandle {
    MixerHandle {
      inner: Arc::clone(&self.inner),
      sample_rate: self.sample_rate,
    }
  }

  /// Number of audio callbacks where the slot lock could not be
  /// acquired (main thread was adding/removing graphs).
  pub fn lock_failures(&self) -> u64 {
    self.inner.lock_failures.load(Ordering::Relaxed)
  }

  /// Number of audio callbacks where a graph produced NaN/Inf
  /// samples.
  pub fn nan_frames(&self) -> u64 {
    self.inner.nan_frames.load(Ordering::Relaxed)
  }

  /// Peak audio callback duration in microseconds since last reset.
  pub fn peak_callback_us(&self) -> u64 {
    self.inner.peak_callback_us.load(Ordering::Relaxed)
  }

  /// Reset the peak callback duration counter.
  pub fn reset_peak_callback_us(&self) {
    self.inner.peak_callback_us.store(0, Ordering::Relaxed);
  }
}

impl MixerHandle {
  /// Add a graph to the mixer and return its slot ID.
  pub fn add(&self, mut graph: Box<dyn AudioUnit>) -> usize {
    graph.set_sample_rate(self.sample_rate);
    graph.allocate();
    let slot = MixerSlot {
      adapter: BigBlockAdapter::new(graph),
      left: Vec::with_capacity(MIXER_BUFFER_FRAMES as usize),
      right: Vec::with_capacity(MIXER_BUFFER_FRAMES as usize),
      prev: None,
    };

    let mut slots = self.inner.slots.lock().unwrap();

    let empty = slots.iter().position(Option::is_none);
    let slot_id = match empty {
      Some(i) => {
        slots[i] = Some(slot);
        i
      }
      None => {
        slots.push(Some(slot));
        slots.len() - 1
      }
    };

    let active = slots.iter().filter(|s| s.is_some()).count();
    tracing::info!(slot_id, active, "MixerHandle: slot added");
    slot_id
  }

  /// Remove the graph at the given slot, silencing it.
  pub fn remove(&self, id: usize) {
    let mut slots = self.inner.slots.lock().unwrap();
    if let Some(entry) = slots.get_mut(id) {
      *entry = None;
    }
    let active = slots.iter().filter(|s| s.is_some()).count();
    tracing::info!(id, active, "MixerHandle: slot removed");
  }

  /// Replace the graph at the given slot, crossfading from the old
  /// graph to the new one over `crossfade_frames`.
  pub fn replace(
    &self,
    id: usize,
    mut graph: Box<dyn AudioUnit>,
    crossfade_frames: usize,
  ) {
    graph.set_sample_rate(self.sample_rate);
    graph.allocate();
    let new_adapter = BigBlockAdapter::new(graph);
    let frames = Ord::max(crossfade_frames, 1);

    let mut slots = self.inner.slots.lock().unwrap();
    if id < slots.len() {
      if let Some(old_slot) = slots[id].take() {
        slots[id] = Some(MixerSlot {
          adapter: new_adapter,
          left: Vec::with_capacity(MIXER_BUFFER_FRAMES as usize),
          right: Vec::with_capacity(MIXER_BUFFER_FRAMES as usize),
          prev: Some(CrossfadeState {
            adapter: old_slot.adapter,
            left: old_slot.left,
            right: old_slot.right,
            remaining_frames: frames,
            total_frames: frames,
          }),
        });
      } else {
        slots[id] = Some(MixerSlot {
          adapter: new_adapter,
          left: Vec::with_capacity(MIXER_BUFFER_FRAMES as usize),
          right: Vec::with_capacity(MIXER_BUFFER_FRAMES as usize),
          prev: None,
        });
      }
    } else {
      slots.resize_with(id + 1, || None);
      slots[id] = Some(MixerSlot {
        adapter: new_adapter,
        left: Vec::with_capacity(MIXER_BUFFER_FRAMES as usize),
        right: Vec::with_capacity(MIXER_BUFFER_FRAMES as usize),
        prev: None,
      });
    }
  }

  pub fn sample_rate(&self) -> f64 {
    self.sample_rate
  }

  /// Number of audio callbacks where the slot lock could not be
  /// acquired.
  pub fn lock_failures(&self) -> u64 {
    self.inner.lock_failures.load(Ordering::Relaxed)
  }

  /// Number of audio callbacks where a graph produced NaN/Inf
  /// samples.
  pub fn nan_frames(&self) -> u64 {
    self.inner.nan_frames.load(Ordering::Relaxed)
  }

  /// Peak audio callback duration in microseconds since last reset.
  pub fn peak_callback_us(&self) -> u64 {
    self.inner.peak_callback_us.load(Ordering::Relaxed)
  }

  /// Reset the peak callback duration counter.
  pub fn reset_peak_callback_us(&self) {
    self.inner.peak_callback_us.store(0, Ordering::Relaxed);
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

  /// Verify that processing two BigBlockAdapters sequentially
  /// (as the mixer callback does) produces a valid summed output
  /// with contributions from both graphs and no NaN.
  #[test]
  fn sequential_adapters_mix_without_nan() {
    let frames = 2048;

    let mut graph_a: Box<dyn AudioUnit> =
      Box::new(sine_hz(440.0) * 0.5 >> pan(0.0));
    graph_a.set_sample_rate(44100.0);
    graph_a.allocate();
    let mut adapter_a = BigBlockAdapter::new(graph_a);

    let mut graph_b: Box<dyn AudioUnit> =
      Box::new(sine_hz(880.0) * 0.3 >> pan(0.0));
    graph_b.set_sample_rate(44100.0);
    graph_b.allocate();
    let mut adapter_b = BigBlockAdapter::new(graph_b);

    let mut left_a = vec![0.0f32; frames];
    let mut right_a = vec![0.0f32; frames];
    let mut left_b = vec![0.0f32; frames];
    let mut right_b = vec![0.0f32; frames];

    adapter_a.process_big(frames, &[], &mut [&mut left_a, &mut right_a]);
    adapter_b.process_big(frames, &[], &mut [&mut left_b, &mut right_b]);

    // Sum (as the mixer callback does).
    let mixed_left: Vec<f32> = left_a
      .iter()
      .zip(left_b.iter())
      .map(|(a, b)| a + b)
      .collect();

    assert!(
      mixed_left.iter().all(|s| s.is_finite()),
      "Mixed output must not contain NaN or Inf"
    );
    assert!(
      left_a.iter().any(|s| s.abs() > 0.001),
      "Graph A should produce non-zero samples"
    );
    assert!(
      left_b.iter().any(|s| s.abs() > 0.001),
      "Graph B should produce non-zero samples"
    );
  }

  /// Verify that health accessors return initial values and that
  /// reset_peak_callback_us works.  Skipped if no audio device is
  /// available (CI).
  #[test]
  fn mixer_handle_health_accessors() {
    let mixer = match AudioMixer::new(None) {
      Ok(m) => m,
      Err(_) => {
        eprintln!("No audio device available, skipping test");
        return;
      }
    };

    assert_eq!(mixer.lock_failures(), 0);
    assert_eq!(mixer.nan_frames(), 0);
    // peak_callback_us may already be non-zero from the running
    // stream, so just test the reset.
    mixer.reset_peak_callback_us();
    assert_eq!(mixer.peak_callback_us(), 0);

    let handle = mixer.handle();
    assert_eq!(handle.lock_failures(), 0);
    assert_eq!(handle.nan_frames(), 0);
    handle.reset_peak_callback_us();
    assert_eq!(handle.peak_callback_us(), 0);
  }
}
