use crate::config::{OverrideInfo, SliderRanges};
use fundsp::prelude32::shared;
use fundsp::shared::Shared;
use serde_json::json;
use sonify_health_lib::{HeartbeatConfig, Patch, PatchLibrary};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
  atomic::{AtomicBool, Ordering},
  Arc, RwLock,
};
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
  pub heartbeats: Vec<HeartbeatState>,
  pub muted: Arc<AtomicBool>,
  pub master_volume: Shared,
  pub heartbeat_trigger: AtomicBool,
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
      heartbeats,
      muted,
      slider_ranges,
      config_path,
      config_writable,
      master_volume: shared(1.0),
      heartbeat_trigger: AtomicBool::new(false),
      broadcast_tx,
      probe_log_tx,
    }
  }

  /// Update effective volume for a heartbeat, accounting for
  /// mute and master volume.  Volume is now master * mute only;
  /// per-note volume is baked into the audio graph.
  pub fn update_effective_volume(&self, index: usize) {
    if let Some(hb) = self.heartbeats.get(index) {
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
    for i in 0..self.heartbeats.len() {
      self.update_effective_volume(i);
    }
  }

  /// Revert all library patches, transitions, and volumes to their
  /// original state.
  pub fn revert(&self) {
    *self.library.write().unwrap() = self.original_library.clone();
    *self.overrides.write().unwrap() = self.original_overrides.clone();
    *self.heartbeat_configs.write().unwrap() =
      self.original_heartbeat_configs.clone();
    for hb in &self.heartbeats {
      *hb.override_value.write().unwrap() = None;
    }
    self.master_volume.set_value(1.0);
    self.update_all_effective_volumes();
  }

  /// Build a full state snapshot JSON string for WebSocket clients.
  pub fn state_snapshot(&self) -> String {
    let lib = self.library.read().unwrap();
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

    let hb_configs = self.heartbeat_configs.read().unwrap();
    let heartbeats_json: Vec<_> = hb_configs
      .iter()
      .enumerate()
      .map(|(i, cfg)| {
        let hb = &self.heartbeats[i];
        let overridden = hb.override_value.read().unwrap().is_some();
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
          "playback": serde_json::to_value(&cfg.playback).unwrap_or_default(),
          "metric": hb.metric.value(),
          "overridden": overridden,
          "cycle_offset_secs": cfg.cycle_offset_secs,
          "crossfade_ms": cfg.crossfade_ms,
          "notes": notes_json,
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
    let ovr = self.overrides.read().unwrap();
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

/// Return a human-readable label for a metric value.
pub fn metric_label(metric: f32) -> &'static str {
  if metric < 0.25 {
    "healthy"
  } else if metric < 0.75 {
    "degraded"
  } else {
    "down"
  }
}
