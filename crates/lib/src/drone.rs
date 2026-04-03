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
/// The graph structure (from the design doc):
///   saw_hz(freq) >> lowpass(cutoff) * lfo(AM) * metric
///
/// Filter cutoff:  metric 0.0..1.0 → 200..8000 Hz
/// AM rate:        metric 0.0..1.0 → 1..60 Hz
/// Volume:         directly proportional to metric value
pub fn drone_graph(
  voice: &Voice,
  register: DroneRegister,
  metric: &Shared,
) -> Box<dyn AudioUnit> {
  let base = voice.base_freq as f32 * register.multiplier();
  let metric_val = metric.clone();
  let metric_am = metric.clone();
  let metric_vol = metric.clone();

  // Sawtooth oscillator as the harmonically rich source.
  let osc = saw_hz(base);

  // Low-pass filter whose cutoff tracks the metric.
  // 0.0 → 200 Hz (warm hum), 1.0 → 8000 Hz (bright buzz).
  let cutoff = lfo(move |_t| {
    let m = metric_val.value();
    200.0 + m * 7800.0
  });

  // Amplitude modulation (tremolo/roughness).
  // 0.0 → 1 Hz gentle pulse, 1.0 → 60 Hz growl.
  let am = lfo(move |t| {
    let m = metric_am.value();
    let rate = 1.0 + m * 59.0;
    0.5 + 0.5 * (t * rate * std::f32::consts::TAU).sin()
  });

  // Volume envelope: metric value smoothed to avoid clicks.
  let vol = var(&metric_vol) >> follow(0.5);

  // Assemble: osc → filter → AM → volume.
  // `cutoff >> resonator(base, bw)` is an option, but a
  // simple lowpass_hz is closer to the design doc.
  //
  // Note: fundsp's `lowpass_hz` is a fixed-cutoff filter.
  // For dynamic cutoff driven by a metric, we pipe the
  // cutoff LFO into `lowpass()` which takes cutoff + Q
  // inputs.  We supply a moderate Q of 0.7.
  let filter_input = (osc | cutoff | dc(0.7)) >> lowpass();

  Box::new(filter_input * am * vol)
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
      .map(|_| graph.get_stereo().0.abs())
      .fold(0.0f32, f32::max);

    assert!(
      peak > 0.01,
      "Drone at metric=0.8 should produce sound, \
       got peak {}",
      peak
    );
  }

  #[test]
  fn drone_silent_when_metric_zero() {
    let metric = shared(0.0);
    let mut graph =
      drone_graph(&Voice::from_hostname("test"), DroneRegister::Mid, &metric);
    graph.set_sample_rate(44100.0);
    graph.allocate();

    // Skip first 4410 samples (100ms) to let the follow
    // filter settle.
    for _ in 0..4410 {
      graph.get_stereo();
    }

    let peak = (0..44100)
      .map(|_| graph.get_stereo().0.abs())
      .fold(0.0f32, f32::max);

    assert!(
      peak < 0.01,
      "Drone at metric=0.0 should be near-silent, \
       got peak {}",
      peak
    );
  }
}
