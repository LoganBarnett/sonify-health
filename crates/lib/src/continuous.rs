use crate::heartbeat::MAX_CUTOFF;
use crate::patch::Patch;
use fundsp::net::Net;
use fundsp::prelude32::*;
use fundsp::shared::Shared;

/// Dynamically morphable parameters driven by `Shared` controls.
/// The audio graph reads these via `var() >> follow()`, so changes
/// propagate smoothly at the audio sample rate.  Parameters that
/// only matter for finite envelopes (attack, release, sustain,
/// duration, chirp_ratio) are omitted — the continuous graph
/// sustains indefinitely.
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
  pub amplitude: Shared,
  pub noise_mix: Shared,
  pub drive: Shared,
  pub crush: Shared,
}

impl ContinuousControls {
  /// Initialize all `Shared` values from a patch snapshot.
  pub fn from_patch(patch: &Patch) -> Self {
    let (sine_w, tri_w, saw_w, square_w) = normalized_weights(patch);
    let (cutoff, q) = filter_params(patch);

    ContinuousControls {
      freq: shared(patch.freq as f32),
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
      amplitude: shared(patch.amplitude as f32),
      noise_mix: shared(patch.noise_mix as f32),
      drive: shared(patch.drive as f32),
      crush: shared(patch.crush as f32),
    }
  }

  /// Write new values into all `Shared` controls.  The graph's
  /// `follow()` nodes smooth the transition at the audio rate.
  pub fn update_from_patch(&self, patch: &Patch) {
    let (sine_w, tri_w, saw_w, square_w) = normalized_weights(patch);
    let (cutoff, q) = filter_params(patch);

    self.freq.set_value(patch.freq as f32);
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
    self.amplitude.set_value(patch.amplitude as f32);
    self.noise_mix.set_value(patch.noise_mix as f32);
    self.drive.set_value(patch.drive as f32);
    self.crush.set_value(patch.crush as f32);
  }
}

/// Normalize waveform ratio weights so they sum to 1.0.
fn normalized_weights(patch: &Patch) -> (f32, f32, f32, f32) {
  let total =
    patch.sine_ratio + patch.tri_ratio + patch.saw_ratio + patch.square_ratio;
  let norm = if total > 0.0 { 1.0 / total } else { 1.0 } as f32;
  (
    patch.sine_ratio as f32 * norm,
    patch.tri_ratio as f32 * norm,
    patch.saw_ratio as f32 * norm,
    patch.square_ratio as f32 * norm,
  )
}

/// Derive filter cutoff and Q from frequency, brightness, and resonance.
fn filter_params(patch: &Patch) -> (f32, f32) {
  let cutoff =
    (patch.freq as f32 * 13.0 * patch.brightness as f32).min(MAX_CUTOFF);
  let q = 0.5 * patch.resonance as f32;
  (cutoff, q)
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
}

impl StructuralParams {
  pub fn from_patch(patch: &Patch) -> Self {
    StructuralParams {
      echo_delay: patch.echo_delay as f32,
      echo_mix: patch.echo_mix as f32,
      reverb_mix: patch.reverb_mix as f32,
      stereo_pan: patch.stereo_pan as f32,
      downsample: patch.downsample as f32,
    }
  }
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
) -> Box<dyn AudioUnit> {
  let smooth = smoothing_secs as f32;

  // Smoothed waveform weights.
  let sine_w_smooth = var(&controls.sine_w) >> follow(smooth);
  let tri_w_smooth = var(&controls.tri_w) >> follow(smooth);
  let saw_w_smooth = var(&controls.saw_w) >> follow(smooth);
  let square_w_smooth = var(&controls.square_w) >> follow(smooth);

  // Modulation parameters read directly in lfo closures — they
  // oscillate already, so additional follow() smoothing is
  // unnecessary.
  let vib_rate = controls.vibrato_rate.clone();
  let vib_depth = controls.vibrato_depth.clone();
  let fm_ratio_s = controls.fm_ratio.clone();
  let fm_depth_s = controls.fm_depth.clone();

  // Frequency modulation via vibrato and FM synthesis.  The lfo
  // reads the current Shared values each sample so changes appear
  // immediately in the modulation.
  let freq_for_lfo = controls.freq.clone();
  let freq_mod = lfo(move |t: f32| {
    let base = freq_for_lfo.value();
    let vr = vib_rate.value();
    let vd = vib_depth.value() as f64;
    let vib = 2f64
      .powf(vd * (std::f64::consts::TAU * vr as f64 * t as f64).sin() / 12.0)
      as f32;
    let fmr = fm_ratio_s.value();
    let fmd = fm_depth_s.value();
    let fm_freq = base * fmr;
    let fm_mod = fmd * fm_freq * (std::f32::consts::TAU * fm_freq * t).sin();
    (base * vib + fm_mod).max(0.01)
  });

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

  // Main waveform: four oscillators weighted by smoothed Shared
  // values.  `freq_mod` drives oscillator pitch with vibrato/FM
  // baked in.
  let waveform = (sine() * sine_w_smooth)
    & (triangle() * tri_w_smooth)
    & (saw() * saw_w_smooth)
    & (square() * square_w_smooth);
  let main_osc = freq_mod >> waveform;

  // Sub-oscillator at half frequency.
  let sub_freq_smooth = var(&controls.freq)
    >> follow(smooth)
    >> map(|f: &Frame<f32, U1>| f[0] * 0.5);
  let sub_sine_w = var(&controls.sine_w) >> follow(smooth);
  let sub_tri_w = var(&controls.tri_w) >> follow(smooth);
  let sub_saw_w = var(&controls.saw_w) >> follow(smooth);
  let sub_square_w = var(&controls.square_w) >> follow(smooth);
  let sub_waveform = (sine() * sub_sine_w)
    & (triangle() * sub_tri_w)
    & (saw() * sub_saw_w)
    & (square() * sub_square_w);
  let sub_mix = var(&controls.sub_octave) >> follow(smooth);
  let sub_osc = (sub_freq_smooth >> sub_waveform) * sub_mix;

  // Drive via map() closure reading Shared, since shape(Tanh(..))
  // bakes the drive value at construction.
  let drive_s = controls.drive.clone();
  let drive_map = map(move |x: &Frame<f32, U1>| {
    let d = drive_s.value().max(0.01);
    (x[0] * d).tanh() / d.tanh()
  });

  // Noise mix.
  let noise_mix_smooth = var(&controls.noise_mix) >> follow(smooth);

  // Filter.
  let cutoff_smooth = var(&controls.filter_cutoff) >> follow(smooth);
  let q_smooth = var(&controls.filter_q) >> follow(smooth) * 0.2;

  // Amplitude.
  let amp_smooth = var(&controls.amplitude) >> follow(smooth);

  // External volume (master × mute).
  let ext = match external_volume {
    Some(s) => s.clone(),
    None => shared(1.0),
  };
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
  let signal =
    (main_osc + sub_osc) >> drive_map >> (pass() + (pink() * noise_mix_smooth));
  let mono = (signal | cutoff_smooth | q_smooth)
    >> (moog() * amp_smooth * trem_mod * ext_vol)
    >> dcblock()
    >> crush_map
    >> hold_hz(ds_rate, 0.0);
  let with_echo = mono
    >> (pass()
      & (feedback(delay(structural.echo_delay) * 0.3) * structural.echo_mix));
  let stereo = with_echo
    >> pan(structural.stereo_pan)
    >> reverb_stereo(0.3, 0.8, structural.reverb_mix);

  Box::new(stereo)
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
) -> (Box<dyn AudioUnit>, Vec<ContinuousControls>, Vec<StructuralParams>) {
  if patches.is_empty() {
    let ext = match external_volume {
      Some(s) => s.clone(),
      None => shared(1.0),
    };
    return (
      Box::new(dc(0.0) * var(&ext) | dc(0.0) * var(&ext)),
      Vec::new(),
      Vec::new(),
    );
  }

  let mut all_controls = Vec::with_capacity(patches.len());
  let mut all_structural = Vec::with_capacity(patches.len());

  let net = patches
    .iter()
    .map(|(patch, volume)| {
      let mut p = patch.clone();
      p.amplitude *= *volume;
      let controls = ContinuousControls::from_patch(&p);
      let structural = StructuralParams::from_patch(&p);
      let graph = continuous_graph(
        &controls,
        smoothing_secs,
        &structural,
        external_volume,
      );
      all_controls.push(controls);
      all_structural.push(structural);
      Net::wrap(graph)
    })
    .reduce(|acc, n| acc + n)
    .unwrap();

  (Box::new(net), all_controls, all_structural)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn continuous_graph_produces_sound() {
    let patch = Patch::default();
    let controls = ContinuousControls::from_patch(&patch);
    let structural = StructuralParams::from_patch(&patch);
    let mut graph = continuous_graph(&controls, 0.5, &structural, None);
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
      ..Default::default()
    };
    let controls = ContinuousControls::from_patch(&lo);
    assert!((controls.freq.value() - 200.0).abs() < 0.01);

    controls.update_from_patch(&hi);
    assert!((controls.freq.value() - 800.0).abs() < 0.01);
    assert!((controls.amplitude.value() - 0.8).abs() < 0.01);
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
    let a = StructuralParams::from_patch(&lo);
    let b = StructuralParams::from_patch(&hi);
    assert_ne!(a, b);

    let c = StructuralParams::from_patch(&lo);
    assert_eq!(a, c);
  }
}
