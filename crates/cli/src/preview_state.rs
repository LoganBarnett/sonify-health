use crate::print;
use fundsp::prelude32::shared;
use fundsp::shared::Shared;
use serde_json::json;
use sonify_health_lib::{
  check::{DroneMetricConfig, HeartbeatCheckConfig},
  drone::{DroneRegister, DroneTexture},
  heartbeat,
  state::{DroneState, HeartbeatState},
  BoopSpec, PentatonicScale, Severity, Voice,
};
use std::collections::HashSet;
use std::sync::{
  atomic::{AtomicBool, AtomicUsize, Ordering},
  Arc, RwLock,
};
use tokio::sync::broadcast;

/// Voice parameter metadata matching `#[voice_param]` ranges.
pub struct VoiceParamMeta {
  pub name: &'static str,
  pub min: f64,
  pub max: f64,
  pub step: f64,
}

pub const VOICE_PARAMS: &[VoiceParamMeta] = &[
  VoiceParamMeta {
    name: "base_freq",
    min: 100.0,
    max: 4000.0,
    step: 1.0,
  },
  VoiceParamMeta {
    name: "sine_ratio",
    min: 0.5,
    max: 1.0,
    step: 0.01,
  },
  VoiceParamMeta {
    name: "tri_ratio",
    min: 0.0,
    max: 0.3,
    step: 0.01,
  },
  VoiceParamMeta {
    name: "saw_ratio",
    min: 0.0,
    max: 0.15,
    step: 0.001,
  },
  VoiceParamMeta {
    name: "attack_ms",
    min: 20.0,
    max: 80.0,
    step: 1.0,
  },
  VoiceParamMeta {
    name: "release_ms",
    min: 80.0,
    max: 250.0,
    step: 1.0,
  },
  VoiceParamMeta {
    name: "chirp_ratio",
    min: 1.0,
    max: 1.5,
    step: 0.01,
  },
  VoiceParamMeta {
    name: "stereo_pan",
    min: -0.3,
    max: 0.3,
    step: 0.01,
  },
  VoiceParamMeta {
    name: "reverb_mix",
    min: 0.3,
    max: 0.6,
    step: 0.01,
  },
  VoiceParamMeta {
    name: "note_seed",
    min: 0.0,
    max: 1.0,
    step: 0.01,
  },
  VoiceParamMeta {
    name: "echo_delay",
    min: 0.05,
    max: 0.25,
    step: 0.01,
  },
  VoiceParamMeta {
    name: "echo_mix",
    min: 0.0,
    max: 0.4,
    step: 0.01,
  },
];

/// Metadata for a configured drone metric.
#[derive(Clone)]
pub struct DroneMetricInfo {
  pub name: String,
  pub register: DroneRegister,
  pub texture: DroneTexture,
}

/// Shared mutable state backing the real-time preview UI.
///
/// Both the Axum WebSocket handler and the `spawn_blocking` daemon
/// thread share an `Arc<PreviewState>`.  Voice parameters, volumes,
/// and override maps are modified via the WebSocket protocol;
/// audio-relevant `Shared<f32>` values feed directly into fundsp
/// graphs.
pub struct PreviewState {
  original_voice: Voice,
  pub voice: RwLock<Voice>,
  pub scale: PentatonicScale,
  pub scale_key: String,
  pub muted: Arc<AtomicBool>,
  /// Per-metric raw volume set by the UI (0.0..=1.0).
  pub drone_volumes: Vec<Shared>,
  /// `mute_factor * per_metric_volume`, wired into audio graphs.
  pub combined_volumes: Vec<Shared>,
  pub heartbeat_volume: Shared,
  pub heartbeat_state: Arc<HeartbeatState>,
  pub drone_state: Arc<DroneState>,
  pub heartbeat_overrides: RwLock<Vec<Option<Severity>>>,
  pub drone_overrides: RwLock<Vec<Option<f32>>>,
  pub drone_rebuild_requested: AtomicBool,
  pub heartbeat_loop: AtomicBool,
  pub heartbeat_trigger: AtomicBool,
  pub broadcast_tx: broadcast::Sender<String>,
  pub check_log_tx: broadcast::Sender<String>,
  pub check_names: Vec<String>,
  pub drone_infos: RwLock<Vec<DroneMetricInfo>>,
  original_drone_infos: Vec<DroneMetricInfo>,
  pub boop_count: AtomicUsize,
  original_boop_count: usize,
  pub locked_params: RwLock<HashSet<String>>,
  pub locked_drones: RwLock<HashSet<usize>>,
  pub boop_specs: RwLock<Vec<BoopSpec>>,
  pub boop_pins: RwLock<Vec<bool>>,
}

impl PreviewState {
  pub fn new(
    voice: Voice,
    scale: PentatonicScale,
    scale_key: String,
    muted: Arc<AtomicBool>,
    heartbeat_checks: &[HeartbeatCheckConfig],
    drone_metrics: &[DroneMetricConfig],
  ) -> Self {
    let drone_count = drone_metrics.len();
    let check_count = heartbeat_checks.len();

    let drone_volumes: Vec<Shared> =
      (0..drone_count).map(|_| shared(1.0)).collect();
    let combined_volumes: Vec<Shared> =
      (0..drone_count).map(|_| shared(1.0)).collect();

    let (broadcast_tx, _) = broadcast::channel(256);
    let (check_log_tx, _) = broadcast::channel(256);

    let drone_infos: Vec<DroneMetricInfo> = drone_metrics
      .iter()
      .enumerate()
      .map(|(i, m)| DroneMetricInfo {
        name: m.name.clone(),
        register: m.register,
        texture: m.texture.unwrap_or_else(|| voice.drone_texture(i)),
      })
      .collect();

    let initial_specs =
      voice.boop_specs(&scale, check_count, 1, heartbeat::TOTAL_BOOP_TIME);
    let initial_pins = vec![false; initial_specs.len()];

    Self {
      original_voice: voice.clone(),
      voice: RwLock::new(voice),
      scale,
      scale_key,
      muted,
      drone_volumes,
      combined_volumes,
      heartbeat_volume: shared(1.0),
      heartbeat_state: Arc::new(HeartbeatState::new(check_count)),
      drone_state: Arc::new(DroneState::new(drone_count)),
      heartbeat_overrides: RwLock::new(vec![None; check_count]),
      drone_overrides: RwLock::new(vec![None; drone_count]),
      drone_rebuild_requested: AtomicBool::new(false),
      heartbeat_loop: AtomicBool::new(false),
      heartbeat_trigger: AtomicBool::new(false),
      broadcast_tx,
      check_log_tx,
      check_names: heartbeat_checks.iter().map(|c| c.name.clone()).collect(),
      original_drone_infos: drone_infos.clone(),
      drone_infos: RwLock::new(drone_infos),
      boop_count: AtomicUsize::new(1),
      original_boop_count: 1,
      locked_params: RwLock::new(HashSet::new()),
      locked_drones: RwLock::new(HashSet::new()),
      boop_specs: RwLock::new(initial_specs),
      boop_pins: RwLock::new(initial_pins),
    }
  }

  /// Recompute `combined_volumes[index]` from mute flag and
  /// per-metric volume.
  pub fn update_combined_volume(&self, index: usize) {
    let mute_factor = if self.muted.load(Ordering::Relaxed) {
      0.0
    } else {
      1.0
    };
    if let (Some(dv), Some(cv)) =
      (self.drone_volumes.get(index), self.combined_volumes.get(index))
    {
      cv.set_value(mute_factor * dv.value());
    }
  }

  /// Update every combined volume (after mute toggle).
  pub fn update_all_combined_volumes(&self) {
    for i in 0..self.drone_volumes.len() {
      self.update_combined_volume(i);
    }
  }

  /// Recompute materialized boop specs from the current voice,
  /// preserving pinned entries.
  pub fn recompute_boop_specs(&self) {
    let boops_per_check = self.boop_count.load(Ordering::Relaxed);
    let total = boops_per_check * self.check_names.len();
    let voice = self.voice.read().unwrap();
    let check_count = self.check_names.len();
    let fresh = voice.boop_specs(
      &self.scale,
      check_count,
      boops_per_check,
      heartbeat::TOTAL_BOOP_TIME,
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
    let voice = self.voice.read().unwrap();
    let hb_overrides = self.heartbeat_overrides.read().unwrap();
    let drone_overrides = self.drone_overrides.read().unwrap();
    let drone_infos = self.drone_infos.read().unwrap();
    let locked = self.locked_params.read().unwrap();
    let specs = self.boop_specs.read().unwrap();
    let pins = self.boop_pins.read().unwrap();

    let voice_json = voice_to_json(&voice);

    let voice_params_json: Vec<_> = VOICE_PARAMS
      .iter()
      .map(|p| {
        json!({
          "name": p.name,
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
        json!({
          "name": info.name,
          "value": self.drone_state.metrics[i].value(),
          "volume": self.drone_volumes[i].value(),
          "texture": texture_str(info.texture),
          "register": register_str(info.register),
          "overridden": drone_overrides[i].is_some(),
        })
      })
      .collect();

    let locked_json: Vec<_> = locked.iter().map(|s| json!(s)).collect();
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

    json!({
      "type": "state",
      "voice": voice_json,
      "voice_params": voice_params_json,
      "muted": self.muted.load(Ordering::Relaxed),
      "heartbeat_volume": self.heartbeat_volume.value(),
      "heartbeat_loop": self.heartbeat_loop.load(Ordering::Relaxed),
      "boop_count": self.boop_count.load(Ordering::Relaxed),
      "checks": checks_json,
      "drones": drones_json,
      "locked_params": locked_json,
      "locked_drones": locked_drones_json,
      "boop_specs": boop_specs_json,
    })
    .to_string()
  }

  /// Format the current voice as a TOML `[voice]` block.
  pub fn export_toml(&self) -> String {
    let voice = self.voice.read().unwrap();
    print::format_toml(&voice, &self.scale_key)
  }

  /// Reset everything to startup values.  Locked voice params
  /// and locked drones survive the reset; boop pins are cleared.
  pub fn revert(&self) {
    // Snapshot locked param values before resetting the voice.
    let locked = self.locked_params.read().unwrap().clone();
    let locked_values: Vec<(String, f64)> = {
      let voice = self.voice.read().unwrap();
      locked
        .iter()
        .filter_map(|name| {
          get_voice_param(&voice, name).map(|v| (name.clone(), v))
        })
        .collect()
    };

    // Snapshot locked drone settings before resetting.
    let locked_drone_indices = self.locked_drones.read().unwrap().clone();
    let locked_drone_snapshots: Vec<(usize, DroneMetricInfo, f32)> = {
      let infos = self.drone_infos.read().unwrap();
      locked_drone_indices
        .iter()
        .filter_map(|&i| {
          infos
            .get(i)
            .map(|info| (i, info.clone(), self.drone_volumes[i].value()))
        })
        .collect()
    };

    *self.voice.write().unwrap() = self.original_voice.clone();

    // Restore locked param values.
    {
      let mut voice = self.voice.write().unwrap();
      for (name, value) in &locked_values {
        set_voice_param(&mut voice, name, *value);
      }
    }

    *self.drone_infos.write().unwrap() = self.original_drone_infos.clone();

    for dv in &self.drone_volumes {
      dv.set_value(1.0);
    }

    // Restore locked drone settings.
    {
      let mut infos = self.drone_infos.write().unwrap();
      for (i, info, vol) in &locked_drone_snapshots {
        if let Some(entry) = infos.get_mut(*i) {
          *entry = info.clone();
        }
        self.drone_volumes[*i].set_value(*vol);
      }
    }
    self.heartbeat_volume.set_value(1.0);
    self.update_all_combined_volumes();

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
    self.drone_rebuild_requested.store(true, Ordering::Relaxed);

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
  })
}

pub fn severity_from_shared(value: f32) -> Severity {
  match value.round() as u8 {
    0 => Severity::Healthy,
    1 => Severity::Degraded,
    _ => Severity::Down,
  }
}

pub fn texture_str(t: DroneTexture) -> &'static str {
  match t {
    DroneTexture::Bong => "bong",
    DroneTexture::Arpeggio => "arpeggio",
    DroneTexture::Thrum => "thrum",
    DroneTexture::Shimmer => "shimmer",
    DroneTexture::Reactor => "reactor",
    DroneTexture::Warpcore => "warpcore",
  }
}

pub fn register_str(r: DroneRegister) -> &'static str {
  match r {
    DroneRegister::Low => "low",
    DroneRegister::Mid => "mid",
    DroneRegister::High => "high",
  }
}

pub fn texture_from_str(s: &str) -> Option<DroneTexture> {
  match s {
    "bong" => Some(DroneTexture::Bong),
    "arpeggio" => Some(DroneTexture::Arpeggio),
    "thrum" => Some(DroneTexture::Thrum),
    "shimmer" => Some(DroneTexture::Shimmer),
    "reactor" => Some(DroneTexture::Reactor),
    "warpcore" => Some(DroneTexture::Warpcore),
    _ => None,
  }
}

pub fn register_from_str(s: &str) -> Option<DroneRegister> {
  match s {
    "low" => Some(DroneRegister::Low),
    "mid" => Some(DroneRegister::Mid),
    "high" => Some(DroneRegister::High),
    _ => None,
  }
}
