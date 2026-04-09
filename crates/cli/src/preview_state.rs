use crate::print;
use fundsp::prelude32::shared;
use fundsp::shared::Shared;
use serde_json::json;
use sonify_health_lib::{
  check::{DroneMetricConfig, HeartbeatCheckConfig},
  state::{DroneState, HeartbeatState},
  NoteSpec, Patch, PatchOverrides, Severity,
};
use std::collections::{HashMap, HashSet};
use std::sync::{
  atomic::{AtomicBool, AtomicUsize, Ordering},
  Arc, RwLock,
};
use tokio::sync::broadcast;

/// Patch parameter metadata matching `#[voice_param]` ranges.
pub struct PatchParamMeta {
  pub name: &'static str,
  pub description: &'static str,
  pub min: f64,
  pub max: f64,
  pub step: f64,
}

pub const PATCH_PARAMS: &[PatchParamMeta] = &[
  PatchParamMeta {
    name: "base_freq",
    description: "Root pitch in Hz. All boop notes derive from this frequency.",
    min: 100.0,
    max: 12000.0,
    step: 1.0,
  },
  PatchParamMeta {
    name: "sine_ratio",
    description: "Relative weight of the sine oscillator. Smooth, pure tone.",
    min: 0.0,
    max: 3.0,
    step: 0.01,
  },
  PatchParamMeta {
    name: "tri_ratio",
    description:
      "Relative weight of the triangle oscillator. Hollow, flute-like.",
    min: 0.0,
    max: 3.0,
    step: 0.01,
  },
  PatchParamMeta {
    name: "saw_ratio",
    description:
      "Relative weight of the sawtooth oscillator. Bright, buzzy edge.",
    min: 0.0,
    max: 3.0,
    step: 0.01,
  },
  PatchParamMeta {
    name: "attack_ms",
    description:
      "Fade-in time in milliseconds. Low = snappy click, high = soft swell.",
    min: 0.0,
    max: 500.0,
    step: 1.0,
  },
  PatchParamMeta {
    name: "release_ms",
    description:
      "Fade-out time in milliseconds. Low = staccato, high = lingering tail.",
    min: 0.0,
    max: 1000.0,
    step: 1.0,
  },
  PatchParamMeta {
    name: "chirp_ratio",
    description:
      "Pitch bend at note onset. 1.0 = none, <1 = downward, >1 = upward chirp.",
    min: 0.5,
    max: 4.0,
    step: 0.01,
  },
  PatchParamMeta {
    name: "stereo_pan",
    description: "Left/right stereo position. -1 = full left, +1 = full right.",
    min: -1.0,
    max: 1.0,
    step: 0.01,
  },
  PatchParamMeta {
    name: "reverb_mix",
    description: "Wet/dry reverb blend. 0 = fully dry, 1 = fully wet.",
    min: 0.0,
    max: 1.0,
    step: 0.01,
  },
  PatchParamMeta {
    name: "note_seed",
    description: "Seed for boop duration selection.",
    min: 0.0,
    max: 1.0,
    step: 0.01,
  },
  PatchParamMeta {
    name: "echo_delay",
    description:
      "Delay time in seconds. Short = slapback, long = distinct repeats.",
    min: 0.01,
    max: 1.0,
    step: 0.01,
  },
  PatchParamMeta {
    name: "echo_mix",
    description: "Echo wet/dry blend. 0 = no echo, 1 = full echo.",
    min: 0.0,
    max: 1.0,
    step: 0.01,
  },
  PatchParamMeta {
    name: "brightness",
    description:
      "Lowpass cutoff scaler. 1.0 = full brightness, lower = darker tone.",
    min: 0.05,
    max: 2.0,
    step: 0.01,
  },
  PatchParamMeta {
    name: "resonance",
    description:
      "Filter Q scaler. 1.0 = default resonance, lower = smoother rolloff, higher = nasal peak.",
    min: 0.1,
    max: 5.0,
    step: 0.01,
  },
  PatchParamMeta {
    name: "sub_octave",
    description:
      "Sub-oscillator mix at one octave below. 0 = off, higher = deeper body.",
    min: 0.0,
    max: 1.0,
    step: 0.01,
  },
  PatchParamMeta {
    name: "vibrato_rate",
    description: "Vibrato speed (Hz). Above ~30 Hz becomes FM synthesis.",
    min: 0.0,
    max: 200.0,
    step: 0.1,
  },
  PatchParamMeta {
    name: "vibrato_depth",
    description: "Vibrato depth (semitones). Large values produce FM sidebands.",
    min: 0.0,
    max: 12.0,
    step: 0.01,
  },
  PatchParamMeta {
    name: "tremolo_rate",
    description: "Tremolo speed (Hz)",
    min: 0.0,
    max: 20.0,
    step: 0.1,
  },
  PatchParamMeta {
    name: "tremolo_depth",
    description: "Tremolo depth (fraction)",
    min: 0.0,
    max: 1.0,
    step: 0.01,
  },
  PatchParamMeta {
    name: "amplitude",
    description: "Output amplitude. 0 = silent, 1 = full scale.",
    min: 0.0,
    max: 1.0,
    step: 0.01,
  },
  PatchParamMeta {
    name: "square_ratio",
    description:
      "Relative weight of the square oscillator. Hollow, reedy tone.",
    min: 0.0,
    max: 3.0,
    step: 0.01,
  },
  PatchParamMeta {
    name: "drive",
    description:
      "Pre-filter saturation. Low = clean, high = heavy distortion.",
    min: 0.01,
    max: 20.0,
    step: 0.1,
  },
  PatchParamMeta {
    name: "noise_mix",
    description:
      "Pink noise mixed before the filter for texture and breath.",
    min: 0.0,
    max: 1.0,
    step: 0.01,
  },
  PatchParamMeta {
    name: "crush",
    description: "Bitcrush intensity. 0 = clean, higher = grungier.",
    min: 0.0,
    max: 1.0,
    step: 0.01,
  },
  PatchParamMeta {
    name: "fm_ratio",
    description:
      "FM modulator frequency as a ratio of the carrier. 1.0 = unison, 2.0 = octave.",
    min: 0.0,
    max: 8.0,
    step: 0.01,
  },
  PatchParamMeta {
    name: "fm_depth",
    description:
      "FM modulation index. 0 = clean, higher = richer metallic warble.",
    min: 0.0,
    max: 10.0,
    step: 0.1,
  },
  PatchParamMeta {
    name: "downsample",
    description:
      "Lo-fi sample rate reduction. 0 = full fidelity, higher = crunchier.",
    min: 0.0,
    max: 1.0,
    step: 0.01,
  },
  PatchParamMeta {
    name: "sustain",
    description:
      "Body amplitude after attack. 1.0 = full level, lower = quieter sustain.",
    min: 0.0,
    max: 1.0,
    step: 0.01,
  },
];

/// Metadata for a configured drone metric.
#[derive(Clone)]
pub struct DroneMetricInfo {
  pub name: String,
  pub boops: usize,
}

/// Identifies which sound-producing entity owns a patch.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PatchOwner {
  Heartbeat,
  DroneLo(usize),
  DroneHi(usize),
}

impl PatchOwner {
  /// Parse from the WebSocket `layer` + optional `index` fields.
  pub fn from_layer_index(layer: &str, index: Option<usize>) -> Option<Self> {
    match layer {
      "heartbeat" => Some(Self::Heartbeat),
      "drone_lo" => index.map(Self::DroneLo),
      "drone_hi" => index.map(Self::DroneHi),
      _ => None,
    }
  }
}

/// Shared mutable state backing the real-time preview UI.
///
/// Both the Axum WebSocket handler and the `spawn_blocking` daemon
/// thread share an `Arc<PreviewState>`.  Patch parameters, volumes,
/// and override maps are modified via the WebSocket protocol;
/// audio-relevant `Shared<f32>` values feed directly into fundsp
/// graphs.
pub struct PreviewState {
  original_patches: HashMap<PatchOwner, Patch>,
  pub patches: RwLock<HashMap<PatchOwner, Patch>>,
  pub muted: Arc<AtomicBool>,
  /// Per-metric raw volume set by the UI (0.0..=1.0).
  pub drone_volumes: Vec<Shared>,
  /// Direct speed multiplier on phrase repetition (0.1..=10.0).
  pub drone_repeat_rates: Vec<Shared>,
  /// Power-curve exponent controlling how the metric reshapes the
  /// gap range (0.1..=5.0).
  pub drone_repeat_curves: Vec<Shared>,
  /// Base gap in seconds between drone phrases (0.0..=16.0).
  pub drone_phrase_gaps: Vec<Shared>,
  /// Power-curve exponent controlling how the metric reshapes the
  /// lo/hi patch interpolation (0.1..=5.0).
  pub drone_interp_curves: Vec<Shared>,
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
  pub locked_params: RwLock<HashMap<PatchOwner, HashSet<String>>>,
  pub locked_drones: RwLock<HashSet<usize>>,
  pub boop_specs: RwLock<Vec<NoteSpec>>,
  pub boop_pins: RwLock<Vec<bool>>,
  pub drone_boop_specs: RwLock<Vec<Vec<NoteSpec>>>,
  pub drone_boop_pins: RwLock<Vec<Vec<bool>>>,
  original_drone_boop_specs: Vec<Vec<NoteSpec>>,
  pub slot_secs: f64,
}

impl PreviewState {
  pub fn new(
    voice: Patch,
    muted: Arc<AtomicBool>,
    heartbeat_checks: &[HeartbeatCheckConfig],
    drone_metrics: &[DroneMetricConfig],
    drone_profile_overrides: &HashMap<String, (PatchOverrides, PatchOverrides)>,
    slot_secs: f64,
    initial_notes: &[NoteSpec],
    initial_drone_notes: &HashMap<String, Vec<NoteSpec>>,
  ) -> Self {
    let drone_count = drone_metrics.len();
    let check_count = heartbeat_checks.len();

    let drone_volumes: Vec<Shared> =
      (0..drone_count).map(|_| shared(1.0)).collect();
    let drone_repeat_rates: Vec<Shared> =
      (0..drone_count).map(|_| shared(1.0)).collect();
    let drone_repeat_curves: Vec<Shared> =
      (0..drone_count).map(|_| shared(1.0)).collect();
    let drone_phrase_gaps: Vec<Shared> =
      (0..drone_count).map(|_| shared(4.0)).collect();
    let drone_interp_curves: Vec<Shared> =
      (0..drone_count).map(|_| shared(1.0)).collect();
    let combined_volumes: Vec<Shared> =
      (0..drone_count).map(|_| shared(1.0)).collect();

    let (broadcast_tx, _) = broadcast::channel(256);
    let (check_log_tx, _) = broadcast::channel(256);

    let drone_infos: Vec<DroneMetricInfo> = drone_metrics
      .iter()
      .map(|m| DroneMetricInfo {
        name: m.name.clone(),
        boops: m.boops.unwrap_or(1),
      })
      .collect();

    let (initial_specs, initial_pins, boop_count) = if initial_notes.is_empty()
    {
      let patches = voice.heartbeat_notes(check_count, 1, slot_secs);
      let specs: Vec<NoteSpec> =
        patches.iter().map(Patch::to_note_spec).collect();
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

    let mut patches = HashMap::new();
    patches.insert(PatchOwner::Heartbeat, voice.clone());
    for (i, dm) in drone_metrics.iter().enumerate() {
      let (lo_voice, hi_voice) = drone_profile_overrides
        .get(&dm.name)
        .map(|(lo_ovr, hi_ovr)| {
          (
            voice.clone().with_overrides(lo_ovr),
            voice.clone().with_overrides(hi_ovr),
          )
        })
        .unwrap_or_else(|| (voice.clone(), voice.clone()));
      patches.insert(PatchOwner::DroneLo(i), lo_voice);
      patches.insert(PatchOwner::DroneHi(i), hi_voice);
    }

    // Per-drone boop specs: use config notes (pinned) when present,
    // otherwise generate algorithmically (unpinned).
    let (drone_specs_init, drone_pins_init): (
      Vec<Vec<NoteSpec>>,
      Vec<Vec<bool>>,
    ) = drone_metrics
      .iter()
      .enumerate()
      .map(|(i, dm)| {
        if let Some(notes) = initial_drone_notes.get(&dm.name) {
          let pins = vec![true; notes.len()];
          (notes.clone(), pins)
        } else {
          let drone_voice = patches
            .get(&PatchOwner::DroneLo(i))
            .cloned()
            .unwrap_or_else(|| voice.clone());
          let info = &drone_infos[i];
          let patches = drone_voice.drone_notes(i, info.boops, slot_secs);
          let specs: Vec<NoteSpec> =
            patches.iter().map(Patch::to_note_spec).collect();
          let pins = vec![false; specs.len()];
          (specs, pins)
        }
      })
      .unzip();

    Self {
      original_patches: patches.clone(),
      patches: RwLock::new(patches),
      muted,
      drone_volumes,
      drone_repeat_rates,
      drone_repeat_curves,
      drone_phrase_gaps,
      drone_interp_curves,
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
      original_drone_boop_specs: drone_specs_init.clone(),
      drone_boop_specs: RwLock::new(drone_specs_init),
      drone_boop_pins: RwLock::new(drone_pins_init),
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

  /// Recompute materialized boop specs from the current patch,
  /// preserving pinned entries.
  pub fn recompute_boop_specs(&self) {
    let boops_per_check = self.boop_count.load(Ordering::Relaxed);
    let total = boops_per_check * self.check_names.len();
    let patches = self.patches.read().unwrap();
    let voice = &patches[&PatchOwner::Heartbeat];
    let check_count = self.check_names.len();
    let fresh: Vec<NoteSpec> = voice
      .heartbeat_notes(check_count, boops_per_check, self.slot_secs)
      .iter()
      .map(Patch::to_note_spec)
      .collect();

    let mut specs = self.boop_specs.write().unwrap();
    let mut pins = self.boop_pins.write().unwrap();

    // Resize pins to match new total, new entries unpinned.
    pins.resize(total, false);

    let merged: Vec<NoteSpec> = fresh
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

  /// Recompute materialized drone boop specs for a single drone,
  /// preserving pinned entries.
  pub fn recompute_drone_specs(&self, index: usize) {
    let (voice, info) = {
      let patches = self.patches.read().unwrap();
      let infos = self.drone_infos.read().unwrap();
      let voice = patches
        .get(&PatchOwner::DroneLo(index))
        .cloned()
        .unwrap_or_else(|| patches[&PatchOwner::Heartbeat].clone());
      let info = match infos.get(index) {
        Some(i) => i.clone(),
        None => return,
      };
      (voice, info)
    };

    let fresh: Vec<NoteSpec> = voice
      .drone_notes(index, info.boops, self.slot_secs)
      .iter()
      .map(Patch::to_note_spec)
      .collect();

    let mut all_specs = self.drone_boop_specs.write().unwrap();
    let mut all_pins = self.drone_boop_pins.write().unwrap();

    if let (Some(specs), Some(pins)) =
      (all_specs.get_mut(index), all_pins.get_mut(index))
    {
      pins.resize(fresh.len(), false);
      let merged: Vec<NoteSpec> = fresh
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
      pins.truncate(merged.len());
      *specs = merged;
    }
  }

  /// Compute the effective drone patch at a given metric value by
  /// interpolating between the lo and hi profiles using the
  /// per-drone interpolation curve.
  pub fn effective_drone_patch(&self, index: usize, metric: f32) -> Patch {
    let patches = self.patches.read().unwrap();
    let lo = patches
      .get(&PatchOwner::DroneLo(index))
      .cloned()
      .unwrap_or_else(|| patches[&PatchOwner::Heartbeat].clone());
    let hi = patches
      .get(&PatchOwner::DroneHi(index))
      .cloned()
      .unwrap_or_else(|| lo.clone());
    let curve = self
      .drone_interp_curves
      .get(index)
      .map(|s| s.value() as f64)
      .unwrap_or(1.0);
    let t = (metric as f64).clamp(0.0, 1.0).powf(curve);
    Patch::lerp(&lo, &hi, t)
  }

  /// Build the full state snapshot JSON sent on connect and on
  /// `get_state` / `revert_all`.
  pub fn state_snapshot(&self) -> String {
    let patches = self.patches.read().unwrap();
    let heartbeat_voice = &patches[&PatchOwner::Heartbeat];
    let hb_overrides = self.heartbeat_overrides.read().unwrap();
    let drone_overrides = self.drone_overrides.read().unwrap();
    let drone_infos = self.drone_infos.read().unwrap();
    let locked = self.locked_params.read().unwrap();
    let specs = self.boop_specs.read().unwrap();
    let pins = self.boop_pins.read().unwrap();

    let heartbeat_voice_json = patch_to_json(heartbeat_voice);

    let heartbeat_locked: Vec<_> = locked
      .get(&PatchOwner::Heartbeat)
      .map(|s| s.iter().map(|p| json!(p)).collect())
      .unwrap_or_default();

    let voice_params_json: Vec<_> = PATCH_PARAMS
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

    let base_freq_meta =
      PATCH_PARAMS.iter().find(|p| p.name == "base_freq").unwrap();

    let drone_specs = self.drone_boop_specs.read().unwrap();
    let drone_pins = self.drone_boop_pins.read().unwrap();

    let drones_json: Vec<_> = drone_infos
      .iter()
      .enumerate()
      .map(|(i, info)| {
        let voice_lo = patches
          .get(&PatchOwner::DroneLo(i))
          .map(patch_to_json)
          .unwrap_or(json!({}));
        let voice_hi = patches
          .get(&PatchOwner::DroneHi(i))
          .map(patch_to_json)
          .unwrap_or(json!({}));
        let lo_locked: Vec<_> = locked
          .get(&PatchOwner::DroneLo(i))
          .map(|s| s.iter().map(|p| json!(p)).collect())
          .unwrap_or_default();
        let hi_locked: Vec<_> = locked
          .get(&PatchOwner::DroneHi(i))
          .map(|s| s.iter().map(|p| json!(p)).collect())
          .unwrap_or_default();
        let d_specs = drone_specs.get(i).cloned().unwrap_or_default();
        let d_pins = drone_pins.get(i).cloned().unwrap_or_default();
        let specs_json: Vec<_> = d_specs
          .iter()
          .enumerate()
          .map(|(j, spec)| {
            json!({
              "freq": spec.freq,
              "duration": spec.duration,
              "pinned": d_pins.get(j).copied().unwrap_or(false),
            })
          })
          .collect();
        json!({
          "name": info.name,
          "value": self.drone_state.metrics[i].value(),
          "volume": self.drone_volumes[i].value(),
          "repeat_rate": self.drone_repeat_rates[i].value(),
          "repeat_curve": self.drone_repeat_curves[i].value(),
          "phrase_gap": self.drone_phrase_gaps[i].value(),
          "interp_curve": self.drone_interp_curves[i].value(),
          "boops": info.boops,
          "overridden": drone_overrides[i].is_some(),
          "patch_lo": voice_lo,
          "patch_hi": voice_hi,
          "locked_params_lo": lo_locked,
          "locked_params_hi": hi_locked,
          "specs": specs_json,
          "spec_ranges": {
            "freq_min": base_freq_meta.min / 2.0,
            "freq_max": base_freq_meta.max,
            "freq_step": 1.0,
            "duration_min": 0.05,
            "duration_max": self.slot_secs,
            "duration_step": 0.01,
          },
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

    json!({
      "type": "state",
      "patch": heartbeat_voice_json,
      "locked_params": heartbeat_locked,
      "patch_params": voice_params_json,
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

  /// Collect per-drone specs as (name, specs) pairs for export.
  fn drone_notes_for_export(&self) -> Vec<(String, Vec<NoteSpec>)> {
    let drone_infos = self.drone_infos.read().unwrap();
    let all_specs = self.drone_boop_specs.read().unwrap();
    drone_infos
      .iter()
      .enumerate()
      .map(|(i, info)| {
        let specs = all_specs.get(i).cloned().unwrap_or_default();
        (info.name.clone(), specs)
      })
      .collect()
  }

  /// Format all patches as a TOML block.
  pub fn export_toml(&self) -> String {
    let patches = self.patches.read().unwrap();
    let heartbeat_voice = &patches[&PatchOwner::Heartbeat];
    let drone_infos = self.drone_infos.read().unwrap();
    let drone_profiles: Vec<_> = drone_infos
      .iter()
      .enumerate()
      .map(|(i, info)| {
        let lo = patches
          .get(&PatchOwner::DroneLo(i))
          .cloned()
          .unwrap_or_else(|| heartbeat_voice.clone());
        let hi = patches
          .get(&PatchOwner::DroneHi(i))
          .cloned()
          .unwrap_or_else(|| heartbeat_voice.clone());
        (info.name.clone(), lo, hi)
      })
      .collect();
    let specs = self.boop_specs.read().unwrap();
    let drone_notes = self.drone_notes_for_export();
    print::format_toml(heartbeat_voice, &drone_profiles, &specs, &drone_notes)
  }

  /// Format all patches as a JSON object.
  pub fn export_json(&self) -> String {
    let patches = self.patches.read().unwrap();
    let heartbeat_voice = &patches[&PatchOwner::Heartbeat];
    let drone_infos = self.drone_infos.read().unwrap();
    let drone_profiles: Vec<_> = drone_infos
      .iter()
      .enumerate()
      .map(|(i, info)| {
        let lo = patches
          .get(&PatchOwner::DroneLo(i))
          .cloned()
          .unwrap_or_else(|| heartbeat_voice.clone());
        let hi = patches
          .get(&PatchOwner::DroneHi(i))
          .cloned()
          .unwrap_or_else(|| heartbeat_voice.clone());
        (info.name.clone(), lo, hi)
      })
      .collect();
    let specs = self.boop_specs.read().unwrap();
    let drone_notes = self.drone_notes_for_export();
    print::format_json(heartbeat_voice, &drone_profiles, &specs, &drone_notes)
  }

  /// Format all patches as a Nix attribute set.
  pub fn export_nix(&self) -> String {
    let patches = self.patches.read().unwrap();
    let heartbeat_voice = &patches[&PatchOwner::Heartbeat];
    let drone_infos = self.drone_infos.read().unwrap();
    let drone_profiles: Vec<_> = drone_infos
      .iter()
      .enumerate()
      .map(|(i, info)| {
        let lo = patches
          .get(&PatchOwner::DroneLo(i))
          .cloned()
          .unwrap_or_else(|| heartbeat_voice.clone());
        let hi = patches
          .get(&PatchOwner::DroneHi(i))
          .cloned()
          .unwrap_or_else(|| heartbeat_voice.clone());
        (info.name.clone(), lo, hi)
      })
      .collect();
    let specs = self.boop_specs.read().unwrap();
    let drone_notes = self.drone_notes_for_export();
    print::format_nix(heartbeat_voice, &drone_profiles, &specs, &drone_notes)
  }

  /// Reset everything to startup values.  Locked patch params
  /// and locked drones survive the reset; boop pins are cleared.
  pub fn revert(&self) {
    // Snapshot per-entity locked param values before resetting.
    let locked = self.locked_params.read().unwrap().clone();
    let locked_values: HashMap<PatchOwner, Vec<(String, f64)>> = {
      let patches = self.patches.read().unwrap();
      locked
        .iter()
        .map(|(owner, params)| {
          let vals: Vec<(String, f64)> = patches
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
    let locked_drone_snapshots: Vec<(
      usize,
      DroneMetricInfo,
      f32,
      f32,
      f32,
      f32,
      f32,
    )> = {
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
              self.drone_repeat_curves[i].value(),
              self.drone_phrase_gaps[i].value(),
              self.drone_interp_curves[i].value(),
            )
          })
        })
        .collect()
    };

    *self.patches.write().unwrap() = self.original_patches.clone();

    // Restore per-entity locked param values.
    {
      let mut patches = self.patches.write().unwrap();
      for (owner, vals) in &locked_values {
        if let Some(voice) = patches.get_mut(owner) {
          for (name, value) in vals {
            set_patch_param(voice, name, *value);
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
    for rc in &self.drone_repeat_curves {
      rc.set_value(1.0);
    }
    for pg in &self.drone_phrase_gaps {
      pg.set_value(4.0);
    }
    for ic in &self.drone_interp_curves {
      ic.set_value(1.0);
    }

    // Restore locked drone settings.
    {
      let mut infos = self.drone_infos.write().unwrap();
      for (i, info, vol, rate, curve, gap, interp) in &locked_drone_snapshots {
        if let Some(entry) = infos.get_mut(*i) {
          *entry = info.clone();
        }
        self.drone_volumes[*i].set_value(*vol);
        self.drone_repeat_rates[*i].set_value(*rate);
        self.drone_repeat_curves[*i].set_value(*curve);
        self.drone_phrase_gaps[*i].set_value(*gap);
        self.drone_interp_curves[*i].set_value(*interp);
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

    // Clear boop pins and recompute specs from reverted patch.
    {
      let mut pins = self.boop_pins.write().unwrap();
      pins.iter_mut().for_each(|p| *p = false);
    }
    self.recompute_boop_specs();

    // Reset drone boop specs: locked drones keep their specs,
    // unlocked drones revert to originals then recompute.
    {
      let mut all_specs = self.drone_boop_specs.write().unwrap();
      let mut all_pins = self.drone_boop_pins.write().unwrap();
      for i in 0..all_specs.len() {
        if !locked_drone_indices.contains(&i) {
          if let Some(original) = self.original_drone_boop_specs.get(i) {
            all_specs[i] = original.clone();
            all_pins[i] = vec![false; original.len()];
          }
        }
      }
    }
    let drone_count = self.drone_infos.read().unwrap().len();
    for i in 0..drone_count {
      if !locked_drone_indices.contains(&i) {
        self.recompute_drone_specs(i);
      }
    }
  }
}

// -- Helpers -----------------------------------------------------------------

pub fn get_voice_param(voice: &Patch, param: &str) -> Option<f64> {
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
    "square_ratio" => Some(voice.square_ratio),
    "drive" => Some(voice.drive),
    "noise_mix" => Some(voice.noise_mix),
    "crush" => Some(voice.crush),
    "fm_ratio" => Some(voice.fm_ratio),
    "fm_depth" => Some(voice.fm_depth),
    "downsample" => Some(voice.downsample),
    "sustain" => Some(voice.sustain),
    _ => None,
  }
}

pub fn set_patch_param(voice: &mut Patch, param: &str, value: f64) -> bool {
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
    "square_ratio" => voice.square_ratio = value,
    "drive" => voice.drive = value,
    "noise_mix" => voice.noise_mix = value,
    "crush" => voice.crush = value,
    "fm_ratio" => voice.fm_ratio = value,
    "fm_depth" => voice.fm_depth = value,
    "downsample" => voice.downsample = value,
    "sustain" => voice.sustain = value,
    _ => return false,
  }
  true
}

fn patch_to_json(voice: &Patch) -> serde_json::Value {
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
    "square_ratio": voice.square_ratio,
    "drive": voice.drive,
    "noise_mix": voice.noise_mix,
    "crush": voice.crush,
    "fm_ratio": voice.fm_ratio,
    "fm_depth": voice.fm_depth,
    "downsample": voice.downsample,
    "sustain": voice.sustain,
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
    Patch,
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
    patch: serde_json::Map<String, serde_json::Value>,
    locked_params: Vec<String>,
    patch_params: Vec<PatchParamContract>,
    muted: bool,
    master_volume: f64,
    heartbeat_volume: f64,
    heartbeat_loop: bool,
    boop_count: u64,
    checks: Vec<CheckContract>,
    drones: Vec<DroneContract>,
    locked_drones: Vec<u64>,
    boop_specs: Vec<NoteSpecContract>,
    boop_spec_ranges: NoteSpecRangesContract,
  }

  #[derive(Deserialize)]
  struct PatchParamContract {
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

  /// Mirrors the Elm `DroneInfo` decoder.
  #[derive(Deserialize)]
  struct DroneContract {
    name: String,
    value: f64,
    volume: f64,
    repeat_rate: f64,
    repeat_curve: f64,
    phrase_gap: f64,
    interp_curve: f64,
    boops: u64,
    overridden: bool,
    patch_lo: serde_json::Map<String, serde_json::Value>,
    patch_hi: serde_json::Map<String, serde_json::Value>,
    locked_params_lo: Vec<String>,
    locked_params_hi: Vec<String>,
    specs: Vec<NoteSpecContract>,
    spec_ranges: NoteSpecRangesContract,
  }

  #[derive(Deserialize)]
  struct NoteSpecContract {
    freq: f64,
    duration: f64,
    pinned: bool,
  }

  #[derive(Deserialize)]
  struct NoteSpecRangesContract {
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
    boops: u64,
  }

  fn test_preview() -> PreviewState {
    let voice = Patch::from_hostname("test");
    let checks = vec![HeartbeatCheckConfig {
      name: "cpu".to_string(),
      command: "echo healthy".to_string(),
      result_mode: ResultMode::ExitCode,
    }];
    let drones = vec![DroneMetricConfig {
      name: "load".to_string(),
      command: "echo 0.5".to_string(),
      result_mode: ResultMode::Stdout,
      boops: Some(2),
    }];
    PreviewState::new(
      voice,
      Arc::new(AtomicBool::new(false)),
      &checks,
      &drones,
      &HashMap::new(), // drone_profile_overrides
      4.0,
      &[],
      &HashMap::new(),
    )
  }

  #[test]
  fn state_snapshot_matches_frontend_contract() {
    let preview = test_preview();
    let json = preview.state_snapshot();
    let state: StateContract = serde_json::from_str(&json)
      .expect("state_snapshot JSON does not match the Elm frontend contract");

    assert_eq!(state.msg_type, "state");
    assert!(!state.patch_params.is_empty());
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
      boops: 3,
    };
    let msg = serde_json::json!({
      "type": "drone_config_changed",
      "index": 0,
      "boops": info.boops,
    })
    .to_string();

    let parsed: DroneConfigChangedContract = serde_json::from_str(&msg).expect(
      "drone_config_changed JSON does not match the Elm frontend \
         contract",
    );

    assert_eq!(parsed.msg_type, "drone_config_changed");
    assert_eq!(parsed.boops, 3);
  }

  #[test]
  fn heartbeat_voice_change_does_not_affect_drone() {
    let preview = test_preview();
    let original_drone_freq = {
      let patches = preview.patches.read().unwrap();
      patches[&PatchOwner::DroneLo(0)].base_freq
    };
    {
      let mut patches = preview.patches.write().unwrap();
      let hb = patches.get_mut(&PatchOwner::Heartbeat).unwrap();
      set_patch_param(hb, "base_freq", 999.0);
    }
    let patches = preview.patches.read().unwrap();
    assert!(
      (patches[&PatchOwner::Heartbeat].base_freq - 999.0).abs() < f64::EPSILON,
    );
    assert!(
      (patches[&PatchOwner::DroneLo(0)].base_freq - original_drone_freq).abs()
        < f64::EPSILON,
    );
  }

  #[test]
  fn drone_voice_change_does_not_affect_heartbeat() {
    let preview = test_preview();
    let original_hb_freq = {
      let patches = preview.patches.read().unwrap();
      patches[&PatchOwner::Heartbeat].base_freq
    };
    {
      let mut patches = preview.patches.write().unwrap();
      let drone = patches.get_mut(&PatchOwner::DroneLo(0)).unwrap();
      set_patch_param(drone, "base_freq", 777.0);
    }
    let patches = preview.patches.read().unwrap();
    assert!(
      (patches[&PatchOwner::DroneLo(0)].base_freq - 777.0).abs() < f64::EPSILON,
    );
    assert!(
      (patches[&PatchOwner::Heartbeat].base_freq - original_hb_freq).abs()
        < f64::EPSILON,
    );
  }

  #[test]
  fn per_entity_lock_survives_revert() {
    let preview = test_preview();
    // Change heartbeat patch and lock it.
    {
      let mut patches = preview.patches.write().unwrap();
      let hb = patches.get_mut(&PatchOwner::Heartbeat).unwrap();
      set_patch_param(hb, "base_freq", 555.0);
    }
    preview
      .locked_params
      .write()
      .unwrap()
      .entry(PatchOwner::Heartbeat)
      .or_default()
      .insert("base_freq".to_string());

    preview.revert();

    let patches = preview.patches.read().unwrap();
    assert!(
      (patches[&PatchOwner::Heartbeat].base_freq - 555.0).abs() < f64::EPSILON,
      "Locked heartbeat base_freq should survive revert",
    );
  }

  #[test]
  fn state_snapshot_encodes_all_patches() {
    let preview = test_preview();
    // Set heartbeat and drone lo/hi to different values.
    {
      let mut patches = preview.patches.write().unwrap();
      set_patch_param(
        patches.get_mut(&PatchOwner::Heartbeat).unwrap(),
        "base_freq",
        111.0,
      );
      set_patch_param(
        patches.get_mut(&PatchOwner::DroneLo(0)).unwrap(),
        "base_freq",
        222.0,
      );
      set_patch_param(
        patches.get_mut(&PatchOwner::DroneHi(0)).unwrap(),
        "base_freq",
        333.0,
      );
    }
    let json = preview.state_snapshot();
    let state: StateContract =
      serde_json::from_str(&json).expect("state_snapshot should decode");
    let hb_freq = state.patch["base_freq"].as_f64().unwrap();
    let lo_freq = state.drones[0].patch_lo["base_freq"].as_f64().unwrap();
    let hi_freq = state.drones[0].patch_hi["base_freq"].as_f64().unwrap();
    assert!((hb_freq - 111.0).abs() < f64::EPSILON);
    assert!((lo_freq - 222.0).abs() < f64::EPSILON);
    assert!((hi_freq - 333.0).abs() < f64::EPSILON);
  }

  /// Load every example config, build a PreviewState, and verify
  /// the state snapshot deserializes through the frontend contract.
  /// Catches dead examples and backend/frontend JSON drift.
  #[test]
  fn example_configs_produce_valid_snapshots() {
    let examples_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
      .parent()
      .unwrap()
      .parent()
      .unwrap()
      .join("examples");
    let mut tested = 0;
    for entry in
      std::fs::read_dir(&examples_dir).expect("examples directory should exist")
    {
      let path = entry.unwrap().path();
      if path.extension().map(|e| e == "toml").unwrap_or(false) {
        let config = crate::config::Config::from_args(
          None,
          None,
          None,
          None,
          Some(&path),
          None,
          None,
          None,
          None,
        )
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));

        let base_voice = Patch::from_hostname("test");
        let voice = match &config.daemon.heartbeat_patch_overrides {
          Some(ovr) => base_voice.with_overrides(ovr),
          None => base_voice,
        };

        let preview = PreviewState::new(
          voice,
          Arc::new(AtomicBool::new(false)),
          &config.daemon.heartbeat_checks,
          &config.daemon.drone_metrics,
          &config.daemon.drone_profile_overrides,
          config.daemon.timing.slot_duration_secs,
          &config.daemon.heartbeat_notes,
          &config.daemon.drone_notes,
        );

        let json = preview.state_snapshot();
        let state: StateContract =
          serde_json::from_str(&json).unwrap_or_else(|e| {
            panic!(
              "{}: state_snapshot does not match frontend contract: {e}",
              path.display()
            )
          });

        assert_eq!(
          state.checks.len(),
          config.daemon.heartbeat_checks.len(),
          "{}: check count mismatch",
          path.display()
        );
        assert_eq!(
          state.drones.len(),
          config.daemon.drone_metrics.len(),
          "{}: drone count mismatch",
          path.display()
        );
        for (i, drone) in state.drones.iter().enumerate() {
          assert_eq!(
            drone.name,
            config.daemon.drone_metrics[i].name,
            "{}: drone[{i}] name mismatch",
            path.display()
          );
          assert!(
            !drone.patch_lo.is_empty(),
            "{}: drone[{i}] patch_lo is empty",
            path.display()
          );
          assert!(
            !drone.patch_hi.is_empty(),
            "{}: drone[{i}] patch_hi is empty",
            path.display()
          );
        }
        tested += 1;
      }
    }
    assert!(
      tested > 0,
      "No example configs found in {}",
      examples_dir.display()
    );
  }
}
