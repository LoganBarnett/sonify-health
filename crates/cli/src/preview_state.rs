use crate::config::{OverrideInfo, SliderRanges};
use crate::lock_util::RecoverPoison;
use crate::metrics::Metrics;
use fundsp::prelude32::shared;
use fundsp::shared::Shared;
use serde_json::json;
use sonify_health_lib::{
  audio::MixerHandle, heartbeat, HeartbeatConfig, Patch, PatchLibrary,
  ResolvedNote,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
  atomic::{AtomicBool, Ordering},
  Arc, RwLock,
};
use std::thread;
use tokio::sync::broadcast;

/// Per-heartbeat runtime state.
pub struct HeartbeatState {
  pub metric: Shared,
  pub override_value: RwLock<Option<f32>>,
  pub effective_volume: Shared,
}

/// Shared mutable state backing the real-time preview UI.
///
/// Both the Axum WebSocket handler and the `spawn_blocking` daemon
/// thread share an `Arc<PreviewState>`.
pub struct PreviewState {
  pub library: RwLock<PatchLibrary>,
  original_library: PatchLibrary,
  pub overrides: RwLock<HashMap<String, OverrideInfo>>,
  original_overrides: HashMap<String, OverrideInfo>,
  pub heartbeat_configs: RwLock<Vec<HeartbeatConfig>>,
  original_heartbeat_configs: Vec<HeartbeatConfig>,
  pub heartbeats: RwLock<Vec<HeartbeatState>>,
  pub running: Arc<AtomicBool>,
  pub muted: Arc<AtomicBool>,
  pub metrics: Metrics,
  pub master_volume: Shared,
  pub mixer_handle: RwLock<Option<MixerHandle>>,
  pub slider_ranges: SliderRanges,
  pub config_path: Option<PathBuf>,
  pub config_writable: bool,
  pub broadcast_tx: broadcast::Sender<String>,
  pub probe_log_tx: broadcast::Sender<String>,
}

impl PreviewState {
  pub fn new(
    library: PatchLibrary,
    overrides: HashMap<String, OverrideInfo>,
    heartbeat_configs: Vec<HeartbeatConfig>,
    muted: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    metrics: Metrics,
    slider_ranges: SliderRanges,
    config_path: Option<PathBuf>,
    config_writable: bool,
  ) -> Self {
    let (broadcast_tx, _) = broadcast::channel(256);
    let (probe_log_tx, _) = broadcast::channel(256);

    let heartbeats: Vec<HeartbeatState> = heartbeat_configs
      .iter()
      .map(|_cfg| HeartbeatState {
        metric: shared(0.0),
        override_value: RwLock::new(None),
        effective_volume: shared(1.0),
      })
      .collect();

    Self {
      original_library: library.clone(),
      library: RwLock::new(library),
      original_overrides: overrides.clone(),
      overrides: RwLock::new(overrides),
      original_heartbeat_configs: heartbeat_configs.clone(),
      heartbeat_configs: RwLock::new(heartbeat_configs),
      heartbeats: RwLock::new(heartbeats),
      running,
      muted,
      metrics,
      slider_ranges,
      config_path,
      config_writable,
      master_volume: shared(1.0),
      mixer_handle: RwLock::new(None),
      broadcast_tx,
      probe_log_tx,
    }
  }

  /// Update effective volume for a heartbeat, accounting for
  /// mute and master volume.  Volume is now master * mute only;
  /// per-note volume is baked into the audio graph.
  pub fn update_effective_volume(&self, index: usize) {
    let hbs = self.heartbeats.read().unwrap_or_recover();
    if let Some(hb) = hbs.get(index) {
      let mute_factor = if self.muted.load(Ordering::Relaxed) {
        0.0
      } else {
        1.0
      };
      let vol = self.master_volume.value() * mute_factor;
      hb.effective_volume.set_value(vol);
    }
  }

  /// Update effective volumes for all heartbeats.
  pub fn update_all_effective_volumes(&self) {
    let hbs = self.heartbeats.read().unwrap_or_recover();
    let mute_factor = if self.muted.load(Ordering::Relaxed) {
      0.0
    } else {
      1.0
    };
    let vol = self.master_volume.value() * mute_factor;
    for hb in hbs.iter() {
      hb.effective_volume.set_value(vol);
    }
  }

  /// Revert all library patches, transitions, and volumes to their
  /// original state.
  pub fn revert(&self) {
    *self.library.write().unwrap_or_recover() = self.original_library.clone();
    *self.overrides.write().unwrap_or_recover() =
      self.original_overrides.clone();
    *self.heartbeat_configs.write().unwrap_or_recover() =
      self.original_heartbeat_configs.clone();
    let hbs = self.heartbeats.read().unwrap_or_recover();
    for hb in hbs.iter() {
      *hb.override_value.write().unwrap_or_recover() = None;
    }
    drop(hbs);
    self.master_volume.set_value(1.0);
    self.update_all_effective_volumes();
  }

  /// Store the mixer handle so trigger_immediate_play can use it.
  pub fn set_mixer_handle(&self, handle: MixerHandle) {
    *self.mixer_handle.write().unwrap_or_recover() = Some(handle);
  }

  /// Play heartbeat `index` immediately as a one-shot sound.
  /// Spawns a fire-and-forget thread that removes the mixer slot
  /// after the sound finishes.
  pub fn trigger_immediate_play(&self, index: usize) {
    let handle = match self.mixer_handle.read().unwrap_or_recover().clone() {
      Some(h) => h,
      None => return,
    };

    let notes = self.resolve_notes(index);
    if notes.is_empty() {
      return;
    }

    self.update_effective_volume(index);
    let eff_vol = {
      let hbs = self.heartbeats.read().unwrap_or_recover();
      match hbs.get(index) {
        Some(hb) => hb.effective_volume.clone(),
        None => return,
      }
    };
    let graph = heartbeat::heartbeat_graph_with_notes(&notes, Some(&eff_vol));
    let dur = heartbeat::heartbeat_notes_duration(&notes);

    let sid = handle.add(graph);
    thread::spawn(move || {
      thread::sleep(dur);
      handle.remove(sid);
    });
  }

  /// Play a named patch immediately as a one-shot sound.
  /// Spawns a fire-and-forget thread that removes the mixer slot
  /// after the sound finishes.
  pub fn play_patch_immediate(&self, name: &str) {
    let handle = match self.mixer_handle.read().unwrap_or_recover().clone() {
      Some(h) => h,
      None => return,
    };

    let patch = match self.library.read().unwrap_or_recover().get(name).cloned()
    {
      Some(p) => p,
      None => return,
    };

    let notes = [ResolvedNote {
      patch,
      volume: 1.0,
      offset: 0.0,
    }];
    let graph = heartbeat::heartbeat_graph_with_notes(&notes, None);
    let dur = heartbeat::heartbeat_notes_duration(&notes);

    let sid = handle.add(graph);
    thread::spawn(move || {
      thread::sleep(dur);
      handle.remove(sid);
    });
  }

  /// Add a new heartbeat at runtime.  Returns the index.
  pub fn add_heartbeat(&self, cfg: HeartbeatConfig) -> usize {
    let mut configs = self.heartbeat_configs.write().unwrap_or_recover();
    configs.push(cfg);
    let index = configs.len() - 1;
    drop(configs);

    let mut hbs = self.heartbeats.write().unwrap_or_recover();
    hbs.push(HeartbeatState {
      metric: shared(0.0),
      override_value: RwLock::new(None),
      effective_volume: shared(1.0),
    });
    drop(hbs);

    self.update_effective_volume(index);
    index
  }

  /// Resolve all notes for heartbeat `index` from the current
  /// metric and transition config.
  fn resolve_notes(&self, index: usize) -> Vec<ResolvedNote> {
    let metric = {
      let hbs = self.heartbeats.read().unwrap_or_recover();
      match hbs.get(index) {
        Some(hb) => hb.metric.value() as f64,
        None => return vec![],
      }
    };
    let note_configs = {
      let cfg = &self.heartbeat_configs.read().unwrap_or_recover()[index];
      cfg.notes.clone()
    };
    let lib = self.library.read().unwrap_or_recover();
    note_configs
      .iter()
      .filter_map(|nc| {
        let patch = nc.transition.resolve(metric, &lib)?;
        Some(ResolvedNote {
          patch,
          volume: nc.volume,
          offset: nc.offset,
        })
      })
      .collect()
  }

  /// Build a full state snapshot JSON string for WebSocket clients.
  pub fn state_snapshot(&self) -> String {
    let lib = self.library.read().unwrap_or_recover();
    let lib_json: serde_json::Map<String, serde_json::Value> = lib
      .iter()
      .map(|(name, patch)| {
        (name.clone(), serde_json::to_value(patch).unwrap_or_default())
      })
      .collect();

    let param_metas: Vec<_> = Patch::PARAMS
      .iter()
      .map(|m| {
        json!({
          "name": m.name,
          "description": m.description,
          "min": m.min,
          "max": m.max,
          "step": m.step,
        })
      })
      .collect();

    let hb_configs = self.heartbeat_configs.read().unwrap_or_recover();
    let hbs = self.heartbeats.read().unwrap_or_recover();
    let heartbeats_json: Vec<_> = hb_configs
      .iter()
      .enumerate()
      .map(|(i, cfg)| {
        let hb = &hbs[i];
        let overridden = hb.override_value.read().unwrap_or_recover().is_some();
        let notes_json: Vec<_> = cfg
          .notes
          .iter()
          .map(|nc| {
            json!({
              "volume": nc.volume,
              "offset": nc.offset,
              "transition": serde_json::to_value(&nc.transition).unwrap_or_default(),
            })
          })
          .collect();
        json!({
          "name": cfg.name,
          "command": cfg.command,
          "result_mode": serde_json::to_value(&cfg.result_mode).unwrap_or_default(),
          "playback": serde_json::to_value(&cfg.playback).unwrap_or_default(),
          "metric": hb.metric.value(),
          "overridden": overridden,
          "poll_interval_secs": cfg.poll_interval_secs,
          "cycle_secs": cfg.cycle_secs,
          "cycle_offset_secs": cfg.cycle_offset_secs,
          "crossfade_ms": cfg.crossfade_ms,
          "phrase_gap": cfg.phrase_gap,
          "repeat_rate": cfg.repeat_rate,
          "notes": notes_json,
          "tiers": serde_json::to_value(&cfg.tiers).unwrap_or_default(),
        })
      })
      .collect();

    let overrides_json = self.overrides_json();

    json!({
      "type": "state",
      "patch_params": param_metas,
      "library": lib_json,
      "muted": self.muted.load(Ordering::Relaxed),
      "master_volume": self.master_volume.value(),
      "heartbeats": heartbeats_json,
      "slider_ranges": serde_json::to_value(&self.slider_ranges).unwrap_or_default(),
      "overrides": overrides_json,
      "config_writable": self.config_writable,
      "config_path": self.config_path.as_ref().map(|p| p.display().to_string()),
    })
    .to_string()
  }

  /// Serialize the overrides map to a JSON value.
  pub fn overrides_json(&self) -> serde_json::Value {
    let ovr = self.overrides.read().unwrap_or_recover();
    let map: serde_json::Map<String, serde_json::Value> = ovr
      .iter()
      .map(|(name, info)| {
        let delta: serde_json::Map<String, serde_json::Value> = info
          .delta
          .iter()
          .map(|(k, v)| (k.clone(), json!(v)))
          .collect();
        (name.clone(), json!({ "base": info.base, "delta": delta }))
      })
      .collect();
    serde_json::Value::Object(map)
  }
}

/// Return a human-readable label for a metric value.  Uses custom
/// tiers when available, otherwise formats the raw value.
pub fn metric_label(
  metric: f32,
  tiers: &[sonify_health_lib::TierConfig],
) -> String {
  for tier in tiers {
    if (metric as f64) < tier.threshold {
      return tier.label.clone();
    }
  }
  tiers
    .last()
    .map(|t| t.label.clone())
    .unwrap_or_else(|| format!("{metric:.3}"))
}
