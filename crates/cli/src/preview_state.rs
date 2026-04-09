use crate::print;
use fundsp::prelude32::shared;
use fundsp::shared::Shared;
use serde_json::json;
use sonify_health_lib::{
  check::CheckConfig, state::CheckState, NoteSpec, Patch, PatchOverrides,
};
use std::collections::{HashMap, HashSet};
use std::sync::{
  atomic::{AtomicBool, AtomicUsize, Ordering},
  Arc, RwLock,
};
use tokio::sync::broadcast;

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
  /// Power-curve exponent controlling how the metric reshapes the
  /// lo/hi patch interpolation (0.1..=5.0).
  pub drone_interp_curves: Vec<Shared>,
  /// `mute_factor * per_metric_volume`, wired into audio graphs.
  pub combined_volumes: Vec<Shared>,
  pub master_volume: Shared,
  pub heartbeat_volume: Shared,
  pub effective_heartbeat_volume: Shared,
  pub heartbeat_state: Arc<CheckState>,
  pub drone_state: Arc<CheckState>,
  pub heartbeat_overrides: RwLock<Vec<Option<f32>>>,
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
    heartbeat_checks: &[CheckConfig],
    drone_metrics: &[CheckConfig],
    drone_profile_overrides: &HashMap<String, (PatchOverrides, PatchOverrides)>,
    slot_secs: f64,
    initial_notes: &[NoteSpec],
    initial_drone_notes: &HashMap<String, Vec<NoteSpec>>,
  ) -> Self {
    let drone_count = drone_metrics.len();
    let check_count = heartbeat_checks.len();

    let drone_interp_curves: Vec<Shared> = drone_metrics
      .iter()
      .map(|m| shared(m.interp_curve.unwrap_or(1.0) as f32))
      .collect();
    // Volume is now a Patch parameter; initialize combined volumes
    // to 1.0 and let the play loop update from the interpolated patch.
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
      drone_interp_curves,
      combined_volumes,
      master_volume: shared(1.0),
      heartbeat_volume: shared(1.0),
      effective_heartbeat_volume: shared(1.0),
      heartbeat_state: Arc::new(CheckState::new(check_count)),
      drone_state: Arc::new(CheckState::new(drone_count)),
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
  /// volume, and the given raw per-check volume.
  pub fn update_combined_volume_with(&self, index: usize, raw: f32) {
    let mute_factor = if self.muted.load(Ordering::Relaxed) {
      0.0
    } else {
      1.0
    };
    let master = self.master_volume.value();
    if let Some(cv) = self.combined_volumes.get(index) {
      cv.set_value(mute_factor * master * raw);
    }
  }

  /// Alias used by the daemon mute-toggle path: re-derive volume
  /// from the current lo-profile patch.
  pub fn update_combined_volume(&self, index: usize) {
    let raw = {
      let patches = self.patches.read().unwrap();
      patches
        .get(&PatchOwner::DroneLo(index))
        .map(|p| p.volume as f32)
        .unwrap_or(1.0)
    };
    self.update_combined_volume_with(index, raw);
  }

  /// Update every combined volume (after mute toggle or master
  /// volume change).
  pub fn update_all_combined_volumes(&self) {
    for i in 0..self.combined_volumes.len() {
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
    let drone_specs_all = self.drone_boop_specs.read().unwrap();
    let drone_pins_all = self.drone_boop_pins.read().unwrap();
    let boops_per_check = self.boop_count.load(Ordering::Relaxed);

    let heartbeat_voice_json =
      serde_json::to_value(heartbeat_voice).unwrap_or_default();

    let heartbeat_locked: Vec<_> = locked
      .get(&PatchOwner::Heartbeat)
      .map(|s| s.iter().map(|p| json!(p)).collect())
      .unwrap_or_default();

    let voice_params_json: Vec<_> = Patch::PARAMS
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

    let freq_meta = Patch::PARAMS.iter().find(|p| p.name == "freq").unwrap();

    let common_spec_ranges = json!({
      "freq_min": freq_meta.min / 2.0,
      "freq_max": freq_meta.max,
      "freq_step": 1.0,
      "duration_min": 0.05,
      "duration_max": self.slot_secs,
      "duration_step": 0.01,
    });

    // Build unified checks array: heartbeat checks first, then
    // drones.  Each entry carries the full set of fields so the
    // frontend can treat them uniformly.
    let heartbeat_checks_json: Vec<_> = self
      .check_names
      .iter()
      .enumerate()
      .map(|(i, name)| {
        let start = i * boops_per_check;
        let end = (start + boops_per_check).min(specs.len());
        let check_specs: Vec<_> = if start < specs.len() {
          specs[start..end]
            .iter()
            .enumerate()
            .map(|(j, spec)| {
              json!({
                "freq": spec.freq,
                "duration": spec.duration,
                "pinned": pins
                  .get(start + j)
                  .copied()
                  .unwrap_or(false),
              })
            })
            .collect()
        } else {
          vec![]
        };
        json!({
          "name": name,
          "kind": "heartbeat",
          "check_index": i,
          "value": self.heartbeat_state.metrics[i].value(),
          "interp_curve": 1.0,
          "boops": boops_per_check,
          "overridden": hb_overrides[i].is_some(),
          "patch_lo": heartbeat_voice_json.clone(),
          "patch_hi": heartbeat_voice_json.clone(),
          "locked_params_lo": heartbeat_locked.clone(),
          "locked_params_hi": heartbeat_locked.clone(),
          "specs": check_specs,
          "spec_ranges": common_spec_ranges.clone(),
        })
      })
      .collect();

    let drone_checks_json: Vec<_> = drone_infos
      .iter()
      .enumerate()
      .map(|(i, info)| {
        let voice_lo = patches
          .get(&PatchOwner::DroneLo(i))
          .and_then(|p| serde_json::to_value(p).ok())
          .unwrap_or(json!({}));
        let voice_hi = patches
          .get(&PatchOwner::DroneHi(i))
          .and_then(|p| serde_json::to_value(p).ok())
          .unwrap_or(json!({}));
        let lo_locked: Vec<_> = locked
          .get(&PatchOwner::DroneLo(i))
          .map(|s| s.iter().map(|p| json!(p)).collect())
          .unwrap_or_default();
        let hi_locked: Vec<_> = locked
          .get(&PatchOwner::DroneHi(i))
          .map(|s| s.iter().map(|p| json!(p)).collect())
          .unwrap_or_default();
        let d_specs = drone_specs_all.get(i).cloned().unwrap_or_default();
        let d_pins = drone_pins_all.get(i).cloned().unwrap_or_default();
        let specs_json: Vec<_> = d_specs
          .iter()
          .enumerate()
          .map(|(j, spec)| {
            json!({
              "freq": spec.freq,
              "duration": spec.duration,
              "pinned": d_pins
                .get(j)
                .copied()
                .unwrap_or(false),
            })
          })
          .collect();
        json!({
          "name": info.name,
          "kind": "drone",
          "check_index": i,
          "value": self.drone_state.metrics[i].value(),
          "interp_curve": self.drone_interp_curves[i].value(),
          "boops": info.boops,
          "overridden": drone_overrides[i].is_some(),
          "patch_lo": voice_lo,
          "patch_hi": voice_hi,
          "locked_params_lo": lo_locked,
          "locked_params_hi": hi_locked,
          "specs": specs_json,
          "spec_ranges": common_spec_ranges.clone(),
        })
      })
      .collect();

    let mut checks_json = heartbeat_checks_json;
    checks_json.extend(drone_checks_json);

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
      "locked_drones": locked_drones_json,
      "boop_specs": boop_specs_json,
      "boop_spec_ranges": common_spec_ranges,
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
                  voice.get_param(name).map(|v| (name.clone(), v))
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
    let locked_drone_snapshots: Vec<(usize, DroneMetricInfo, f32)> = {
      let infos = self.drone_infos.read().unwrap();
      locked_drone_indices
        .iter()
        .filter_map(|&i| {
          infos
            .get(i)
            .map(|info| (i, info.clone(), self.drone_interp_curves[i].value()))
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
            voice.set_param(name, *value);
          }
        }
      }
    }

    *self.drone_infos.write().unwrap() = self.original_drone_infos.clone();

    // Reset interp curves to 1.0 (default).
    for ic in &self.drone_interp_curves {
      ic.set_value(1.0);
    }

    // Restore locked drone settings.
    {
      let mut infos = self.drone_infos.write().unwrap();
      for (i, info, interp) in &locked_drone_snapshots {
        if let Some(entry) = infos.get_mut(*i) {
          *entry = info.clone();
        }
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

/// Convert a normalized metric (0.0–1.0) to a display label.
/// 0.0→"healthy", 0.5→"degraded", 1.0→"down".
pub fn metric_label(value: f32) -> &'static str {
  match (value * 2.0).round() as u8 {
    0 => "healthy",
    1 => "degraded",
    _ => "down",
  }
}

#[cfg(test)]
#[allow(dead_code)]
mod tests {
  use super::*;
  use serde::Deserialize;
  use sonify_health_lib::{
    check::{CheckConfig, ResultMode},
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

  /// Unified check contract — heartbeat and drone checks share
  /// the same shape.
  #[derive(Deserialize)]
  struct CheckContract {
    name: String,
    kind: String,
    check_index: u64,
    value: f64,
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

  #[derive(Deserialize)]
  struct ParamChangedContract {
    #[serde(rename = "type")]
    msg_type: String,
    layer: String,
    param: String,
    value: f64,
    index: Option<u64>,
  }

  #[derive(Deserialize)]
  struct MuteChangedContract {
    #[serde(rename = "type")]
    msg_type: String,
    muted: bool,
  }

  #[derive(Deserialize)]
  struct VolumeChangedContract {
    #[serde(rename = "type")]
    msg_type: String,
    layer: String,
    volume: f64,
    index: Option<u64>,
  }

  #[derive(Deserialize)]
  struct OverrideChangedContract {
    #[serde(rename = "type")]
    msg_type: String,
    layer: String,
    index: u64,
    value: Option<f64>,
    overridden: bool,
  }

  #[derive(Deserialize)]
  struct HeartbeatLoopChangedContract {
    #[serde(rename = "type")]
    msg_type: String,
    enabled: bool,
  }

  #[derive(Deserialize)]
  struct BoopCountChangedContract {
    #[serde(rename = "type")]
    msg_type: String,
    count: u64,
  }

  #[derive(Deserialize)]
  struct DroneInterpCurveChangedContract {
    #[serde(rename = "type")]
    msg_type: String,
    index: u64,
    curve: f64,
  }

  #[derive(Deserialize)]
  struct CheckLogContract {
    #[serde(rename = "type")]
    msg_type: String,
    timestamp: f64,
    layer: String,
    name: String,
    result: String,
    overridden: bool,
  }

  #[derive(Deserialize)]
  struct PatchExportContract {
    #[serde(rename = "type")]
    msg_type: String,
    toml: String,
    json: String,
    nix: String,
  }

  #[derive(Deserialize)]
  struct LockedParamsChangedContract {
    #[serde(rename = "type")]
    msg_type: String,
    layer: String,
    params: Vec<String>,
    index: Option<u64>,
  }

  #[derive(Deserialize)]
  struct LockedDronesChangedContract {
    #[serde(rename = "type")]
    msg_type: String,
    indices: Vec<u64>,
  }

  #[derive(Deserialize)]
  struct BoopSpecsChangedContract {
    #[serde(rename = "type")]
    msg_type: String,
    specs: Vec<NoteSpecContract>,
  }

  #[derive(Deserialize)]
  struct DroneSpecsChangedContract {
    #[serde(rename = "type")]
    msg_type: String,
    index: u64,
    specs: Vec<NoteSpecContract>,
  }

  #[derive(Deserialize)]
  struct ImportErrorContract {
    #[serde(rename = "type")]
    msg_type: String,
    message: String,
  }

  fn test_preview() -> PreviewState {
    let voice = Patch::from_hostname("test");
    let checks = vec![CheckConfig {
      name: "cpu".to_string(),
      command: "echo healthy".to_string(),
      result_mode: ResultMode::ExitCodeSeverity,
      boops: None,
      interp_curve: None,
    }];
    let drones = vec![CheckConfig {
      name: "load".to_string(),
      command: "echo 0.5".to_string(),
      result_mode: ResultMode::Stdout,
      boops: Some(2),
      interp_curve: None,
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
    // Unified checks: 1 heartbeat ("cpu") + 1 drone ("load").
    assert_eq!(state.checks.len(), 2);
    assert_eq!(state.checks[0].name, "cpu");
    assert_eq!(state.checks[0].kind, "heartbeat");
    assert_eq!(state.checks[1].name, "load");
    assert_eq!(state.checks[1].kind, "drone");
    assert_eq!(state.checks[1].boops, 2);
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
      patches[&PatchOwner::DroneLo(0)].freq
    };
    {
      let mut patches = preview.patches.write().unwrap();
      let hb = patches.get_mut(&PatchOwner::Heartbeat).unwrap();
      hb.set_param("freq", 999.0);
    }
    let patches = preview.patches.read().unwrap();
    assert!(
      (patches[&PatchOwner::Heartbeat].freq - 999.0).abs() < f64::EPSILON,
    );
    assert!(
      (patches[&PatchOwner::DroneLo(0)].freq - original_drone_freq).abs()
        < f64::EPSILON,
    );
  }

  #[test]
  fn drone_voice_change_does_not_affect_heartbeat() {
    let preview = test_preview();
    let original_hb_freq = {
      let patches = preview.patches.read().unwrap();
      patches[&PatchOwner::Heartbeat].freq
    };
    {
      let mut patches = preview.patches.write().unwrap();
      let drone = patches.get_mut(&PatchOwner::DroneLo(0)).unwrap();
      drone.set_param("freq", 777.0);
    }
    let patches = preview.patches.read().unwrap();
    assert!(
      (patches[&PatchOwner::DroneLo(0)].freq - 777.0).abs() < f64::EPSILON,
    );
    assert!(
      (patches[&PatchOwner::Heartbeat].freq - original_hb_freq).abs()
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
      hb.set_param("freq", 555.0);
    }
    preview
      .locked_params
      .write()
      .unwrap()
      .entry(PatchOwner::Heartbeat)
      .or_default()
      .insert("freq".to_string());

    preview.revert();

    let patches = preview.patches.read().unwrap();
    assert!(
      (patches[&PatchOwner::Heartbeat].freq - 555.0).abs() < f64::EPSILON,
      "Locked heartbeat freq should survive revert",
    );
  }

  #[test]
  fn state_snapshot_encodes_all_patches() {
    let preview = test_preview();
    // Set heartbeat and drone lo/hi to different values.
    {
      let mut patches = preview.patches.write().unwrap();
      patches
        .get_mut(&PatchOwner::Heartbeat)
        .unwrap()
        .set_param("freq", 111.0);
      patches
        .get_mut(&PatchOwner::DroneLo(0))
        .unwrap()
        .set_param("freq", 222.0);
      patches
        .get_mut(&PatchOwner::DroneHi(0))
        .unwrap()
        .set_param("freq", 333.0);
    }
    let json = preview.state_snapshot();
    let state: StateContract =
      serde_json::from_str(&json).expect("state_snapshot should decode");
    let hb_freq = state.patch["freq"].as_f64().unwrap();
    // The drone is the second entry in the unified checks array
    // (index 1, after the heartbeat check at index 0).
    let lo_freq = state.checks[1].patch_lo["freq"].as_f64().unwrap();
    let hi_freq = state.checks[1].patch_hi["freq"].as_f64().unwrap();
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
        let voice = base_voice.with_overrides(config.patch_overrides_ref());

        let heartbeat_checks: Vec<_> = config
          .daemon
          .checks
          .iter()
          .filter(|c| {
            c.result_mode
              == sonify_health_lib::check::ResultMode::ExitCodeSeverity
          })
          .cloned()
          .collect();
        let drone_checks: Vec<_> = config
          .daemon
          .checks
          .iter()
          .filter(|c| {
            c.result_mode
              != sonify_health_lib::check::ResultMode::ExitCodeSeverity
          })
          .cloned()
          .collect();

        let heartbeat_notes: Vec<sonify_health_lib::NoteSpec> =
          heartbeat_checks
            .iter()
            .flat_map(|c| {
              config
                .daemon
                .check_notes
                .get(&c.name)
                .cloned()
                .unwrap_or_default()
            })
            .collect();
        let drone_notes: std::collections::HashMap<
          String,
          Vec<sonify_health_lib::NoteSpec>,
        > = drone_checks
          .iter()
          .filter_map(|c| {
            config
              .daemon
              .check_notes
              .get(&c.name)
              .map(|n| (c.name.clone(), n.clone()))
          })
          .collect();

        let preview = PreviewState::new(
          voice,
          Arc::new(AtomicBool::new(false)),
          &heartbeat_checks,
          &drone_checks,
          &config.daemon.profile_overrides,
          config.daemon.timing.slot_duration_secs,
          &heartbeat_notes,
          &drone_notes,
        );

        let json = preview.state_snapshot();
        let state: StateContract =
          serde_json::from_str(&json).unwrap_or_else(|e| {
            panic!(
              "{}: state_snapshot does not match frontend contract: {e}",
              path.display()
            )
          });

        let total = heartbeat_checks.len() + drone_checks.len();
        assert_eq!(
          state.checks.len(),
          total,
          "{}: unified check count mismatch",
          path.display()
        );
        // Heartbeat checks come first.
        for (i, c) in state.checks.iter().enumerate() {
          if i < heartbeat_checks.len() {
            assert_eq!(
              c.kind,
              "heartbeat",
              "{}: check[{i}] kind",
              path.display()
            );
          } else {
            let di = i - heartbeat_checks.len();
            assert_eq!(c.kind, "drone", "{}: check[{i}] kind", path.display());
            assert_eq!(
              c.name,
              drone_checks[di].name,
              "{}: check[{i}] name",
              path.display()
            );
            assert!(
              !c.patch_lo.is_empty(),
              "{}: check[{i}] patch_lo is empty",
              path.display()
            );
          }
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

  // -- Broadcast message contract tests ------------------------------------
  //
  // Each test builds JSON mirroring the production code in websocket.rs
  // and daemon.rs, then deserializes through the contract struct.  If the
  // backend shape drifts the test fails immediately.

  #[test]
  fn param_changed_with_index_matches_contract() {
    let msg = json!({
      "type": "param_changed",
      "layer": "drone_lo",
      "index": 0,
      "param": "freq",
      "value": 440.0,
    })
    .to_string();
    let parsed: ParamChangedContract = serde_json::from_str(&msg)
      .expect("param_changed (with index) does not match contract");
    assert_eq!(parsed.msg_type, "param_changed");
    assert_eq!(parsed.index, Some(0));
  }

  #[test]
  fn param_changed_without_index_matches_contract() {
    let msg = json!({
      "type": "param_changed",
      "layer": "heartbeat",
      "param": "freq",
      "value": 440.0,
    })
    .to_string();
    let parsed: ParamChangedContract = serde_json::from_str(&msg)
      .expect("param_changed (without index) does not match contract");
    assert_eq!(parsed.msg_type, "param_changed");
    assert_eq!(parsed.index, None);
  }

  #[test]
  fn mute_changed_matches_contract() {
    let msg = json!({
      "type": "mute_changed",
      "muted": true,
    })
    .to_string();
    let parsed: MuteChangedContract =
      serde_json::from_str(&msg).expect("mute_changed does not match contract");
    assert_eq!(parsed.msg_type, "mute_changed");
    assert!(parsed.muted);
  }

  #[test]
  fn volume_changed_with_index_matches_contract() {
    let msg = json!({
      "type": "volume_changed",
      "layer": "master",
      "volume": 0.75,
      "index": 2,
    })
    .to_string();
    let parsed: VolumeChangedContract = serde_json::from_str(&msg)
      .expect("volume_changed (with index) does not match contract");
    assert_eq!(parsed.msg_type, "volume_changed");
    assert_eq!(parsed.index, Some(2));
  }

  #[test]
  fn volume_changed_without_index_matches_contract() {
    let msg = json!({
      "type": "volume_changed",
      "layer": "heartbeat",
      "volume": 0.5,
    })
    .to_string();
    let parsed: VolumeChangedContract = serde_json::from_str(&msg)
      .expect("volume_changed (without index) does not match contract");
    assert_eq!(parsed.msg_type, "volume_changed");
    assert_eq!(parsed.index, None);
  }

  #[test]
  fn override_changed_set_matches_contract() {
    let msg = json!({
      "type": "override_changed",
      "layer": "heartbeat",
      "index": 0,
      "value": 0.5,
      "overridden": true,
    })
    .to_string();
    let parsed: OverrideChangedContract = serde_json::from_str(&msg)
      .expect("override_changed (set) does not match contract");
    assert_eq!(parsed.msg_type, "override_changed");
    assert!(parsed.overridden);
    assert_eq!(parsed.value, Some(0.5));
  }

  #[test]
  fn override_changed_clear_matches_contract() {
    let msg = json!({
      "type": "override_changed",
      "layer": "drone",
      "index": 1,
      "value": null,
      "overridden": false,
    })
    .to_string();
    let parsed: OverrideChangedContract = serde_json::from_str(&msg)
      .expect("override_changed (clear) does not match contract");
    assert_eq!(parsed.msg_type, "override_changed");
    assert!(!parsed.overridden);
    assert_eq!(parsed.value, None);
  }

  #[test]
  fn heartbeat_loop_changed_matches_contract() {
    let msg = json!({
      "type": "heartbeat_loop_changed",
      "enabled": true,
    })
    .to_string();
    let parsed: HeartbeatLoopChangedContract = serde_json::from_str(&msg)
      .expect("heartbeat_loop_changed does not match contract");
    assert_eq!(parsed.msg_type, "heartbeat_loop_changed");
    assert!(parsed.enabled);
  }

  #[test]
  fn boop_count_changed_matches_contract() {
    let msg = json!({
      "type": "boop_count_changed",
      "count": 4,
    })
    .to_string();
    let parsed: BoopCountChangedContract = serde_json::from_str(&msg)
      .expect("boop_count_changed does not match contract");
    assert_eq!(parsed.msg_type, "boop_count_changed");
    assert_eq!(parsed.count, 4);
  }

  #[test]
  fn drone_interp_curve_changed_matches_contract() {
    let msg = json!({
      "type": "drone_interp_curve_changed",
      "index": 0,
      "curve": 2.5,
    })
    .to_string();
    let parsed: DroneInterpCurveChangedContract = serde_json::from_str(&msg)
      .expect("drone_interp_curve_changed does not match contract");
    assert_eq!(parsed.msg_type, "drone_interp_curve_changed");
    assert_eq!(parsed.index, 0);
  }

  #[test]
  fn check_log_matches_contract() {
    let msg = json!({
      "type": "check_log",
      "timestamp": 1700000000.0,
      "layer": "heartbeat",
      "name": "cpu",
      "result": "healthy",
      "overridden": false,
    })
    .to_string();
    let parsed: CheckLogContract =
      serde_json::from_str(&msg).expect("check_log does not match contract");
    assert_eq!(parsed.msg_type, "check_log");
    assert_eq!(parsed.name, "cpu");
  }

  #[test]
  fn patch_export_matches_contract() {
    let msg = json!({
      "type": "patch_export",
      "toml": "[patch]\nfreq = 440.0",
      "json": "{\"freq\": 440.0}",
      "nix": "{ freq = 440.0; }",
    })
    .to_string();
    let parsed: PatchExportContract =
      serde_json::from_str(&msg).expect("patch_export does not match contract");
    assert_eq!(parsed.msg_type, "patch_export");
    assert!(!parsed.toml.is_empty());
  }

  #[test]
  fn locked_params_changed_with_index_matches_contract() {
    let msg = json!({
      "type": "locked_params_changed",
      "layer": "drone_lo",
      "index": 0,
      "params": ["freq", "volume"],
    })
    .to_string();
    let parsed: LockedParamsChangedContract = serde_json::from_str(&msg)
      .expect("locked_params_changed (with index) does not match contract");
    assert_eq!(parsed.msg_type, "locked_params_changed");
    assert_eq!(parsed.index, Some(0));
    assert_eq!(parsed.params.len(), 2);
  }

  #[test]
  fn locked_params_changed_without_index_matches_contract() {
    let msg = json!({
      "type": "locked_params_changed",
      "layer": "heartbeat",
      "params": ["freq"],
    })
    .to_string();
    let parsed: LockedParamsChangedContract = serde_json::from_str(&msg)
      .expect("locked_params_changed (without index) does not match contract");
    assert_eq!(parsed.msg_type, "locked_params_changed");
    assert_eq!(parsed.index, None);
  }

  #[test]
  fn locked_drones_changed_matches_contract() {
    let msg = json!({
      "type": "locked_drones_changed",
      "indices": [0, 2],
    })
    .to_string();
    let parsed: LockedDronesChangedContract = serde_json::from_str(&msg)
      .expect("locked_drones_changed does not match contract");
    assert_eq!(parsed.msg_type, "locked_drones_changed");
    assert_eq!(parsed.indices, vec![0, 2]);
  }

  #[test]
  fn boop_specs_changed_matches_contract() {
    let msg = json!({
      "type": "boop_specs_changed",
      "specs": [
        {"freq": 440.0, "duration": 0.2, "pinned": false},
        {"freq": 880.0, "duration": 0.1, "pinned": true},
      ],
    })
    .to_string();
    let parsed: BoopSpecsChangedContract = serde_json::from_str(&msg)
      .expect("boop_specs_changed does not match contract");
    assert_eq!(parsed.msg_type, "boop_specs_changed");
    assert_eq!(parsed.specs.len(), 2);
  }

  #[test]
  fn drone_specs_changed_matches_contract() {
    let msg = json!({
      "type": "drone_specs_changed",
      "index": 1,
      "specs": [
        {"freq": 220.0, "duration": 0.5, "pinned": false},
      ],
    })
    .to_string();
    let parsed: DroneSpecsChangedContract = serde_json::from_str(&msg)
      .expect("drone_specs_changed does not match contract");
    assert_eq!(parsed.msg_type, "drone_specs_changed");
    assert_eq!(parsed.index, 1);
    assert_eq!(parsed.specs.len(), 1);
  }

  #[test]
  fn import_error_matches_contract() {
    let msg = json!({
      "type": "import_error",
      "message": "Invalid TOML: unexpected key",
    })
    .to_string();
    let parsed: ImportErrorContract =
      serde_json::from_str(&msg).expect("import_error does not match contract");
    assert_eq!(parsed.msg_type, "import_error");
    assert!(!parsed.message.is_empty());
  }
}
