use crate::downsample::exact_hold_hz;
use crate::patch::Patch;
use fundsp::net::Net;
use fundsp::prelude32::{
  bell_hz, busi, db_amp, dc, dcblock, delay, envelope, feedback, follow,
  highpass_hz, lfo, lowpass_hz, moog, mul, multipass, pan, pass, pink,
  reverb_stereo, saw, shape, sine, square, triangle, var, white, An, AudioNode,
  AudioUnit, Crush, Tanh, U0, U1, U2,
};
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

/// Fraction of the sample rate above which a cutoff cannot be realized:
/// fundsp's filters compute `tan(pi * cutoff / rate)`, which is only meaningful
/// below Nyquist (rate / 2); 0.45 leaves headroom before the asymptote.
const CUTOFF_RATE_FRACTION: f64 = 0.45;

/// Highest stable filter cutoff in Hz at the given sample rate.  This is the
/// device-dependent hard limit on `highpass` and `eq_hz`, which patch.rs cannot
/// declare statically.
pub(crate) fn rate_ceiling(sample_rate: f64) -> f32 {
  (CUTOFF_RATE_FRACTION * sample_rate) as f32
}

/// The rate ceiling, capped at the lowpass slider's top so that position is
/// "off" on every common device.  It also caps the pitch-tracking ladder.
pub(crate) fn cutoff_ceiling(sample_rate: f64) -> f32 {
  MAX_CUTOFF.min(rate_ceiling(sample_rate))
}

/// Effective highpass cutoff in Hz, or `None` when the filter is off.  `0 =
/// off` means the filter stage is absent from the graph rather than parked at
/// an inaudible sentinel cutoff, so "off" is bit-exact on every device.  The
/// comparison also routes a NaN value to off.
pub(crate) fn highpass_cutoff(patch: &Patch, sample_rate: f64) -> Option<f32> {
  (patch.highpass > 0.0)
    .then_some((patch.highpass as f32).min(rate_ceiling(sample_rate)))
}

/// Effective lowpass cutoff in Hz, or `None` when the filter is off.  Off sits
/// at the top of the range: at or above the stable ceiling the requested
/// passband already covers everything the device can represent, so omitting the
/// stage renders the setting exactly.  On 44.1/48 kHz devices the ceiling is
/// `MAX_CUTOFF`, making the slider's top position a true bypass; only degraded
/// low-rate devices (Bluetooth HFP, 22.05/16 kHz) pull it lower.  The
/// comparison routes a NaN value to off.
pub(crate) fn lowpass_cutoff(patch: &Patch, sample_rate: f64) -> Option<f32> {
  let cutoff = patch.lowpass as f32;
  (cutoff < cutoff_ceiling(sample_rate)).then_some(cutoff)
}

/// One parametric EQ band in the form fundsp's bell takes: centre in Hz, Q,
/// and linear amplitude gain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EqBand {
  pub hz: f32,
  pub q: f32,
  pub gain: f32,
}

impl EqBand {
  /// The band for a patch already held to its hard limits.  The centre's
  /// upper limit is the device's rate ceiling.
  pub(crate) fn from_patch(patch: &Patch, sample_rate: f64) -> Self {
    EqBand {
      hz: (patch.eq_hz as f32).min(rate_ceiling(sample_rate)),
      q: patch.eq_q as f32,
      gain: db_amp(patch.eq_db as f32),
    }
  }

  /// Unity gain gives `m1 = m2 = 0` in fundsp's state-variable filter, an
  /// exact passthrough, so a 0 dB band contributes no node and "off" is a
  /// true bypass.
  pub(crate) fn engaged(&self) -> bool {
    self.gain != 1.0
  }

  /// The band as a graph stage, or `None` while it is a bypass.
  pub(crate) fn stage(&self) -> Option<Net> {
    self
      .engaged()
      .then(|| Net::wrap(Box::new(bell_hz(self.hz, self.q, self.gain))))
  }
}

/// Append the engaged filter stages to a mono chain.  The EQ band precedes the
/// band limits so those have the last word; a disengaged stage contributes no
/// node, so "off" is a true bypass.
fn with_filter_stages(mono: Net, patch: &Patch, sample_rate: f64) -> Net {
  [
    EqBand::from_patch(patch, sample_rate).stage(),
    highpass_cutoff(patch, sample_rate)
      .map(|hz| Net::wrap(Box::new(highpass_hz(hz, 0.7)))),
    lowpass_cutoff(patch, sample_rate)
      .map(|hz| Net::wrap(Box::new(lowpass_hz(hz, 0.7)))),
  ]
  .into_iter()
  .flatten()
  .fold(mono, |acc, stage| acc >> stage)
}

/// Start phase, in turns, of every oscillator in every graph.
///
/// Engaging a filter stage or adding a note changed a driven patch's timbre (a
/// 20 Hz highpass stage shifted a 33 Hz partial by 7 dB): left unset, fundsp
/// starts each oscillator at a phase drawn from a hash of the whole graph, and
/// `Net` rehashes on every structural change, which moves every start phase
/// and, through the drive stage, the timbre.  Zero puts every waveform at a
/// rising zero crossing.
pub(crate) const OSCILLATOR_PHASE: f32 = 0.0;

/// Seed of the pink-noise source.  Unseeded, fundsp draws the noise sequence
/// from the graph hash, which the same structural changes reshuffle.  Notes
/// sounding together would otherwise share one sequence, so each adds its
/// index.
pub(crate) const NOISE_SEED_BASE: u64 = 1;

/// Seed base of the hiss source; each note adds its noise index.  Pink is
/// filtered white, so a hiss seed equal to a pink seed (`NOISE_SEED_BASE` plus
/// the index) would correlate the two sources, and notes sounding together
/// would otherwise share one hiss sequence.
pub(crate) const HISS_SEED_BASE: u64 = 1 << 32;

/// Room size, in metres, of the hall every patch's reverb runs through.
pub(crate) const REVERB_ROOM_M: f32 = 10.0;

/// Decay argument handed to fundsp's reverb.  The tail it produces lasts about
/// 1.6 times this value (measured: 1.12 s at 0.7): fundsp derives the per-pass
/// gain from a nominal 30 ms delay line while the lines it builds average
/// 62 ms.  `reverb_tail_decays_within_budget` keeps the realised tail inside
/// `REVERB_TAIL_SECS`.
pub(crate) const REVERB_TIME_S: f32 = 0.95;

/// High-frequency damping of the hall, in fundsp's 0..1.
pub(crate) const REVERB_DAMPING: f32 = 0.5;

/// Gain on the wet path, fixed by `reverb_mix_preserves_sustained_loudness` so
/// a sustained tone at `reverb_mix = 1` lands within 3 dB of the dry tone.
pub(crate) const REVERB_WET_GAIN: f32 = 5.6;

/// Seconds a note's reverb tail is given before its slot is removed.
pub(crate) const REVERB_TAIL_SECS: f64 = 1.8;

/// Pitch in Hz after `detune`, which both graphs apply to every oscillator.
pub(crate) fn pitch_hz(patch: &Patch) -> f32 {
  patch.freq as f32 * (2.0_f32).powf(patch.detune as f32 / 1200.0)
}

/// Waveform weights as (sine, triangle, saw, square): the ratios normalised to
/// sum to one, then `harshness_offset` moves weight from sine to saw.
pub(crate) fn waveform_weights(patch: &Patch) -> (f32, f32, f32, f32) {
  let total_ratio =
    patch.sine_ratio + patch.tri_ratio + patch.saw_ratio + patch.square_ratio;
  let norm = if total_ratio > 0.0 {
    1.0 / total_ratio
  } else {
    1.0
  } as f32;
  let h = patch.harshness_offset as f32;
  (
    patch.sine_ratio as f32 * norm * (1.0 - h),
    patch.tri_ratio as f32 * norm,
    (patch.saw_ratio as f32 * norm + h).max(0.0),
    patch.square_ratio as f32 * norm,
  )
}

/// Pitch ratio of spread voice `voice` (0 = lower, 1 = upper), each half the
/// spread away from the pitch.  At zero cents both ratios are exactly 1.0 and
/// `a * 0.5 + a * 0.5 == a` in f32, so the default sums to the single voice.
pub(crate) fn spread_voice_ratio(cents: f32, voice: u64) -> f32 {
  let sign = if voice == 0 { -1.0 } else { 1.0 };
  2.0_f32.powf(sign * cents / 2400.0)
}

/// Bend exponent of the envelope ramps, or `None` for straight lines.
/// `powf(x, 1.0) == x` is not guaranteed in f32 and the fit tool compares
/// renders bit for bit, so a curve of 1 reads as `None` and the linear arms
/// keep their plain expressions.
pub(crate) fn envelope_curve(patch: &Patch) -> Option<f32> {
  let curve = patch.envelope_curve as f32;
  (curve != 1.0).then_some(curve)
}

/// The amplitude envelope of a one-shot note, in seconds from its start.
/// Continuous playback has no envelope, so `curve` has no effect there.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Adsr {
  pub attack: f32,
  pub decay: f32,
  pub sustain: f32,
  /// Seconds held at `sustain` between the decay and the release.
  pub body: f32,
  pub release: f32,
  /// Bend of the ramps; see `envelope_curve`.
  pub curve: Option<f32>,
}

impl Adsr {
  /// Level in 0..=1 at `t` seconds after the note starts.  A curve raises the
  /// attack's progress, and the decay's and release's remaining fraction, to
  /// its power, so above 1 the attack lingers before swelling and the falls
  /// start fast then trail off; below 1 the reverse.
  pub(crate) fn level(&self, t: f32) -> f32 {
    let Adsr {
      attack,
      decay,
      sustain,
      body,
      release,
      curve,
    } = *self;
    if attack > 0.0 && t < attack {
      curve.map_or(t / attack, |c| (t / attack).powf(c))
    } else if decay > 0.0 && t < attack + decay {
      curve.map_or(1.0 + (sustain - 1.0) * (t - attack) / decay, |c| {
        sustain + (1.0 - sustain) * (1.0 - (t - attack) / decay).powf(c)
      })
    } else {
      let body_end = attack + decay + body;
      if t <= body_end {
        sustain
      } else if release > 0.0 {
        curve.map_or(
          (sustain * (body_end + release - t) / release).max(0.0),
          |c| sustain * ((body_end + release - t) / release).max(0.0).powf(c),
        )
      } else {
        0.0
      }
    }
  }
}

/// The four waveforms, each started at `phase` and scaled by its weight,
/// summed.  Weights arrive as nodes so that constants and smoothed `Shared`s
/// both fit.
pub(crate) fn oscillator_bank<W>(
  phase: f32,
  (sine_w, tri_w, saw_w, square_w): (An<W>, An<W>, An<W>, An<W>),
) -> An<impl AudioNode<Inputs = U1, Outputs = U1>>
where
  W: AudioNode<Inputs = U0, Outputs = U1>,
{
  // The phase builder addresses the node it is called on, so it has to reach
  // the bare oscillator: `Binop` would route it to the weight instead.
  (sine().phase(phase) * sine_w)
    & (triangle().phase(phase) * tri_w)
    & (saw().phase(phase) * saw_w)
    & (square().phase(phase) * square_w)
}

/// Dry/wet reverb stage, or `None` when the mix is zero so that "off" is a
/// true bypass.
///
/// fundsp's `reverb_stereo` is wet-only, so the dry path is mixed in here and
/// the room is fixed.
pub(crate) fn reverb_stage(mix: f32) -> Option<Net> {
  (mix > 0.0).then(|| {
    Net::wrap(Box::new(
      (multipass::<U2>() * (1.0 - mix))
        & (reverb_stereo(REVERB_ROOM_M, REVERB_TIME_S, REVERB_DAMPING)
          * (mix * REVERB_WET_GAIN)),
    ))
  })
}

/// Total wall-clock duration of a multi-note heartbeat.  Each note is
/// independently timed from its offset, so the duration is the maximum across
/// all notes of its envelope plus the echo and reverb tails it has engaged,
/// plus a safety margin.  A length past `Duration`'s range saturates.
pub fn heartbeat_notes_duration(notes: &[ResolvedNote]) -> Duration {
  if notes.is_empty() {
    return Duration::ZERO;
  }
  let max = notes
    .iter()
    .map(|n| {
      let p = &n.patch.limited().0;
      let attack = p.attack_ms / 1000.0;
      let decay = p.decay_ms / 1000.0;
      let release = p.release_ms / 1000.0;
      let echo_tail = if p.echo_mix > 0.0 {
        4.0 * p.echo_delay
      } else {
        0.0
      };
      let reverb_tail = if p.reverb_mix > 0.0 {
        REVERB_TAIL_SECS
      } else {
        0.0
      };
      n.offset + attack + decay + p.duration + release + echo_tail + reverb_tail
    })
    .fold(0.0f64, f64::max);
  saturating_secs(max + 0.05)
}

/// Content-only duration of a multi-note heartbeat: the maximum
/// across all notes of `offset + attack + decay + duration + gap`.
/// Excludes release tails, echo decay, and safety margin so that
/// `replace()` fires while the last note is still sustaining,
/// letting the crossfade overlap sound with sound.  The per-note
/// `gap` shifts repeat timing: positive adds silence between
/// repetitions, negative causes overlapping re-triggers.  The
/// result is clamped to a 0.05 s floor to prevent a tight loop, and a
/// length past `Duration`'s range saturates.
pub fn heartbeat_notes_content_duration(notes: &[ResolvedNote]) -> Duration {
  if notes.is_empty() {
    return Duration::ZERO;
  }
  let max = notes
    .iter()
    .map(|n| {
      let p = &n.patch.limited().0;
      let attack = p.attack_ms / 1000.0;
      let decay = p.decay_ms / 1000.0;
      n.offset + attack + decay + p.duration + p.gap
    })
    .fold(0.0f64, f64::max);
  saturating_secs(max.max(0.05))
}

/// `seconds` as a `Duration`.  A patch's times have no upper limit, and
/// `Duration::from_secs_f64` panics past its range, so a note too long to
/// represent lasts `Duration::MAX` instead.
fn saturating_secs(seconds: f64) -> Duration {
  Duration::try_from_secs_f64(seconds).unwrap_or(Duration::MAX)
}

/// Build a complete stereo graph for a single note.  All synthesis
/// parameters are read from the note's own patch — nothing is shared
/// with other notes.  The note is silent until `offset` seconds, then
/// plays its full ADSR envelope through the same effects chain as
/// every other mode.
fn note_graph(
  patch: &Patch,
  offset: f64,
  external_volume: Option<&Shared>,
  sample_rate: f64,
  noise_seed: u64,
) -> Box<dyn AudioUnit> {
  let freq = pitch_hz(patch);
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
  let drive = patch.drive as f32;
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
  let (sine_w, tri_w, saw_w, square_w) = waveform_weights(patch);
  let spread = patch.spread as f32;
  let hiss = patch.hiss as f32;
  let adsr = Adsr {
    attack,
    decay,
    sustain: sustain_level,
    body: dur,
    release,
    curve: envelope_curve(patch),
  };

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
    let level = adsr.level(t - offset);
    let trem = (1.0
      - tremolo_depth
        * (1.0 - (std::f64::consts::TAU * tremolo_rate * t as f64).sin())
        / 2.0) as f32;
    level * amp * trem
  });

  // Two copies of the main bank at half weight, a half spread below and above
  // the pitch.
  let main_osc = freq_env
    >> busi::<U2, _, _>(move |voice| {
      mul(spread_voice_ratio(spread, voice))
        >> (oscillator_bank(
          OSCILLATOR_PHASE,
          (dc(sine_w), dc(tri_w), dc(saw_w), dc(square_w)),
        ) * 0.5)
    });

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
  let sub_osc = sub_freq_env
    >> oscillator_bank(
      OSCILLATOR_PHASE + patch.sub_phase as f32,
      (dc(sine_w), dc(tri_w), dc(saw_w), dc(square_w)),
    );

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
    >> (pass() + (pink().seed(noise_seed) * noise_mix));
  // Asymmetric drive leaves a DC offset as large as the signal itself (a mean
  // of -0.0068 against an RMS of 0.011 on a driven saw with a sub-octave), and
  // the continuous graph already blocks it; the same stage here keeps the two
  // graphs equivalent.
  //
  // Noise mixed before the ladder can only add rumble under a dark filter, and
  // noise outside the envelope would sound between notes, so hiss joins after
  // the ladder and under the envelope.
  let mono = (driven | cutoff | q_val)
    >> ((moog() + (white().seed(HISS_SEED_BASE + noise_seed) * hiss))
      * amp_env
      * ext_vol)
    >> dcblock()
    >> shape(Crush(crush_levels))
    >> exact_hold_hz(ds_rate);
  let filtered =
    with_filter_stages(Net::wrap(Box::new(mono)), patch, sample_rate);
  let tail = (pass() & (feedback(delay(echo_delay) * 0.3) * echo_mix))
    >> pan(patch.stereo_pan as f32);
  Box::new(
    reverb_stage(patch.reverb_mix as f32)
      .into_iter()
      .fold(filtered >> Net::wrap(Box::new(tail)), |acc, stage| acc >> stage),
  )
}

/// Build a multi-note heartbeat audio graph.  Each note is rendered
/// as a fully independent stereo graph with its own synthesis
/// parameters, then summed via `Net`.  Per-note volume scales the
/// note's amplitude.  The optional external volume `Shared`
/// multiplies each note's output.  Each patch is held to its hard limits
/// first, so one built in code gets the same guarantees as one loaded from a
/// config file.
pub fn heartbeat_graph_with_notes(
  notes: &[ResolvedNote],
  external_volume: Option<&Shared>,
  sample_rate: f64,
) -> Box<dyn AudioUnit> {
  let mut iter = notes.iter().enumerate().map(|(index, n)| {
    let mut p = n.patch.limited().0;
    p.amplitude *= n.volume;
    Net::wrap(note_graph(
      &p,
      n.offset,
      external_volume,
      sample_rate,
      NOISE_SEED_BASE + index as u64,
    ))
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
  let patch = &patch.limited().0;
  let freq = pitch_hz(patch);
  let amp = patch.amplitude as f32;
  let attack = (patch.attack_ms / 1000.0) as f32;
  let decay = (patch.decay_ms / 1000.0) as f32;
  let release = (patch.release_ms / 1000.0).min(patch.duration * 0.5) as f32;
  let dur = patch.duration as f32;
  let (sine_w, tri_w, saw_w, square_w) = waveform_weights(patch);
  let spread = patch.spread as f32;
  let hiss = patch.hiss as f32;

  let drive = patch.drive as f32;
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
  let main_osc = freq_source
    >> busi::<U2, _, _>(move |voice| {
      mul(spread_voice_ratio(spread, voice))
        >> (oscillator_bank(
          OSCILLATOR_PHASE,
          (dc(sine_w), dc(tri_w), dc(saw_w), dc(square_w)),
        ) * 0.5)
    });

  let sub_half = freq * 0.5;
  let sub_fm_freq = sub_half * fm_ratio;
  let sub_freq_source = lfo(move |t: f32| {
    let fm_mod =
      fm_depth * sub_fm_freq * (std::f32::consts::TAU * sub_fm_freq * t).sin();
    (sub_half + fm_mod).max(0.01)
  });
  let sub_osc = (sub_freq_source
    >> oscillator_bank(
      OSCILLATOR_PHASE + patch.sub_phase as f32,
      (dc(sine_w), dc(tri_w), dc(saw_w), dc(square_w)),
    ))
    * patch.sub_octave as f32;
  let combined = main_osc + sub_osc;

  let cutoff = dc(
    (freq * 13.0 * patch.brightness as f32).min(cutoff_ceiling(sample_rate)),
  );
  let q_val = dc((0.5 * patch.resonance as f32 * 0.2).min(0.95));

  let adsr = Adsr {
    attack,
    decay,
    sustain: patch.sustain as f32,
    body: dur,
    release,
    curve: envelope_curve(patch),
  };
  let env = envelope(move |t: f32| adsr.level(t) * amp);

  let echo_delay = patch.echo_delay as f32;
  let echo_mix = patch.echo_mix as f32;

  let driven = combined
    >> (shape(Tanh(drive)) * drive_norm)
    >> (pass() + (pink().seed(NOISE_SEED_BASE) * noise_mix));
  let mono = (driven | cutoff | q_val)
    >> ((moog() + (white().seed(HISS_SEED_BASE + NOISE_SEED_BASE) * hiss))
      * env)
    >> dcblock()
    >> shape(Crush(crush_levels))
    >> exact_hold_hz(ds_rate);
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
      reverb_mix: 0.0,
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
    assert_eq!(lowpass_cutoff(&patch, 44100.0), Some(0.0));
  }

  #[test]
  fn highpass_cutoff_bypasses_at_zero() {
    let mut patch = Patch::default();
    assert_eq!(highpass_cutoff(&patch, 44100.0), None);
    patch.highpass = 120.0;
    assert_eq!(highpass_cutoff(&patch, 44100.0), Some(120.0));
    patch.highpass = f64::NAN;
    assert_eq!(highpass_cutoff(&patch, 44100.0), None);
    patch.highpass = 30000.0;
    assert_eq!(highpass_cutoff(&patch, 44100.0), Some(rate_ceiling(44100.0)));
  }

  /// Render note_graph at various frequencies using the star-trek-ok patch
  /// shape and measure the peak amplitude in the final 256 samples (just before
  /// the mixer would hard-remove the slot).  Whatever is still sounding there
  /// comes from the echo feedback and the reverb tail, since the envelope has
  /// already silenced the oscillators.  The mixer's `remove()` method
  /// crossfades to silence over `REMOVE_FADEOUT_FRAMES` to mask a small
  /// residual, and this test documents which frequencies leave one.
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

  /// A driven patch with a sub-octave and no envelope movement, so that every
  /// control signal is constant once the note is sounding and two renders can
  /// be compared sample for sample.
  fn flat_patch() -> Patch {
    Patch {
      freq: 110.0,
      duration: 0.3,
      attack_ms: 0.0,
      decay_ms: 0.0,
      release_ms: 0.0,
      sine_ratio: 1.0,
      saw_ratio: 1.0,
      drive: 4.0,
      sub_octave: 0.6,
      reverb_mix: 0.0,
      ..Default::default()
    }
  }

  fn single(patch: Patch) -> Vec<ResolvedNote> {
    vec![ResolvedNote {
      patch,
      volume: 1.0,
      offset: 0.0,
    }]
  }

  /// Left channel of a heartbeat over `seconds`, from the start.
  fn left_channel(notes: &[ResolvedNote], seconds: f32) -> Vec<f32> {
    let mut graph = heartbeat_graph_with_notes(notes, None, 44100.0);
    graph.set_sample_rate(44100.0);
    graph.allocate();
    (0..(seconds * 44100.0) as usize)
      .map(|_| graph.get_stereo().0)
      .collect()
  }

  fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
  }

  /// RMS of the sample-wise difference between two renders, relative to the
  /// RMS of the first, over the body after the first 50 ms.
  fn relative_difference(a: &[f32], b: &[f32]) -> f32 {
    let from = 2205;
    let diff: Vec<f32> = a[from..]
      .iter()
      .zip(&b[from..])
      .map(|(x, y)| x - y)
      .collect();
    rms(&diff) / rms(&a[from..])
  }

  #[test]
  fn pitch_hz_applies_detune() {
    let up = Patch {
      freq: 440.0,
      detune: 100.0,
      ..Default::default()
    };
    assert!((pitch_hz(&up) - 440.0 * 2f32.powf(1.0 / 12.0)).abs() < 0.01);
    assert_eq!(pitch_hz(&Patch::default()), 440.0);
  }

  #[test]
  fn waveform_weights_normalise_then_shift_sine_towards_saw() {
    let even = Patch {
      sine_ratio: 1.0,
      saw_ratio: 1.0,
      ..Default::default()
    };
    assert_eq!(waveform_weights(&even), (0.5, 0.0, 0.5, 0.0));
    let harsh = Patch {
      harshness_offset: 1.0,
      ..even
    };
    assert_eq!(waveform_weights(&harsh), (0.0, 0.0, 1.5, 0.0));
    let soft = Patch {
      harshness_offset: -2.0,
      sine_ratio: 1.0,
      saw_ratio: 1.0,
      ..Default::default()
    };
    // The patch's hard limit holds the offset at -1, and a negative saw weight
    // floors at zero.
    assert_eq!(waveform_weights(&soft.limited().0), (1.0, 0.0, 0.0, 0.0));
  }

  #[test]
  fn later_note_does_not_change_the_first_note() {
    let alone = left_channel(&single(flat_patch()), 0.25);
    let with_later = [0.0, 5.0].map(|offset| ResolvedNote {
      patch: flat_patch(),
      volume: 1.0,
      offset,
    });
    let accompanied = left_channel(&with_later, 0.25);
    assert_eq!(alone, accompanied);
  }

  #[test]
  fn transparent_highpass_stage_does_not_change_the_timbre() {
    let plain = left_channel(&single(flat_patch()), 0.3);
    let staged = left_channel(
      &single(Patch {
        highpass: 0.5,
        ..flat_patch()
      }),
      0.3,
    );
    let difference = relative_difference(&plain, &staged);
    assert!(
      difference < 0.03,
      "a 0.5 Hz highpass stage changed the render by {:.1}% RMS",
      difference * 100.0
    );
  }

  #[test]
  fn sub_phase_changes_the_driven_timbre() {
    let aligned = left_channel(&single(flat_patch()), 0.3);
    let shifted = left_channel(
      &single(Patch {
        sub_phase: 0.5,
        ..flat_patch()
      }),
      0.3,
    );
    let difference = relative_difference(&aligned, &shifted);
    assert!(
      difference > 0.05,
      "sub_phase=0.5 changed the render by only {:.1}% RMS",
      difference * 100.0
    );
  }

  #[test]
  fn note_graph_output_has_no_dc_offset() {
    let body = &left_channel(
      &single(Patch {
        sine_ratio: 0.0,
        saw_ratio: 0.0,
        square_ratio: 1.0,
        drive: 6.0,
        sub_octave: 0.5,
        ..flat_patch()
      }),
      0.3,
    )[2205..];
    let mean = body.iter().sum::<f32>() / body.len() as f32;
    assert!(
      mean.abs() < 0.05 * rms(body),
      "mean {mean:.5} against rms {:.5}",
      rms(body)
    );
  }

  /// Peak of the reverb stage's response to a stereo impulse, per sample.
  fn reverb_impulse_response(mix: f32, seconds: f32) -> Vec<f32> {
    let mut stage = reverb_stage(mix).expect("a non-zero mix engages reverb");
    stage.set_sample_rate(44100.0);
    stage.allocate();
    let mut out = [0.0f32; 2];
    (0..(seconds * 44100.0) as usize)
      .map(|i| {
        let x = if i == 0 { 1.0 } else { 0.0 };
        stage.tick(&[x, x], &mut out);
        out[0].abs().max(out[1].abs())
      })
      .collect()
  }

  #[test]
  fn reverb_tail_decays_within_budget() {
    let response = reverb_impulse_response(1.0, 4.0);
    let loudest = response.iter().copied().fold(0.0f32, f32::max);
    let last_audible = response
      .iter()
      .rposition(|&y| y > loudest * 0.001)
      .map_or(0.0, |i| i as f64 / 44100.0);
    eprintln!("reverb tail reaches -60 dB after {last_audible:.2} s");
    assert!(
      last_audible <= REVERB_TAIL_SECS,
      "the tail ({last_audible:.2} s) outlasts REVERB_TAIL_SECS"
    );
    assert!(
      last_audible >= 0.5,
      "the tail ({last_audible:.2} s) is too short to be a hall"
    );
  }

  #[test]
  fn reverb_mix_preserves_sustained_loudness() {
    let tone = Patch {
      freq: 440.0,
      duration: 1.5,
      attack_ms: 0.0,
      decay_ms: 0.0,
      release_ms: 0.0,
      ..Default::default()
    };
    let dry = left_channel(
      &single(Patch {
        reverb_mix: 0.0,
        ..tone.clone()
      }),
      1.4,
    );
    let wet = left_channel(
      &single(Patch {
        reverb_mix: 1.0,
        ..tone
      }),
      1.4,
    );
    let window = 35280..61740;
    let ratio = rms(&wet[window.clone()]) / rms(&dry[window]);
    eprintln!("wet/dry sustained rms ratio {ratio:.3}");
    assert!(
      (0.7..1.4).contains(&ratio),
      "reverb_mix=1 changes sustained loudness by a factor of {ratio:.3}"
    );
  }

  #[test]
  fn reverb_mix_zero_is_a_true_bypass() {
    assert!(reverb_stage(0.0).is_none());
    assert!(reverb_stage(f32::NAN).is_none());
    assert!(reverb_stage(0.2).is_some());
  }

  #[test]
  fn notes_duration_includes_reverb_tail() {
    let base = Patch {
      duration: 1.0,
      attack_ms: 0.0,
      release_ms: 0.0,
      echo_mix: 0.0,
      ..Default::default()
    };
    let without = heartbeat_notes_duration(&single(Patch {
      reverb_mix: 0.0,
      ..base.clone()
    }));
    let with = heartbeat_notes_duration(&single(Patch {
      reverb_mix: 0.2,
      ..base
    }));
    assert!(
      (with.as_secs_f64() - without.as_secs_f64() - REVERB_TAIL_SECS).abs()
        < 1e-10
    );
  }

  #[test]
  fn an_unrepresentably_long_note_saturates_instead_of_panicking() {
    let notes = single(Patch {
      attack_ms: 1e25,
      ..Default::default()
    });
    assert_eq!(heartbeat_notes_duration(&notes), Duration::MAX);
    assert_eq!(heartbeat_notes_content_duration(&notes), Duration::MAX);
  }

  #[test]
  fn graph_entry_holds_a_hand_built_patch_to_its_hard_limits() {
    let at_the_limits = Patch {
      reverb_mix: 1.0,
      harshness_offset: -1.0,
      ..flat_patch()
    };
    let past_the_limits = Patch {
      reverb_mix: 1.5,
      harshness_offset: -3.0,
      ..flat_patch()
    };
    assert_eq!(
      left_channel(&single(at_the_limits), 0.1),
      left_channel(&single(past_the_limits), 0.1)
    );
  }

  #[test]
  fn spread_voice_ratio_is_unity_at_zero() {
    assert_eq!(spread_voice_ratio(0.0, 0), 1.0);
    assert_eq!(spread_voice_ratio(0.0, 1), 1.0);
    let lower = spread_voice_ratio(100.0, 0);
    let upper = spread_voice_ratio(100.0, 1);
    assert!((lower * upper - 1.0).abs() < 1e-6);
    assert!((upper - 2f32.powf(50.0 / 1200.0)).abs() < 1e-6);
  }

  #[test]
  fn eq_band_bypasses_at_zero_db() {
    let band = |patch: Patch| EqBand::from_patch(&patch, 44100.0);
    assert!(!band(Patch::default()).engaged());
    assert!(band(Patch::default()).stage().is_none());
    let boosted = band(Patch {
      eq_db: 6.0,
      ..Default::default()
    });
    assert!(boosted.engaged());
    assert!((boosted.gain - 1.995).abs() < 0.01);
    let high = band(Patch {
      eq_hz: 30000.0,
      ..Default::default()
    });
    assert_eq!(high.hz, rate_ceiling(44100.0));
  }

  #[test]
  fn eq_cut_attenuates_note_graph() {
    let tone = Patch {
      freq: 440.0,
      duration: 0.3,
      attack_ms: 10.0,
      release_ms: 50.0,
      reverb_mix: 0.0,
      eq_hz: 440.0,
      ..Default::default()
    };
    let plain = note_body_mean_square(&tone);
    let cut = note_body_mean_square(&Patch {
      eq_db: -24.0,
      ..tone.clone()
    });
    let boosted = note_body_mean_square(&Patch {
      eq_db: 12.0,
      ..tone
    });
    assert!(
      cut < plain * 0.25,
      "eq_db=-24 at the fundamental left {cut:.6} against {plain:.6}"
    );
    assert!(
      boosted > plain * 4.0,
      "eq_db=12 at the fundamental gave {boosted:.6} against {plain:.6}"
    );
  }

  #[test]
  fn envelope_curve_one_is_linear() {
    assert_eq!(envelope_curve(&Patch::default()), None);

    let (attack, decay, sustain, body, release) =
      (0.02f32, 0.05f32, 0.4f32, 0.1f32, 0.15f32);
    let adsr = Adsr {
      attack,
      decay,
      sustain,
      body,
      release,
      curve: None,
    };
    let plain = |t: f32| {
      if t < attack {
        t / attack
      } else if t < attack + decay {
        1.0 + (sustain - 1.0) * (t - attack) / decay
      } else if t <= attack + decay + body {
        sustain
      } else {
        (sustain * (attack + decay + body + release - t) / release).max(0.0)
      }
    };
    for i in 0..400 {
      let t = i as f32 * 0.001;
      assert_eq!(adsr.level(t).to_bits(), plain(t).to_bits(), "t = {t}");
    }
  }

  #[test]
  fn envelope_curve_above_one_delays_the_attack() {
    let straight = Adsr {
      attack: 0.1,
      decay: 0.0,
      sustain: 0.5,
      body: 0.2,
      release: 0.1,
      curve: None,
    };
    let bent = Adsr {
      curve: Some(2.0),
      ..straight
    };
    assert!((straight.level(0.05) - 0.5).abs() < 1e-6);
    assert!((bent.level(0.05) - 0.25).abs() < 1e-6);
    assert_eq!(bent.level(0.2), 0.5);
    // Halfway through the release the straight ramp is at half the sustain;
    // the bent one has already fallen to a quarter.
    assert!((straight.level(0.35) - 0.25).abs() < 1e-5);
    assert!((bent.level(0.35) - 0.125).abs() < 1e-5);
  }

  #[test]
  fn spread_produces_beating() {
    let tone = Patch {
      freq: 440.0,
      duration: 0.5,
      attack_ms: 0.0,
      decay_ms: 0.0,
      release_ms: 0.0,
      reverb_mix: 0.0,
      ..Default::default()
    };
    // Peak of each 10 ms window after the first five, in which the ladder and
    // the DC blocker settle.  Spread 20 at 440 Hz beats at 5.08 Hz, so a 10 ms
    // window near a null peaks well under the loudest one; a 50 ms window
    // would not, because a null can sit at a window's edge.
    let window_peaks = |patch: Patch| -> Vec<f32> {
      left_channel(&single(patch), 0.45)
        .chunks(441)
        .skip(5)
        .map(|window| window.iter().fold(0.0f32, |m, s| m.max(s.abs())))
        .collect()
    };
    let swing = |peaks: &[f32]| {
      let min = peaks.iter().copied().fold(f32::MAX, f32::min);
      let max = peaks.iter().copied().fold(0.0f32, f32::max);
      min / max
    };
    let steady = swing(&window_peaks(tone.clone()));
    let beating = swing(&window_peaks(Patch {
      spread: 20.0,
      ..tone
    }));
    assert!(steady > 0.9, "a single voice swung to {steady:.3}");
    assert!(beating < 0.5, "spread=20 swung only to {beating:.3}");
  }

  #[test]
  fn hiss_passes_a_dark_ladder() {
    let dark = Patch {
      freq: 440.0,
      duration: 0.3,
      attack_ms: 0.0,
      release_ms: 0.0,
      reverb_mix: 0.0,
      brightness: 0.08,
      highpass: 2000.0,
      ..Default::default()
    };
    let floor = note_body_mean_square(&dark);
    let hissing = note_body_mean_square(&Patch {
      hiss: 0.3,
      ..dark.clone()
    });
    let breathy = note_body_mean_square(&Patch {
      noise_mix: 0.3,
      ..dark
    });
    assert!(
      hissing > floor * 10.0,
      "hiss=0.3 above a 2 kHz highpass gave {hissing:.6} against a floor of \
       {floor:.6}"
    );
    assert!(
      breathy < floor * 2.0,
      "noise_mix=0.3 under a dark ladder gave {breathy:.6} against a floor \
       of {floor:.6}"
    );
  }
}
