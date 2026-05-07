//! Outbound WebSocket client that mirrors a remote sonify-health
//! instance's state into a Remote Source on this `PreviewState`.
//!
//! The connector runs as one tokio task per Remote Source.  It
//! opens a WebSocket to the remote, parses incoming messages, and
//! applies them to the source's mirror.  On disconnect or error,
//! it reconnects with exponential backoff.  It never sends mutating
//! messages back — Remote Sources are read-only from the local UI.
//!
//! For the first cut the connector handles two message types
//! materially: `state` (full snapshot, replaces the mirror) and
//! `metric_changed` (incremental metric update).  Other incremental
//! types (patch_param_changed, notes_changed, …) are ignored; a
//! `get_state` request is sent to refresh the mirror after any such
//! message.  This keeps the connector simple at the cost of an
//! occasional full state round-trip when the remote is being
//! configured live.

use crate::config::{OverrideInfo, SliderRanges};
use crate::preview_state::{
  ConnectionStatus, HeartbeatState, PreviewState, Source, SourceKind,
};
use fundsp::prelude32::shared;
use futures::{SinkExt, StreamExt};
use parking_lot::RwLock;
use serde::Deserialize;
use sonify_health_lib::{
  HeartbeatConfig, NoteConfig, Patch, Playback, ResultMode, TierConfig,
  Transition,
};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

/// Initial backoff delay after a failed connection attempt.
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);

/// Cap on backoff between connection attempts.
const MAX_BACKOFF: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Wire-format structs
// ---------------------------------------------------------------------------

/// The subset of a remote `state` snapshot the mirror cares about.
/// Fields the mirror does not consume (patch_params, muted,
/// master_volume, config_writable, config_path, headless) are
/// silently ignored — there is no `deny_unknown_fields` here so the
/// remote can grow new fields without breaking older subscribers.
#[derive(Debug, Deserialize)]
pub struct WireStateSnapshot {
  pub library: HashMap<String, Patch>,
  pub heartbeats: Vec<WireHeartbeat>,
  #[serde(default)]
  pub slider_ranges: SliderRanges,
  #[serde(default)]
  pub overrides: HashMap<String, OverrideInfo>,
}

/// One heartbeat as seen on the wire.  This is `HeartbeatConfig`
/// plus the runtime `metric` value the local renderer needs in
/// order to play sounds at the right severity.  Kept as a separate
/// struct because `HeartbeatConfig` carries `deny_unknown_fields`
/// and would reject the runtime-only fields the snapshot adds
/// (`metric`, `overridden`).
#[derive(Debug, Deserialize)]
pub struct WireHeartbeat {
  pub name: String,
  pub command: String,
  pub result_mode: ResultMode,
  pub notes: Vec<WireNoteConfig>,
  pub playback: Playback,
  pub poll_interval_secs: f64,
  pub cycle_secs: f64,
  pub cycle_offset_secs: f64,
  pub crossfade_ms: f64,
  pub phrase_gap: f64,
  pub repeat_rate: f64,
  pub tiers: Vec<TierConfig>,
  /// Latest probe metric from the remote.  Mirrored into the
  /// `HeartbeatState::metric` Shared so the local renderer (and
  /// state_snapshot) sees the same value.
  pub metric: f32,
}

/// `NoteConfig` on the wire — same shape as the in-process struct
/// but without `deny_unknown_fields` so the wire can grow.
#[derive(Debug, Deserialize)]
pub struct WireNoteConfig {
  pub transition: Transition,
  pub volume: f64,
  pub offset: f64,
}

impl From<WireNoteConfig> for NoteConfig {
  fn from(w: WireNoteConfig) -> Self {
    NoteConfig {
      transition: w.transition,
      volume: w.volume,
      offset: w.offset,
    }
  }
}

impl WireHeartbeat {
  /// Convert the wire shape into the parts the mirror needs: a
  /// `HeartbeatConfig` (config-side state) and the runtime metric
  /// value that goes into `HeartbeatState::metric`.
  pub fn into_parts(self) -> (HeartbeatConfig, f32) {
    let cfg = HeartbeatConfig::new(
      self.name,
      self.command,
      self.result_mode,
      self.notes.into_iter().map(NoteConfig::from).collect(),
      self.playback,
      self.phrase_gap,
      self.repeat_rate,
      self.poll_interval_secs,
      self.cycle_secs,
      self.cycle_offset_secs,
      self.crossfade_ms,
      self.tiers,
    );
    (cfg, self.metric)
  }
}

// ---------------------------------------------------------------------------
// Mirror application
// ---------------------------------------------------------------------------

/// Apply a full state snapshot to a Remote Source's mirror,
/// replacing the patch library, heartbeat configs, slider ranges,
/// override map, and per-heartbeat metric values.  Existing
/// `HeartbeatState` entries are reused when the heartbeat list is
/// the same length so any `Shared` references handed out earlier
/// stay live; otherwise the list is rebuilt from scratch.
pub fn apply_state_snapshot(source: &Source, snapshot: WireStateSnapshot) {
  *source.library.write() = snapshot.library;
  *source.overrides.write() = snapshot.overrides;
  // SliderRanges is plain data (no RwLock) so we cannot replace it
  // via interior mutability; the field is owned by the Source.
  // For step 4 the mirror's slider_ranges stays at its initialized
  // default — they are presentation hints for the source's own UI,
  // and the local UI uses its own ranges.  When step 6 renders
  // remote sources distinctly, we will revisit how (or whether) to
  // propagate them.
  let _ = snapshot.slider_ranges;

  let (configs, metrics): (Vec<_>, Vec<_>) = snapshot
    .heartbeats
    .into_iter()
    .map(WireHeartbeat::into_parts)
    .unzip();

  *source.heartbeat_configs.write() = configs;

  let mut hbs = source.heartbeats.write();
  if hbs.len() == metrics.len() {
    // Same shape: write the new metric values into the existing
    // Shared instances so any handles already cloned to render
    // threads observe the update.
    for (hb, m) in hbs.iter().zip(metrics.iter()) {
      hb.metric.set_value(*m);
    }
  } else {
    *hbs = metrics
      .into_iter()
      .map(|m| HeartbeatState {
        metric: shared(m),
        override_value: RwLock::new(None),
        effective_volume: shared(1.0),
      })
      .collect();
  }
}

/// Apply an incremental `metric_changed` update.  Silently ignored
/// if `hb_idx` is out of range for the source's current mirror.
pub fn apply_metric_changed(source: &Source, hb_idx: usize, value: f32) {
  let hbs = source.heartbeats.read();
  if let Some(hb) = hbs.get(hb_idx) {
    hb.metric.set_value(value);
  }
}

// ---------------------------------------------------------------------------
// Connector loop
// ---------------------------------------------------------------------------

fn set_status(source: &Source, status: ConnectionStatus) {
  if let SourceKind::Remote { status: s, .. } = &source.kind {
    *s.write() = status;
  }
}

/// Run the outbound WebSocket loop for the Remote Source named
/// `source_name`.  Opens the connection, mirrors state, and on
/// disconnect or error sleeps with exponential backoff before
/// retrying.  Exits when `preview.running` flips to false, or when
/// the Source is removed at runtime (signaled via
/// `kind.is_alive() == false`).
pub async fn run_connector(preview: Arc<PreviewState>, source_name: String) {
  let source = match preview.source_by_name(&source_name) {
    Some(s) => s,
    None => {
      error!(
        source = %source_name,
        "run_connector called for a Source that no longer exists"
      );
      return;
    }
  };
  let url = match &source.kind {
    SourceKind::Remote { url, .. } => url.clone(),
    SourceKind::Local => {
      error!(
        source = %source_name,
        "run_connector called on a Local Source — refusing to connect"
      );
      return;
    }
  };

  let mut backoff = INITIAL_BACKOFF;

  while preview.running.load(Ordering::Relaxed) && source.kind.is_alive() {
    set_status(&source, ConnectionStatus::Connecting);
    info!(source = %source_name, url = %url, "Remote source connecting");

    match connect_async(&url).await {
      Ok((ws, _resp)) => {
        backoff = INITIAL_BACKOFF;
        set_status(&source, ConnectionStatus::Connected);
        info!(source = %source_name, "Remote source connected");

        let disconnect_reason =
          run_session(&preview, &source, &source_name, ws).await;

        warn!(
          source = %source_name,
          reason = %disconnect_reason,
          "Remote source disconnected"
        );
        set_status(
          &source,
          ConnectionStatus::Disconnected {
            error: Some(disconnect_reason),
          },
        );
      }
      Err(e) => {
        let msg = format!("{e}");
        warn!(
          source = %source_name,
          error = %msg,
          backoff_secs = backoff.as_secs(),
          "Remote source connection failed"
        );
        set_status(
          &source,
          ConnectionStatus::Disconnected { error: Some(msg) },
        );
      }
    }

    if !preview.running.load(Ordering::Relaxed) || !source.kind.is_alive() {
      break;
    }

    tokio::time::sleep(backoff).await;
    backoff = (backoff * 2).min(MAX_BACKOFF);
  }

  info!(source = %source_name, "Remote source connector stopping");
}

/// Drive a single connected session: read messages until the
/// stream ends or yields an error.  Returns a short reason
/// string for the disconnect log.
async fn run_session(
  preview: &Arc<PreviewState>,
  source: &Source,
  source_name: &str,
  ws: tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
  >,
) -> String {
  let (mut ws_tx, mut ws_rx) = ws.split();

  while let Some(frame) = ws_rx.next().await {
    if !preview.running.load(Ordering::Relaxed) {
      return "shutdown".to_string();
    }
    if !source.kind.is_alive() {
      return "source removed".to_string();
    }
    match frame {
      Ok(Message::Text(text)) => {
        if let Err(e) =
          handle_text_message(preview, source, source_name, &text, &mut ws_tx)
            .await
        {
          return format!("message-handling error: {e}");
        }
      }
      Ok(Message::Close(_)) => return "remote closed".to_string(),
      Ok(_) => {
        // Binary, Ping/Pong handled by tungstenite under the hood;
        // anything else is unexpected on this protocol.
      }
      Err(e) => return format!("read error: {e}"),
    }
  }
  "stream ended".to_string()
}

/// Parse and apply one text frame.  Unknown message types and
/// most incremental updates trigger a `get_state` request; the
/// next state snapshot from the remote brings the mirror back in
/// sync.
async fn handle_text_message(
  preview: &Arc<PreviewState>,
  source: &Source,
  source_name: &str,
  text: &str,
  ws_tx: &mut futures::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<
      tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    Message,
  >,
) -> Result<(), String> {
  let raw: serde_json::Value =
    serde_json::from_str(text).map_err(|e| format!("invalid JSON: {e}"))?;
  let msg_type = raw
    .get("type")
    .and_then(|v| v.as_str())
    .ok_or_else(|| "message missing `type` field".to_string())?;

  match msg_type {
    "state" => {
      let snapshot: WireStateSnapshot = serde_json::from_value(raw.clone())
        .map_err(|e| format!("invalid state snapshot: {e}"))?;
      apply_state_snapshot(source, snapshot);
      debug!(source = %source_name, "Mirrored full state snapshot");
      // Rebroadcast a local snapshot so any frontend connected to
      // this instance sees the freshly mirrored remote state.
      let _ = preview.broadcast_tx.send(preview.state_snapshot());
      Ok(())
    }
    "metric_changed" => {
      let hb_idx = raw
        .get("index")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "metric_changed missing `index`".to_string())?
        as usize;
      let value = raw
        .get("value")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| "metric_changed missing `value`".to_string())?
        as f32;
      apply_metric_changed(source, hb_idx, value);
      // Coarse-grained: rebroadcast a full local snapshot so the
      // frontend sees the metric change.  Step 6c will refine to
      // an incremental message that carries the source name.
      let _ = preview.broadcast_tx.send(preview.state_snapshot());
      Ok(())
    }
    // Anything else (config edits, library changes, probe logs,
    // …) is handled by asking the remote for a fresh snapshot.
    // Inefficient but correct, and avoids a per-message decoder
    // for every incremental type the protocol can grow.
    _ => {
      debug!(
        source = %source_name,
        msg_type,
        "Ignoring incremental update; requesting fresh state"
      );
      let req = serde_json::json!({"type": "get_state"}).to_string();
      ws_tx
        .send(Message::Text(req.into()))
        .await
        .map_err(|e| format!("send get_state: {e}"))
    }
  }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
  use super::*;
  use crate::metrics::Metrics;
  use crate::preview_state::PreviewState;
  use sonify_health_lib::builtin_library;
  use std::sync::atomic::AtomicBool;

  fn make_preview_with_remote() -> (Arc<PreviewState>, String) {
    let preview = PreviewState::new(
      builtin_library(),
      HashMap::new(),
      vec![],
      Arc::new(AtomicBool::new(false)),
      Arc::new(AtomicBool::new(true)),
      Metrics::new().expect("Metrics::new in test"),
      SliderRanges::default(),
      None,
      false,
      false,
    );
    let preview = Arc::new(preview);
    let name = "test-remote".to_string();
    preview
      .add_remote_source(name.clone(), "ws://example/ws".to_string())
      .unwrap();
    (preview, name)
  }

  fn sample_snapshot_json() -> String {
    // Mirrors what `state_snapshot` emits for one heartbeat.  We
    // include only the fields the deserializer reads; serde
    // ignores the rest.
    serde_json::json!({
      "type": "state",
      "library": {
        "sine": serde_json::to_value(Patch::default()).unwrap(),
      },
      "heartbeats": [{
        "name": "remote-hb",
        "command": "echo 0",
        "result_mode": "exit-code",
        "playback": "clock",
        "metric": 0.42,
        "overridden": false,
        "poll_interval_secs": 5.0,
        "cycle_secs": 10.0,
        "cycle_offset_secs": 0.0,
        "crossfade_ms": 100.0,
        "phrase_gap": 0.0,
        "repeat_rate": 1.0,
        "notes": [{
          "transition": {
            "type": "discrete",
            "states": [{"threshold": 1.01, "patch": "sine"}]
          },
          "volume": 0.3,
          "offset": 0.0,
        }],
        "tiers": [],
      }],
      "slider_ranges": serde_json::to_value(SliderRanges::default()).unwrap(),
      "overrides": {},
    })
    .to_string()
  }

  #[test]
  fn apply_state_snapshot_replaces_mirror() {
    let (preview, name) = make_preview_with_remote();
    let source = preview.source_by_name(&name).unwrap();
    let json = sample_snapshot_json();
    let snap: WireStateSnapshot =
      serde_json::from_str(&json).expect("snapshot decodes");

    apply_state_snapshot(&source, snap);

    let configs = source.heartbeat_configs.read();
    assert_eq!(configs.len(), 1);
    assert_eq!(configs[0].name, "remote-hb");
    drop(configs);

    let lib = source.library.read();
    assert!(lib.contains_key("sine"));
    drop(lib);

    let hbs = source.heartbeats.read();
    assert_eq!(hbs.len(), 1);
    assert!((hbs[0].metric.value() - 0.42).abs() < 1e-6);
  }

  #[test]
  fn apply_metric_changed_updates_existing_entry() {
    let (preview, name) = make_preview_with_remote();
    let source = preview.source_by_name(&name).unwrap();
    let snap: WireStateSnapshot =
      serde_json::from_str(&sample_snapshot_json()).unwrap();
    apply_state_snapshot(&source, snap);

    apply_metric_changed(&source, 0, 0.91);

    let hbs = source.heartbeats.read();
    assert!((hbs[0].metric.value() - 0.91).abs() < 1e-6);
  }

  #[test]
  fn apply_metric_changed_ignores_out_of_range() {
    let (preview, name) = make_preview_with_remote();
    let source = preview.source_by_name(&name).unwrap();
    // No heartbeats yet — the call should be a no-op, not panic.
    apply_metric_changed(&source, 0, 0.5);
    apply_metric_changed(&source, 99, 0.5);
  }

  #[test]
  fn snapshot_replace_preserves_shared_handles_when_shape_matches() {
    let (preview, name) = make_preview_with_remote();
    let source = preview.source_by_name(&name).unwrap();
    let snap: WireStateSnapshot =
      serde_json::from_str(&sample_snapshot_json()).unwrap();
    apply_state_snapshot(&source, snap);

    // Hold a clone of the existing Shared metric handle (this is
    // what the play thread would do).
    let metric_handle = {
      let hbs = source.heartbeats.read();
      hbs[0].metric.clone()
    };

    // Apply another snapshot with the same shape but different
    // metric value.
    let json = sample_snapshot_json().replace("0.42", "0.77");
    let snap: WireStateSnapshot = serde_json::from_str(&json).unwrap();
    apply_state_snapshot(&source, snap);
    let _ = preview;

    // The held handle reflects the new value — the underlying
    // atomic was reused rather than recreated.
    assert!((metric_handle.value() - 0.77).abs() < 1e-6);
  }
}
