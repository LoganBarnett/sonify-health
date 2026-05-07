use crate::config::{OverrideInfo, RemoteSourceConfig, SliderRanges};
use crate::metrics::Metrics;
use fundsp::prelude32::shared;
use fundsp::shared::Shared;
use parking_lot::RwLock;
use serde_json::json;
use sonify_health_lib::{
  audio::MixerHandle, heartbeat, HeartbeatConfig, Patch, PatchLibrary,
  ResolvedNote,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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
  /// Wire-format discriminator.  An error-bearing `Disconnected`
  /// reports as `"error"` so the UI can render it distinctly from
  /// a clean disconnect (which doesn't currently happen but is
  /// represented for symmetry).
  pub fn as_str(&self) -> &'static str {
    match self {
      ConnectionStatus::Connecting => "connecting",
      ConnectionStatus::Connected => "connected",
      ConnectionStatus::Disconnected { error: None } => "disconnected",
      ConnectionStatus::Disconnected { error: Some(_) } => "error",
    }
  }

  /// Human-readable failure message when the connector ended
  /// abnormally.  `None` whenever the state is not an error case.
  pub fn error_message(&self) -> Option<&str> {
    match self {
      ConnectionStatus::Disconnected { error: Some(msg) } => Some(msg.as_str()),
      _ => None,
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
    /// Set to false by `remove_remote_source` so the connector
    /// task and any play threads holding an `Arc<Source>` exit
    /// cleanly even though the Arc keeps the Source itself alive
    /// until the last reference drops.
    alive: AtomicBool,
  },
}

impl SourceKind {
  pub fn is_local(&self) -> bool {
    matches!(self, SourceKind::Local)
  }

  pub fn is_remote(&self) -> bool {
    matches!(self, SourceKind::Remote { .. })
  }

  /// True when the Source is still part of `PreviewState::sources`.
  /// Local sources are always alive (they're never removed).
  /// Remote sources flip `alive` to false when removed, so async
  /// tasks holding an `Arc<Source>` notice and exit.
  pub fn is_alive(&self) -> bool {
    match self {
      SourceKind::Local => true,
      SourceKind::Remote { alive, .. } => alive.load(Ordering::Relaxed),
    }
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
/// The WebSocket protocol implicitly addresses the Local Source
/// today; call sites that depend on that convention go through
/// [`PreviewState::local`].  Storing `local` as a separate
/// `Arc<Source>` field rather than as element 0 of a `Vec` lets
/// the type system carry the "Local always exists" invariant —
/// no runtime assertion (no `.expect()`) is needed to access it.
pub struct PreviewState {
  /// The Local Source.  Set once at construction and never
  /// removed; the type system enforces "always present" so
  /// `local()` doesn't need to assert it at runtime.
  pub local: Arc<Source>,
  /// Remote Sources mirrored over outbound WebSockets.  Wrapped
  /// in `RwLock<Vec<...>>` so remotes can be added (via
  /// `add_remote_source`) and removed (via `remove_remote_source`)
  /// at runtime: readers clone the Arc out under a brief read
  /// lock and use it without holding the outer guard.
  pub remote_sources: RwLock<Vec<Arc<Source>>>,
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
      local: Arc::new(local),
      remote_sources: RwLock::new(Vec::new()),
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

  /// Snapshot the current Sources list as a `Vec<Arc<Source>>` —
  /// the Local Source first, then every Remote Source in
  /// insertion order.  Holds the remote read lock only long
  /// enough to clone the Arcs; callers can iterate at leisure
  /// without blocking writers.
  pub fn sources_snapshot(&self) -> Vec<Arc<Source>> {
    let remotes = self.remote_sources.read();
    let mut out = Vec::with_capacity(remotes.len() + 1);
    out.push(Arc::clone(&self.local));
    out.extend(remotes.iter().cloned());
    out
  }

  /// Look up a Source by name.  Returns `None` if no such Source
  /// exists.  Checks Local first (it's the most common lookup);
  /// if `name` isn't `LOCAL_SOURCE_NAME` we fall through to a
  /// linear scan of the remotes Vec.
  pub fn source_by_name(&self, name: &str) -> Option<Arc<Source>> {
    if name == LOCAL_SOURCE_NAME {
      return Some(Arc::clone(&self.local));
    }
    self
      .remote_sources
      .read()
      .iter()
      .find(|s| s.name == name)
      .map(Arc::clone)
  }

  /// The Local Source.  Used by call sites whose external inputs
  /// (most WebSocket mutation messages, save/export, the runtime
  /// `add_heartbeat` API) implicitly address the local instance.
  /// `local` is structurally guaranteed to exist — it's a direct
  /// `Arc<Source>` field on `PreviewState` rather than an entry in
  /// a `Vec`, so this accessor never has to assert presence.
  pub fn local(&self) -> Arc<Source> {
    Arc::clone(&self.local)
  }

  /// Update the effective volume for `hb_idx` within `source`,
  /// accounting for mute and master volume.  Volume is master *
  /// mute only; per-note volume is baked into the audio graph.
  pub fn update_effective_volume(&self, source: &Source, hb_idx: usize) {
    let hbs = source.heartbeats.read();
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
    for source in self.sources_snapshot() {
      let hbs = source.heartbeats.read();
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
    for source in self.sources_snapshot() {
      if !source.kind.is_local() {
        continue;
      }
      *source.library.write() = source.original_library.clone();
      *source.overrides.write() = source.original_overrides.clone();
      *source.heartbeat_configs.write() =
        source.original_heartbeat_configs.clone();
      let hbs = source.heartbeats.read();
      for hb in hbs.iter() {
        *hb.override_value.write() = None;
      }
    }
    self.master_volume.set_value(1.0);
    self.update_all_effective_volumes();
  }

  /// Store the mixer handle so trigger_immediate_play can use it.
  pub fn set_mixer_handle(&self, handle: MixerHandle) {
    *self.mixer_handle.write() = Some(handle);
  }

  /// Play the local heartbeat at `hb_idx` immediately as a one-shot
  /// sound.  Spawns a fire-and-forget thread that removes the mixer
  /// slot after the sound finishes.  The wire protocol implicitly
  /// addresses the local source today; see [`local`](Self::local).
  pub fn trigger_immediate_play(&self, hb_idx: usize) {
    let handle = match self.mixer_handle.read().clone() {
      Some(h) => h,
      None => return,
    };

    let notes = self.resolve_local_notes(hb_idx);
    if notes.is_empty() {
      return;
    }

    let local = self.local();
    self.update_effective_volume(&local, hb_idx);
    let eff_vol = {
      let hbs = local.heartbeats.read();
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
    let handle = match self.mixer_handle.read().clone() {
      Some(h) => h,
      None => return,
    };

    let patch = match self.local().library.read().get(name).cloned() {
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

  /// Flip the `playback_enabled` atomic on the Remote Source named
  /// `source_name`.  Returns `true` if the source exists, is a
  /// Remote (Local sources don't carry the toggle), and the value
  /// was applied; `false` otherwise.
  pub fn set_remote_playback_enabled(
    &self,
    source_name: &str,
    enabled: bool,
  ) -> bool {
    match self.source_by_name(source_name) {
      Some(source) => match &source.kind {
        SourceKind::Remote {
          playback_enabled, ..
        } => {
          playback_enabled.store(enabled, Ordering::Relaxed);
          true
        }
        SourceKind::Local => false,
      },
      None => false,
    }
  }

  /// Project the current Remote Sources back to their config-side
  /// `RemoteSourceConfig` shape so save/export round-trips pick up
  /// runtime changes (e.g. the user toggling `playback_enabled`).
  pub fn remote_source_configs(&self) -> Vec<RemoteSourceConfig> {
    self
      .sources_snapshot()
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

  /// Append an empty Remote Source to `self.remote_sources`.  The
  /// source starts with no library, no heartbeats, and a
  /// connection status of `Connecting` — the connector task fills
  /// these in when it receives the remote's first state snapshot.
  /// Playback is disabled by default; the user opts in via the
  /// per-Source playback toggle.
  ///
  /// Returns the index of the new source within
  /// `self.remote_sources`.
  ///
  /// # Errors
  ///
  /// Returns an error if `name` collides with an existing Source
  /// name (uniqueness is required across all Sources, Local and
  /// Remote) or if `name` is the reserved Local Source name.
  pub fn add_remote_source(
    &self,
    name: String,
    url: String,
  ) -> Result<usize, AddSourceError> {
    if name == LOCAL_SOURCE_NAME {
      return Err(AddSourceError::DuplicateName { name });
    }
    let mut remotes = self.remote_sources.write();
    if remotes.iter().any(|s| s.name == name) {
      return Err(AddSourceError::DuplicateName { name });
    }
    let source = Source {
      name,
      kind: SourceKind::Remote {
        url,
        status: RwLock::new(ConnectionStatus::Connecting),
        playback_enabled: AtomicBool::new(false),
        alive: AtomicBool::new(true),
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
    remotes.push(Arc::new(source));
    Ok(remotes.len() - 1)
  }

  /// Remove the Remote Source named `name` from
  /// `self.remote_sources` and signal any tasks holding an
  /// `Arc<Source>` to that entry — the connector task and any play
  /// threads for the source's heartbeats — to exit cleanly via the
  /// `alive` flag.  Returns `true` if a matching Remote Source was
  /// removed, `false` if no such Source exists or `name` referred
  /// to the Local Source (which is never removed).
  pub fn remove_remote_source(&self, name: &str) -> bool {
    if name == LOCAL_SOURCE_NAME {
      return false;
    }
    let mut remotes = self.remote_sources.write();
    let Some(idx) = remotes.iter().position(|s| s.name == name) else {
      return false;
    };
    if let SourceKind::Remote { alive, .. } = &remotes[idx].kind {
      alive.store(false, Ordering::Relaxed);
    }
    remotes.remove(idx);
    true
  }

  /// Add a new heartbeat to the Local Source at runtime.  Returns
  /// the heartbeat index inside the Local Source.
  pub fn add_heartbeat(&self, cfg: HeartbeatConfig) -> usize {
    let local = self.local();
    let mut configs = local.heartbeat_configs.write();
    configs.push(cfg);
    let hb_idx = configs.len() - 1;
    drop(configs);

    let mut hbs = local.heartbeats.write();
    hbs.push(HeartbeatState {
      metric: shared(0.0),
      override_value: RwLock::new(None),
      effective_volume: shared(1.0),
    });
    drop(hbs);

    self.update_effective_volume(&local, hb_idx);
    hb_idx
  }

  /// Resolve all notes for the local heartbeat at `hb_idx` from the
  /// current metric and transition config.
  fn resolve_local_notes(&self, hb_idx: usize) -> Vec<ResolvedNote> {
    let local = self.local();
    let metric = {
      let hbs = local.heartbeats.read();
      match hbs.get(hb_idx) {
        Some(hb) => hb.metric.value() as f64,
        None => return vec![],
      }
    };
    let note_configs = {
      let cfg = &local.heartbeat_configs.read()[hb_idx];
      cfg.notes.clone()
    };
    let lib = local.library.read();
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

    let snapshot = self.sources_snapshot();
    let sources_json: Vec<_> =
      snapshot.iter().map(|s| source_state_json(s)).collect();

    let local = self.local();
    let local_json = source_state_json(&local);

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
    source_overrides_json(&self.local())
  }
}

/// Build the per-Source JSON entry that goes into the snapshot's
/// `sources` array.  Common fields (library, heartbeats, slider
/// ranges, override map, name, kind) are emitted for every Source;
/// kind-specific fields layer on after.
fn source_state_json(source: &Source) -> serde_json::Value {
  let lib = source.library.read();
  let lib_json: serde_json::Map<String, serde_json::Value> = lib
    .iter()
    .map(|(name, patch)| {
      (name.clone(), serde_json::to_value(patch).unwrap_or_default())
    })
    .collect();

  let hb_configs = source.heartbeat_configs.read();
  let hbs = source.heartbeats.read();
  let heartbeats_json: Vec<_> = hb_configs
    .iter()
    .enumerate()
    .map(|(i, cfg)| {
      let hb = &hbs[i];
      let overridden = hb.override_value.read().is_some();
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
        "playback": serde_json::to_value(cfg.playback).unwrap_or_default(),
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

  // Build as a `serde_json::Map` directly rather than constructing
  // a `Value::Object` and immediately re-extracting it via
  // `as_object_mut()`.  Operating on the typed `Map` makes
  // kind-specific fields a straight series of `insert` calls and
  // removes the runtime "must still be an object" assertion.
  let mut obj = serde_json::Map::new();
  obj.insert("name".to_string(), json!(source.name));
  obj.insert("library".to_string(), json!(lib_json));
  obj.insert("heartbeats".to_string(), json!(heartbeats_json));
  obj.insert(
    "slider_ranges".to_string(),
    serde_json::to_value(&source.slider_ranges).unwrap_or_default(),
  );
  obj.insert("overrides".to_string(), source_overrides_json(source));

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
      alive: _,
    } => {
      obj.insert("kind".to_string(), json!("remote"));
      obj.insert("url".to_string(), json!(url));
      let status_guard = status.read();
      obj.insert("connection_status".to_string(), json!(status_guard.as_str()));
      // Emit `connection_error` only when the status is an error
      // case; absent for connecting/connected/clean-disconnect so
      // the frontend can branch on field presence as well as on
      // status discriminator.
      if let Some(msg) = status_guard.error_message() {
        obj.insert("connection_error".to_string(), json!(msg));
      }
      obj.insert(
        "playback_enabled".to_string(),
        json!(playback_enabled.load(Ordering::Relaxed)),
      );
    }
  }
  serde_json::Value::Object(obj)
}

/// Serialize a Source's override map to a JSON value matching the
/// shape that `OverrideInfo`'s derived `Serialize` would produce.
fn source_overrides_json(source: &Source) -> serde_json::Value {
  let ovr = source.overrides.read();
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
      Metrics::new().expect("Metrics::new in test"),
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
    let preview = make_preview(false);
    let idx = preview
      .add_remote_source(
        "prod-db-1".to_string(),
        "ws://db1.example/ws".to_string(),
      )
      .unwrap();
    // First remote in `remote_sources` is at index 0 — local has
    // its own field and is no longer mixed into the same Vec.
    assert_eq!(idx, 0);
    let remote = preview.source_by_name("prod-db-1").expect("source exists");
    assert_eq!(remote.name, "prod-db-1");
    assert!(remote.kind.is_remote());
    assert!(remote.heartbeat_configs.read().is_empty());
    assert!(remote.library.read().is_empty());
    match &remote.kind {
      SourceKind::Remote {
        url,
        playback_enabled,
        status,
        alive,
      } => {
        assert_eq!(url, "ws://db1.example/ws");
        assert!(!playback_enabled.load(Ordering::Relaxed));
        assert!(alive.load(Ordering::Relaxed));
        assert!(matches!(*status.read(), ConnectionStatus::Connecting));
      }
      SourceKind::Local => panic!("expected Remote kind"),
    }
  }

  #[test]
  fn add_remote_source_rejects_duplicate_name() {
    let preview = make_preview(false);
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
  fn remove_remote_source_drops_from_list_and_clears_alive() {
    let preview = make_preview(false);
    preview
      .add_remote_source(
        "edge-1".to_string(),
        "ws://edge1.example/ws".to_string(),
      )
      .unwrap();
    let source = preview
      .source_by_name("edge-1")
      .expect("present before remove");

    assert!(preview.remove_remote_source("edge-1"));
    assert!(preview.source_by_name("edge-1").is_none());
    // Tasks holding the Arc still see is_alive() == false so they
    // can exit cleanly.
    assert!(!source.kind.is_alive());

    // Removing a non-existent source is a no-op (returns false).
    assert!(!preview.remove_remote_source("not-there"));
    // Local cannot be removed.
    assert!(!preview.remove_remote_source(LOCAL_SOURCE_NAME));
  }

  #[test]
  fn state_snapshot_includes_sources_array() {
    let preview = make_preview(false);
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
    // `connection_error` is omitted whenever the status isn't an
    // error case so the frontend can branch on field presence.
    assert!(remote.get("connection_error").is_none());
    // Empty mirror until the connector populates it.
    assert!(remote["heartbeats"].as_array().unwrap().is_empty());
  }

  #[test]
  fn state_snapshot_surfaces_connection_error() {
    let preview = make_preview(false);
    preview
      .add_remote_source(
        "edge-1".to_string(),
        "wss://edge1.example/ws".to_string(),
      )
      .unwrap();
    // Simulate a connector that handed back a failure reason.
    let source = preview.source_by_name("edge-1").unwrap();
    if let SourceKind::Remote { status, .. } = &source.kind {
      *status.write() = ConnectionStatus::Disconnected {
        error: Some("URL error: TLS support not compiled in".to_string()),
      };
    } else {
      panic!("edge-1 was not Remote");
    }

    let snap: serde_json::Value =
      serde_json::from_str(&preview.state_snapshot()).unwrap();
    let remote = snap["sources"]
      .as_array()
      .unwrap()
      .iter()
      .find(|s| s["name"] == "edge-1")
      .unwrap();
    assert_eq!(remote["connection_status"], "error");
    assert_eq!(
      remote["connection_error"],
      "URL error: TLS support not compiled in"
    );
  }

  #[test]
  fn state_snapshot_flat_fields_still_mirror_local() {
    let preview = make_preview(false);
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
    let preview = make_preview(false);
    preview
      .add_remote_source("remote".to_string(), "ws://remote/ws".to_string())
      .unwrap();
    let remote = preview.source_by_name("remote").unwrap();
    // Simulate the connector having mirrored some state into the
    // remote source.
    remote
      .library
      .write()
      .insert("from-remote".to_string(), Patch::default());

    preview.revert();

    // The remote's mirrored library survives revert; revert is a
    // local-config-only concept.
    assert!(remote.library.read().contains_key("from-remote"));
  }
}
