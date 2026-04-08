use crate::severity::Severity;
use crate::voice::{BoopSpec, Voice};
use fundsp::prelude32::*;
use fundsp::shared::Shared;
use std::time::Duration;

/// Beats in one bar (4/4 time).
pub const BEATS_PER_BAR: f64 = 4.0;

/// Candidate note values in beats, longest first.
pub const NOTE_VALUES: [f64; 4] = [4.0, 2.0, 1.0, 0.5];

/// Shortest allowed note value (an eighth note).
pub const MIN_NOTE_VALUE: f64 = 0.5;

/// Maximum lowpass cutoff to avoid filter instability near Nyquist.
const MAX_CUTOFF: f32 = 18000.0;

/// Base gap between consecutive boops.  Actual gaps scale
/// proportionally with the shorter of the two adjacent boops.
const GAP_SECS: f64 = 0.1;

/// Convert a cent offset to a frequency multiplier.
fn cents_to_ratio(cents: f64) -> f64 {
  2.0_f64.powf(cents / 1200.0)
}

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
/// proportional gaps, the release tail, and echo decay.
/// When `echo_mix > 0.0`, four echo bounces (at 0.3 feedback) are
/// appended so the last repeat is audible before the slot ends.
pub fn heartbeat_duration(
  specs: &[BoopSpec],
  attack_secs: f64,
  release_secs: f64,
  echo_delay: f64,
  echo_mix: f64,
) -> Duration {
  if specs.is_empty() {
    return Duration::ZERO;
  }

  let max_dur = specs.iter().map(|s| s.duration).fold(0.0f64, f64::max);

  let attack_total = attack_secs * specs.len() as f64;
  let boop_sum: f64 = specs.iter().map(|s| s.duration).sum();
  let gap_sum: f64 = specs
    .windows(2)
    .map(|w| gap_between(w[0].duration, w[1].duration, max_dur))
    .sum();

  let echo_tail = if echo_mix > 0.0 {
    4.0 * echo_delay
  } else {
    0.0
  };

  Duration::from_secs_f64(
    attack_total + boop_sum + gap_sum + release_secs + echo_tail + 0.05,
  )
}

/// Parameters for a single boop inside the heartbeat closure.
#[derive(Clone)]
struct BoopTiming {
  start: f32,
  /// End including the release tail (start + dur + release).
  tail_end: f32,
  freq: f32,
  amp: f32,
  dur: f32,
  attack: f32,
  release: f32,
  harshness: f32,
  filter_cutoff: f32,
  filter_q: f32,
}

/// Build a complete heartbeat audio graph (no external volume).
pub fn heartbeat_graph(
  voice: &Voice,
  severities: &[Severity],
  specs: &[BoopSpec],
) -> Box<dyn AudioUnit> {
  heartbeat_graph_with_volume(voice, severities, specs, None)
}

/// Build a heartbeat audio graph with optional external volume
/// multiplier (used for preview volume control).  Each boop plays
/// its own pentatonic note with severity-driven timbre: detuning
/// for dissonance, harshness via saw bleed-in, and a resonant
/// lowpass filter.  The chirp onset sweep is preserved at all
/// severity levels.
pub fn heartbeat_graph_with_volume(
  voice: &Voice,
  severities: &[Severity],
  specs: &[BoopSpec],
  external_volume: Option<&Shared>,
) -> Box<dyn AudioUnit> {
  let count = Ord::min(specs.len(), severities.len());
  let chirp_ratio = voice.chirp_ratio as f32;
  let brightness = voice.brightness as f32;
  let resonance = voice.resonance as f32;
  let sub_mix = voice.sub_octave as f32;
  let voice_amplitude = voice.amplitude as f32;
  let drive = (voice.drive as f32).max(0.01);
  let drive_norm = 1.0 / drive.tanh();
  let noise_mix = voice.noise_mix as f32;
  let crush_param = voice.crush as f32;
  let crush_levels = 2.0_f32.powf(1.0 + 15.0 * (1.0 - crush_param));
  let downsample = voice.downsample as f32;
  let ds_rate = 100_000.0_f32 / 2.0_f32.powf(downsample * 8.0);
  let vibrato_rate = voice.vibrato_rate;
  let vibrato_depth = voice.vibrato_depth;
  let tremolo_rate = voice.tremolo_rate;
  let tremolo_depth = voice.tremolo_depth;
  let fm_ratio = voice.fm_ratio as f32;
  let fm_depth = voice.fm_depth as f32;

  let total_ratio =
    voice.sine_ratio + voice.tri_ratio + voice.saw_ratio + voice.square_ratio;
  let norm = if total_ratio > 0.0 {
    1.0 / total_ratio
  } else {
    1.0
  } as f32;

  let sine_w = voice.sine_ratio as f32 * norm;
  let tri_w = voice.tri_ratio as f32 * norm;
  let saw_w = voice.saw_ratio as f32 * norm;
  let square_w = voice.square_ratio as f32 * norm;

  let max_dur = specs[..count]
    .iter()
    .map(|s| s.duration)
    .fold(0.0f64, f64::max);

  // Pre-compute start times and per-boop parameters.
  let mut timings = Vec::with_capacity(count);
  let mut t = 0.0f64;
  for i in 0..count {
    let profile = severities[i].profile();
    let detune_sign = if i % 2 == 0 { 1.0 } else { -1.0 };
    let freq = (specs[i].freq
      * cents_to_ratio(profile.detune_cents * detune_sign))
      as f32;
    let amp = voice_amplitude * profile.amplitude as f32;
    let dur = specs[i].duration as f32;
    let attack = voice.attack_ms as f32 / 1000.0;
    let release = voice.release_ms as f32 / 1000.0;
    let start = t as f32;

    timings.push(BoopTiming {
      start,
      tail_end: start + attack + dur + release,
      freq,
      amp,
      dur,
      attack,
      release,
      harshness: profile.harshness as f32,
      filter_cutoff: (freq * profile.filter_cutoff as f32 * brightness)
        .min(MAX_CUTOFF),
      filter_q: profile.filter_q as f32 * resonance,
    });

    t += (voice.attack_ms / 1000.0) + specs[i].duration;
    if i + 1 < count {
      t += gap_between(specs[i].duration, specs[i + 1].duration, max_dur);
    }
  }

  // Frequency LFO: switches between boop frequencies with chirp
  // onset sweep, holds frequency through the release tail,
  // outputs near-zero between boops.  Reverse iteration so the
  // latest note's attack wins over a dying note's release tail.
  let freq_timings = timings.clone();
  let freq_env = lfo(move |t: f32| {
    for p in freq_timings.iter().rev() {
      if t >= p.start && t < p.tail_end {
        let body_t = (t - p.start - p.attack).max(0.0);
        let chirp_t = (body_t / 0.04).min(1.0);
        let base =
          p.freq * chirp_ratio + (p.freq - p.freq * chirp_ratio) * chirp_t;
        let vib = 2f64.powf(
          vibrato_depth
            * (std::f64::consts::TAU * vibrato_rate * t as f64).sin()
            / 12.0,
        ) as f32;
        let fm_freq = base * fm_ratio;
        let fm_mod =
          fm_depth * fm_freq * (std::f32::consts::TAU * fm_freq * t).sin();
        return (base * vib + fm_mod).max(0.01);
      }
    }
    0.01
  });

  // Amplitude envelope: attack fills the slot body, release
  // tail bleeds past the slot boundary so short notes decay
  // naturally rather than being truncated.  Reverse iteration
  // so a fresh note's attack crushes the previous release.
  let amp_timings = timings.clone();
  let amp_env = envelope(move |t: f32| {
    for p in amp_timings.iter().rev() {
      if t >= p.start && t < p.tail_end {
        let local_t = t - p.start;
        let fade_in = if p.attack > 0.0 {
          (local_t / p.attack).min(1.0)
        } else {
          1.0
        };
        let body_end = p.attack + p.dur;
        let fade_out = if local_t > body_end && p.release > 0.0 {
          ((body_end + p.release - local_t) / p.release).max(0.0)
        } else {
          1.0
        };
        let trem = (1.0
          - tremolo_depth
            * (1.0 - (std::f64::consts::TAU * tremolo_rate * t as f64).sin())
            / 2.0) as f32;
        return fade_in * fade_out * p.amp * trem;
      }
    }
    0.0
  });

  // Sine weight envelope: reduces with harshness so the tone
  // loses its pure-voice warmth as severity increases.
  let sine_timings = timings.clone();
  let sine_w_env = envelope(move |t: f32| {
    for p in sine_timings.iter().rev() {
      if t >= p.start && t < p.tail_end {
        return sine_w * (1.0 - p.harshness);
      }
    }
    sine_w
  });

  // Saw weight envelope: increases with harshness, adding buzzy
  // harmonics that make degraded/down boops sound shrill.
  let saw_timings = timings.clone();
  let saw_w_env = envelope(move |t: f32| {
    for p in saw_timings.iter().rev() {
      if t >= p.start && t < p.tail_end {
        return saw_w + p.harshness;
      }
    }
    saw_w
  });

  // Lowpass cutoff: healthy is open/bright, down is narrow/nasal.
  let cutoff_timings = timings.clone();
  let cutoff_env = lfo(move |t: f32| {
    for p in cutoff_timings.iter().rev() {
      if t >= p.start && t < p.tail_end {
        return p.filter_cutoff;
      }
    }
    20000.0
  });

  // Sub-octave oscillator: sine at half the boop frequency,
  // mixed in before the lowpass to add low-end body.
  let sub_freq_timings = timings.clone();
  let sub_freq_env = lfo(move |t: f32| {
    for p in sub_freq_timings.iter().rev() {
      if t >= p.start && t < p.tail_end {
        let body_t = (t - p.start - p.attack).max(0.0);
        let chirp_t = (body_t / 0.04).min(1.0);
        let half = p.freq * 0.5;
        let base = half * chirp_ratio + (half - half * chirp_ratio) * chirp_t;
        let vib = 2f64.powf(
          vibrato_depth
            * (std::f64::consts::TAU * vibrato_rate * t as f64).sin()
            / 12.0,
        ) as f32;
        let fm_freq = base * fm_ratio;
        let fm_mod =
          fm_depth * fm_freq * (std::f32::consts::TAU * fm_freq * t).sin();
        return (base * vib + fm_mod).max(0.01);
      }
    }
    0.01
  });
  let sub_waveform = (sine() * sine_w)
    & (triangle() * tri_w)
    & (saw() * saw_w)
    & (square() * square_w);
  let sub_osc = sub_freq_env >> sub_waveform;

  // Lowpass Q: higher resonance at worse severity creates a
  // honky, nasal peak — shrill without just being high-pitched.
  let q_timings = timings;
  let q_env = lfo(move |t: f32| {
    for p in q_timings.iter().rev() {
      if t >= p.start && t < p.tail_end {
        return p.filter_q;
      }
    }
    0.5
  });

  let ext = match external_volume {
    Some(s) => s.clone(),
    None => shared(1.0),
  };
  let ext_vol = var(&ext) >> follow(0.1);

  let echo_delay = voice.echo_delay as f32;
  let echo_mix = voice.echo_mix as f32;

  let waveform = (sine() * sine_w_env)
    & (triangle() * tri_w)
    & (saw() * saw_w_env)
    & (square() * square_w);
  let signal = ((freq_env >> waveform) + (sub_osc * sub_mix))
    >> (shape(Tanh(drive)) * drive_norm)
    >> (pass() + (pink() * noise_mix));
  let moog_q = q_env * 0.2;
  let mono = (signal | cutoff_env | moog_q)
    >> (moog() * amp_env * ext_vol)
    >> shape(Crush(crush_levels))
    >> hold_hz(ds_rate, 0.0);
  let with_echo =
    mono >> (pass() & (feedback(delay(echo_delay) * 0.3) * echo_mix));
  let stereo = with_echo
    >> pan(voice.stereo_pan as f32)
    >> reverb_stereo(0.3, 0.8, voice.reverb_mix as f32);
  Box::new(stereo)
}

/// Build an audio graph for a single boop at the given severity
/// and duration.  Applies the same timbre model as the heartbeat
/// graph: harshness crossfade and resonant lowpass filter.
pub fn boop_graph(
  voice: &Voice,
  severity: Severity,
  duration_secs: f64,
) -> Box<dyn AudioUnit> {
  let profile = severity.profile();
  let freq = (voice.base_freq * cents_to_ratio(profile.detune_cents)) as f32;
  let amp = voice.amplitude as f32 * profile.amplitude as f32;
  let harshness = profile.harshness as f32;
  let attack = (voice.attack_ms / 1000.0) as f32;
  let release = (voice.release_ms / 1000.0).min(duration_secs * 0.5) as f32;
  let dur = duration_secs as f32;

  let total_ratio =
    voice.sine_ratio + voice.tri_ratio + voice.saw_ratio + voice.square_ratio;
  let norm = if total_ratio > 0.0 {
    1.0 / total_ratio
  } else {
    1.0
  } as f32;

  let sine_w = voice.sine_ratio as f32 * norm * (1.0 - harshness);
  let tri_w = voice.tri_ratio as f32 * norm;
  let saw_w = voice.saw_ratio as f32 * norm + harshness;
  let square_w = voice.square_ratio as f32 * norm;

  let drive = (voice.drive as f32).max(0.01);
  let drive_norm = 1.0 / drive.tanh();
  let noise_mix = voice.noise_mix as f32;
  let crush_param = voice.crush as f32;
  let crush_levels = 2.0_f32.powf(1.0 + 15.0 * (1.0 - crush_param));
  let fm_ratio = voice.fm_ratio as f32;
  let fm_depth = voice.fm_depth as f32;
  let downsample = voice.downsample as f32;
  let ds_rate = 100_000.0_f32 / 2.0_f32.powf(downsample * 8.0);

  let fm_freq = freq * fm_ratio;
  let freq_source = lfo(move |t: f32| {
    let fm_mod =
      fm_depth * fm_freq * (std::f32::consts::TAU * fm_freq * t).sin();
    (freq + fm_mod).max(0.01)
  });
  let waveform = (sine() * sine_w)
    & (triangle() * tri_w)
    & (saw() * saw_w)
    & (square() * square_w);
  let main_osc = freq_source >> waveform;

  let sub_half = freq * 0.5;
  let sub_fm_freq = sub_half * fm_ratio;
  let sub_freq_source = lfo(move |t: f32| {
    let fm_mod =
      fm_depth * sub_fm_freq * (std::f32::consts::TAU * sub_fm_freq * t).sin();
    (sub_half + fm_mod).max(0.01)
  });
  let sub_waveform = (sine() * sine_w)
    & (triangle() * tri_w)
    & (saw() * saw_w)
    & (square() * square_w);
  let sub_osc = (sub_freq_source >> sub_waveform) * voice.sub_octave as f32;
  let combined = main_osc + sub_osc;

  let cutoff = dc(
    (freq * profile.filter_cutoff as f32 * voice.brightness as f32)
      .min(MAX_CUTOFF),
  );
  let q_val =
    dc((profile.filter_q as f32 * voice.resonance as f32 * 0.2).min(0.95));

  let env = envelope(move |t: f32| {
    let fade_in = if attack > 0.0 {
      (t / attack).min(1.0)
    } else {
      1.0
    };
    let body_end = attack + dur;
    let fade_out = if t > body_end && release > 0.0 {
      ((body_end + release - t) / release).max(0.0)
    } else {
      1.0
    };
    fade_in * fade_out * amp
  });

  let echo_delay = voice.echo_delay as f32;
  let echo_mix = voice.echo_mix as f32;

  let driven = combined
    >> (shape(Tanh(drive)) * drive_norm)
    >> (pass() + (pink() * noise_mix));
  let mono = (driven | cutoff | q_val)
    >> (moog() * env)
    >> shape(Crush(crush_levels))
    >> hold_hz(ds_rate, 0.0);
  Box::new(mono >> (pass() & (feedback(delay(echo_delay) * 0.3) * echo_mix)))
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
  fn severity_profiles_produce_equal_amplitude() {
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
      (down_peak - healthy_peak).abs() < 0.01,
      "Flattened profiles should produce similar amplitude: \
       down={}, healthy={}",
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
    let dur = heartbeat_duration(&specs, 0.0, 0.15, 0.0, 0.0);
    // 3 × 0.4 = 1.2 boop time, plus gaps, release, and 0.05 tail.
    assert!(
      dur.as_secs_f64() > 1.2,
      "Duration should exceed boop sum, got {:.3}",
      dur.as_secs_f64()
    );
  }

  #[test]
  fn heartbeat_duration_empty() {
    assert_eq!(heartbeat_duration(&[], 0.0, 0.15, 0.0, 0.0), Duration::ZERO);
  }

  #[test]
  fn heartbeat_duration_single_boop() {
    let specs = vec![BoopSpec {
      freq: 440.0,
      duration: 1.2,
    }];
    let dur = heartbeat_duration(&specs, 0.0, 0.15, 0.0, 0.0);
    // Single boop: 1.2 + 0.15 release + 0.05 tail, no gaps.
    assert!(
      (dur.as_secs_f64() - 1.4).abs() < 1e-10,
      "Single boop should be duration + release + tail, got {:.3}",
      dur.as_secs_f64()
    );
  }

  #[test]
  fn heartbeat_duration_includes_echo_tail() {
    let specs = vec![BoopSpec {
      freq: 440.0,
      duration: 1.0,
    }];
    let without_echo = heartbeat_duration(&specs, 0.0, 0.15, 0.3, 0.0);
    let with_echo = heartbeat_duration(&specs, 0.0, 0.15, 0.3, 0.5);
    // Echo adds 4 × echo_delay = 1.2 s.
    assert!(
      (with_echo.as_secs_f64() - without_echo.as_secs_f64() - 1.2).abs()
        < 1e-10,
      "Echo tail should add 4 × delay, got delta {:.3}",
      with_echo.as_secs_f64() - without_echo.as_secs_f64()
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

    let attack_secs = voice.attack_ms / 1000.0;
    let release_secs = voice.release_ms / 1000.0;
    let samples =
      (heartbeat_duration(&specs, attack_secs, release_secs, 0.0, 0.0)
        .as_secs_f32()
        * 44100.0) as usize;
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

    let attack_secs = voice.attack_ms / 1000.0;
    let release_secs = voice.release_ms / 1000.0;
    let samples =
      (heartbeat_duration(&specs, attack_secs, release_secs, 0.0, 0.0)
        .as_secs_f32()
        * 44100.0) as usize;
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

  #[test]
  fn cents_to_ratio_identity() {
    assert!((cents_to_ratio(0.0) - 1.0).abs() < 1e-10);
  }

  #[test]
  fn cents_to_ratio_octave() {
    assert!((cents_to_ratio(1200.0) - 2.0).abs() < 1e-10);
  }

  #[test]
  fn cents_to_ratio_negative() {
    assert!((cents_to_ratio(-1200.0) - 0.5).abs() < 1e-10);
  }
}
