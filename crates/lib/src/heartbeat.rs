use crate::severity::Severity;
use crate::voice::{BoopSpec, Voice};
use fundsp::prelude32::*;
use std::time::Duration;

/// Total time budget for boops (excluding gaps).
pub const TOTAL_BOOP_TIME: f64 = 1.2;

/// Base gap between consecutive boops.  Actual gaps scale
/// proportionally with the shorter of the two adjacent boops.
const GAP_SECS: f64 = 0.1;

/// Compute the gap between two adjacent boops.  Proportional to
/// the shorter duration so that quick boops cluster tightly while
/// long boops get breathing room.
fn gap_between(dur_a: f64, dur_b: f64, max_dur: f64) -> f64 {
  if max_dur <= 0.0 {
    return GAP_SECS;
  }
  GAP_SECS * (dur_a.min(dur_b) / max_dur)
}

/// Total wall-clock duration of a heartbeat sequence including
/// proportional gaps and a small release tail.
pub fn heartbeat_duration(specs: &[BoopSpec]) -> Duration {
  if specs.is_empty() {
    return Duration::ZERO;
  }

  let max_dur = specs.iter().map(|s| s.duration).fold(0.0f64, f64::max);

  let boop_sum: f64 = specs.iter().map(|s| s.duration).sum();
  let gap_sum: f64 = specs
    .windows(2)
    .map(|w| gap_between(w[0].duration, w[1].duration, max_dur))
    .sum();

  Duration::from_secs_f64(boop_sum + gap_sum + 0.05)
}

/// Parameters for a single boop inside the heartbeat closure.
#[derive(Clone)]
struct BoopTiming {
  start: f32,
  end: f32,
  freq: f32,
  amp: f32,
  dur: f32,
  attack: f32,
  release: f32,
}

/// Build a complete heartbeat audio graph that renders N boops in
/// a single stream.  Each boop plays its own pentatonic note at
/// the severity-adjusted pitch, with a chirp onset sweep, and the
/// output is panned center with a short reverb tail.
pub fn heartbeat_graph(
  voice: &Voice,
  severities: &[Severity],
  specs: &[BoopSpec],
) -> Box<dyn AudioUnit> {
  let count = Ord::min(specs.len(), severities.len());
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

  let max_dur = specs[..count]
    .iter()
    .map(|s| s.duration)
    .fold(0.0f64, f64::max);

  // Pre-compute start times and per-boop parameters.
  let mut timings = Vec::with_capacity(count);
  let mut t = 0.0f64;
  for i in 0..count {
    let freq = (specs[i].freq * severities[i].pitch_ratio()) as f32;
    let amp = severities[i].amplitude() as f32;
    let dur = specs[i].duration as f32;
    let attack = (voice.attack_ms as f32 / 1000.0).min(dur * 0.3);
    let release = (voice.release_ms as f32 / 1000.0).min(dur * 0.5);
    let start = t as f32;

    timings.push(BoopTiming {
      start,
      end: start + dur,
      freq,
      amp,
      dur,
      attack,
      release,
    });

    t += specs[i].duration;
    if i + 1 < count {
      t += gap_between(specs[i].duration, specs[i + 1].duration, max_dur);
    }
  }

  // Frequency LFO: switches between boop frequencies with chirp
  // onset sweep, outputs near-zero between boops.
  let freq_timings = timings.clone();
  let freq_env = lfo(move |t: f32| {
    for p in &freq_timings {
      if t >= p.start && t < p.end {
        let local_t = t - p.start;
        let chirp_t = (local_t / 0.04).min(1.0);
        return p.freq * chirp_ratio
          + (p.freq - p.freq * chirp_ratio) * chirp_t;
      }
    }
    0.01
  });

  // Amplitude envelope: attack/release per boop, silence between.
  let amp_timings = timings;
  let amp_env = envelope(move |t: f32| {
    for p in &amp_timings {
      if t >= p.start && t < p.end {
        let local_t = t - p.start;
        let fade_in = (local_t / p.attack).min(1.0);
        let sustain_end = p.dur - p.release;
        let fade_out = if local_t > sustain_end && p.release > 0.0 {
          ((p.dur - local_t) / p.release).max(0.0)
        } else {
          1.0
        };
        return fade_in * fade_out * p.amp;
      }
    }
    0.0
  });

  let waveform = (sine() * sine_w) & (triangle() * tri_w) & (saw() * saw_w);
  let mix = (freq_env >> waveform) * amp_env;
  let stereo = mix >> pan(0.0) >> reverb_stereo(0.3, 0.8, 0.6);
  Box::new(stereo)
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
  fn heartbeat_duration_sums_correctly() {
    let specs = vec![
      BoopSpec {
        freq: 440.0,
        duration: 0.4,
      },
      BoopSpec {
        freq: 550.0,
        duration: 0.4,
      },
      BoopSpec {
        freq: 660.0,
        duration: 0.4,
      },
    ];
    let dur = heartbeat_duration(&specs);
    // 3 × 0.4 = 1.2 boop time, plus gaps and 0.05 tail.
    assert!(
      dur.as_secs_f64() > 1.2,
      "Duration should exceed boop sum, got {:.3}",
      dur.as_secs_f64()
    );
  }

  #[test]
  fn heartbeat_duration_empty() {
    assert_eq!(heartbeat_duration(&[]), Duration::ZERO);
  }

  #[test]
  fn heartbeat_duration_single_boop() {
    let specs = vec![BoopSpec {
      freq: 440.0,
      duration: 1.2,
    }];
    let dur = heartbeat_duration(&specs);
    // Single boop: 1.2 + 0.05 tail, no gaps.
    assert!(
      (dur.as_secs_f64() - 1.25).abs() < 1e-10,
      "Single boop should be duration + tail, got {:.3}",
      dur.as_secs_f64()
    );
  }

  #[test]
  fn heartbeat_graph_produces_sound() {
    let voice = Voice::from_hostname("test");
    let specs = vec![
      BoopSpec {
        freq: 440.0,
        duration: 0.4,
      },
      BoopSpec {
        freq: 550.0,
        duration: 0.4,
      },
      BoopSpec {
        freq: 660.0,
        duration: 0.4,
      },
    ];
    let severities = [Severity::Healthy, Severity::Degraded, Severity::Down];
    let mut graph = heartbeat_graph(&voice, &severities, &specs);
    graph.set_sample_rate(44100.0);
    graph.allocate();

    let samples = (heartbeat_duration(&specs).as_secs_f32() * 44100.0) as usize;
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

  #[test]
  fn heartbeat_graph_five_boops() {
    let voice = Voice::from_hostname("test");
    let specs = vec![
      BoopSpec {
        freq: 220.0,
        duration: 0.24,
      },
      BoopSpec {
        freq: 330.0,
        duration: 0.24,
      },
      BoopSpec {
        freq: 440.0,
        duration: 0.24,
      },
      BoopSpec {
        freq: 550.0,
        duration: 0.24,
      },
      BoopSpec {
        freq: 660.0,
        duration: 0.24,
      },
    ];
    let severities = [
      Severity::Healthy,
      Severity::Degraded,
      Severity::Down,
      Severity::Healthy,
      Severity::Degraded,
    ];
    let mut graph = heartbeat_graph(&voice, &severities, &specs);
    graph.set_sample_rate(44100.0);
    graph.allocate();

    let samples = (heartbeat_duration(&specs).as_secs_f32() * 44100.0) as usize;
    let peak = (0..samples)
      .map(|_| {
        let (l, r) = graph.get_stereo();
        l.abs().max(r.abs())
      })
      .fold(0.0f32, f32::max);

    assert!(
      peak > 0.001,
      "Five-boop heartbeat should produce audible samples, \
       got peak {}",
      peak
    );
  }
}
