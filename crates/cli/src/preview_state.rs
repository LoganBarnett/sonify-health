use crate::print;
use fundsp::prelude32::shared;
use fundsp::shared::Shared;
use serde_json::json;
use sonify_health_lib::{
  check::{DroneMetricConfig, HeartbeatCheckConfig},
  state::{DroneState, HeartbeatState},
  BoopSpec, PentatonicScale, Severity, Voice, VoiceOverrides,
};
use std::collections::{HashMap, HashSet};
use std::sync::{
  atomic::{AtomicBool, AtomicUsize, Ordering},
  Arc, RwLock,
};
use tokio::sync::broadcast;

/// Voice parameter metadata matching `#[voice_param]` ranges.
pub struct VoiceParamMeta {
  pub name: &'static str,
  pub description: &'static str,
  pub min: f64,
  pub max: f64,
  pub step: f64,
}

pub const VOICE_PARAMS: &[VoiceParamMeta] = &[
  VoiceParamMeta {
    name: "base_freq",
    description: "Root pitch in Hz. All boop notes derive from this frequency.",
    min: 100.0,
    max: 12000.0,
    step: 1.0,
  },
  VoiceParamMeta {
    name: "sine_ratio",
    description: "Relative weight of the sine oscillator. Smooth, pure tone.",
    min: 0.0,
    max: 3.0,
    step: 0.01,
  },
  VoiceParamMeta {
    name: "tri_ratio",
    description:
      "Relative weight of the triangle oscillator. Hollow, flute-like.",
    min: 0.0,
    max: 3.0,
    step: 0.01,
  },
  VoiceParamMeta {
    name: "saw_ratio",
    description:
      "Relative weight of the sawtooth oscillator. Bright, buzzy edge.",
    min: 0.0,
    max: 3.0,
    step: 0.01,
  },
  VoiceParamMeta {
    name: "attack_ms",
    description:
      "Fade-in time in milliseconds. Low = snappy click, high = soft swell.",
    min: 1.0,
    max: 500.0,
    step: 1.0,
  },
  VoiceParamMeta {
    name: "release_ms",
    description:
      "Fade-out time in milliseconds. Low = staccato, high = lingering tail.",
    min: 10.0,
    max: 1000.0,
    step: 1.0,
  },
  VoiceParamMeta {
    name: "chirp_ratio",
    description:
      "Pitch bend at note onset. 1.0 = none, <1 = downward, >1 = upward chirp.",
    min: 0.5,
    max: 4.0,
    step: 0.01,
  },
  VoiceParamMeta {
    name: "stereo_pan",
    description: "Left/right stereo position. -1 = full left, +1 = full right.",
    min: -1.0,
    max: 1.0,
    step: 0.01,
  },
  VoiceParamMeta {
    name: "reverb_mix",
    description: "Wet/dry reverb blend. 0 = fully dry, 1 = fully wet.",
    min: 0.0,
    max: 1.0,
    step: 0.01,
  },
  VoiceParamMeta {
    name: "note_seed",
    description: "Seed for boop note selection within the pentatonic scale.",
    min: 0.0,
    max: 1.0,
    step: 0.01,
  },
  VoiceParamMeta {
    name: "echo_delay",
    description:
      "Delay time in seconds. Short = slapback, long = distinct repeats.",
    min: 0.01,
    max: 1.0,
    step: 0.01,
  },
  VoiceParamMeta {
    name: "echo_mix",
    description: "Echo wet/dry blend. 0 = no echo, 1 = full echo.",
    min: 0.0,
    max: 1.0,
    step: 0.01,
  },
  VoiceParamMeta {
    name: "brightness",
    description:
      "Lowpass cutoff scaler. 1.0 = full brightness, lower = darker tone.",
    min: 0.05,
    max: 2.0,
    step: 0.01,
  },
  VoiceParamMeta {
    name: "resonance",
    description:
      "Filter Q scaler. 1.0 = default resonance, lower = smoother rolloff, higher = nasal peak.",
    min: 0.1,
    max: 5.0,
    step: 0.01,
  },
  VoiceParamMeta {
    name: "sub_octave",
    description:
      "Sub-oscillator mix at one octave below. 0 = off, higher = deeper body.",
    min: 0.0,
    max: 1.0,
    step: 0.01,
  },
  VoiceParamMeta {
    name: "note_spread",
    description:
      "Range in octaves around base frequency for note selection.",
    min: 0.0,
    max: 2.0,
    step: 0.01,
  },
  VoiceParamMeta {
    name: "vibrato_rate",
    description: "Vibrato speed (Hz)",
    min: 0.0,
    max: 20.0,
    step: 0.1,
  },
  VoiceParamMeta {
    name: "vibrato_depth",
    description: "Vibrato depth (semitones)",
    min: 0.0,
    max: 2.0,
    step: 0.01,
  },
  VoiceParamMeta {
    name: "tremolo_rate",
    description: "Tremolo speed (Hz)",
    min: 0.0,
    max: 20.0,
    step: 0.1,
  },
  VoiceParamMeta {
    name: "tremolo_depth",
    description: "Tremolo depth (fraction)",
    min: 0.0,
    max: 1.0,
    step: 0.01,
  },
  VoiceParamMeta {
    name: "amplitude",
    description: "Output amplitude. 0 = silent, 1 = full scale.",
    min: 0.0,
    max: 1.0,
    step: 0.01,
  },
];

/// Metadata for a configured drone metric.
#[derive(Clone)]
pub struct DroneMetricInfo {
  pub name: String,
  pub base_freq: Option<f64>,
  pub boops: usize,
}

/// Identifies which sound-producing entity owns a voice.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum VoiceOwner {
  Heartbeat,
  Drone(usize),
}

impl VoiceOwner {
  /// Parse from the WebSocket `layer` + optional `index` fields.
  pub fn from_layer_index(layer: &str, index: Option<usize>) -> Option<Self> {
    match layer {
      "heartbeat" => Some(Self::Heartbeat),
      "drone" => index.map(Self::Drone),
      _ => None,
    }
  }
}

/// Shared mutable state backing the real-time preview UI.
///
/// Both the Axum WebSocket handler and the `spawn_blocking` daemon
/// thread share an `Arc<PreviewState>`.  Voice parameters, volumes,
/// and override maps are modified via the WebSocket protocol;
/// audio-relevant `Shared<f32>` values feed directly into fundsp
/// graphs.
pub struct PreviewState {
  original_voices: HashMap<VoiceOwner, Voice>,
  pub voices: RwLock<HashMap<VoiceOwner, Voice>>,
  pub scale: PentatonicScale,
  pub scale_key: String,
  pub muted: Arc<AtomicBool>,
  /// Per-metric raw volume set by the UI (0.0..=1.0).
  pub drone_volumes: Vec<Shared>,
  /// Direct speed multiplier on phrase repetition (0.1..=10.0).
  pub drone_repeat_rates: Vec<Shared>,
  /// How much the polled metric value contributes to repeat speed
  /// (0.0..=5.0).
  pub drone_repeat_factors: Vec<Shared>,
  /// `mute_factor * per_metric_volume`, wired into audio graphs.
  pub combined_volumes: Vec<Shared>,
  pub master_volume: Shared,
  pub heartbeat_volume: Shared,
  pub effective_heartbeat_volume: Shared,
  pub heartbeat_state: Arc<HeartbeatState>,
  pub drone_state: Arc<DroneState>,
  pub heartbeat_overrides: RwLock<Vec<Option<Severity>>>,
  pub drone_overrides: RwLock<Vec<Option<f32>>>,
  pub heartbeat_loop: AtomicBool,
  pub heartbeat_trigger: AtomicBool,
  pub broadcast_tx: broadcast::Sender<String>,
  pub check_log_tx: broadcast::Sender<String>,
  pub check_names: Vec<String>,
  pub drone_infos: RwLock<Vec<DroneMetricInfo>>,
  original_drone_infos: Vec<DroneMetricInfo>,
  pub boop_count: AtomicUsize,
  original_boop_count: usize,
  pub locked_params: RwLock<HashMap<VoiceOwner, HashSet<String>>>,
  pub locked_drones: RwLock<HashSet<usize>>,
  pub boop_specs: RwLock<Vec<BoopSpec>>,
  pub boop_pins: RwLock<Vec<bool>>,
  pub slot_secs: f64,
}

impl PreviewState {
  pub fn new(
    voice: Voice,
    scale: PentatonicScale,
    scale_key: String,
    muted: Arc<AtomicBool>,
    heartbeat_checks: &[HeartbeatCheckConfig],
    drone_metrics: &[DroneMetricConfig],
    drone_voice_overrides: &HashMap<String, VoiceOverrides>,
    slot_secs: f64,
    initial_notes: &[BoopSpec],
  ) -> Self {
    let drone_count = drone_metrics.len();
    let check_count = heartbeat_checks.len();

    let drone_volumes: Vec<Shared> =
      (0..drone_count).map(|_| shared(1.0)).collect();
    let drone_repeat_rates: Vec<Shared> =
      (0..drone_count).map(|_| shared(1.0)).collect();
    let drone_repeat_factors: Vec<Shared> =
      (0..drone_count).map(|_| shared(1.0)).collect();
    let combined_volumes: Vec<Shared> =
      (0..drone_count).map(|_| shared(1.0)).collect();

    let (broadcast_tx, _) = broadcast::channel(256);
    let (check_log_tx, _) = broadcast::channel(256);

    let drone_infos: Vec<DroneMetricInfo> = drone_metrics
      .iter()
      .map(|m| DroneMetricInfo {
        name: m.name.clone(),
        base_freq: m.base_freq,
        boops: m.boops.unwrap_or(1),
      })
      .collect();

    let (initial_specs, initial_pins, boop_count) = if initial_notes.is_empty()
    {
      let specs = voice.boop_specs(&scale, check_count, 1, slot_secs);
      let pins = vec![false; specs.len()];
      (specs, pins, 1)
    } else {
      let pins = vec![true; initial_notes.len()];
      let count = if check_count > 0 {
        (initial_notes.len() / check_count).max(1)
      } else {
        initial_notes.len().max(1)
      };
      (initial_notes.to_vec(), pins, count)
    };

    let mut voices = HashMap::new();
    voices.insert(VoiceOwner::Heartbeat, voice.clone());
    for (i, dm) in drone_metrics.iter().enumerate() {
      let drone_voice = drone_voice_overrides
        .get(&dm.name)
        .map(|ovr| voice.clone().with_overrides(ovr))
        .unwrap_or_else(|| voice.clone());
      voices.insert(VoiceOwner::Drone(i), drone_voice);
    }

    Self {
      original_voices: voices.clone(),
      voices: RwLock::new(voices),
      scale,
      scale_key,
      muted,
      drone_volumes,
      drone_repeat_rates,
      drone_repeat_factors,
      combined_volumes,
      master_volume: shared(1.0),
      heartbeat_volume: shared(1.0),
      effective_heartbeat_volume: shared(1.0),
      heartbeat_state: Arc::new(HeartbeatState::new(check_count)),
      drone_state: Arc::new(DroneState::new(drone_count)),
      heartbeat_overrides: RwLock::new(vec![None; check_count]),
      drone_overrides: RwLock::new(vec![None; drone_count]),
      heartbeat_loop: AtomicBool::new(false),
      heartbeat_trigger: AtomicBool::new(false),
      broadcast_tx,
      check_log_tx,
      check_names: heartbeat_checks.iter().map(|c| c.name.clone()).collect(),
      original_drone_infos: drone_infos.clone(),
      drone_infos: RwLock::new(drone_infos),
      boop_count: AtomicUsize::new(boop_count),
      original_boop_count: boop_count,
      locked_params: RwLock::new(HashMap::new()),
      locked_drones: RwLock::new(HashSet::new()),
      boop_specs: RwLock::new(initial_specs),
      boop_pins: RwLock::new(initial_pins),
      slot_secs,
    }
  }

  /// Recompute `combined_volumes[index]` from mute flag, master
  /// volume, and per-metric volume.
  pub fn update_combined_volume(&self, index: usize) {
    let mute_factor = if self.muted.load(Ordering::Relaxed) {
      0.0
    } else {
      1.0
    };
    let master = self.master_volume.value();
    if let (Some(dv), Some(cv)) =
      (self.drone_volumes.get(index), self.combined_volumes.get(index))
    {
      cv.set_value(mute_factor * master * dv.value());
    }
  }

  /// Update every combined volume (after mute toggle or master
  /// volume change).
  pub fn update_all_combined_volumes(&self) {
    for i in 0..self.drone_volumes.len() {
      self.update_combined_volume(i);
    }
  }

  /// Recompute the effective heartbeat volume from mute flag,
  /// master volume, and heartbeat volume.
  pub fn update_effective_heartbeat_volume(&self) {
    let mute_factor = if self.muted.load(Ordering::Relaxed) {
      0.0
    } else {
      1.0
    };
    let master = self.master_volume.value();
    self
      .effective_heartbeat_volume
      .set_value(mute_factor * master * self.heartbeat_volume.value());
  }

  /// Recompute materialized boop specs from the current voice,
  /// preserving pinned entries.
  pub fn recompute_boop_specs(&self) {
    let boops_per_check = self.boop_count.load(Ordering::Relaxed);
    let total = boops_per_check * self.check_names.len();
    let voices = self.voices.read().unwrap();
    let voice = &voices[&VoiceOwner::Heartbeat];
    let check_count = self.check_names.len();
    let fresh = voice.boop_specs(
      &self.scale,
      check_count,
      boops_per_check,
      self.slot_secs,
    );

    let mut specs = self.boop_specs.write().unwrap();
    let mut pins = self.boop_pins.write().unwrap();

    // Resize pins to match new total, new entries unpinned.
    pins.resize(total, false);

    let merged: Vec<BoopSpec> = fresh
      .into_iter()
      .enumerate()
      .map(|(i, fresh_spec)| {
        if i < specs.len() && i < pins.len() && pins[i] {
          specs[i].clone()
        } else {
          fresh_spec
        }
      })
      .collect();

    *specs = merged;
    // Trim pins if total shrank.
    pins.truncate(total);
  }

  /// Build the full state snapshot JSON sent on connect and on
  /// `get_state` / `revert_all`.
  pub fn state_snapshot(&self) -> String {
    let voices = self.voices.read().unwrap();
    let heartbeat_voice = &voices[&VoiceOwner::Heartbeat];
    let hb_overrides = self.heartbeat_overrides.read().unwrap();
    let drone_overrides = self.drone_overrides.read().unwrap();
    let drone_infos = self.drone_infos.read().unwrap();
    let locked = self.locked_params.read().unwrap();
    let specs = self.boop_specs.read().unwrap();
    let pins = self.boop_pins.read().unwrap();

    let heartbeat_voice_json = voice_to_json(heartbeat_voice);

    let heartbeat_locked: Vec<_> = locked
      .get(&VoiceOwner::Heartbeat)
      .map(|s| s.iter().map(|p| json!(p)).collect())
      .unwrap_or_default();

    let voice_params_json: Vec<_> = VOICE_PARAMS
      .iter()
      .map(|p| {
        json!({
          "name": p.name,
          "description": p.description,
          "min": p.min,
          "max": p.max,
          "step": p.step,
        })
      })
      .collect();

    let checks_json: Vec<_> = self
      .check_names
      .iter()
      .enumerate()
      .map(|(i, name)| {
        let severity =
          severity_from_shared(self.heartbeat_state.boops[i].value());
        json!({
          "name": name,
          "severity": severity.to_string(),
          "overridden": hb_overrides[i].is_some(),
        })
      })
      .collect();

    let drones_json: Vec<_> = drone_infos
      .iter()
      .enumerate()
      .map(|(i, info)| {
        let drone_voice = voices
          .get(&VoiceOwner::Drone(i))
          .map(voice_to_json)
          .unwrap_or(json!({}));
        let drone_locked: Vec<_> = locked
          .get(&VoiceOwner::Drone(i))
          .map(|s| s.iter().map(|p| json!(p)).collect())
          .unwrap_or_default();
        json!({
          "name": info.name,
          "value": self.drone_state.metrics[i].value(),
          "volume": self.drone_volumes[i].value(),
          "repeat_rate": self.drone_repeat_rates[i].value(),
          "repeat_factor": self.drone_repeat_factors[i].value(),
          "base_freq": info.base_freq,
          "boops": info.boops,
          "overridden": drone_overrides[i].is_some(),
          "voice": drone_voice,
          "locked_params": drone_locked,
        })
      })
      .collect();

    let locked_drones = self.locked_drones.read().unwrap();
    let locked_drones_json: Vec<_> =
      locked_drones.iter().map(|i| json!(i)).collect();

    let boop_specs_json: Vec<_> = specs
      .iter()
      .enumerate()
      .map(|(i, spec)| {
        json!({
          "freq": spec.freq,
          "duration": spec.duration,
          "pinned": pins.get(i).copied().unwrap_or(false),
        })
      })
      .collect();

    let base_freq_meta =
      VOICE_PARAMS.iter().find(|p| p.name == "base_freq").unwrap();

    json!({
      "type": "state",
      "voice": heartbeat_voice_json,
      "locked_params": heartbeat_locked,
      "voice_params": voice_params_json,
      "muted": self.muted.load(Ordering::Relaxed),
      "master_volume": self.master_volume.value(),
      "heartbeat_volume": self.heartbeat_volume.value(),
      "heartbeat_loop": self.heartbeat_loop.load(Ordering::Relaxed),
      "boop_count": self.boop_count.load(Ordering::Relaxed),
      "checks": checks_json,
      "drones": drones_json,
      "locked_drones": locked_drones_json,
      "boop_specs": boop_specs_json,
      "boop_spec_ranges": {
        "freq_min": base_freq_meta.min / 2.0,
        "freq_max": base_freq_meta.max,
        "freq_step": 1.0,
        "duration_min": 0.05,
        "duration_max": self.slot_secs,
        "duration_step": 0.01,
      },
    })
    .to_string()
  }

  /// Format all voices as a TOML block.
  pub fn export_toml(&self) -> String {
    let voices = self.voices.read().unwrap();
    let heartbeat_voice = &voices[&VoiceOwner::Heartbeat];
    let drone_infos = self.drone_infos.read().unwrap();
    let drone_voices: Vec<_> = drone_infos
      .iter()
      .enumerate()
      .map(|(i, info)| {
        let v = voices
          .get(&VoiceOwner::Drone(i))
          .cloned()
          .unwrap_or_else(|| heartbeat_voice.clone());
        (info.name.clone(), v)
      })
      .collect();
    let specs = self.boop_specs.read().unwrap();
    print::format_toml(heartbeat_voice, &drone_voices, &self.scale_key, &specs)
  }

  /// Format all voices as a JSON object.
  pub fn export_json(&self) -> String {
    let voices = self.voices.read().unwrap();
    let heartbeat_voice = &voices[&VoiceOwner::Heartbeat];
    let drone_infos = self.drone_infos.read().unwrap();
    let drone_voices: Vec<_> = drone_infos
      .iter()
      .enumerate()
      .map(|(i, info)| {
        let v = voices
          .get(&VoiceOwner::Drone(i))
          .cloned()
          .unwrap_or_else(|| heartbeat_voice.clone());
        (info.name.clone(), v)
      })
      .collect();
    let specs = self.boop_specs.read().unwrap();
    print::format_json(heartbeat_voice, &drone_voices, &self.scale_key, &specs)
  }

  /// Format all voices as a Nix attribute set.
  pub fn export_nix(&self) -> String {
    let voices = self.voices.read().unwrap();
    let heartbeat_voice = &voices[&VoiceOwner::Heartbeat];
    let drone_infos = self.drone_infos.read().unwrap();
    let drone_voices: Vec<_> = drone_infos
      .iter()
      .enumerate()
      .map(|(i, info)| {
        let v = voices
          .get(&VoiceOwner::Drone(i))
          .cloned()
          .unwrap_or_else(|| heartbeat_voice.clone());
        (info.name.clone(), v)
      })
      .collect();
    let specs = self.boop_specs.read().unwrap();
    print::format_nix(heartbeat_voice, &drone_voices, &self.scale_key, &specs)
  }

  /// Reset everything to startup values.  Locked voice params
  /// and locked drones survive the reset; boop pins are cleared.
  pub fn revert(&self) {
    // Snapshot per-entity locked param values before resetting.
    let locked = self.locked_params.read().unwrap().clone();
    let locked_values: HashMap<VoiceOwner, Vec<(String, f64)>> = {
      let voices = self.voices.read().unwrap();
      locked
        .iter()
        .map(|(owner, params)| {
          let vals: Vec<(String, f64)> = voices
            .get(owner)
            .map(|voice| {
              params
                .iter()
                .filter_map(|name| {
                  get_voice_param(voice, name).map(|v| (name.clone(), v))
                })
                .collect()
            })
            .unwrap_or_default();
          (owner.clone(), vals)
        })
        .collect()
    };

    // Snapshot locked drone settings before resetting.
    let locked_drone_indices = self.locked_drones.read().unwrap().clone();
    let locked_drone_snapshots: Vec<(usize, DroneMetricInfo, f32, f32, f32)> = {
      let infos = self.drone_infos.read().unwrap();
      locked_drone_indices
        .iter()
        .filter_map(|&i| {
          infos.get(i).map(|info| {
            (
              i,
              info.clone(),
              self.drone_volumes[i].value(),
              self.drone_repeat_rates[i].value(),
              self.drone_repeat_factors[i].value(),
            )
          })
        })
        .collect()
    };

    *self.voices.write().unwrap() = self.original_voices.clone();

    // Restore per-entity locked param values.
    {
      let mut voices = self.voices.write().unwrap();
      for (owner, vals) in &locked_values {
        if let Some(voice) = voices.get_mut(owner) {
          for (name, value) in vals {
            set_voice_param(voice, name, *value);
          }
        }
      }
    }

    *self.drone_infos.write().unwrap() = self.original_drone_infos.clone();

    for dv in &self.drone_volumes {
      dv.set_value(1.0);
    }
    for rr in &self.drone_repeat_rates {
      rr.set_value(1.0);
    }
    for rf in &self.drone_repeat_factors {
      rf.set_value(1.0);
    }

    // Restore locked drone settings.
    {
      let mut infos = self.drone_infos.write().unwrap();
      for (i, info, vol, rate, factor) in &locked_drone_snapshots {
        if let Some(entry) = infos.get_mut(*i) {
          *entry = info.clone();
        }
        self.drone_volumes[*i].set_value(*vol);
        self.drone_repeat_rates[*i].set_value(*rate);
        self.drone_repeat_factors[*i].set_value(*factor);
      }
    }
    self.master_volume.set_value(1.0);
    self.heartbeat_volume.set_value(1.0);
    self.update_all_combined_volumes();
    self.update_effective_heartbeat_volume();

    {
      let mut hb = self.heartbeat_overrides.write().unwrap();
      hb.iter_mut().for_each(|o| *o = None);
    }
    {
      let mut dr = self.drone_overrides.write().unwrap();
      dr.iter_mut().for_each(|o| *o = None);
    }

    self.heartbeat_loop.store(false, Ordering::Relaxed);
    self
      .boop_count
      .store(self.original_boop_count, Ordering::Relaxed);

    // Clear boop pins and recompute specs from reverted voice.
    {
      let mut pins = self.boop_pins.write().unwrap();
      pins.iter_mut().for_each(|p| *p = false);
    }
    self.recompute_boop_specs();
  }
}

// -- Helpers -----------------------------------------------------------------

pub fn get_voice_param(voice: &Voice, param: &str) -> Option<f64> {
  match param {
    "base_freq" => Some(voice.base_freq),
    "sine_ratio" => Some(voice.sine_ratio),
    "tri_ratio" => Some(voice.tri_ratio),
    "saw_ratio" => Some(voice.saw_ratio),
    "attack_ms" => Some(voice.attack_ms),
    "release_ms" => Some(voice.release_ms),
    "chirp_ratio" => Some(voice.chirp_ratio),
    "stereo_pan" => Some(voice.stereo_pan),
    "reverb_mix" => Some(voice.reverb_mix),
    "note_seed" => Some(voice.note_seed),
    "echo_delay" => Some(voice.echo_delay),
    "echo_mix" => Some(voice.echo_mix),
    "brightness" => Some(voice.brightness),
    "resonance" => Some(voice.resonance),
    "sub_octave" => Some(voice.sub_octave),
    "vibrato_rate" => Some(voice.vibrato_rate),
    "vibrato_depth" => Some(voice.vibrato_depth),
    "tremolo_rate" => Some(voice.tremolo_rate),
    "tremolo_depth" => Some(voice.tremolo_depth),
    "amplitude" => Some(voice.amplitude),
    "note_spread" => Some(voice.note_spread),
    _ => None,
  }
}

pub fn set_voice_param(voice: &mut Voice, param: &str, value: f64) -> bool {
  match param {
    "base_freq" => voice.base_freq = value,
    "sine_ratio" => voice.sine_ratio = value,
    "tri_ratio" => voice.tri_ratio = value,
    "saw_ratio" => voice.saw_ratio = value,
    "attack_ms" => voice.attack_ms = value,
    "release_ms" => voice.release_ms = value,
    "chirp_ratio" => voice.chirp_ratio = value,
    "stereo_pan" => voice.stereo_pan = value,
    "reverb_mix" => voice.reverb_mix = value,
    "note_seed" => voice.note_seed = value,
    "echo_delay" => voice.echo_delay = value,
    "echo_mix" => voice.echo_mix = value,
    "brightness" => voice.brightness = value,
    "resonance" => voice.resonance = value,
    "sub_octave" => voice.sub_octave = value,
    "vibrato_rate" => voice.vibrato_rate = value,
    "vibrato_depth" => voice.vibrato_depth = value,
    "tremolo_rate" => voice.tremolo_rate = value,
    "tremolo_depth" => voice.tremolo_depth = value,
    "amplitude" => voice.amplitude = value,
    "note_spread" => voice.note_spread = value,
    _ => return false,
  }
  true
}

fn voice_to_json(voice: &Voice) -> serde_json::Value {
  json!({
    "base_freq": voice.base_freq,
    "sine_ratio": voice.sine_ratio,
    "tri_ratio": voice.tri_ratio,
    "saw_ratio": voice.saw_ratio,
    "attack_ms": voice.attack_ms,
    "release_ms": voice.release_ms,
    "chirp_ratio": voice.chirp_ratio,
    "stereo_pan": voice.stereo_pan,
    "reverb_mix": voice.reverb_mix,
    "note_seed": voice.note_seed,
    "echo_delay": voice.echo_delay,
    "echo_mix": voice.echo_mix,
    "brightness": voice.brightness,
    "resonance": voice.resonance,
    "sub_octave": voice.sub_octave,
    "vibrato_rate": voice.vibrato_rate,
    "vibrato_depth": voice.vibrato_depth,
    "tremolo_rate": voice.tremolo_rate,
    "tremolo_depth": voice.tremolo_depth,
    "amplitude": voice.amplitude,
    "note_spread": voice.note_spread,
  })
}

pub fn severity_from_shared(value: f32) -> Severity {
  match value.round() as u8 {
    0 => Severity::Healthy,
    1 => Severity::Degraded,
    _ => Severity::Down,
  }
}

#[cfg(test)]
#[allow(dead_code)]
mod tests {
  use super::*;
  use serde::Deserialize;
  use sonify_health_lib::{
    check::{DroneMetricConfig, HeartbeatCheckConfig, ResultMode},
    Voice,
  };

  // Contract structs mirror the Elm frontend decoders exactly.
  // If the backend JSON shape drifts from what the frontend expects,
  // deserialization fails and the test catches it.
  //
  // Fields are never read directly — their purpose is to fail
  // deserialization when the backend omits or renames them.

  #[derive(Deserialize)]
  struct StateContract {
    #[serde(rename = "type")]
    msg_type: String,
    voice: serde_json::Map<String, serde_json::Value>,
    locked_params: Vec<String>,
    voice_params: Vec<VoiceParamContract>,
    muted: bool,
    master_volume: f64,
    heartbeat_volume: f64,
    heartbeat_loop: bool,
    boop_count: u64,
    checks: Vec<CheckContract>,
    drones: Vec<DroneContract>,
    locked_drones: Vec<u64>,
    boop_specs: Vec<BoopSpecContract>,
    boop_spec_ranges: BoopSpecRangesContract,
  }

  #[derive(Deserialize)]
  struct VoiceParamContract {
    name: String,
    description: String,
    min: f64,
    max: f64,
    step: f64,
  }

  #[derive(Deserialize)]
  struct CheckContract {
    name: String,
    severity: String,
    overridden: bool,
  }

  /// Mirrors the Elm `DroneInfo` decoder.  The `base_freq` field is
  /// nullable (Elm `Maybe Float`), so it uses `Option<f64>`.
  #[derive(Deserialize)]
  struct DroneContract {
    name: String,
    value: f64,
    volume: f64,
    repeat_rate: f64,
    repeat_factor: f64,
    base_freq: Option<f64>,
    boops: u64,
    overridden: bool,
    voice: serde_json::Map<String, serde_json::Value>,
    locked_params: Vec<String>,
  }

  #[derive(Deserialize)]
  struct BoopSpecContract {
    freq: f64,
    duration: f64,
    pinned: bool,
  }

  #[derive(Deserialize)]
  struct BoopSpecRangesContract {
    freq_min: f64,
    freq_max: f64,
    freq_step: f64,
    duration_min: f64,
    duration_max: f64,
    duration_step: f64,
  }

  /// Mirrors the Elm `DroneConfigChanged` decoder.
  #[derive(Deserialize)]
  struct DroneConfigChangedContract {
    #[serde(rename = "type")]
    msg_type: String,
    index: u64,
    base_freq: Option<f64>,
    boops: u64,
  }

  fn test_preview() -> PreviewState {
    let voice = Voice::from_hostname("test");
    let scale = PentatonicScale::from_key("C");
    let checks = vec![HeartbeatCheckConfig {
      name: "cpu".to_string(),
      command: "echo healthy".to_string(),
      result_mode: ResultMode::ExitCode,
    }];
    let drones = vec![DroneMetricConfig {
      name: "load".to_string(),
      command: "echo 0.5".to_string(),
      result_mode: ResultMode::Stdout,
      base_freq: None,
      boops: Some(2),
    }];
    PreviewState::new(
      voice,
      scale,
      "C".to_string(),
      Arc::new(AtomicBool::new(false)),
      &checks,
      &drones,
      &HashMap::new(),
      4.0,
      &[],
    )
  }

  #[test]
  fn state_snapshot_matches_frontend_contract() {
    let preview = test_preview();
    let json = preview.state_snapshot();
    let state: StateContract = serde_json::from_str(&json)
      .expect("state_snapshot JSON does not match the Elm frontend contract");

    assert_eq!(state.msg_type, "state");
    assert!(!state.voice_params.is_empty());
    assert_eq!(state.checks.len(), 1);
    assert_eq!(state.checks[0].name, "cpu");
    assert_eq!(state.drones.len(), 1);
    assert_eq!(state.drones[0].name, "load");
    assert_eq!(state.drones[0].boops, 2);
  }

  #[test]
  fn drone_config_changed_matches_frontend_contract() {
    let info = DroneMetricInfo {
      name: "load".to_string(),
      base_freq: Some(220.0),
      boops: 3,
    };
    let msg = serde_json::json!({
      "type": "drone_config_changed",
      "index": 0,
      "base_freq": info.base_freq,
      "boops": info.boops,
    })
    .to_string();

    let parsed: DroneConfigChangedContract = serde_json::from_str(&msg).expect(
      "drone_config_changed JSON does not match the Elm frontend \
         contract",
    );

    assert_eq!(parsed.msg_type, "drone_config_changed");
    assert_eq!(parsed.base_freq, Some(220.0));
    assert_eq!(parsed.boops, 3);
  }

  #[test]
  fn drone_config_changed_null_base_freq() {
    let msg = serde_json::json!({
      "type": "drone_config_changed",
      "index": 0,
      "base_freq": null,
      "boops": 1,
    })
    .to_string();

    let parsed: DroneConfigChangedContract = serde_json::from_str(&msg)
      .expect("drone_config_changed with null base_freq should still decode");
    assert_eq!(parsed.base_freq, None);
  }

  #[test]
  fn heartbeat_voice_change_does_not_affect_drone() {
    let preview = test_preview();
    let original_drone_freq = {
      let voices = preview.voices.read().unwrap();
      voices[&VoiceOwner::Drone(0)].base_freq
    };
    {
      let mut voices = preview.voices.write().unwrap();
      let hb = voices.get_mut(&VoiceOwner::Heartbeat).unwrap();
      set_voice_param(hb, "base_freq", 999.0);
    }
    let voices = preview.voices.read().unwrap();
    assert!(
      (voices[&VoiceOwner::Heartbeat].base_freq - 999.0).abs() < f64::EPSILON,
    );
    assert!(
      (voices[&VoiceOwner::Drone(0)].base_freq - original_drone_freq).abs()
        < f64::EPSILON,
    );
  }

  #[test]
  fn drone_voice_change_does_not_affect_heartbeat() {
    let preview = test_preview();
    let original_hb_freq = {
      let voices = preview.voices.read().unwrap();
      voices[&VoiceOwner::Heartbeat].base_freq
    };
    {
      let mut voices = preview.voices.write().unwrap();
      let drone = voices.get_mut(&VoiceOwner::Drone(0)).unwrap();
      set_voice_param(drone, "base_freq", 777.0);
    }
    let voices = preview.voices.read().unwrap();
    assert!(
      (voices[&VoiceOwner::Drone(0)].base_freq - 777.0).abs() < f64::EPSILON,
    );
    assert!(
      (voices[&VoiceOwner::Heartbeat].base_freq - original_hb_freq).abs()
        < f64::EPSILON,
    );
  }

  #[test]
  fn per_entity_lock_survives_revert() {
    let preview = test_preview();
    // Change heartbeat voice and lock it.
    {
      let mut voices = preview.voices.write().unwrap();
      let hb = voices.get_mut(&VoiceOwner::Heartbeat).unwrap();
      set_voice_param(hb, "base_freq", 555.0);
    }
    preview
      .locked_params
      .write()
      .unwrap()
      .entry(VoiceOwner::Heartbeat)
      .or_default()
      .insert("base_freq".to_string());

    preview.revert();

    let voices = preview.voices.read().unwrap();
    assert!(
      (voices[&VoiceOwner::Heartbeat].base_freq - 555.0).abs() < f64::EPSILON,
      "Locked heartbeat base_freq should survive revert",
    );
  }

  #[test]
  fn state_snapshot_encodes_all_voices() {
    let preview = test_preview();
    // Set heartbeat and drone to different values.
    {
      let mut voices = preview.voices.write().unwrap();
      set_voice_param(
        voices.get_mut(&VoiceOwner::Heartbeat).unwrap(),
        "base_freq",
        111.0,
      );
      set_voice_param(
        voices.get_mut(&VoiceOwner::Drone(0)).unwrap(),
        "base_freq",
        222.0,
      );
    }
    let json = preview.state_snapshot();
    let state: StateContract =
      serde_json::from_str(&json).expect("state_snapshot should decode");
    let hb_freq = state.voice["base_freq"].as_f64().unwrap();
    let drone_freq = state.drones[0].voice["base_freq"].as_f64().unwrap();
    assert!((hb_freq - 111.0).abs() < f64::EPSILON);
    assert!((drone_freq - 222.0).abs() < f64::EPSILON);
  }
}
