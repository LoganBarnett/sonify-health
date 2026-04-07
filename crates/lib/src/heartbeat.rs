use crate::severity::Severity;
use crate::voice::Voice;
use fundsp::prelude32::*;
use std::time::Duration;

/// Total time budget for the three boops (excluding gaps).
const TOTAL_BOOP_TIME: f64 = 1.2;

/// Silence between consecutive boops.
const GAP_SECS: f64 = 0.1;

/// Duration of each of the three boops based on the voice's
/// rhythmic proportions.
pub fn boop_durations(voice: &Voice) -> [f64; 3] {
  let b1 = voice.boop1_ratio * TOTAL_BOOP_TIME;
  let b2 = voice.boop2_ratio * TOTAL_BOOP_TIME;
  let b3 = (1.0 - voice.boop1_ratio - voice.boop2_ratio) * TOTAL_BOOP_TIME;
  [b1, b2, b3]
}

/// Total wall-clock duration of a full heartbeat sequence
/// including gaps and a small release tail.
pub fn heartbeat_duration(voice: &Voice) -> Duration {
  let [b1, b2, b3] = boop_durations(voice);
  let total = b1 + GAP_SECS + b2 + GAP_SECS + b3 + 0.05;
  Duration::from_secs_f64(total)
}

/// Build an audio graph for a single boop at the given severity
/// and duration.  The graph includes an attack/release envelope.
pub fn boop_graph(
  voice: &Voice,
  severity: Severity,
  duration_secs: f64,
) -> Box<dyn AudioUnit> {
  let freq = (voice.base_freq * severity.pitch_ratio()) as f32;
  let amp = severity.amplitude() as f32;
  let attack = (voice.attack_ms / 1000.0).min(duration_secs * 0.3) as f32;
  let release = (voice.release_ms / 1000.0).min(duration_secs * 0.5) as f32;
  let dur = duration_secs as f32;

  // Normalize waveform blend so the peak doesn't exceed 1.0
  // before amplitude scaling.
  let total_ratio = voice.sine_ratio + voice.tri_ratio + voice.saw_ratio;
  let norm = if total_ratio > 0.0 {
    1.0 / total_ratio
  } else {
    1.0
  } as f32;

  let sine_w = voice.sine_ratio as f32 * norm;
  let tri_w = voice.tri_ratio as f32 * norm;
  let saw_w = voice.saw_ratio as f32 * norm;

  let waveform =
    sine_hz(freq) * sine_w + triangle_hz(freq) * tri_w + saw_hz(freq) * saw_w;

  let env = envelope(move |t: f32| {
    let fade_in = (t / attack).min(1.0);
    let sustain_end = dur - release;
    let fade_out = if t > sustain_end && release > 0.0 {
      ((dur - t) / release).max(0.0)
    } else {
      1.0
    };
    fade_in * fade_out * amp
  });

  Box::new(waveform * env)
}

/// Build a complete heartbeat audio graph that renders all three
/// boops in a single stream.  Each boop includes a chirp onset
/// sweep and the output is panned center with a short reverb tail.
///
/// This eliminates the three separate audio streams that the old
/// per-boop approach required.
pub fn heartbeat_graph(
  voice: &Voice,
  severities: [Severity; 3],
) -> Box<dyn AudioUnit> {
  let durations = boop_durations(voice);
  let chirp_ratio = voice.chirp_ratio as f32;

  let total_ratio = voice.sine_ratio + voice.tri_ratio + voice.saw_ratio;
  let norm = if total_ratio > 0.0 {
    1.0 / total_ratio
  } else {
    1.0
  } as f32;

  let sine_w = voice.sine_ratio as f32 * norm;
  let tri_w = voice.tri_ratio as f32 * norm;
  let saw_w = voice.saw_ratio as f32 * norm;

  let starts: [f32; 3] = [
    0.0,
    (durations[0] + GAP_SECS) as f32,
    (durations[0] + GAP_SECS + durations[1] + GAP_SECS) as f32,
  ];

  let build_boop = |i: usize| {
    let freq = (voice.base_freq * severities[i].pitch_ratio()) as f32;
    let amp = severities[i].amplitude() as f32;
    let dur = durations[i] as f32;
    let start = starts[i];
    let end = start + dur;
    let attack = (voice.attack_ms as f32 / 1000.0).min(dur * 0.3);
    let release = (voice.release_ms as f32 / 1000.0).min(dur * 0.5);

    // Chirp: sweep from freq × chirp_ratio down to freq over
    // 40 ms at boop onset.
    let freq_env = lfo(move |t: f32| {
      if t < start || t >= end {
        return 0.01;
      }
      let local_t = t - start;
      let chirp_t = (local_t / 0.04).min(1.0);
      freq * chirp_ratio + (freq - freq * chirp_ratio) * chirp_t
    });

    let amp_env = envelope(move |t: f32| {
      if t < start || t >= end {
        return 0.0;
      }
      let local_t = t - start;
      let fade_in = (local_t / attack).min(1.0);
      let sustain_end = dur - release;
      let fade_out = if local_t > sustain_end && release > 0.0 {
        ((dur - local_t) / release).max(0.0)
      } else {
        1.0
      };
      fade_in * fade_out * amp
    });

    let waveform = sine() * sine_w & triangle() * tri_w & saw() * saw_w;
    (freq_env >> waveform) * amp_env
  };

  let mix = build_boop(0) + build_boop(1) + build_boop(2);
  let stereo = mix >> pan(0.0) >> reverb_stereo(0.3, 0.8, 0.6);
  Box::new(stereo)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn boop_produces_sound() {
    let voice = Voice::from_hostname("test");
    let mut graph = boop_graph(&voice, Severity::Healthy, 0.5);
    graph.set_sample_rate(44100.0);
    graph.allocate();

    let mut peak: f32 = 0.0;
    for _ in 0..22050 {
      let (l, _) = graph.get_stereo();
      peak = peak.max(l.abs());
    }
    assert!(
      peak > 0.01,
      "Boop should produce audible samples, got peak {}",
      peak
    );
  }

  #[test]
  fn severity_down_louder_than_healthy() {
    let voice = Voice::from_hostname("test");

    let mut healthy = boop_graph(&voice, Severity::Healthy, 0.5);
    healthy.set_sample_rate(44100.0);
    healthy.allocate();

    let mut down = boop_graph(&voice, Severity::Down, 0.5);
    down.set_sample_rate(44100.0);
    down.allocate();

    let healthy_peak = (0..22050)
      .map(|_| healthy.get_stereo().0.abs())
      .fold(0.0f32, f32::max);

    let down_peak = (0..22050)
      .map(|_| down.get_stereo().0.abs())
      .fold(0.0f32, f32::max);

    assert!(
      down_peak > healthy_peak,
      "Down peak ({}) should exceed healthy peak ({})",
      down_peak,
      healthy_peak
    );
  }

  #[test]
  fn boop_durations_sum_correctly() {
    let voice = Voice::from_hostname("test");
    let [b1, b2, b3] = boop_durations(&voice);
    let sum = b1 + b2 + b3;
    assert!(
      (sum - TOTAL_BOOP_TIME).abs() < 1e-10,
      "Boop durations should sum to {}, got {}",
      TOTAL_BOOP_TIME,
      sum
    );
  }

  #[test]
  fn heartbeat_graph_produces_sound() {
    let voice = Voice::from_hostname("test");
    let severities = [Severity::Healthy, Severity::Degraded, Severity::Down];
    let mut graph = heartbeat_graph(&voice, severities);
    graph.set_sample_rate(44100.0);
    graph.allocate();

    let samples = (heartbeat_duration(&voice).as_secs_f32() * 44100.0) as usize;
    let peak = (0..samples)
      .map(|_| {
        let (l, r) = graph.get_stereo();
        l.abs().max(r.abs())
      })
      .fold(0.0f32, f32::max);

    assert!(
      peak > 0.001,
      "Heartbeat graph should produce audible samples, \
       got peak {}",
      peak
    );
  }
}
