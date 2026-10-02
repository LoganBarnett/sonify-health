use crate::patch::Patch;
use fundsp::net::Net;
use fundsp::prelude32::*;
use fundsp::shared::Shared;
use std::time::Duration;

/// A single resolved note ready for audio rendering.
#[derive(Clone)]
pub struct ResolvedNote {
  pub patch: Patch,
  pub volume: f64,
  pub offset: f64,
}

/// Maximum lowpass cutoff in Hz; equals the `lowpass` parameter's `max` in
/// patch.rs (enforced by a test there), where the top of the range means "off".
pub const MAX_CUTOFF: f32 = 18000.0;

/// Maximum highpass cutoff in Hz; equals the `highpass` parameter's `max` in
/// patch.rs (enforced by a test there).  Sits far enough below
/// `CUTOFF_RATE_FRACTION` of any real device rate that a highpass stage never
/// needs rate-aware handling.
pub const MAX_HIGHPASS: f32 = 2000.0;

/// Fraction of the sample rate above which a cutoff cannot be realized:
/// fundsp's filters compute `tan(pi * cutoff / rate)`, which is only meaningful
/// below Nyquist (rate / 2); 0.45 leaves headroom before the asymptote.
const CUTOFF_RATE_FRACTION: f64 = 0.45;

/// Highest stable filter cutoff in Hz at the given sample rate.
pub(crate) fn cutoff_ceiling(sample_rate: f64) -> f32 {
  MAX_CUTOFF.min((CUTOFF_RATE_FRACTION * sample_rate) as f32)
}

/// Effective highpass cutoff in Hz, or `None` when the filter is off.  `0 =
/// off` means the filter stage is absent from the graph rather than parked at
/// an inaudible sentinel cutoff, so "off" is bit-exact on every device.  The
/// comparison also routes a NaN value to off.
pub(crate) fn highpass_cutoff(patch: &Patch) -> Option<f32> {
  (patch.highpass > 0.0).then_some((patch.highpass as f32).min(MAX_HIGHPASS))
}

/// Effective lowpass cutoff in Hz, or `None` when the filter is off.  Off sits
/// at the top of the range: at or above the stable ceiling the requested
/// passband already covers everything the device can represent, so omitting the
/// stage renders the setting exactly.  On 44.1/48 kHz devices the ceiling is
/// `MAX_CUTOFF`, making the slider's top position a true bypass; only degraded
/// low-rate devices (Bluetooth HFP, 22.05/16 kHz) pull it lower.  The
/// comparison routes a NaN value to off; the 20 Hz floor matches the
/// parameter's `min` in patch.rs, guarding values that arrive through config
/// files, which bypass `set_param`'s clamping.
pub(crate) fn lowpass_cutoff(patch: &Patch, sample_rate: f64) -> Option<f32> {
  let cutoff = patch.lowpass as f32;
  (cutoff < cutoff_ceiling(sample_rate)).then_some(cutoff.max(20.0))
}

/// Append the engaged filter stages to a mono chain.  A disengaged filter
/// contributes no node at all, so "off" is a true bypass.
fn with_filter_stages(mono: Net, patch: &Patch, sample_rate: f64) -> Net {
  [
    highpass_cutoff(patch).map(|hz| Net::wrap(Box::new(highpass_hz(hz, 0.7)))),
    lowpass_cutoff(patch, sample_rate)
      .map(|hz| Net::wrap(Box::new(lowpass_hz(hz, 0.7)))),
  ]
  .into_iter()
  .flatten()
  .fold(mono, |acc, stage| acc >> stage)
}

/// Total wall-clock duration of a multi-note heartbeat.  Each note
/// is independently timed from its offset, so the duration is the
/// maximum across all notes of `offset + attack + duration + release
/// + echo_tail`, plus a safety margin.
pub fn heartbeat_notes_duration(notes: &[ResolvedNote]) -> Duration {
  if notes.is_empty() {
    return Duration::ZERO;
  }
  let max = notes
    .iter()
    .map(|n| {
      let p = &n.patch;
      let attack = p.attack_ms / 1000.0;
      let decay = p.decay_ms / 1000.0;
      let release = p.release_ms / 1000.0;
      let echo_tail = if p.echo_mix > 0.0 {
        4.0 * p.echo_delay
      } else {
        0.0
      };
      n.offset + attack + decay + p.duration + release + echo_tail
    })
    .fold(0.0f64, f64::max);
  Duration::from_secs_f64(max + 0.05)
}

/// Content-only duration of a multi-note heartbeat: the maximum
/// across all notes of `offset + attack + decay + duration + gap`.
/// Excludes release tails, echo decay, and safety margin so that
/// `replace()` fires while the last note is still sustaining,
/// letting the crossfade overlap sound with sound.  The per-note
/// `gap` shifts repeat timing: positive adds silence between
/// repetitions, negative causes overlapping re-triggers.  The
/// result is clamped to a 0.05 s floor to prevent a tight loop.
pub fn heartbeat_notes_content_duration(notes: &[ResolvedNote]) -> Duration {
  if notes.is_empty() {
    return Duration::ZERO;
  }
  let max = notes
    .iter()
    .map(|n| {
      let attack = n.patch.attack_ms / 1000.0;
      let decay = n.patch.decay_ms / 1000.0;
      n.offset + attack + decay + n.patch.duration + n.patch.gap
    })
    .fold(0.0f64, f64::max);
  Duration::from_secs_f64(max.max(0.05))
}

/// Build a complete stereo graph for a single note.  All synthesis
/// parameters are read from the note's own patch — nothing is shared
/// with other notes.  The note is silent until `offset` seconds, then
/// plays its full ADSR envelope with drive, noise, sub-octave, FM,
/// tremolo, echo, pan, and reverb.
fn note_graph(
  patch: &Patch,
  offset: f64,
  external_volume: Option<&Shared>,
  sample_rate: f64,
) -> Box<dyn AudioUnit> {
  let freq = patch.freq as f32 * (2.0_f32).powf(patch.detune as f32 / 1200.0);
  let amp = patch.amplitude as f32;
  let attack = patch.attack_ms as f32 / 1000.0;
  let decay = patch.decay_ms as f32 / 1000.0;
  let release = patch.release_ms as f32 / 1000.0;
  let dur = patch.duration as f32;
  let sustain_level = patch.sustain as f32;
  let offset = offset as f32;
  let tail_end = offset + attack + decay + dur + release;

  let chirp_ratio = patch.chirp_ratio as f32;
  let brightness = patch.brightness as f32;
  let resonance = patch.resonance as f32;
  let sub_mix = patch.sub_octave as f32;
  let drive = (patch.drive as f32).max(0.01);
  let drive_norm = 1.0 / drive.tanh();
  let noise_mix = patch.noise_mix as f32;
  let crush_param = patch.crush as f32;
  let crush_levels = 2.0_f32.powf(1.0 + 15.0 * (1.0 - crush_param));
  let downsample = patch.downsample as f32;
  let ds_rate = 100_000.0_f32 / 2.0_f32.powf(downsample * 8.0);
  let vibrato_rate = patch.vibrato_rate;
  let vibrato_depth = patch.vibrato_depth;
  let tremolo_rate = patch.tremolo_rate;
  let tremolo_depth = patch.tremolo_depth;
  let fm_ratio = patch.fm_ratio as f32;
  let fm_depth = patch.fm_depth as f32;

  let total_ratio =
    patch.sine_ratio + patch.tri_ratio + patch.saw_ratio + patch.square_ratio;
  let norm = if total_ratio > 0.0 {
    1.0 / total_ratio
  } else {
    1.0
  } as f32;

  let h = (patch.harshness_offset as f32).clamp(-1.0, 1.0);
  let sine_w = (patch.sine_ratio as f32 * norm * (1.0 - h)).max(0.0);
  let tri_w = patch.tri_ratio as f32 * norm;
  let saw_w = (patch.saw_ratio as f32 * norm + h).max(0.0);
  let square_w = patch.square_ratio as f32 * norm;

  // Frequency LFO with offset gating.
  let freq_env = lfo(move |t: f32| {
    if t < offset || t >= tail_end {
      return 0.01;
    }
    let local_t = t - offset;
    let body_t = (local_t - attack).max(0.0);
    let chirp_t = (body_t / 0.04).min(1.0);
    let base = freq * chirp_ratio + (freq - freq * chirp_ratio) * chirp_t;
    let vib = 2f64.powf(
      vibrato_depth * (std::f64::consts::TAU * vibrato_rate * t as f64).sin()
        / 12.0,
    ) as f32;
    let fm_freq = base * fm_ratio;
    let fm_mod =
      fm_depth * fm_freq * (std::f32::consts::TAU * fm_freq * t).sin();
    (base * vib + fm_mod).max(0.01)
  });

  // Amplitude envelope with offset gating.
  let amp_env = envelope(move |t: f32| {
    if t < offset || t >= tail_end {
      return 0.0;
    }
    let local_t = t - offset;
    let level = if attack > 0.0 && local_t < attack {
      local_t / attack
    } else if decay > 0.0 && local_t < attack + decay {
      1.0 + (sustain_level - 1.0) * (local_t - attack) / decay
    } else {
      let body_end = attack + decay + dur;
      if local_t <= body_end {
        sustain_level
      } else if release > 0.0 {
        (sustain_level * (body_end + release - local_t) / release).max(0.0)
      } else {
        0.0
      }
    };
    let trem = (1.0
      - tremolo_depth
        * (1.0 - (std::f64::consts::TAU * tremolo_rate * t as f64).sin())
        / 2.0) as f32;
    level * amp * trem
  });

  // Main oscillator.
  let waveform = (sine() * sine_w)
    & (triangle() * tri_w)
    & (saw() * saw_w)
    & (square() * square_w);
  let main_osc = freq_env >> waveform;

  // Sub-octave oscillator with offset gating.
  let sub_freq_env = lfo(move |t: f32| {
    if t < offset || t >= tail_end {
      return 0.01;
    }
    let local_t = t - offset;
    let body_t = (local_t - attack).max(0.0);
    let chirp_t = (body_t / 0.04).min(1.0);
    let half = freq * 0.5;
    let base = half * chirp_ratio + (half - half * chirp_ratio) * chirp_t;
    let vib = 2f64.powf(
      vibrato_depth * (std::f64::consts::TAU * vibrato_rate * t as f64).sin()
        / 12.0,
    ) as f32;
    let fm_freq = base * fm_ratio;
    let fm_mod =
      fm_depth * fm_freq * (std::f32::consts::TAU * fm_freq * t).sin();
    (base * vib + fm_mod).max(0.01)
  });
  let sub_waveform = (sine() * sine_w)
    & (triangle() * tri_w)
    & (saw() * saw_w)
    & (square() * square_w);
  let sub_osc = sub_freq_env >> sub_waveform;

  // Drive, noise, filter, effects.
  let cutoff = dc((freq * 13.0 * brightness).min(cutoff_ceiling(sample_rate)));
  let q_val = dc(0.5 * resonance * 0.2);

  let ext = external_volume
    .map_or_else(|| fundsp::prelude32::shared(1.0), |s| s.clone());
  let ext_vol = var(&ext) >> follow(0.1);

  let echo_delay = patch.echo_delay as f32;
  let echo_mix = patch.echo_mix as f32;

  let driven = (main_osc + (sub_osc * sub_mix))
    >> (shape(Tanh(drive)) * drive_norm)
    >> (pass() + (pink() * noise_mix));
  let mono = (driven | cutoff | q_val)
    >> (moog() * amp_env * ext_vol)
    >> shape(Crush(crush_levels))
    >> hold_hz(ds_rate, 0.0);
  let filtered =
    with_filter_stages(Net::wrap(Box::new(mono)), patch, sample_rate);
  let tail = (pass() & (feedback(delay(echo_delay) * 0.3) * echo_mix))
    >> pan(patch.stereo_pan as f32)
    >> reverb_stereo(0.3, 0.8, patch.reverb_mix as f32);
  Box::new(filtered >> Net::wrap(Box::new(tail)))
}

/// Build a multi-note heartbeat audio graph.  Each note is rendered
/// as a fully independent stereo graph with its own synthesis
/// parameters, then summed via `Net`.  Per-note volume scales the
/// note's amplitude.  The optional external volume `Shared`
/// multiplies each note's output.
pub fn heartbeat_graph_with_notes(
  notes: &[ResolvedNote],
  external_volume: Option<&Shared>,
  sample_rate: f64,
) -> Box<dyn AudioUnit> {
  let mut iter = notes.iter().map(|n| {
    let mut p = n.patch.clone();
    p.amplitude *= n.volume;
    Net::wrap(note_graph(&p, n.offset, external_volume, sample_rate))
  });
  let Some(first) = iter.next() else {
    // Empty notes — return silence with the same external-volume
    // wiring the populated case uses, so a downstream consumer
    // that compares the two graphs sees the same I/O shape.
    let ext = external_volume
      .map_or_else(|| fundsp::prelude32::shared(1.0), |s| s.clone());
    return Box::new((dc(0.0) * var(&ext)) | (dc(0.0) * var(&ext)));
  };
  Box::new(iter.fold(first, |acc, n| acc + n))
}

/// Build an audio graph for a single boop.  Duration and
/// frequency come from the patch itself.
pub fn boop_graph(patch: &Patch, sample_rate: f64) -> Box<dyn AudioUnit> {
  let freq = patch.freq as f32 * (2.0_f32).powf(patch.detune as f32 / 1200.0);
  let amp = patch.amplitude as f32;
  let harshness = (patch.harshness_offset as f32).clamp(-1.0, 1.0);
  let attack = (patch.attack_ms / 1000.0) as f32;
  let decay = (patch.decay_ms / 1000.0) as f32;
  let release = (patch.release_ms / 1000.0).min(patch.duration * 0.5) as f32;
  let dur = patch.duration as f32;

  let total_ratio =
    patch.sine_ratio + patch.tri_ratio + patch.saw_ratio + patch.square_ratio;
  let norm = if total_ratio > 0.0 {
    1.0 / total_ratio
  } else {
    1.0
  } as f32;

  let sine_w = (patch.sine_ratio as f32 * norm * (1.0 - harshness)).max(0.0);
  let tri_w = patch.tri_ratio as f32 * norm;
  let saw_w = (patch.saw_ratio as f32 * norm + harshness).max(0.0);
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

  let cutoff = dc(
    (freq * 13.0 * patch.brightness as f32).min(cutoff_ceiling(sample_rate)),
  );
  let q_val = dc((0.5 * patch.resonance as f32 * 0.2).min(0.95));

  let sustain_level = patch.sustain as f32;
  let env = envelope(move |t: f32| {
    let level = if attack > 0.0 && t < attack {
      t / attack
    } else if decay > 0.0 && t < attack + decay {
      1.0 + (sustain_level - 1.0) * (t - attack) / decay
    } else {
      let body_end = attack + decay + dur;
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
  let filtered =
    with_filter_stages(Net::wrap(Box::new(mono)), patch, sample_rate);
  let tail = pass() & (feedback(delay(echo_delay) * 0.3) * echo_mix);
  Box::new(filtered >> Net::wrap(Box::new(tail)))
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

  /// Mean square of a graph's left channel over the note body (0.015-0.25 s at
  /// 44.1 kHz), skipping the attack transient.
  fn body_mean_square(graph: &mut Box<dyn AudioUnit>) -> f32 {
    graph.set_sample_rate(44100.0);
    graph.allocate();
    let body_start = (0.015 * 44100.0) as usize;
    let body_end = (0.25 * 44100.0) as usize;
    for _ in 0..body_start {
      graph.get_stereo();
    }
    (body_start..body_end)
      .map(|_| {
        let (l, _) = graph.get_stereo();
        l * l
      })
      .sum::<f32>()
      / (body_end - body_start) as f32
  }

  fn boop_body_mean_square(patch: &Patch) -> f32 {
    body_mean_square(&mut boop_graph(patch, 44100.0))
  }

  /// Same measurement through the note_graph path the daemon plays.
  fn note_body_mean_square(patch: &Patch) -> f32 {
    let notes = [ResolvedNote {
      patch: patch.clone(),
      volume: 1.0,
      offset: 0.0,
    }];
    body_mean_square(&mut heartbeat_graph_with_notes(&notes, None, 44100.0))
  }

  #[test]
  fn boop_produces_sound() {
    let patch = test_patch(440.0, 0.5);
    let mut graph = boop_graph(&patch, 44100.0);
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
  fn notes_duration_single() {
    let patch = Patch {
      freq: 440.0,
      duration: 1.2,
      attack_ms: 0.0,
      release_ms: 150.0,
      echo_mix: 0.0,
      ..Default::default()
    };
    let notes = [ResolvedNote {
      patch,
      volume: 1.0,
      offset: 0.0,
    }];
    let dur = heartbeat_notes_duration(&notes);
    assert!(
      (dur.as_secs_f64() - 1.4).abs() < 1e-10,
      "Single note should be duration + release + margin, got {:.3}",
      dur.as_secs_f64()
    );
  }

  #[test]
  fn notes_duration_empty() {
    assert_eq!(heartbeat_notes_duration(&[]), Duration::ZERO);
  }

  #[test]
  fn notes_duration_includes_echo_tail() {
    let base = Patch {
      freq: 440.0,
      duration: 1.0,
      attack_ms: 0.0,
      release_ms: 150.0,
      echo_delay: 0.3,
      ..Default::default()
    };
    let without = heartbeat_notes_duration(&[ResolvedNote {
      patch: Patch {
        echo_mix: 0.0,
        ..base.clone()
      },
      volume: 1.0,
      offset: 0.0,
    }]);
    let with = heartbeat_notes_duration(&[ResolvedNote {
      patch: Patch {
        echo_mix: 0.5,
        ..base
      },
      volume: 1.0,
      offset: 0.0,
    }]);
    assert!(
      (with.as_secs_f64() - without.as_secs_f64() - 1.2).abs() < 1e-10,
      "Echo tail should add 4 x delay, got delta {:.3}",
      with.as_secs_f64() - without.as_secs_f64()
    );
  }

  #[test]
  fn multi_note_graph_produces_sound() {
    let base = Patch::default();
    let notes: Vec<ResolvedNote> = [440.0, 550.0, 660.0]
      .iter()
      .enumerate()
      .map(|(i, &f)| ResolvedNote {
        patch: Patch {
          freq: f,
          duration: 0.4,
          ..base.clone()
        },
        volume: 1.0,
        offset: i as f64 * 0.5,
      })
      .collect();
    let mut graph = heartbeat_graph_with_notes(&notes, None, 44100.0);
    graph.set_sample_rate(44100.0);
    graph.allocate();

    let samples =
      (heartbeat_notes_duration(&notes).as_secs_f32() * 44100.0) as usize;
    let peak = (0..samples)
      .map(|_| {
        let (l, r) = graph.get_stereo();
        l.abs().max(r.abs())
      })
      .fold(0.0f32, f32::max);

    assert!(
      peak > 0.001,
      "Multi-note heartbeat should produce audible samples, \
       got peak {}",
      peak
    );
  }

  #[test]
  fn five_note_graph_produces_sound() {
    let base = Patch::default();
    let notes: Vec<ResolvedNote> = [220.0, 330.0, 440.0, 550.0, 660.0]
      .iter()
      .enumerate()
      .map(|(i, &f)| ResolvedNote {
        patch: Patch {
          freq: f,
          duration: 0.24,
          ..base.clone()
        },
        volume: 1.0,
        offset: i as f64 * 0.3,
      })
      .collect();
    let mut graph = heartbeat_graph_with_notes(&notes, None, 44100.0);
    graph.set_sample_rate(44100.0);
    graph.allocate();

    let samples =
      (heartbeat_notes_duration(&notes).as_secs_f32() * 44100.0) as usize;
    let peak = (0..samples)
      .map(|_| {
        let (l, r) = graph.get_stereo();
        l.abs().max(r.abs())
      })
      .fold(0.0f32, f32::max);

    assert!(
      peak > 0.001,
      "Five-note heartbeat should produce audible samples, \
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

    let full_ms = boop_body_mean_square(&full);
    let half_ms = boop_body_mean_square(&half);

    assert!(
      half_ms < full_ms,
      "sustain=0.5 body mean square ({half_ms:.6}) should be lower \
       than sustain=1.0 ({full_ms:.6})"
    );
  }

  #[test]
  fn lowpass_attenuates_bright_patch() {
    let bright = Patch {
      freq: 880.0,
      duration: 0.3,
      sine_ratio: 0.0,
      saw_ratio: 1.0,
      attack_ms: 10.0,
      release_ms: 50.0,
      ..Default::default()
    };
    let dark = Patch {
      lowpass: 200.0,
      ..bright.clone()
    };

    let bright_ms = boop_body_mean_square(&bright);
    let dark_ms = boop_body_mean_square(&dark);

    // A 200 Hz second-order lowpass on an 880 Hz saw cuts the fundamental to
    // roughly (200/880)^4 of its power, so 0.5 leaves a wide margin.
    assert!(
      dark_ms < bright_ms * 0.5,
      "lowpass=200 body mean square ({dark_ms:.6}) should be well \
       below the unfiltered value ({bright_ms:.6})"
    );
  }

  #[test]
  fn lowpass_attenuates_note_graph() {
    let bright = Patch {
      freq: 880.0,
      duration: 0.3,
      sine_ratio: 0.0,
      saw_ratio: 1.0,
      attack_ms: 10.0,
      release_ms: 50.0,
      ..Default::default()
    };
    let dark = Patch {
      lowpass: 200.0,
      ..bright.clone()
    };

    let bright_ms = note_body_mean_square(&bright);
    let dark_ms = note_body_mean_square(&dark);

    assert!(
      dark_ms < bright_ms * 0.5,
      "lowpass=200 note body mean square ({dark_ms:.6}) should be \
       well below the unfiltered value ({bright_ms:.6})"
    );
  }

  #[test]
  fn lowpass_cutoff_bypasses_at_ceiling() {
    let mut patch = Patch::default();
    assert_eq!(lowpass_cutoff(&patch, 44100.0), None);
    patch.lowpass = 5000.0;
    assert_eq!(lowpass_cutoff(&patch, 44100.0), Some(5000.0));
    // A device whose representable band sits entirely inside the
    // requested passband renders the setting exactly by omitting the
    // stage.
    assert_eq!(lowpass_cutoff(&patch, 8000.0), None);
    patch.lowpass = f64::NAN;
    assert_eq!(lowpass_cutoff(&patch, 44100.0), None);
    patch.lowpass = 0.0;
    assert_eq!(lowpass_cutoff(&patch, 44100.0), Some(20.0));
  }

  #[test]
  fn highpass_cutoff_bypasses_at_zero() {
    let mut patch = Patch::default();
    assert_eq!(highpass_cutoff(&patch), None);
    patch.highpass = 120.0;
    assert_eq!(highpass_cutoff(&patch), Some(120.0));
    patch.highpass = f64::NAN;
    assert_eq!(highpass_cutoff(&patch), None);
    patch.highpass = 30000.0;
    assert_eq!(highpass_cutoff(&patch), Some(MAX_HIGHPASS));
  }

  /// Render note_graph at various frequencies using the star-trek-ok
  /// patch shape and measure the peak amplitude in the final 256
  /// samples (just before the mixer would hard-remove the slot).
  /// The Moog filter leaves residual energy at certain frequencies
  /// (notably 440 Hz and 780 Hz).  The mixer's `remove()` method
  /// crossfades to silence over `REMOVE_FADEOUT_FRAMES` to mask
  /// this, but this test documents which frequencies are affected.
  #[test]
  fn tail_residual_across_frequencies() {
    let base = Patch {
      freq: 4307.0,
      sine_ratio: 2.37,
      tri_ratio: 1.22,
      saw_ratio: 0.02,
      square_ratio: 0.0,
      duration: 0.22,
      attack_ms: 6.0,
      decay_ms: 0.0,
      release_ms: 22.0,
      reverb_mix: 0.89,
      chirp_ratio: 1.01,
      amplitude: 0.327,
      brightness: 1.82,
      drive: 0.5,
      echo_delay: 0.32,
      echo_mix: 0.33,
      resonance: 3.72,
      stereo_pan: -0.42,
      sub_octave: 0.03,
      sustain: 1.0,
      tremolo_depth: 0.11,
      vibrato_depth: 0.49,
      ..Default::default()
    };

    let test_freqs = [
      100.0, 200.0, 300.0, 440.0, 600.0, 780.0, 1000.0, 1538.0, 2000.0, 3000.0,
      4307.0,
    ];
    let tail_window = 256;
    // Threshold: anything above this in the final samples will click.
    let click_threshold = 0.001;
    let mut failures = Vec::new();

    for &freq in &test_freqs {
      let patch = Patch {
        freq,
        ..base.clone()
      };
      let notes = [ResolvedNote {
        patch,
        volume: 1.0,
        offset: 0.0,
      }];
      let dur = heartbeat_notes_duration(&notes);
      let total_samples = (dur.as_secs_f32() * 44100.0) as usize;

      let mut graph = heartbeat_graph_with_notes(&notes, None, 44100.0);
      graph.set_sample_rate(44100.0);
      graph.allocate();

      // Render all but the last tail_window samples.
      for _ in 0..(total_samples - tail_window) {
        graph.get_stereo();
      }

      // Measure peak in the final window.
      let mut tail_peak: f32 = 0.0;
      for _ in 0..tail_window {
        let (l, r) = graph.get_stereo();
        tail_peak = tail_peak.max(l.abs()).max(r.abs());
      }

      eprintln!(
        "freq={freq:>7.1}  tail_peak={tail_peak:.6}  {}",
        if tail_peak > click_threshold {
          "CLICK"
        } else {
          "ok"
        }
      );

      if tail_peak > click_threshold {
        failures.push((freq, tail_peak));
      }
    }

    // Document affected frequencies rather than failing — the mixer
    // fadeout in `remove()` handles these at runtime.
    if !failures.is_empty() {
      eprintln!(
        "Frequencies with Moog filter residual (handled by mixer fadeout): {:?}",
        failures
      );
    }
    // No frequency should exceed a hard ceiling that would click
    // even through the 128-frame fadeout.  At 128 frames the
    // fadeout attenuates linearly, so the final sample is
    // peak * (1/128) ≈ peak * 0.008.  Anything under 0.125 would
    // produce a final sample below 0.001 after fadeout.
    let hard_ceiling = 0.125;
    let severe: Vec<_> = failures
      .iter()
      .filter(|(_, peak)| *peak > hard_ceiling)
      .collect();
    assert!(
      severe.is_empty(),
      "Frequencies with residual too large for fadeout: {:?}",
      severe
    );
  }
}
