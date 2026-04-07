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

/// Drone texture: the signal-chain character of a drone voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum DroneTexture {
  Bong,
  Arpeggio,
  Thrum,
  Shimmer,
}

impl DroneTexture {
  /// Cycle through textures by metric index so different metrics
  /// on the same host automatically get distinct textures.
  pub fn from_index(i: usize) -> Self {
    match i % 4 {
      0 => DroneTexture::Bong,
      1 => DroneTexture::Arpeggio,
      2 => DroneTexture::Thrum,
      _ => DroneTexture::Shimmer,
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
  }
}

// ------------------------------------------------------------------
// Bong: periodic bell strikes (existing behaviour)
// ------------------------------------------------------------------

/// Periodic bell strikes with exponential-decay pulse train.
/// Metric drives event rate (0.1-2 Hz) and brightness (200-1500 Hz
/// lowpole cutoff).
fn bong_graph(
  voice: &Voice,
  register: DroneRegister,
  metric: &Shared,
  external_volume: Option<&Shared>,
) -> Box<dyn AudioUnit> {
  let base = voice.base_freq as f32 * register.multiplier();
  let metric_cutoff = metric.clone();
  let metric_bong = metric.clone();

  let (sine_w, tri_w, saw_w) = blend_weights(voice);
  let osc =
    sine_hz(base) * sine_w + triangle_hz(base) * tri_w + saw_hz(base) * saw_w;

  let cutoff = lfo(move |_t| {
    let m = metric_cutoff.value();
    200.0 + m * 1300.0
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

  let mono = (osc | cutoff) >> (lowpole() * am * vol * ext_vol);
  let stereo =
    mono >> pan(voice.stereo_pan as f32) >> reverb_stereo(0.6, 3.0, 0.4);
  Box::new(stereo)
}

// ------------------------------------------------------------------
// Arpeggio: cycling pentatonic notes with overlapping reverb tails
// ------------------------------------------------------------------

/// Cycling pentatonic arpeggio with soft per-note envelopes and
/// long reverb.  Metric drives cycle speed (0.05-0.5 Hz, one full
/// cycle every 2-20 s) and brightness.
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

  let (sine_w, tri_w, saw_w) = blend_weights(voice);

  // Voice-blended waveform stepping through arpeggio notes.
  // Computed per-sample so frequency changes are discrete.
  let metric_osc = metric.clone();
  let notes_osc = scaled.clone();
  let osc = lfo(move |t| {
    let m = metric_osc.value();
    let rate = 0.05 + m * 0.45;
    let phase = (t * rate) % 1.0;
    let idx = (phase * count as f32).floor() as usize % count;
    let freq = notes_osc[idx];
    let p = (t * freq) % 1.0;
    let s = (p * std::f32::consts::TAU).sin();
    let tri = 4.0 * (p - (p + 0.5).floor()).abs() - 1.0;
    let sw = 2.0 * p - 1.0;
    s * sine_w + tri * tri_w + sw * saw_w
  });

  let metric_cutoff = metric.clone();
  let cutoff = lfo(move |_t| {
    let m = metric_cutoff.value();
    200.0 + m * 1300.0
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

  let mono = (osc | cutoff) >> (lowpole() * am * vol * ext_vol);
  let stereo =
    mono >> pan(voice.stereo_pan as f32) >> reverb_stereo(0.6, 4.0, 0.2);
  Box::new(stereo)
}

// ------------------------------------------------------------------
// Thrum: continuous tone with sinusoidal tremolo
// ------------------------------------------------------------------

/// Continuous tone that never goes silent.  Metric drives tremolo
/// rate (0.5-6 Hz) and depth (0.3-0.9).  Warmer cutoff range
/// (150-750 Hz), shorter reverb (2.5 s).
fn thrum_graph(
  voice: &Voice,
  register: DroneRegister,
  metric: &Shared,
  external_volume: Option<&Shared>,
) -> Box<dyn AudioUnit> {
  let base = voice.base_freq as f32 * register.multiplier();

  let (sine_w, tri_w, saw_w) = blend_weights(voice);
  let osc =
    sine_hz(base) * sine_w + triangle_hz(base) * tri_w + saw_hz(base) * saw_w;

  let metric_cutoff = metric.clone();
  let cutoff = lfo(move |_t| {
    let m = metric_cutoff.value();
    150.0 + m * 600.0
  });

  // Sinusoidal tremolo.  Amplitude oscillates between (1-depth)
  // and 1.0 so the tone is always audible.
  let metric_trem = metric.clone();
  let am = lfo(move |t| {
    let m = metric_trem.value();
    let rate = 0.5 + m * 5.5;
    let depth = 0.3 + m * 0.6;
    let half = 0.5 * (1.0 + (t * rate * std::f32::consts::TAU).sin());
    1.0 - depth + depth * half
  });

  let vol = dc(0.30);

  let ext = match external_volume {
    Some(s) => s.clone(),
    None => shared(1.0),
  };
  let ext_vol = var(&ext) >> follow(0.5);

  let mono = (osc | cutoff) >> (lowpole() * am * vol * ext_vol);
  let stereo =
    mono >> pan(voice.stereo_pan as f32) >> reverb_stereo(0.6, 2.5, 0.4);
  Box::new(stereo)
}

// ------------------------------------------------------------------
// Shimmer: detuned sine pair with slow beating/phasing
// ------------------------------------------------------------------

/// Detuned sine pair creating slow beating.  Metric drives detune
/// amount (0.2-1.2 %) and a slow amplitude swell (1.5-4 s period).
/// Bright cutoff range (400-2400 Hz), long reverb (5.0 s).
fn shimmer_graph(
  voice: &Voice,
  register: DroneRegister,
  metric: &Shared,
  external_volume: Option<&Shared>,
) -> Box<dyn AudioUnit> {
  let base = voice.base_freq as f32 * register.multiplier();

  let (sine_w, tri_w, saw_w) = blend_weights(voice);

  // Combine voice-blended oscillator with a detuned sine into a
  // single LFO so all types are concrete for the fundsp operators.
  let metric_osc = metric.clone();
  let mixed = lfo(move |t| {
    let m = metric_osc.value();
    let detune = 0.002 + m * 0.010;
    let freq2 = base * (1.0 + detune);

    // Primary voice blend at base pitch.
    let p1 = (t * base) % 1.0;
    let s1 = (p1 * std::f32::consts::TAU).sin();
    let tri1 = 4.0 * (p1 - (p1 + 0.5).floor()).abs() - 1.0;
    let sw1 = 2.0 * p1 - 1.0;
    let primary = s1 * sine_w + tri1 * tri_w + sw1 * saw_w;

    // Detuned sine for beating.
    let secondary = (t * freq2 * std::f32::consts::TAU).sin();

    0.5 * primary + 0.5 * secondary
  });

  let metric_cutoff = metric.clone();
  let cutoff = lfo(move |_t| {
    let m = metric_cutoff.value();
    400.0 + m * 2000.0
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

  let mono = (mixed | cutoff) >> (lowpole() * am * vol * ext_vol);
  let stereo =
    mono >> pan(voice.stereo_pan as f32) >> reverb_stereo(0.6, 5.0, 0.3);
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
  fn texture_from_index_cycles() {
    assert_eq!(DroneTexture::from_index(0), DroneTexture::Bong);
    assert_eq!(DroneTexture::from_index(1), DroneTexture::Arpeggio);
    assert_eq!(DroneTexture::from_index(2), DroneTexture::Thrum);
    assert_eq!(DroneTexture::from_index(3), DroneTexture::Shimmer);
    assert_eq!(DroneTexture::from_index(4), DroneTexture::Bong);
  }
}
