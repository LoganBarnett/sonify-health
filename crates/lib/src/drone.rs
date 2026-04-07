use crate::voice::Voice;
use clap::ValueEnum;
use fundsp::math::sin_hz;
use fundsp::prelude32::*;
use fundsp::shared::Shared;
use serde::Deserialize;
use tracing::debug;

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

/// Drone texture: the signal-chain character of a drone voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum DroneTexture {
  Bong,
  Arpeggio,
  Thrum,
  Shimmer,
  Reactor,
  Warpcore,
}

impl DroneTexture {
  /// Cycle through textures by metric index so different metrics
  /// on the same host automatically get distinct textures.
  pub fn from_index(i: usize) -> Self {
    match i % 6 {
      0 => DroneTexture::Bong,
      1 => DroneTexture::Arpeggio,
      2 => DroneTexture::Thrum,
      3 => DroneTexture::Shimmer,
      4 => DroneTexture::Reactor,
      _ => DroneTexture::Warpcore,
    }
  }
}

/// Normalized voice-blend weights from a Voice.
fn blend_weights(voice: &Voice) -> (f32, f32, f32) {
  let total = voice.sine_ratio + voice.tri_ratio + voice.saw_ratio;
  let norm = if total > 0.0 { 1.0 / total } else { 1.0 } as f32;
  (
    voice.sine_ratio as f32 * norm,
    voice.tri_ratio as f32 * norm,
    voice.saw_ratio as f32 * norm,
  )
}

/// Build a single drone graph for preview (no external volume).
pub fn drone_graph(
  voice: &Voice,
  register: DroneRegister,
  texture: DroneTexture,
  metric: &Shared,
  notes: &[f64],
) -> Box<dyn AudioUnit> {
  drone_graph_with_volume(voice, register, texture, metric, notes, None)
}

/// Build a drone graph with optional external volume multiplier
/// (used for daemon mute control).  Dispatches to the per-texture
/// builder.  `notes` is used by arpeggio only; pass an empty
/// slice for other textures.
pub fn drone_graph_with_volume(
  voice: &Voice,
  register: DroneRegister,
  texture: DroneTexture,
  metric: &Shared,
  notes: &[f64],
  external_volume: Option<&Shared>,
) -> Box<dyn AudioUnit> {
  match texture {
    DroneTexture::Bong => bong_graph(voice, register, metric, external_volume),
    DroneTexture::Arpeggio => {
      arpeggio_graph(voice, register, metric, external_volume, notes)
    }
    DroneTexture::Thrum => {
      thrum_graph(voice, register, metric, external_volume)
    }
    DroneTexture::Shimmer => {
      shimmer_graph(voice, register, metric, external_volume)
    }
    DroneTexture::Reactor => {
      reactor_graph(voice, register, metric, external_volume)
    }
    DroneTexture::Warpcore => {
      warp_core_graph(voice, register, metric, external_volume)
    }
  }
}

// ------------------------------------------------------------------
// Bong: periodic bell strikes with inharmonic partials
// ------------------------------------------------------------------

/// Periodic bell strikes with inharmonic bell partials (2.76x,
/// 5.4x), sub-octave weight, and filtered pink noise bed.  Metric
/// drives event rate (0.1-2 Hz) and brightness (200-1500 Hz
/// lowpass cutoff, Q 0.3-0.8).
fn bong_graph(
  voice: &Voice,
  register: DroneRegister,
  metric: &Shared,
  external_volume: Option<&Shared>,
) -> Box<dyn AudioUnit> {
  let freq = voice.base_freq as f32 * register.multiplier();
  let reverb_mix = voice.reverb_mix as f32;
  let metric_cutoff = metric.clone();
  let metric_q = metric.clone();
  let metric_bong = metric.clone();

  debug!(
    texture = "bong",
    register = ?register,
    base_freq = format_args!("{:.1} Hz", freq),
    sub_octave = format_args!("{:.1} Hz", freq * 0.5),
    bell_partial_1 = format_args!("{:.1} Hz (2.76x)", freq * 2.76),
    bell_partial_2 = format_args!("{:.1} Hz (5.4x)", freq * 5.4),
    mix = "primary 0.55, bell1 0.15, bell2 0.06, sub 0.20, noise 0.04",
    noise_filter = "lowpole 300 Hz",
    filter = "lowpass, Q 0.3-0.8",
    cutoff_range = "200-1500 Hz",
    reverb_mix = format_args!("{:.3}", voice.reverb_mix),
    "Bong harmonic recipe"
  );

  let (sine_w, tri_w, saw_w) = blend_weights(voice);
  let primary =
    sine_hz(freq) * sine_w + triangle_hz(freq) * tri_w + saw_hz(freq) * saw_w;
  let bell1 = sine_hz(freq * 2.76);
  let bell2 = sine_hz(freq * 5.4);
  let sub = sine_hz(freq * 0.5);
  let noise = pink() >> lowpole_hz(300.0);

  let osc =
    primary * 0.55 + bell1 * 0.15 + bell2 * 0.06 + sub * 0.20 + noise * 0.04;

  let cutoff = lfo(move |_t| {
    let m = metric_cutoff.value();
    200.0 + m * 1300.0
  });

  let q = lfo(move |_t| {
    let m = metric_q.value();
    0.3 + m * 0.5
  });

  // Exponential-decay pulse train.  Rate scales from one event per
  // 10 seconds (idle) to twice per second (full load).
  let am = lfo(move |t| {
    let m = metric_bong.value();
    let rate = 0.1 + m * 1.9;
    let phase = (t * rate) % 1.0;
    (-phase * 5.0).exp()
  });

  let vol = dc(0.30);

  let ext = match external_volume {
    Some(s) => s.clone(),
    None => shared(1.0),
  };
  let ext_vol = var(&ext) >> follow(0.5);

  let mono = (osc | cutoff | q) >> (lowpass() * am * vol * ext_vol);
  let stereo =
    mono >> pan(voice.stereo_pan as f32) >> reverb_stereo(0.6, 3.0, reverb_mix);
  Box::new(stereo)
}

// ------------------------------------------------------------------
// Arpeggio: cycling pentatonic notes with detuned unison and chorus
// ------------------------------------------------------------------

/// Cycling pentatonic arpeggio with detuned unison (+3 cents),
/// octave-above ghost tone, filtered pink noise bed, and chorus
/// for spatial width.  Metric drives cycle speed (0.05-0.5 Hz)
/// and brightness (lowpass, Q 0.5-0.8).
fn arpeggio_graph(
  voice: &Voice,
  register: DroneRegister,
  metric: &Shared,
  external_volume: Option<&Shared>,
  notes: &[f64],
) -> Box<dyn AudioUnit> {
  let mult = register.multiplier();
  let mut scaled: Vec<f32> = notes.iter().map(|&n| n as f32 * mult).collect();
  if scaled.is_empty() {
    scaled.push(voice.base_freq as f32 * mult);
  }
  let count = scaled.len();
  let reverb_mix = voice.reverb_mix as f32;

  let (sine_w, tri_w, saw_w) = blend_weights(voice);

  debug!(
    texture = "arpeggio",
    register = ?register,
    notes = ?scaled.iter().map(|n| format!("{:.1}", n)).collect::<Vec<_>>(),
    mix = "primary 0.55, detuned(x1.002) 0.30, octave_up 0.15",
    noise_filter = "lowpole 400 Hz, gain 0.03",
    chorus_params = "seed 0, sep 0.015, var 0.005, rate 0.2",
    filter = "lowpass, Q 0.5-0.8",
    cutoff_range = "200-1500 Hz",
    reverb_mix = format_args!("{:.3}", voice.reverb_mix),
    "Arpeggio harmonic recipe"
  );

  // Voice-blended waveform stepping through arpeggio notes with
  // detuned unison and octave-above ghost tone.
  let metric_osc = metric.clone();
  let notes_osc = scaled.clone();
  let osc = lfo(move |t| {
    let m = metric_osc.value();
    let rate = 0.05 + m * 0.45;
    let phase = (t * rate) % 1.0;
    let idx = (phase * count as f32).floor() as usize % count;
    let freq = notes_osc[idx];

    // Primary voice blend.
    let p = (t * freq) % 1.0;
    let s = (p * std::f32::consts::TAU).sin();
    let tri = 4.0 * (p - (p + 0.5).floor()).abs() - 1.0;
    let sw = 2.0 * p - 1.0;
    let primary = s * sine_w + tri * tri_w + sw * saw_w;

    // Detuned unison (+3 cents).
    let det_freq = freq * 1.002;
    let dp = (t * det_freq) % 1.0;
    let ds = (dp * std::f32::consts::TAU).sin();
    let dtri = 4.0 * (dp - (dp + 0.5).floor()).abs() - 1.0;
    let dsw = 2.0 * dp - 1.0;
    let detuned = ds * sine_w + dtri * tri_w + dsw * saw_w;

    // Octave-above ghost (sine only for ethereal quality).
    let ghost = (t * freq * 2.0 * std::f32::consts::TAU).sin();

    primary * 0.55 + detuned * 0.30 + ghost * 0.15
  });

  // Pink noise bed outside the note LFO.
  let noise = pink() >> lowpole_hz(400.0);
  let osc_with_noise = osc + noise * 0.03;

  let metric_cutoff = metric.clone();
  let cutoff = lfo(move |_t| {
    let m = metric_cutoff.value();
    200.0 + m * 1300.0
  });

  let metric_q = metric.clone();
  let q = lfo(move |_t| {
    let m = metric_q.value();
    0.5 + m * 0.3
  });

  // Soft per-note envelope masks phase discontinuities at note
  // boundaries.
  let metric_env = metric.clone();
  let am = lfo(move |t| {
    let m = metric_env.value();
    let rate = 0.05 + m * 0.45;
    let phase = (t * rate) % 1.0;
    let note_phase = (phase * count as f32) % 1.0;
    let attack = (note_phase * 10.0).min(1.0);
    let release = ((1.0 - note_phase) * 5.0).min(1.0);
    attack * release
  });

  let vol = dc(0.30);

  let ext = match external_volume {
    Some(s) => s.clone(),
    None => shared(1.0),
  };
  let ext_vol = var(&ext) >> follow(0.5);

  let mono = (osc_with_noise | cutoff | q) >> (lowpass() * am * vol * ext_vol);
  let mono_chorus = mono >> chorus(0, 0.015, 0.005, 0.2);
  let stereo = mono_chorus
    >> pan(voice.stereo_pan as f32)
    >> reverb_stereo(0.6, 5.0, reverb_mix);
  Box::new(stereo)
}

// ------------------------------------------------------------------
// Thrum: continuous tone with macro-pulse tremolo and filter wobble
// ------------------------------------------------------------------

/// Continuous tone with sub-octave triangle, 5th-harmonic saw
/// edge, and brown noise rumble.  Dual tremolo (primary + macro-
/// pulse at 0.03 Hz, blended 70/30) and slow filter wobble
/// (+/-60 Hz at 0.08 Hz).  Metric drives tremolo rate and depth.
fn thrum_graph(
  voice: &Voice,
  register: DroneRegister,
  metric: &Shared,
  external_volume: Option<&Shared>,
) -> Box<dyn AudioUnit> {
  let freq = voice.base_freq as f32 * register.multiplier();
  let reverb_mix = voice.reverb_mix as f32;

  let (sine_w, tri_w, saw_w) = blend_weights(voice);

  debug!(
    texture = "thrum",
    register = ?register,
    base_freq = format_args!("{:.1} Hz", freq),
    sub_tri = format_args!("{:.1} Hz (0.5x)", freq * 0.5),
    saw_5th = format_args!("{:.1} Hz (5x)", freq * 5.0),
    mix = "primary 0.55, sub_tri 0.25, saw_5x 0.04, brown 0.05",
    noise_filter = "lowpole 120 Hz",
    tremolo = "primary 0.5-6 Hz + macro 0.03 Hz, blend 70/30",
    filter_wobble = "0.08 Hz, +/-60 Hz",
    filter = "lowpass, Q 0.4-0.8",
    cutoff_range = "150-750 Hz",
    reverb_mix = format_args!("{:.3}", voice.reverb_mix),
    "Thrum harmonic recipe"
  );

  let primary =
    sine_hz(freq) * sine_w + triangle_hz(freq) * tri_w + saw_hz(freq) * saw_w;
  let sub_tri = triangle_hz(freq * 0.5);
  let saw_5x = saw_hz(freq * 5.0);
  let noise = brown() >> lowpole_hz(120.0);

  let osc = primary * 0.55 + sub_tri * 0.25 + saw_5x * 0.04 + noise * 0.05;

  // Cutoff with slow filter wobble at 0.08 Hz.
  let metric_cutoff = metric.clone();
  let cutoff = lfo(move |t| {
    let m = metric_cutoff.value();
    let center = 150.0 + m * 600.0;
    let wobble = (t * 0.08 * std::f32::consts::TAU).sin() * 60.0;
    center + wobble
  });

  let metric_q = metric.clone();
  let q = lfo(move |_t| {
    let m = metric_q.value();
    0.4 + m * 0.4
  });

  // Dual tremolo: primary sinusoidal tremolo blended 70/30 with a
  // macro-pulse at 0.03 Hz (~33 s period).
  let metric_trem = metric.clone();
  let am = lfo(move |t| {
    let m = metric_trem.value();
    let rate = 0.5 + m * 5.5;
    let depth = 0.3 + m * 0.6;

    let half = 0.5 * (1.0 + (t * rate * std::f32::consts::TAU).sin());
    let primary_trem = 1.0 - depth + depth * half;

    let macro_half = 0.5 * (1.0 + (t * 0.03 * std::f32::consts::TAU).sin());
    let macro_trem = 1.0 - depth + depth * macro_half;

    primary_trem * 0.7 + macro_trem * 0.3
  });

  let vol = dc(0.30);

  let ext = match external_volume {
    Some(s) => s.clone(),
    None => shared(1.0),
  };
  let ext_vol = var(&ext) >> follow(0.5);

  let mono = (osc | cutoff | q) >> (lowpass() * am * vol * ext_vol);
  let stereo =
    mono >> pan(voice.stereo_pan as f32) >> reverb_stereo(0.6, 2.5, reverb_mix);
  Box::new(stereo)
}

// ------------------------------------------------------------------
// Shimmer: 4-voice detuned pad with bandpass air and chorus
// ------------------------------------------------------------------

/// Four-voice detuned pad (primary, detuned-up, detuned-down,
/// octave ghost) with bandpass-filtered pink noise "air" and
/// post-filter chorus.  Metric drives detune width and swell
/// period.  Long reverb (6.0 s).
fn shimmer_graph(
  voice: &Voice,
  register: DroneRegister,
  metric: &Shared,
  external_volume: Option<&Shared>,
) -> Box<dyn AudioUnit> {
  let base = voice.base_freq as f32 * register.multiplier();
  let reverb_mix = voice.reverb_mix as f32;

  let (sine_w, tri_w, saw_w) = blend_weights(voice);

  debug!(
    texture = "shimmer",
    register = ?register,
    base_freq = format_args!("{:.1} Hz", base),
    mix = "primary 0.40, det_up 0.25, det_down 0.25, ghost(2x) 0.10",
    air_filter = "bandpass 2000 Hz Q=2.0, gain 0.02",
    chorus_params = "seed 42, sep 0.020, var 0.008, rate 0.15",
    filter = "lowpass, Q 0.5-0.7",
    cutoff_range = "400-2400 Hz",
    reverb_mix = format_args!("{:.3}", voice.reverb_mix),
    "Shimmer harmonic recipe"
  );

  // Four-voice detuned pad computed in a single LFO so all voice
  // types remain concrete for fundsp operators.
  let metric_osc = metric.clone();
  let mixed = lfo(move |t| {
    let m = metric_osc.value();
    let detune = 0.002 + m * 0.010;
    let freq_up = base * (1.0 + detune);
    let freq_down = base * (1.0 - detune);

    // Primary voice blend at base pitch.
    let p1 = (t * base) % 1.0;
    let s1 = (p1 * std::f32::consts::TAU).sin();
    let tri1 = 4.0 * (p1 - (p1 + 0.5).floor()).abs() - 1.0;
    let sw1 = 2.0 * p1 - 1.0;
    let primary = s1 * sine_w + tri1 * tri_w + sw1 * saw_w;

    // Detuned-up sine.
    let det_up = (t * freq_up * std::f32::consts::TAU).sin();

    // Detuned-down sine.
    let det_down = (t * freq_down * std::f32::consts::TAU).sin();

    // Octave-up ghost.
    let ghost = (t * base * 2.0 * std::f32::consts::TAU).sin();

    primary * 0.40 + det_up * 0.25 + det_down * 0.25 + ghost * 0.10
  });

  // Bandpass-filtered pink noise for focused "air".
  let air = pink() >> bandpass_hz(2000.0, 2.0);
  let osc_with_air = mixed + air * 0.02;

  let metric_cutoff = metric.clone();
  let cutoff = lfo(move |_t| {
    let m = metric_cutoff.value();
    400.0 + m * 2000.0
  });

  let metric_q = metric.clone();
  let q = lfo(move |_t| {
    let m = metric_q.value();
    0.5 + m * 0.2
  });

  // Slow amplitude swell.  Period shortens with metric (4 s idle,
  // 1.5 s full load).  Never fully silent.
  let metric_swell = metric.clone();
  let am = lfo(move |t| {
    let m = metric_swell.value();
    let period = 4.0 - m * 2.5;
    let half = 0.5 * (1.0 + (t / period * std::f32::consts::TAU).sin());
    0.4 + 0.6 * half
  });

  let vol = dc(0.30);

  let ext = match external_volume {
    Some(s) => s.clone(),
    None => shared(1.0),
  };
  let ext_vol = var(&ext) >> follow(0.5);

  let mono = (osc_with_air | cutoff | q) >> (lowpass() * am * vol * ext_vol);
  let mono_chorus = mono >> chorus(42, 0.020, 0.008, 0.15);
  let stereo = mono_chorus
    >> pan(voice.stereo_pan as f32)
    >> reverb_stereo(0.6, 6.0, reverb_mix);
  Box::new(stereo)
}

// ------------------------------------------------------------------
// Reactor: deep, slowly pulsing power hum
// ------------------------------------------------------------------

/// Deep power hum with harmonic stack (fundamental through 4th
/// harmonic), sub-octave sine, and brown noise rumble.  Slow power
/// pulse (0.05-0.15 Hz, floor 60%) with high-Q resonant filter
/// (0.5-1.1).  Low cutoff range (100-600 Hz).
fn reactor_graph(
  voice: &Voice,
  register: DroneRegister,
  metric: &Shared,
  external_volume: Option<&Shared>,
) -> Box<dyn AudioUnit> {
  let freq = voice.base_freq as f32 * register.multiplier();
  let reverb_mix = voice.reverb_mix as f32;

  let (sine_w, tri_w, saw_w) = blend_weights(voice);

  debug!(
    texture = "reactor",
    register = ?register,
    base_freq = format_args!("{:.1} Hz", freq),
    sub_octave = format_args!("{:.1} Hz", freq * 0.5),
    h2 = format_args!("{:.1} Hz (2x)", freq * 2.0),
    h3 = format_args!("{:.1} Hz (3x)", freq * 3.0),
    h4 = format_args!("{:.1} Hz (4x)", freq * 4.0),
    mix = "primary 0.40, sub 0.30, h2 0.12, h3 0.06, h4 0.03, brown 0.06",
    noise_filter = "lowpole 80 Hz",
    power_pulse = "0.05-0.15 Hz, floor 60%",
    filter = "lowpass, Q 0.5-1.1",
    cutoff_range = "100-600 Hz",
    reverb_mix = format_args!("{:.3}", voice.reverb_mix),
    "Reactor harmonic recipe"
  );

  let primary =
    sine_hz(freq) * sine_w + triangle_hz(freq) * tri_w + saw_hz(freq) * saw_w;
  let sub = sine_hz(freq * 0.5);
  let h2 = sine_hz(freq * 2.0);
  let h3 = sine_hz(freq * 3.0);
  let h4 = sine_hz(freq * 4.0);
  let noise = brown() >> lowpole_hz(80.0);

  let osc = primary * 0.40
    + sub * 0.30
    + h2 * 0.12
    + h3 * 0.06
    + h4 * 0.03
    + noise * 0.06;

  let metric_cutoff = metric.clone();
  let cutoff = lfo(move |_t| {
    let m = metric_cutoff.value();
    100.0 + m * 500.0
  });

  let metric_q = metric.clone();
  let q = lfo(move |_t| {
    let m = metric_q.value();
    0.5 + m * 0.6
  });

  // Slow power pulse.  Rate 0.05-0.15 Hz (7-20 s period), floor
  // at 60% so the reactor never fully dims.
  let metric_pulse = metric.clone();
  let am = lfo(move |t| {
    let m = metric_pulse.value();
    let rate = 0.05 + m * 0.10;
    let half = 0.5 * (1.0 + (t * rate * std::f32::consts::TAU).sin());
    0.6 + 0.4 * half
  });

  let vol = dc(0.30);

  let ext = match external_volume {
    Some(s) => s.clone(),
    None => shared(1.0),
  };
  let ext_vol = var(&ext) >> follow(0.5);

  let mono = (osc | cutoff | q) >> (lowpass() * am * vol * ext_vol);
  let stereo =
    mono >> pan(voice.stereo_pan as f32) >> reverb_stereo(0.7, 3.0, reverb_mix);
  Box::new(stereo)
}

// ------------------------------------------------------------------
// WarpCore: rhythmic pulse with synchronized spectral sweep
// ------------------------------------------------------------------

/// Rhythmic pulse with detuned sawtooth "plasma" pair, sub-octave
/// triangle, and pink noise.  Filter cutoff sweeps in sync with
/// the AM pulse for a "whooOOM" spectral effect.  Shaped sine
/// envelope (.powf(0.6)) and post-filter phaser.
fn warp_core_graph(
  voice: &Voice,
  register: DroneRegister,
  metric: &Shared,
  external_volume: Option<&Shared>,
) -> Box<dyn AudioUnit> {
  let freq = voice.base_freq as f32 * register.multiplier();
  let reverb_mix = voice.reverb_mix as f32;

  let (sine_w, tri_w, saw_w) = blend_weights(voice);

  debug!(
    texture = "warpcore",
    register = ?register,
    base_freq = format_args!("{:.1} Hz", freq),
    plasma_up = format_args!("{:.1} Hz (x1.003)", freq * 1.003),
    plasma_down = format_args!("{:.1} Hz (x0.997)", freq * 0.997),
    sub_tri = format_args!("{:.1} Hz (0.5x)", freq * 0.5),
    mix = "primary 0.50, plasma_up 0.08, plasma_down 0.08, sub_tri 0.18, noise 0.03",
    noise_filter = "lowpole 400 Hz",
    pulse = "0.3-2.0 Hz, floor 30%, shaped .powf(0.6)",
    phaser_params = "feedback 0.5, rate 0.04 Hz",
    filter = "lowpass, Q 0.4-0.8",
    cutoff_range = "200-1800 Hz",
    reverb_mix = format_args!("{:.3}", voice.reverb_mix),
    "WarpCore harmonic recipe"
  );

  let primary =
    sine_hz(freq) * sine_w + triangle_hz(freq) * tri_w + saw_hz(freq) * saw_w;
  let plasma_up = saw_hz(freq * 1.003);
  let plasma_down = saw_hz(freq * 0.997);
  let sub_tri = triangle_hz(freq * 0.5);
  let noise = pink() >> lowpole_hz(400.0);

  let osc = primary * 0.50
    + plasma_up * 0.08
    + plasma_down * 0.08
    + sub_tri * 0.18
    + noise * 0.03;

  // Filter cutoff sweeps in sync with AM pulse (phase-offset) for
  // a spectral "whooOOM" sweep.
  let metric_cutoff = metric.clone();
  let cutoff = lfo(move |t| {
    let m = metric_cutoff.value();
    let rate = 0.3 + m * 1.7;
    let sweep = 0.5 * (1.0 + (t * rate * std::f32::consts::TAU + 0.5).sin());
    200.0 + sweep * 1600.0
  });

  let metric_q = metric.clone();
  let q = lfo(move |_t| {
    let m = metric_q.value();
    0.4 + m * 0.4
  });

  // Shaped sine envelope with .powf(0.6) for softer peaks.  Floor
  // at 30%.
  let metric_am = metric.clone();
  let am = lfo(move |t| {
    let m = metric_am.value();
    let rate = 0.3 + m * 1.7;
    let half = 0.5 * (1.0 + (t * rate * std::f32::consts::TAU).sin());
    0.3 + 0.7 * half.powf(0.6)
  });

  let vol = dc(0.30);

  let ext = match external_volume {
    Some(s) => s.clone(),
    None => shared(1.0),
  };
  let ext_vol = var(&ext) >> follow(0.5);

  let mono = (osc | cutoff | q) >> (lowpass() * am * vol * ext_vol);
  let mono_phased = mono >> phaser(0.5, |t| sin_hz(0.04, t) * 0.5 + 0.5);
  let stereo = mono_phased
    >> pan(voice.stereo_pan as f32)
    >> reverb_stereo(0.6, 3.5, reverb_mix);
  Box::new(stereo)
}

#[cfg(test)]
mod tests {
  use super::*;
  use fundsp::prelude32::shared;

  #[test]
  fn drone_produces_sound_when_metric_nonzero() {
    let metric = shared(0.8);
    let mut graph = drone_graph(
      &Voice::from_hostname("test"),
      DroneRegister::Low,
      DroneTexture::Bong,
      &metric,
      &[],
    );
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
    let mut graph = drone_graph(
      &Voice::from_hostname("test"),
      DroneRegister::Mid,
      DroneTexture::Bong,
      &metric,
      &[],
    );
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
  fn high_metric_denser_than_low() {
    let metric_lo = shared(0.1);
    let metric_hi = shared(0.9);

    let mut lo_graph = drone_graph(
      &Voice::from_hostname("test"),
      DroneRegister::Mid,
      DroneTexture::Bong,
      &metric_lo,
      &[],
    );
    let mut hi_graph = drone_graph(
      &Voice::from_hostname("test"),
      DroneRegister::Mid,
      DroneTexture::Bong,
      &metric_hi,
      &[],
    );

    lo_graph.set_sample_rate(44100.0);
    hi_graph.set_sample_rate(44100.0);
    lo_graph.allocate();
    hi_graph.allocate();

    // Count zero-crossings as a proxy for event density.
    let crossings = |graph: &mut Box<dyn AudioUnit>| -> usize {
      let mut prev = 0.0f32;
      let mut count = 0usize;
      for _ in 0..88200 {
        let (l, _) = graph.get_stereo();
        if l * prev < 0.0 {
          count += 1;
        }
        prev = l;
      }
      count
    };

    let xings_lo = crossings(&mut lo_graph);
    let xings_hi = crossings(&mut hi_graph);

    assert!(
      xings_hi > xings_lo,
      "Higher metric should produce more zero-crossings \
       ({}) than lower ({})",
      xings_hi,
      xings_lo
    );
  }

  #[test]
  fn arpeggio_produces_sound() {
    let metric = shared(0.5);
    let notes = vec![220.0, 261.6, 329.6, 392.0];
    let mut graph = drone_graph(
      &Voice::from_hostname("test"),
      DroneRegister::Mid,
      DroneTexture::Arpeggio,
      &metric,
      &notes,
    );
    graph.set_sample_rate(44100.0);
    graph.allocate();

    let peak = (0..44100)
      .map(|_| {
        let (l, r) = graph.get_stereo();
        l.abs().max(r.abs())
      })
      .fold(0.0f32, f32::max);

    assert!(peak > 0.001, "Arpeggio should produce sound, got peak {}", peak);
  }

  #[test]
  fn thrum_produces_sound() {
    let metric = shared(0.5);
    let mut graph = drone_graph(
      &Voice::from_hostname("test"),
      DroneRegister::Mid,
      DroneTexture::Thrum,
      &metric,
      &[],
    );
    graph.set_sample_rate(44100.0);
    graph.allocate();

    let peak = (0..44100)
      .map(|_| {
        let (l, r) = graph.get_stereo();
        l.abs().max(r.abs())
      })
      .fold(0.0f32, f32::max);

    assert!(peak > 0.001, "Thrum should produce sound, got peak {}", peak);
  }

  #[test]
  fn shimmer_produces_sound() {
    let metric = shared(0.5);
    let mut graph = drone_graph(
      &Voice::from_hostname("test"),
      DroneRegister::Mid,
      DroneTexture::Shimmer,
      &metric,
      &[],
    );
    graph.set_sample_rate(44100.0);
    graph.allocate();

    let peak = (0..44100)
      .map(|_| {
        let (l, r) = graph.get_stereo();
        l.abs().max(r.abs())
      })
      .fold(0.0f32, f32::max);

    assert!(peak > 0.001, "Shimmer should produce sound, got peak {}", peak);
  }

  #[test]
  fn reactor_produces_sound() {
    let metric = shared(0.5);
    let mut graph = drone_graph(
      &Voice::from_hostname("test"),
      DroneRegister::Low,
      DroneTexture::Reactor,
      &metric,
      &[],
    );
    graph.set_sample_rate(44100.0);
    graph.allocate();

    let peak = (0..44100)
      .map(|_| {
        let (l, r) = graph.get_stereo();
        l.abs().max(r.abs())
      })
      .fold(0.0f32, f32::max);

    assert!(peak > 0.001, "Reactor should produce sound, got peak {}", peak);
  }

  #[test]
  fn warp_core_produces_sound() {
    let metric = shared(0.5);
    let mut graph = drone_graph(
      &Voice::from_hostname("test"),
      DroneRegister::Mid,
      DroneTexture::Warpcore,
      &metric,
      &[],
    );
    graph.set_sample_rate(44100.0);
    graph.allocate();

    let peak = (0..44100)
      .map(|_| {
        let (l, r) = graph.get_stereo();
        l.abs().max(r.abs())
      })
      .fold(0.0f32, f32::max);

    assert!(peak > 0.001, "WarpCore should produce sound, got peak {}", peak);
  }

  #[test]
  fn texture_from_index_cycles() {
    assert_eq!(DroneTexture::from_index(0), DroneTexture::Bong);
    assert_eq!(DroneTexture::from_index(1), DroneTexture::Arpeggio);
    assert_eq!(DroneTexture::from_index(2), DroneTexture::Thrum);
    assert_eq!(DroneTexture::from_index(3), DroneTexture::Shimmer);
    assert_eq!(DroneTexture::from_index(4), DroneTexture::Reactor);
    assert_eq!(DroneTexture::from_index(5), DroneTexture::Warpcore);
    assert_eq!(DroneTexture::from_index(6), DroneTexture::Bong);
  }
}
