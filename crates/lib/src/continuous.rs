use crate::downsample::exact_hold_hz;
use crate::heartbeat::{
  cutoff_ceiling, highpass_cutoff, lowpass_cutoff, oscillator_bank, pitch_hz,
  reverb_stage, spread_voice_ratio, waveform_weights, EqBand, HISS_SEED_BASE,
  OSCILLATOR_PHASE,
};
use crate::patch::Patch;
use fundsp::net::Net;
use fundsp::prelude32::{
  bell, busi, dc, dcblock, delay, feedback, follow, highpass_q, lfo, lowpass_q,
  map, moog, pan, pass, pink, shared, var, white, An, AudioNode, AudioUnit,
  Frame, U0, U1, U2,
};
use fundsp::shared::Shared;

/// Dynamically morphable parameters driven by `Shared` controls.
/// The audio graph reads these via `var() >> follow()`, so changes
/// propagate smoothly at the audio sample rate.  Parameters that
/// only shape the finite ADSR envelope are omitted -- the continuous
/// graph sustains indefinitely.
pub struct ContinuousControls {
  pub freq: Shared,
  pub sine_w: Shared,
  pub tri_w: Shared,
  pub saw_w: Shared,
  pub square_w: Shared,
  pub sub_octave: Shared,
  pub vibrato_rate: Shared,
  pub vibrato_depth: Shared,
  pub tremolo_rate: Shared,
  pub tremolo_depth: Shared,
  pub fm_ratio: Shared,
  pub fm_depth: Shared,
  pub filter_cutoff: Shared,
  pub filter_q: Shared,
  /// Effective highpass cutoff in Hz.  Read only while the stage is engaged;
  /// holds a transparent floor while bypassed (see `highpass_shared_value`).
  pub highpass: Shared,
  /// Effective lowpass cutoff in Hz.  Read only while the stage is engaged;
  /// holds the stable ceiling while bypassed (see `lowpass_shared_value`).
  pub lowpass: Shared,
  pub amplitude: Shared,
  pub noise_mix: Shared,
  pub drive: Shared,
  pub crush: Shared,
  /// Spread between the two main voices in cents.
  pub spread: Shared,
  pub hiss: Shared,
  pub eq_hz: Shared,
  pub eq_q: Shared,
  /// Linear amplitude gain of the EQ band, the form fundsp's bell takes.
  pub eq_gain: Shared,
}

impl ContinuousControls {
  /// Initialize all `Shared` values from a patch snapshot, held to its hard
  /// limits.
  pub fn from_patch(patch: &Patch, sample_rate: f64) -> Self {
    let patch = &patch.limited().0;
    let (sine_w, tri_w, saw_w, square_w) = waveform_weights(patch);
    let (cutoff, q) = filter_params(patch, sample_rate);
    let eq = EqBand::from_patch(patch, sample_rate);

    ContinuousControls {
      freq: shared(pitch_hz(patch)),
      sine_w: shared(sine_w),
      tri_w: shared(tri_w),
      saw_w: shared(saw_w),
      square_w: shared(square_w),
      sub_octave: shared(patch.sub_octave as f32),
      vibrato_rate: shared(patch.vibrato_rate as f32),
      vibrato_depth: shared(patch.vibrato_depth as f32),
      tremolo_rate: shared(patch.tremolo_rate as f32),
      tremolo_depth: shared(patch.tremolo_depth as f32),
      fm_ratio: shared(patch.fm_ratio as f32),
      fm_depth: shared(patch.fm_depth as f32),
      filter_cutoff: shared(cutoff),
      filter_q: shared(q),
      highpass: shared(highpass_shared_value(patch, sample_rate)),
      lowpass: shared(lowpass_shared_value(patch, sample_rate)),
      amplitude: shared(patch.amplitude as f32),
      noise_mix: shared(patch.noise_mix as f32),
      drive: shared(patch.drive as f32),
      crush: shared(patch.crush as f32),
      spread: shared(patch.spread as f32),
      hiss: shared(patch.hiss as f32),
      eq_hz: shared(eq.hz),
      eq_q: shared(eq.q),
      eq_gain: shared(eq.gain),
    }
  }

  /// Write new values into all `Shared` controls.  The graph's
  /// `follow()` nodes smooth the transition at the audio rate.  The patch is
  /// held to its hard limits first.
  pub fn update_from_patch(&self, patch: &Patch, sample_rate: f64) {
    let patch = &patch.limited().0;
    let (sine_w, tri_w, saw_w, square_w) = waveform_weights(patch);
    let (cutoff, q) = filter_params(patch, sample_rate);
    let eq = EqBand::from_patch(patch, sample_rate);

    self.freq.set_value(pitch_hz(patch));
    self.sine_w.set_value(sine_w);
    self.tri_w.set_value(tri_w);
    self.saw_w.set_value(saw_w);
    self.square_w.set_value(square_w);
    self.sub_octave.set_value(patch.sub_octave as f32);
    self.vibrato_rate.set_value(patch.vibrato_rate as f32);
    self.vibrato_depth.set_value(patch.vibrato_depth as f32);
    self.tremolo_rate.set_value(patch.tremolo_rate as f32);
    self.tremolo_depth.set_value(patch.tremolo_depth as f32);
    self.fm_ratio.set_value(patch.fm_ratio as f32);
    self.fm_depth.set_value(patch.fm_depth as f32);
    self.filter_cutoff.set_value(cutoff);
    self.filter_q.set_value(q);
    self
      .highpass
      .set_value(highpass_shared_value(patch, sample_rate));
    self
      .lowpass
      .set_value(lowpass_shared_value(patch, sample_rate));
    self.amplitude.set_value(patch.amplitude as f32);
    self.noise_mix.set_value(patch.noise_mix as f32);
    self.drive.set_value(patch.drive as f32);
    self.crush.set_value(patch.crush as f32);
    self.spread.set_value(patch.spread as f32);
    self.hiss.set_value(patch.hiss as f32);
    self.eq_hz.set_value(eq.hz);
    self.eq_q.set_value(eq.q);
    self.eq_gain.set_value(eq.gain);
  }
}

/// Derive filter cutoff and Q from pitch, brightness, and resonance.
fn filter_params(patch: &Patch, sample_rate: f64) -> (f32, f32) {
  let cutoff = (pitch_hz(patch) * 13.0 * patch.brightness as f32)
    .min(cutoff_ceiling(sample_rate));
  let q = 0.5 * patch.resonance as f32;
  (cutoff, q)
}

/// Value for the highpass `Shared`: the effective cutoff, or a transparent 1 Hz
/// floor while the stage is bypassed.  The floor only sounds during the poll
/// interval between a live edit crossing the bypass boundary and the structural
/// rebuild that removes the stage.
fn highpass_shared_value(patch: &Patch, sample_rate: f64) -> f32 {
  highpass_cutoff(patch, sample_rate).unwrap_or(1.0)
}

/// Value for the lowpass `Shared`: the effective cutoff, or the stable ceiling
/// while the stage is bypassed (same transitional window as
/// `highpass_shared_value`).
fn lowpass_shared_value(patch: &Patch, sample_rate: f64) -> f32 {
  lowpass_cutoff(patch, sample_rate)
    .unwrap_or_else(|| cutoff_ceiling(sample_rate))
}

/// Parameters baked into the graph topology that require a rebuild
/// to change.  Derives `PartialEq` so the daemon can detect when a
/// rebuild is necessary.
#[derive(Clone, Debug, PartialEq)]
pub struct StructuralParams {
  pub echo_delay: f32,
  pub echo_mix: f32,
  pub reverb_mix: f32,
  pub stereo_pan: f32,
  pub downsample: f32,
  /// An oscillator's start phase is fixed when it is built, so a change
  /// needs a rebuild.
  pub sub_phase: f32,
  /// Filter stages exist or don't in the topology, so crossing a bypass
  /// boundary is a structural change.
  pub highpass_bypassed: bool,
  pub lowpass_bypassed: bool,
}

impl StructuralParams {
  /// The structural parameters of a patch held to its hard limits.
  pub fn from_patch(patch: &Patch, sample_rate: f64) -> Self {
    let patch = &patch.limited().0;
    StructuralParams {
      echo_delay: patch.echo_delay as f32,
      echo_mix: patch.echo_mix as f32,
      reverb_mix: patch.reverb_mix as f32,
      stereo_pan: patch.stereo_pan as f32,
      downsample: patch.downsample as f32,
      sub_phase: patch.sub_phase as f32,
      highpass_bypassed: highpass_cutoff(patch, sample_rate).is_none(),
      lowpass_bypassed: lowpass_cutoff(patch, sample_rate).is_none(),
    }
  }
}

/// Pitch in Hz, scaled by `octave_scale`, with vibrato and FM applied.  The
/// `Shared`s are read inside the lfo so an edit takes effect at once.
fn modulated_pitch(
  controls: &ContinuousControls,
  octave_scale: f32,
) -> An<impl AudioNode<Inputs = U0, Outputs = U1>> {
  let freq = controls.freq.clone();
  let vib_rate = controls.vibrato_rate.clone();
  let vib_depth = controls.vibrato_depth.clone();
  let fm_ratio = controls.fm_ratio.clone();
  let fm_depth = controls.fm_depth.clone();
  lfo(move |t: f32| {
    let base = freq.value() * octave_scale;
    let vib = 2f64.powf(
      vib_depth.value() as f64
        * (std::f64::consts::TAU * vib_rate.value() as f64 * t as f64).sin()
        / 12.0,
    ) as f32;
    let fm_freq = base * fm_ratio.value();
    let fm_mod =
      fm_depth.value() * fm_freq * (std::f32::consts::TAU * fm_freq * t).sin();
    (base * vib + fm_mod).max(0.01)
  })
}

/// Build a continuously sustaining audio graph driven by `Shared`
/// controls.  The signal chain mirrors `heartbeat_graph` but
/// replaces time-based envelopes with `var() >> follow()` smoothers
/// and removes the finite attack/sustain/release envelope.
///
/// `smoothing_secs` sets the `follow()` time constant — larger
/// values give slower, smoother morphs.
pub fn continuous_graph(
  controls: &ContinuousControls,
  smoothing_secs: f64,
  structural: &StructuralParams,
  external_volume: Option<&Shared>,
  noise_seed: u64,
) -> Box<dyn AudioUnit> {
  let smooth = smoothing_secs as f32;
  let weights = || {
    (
      var(&controls.sine_w) >> follow(smooth),
      var(&controls.tri_w) >> follow(smooth),
      var(&controls.saw_w) >> follow(smooth),
      var(&controls.square_w) >> follow(smooth),
    )
  };

  // Tremolo amplitude modulation via lfo.
  let trem_rate = controls.tremolo_rate.clone();
  let trem_depth = controls.tremolo_depth.clone();
  let trem_mod = lfo(move |t: f32| {
    let rate = trem_rate.value() as f64;
    let depth = trem_depth.value() as f64;
    (1.0
      - depth * (1.0 - (std::f64::consts::TAU * rate * t as f64).sin()) / 2.0)
      as f32
  });

  // Two copies of the main bank at half weight, a half spread below and above
  // the pitch.  The ratio multiplies the modulated pitch signal, so the
  // modulation rides along and a live edit sweeps through the smoother.
  let main_osc = modulated_pitch(controls, 1.0)
    >> busi::<U2, _, _>(|voice| {
      let ratio = var(&controls.spread)
        >> follow(smooth)
        >> map(move |c: &Frame<f32, U1>| spread_voice_ratio(c[0], voice));
      (pass() * ratio) >> (oscillator_bank(OSCILLATOR_PHASE, weights()) * 0.5)
    });
  let sub_mix = var(&controls.sub_octave) >> follow(smooth);
  let sub_osc = (modulated_pitch(controls, 0.5)
    >> oscillator_bank(OSCILLATOR_PHASE + structural.sub_phase, weights()))
    * sub_mix;

  // Drive via map() closure reading Shared, since shape(Tanh(..))
  // bakes the drive value at construction.  The Shared only ever holds a
  // limited drive, which stays above zero.
  let drive_s = controls.drive.clone();
  let drive_map = map(move |x: &Frame<f32, U1>| {
    let d = drive_s.value();
    (x[0] * d).tanh() / d.tanh()
  });

  // Noise mix.
  let noise_mix_smooth = var(&controls.noise_mix) >> follow(smooth);
  let hiss_smooth = var(&controls.hiss) >> follow(smooth);

  // A bell at unity gain is an exact passthrough, so the EQ band is always
  // present and reads its smoothed controls rather than being a structural
  // stage whose engagement would need a rebuild.
  let eq_stage = (pass()
    | (var(&controls.eq_hz) >> follow(smooth))
    | (var(&controls.eq_q) >> follow(smooth))
    | (var(&controls.eq_gain) >> follow(smooth)))
    >> bell();

  // Filter.
  let cutoff_smooth = var(&controls.filter_cutoff) >> follow(smooth);
  let q_smooth = var(&controls.filter_q) >> (follow(smooth) * 0.2);

  // Amplitude.
  let amp_smooth = var(&controls.amplitude) >> follow(smooth);

  // External volume (master × mute).
  let ext = external_volume.map_or_else(|| shared(1.0), |s| s.clone());
  let ext_vol = var(&ext) >> follow(0.1);

  // Bitcrush via map() closure reading Shared.
  let crush_s = controls.crush.clone();
  let crush_map = map(move |x: &Frame<f32, U1>| {
    let c = crush_s.value();
    let levels = 2.0_f32.powf(1.0 + 15.0 * (1.0 - c));
    (x[0] * levels).round() / levels
  });

  // Downsample rate is structural — baked at build time.
  let ds_rate = 100_000.0_f32 / 2.0_f32.powf(structural.downsample * 8.0);

  // Assemble signal chain (same topology as heartbeat_graph).
  let signal = (main_osc + sub_osc)
    >> drive_map
    >> (pass() + (pink().seed(noise_seed) * noise_mix_smooth));
  let mono = (signal | cutoff_smooth | q_smooth)
    >> ((moog() + (white().seed(HISS_SEED_BASE + noise_seed) * hiss_smooth))
      * amp_smooth
      * trem_mod
      * ext_vol)
    >> dcblock()
    >> crush_map
    >> exact_hold_hz(ds_rate)
    >> eq_stage;
  // Filter stages are topological: a bypassed filter contributes no node (see
  // `StructuralParams`), and an engaged one reads its smoothed Shared cutoff so
  // live edits sweep.  `follow()` snaps on its first sample, so a fresh graph
  // starts at the right value.
  let filtered = [
    (!structural.highpass_bypassed).then(|| {
      let hp_smooth = var(&controls.highpass) >> follow(smooth);
      Net::wrap(Box::new((pass() | hp_smooth) >> highpass_q(0.7)))
    }),
    (!structural.lowpass_bypassed).then(|| {
      let lp_smooth = var(&controls.lowpass) >> follow(smooth);
      Net::wrap(Box::new((pass() | lp_smooth) >> lowpass_q(0.7)))
    }),
  ]
  .into_iter()
  .flatten()
  .fold(Net::wrap(Box::new(mono)), |acc, stage| acc >> stage);
  let tail = (pass()
    & (feedback(delay(structural.echo_delay) * 0.3) * structural.echo_mix))
    >> pan(structural.stereo_pan);

  Box::new(
    reverb_stage(structural.reverb_mix)
      .into_iter()
      .fold(filtered >> Net::wrap(Box::new(tail)), |acc, stage| acc >> stage),
  )
}

/// Multi-note continuous graph: one independent `continuous_graph`
/// per note, with per-note volume baked into the amplitude control,
/// summed via `Net::wrap` + `reduce`.
///
/// Returns a tuple of `(graph, controls_vec, structural_vec)` so
/// the caller can update controls and detect structural changes
/// per-note.
pub fn continuous_graph_with_notes(
  patches: &[(Patch, f64)],
  smoothing_secs: f64,
  external_volume: Option<&Shared>,
  sample_rate: f64,
) -> (Box<dyn AudioUnit>, Vec<ContinuousControls>, Vec<StructuralParams>) {
  let mut all_controls = Vec::with_capacity(patches.len());
  let mut all_structural = Vec::with_capacity(patches.len());

  let mut iter = patches.iter().enumerate().map(|(index, (patch, volume))| {
    let mut p = patch.limited().0;
    p.amplitude *= *volume;
    let controls = ContinuousControls::from_patch(&p, sample_rate);
    let structural = StructuralParams::from_patch(&p, sample_rate);
    let graph = continuous_graph(
      &controls,
      smoothing_secs,
      &structural,
      external_volume,
      crate::heartbeat::NOISE_SEED_BASE + index as u64,
    );
    all_controls.push(controls);
    all_structural.push(structural);
    Net::wrap(graph)
  });

  let Some(first) = iter.next() else {
    // Empty patches — return silence with matching external-volume
    // wiring so the consumer's I/O shape is identical to the
    // populated case.  The control/structural Vecs come back
    // empty because nothing was iterated.
    let ext = external_volume.map_or_else(|| shared(1.0), |s| s.clone());
    return (
      Box::new((dc(0.0) * var(&ext)) | (dc(0.0) * var(&ext))),
      Vec::new(),
      Vec::new(),
    );
  };
  let net = iter.fold(first, |acc, n| acc + n);

  (Box::new(net), all_controls, all_structural)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn continuous_graph_produces_sound() {
    let patch = Patch::default();
    let controls = ContinuousControls::from_patch(&patch, 44100.0);
    let structural = StructuralParams::from_patch(&patch, 44100.0);
    let mut graph = continuous_graph(
      &controls,
      0.5,
      &structural,
      None,
      crate::heartbeat::NOISE_SEED_BASE,
    );
    graph.set_sample_rate(44100.0);
    graph.allocate();

    let mut peak: f32 = 0.0;
    for _ in 0..44100 {
      let (l, r) = graph.get_stereo();
      peak = peak.max(l.abs()).max(r.abs());
    }
    assert!(
      peak > 0.001,
      "Continuous graph should produce audible samples, got peak {peak}"
    );
  }

  #[test]
  fn update_from_patch_changes_controls() {
    let lo = Patch {
      freq: 200.0,
      amplitude: 0.2,
      ..Default::default()
    };
    let hi = Patch {
      freq: 800.0,
      amplitude: 0.8,
      highpass: 120.0,
      lowpass: 4000.0,
      spread: 20.0,
      hiss: 0.3,
      eq_hz: 300.0,
      eq_db: 6.0,
      eq_q: 2.0,
      ..Default::default()
    };
    let controls = ContinuousControls::from_patch(&lo, 44100.0);
    assert!((controls.freq.value() - 200.0).abs() < 0.01);
    // Default patch: both filters are bypassed, so the Shareds hold their
    // transitional values — the 1 Hz highpass floor and the lowpass stable
    // ceiling.
    assert!((controls.highpass.value() - 1.0).abs() < 0.01);
    assert!((controls.lowpass.value() - cutoff_ceiling(44100.0)).abs() < 0.01);

    controls.update_from_patch(&hi, 44100.0);
    assert!((controls.freq.value() - 800.0).abs() < 0.01);
    assert!((controls.amplitude.value() - 0.8).abs() < 0.01);
    assert!((controls.highpass.value() - 120.0).abs() < 0.01);
    assert!((controls.lowpass.value() - 4000.0).abs() < 0.01);
    assert!((controls.spread.value() - 20.0).abs() < 0.01);
    assert!((controls.hiss.value() - 0.3).abs() < 0.01);
    assert!((controls.eq_hz.value() - 300.0).abs() < 0.01);
    assert!((controls.eq_q.value() - 2.0).abs() < 0.01);
    assert!((controls.eq_gain.value() - 1.995).abs() < 0.01);
  }

  /// Render half a second of a graph after a quarter-second warmup and return
  /// the mean square of the left channel.  The warmup exists for the reverb
  /// tail to build up — `follow()` needs none, since it snaps to the Shared's
  /// value on its first sample.
  fn rendered_mean_square(patch: &Patch) -> f32 {
    let controls = ContinuousControls::from_patch(patch, 44100.0);
    let structural = StructuralParams::from_patch(patch, 44100.0);
    let mut graph = continuous_graph(
      &controls,
      0.05,
      &structural,
      None,
      crate::heartbeat::NOISE_SEED_BASE,
    );
    graph.set_sample_rate(44100.0);
    graph.allocate();

    for _ in 0..11025 {
      graph.get_stereo();
    }
    (0..22050)
      .map(|_| {
        let (l, _) = graph.get_stereo();
        l * l
      })
      .sum::<f32>()
      / 22050.0
  }

  #[test]
  fn continuous_lowpass_attenuates() {
    let bright = Patch {
      freq: 880.0,
      sine_ratio: 0.0,
      saw_ratio: 1.0,
      ..Default::default()
    };
    let dark = Patch {
      lowpass: 150.0,
      ..bright.clone()
    };
    let bright_ms = rendered_mean_square(&bright);
    let dark_ms = rendered_mean_square(&dark);
    assert!(
      dark_ms < bright_ms * 0.5,
      "lowpass=150 mean square ({dark_ms:.6}) should be well below \
       the unfiltered mean square ({bright_ms:.6})"
    );
  }

  #[test]
  fn continuous_highpass_attenuates() {
    let low_tone = Patch {
      freq: 200.0,
      ..Default::default()
    };
    let thin = Patch {
      highpass: 2000.0,
      ..low_tone.clone()
    };
    let full_ms = rendered_mean_square(&low_tone);
    let thin_ms = rendered_mean_square(&thin);
    assert!(
      thin_ms < full_ms * 0.5,
      "highpass=2000 mean square ({thin_ms:.6}) should be well below \
       the unfiltered mean square ({full_ms:.6})"
    );
  }

  #[test]
  fn structural_params_detect_change() {
    let lo = Patch {
      echo_delay: 0.25,
      reverb_mix: 0.2,
      ..Default::default()
    };
    let hi = Patch {
      echo_delay: 0.5,
      reverb_mix: 0.2,
      ..Default::default()
    };
    let a = StructuralParams::from_patch(&lo, 44100.0);
    let b = StructuralParams::from_patch(&hi, 44100.0);
    assert_ne!(a, b);

    let c = StructuralParams::from_patch(&lo, 44100.0);
    assert_eq!(a, c);

    // Crossing a filter's bypass boundary is a structural change.
    let engaged = Patch {
      lowpass: 5000.0,
      ..lo.clone()
    };
    let d = StructuralParams::from_patch(&engaged, 44100.0);
    assert!(a.lowpass_bypassed);
    assert!(!d.lowpass_bypassed);
    assert_ne!(a, d);

    // A start phase is baked into the oscillator at build time.
    let turned = Patch {
      sub_phase: 0.25,
      ..lo.clone()
    };
    assert_ne!(a, StructuralParams::from_patch(&turned, 44100.0));

    // Spread, hiss, and the EQ band read smoothed controls, so editing them
    // needs no rebuild.
    let smoothed = Patch {
      spread: 20.0,
      hiss: 0.3,
      eq_hz: 300.0,
      eq_db: 6.0,
      eq_q: 2.0,
      ..lo.clone()
    };
    assert_eq!(a, StructuralParams::from_patch(&smoothed, 44100.0));
  }

  #[test]
  fn controls_apply_detune_and_harshness() {
    let patch = Patch {
      freq: 200.0,
      detune: 100.0,
      sine_ratio: 1.0,
      saw_ratio: 1.0,
      harshness_offset: 1.0,
      ..Default::default()
    };
    let controls = ContinuousControls::from_patch(&patch, 44100.0);
    assert!(
      (controls.freq.value() - 200.0 * 2f32.powf(1.0 / 12.0)).abs() < 0.01
    );
    assert_eq!(controls.sine_w.value(), 0.0);
    assert_eq!(controls.saw_w.value(), 1.5);
    assert!(
      (controls.filter_cutoff.value() - controls.freq.value() * 13.0).abs()
        < 0.01
    );
  }

  /// First `count` values of a control-rate pitch node, sampled every 10 ms.
  fn pitch_samples(
    pitch: &mut An<impl AudioNode<Inputs = U0, Outputs = U1>>,
    count: usize,
  ) -> Vec<f32> {
    pitch.set_sample_rate(44100.0);
    pitch.reset();
    (0..count)
      .map(|_| (0..441).map(|_| pitch.get_mono()).last().unwrap_or(0.0))
      .collect()
  }

  #[test]
  fn sub_oscillator_pitch_follows_vibrato() {
    let steady = ContinuousControls::from_patch(
      &Patch {
        freq: 400.0,
        ..Default::default()
      },
      44100.0,
    );
    let flat = pitch_samples(&mut modulated_pitch(&steady, 0.5), 20);
    assert!(flat.iter().all(|&hz| (hz - 200.0).abs() < 0.01), "{flat:?}");

    let wobbling = ContinuousControls::from_patch(
      &Patch {
        freq: 400.0,
        vibrato_rate: 5.0,
        vibrato_depth: 12.0,
        ..Default::default()
      },
      44100.0,
    );
    let moving = pitch_samples(&mut modulated_pitch(&wobbling, 0.5), 20);
    let lowest = moving.iter().copied().fold(f32::MAX, f32::min);
    let highest = moving.iter().copied().fold(0.0f32, f32::max);
    assert!(lowest < 150.0 && highest > 250.0, "{moving:?}");
  }

  /// Left channel of a continuous graph over `seconds`, after a 0.1 s
  /// settling period.
  fn settled_left_channel(patch: &Patch, seconds: f32) -> Vec<f32> {
    let controls = ContinuousControls::from_patch(patch, 44100.0);
    let structural = StructuralParams::from_patch(patch, 44100.0);
    let mut graph = continuous_graph(
      &controls,
      0.05,
      &structural,
      None,
      crate::heartbeat::NOISE_SEED_BASE,
    );
    graph.set_sample_rate(44100.0);
    graph.allocate();
    (0..4410).for_each(|_| {
      graph.get_stereo();
    });
    (0..(seconds * 44100.0) as usize)
      .map(|_| graph.get_stereo().0)
      .collect()
  }

  #[test]
  fn transparent_highpass_stage_does_not_change_the_timbre() {
    let driven = Patch {
      freq: 110.0,
      sine_ratio: 1.0,
      saw_ratio: 1.0,
      drive: 4.0,
      sub_octave: 0.6,
      reverb_mix: 0.0,
      ..Default::default()
    };
    let plain = settled_left_channel(&driven, 0.25);
    let staged = settled_left_channel(
      &Patch {
        highpass: 0.5,
        ..driven
      },
      0.25,
    );
    let rms = |s: &[f32]| {
      (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt()
    };
    let diff: Vec<f32> =
      plain.iter().zip(&staged).map(|(a, b)| a - b).collect();
    let difference = rms(&diff) / rms(&plain);
    assert!(
      difference < 0.03,
      "a 0.5 Hz highpass stage changed the render by {:.1}% RMS",
      difference * 100.0
    );
  }

  #[test]
  fn continuous_eq_cut_attenuates() {
    let tone = Patch {
      freq: 440.0,
      reverb_mix: 0.0,
      eq_hz: 440.0,
      ..Default::default()
    };
    let plain = rendered_mean_square(&tone);
    let cut = rendered_mean_square(&Patch {
      eq_db: -24.0,
      ..tone.clone()
    });
    let boosted = rendered_mean_square(&Patch {
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
  fn continuous_eq_at_zero_db_is_transparent() {
    let plain = settled_left_channel(&Patch::default(), 0.25);
    let parked = settled_left_channel(
      &Patch {
        eq_hz: 300.0,
        eq_q: 5.0,
        ..Default::default()
      },
      0.25,
    );
    // The band's `v0 + 0 * v1` turns a -0.0 into +0.0, which is not a change
    // in the signal, so the renders are compared with `==` rather than
    // `to_bits`.
    assert!(plain == parked, "a 0 dB band changed the render");
  }

  #[test]
  fn continuous_spread_changes_output() {
    let tone = Patch {
      freq: 440.0,
      reverb_mix: 0.0,
      ..Default::default()
    };
    let one_voice = settled_left_channel(&tone, 0.25);
    let two_voices = settled_left_channel(
      &Patch {
        spread: 20.0,
        ..tone
      },
      0.25,
    );
    let rms = |s: &[f32]| {
      (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt()
    };
    let diff: Vec<f32> = one_voice
      .iter()
      .zip(&two_voices)
      .map(|(a, b)| a - b)
      .collect();
    let difference = rms(&diff) / rms(&one_voice);
    assert!(
      difference > 0.1,
      "spread=20 changed the render by only {:.1}% RMS",
      difference * 100.0
    );
  }

  #[test]
  fn continuous_hiss_passes_a_dark_ladder() {
    let dark = Patch {
      freq: 440.0,
      reverb_mix: 0.0,
      brightness: 0.08,
      highpass: 2000.0,
      ..Default::default()
    };
    let floor = rendered_mean_square(&dark);
    let hissing = rendered_mean_square(&Patch {
      hiss: 0.3,
      ..dark.clone()
    });
    let breathy = rendered_mean_square(&Patch {
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
