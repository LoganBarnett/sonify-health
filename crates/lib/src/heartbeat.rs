use crate::patch::Patch;
use fundsp::prelude32::*;
use fundsp::shared::Shared;
use std::time::Duration;

/// Maximum lowpass cutoff to avoid filter instability near Nyquist.
const MAX_CUTOFF: f32 = 18000.0;

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
/// proportional gaps, the release tail, and echo decay.
/// When `echo_mix > 0.0`, four echo bounces (at 0.3 feedback) are
/// appended so the last repeat is audible before the slot ends.
///
/// Shared timing params (attack, release, echo) are read from
/// `patches[0]`; per-note duration from each patch.
pub fn heartbeat_duration(patches: &[Patch]) -> Duration {
  if patches.is_empty() {
    return Duration::ZERO;
  }

  let shared = &patches[0];
  let attack_secs = shared.attack_ms / 1000.0;
  let release_secs = shared.release_ms / 1000.0;

  let max_dur = patches.iter().map(|p| p.duration).fold(0.0f64, f64::max);

  let attack_total = attack_secs * patches.len() as f64;
  let boop_sum: f64 = patches.iter().map(|p| p.duration).sum();
  let gap_sum: f64 = patches
    .windows(2)
    .map(|w| gap_between(w[0].duration, w[1].duration, max_dur))
    .sum();

  let echo_tail = if shared.echo_mix > 0.0 {
    4.0 * shared.echo_delay
  } else {
    0.0
  };

  Duration::from_secs_f64(
    attack_total + boop_sum + gap_sum + release_secs + echo_tail + 0.05,
  )
}

/// Content-only duration of a heartbeat phrase: attack ramps, boop
/// bodies, and inter-boop gaps.  Excludes the release tail, echo
/// decay, and safety margin.  Used for gap=0 drone looping so
/// `replace()` fires while the last note is still sustaining,
/// letting the crossfade overlap sound with sound.
pub fn heartbeat_content_duration(patches: &[Patch]) -> Duration {
  if patches.is_empty() {
    return Duration::ZERO;
  }

  let attack_secs = patches[0].attack_ms / 1000.0;
  let max_dur = patches.iter().map(|p| p.duration).fold(0.0f64, f64::max);

  let attack_total = attack_secs * patches.len() as f64;
  let boop_sum: f64 = patches.iter().map(|p| p.duration).sum();
  let gap_sum: f64 = patches
    .windows(2)
    .map(|w| gap_between(w[0].duration, w[1].duration, max_dur))
    .sum();

  Duration::from_secs_f64(attack_total + boop_sum + gap_sum)
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
  sustain: f32,
  harshness: f32,
  filter_cutoff: f32,
  filter_q: f32,
}

/// Build a complete heartbeat audio graph (no external volume).
pub fn heartbeat_graph(patches: &[Patch]) -> Box<dyn AudioUnit> {
  heartbeat_graph_with_volume(patches, None)
}

/// Build a heartbeat audio graph with optional external volume
/// multiplier (used for preview volume control).  Each boop plays
/// its own note with a resonant lowpass filter.
///
/// Shared synthesis params (waveform weights, reverb, pan, echo)
/// are read from `patches[0]`; per-note params (`freq`,
/// `duration`, `attack_ms`, `release_ms`, `sustain`) from each
/// `patches[i]`.
pub fn heartbeat_graph_with_volume(
  patches: &[Patch],
  external_volume: Option<&Shared>,
) -> Box<dyn AudioUnit> {
  let count = patches.len();
  let shared = &patches[0];
  let chirp_ratio = shared.chirp_ratio as f32;
  let brightness = shared.brightness as f32;
  let resonance = shared.resonance as f32;
  let sub_mix = shared.sub_octave as f32;
  let drive = (shared.drive as f32).max(0.01);
  let drive_norm = 1.0 / drive.tanh();
  let noise_mix = shared.noise_mix as f32;
  let crush_param = shared.crush as f32;
  let crush_levels = 2.0_f32.powf(1.0 + 15.0 * (1.0 - crush_param));
  let downsample = shared.downsample as f32;
  let ds_rate = 100_000.0_f32 / 2.0_f32.powf(downsample * 8.0);
  let vibrato_rate = shared.vibrato_rate;
  let vibrato_depth = shared.vibrato_depth;
  let tremolo_rate = shared.tremolo_rate;
  let tremolo_depth = shared.tremolo_depth;
  let fm_ratio = shared.fm_ratio as f32;
  let fm_depth = shared.fm_depth as f32;

  let total_ratio = shared.sine_ratio
    + shared.tri_ratio
    + shared.saw_ratio
    + shared.square_ratio;
  let norm = if total_ratio > 0.0 {
    1.0 / total_ratio
  } else {
    1.0
  } as f32;

  let sine_w = shared.sine_ratio as f32 * norm;
  let tri_w = shared.tri_ratio as f32 * norm;
  let saw_w = shared.saw_ratio as f32 * norm;
  let square_w = shared.square_ratio as f32 * norm;

  let max_dur = patches[..count]
    .iter()
    .map(|p| p.duration)
    .fold(0.0f64, f64::max);

  // Pre-compute start times and per-boop parameters.
  let mut timings = Vec::with_capacity(count);
  let mut t = 0.0f64;
  for i in 0..count {
    let p = &patches[i];
    let freq = p.freq as f32;
    let amp = p.amplitude as f32;
    let dur = p.duration as f32;
    let attack = p.attack_ms as f32 / 1000.0;
    let release = p.release_ms as f32 / 1000.0;
    let sustain_level = p.sustain as f32;
    let start = t as f32;

    timings.push(BoopTiming {
      start,
      tail_end: start + attack + dur + release,
      freq,
      amp,
      dur,
      attack,
      release,
      sustain: sustain_level,
      harshness: 0.0,
      filter_cutoff: (freq * 13.0 * brightness).min(MAX_CUTOFF),
      filter_q: 0.5 * resonance,
    });

    t += (p.attack_ms / 1000.0) + p.duration;
    if i + 1 < count {
      t += gap_between(p.duration, patches[i + 1].duration, max_dur);
    }
  }

  // Frequency LFO: switches between boop frequencies with chirp
  // onset sweep, holds frequency through the release tail,
  // outputs near-zero between boops.
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

  // Amplitude envelope.
  let amp_timings = timings.clone();
  let amp_env = envelope(move |t: f32| {
    for p in amp_timings.iter().rev() {
      if t >= p.start && t < p.tail_end {
        let local_t = t - p.start;
        let level = if p.attack > 0.0 && local_t < p.attack {
          local_t / p.attack
        } else {
          let body_end = p.attack + p.dur;
          if local_t <= body_end {
            p.sustain
          } else if p.release > 0.0 {
            (p.sustain * (body_end + p.release - local_t) / p.release).max(0.0)
          } else {
            0.0
          }
        };
        let trem = (1.0
          - tremolo_depth
            * (1.0 - (std::f64::consts::TAU * tremolo_rate * t as f64).sin())
            / 2.0) as f32;
        return level * p.amp * trem;
      }
    }
    0.0
  });

  // Sine weight envelope.
  let sine_timings = timings.clone();
  let sine_w_env = envelope(move |t: f32| {
    for p in sine_timings.iter().rev() {
      if t >= p.start && t < p.tail_end {
        return sine_w * (1.0 - p.harshness);
      }
    }
    sine_w
  });

  // Saw weight envelope.
  let saw_timings = timings.clone();
  let saw_w_env = envelope(move |t: f32| {
    for p in saw_timings.iter().rev() {
      if t >= p.start && t < p.tail_end {
        return saw_w + p.harshness;
      }
    }
    saw_w
  });

  // Lowpass cutoff envelope.
  let cutoff_timings = timings.clone();
  let cutoff_env = lfo(move |t: f32| {
    for p in cutoff_timings.iter().rev() {
      if t >= p.start && t < p.tail_end {
        return p.filter_cutoff;
      }
    }
    20000.0
  });

  // Sub-octave oscillator.
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

  // Lowpass Q envelope.
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
    None => fundsp::prelude32::shared(1.0),
  };
  let ext_vol = var(&ext) >> follow(0.1);

  let echo_delay = shared.echo_delay as f32;
  let echo_mix = shared.echo_mix as f32;

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
    >> pan(shared.stereo_pan as f32)
    >> reverb_stereo(0.3, 0.8, shared.reverb_mix as f32);
  Box::new(stereo)
}

/// Build an audio graph for a single boop.  Duration and
/// frequency come from the patch itself.
pub fn boop_graph(patch: &Patch) -> Box<dyn AudioUnit> {
  let freq = patch.freq as f32;
  let amp = patch.amplitude as f32;
  let harshness = 0.0_f32;
  let attack = (patch.attack_ms / 1000.0) as f32;
  let release = (patch.release_ms / 1000.0).min(patch.duration * 0.5) as f32;
  let dur = patch.duration as f32;

  let total_ratio =
    patch.sine_ratio + patch.tri_ratio + patch.saw_ratio + patch.square_ratio;
  let norm = if total_ratio > 0.0 {
    1.0 / total_ratio
  } else {
    1.0
  } as f32;

  let sine_w = patch.sine_ratio as f32 * norm * (1.0 - harshness);
  let tri_w = patch.tri_ratio as f32 * norm;
  let saw_w = patch.saw_ratio as f32 * norm + harshness;
  let square_w = patch.square_ratio as f32 * norm;

  let drive = (patch.drive as f32).max(0.01);
  let drive_norm = 1.0 / drive.tanh();
  let noise_mix = patch.noise_mix as f32;
  let crush_param = patch.crush as f32;
  let crush_levels = 2.0_f32.powf(1.0 + 15.0 * (1.0 - crush_param));
  let fm_ratio = patch.fm_ratio as f32;
  let fm_depth = patch.fm_depth as f32;
  let downsample = patch.downsample as f32;
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
  let sub_osc = (sub_freq_source >> sub_waveform) * patch.sub_octave as f32;
  let combined = main_osc + sub_osc;

  let cutoff = dc((freq * 13.0 * patch.brightness as f32).min(MAX_CUTOFF));
  let q_val = dc((0.5 * patch.resonance as f32 * 0.2).min(0.95));

  let sustain_level = patch.sustain as f32;
  let env = envelope(move |t: f32| {
    let level = if attack > 0.0 && t < attack {
      t / attack
    } else {
      let body_end = attack + dur;
      if t <= body_end {
        sustain_level
      } else if release > 0.0 {
        (sustain_level * (body_end + release - t) / release).max(0.0)
      } else {
        0.0
      }
    };
    level * amp
  });

  let echo_delay = patch.echo_delay as f32;
  let echo_mix = patch.echo_mix as f32;

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

  fn test_patch(freq: f64, duration: f64) -> Patch {
    Patch {
      freq,
      duration,
      ..Default::default()
    }
  }

  #[test]
  fn boop_produces_sound() {
    let patch = test_patch(440.0, 0.5);
    let mut graph = boop_graph(&patch);
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
  fn heartbeat_duration_sums_correctly() {
    let base = Patch {
      attack_ms: 0.0,
      release_ms: 150.0,
      echo_mix: 0.0,
      ..Default::default()
    };
    let patches: Vec<Patch> = [440.0, 550.0, 660.0]
      .iter()
      .map(|&f| Patch {
        freq: f,
        duration: 0.4,
        ..base.clone()
      })
      .collect();
    let dur = heartbeat_duration(&patches);
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
    let patch = Patch {
      freq: 440.0,
      duration: 1.2,
      attack_ms: 0.0,
      release_ms: 150.0,
      echo_mix: 0.0,
      ..Default::default()
    };
    let dur = heartbeat_duration(&[patch]);
    assert!(
      (dur.as_secs_f64() - 1.4).abs() < 1e-10,
      "Single boop should be duration + release + tail, got {:.3}",
      dur.as_secs_f64()
    );
  }

  #[test]
  fn heartbeat_duration_includes_echo_tail() {
    let base = Patch {
      freq: 440.0,
      duration: 1.0,
      attack_ms: 0.0,
      release_ms: 150.0,
      echo_delay: 0.3,
      ..Default::default()
    };
    let without_echo = heartbeat_duration(&[Patch {
      echo_mix: 0.0,
      ..base.clone()
    }]);
    let with_echo = heartbeat_duration(&[Patch {
      echo_mix: 0.5,
      ..base
    }]);
    assert!(
      (with_echo.as_secs_f64() - without_echo.as_secs_f64() - 1.2).abs()
        < 1e-10,
      "Echo tail should add 4 x delay, got delta {:.3}",
      with_echo.as_secs_f64() - without_echo.as_secs_f64()
    );
  }

  #[test]
  fn heartbeat_graph_produces_sound() {
    let base = Patch::default();
    let patches: Vec<Patch> = [440.0, 550.0, 660.0]
      .iter()
      .map(|&f| Patch {
        freq: f,
        duration: 0.4,
        ..base.clone()
      })
      .collect();
    let mut graph = heartbeat_graph(&patches);
    graph.set_sample_rate(44100.0);
    graph.allocate();

    let samples =
      (heartbeat_duration(&patches).as_secs_f32() * 44100.0) as usize;
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
    let base = Patch::default();
    let freqs = [220.0, 330.0, 440.0, 550.0, 660.0];
    let patches: Vec<Patch> = freqs
      .iter()
      .map(|&f| Patch {
        freq: f,
        duration: 0.24,
        ..base.clone()
      })
      .collect();
    let mut graph = heartbeat_graph(&patches);
    graph.set_sample_rate(44100.0);
    graph.allocate();

    let samples =
      (heartbeat_duration(&patches).as_secs_f32() * 44100.0) as usize;
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
  fn sustain_below_one_lowers_body_amplitude() {
    let full = Patch {
      freq: 440.0,
      duration: 0.3,
      sustain: 1.0,
      attack_ms: 10.0,
      release_ms: 50.0,
      ..Default::default()
    };
    let half = Patch {
      sustain: 0.5,
      ..full.clone()
    };

    let mut g_full = boop_graph(&full);
    g_full.set_sample_rate(44100.0);
    g_full.allocate();

    let mut g_half = boop_graph(&half);
    g_half.set_sample_rate(44100.0);
    g_half.allocate();

    let body_start = (0.015 * 44100.0) as usize;
    let body_end = (0.25 * 44100.0) as usize;

    for _ in 0..body_start {
      g_full.get_stereo();
      g_half.get_stereo();
    }

    let full_rms: f32 = (body_start..body_end)
      .map(|_| {
        let (l, _) = g_full.get_stereo();
        l * l
      })
      .sum::<f32>()
      / (body_end - body_start) as f32;

    let half_rms: f32 = (body_start..body_end)
      .map(|_| {
        let (l, _) = g_half.get_stereo();
        l * l
      })
      .sum::<f32>()
      / (body_end - body_start) as f32;

    assert!(
      half_rms < full_rms,
      "sustain=0.5 body RMS ({half_rms:.6}) should be lower \
       than sustain=1.0 ({full_rms:.6})"
    );
  }
}
