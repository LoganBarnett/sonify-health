use crate::voice::Voice;
use clap::ValueEnum;
use fundsp::prelude32::*;
use fundsp::shared::Shared;
use serde::Deserialize;

/// Pitch register for a drone voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum DroneRegister {
  Low,
  Mid,
  High,
}

impl DroneRegister {
  /// Frequency multiplier relative to the voice's base.
  fn multiplier(self) -> f32 {
    match self {
      DroneRegister::Low => 0.5,
      DroneRegister::Mid => 1.0,
      DroneRegister::High => 2.0,
    }
  }
}

/// Build a single drone graph whose timbre and volume shift in
/// real time with the given metric shared variable.
///
/// The aesthetic target is periodic bell/bong tones with long
/// reverb tails that overlap into a continuous ambient bed.
/// Event rate and brightness encode the metric value.
///
/// Graph structure:
///   voice_blend(sine + triangle + saw)
///     → lowpole(200–1500 Hz cutoff)
///     × bong_envelope(exp-decay pulse, rate 0.1–2 Hz)
///     × volume(floor 0.02 + quadratic, max 0.15)
///     → pan(voice.stereo_pan)
///     → reverb_stereo(room=0.6, time=3.0, damp=0.4)
///     × external_volume
pub fn drone_graph(
  voice: &Voice,
  register: DroneRegister,
  metric: &Shared,
) -> Box<dyn AudioUnit> {
  drone_graph_with_volume(voice, register, metric, None)
}

/// Like `drone_graph` but with an optional external volume
/// multiplier (used for daemon mute control).
pub fn drone_graph_with_volume(
  voice: &Voice,
  register: DroneRegister,
  metric: &Shared,
  external_volume: Option<&Shared>,
) -> Box<dyn AudioUnit> {
  let base = voice.base_freq as f32 * register.multiplier();
  let metric_cutoff = metric.clone();
  let metric_bong = metric.clone();
  let metric_vol = metric.clone();

  // Normalize waveform blend so the peak stays at 1.0 before
  // amplitude scaling.
  let total_ratio = voice.sine_ratio + voice.tri_ratio + voice.saw_ratio;
  let norm = if total_ratio > 0.0 {
    1.0 / total_ratio
  } else {
    1.0
  } as f32;

  let sine_w = voice.sine_ratio as f32 * norm;
  let tri_w = voice.tri_ratio as f32 * norm;
  let saw_w = voice.saw_ratio as f32 * norm;

  // Voice-blend oscillator: weighted sum of sine, triangle, and
  // sawtooth at the base frequency.
  let osc =
    sine_hz(base) * sine_w + triangle_hz(base) * tri_w + saw_hz(base) * saw_w;

  // One-pole lowpass (6 dB/oct, gentle rolloff) with dynamic
  // cutoff.  Capped at 1500 Hz to stay warm even under load.
  let cutoff = lfo(move |_t| {
    let m = metric_cutoff.value();
    200.0 + m * 1300.0
  });

  // Bong envelope: exponential-decay pulse train.  Rate scales
  // from one event per 10 seconds (idle) to twice per second
  // (full load).  Decay is constant so each bong rings out
  // fully; at high rates the tails overlap through the reverb,
  // building density rather than cutting off.
  let am = lfo(move |t| {
    let m = metric_bong.value();
    let rate = 0.1 + m * 1.9;
    let phase = (t * rate) % 1.0;
    (-phase * 5.0).exp()
  });

  // Volume shifts texture, not loudness.  The floor keeps the
  // drone faintly present at idle; the ceiling stays gentle
  // even at full load.
  let vol = lfo(move |_t| {
    let m = metric_vol.value();
    0.30 + m * m * 0.70
  });

  let ext_shared = match external_volume {
    Some(ext) => ext.clone(),
    None => shared(1.0),
  };
  let ext_vol = var(&ext_shared) >> follow(0.5);

  let mono = (osc | cutoff) >> lowpole() * am * vol * ext_vol;
  let stereo =
    mono >> pan(voice.stereo_pan as f32) >> reverb_stereo(0.6, 3.0, 0.4);
  Box::new(stereo)
}

#[cfg(test)]
mod tests {
  use super::*;
  use fundsp::prelude32::shared;

  #[test]
  fn drone_produces_sound_when_metric_nonzero() {
    let metric = shared(0.8);
    let mut graph =
      drone_graph(&Voice::from_hostname("test"), DroneRegister::Low, &metric);
    graph.set_sample_rate(44100.0);
    graph.allocate();

    let peak = (0..44100)
      .map(|_| {
        let (l, r) = graph.get_stereo();
        l.abs().max(r.abs())
      })
      .fold(0.0f32, f32::max);

    assert!(
      peak > 0.001,
      "Drone at metric=0.8 should produce sound, \
       got peak {}",
      peak
    );
  }

  #[test]
  fn drone_quiet_when_metric_zero() {
    let metric = shared(0.0);
    let mut graph =
      drone_graph(&Voice::from_hostname("test"), DroneRegister::Mid, &metric);
    graph.set_sample_rate(44100.0);
    graph.allocate();

    // Skip 200ms to let filters and reverb settle.
    for _ in 0..8820 {
      graph.get_stereo();
    }

    let peak = (0..44100)
      .map(|_| {
        let (l, r) = graph.get_stereo();
        l.abs().max(r.abs())
      })
      .fold(0.0f32, f32::max);

    // Volume floor is 0.08, so the signal should be quiet but
    // not silent.
    assert!(
      peak < 0.3,
      "Drone at metric=0.0 should be very quiet, \
       got peak {}",
      peak
    );
  }

  #[test]
  fn high_metric_louder_than_low() {
    let metric_lo = shared(0.1);
    let metric_hi = shared(0.9);

    let mut lo_graph = drone_graph(
      &Voice::from_hostname("test"),
      DroneRegister::Mid,
      &metric_lo,
    );
    let mut hi_graph = drone_graph(
      &Voice::from_hostname("test"),
      DroneRegister::Mid,
      &metric_hi,
    );

    lo_graph.set_sample_rate(44100.0);
    hi_graph.set_sample_rate(44100.0);
    lo_graph.allocate();
    hi_graph.allocate();

    let rms = |graph: &mut Box<dyn AudioUnit>| -> f32 {
      let sum: f32 = (0..88200)
        .map(|_| {
          let (l, r) = graph.get_stereo();
          l * l + r * r
        })
        .sum();
      (sum / 88200.0).sqrt()
    };

    let rms_lo = rms(&mut lo_graph);
    let rms_hi = rms(&mut hi_graph);

    assert!(
      rms_hi > rms_lo,
      "Higher metric RMS ({}) should exceed lower ({})",
      rms_hi,
      rms_lo
    );
  }
}
