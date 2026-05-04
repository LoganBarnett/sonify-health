use crate::config::{OverrideInfo, RemoteSourceConfig, SliderRanges};
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
use thiserror::Error;
use tokio::sync::broadcast;

/// Errors returned by [`PreviewState::add_remote_source`].
#[derive(Debug, Error)]
pub enum AddSourceError {
  #[error("source name {name:?} is already in use")]
  DuplicateName { name: String },
}

/// Hardcoded name of the Local Source.  Source names are required
/// to be unique across `PreviewState::sources` (uniqueness will be
/// enforced when remote sources can be configured); the local
/// source's reservation of `"localhost"` is the canonical example.
/// Remote sources added through the UI default their name to the
/// hostname parsed from the configured URL — the user can override
/// before saving — and the name is a required config field for any
/// remote source loaded from a config file.
pub const LOCAL_SOURCE_NAME: &str = "localhost";

/// Per-heartbeat runtime state.
pub struct HeartbeatState {
  pub metric: Shared,
  pub override_value: RwLock<Option<f32>>,
  pub effective_volume: Shared,
}

/// Connection state of a Remote Source's outbound WebSocket.
#[derive(Debug, Clone)]
pub enum ConnectionStatus {
  /// Initial state and the state during a reconnect attempt.
  Connecting,
  /// WebSocket handshake completed, mirroring is live.
  Connected,
  /// Last attempt failed; the connector is waiting before retrying.
  /// `error` carries the most recent failure for display / logs.
  Disconnected { error: Option<String> },
}

impl ConnectionStatus {
  pub fn as_str(&self) -> &'static str {
    match self {
      ConnectionStatus::Connecting => "connecting",
      ConnectionStatus::Connected => "connected",
      ConnectionStatus::Disconnected { .. } => "disconnected",
    }
  }
}

/// What kind of Source this is.  Local sources have a poller running
/// in this process and own configuration that can be edited and
/// saved; Remote sources mirror state over an outbound WebSocket and
/// are read-only from the local UI.
pub enum SourceKind {
  Local,
  Remote {
    /// WebSocket URL (e.g. ~ws://host:3000/ws~ or ~wss://...~).
    url: String,
    /// Connection state, written by the connector task.
    status: RwLock<ConnectionStatus>,
    /// User toggle: when false, the local renderer mirrors state
    /// but never schedules audio for this Source's heartbeats.
    /// Default is false so a newly-added Remote does not start
    /// playing audio without an explicit opt-in.
    playback_enabled: AtomicBool,
  },
}

impl SourceKind {
  pub fn is_local(&self) -> bool {
    matches!(self, SourceKind::Local)
  }

  pub fn is_remote(&self) -> bool {
    matches!(self, SourceKind::Remote { .. })
  }
}

/// Per-Source state.  A Source is something that produces heartbeat
/// state for the local instance to render: a Local Source's poller
/// runs in this process, a Remote Source's state is mirrored in over
/// a WebSocket.  Fields kept here are the ones that conceptually
/// scope to a single Source: its name, patch library, heartbeat
/// configs, runtime heartbeat state, slider ranges, override map,
/// and (Local-only) the path it was loaded from.
///
/// `name` is the Source's user-facing identifier.  It must be unique
/// across all Sources in a `PreviewState`, and is the preferred form
/// for logs and UI labels (more semantic than the index into
/// `PreviewState::sources`).  See [`LOCAL_SOURCE_NAME`] for the
/// hardcoded local name.
pub struct Source {
  pub name: String,
  pub kind: SourceKind,
  pub library: RwLock<PatchLibrary>,
  original_library: PatchLibrary,
  pub overrides: RwLock<HashMap<String, OverrideInfo>>,
  original_overrides: HashMap<String, OverrideInfo>,
  pub heartbeat_configs: RwLock<Vec<HeartbeatConfig>>,
  original_heartbeat_configs: Vec<HeartbeatConfig>,
  pub heartbeats: RwLock<Vec<HeartbeatState>>,
  pub slider_ranges: SliderRanges,
  pub config_path: Option<PathBuf>,
  pub config_writable: bool,
}

/// Shared mutable state backing the real-time preview UI.
///
/// Both the Axum WebSocket handler and the `spawn_blocking` daemon
/// thread share an `Arc<PreviewState>`.  Per-Source data lives on
/// each entry of `sources`; the remaining fields are renderer- or
/// process-global state shared across all Sources (one mixer, one
/// mute switch, one metrics registry).
///
/// Today `sources` contains exactly one entry — the Local Source —
/// and the WebSocket protocol implicitly addresses that one Source.
/// Call sites that depend on that convention go through
/// [`PreviewState::local`] so the assumption is grep-able when the
/// protocol grows a source field.
pub struct PreviewState {
  pub sources: Vec<Source>,
  pub running: Arc<AtomicBool>,
  pub muted: Arc<AtomicBool>,
  pub metrics: Metrics,
  pub master_volume: Shared,
  pub mixer_handle: RwLock<Option<MixerHandle>>,
  pub broadcast_tx: broadcast::Sender<String>,
  pub probe_log_tx: broadcast::Sender<String>,
  /// True when this instance was started without an audio device:
  /// it polls heartbeats and serves state, but never opens a mixer
  /// or spawns play threads.  Surfaced in the state snapshot so a
  /// connected client knows the instance can't sound itself.
  pub headless: bool,
}

impl PreviewState {
  #[allow(clippy::too_many_arguments)]
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
    headless: bool,
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

    let local = Source {
      name: LOCAL_SOURCE_NAME.to_string(),
      kind: SourceKind::Local,
      original_library: library.clone(),
      library: RwLock::new(library),
      original_overrides: overrides.clone(),
      overrides: RwLock::new(overrides),
      original_heartbeat_configs: heartbeat_configs.clone(),
      heartbeat_configs: RwLock::new(heartbeat_configs),
      heartbeats: RwLock::new(heartbeats),
      slider_ranges,
      config_path,
      config_writable,
    };

    Self {
      sources: vec![local],
      running,
      muted,
      metrics,
      master_volume: shared(1.0),
      mixer_handle: RwLock::new(None),
      broadcast_tx,
      probe_log_tx,
      headless,
    }
  }

  /// Borrow the Source named `name`, or `None` if no such Source
  /// exists.  This is the name-based lookup the WebSocket protocol
  /// will use once it carries source identifiers.
  pub fn source_by_name(&self, name: &str) -> Option<&Source> {
    self.sources.iter().find(|s| s.name == name)
  }

  /// Borrow the Local Source.  Used by call sites whose external
  /// inputs (most WebSocket messages, save/export, the runtime
  /// `add_heartbeat` API) implicitly address the local instance.
  /// When the protocol grows a source identifier, those call sites
  /// will instead pass the wire-supplied name to `source_by_name`.
  pub fn local(&self) -> &Source {
    self
      .source_by_name(LOCAL_SOURCE_NAME)
      .expect("Local Source must exist in PreviewState::sources")
  }

  /// Update the effective volume for `hb_idx` within `source`,
  /// accounting for mute and master volume.  Volume is master *
  /// mute only; per-note volume is baked into the audio graph.
  pub fn update_effective_volume(&self, source: &Source, hb_idx: usize) {
    let hbs = source.heartbeats.read().unwrap_or_recover();
    if let Some(hb) = hbs.get(hb_idx) {
      let mute_factor = if self.muted.load(Ordering::Relaxed) {
        0.0
      } else {
        1.0
      };
      let vol = self.master_volume.value() * mute_factor;
      hb.effective_volume.set_value(vol);
    }
  }

  /// Update effective volumes for every heartbeat across every
  /// Source.  Mute and master volume are global.
  pub fn update_all_effective_volumes(&self) {
    let mute_factor = if self.muted.load(Ordering::Relaxed) {
      0.0
    } else {
      1.0
    };
    let vol = self.master_volume.value() * mute_factor;
    for source in &self.sources {
      let hbs = source.heartbeats.read().unwrap_or_recover();
      for hb in hbs.iter() {
        hb.effective_volume.set_value(vol);
      }
    }
  }

  /// Revert library patches, transitions, overrides, and master
  /// volume to their loaded-from-config state.  Only Local Sources
  /// have a meaningful "original" snapshot to revert to; Remote
  /// Sources are skipped — their state is the live mirror of the
  /// remote, which has nothing to revert to locally.
  pub fn revert(&self) {
    for source in &self.sources {
      if !source.kind.is_local() {
        continue;
      }
      *source.library.write().unwrap_or_recover() =
        source.original_library.clone();
      *source.overrides.write().unwrap_or_recover() =
        source.original_overrides.clone();
      *source.heartbeat_configs.write().unwrap_or_recover() =
        source.original_heartbeat_configs.clone();
      let hbs = source.heartbeats.read().unwrap_or_recover();
      for hb in hbs.iter() {
        *hb.override_value.write().unwrap_or_recover() = None;
      }
    }
    self.master_volume.set_value(1.0);
    self.update_all_effective_volumes();
  }

  /// Store the mixer handle so trigger_immediate_play can use it.
  pub fn set_mixer_handle(&self, handle: MixerHandle) {
    *self.mixer_handle.write().unwrap_or_recover() = Some(handle);
  }

  /// Play the local heartbeat at `hb_idx` immediately as a one-shot
  /// sound.  Spawns a fire-and-forget thread that removes the mixer
  /// slot after the sound finishes.  The wire protocol implicitly
  /// addresses the local source today; see [`local`](Self::local).
  pub fn trigger_immediate_play(&self, hb_idx: usize) {
    let handle = match self.mixer_handle.read().unwrap_or_recover().clone() {
      Some(h) => h,
      None => return,
    };

    let notes = self.resolve_local_notes(hb_idx);
    if notes.is_empty() {
      return;
    }

    let local = self.local();
    self.update_effective_volume(local, hb_idx);
    let eff_vol = {
      let hbs = local.heartbeats.read().unwrap_or_recover();
      match hbs.get(hb_idx) {
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

  /// Play a named patch from the local library immediately as a
  /// one-shot sound.  Spawns a fire-and-forget thread that removes
  /// the mixer slot after the sound finishes.
  pub fn play_patch_immediate(&self, name: &str) {
    let handle = match self.mixer_handle.read().unwrap_or_recover().clone() {
      Some(h) => h,
      None => return,
    };

    let patch = match self
      .local()
      .library
      .read()
      .unwrap_or_recover()
      .get(name)
      .cloned()
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

  /// Project the current Remote Sources back to their config-side
  /// `RemoteSourceConfig` shape so save/export round-trips pick up
  /// runtime changes (e.g. the user toggling `playback_enabled`).
  pub fn remote_source_configs(&self) -> Vec<RemoteSourceConfig> {
    self
      .sources
      .iter()
      .filter_map(|s| match &s.kind {
        SourceKind::Remote {
          url,
          playback_enabled,
          ..
        } => Some(RemoteSourceConfig {
          name: s.name.clone(),
          url: url.clone(),
          playback_enabled: playback_enabled.load(Ordering::Relaxed),
        }),
        SourceKind::Local => None,
      })
      .collect()
  }

  /// Append an empty Remote Source to `self.sources`.  The source
  /// starts with no library, no heartbeats, and a connection status
  /// of `Connecting` — the connector task fills these in when it
  /// receives the remote's first state snapshot.  Playback is
  /// disabled by default; the user opts in via the per-Source
  /// playback toggle.
  ///
  /// Must be called before `self` is wrapped in an `Arc`, since the
  /// `sources` Vec is otherwise immutable for the lifetime of the
  /// shared state.  Returns the new source's index.
  ///
  /// # Errors
  ///
  /// Returns an error if `name` collides with an existing Source
  /// name (uniqueness is required across all Sources).
  pub fn add_remote_source(
    &mut self,
    name: String,
    url: String,
  ) -> Result<usize, AddSourceError> {
    if self.sources.iter().any(|s| s.name == name) {
      return Err(AddSourceError::DuplicateName { name });
    }
    let source = Source {
      name,
      kind: SourceKind::Remote {
        url,
        status: RwLock::new(ConnectionStatus::Connecting),
        playback_enabled: AtomicBool::new(false),
      },
      library: RwLock::new(PatchLibrary::new()),
      original_library: PatchLibrary::new(),
      overrides: RwLock::new(HashMap::new()),
      original_overrides: HashMap::new(),
      heartbeat_configs: RwLock::new(Vec::new()),
      original_heartbeat_configs: Vec::new(),
      heartbeats: RwLock::new(Vec::new()),
      slider_ranges: SliderRanges::default(),
      config_path: None,
      config_writable: false,
    };
    self.sources.push(source);
    Ok(self.sources.len() - 1)
  }

  /// Add a new heartbeat to the Local Source at runtime.  Returns
  /// the heartbeat index inside the Local Source.
  pub fn add_heartbeat(&self, cfg: HeartbeatConfig) -> usize {
    let local = self.local();
    let mut configs = local.heartbeat_configs.write().unwrap_or_recover();
    configs.push(cfg);
    let hb_idx = configs.len() - 1;
    drop(configs);

    let mut hbs = local.heartbeats.write().unwrap_or_recover();
    hbs.push(HeartbeatState {
      metric: shared(0.0),
      override_value: RwLock::new(None),
      effective_volume: shared(1.0),
    });
    drop(hbs);

    self.update_effective_volume(local, hb_idx);
    hb_idx
  }

  /// Resolve all notes for the local heartbeat at `hb_idx` from the
  /// current metric and transition config.
  fn resolve_local_notes(&self, hb_idx: usize) -> Vec<ResolvedNote> {
    let local = self.local();
    let metric = {
      let hbs = local.heartbeats.read().unwrap_or_recover();
      match hbs.get(hb_idx) {
        Some(hb) => hb.metric.value() as f64,
        None => return vec![],
      }
    };
    let note_configs = {
      let cfg = &local.heartbeat_configs.read().unwrap_or_recover()[hb_idx];
      cfg.notes.clone()
    };
    let lib = local.library.read().unwrap_or_recover();
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
  ///
  /// The snapshot carries a `sources` array — one entry per Source
  /// in `self.sources`, each with its own library, heartbeats,
  /// slider ranges, overrides, kind, and (for Remote sources)
  /// connection status / playback toggle.
  ///
  /// The legacy flat fields (`library`, `heartbeats`,
  /// `slider_ranges`, `overrides`, `config_writable`, `config_path`)
  /// continue to mirror the Local Source's data so the existing
  /// frontend keeps working during the transition; the next step
  /// switches the frontend to consume `sources` and drops these.
  pub fn state_snapshot(&self) -> String {
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

    let sources_json: Vec<_> =
      self.sources.iter().map(source_state_json).collect();

    let local = self.local();
    let local_json = source_state_json(local);

    json!({
      "type": "state",
      "patch_params": param_metas,
      "library": local_json["library"].clone(),
      "muted": self.muted.load(Ordering::Relaxed),
      "master_volume": self.master_volume.value(),
      "heartbeats": local_json["heartbeats"].clone(),
      "slider_ranges": local_json["slider_ranges"].clone(),
      "overrides": local_json["overrides"].clone(),
      "config_writable": local.config_writable,
      "config_path": local.config_path.as_ref().map(|p| p.display().to_string()),
      "headless": self.headless,
      "sources": sources_json,
    })
    .to_string()
  }

  /// Serialize the local override map to a JSON value.
  pub fn overrides_json(&self) -> serde_json::Value {
    source_overrides_json(self.local())
  }
}

/// Build the per-Source JSON entry that goes into the snapshot's
/// `sources` array.  Common fields (library, heartbeats, slider
/// ranges, override map, name, kind) are emitted for every Source;
/// kind-specific fields layer on after.
fn source_state_json(source: &Source) -> serde_json::Value {
  let lib = source.library.read().unwrap_or_recover();
  let lib_json: serde_json::Map<String, serde_json::Value> = lib
    .iter()
    .map(|(name, patch)| {
      (name.clone(), serde_json::to_value(patch).unwrap_or_default())
    })
    .collect();

  let hb_configs = source.heartbeat_configs.read().unwrap_or_recover();
  let hbs = source.heartbeats.read().unwrap_or_recover();
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

  let mut entry = json!({
    "name": source.name,
    "library": lib_json,
    "heartbeats": heartbeats_json,
    "slider_ranges": serde_json::to_value(&source.slider_ranges).unwrap_or_default(),
    "overrides": source_overrides_json(source),
  });

  let obj = entry.as_object_mut().expect("entry built as object");
  match &source.kind {
    SourceKind::Local => {
      obj.insert("kind".to_string(), json!("local"));
      obj.insert("config_writable".to_string(), json!(source.config_writable));
      obj.insert(
        "config_path".to_string(),
        match &source.config_path {
          Some(p) => json!(p.display().to_string()),
          None => serde_json::Value::Null,
        },
      );
    }
    SourceKind::Remote {
      url,
      status,
      playback_enabled,
    } => {
      obj.insert("kind".to_string(), json!("remote"));
      obj.insert("url".to_string(), json!(url));
      obj.insert(
        "connection_status".to_string(),
        json!(status.read().unwrap_or_recover().as_str()),
      );
      obj.insert(
        "playback_enabled".to_string(),
        json!(playback_enabled.load(Ordering::Relaxed)),
      );
    }
  }
  entry
}

/// Serialize a Source's override map to a JSON value matching the
/// shape that `OverrideInfo`'s derived `Serialize` would produce.
fn source_overrides_json(source: &Source) -> serde_json::Value {
  let ovr = source.overrides.read().unwrap_or_recover();
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

#[cfg(test)]
mod tests {
  use super::*;
  use crate::metrics::Metrics;
  use sonify_health_lib::builtin_library;

  fn make_preview(headless: bool) -> PreviewState {
    PreviewState::new(
      builtin_library(),
      HashMap::new(),
      vec![],
      Arc::new(AtomicBool::new(false)),
      Arc::new(AtomicBool::new(true)),
      Metrics::new(),
      SliderRanges::default(),
      None,
      false,
      headless,
    )
  }

  #[test]
  fn state_snapshot_includes_headless_flag() {
    let preview = make_preview(true);
    let snap: serde_json::Value =
      serde_json::from_str(&preview.state_snapshot()).unwrap();
    assert_eq!(snap["headless"], serde_json::Value::Bool(true));

    let preview = make_preview(false);
    let snap: serde_json::Value =
      serde_json::from_str(&preview.state_snapshot()).unwrap();
    assert_eq!(snap["headless"], serde_json::Value::Bool(false));
  }

  #[test]
  fn local_source_is_named_localhost() {
    let preview = make_preview(false);
    assert_eq!(preview.local().name, LOCAL_SOURCE_NAME);
    assert!(preview.source_by_name(LOCAL_SOURCE_NAME).is_some());
  }

  #[test]
  fn add_remote_source_appends_with_remote_kind() {
    let mut preview = make_preview(false);
    let idx = preview
      .add_remote_source(
        "prod-db-1".to_string(),
        "ws://db1.example/ws".to_string(),
      )
      .unwrap();
    assert_eq!(idx, 1);
    assert_eq!(preview.sources.len(), 2);
    let remote = &preview.sources[idx];
    assert_eq!(remote.name, "prod-db-1");
    assert!(remote.kind.is_remote());
    assert!(remote.heartbeat_configs.read().unwrap().is_empty());
    assert!(remote.library.read().unwrap().is_empty());
    match &remote.kind {
      SourceKind::Remote {
        url,
        playback_enabled,
        status,
      } => {
        assert_eq!(url, "ws://db1.example/ws");
        assert!(!playback_enabled.load(Ordering::Relaxed));
        assert!(matches!(
          *status.read().unwrap(),
          ConnectionStatus::Connecting
        ));
      }
      SourceKind::Local => panic!("expected Remote kind"),
    }
  }

  #[test]
  fn add_remote_source_rejects_duplicate_name() {
    let mut preview = make_preview(false);
    // Collides with the Local Source's reserved name.
    let err = preview
      .add_remote_source(
        LOCAL_SOURCE_NAME.to_string(),
        "ws://elsewhere/ws".to_string(),
      )
      .unwrap_err();
    assert!(matches!(err, AddSourceError::DuplicateName { .. }));

    preview
      .add_remote_source("only-once".to_string(), "ws://a/ws".to_string())
      .unwrap();
    let err = preview
      .add_remote_source("only-once".to_string(), "ws://b/ws".to_string())
      .unwrap_err();
    assert!(matches!(err, AddSourceError::DuplicateName { .. }));
  }

  #[test]
  fn state_snapshot_includes_sources_array() {
    let mut preview = make_preview(false);
    preview
      .add_remote_source(
        "edge-1".to_string(),
        "wss://edge1.example/ws".to_string(),
      )
      .unwrap();
    let snap: serde_json::Value =
      serde_json::from_str(&preview.state_snapshot()).unwrap();

    let sources = snap["sources"].as_array().expect("sources is array");
    assert_eq!(sources.len(), 2);

    let local = sources
      .iter()
      .find(|s| s["name"] == "localhost")
      .expect("local source entry");
    assert_eq!(local["kind"], "local");
    assert!(local.get("library").is_some());
    assert!(local.get("heartbeats").is_some());

    let remote = sources
      .iter()
      .find(|s| s["name"] == "edge-1")
      .expect("remote source entry");
    assert_eq!(remote["kind"], "remote");
    assert_eq!(remote["url"], "wss://edge1.example/ws");
    assert_eq!(remote["connection_status"], "connecting");
    assert_eq!(remote["playback_enabled"], false);
    // Empty mirror until the connector populates it.
    assert!(remote["heartbeats"].as_array().unwrap().is_empty());
  }

  #[test]
  fn state_snapshot_flat_fields_still_mirror_local() {
    // Backward-compat assertion: until the frontend cuts over to
    // `sources`, the existing flat fields must keep reflecting the
    // Local Source so step 6a is invisible to the current UI.
    let mut preview = make_preview(false);
    preview
      .add_remote_source(
        "edge-1".to_string(),
        "wss://edge1.example/ws".to_string(),
      )
      .unwrap();
    let snap: serde_json::Value =
      serde_json::from_str(&preview.state_snapshot()).unwrap();

    let sources = snap["sources"].as_array().unwrap();
    let local = sources.iter().find(|s| s["name"] == "localhost").unwrap();
    assert_eq!(snap["library"], local["library"]);
    assert_eq!(snap["heartbeats"], local["heartbeats"]);
    assert_eq!(snap["overrides"], local["overrides"]);
    assert_eq!(snap["slider_ranges"], local["slider_ranges"]);
  }

  #[test]
  fn revert_skips_remote_sources() {
    let mut preview = make_preview(false);
    let remote_idx = preview
      .add_remote_source("remote".to_string(), "ws://remote/ws".to_string())
      .unwrap();
    // Simulate the connector having mirrored some state into the
    // remote source.
    preview.sources[remote_idx]
      .library
      .write()
      .unwrap()
      .insert("from-remote".to_string(), Patch::default());

    preview.revert();

    // The remote's mirrored library survives revert; revert is a
    // local-config-only concept.
    assert!(preview.sources[remote_idx]
      .library
      .read()
      .unwrap()
      .contains_key("from-remote"));
  }
}
