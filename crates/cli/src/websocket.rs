use crate::config::{build_save_toml, OverrideInfo};
use crate::preview_state::PreviewState;
use axum::{
  extract::{
    ws::{Message, WebSocket, WebSocketUpgrade},
    State,
  },
  response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use serde_json::json;
use sonify_health_lib::heartbeat_config::default_volume;
use sonify_health_lib::{NoteConfig, Patch, Transition};
use std::collections::HashMap;
use std::sync::{atomic::Ordering, Arc};
use tokio::sync::broadcast;

use crate::web_base::AppState;

pub async fn ws_handler(
  ws: WebSocketUpgrade,
  State(state): State<AppState>,
) -> impl IntoResponse {
  ws.on_upgrade(move |socket| handle_socket(socket, state.preview.clone()))
}

async fn handle_socket(socket: WebSocket, preview: Arc<PreviewState>) {
  let (mut ws_sender, mut ws_receiver) = socket.split();

  let mut broadcast_rx = preview.broadcast_tx.subscribe();
  let mut log_rx = preview.probe_log_tx.subscribe();

  let (reply_tx, mut reply_rx) =
    tokio::sync::mpsc::unbounded_channel::<String>();

  // Send initial state snapshot.
  let snapshot = preview.state_snapshot();
  if ws_sender
    .send(Message::Text(snapshot.into()))
    .await
    .is_err()
  {
    return;
  }

  let send_task = tokio::spawn(async move {
    loop {
      let text = tokio::select! {
        msg = broadcast_rx.recv() => match msg {
          Ok(t) => t,
          Err(broadcast::error::RecvError::Lagged(_)) => continue,
          Err(_) => break,
        },
        msg = log_rx.recv() => match msg {
          Ok(t) => t,
          Err(broadcast::error::RecvError::Lagged(_)) => continue,
          Err(_) => break,
        },
        msg = reply_rx.recv() => match msg {
          Some(t) => t,
          None => break,
        },
      };
      if ws_sender.send(Message::Text(text.into())).await.is_err() {
        break;
      }
    }
  });

  let recv_preview = preview.clone();
  let recv_task = tokio::spawn(async move {
    while let Some(Ok(msg)) = ws_receiver.next().await {
      match msg {
        Message::Text(text) => {
          if let Some(reply) = handle_client_message(&recv_preview, &text) {
            let _ = reply_tx.send(reply);
          }
        }
        Message::Close(_) => break,
        _ => {}
      }
    }
  });

  tokio::select! {
    _ = send_task => {},
    _ = recv_task => {},
  }

  tracing::debug!("WebSocket client disconnected");
}

fn handle_client_message(preview: &PreviewState, text: &str) -> Option<String> {
  let msg: serde_json::Value = serde_json::from_str(text).ok()?;
  let msg_type = msg.get("type").and_then(|v| v.as_str())?;

  match msg_type {
    "get_state" => Some(preview.state_snapshot()),

    "set_patch_param" => {
      let patch_name = msg.get("patch_name").and_then(|v| v.as_str())?;
      let param = msg.get("param").and_then(|v| v.as_str())?;
      let value = msg.get("value").and_then(|v| v.as_f64())?;
      {
        let mut lib = preview.library.write().unwrap();
        let patch = lib.get_mut(patch_name)?;
        if !patch.set_param(param, value) {
          return None;
        }
      }
      let _ = preview.broadcast_tx.send(
        json!({
          "type": "patch_param_changed",
          "patch_name": patch_name,
          "param": param,
          "value": value,
        })
        .to_string(),
      );

      // If the target is an override patch, record in its delta.
      {
        let mut ovr = preview.overrides.write().unwrap();
        if let Some(info) = ovr.get_mut(patch_name) {
          info.delta.insert(param.to_string(), value);
        }
      }

      // If the target is a base patch, propagate to dependents
      // whose delta does not override this param.
      propagate_base_change(preview, patch_name, param, value);

      broadcast_library(preview);
      broadcast_overrides(preview);
      None
    }

    "set_note_volume" => set_note_field(
      preview,
      &msg,
      |nc, v| nc.volume = v.clamp(0.0, 1.0),
      "note_volume_changed",
    ),

    "set_note_offset" => set_note_field(
      preview,
      &msg,
      |nc, v| nc.offset = v.max(0.0),
      "note_offset_changed",
    ),

    "set_note_transition" => {
      let index = msg.get("index").and_then(|v| v.as_u64())? as usize;
      let note = msg.get("note").and_then(|v| v.as_u64())? as usize;
      let raw = msg.get("transition")?;
      let transition: Transition = serde_json::from_value(raw.clone()).ok()?;
      {
        let mut configs = preview.heartbeat_configs.write().unwrap();
        configs.get_mut(index)?.notes.get_mut(note)?.transition =
          transition.clone();
      }
      let _ = preview.broadcast_tx.send(
        json!({
          "type": "note_transition_changed",
          "index": index,
          "note": note,
          "transition": serde_json::to_value(&transition).unwrap_or_default(),
        })
        .to_string(),
      );
      None
    }

    "add_note" => {
      let index = msg.get("index").and_then(|v| v.as_u64())? as usize;
      let first_patch = {
        let lib = preview.library.read().unwrap();
        lib
          .keys()
          .next()
          .cloned()
          .unwrap_or_else(|| "sine".to_string())
      };
      let new_note = NoteConfig {
        transition: Transition::Discrete {
          states: vec![sonify_health_lib::transition::DiscreteState {
            threshold: 1.01,
            patch: first_patch,
          }],
        },
        volume: default_volume(),
        offset: 0.0,
      };
      let notes_json;
      {
        let mut configs = preview.heartbeat_configs.write().unwrap();
        let cfg = configs.get_mut(index)?;
        cfg.notes.push(new_note);
        notes_json = notes_to_json(&cfg.notes);
      }
      let _ = preview.broadcast_tx.send(
        json!({
          "type": "notes_changed",
          "index": index,
          "notes": notes_json,
        })
        .to_string(),
      );
      None
    }

    "remove_note" => {
      let index = msg.get("index").and_then(|v| v.as_u64())? as usize;
      let note = msg.get("note").and_then(|v| v.as_u64())? as usize;
      let notes_json;
      {
        let mut configs = preview.heartbeat_configs.write().unwrap();
        let cfg = configs.get_mut(index)?;
        if cfg.notes.len() <= 1 {
          return None;
        }
        if note >= cfg.notes.len() {
          return None;
        }
        cfg.notes.remove(note);
        notes_json = notes_to_json(&cfg.notes);
      }
      let _ = preview.broadcast_tx.send(
        json!({
          "type": "notes_changed",
          "index": index,
          "notes": notes_json,
        })
        .to_string(),
      );
      None
    }

    "override_heartbeat" => {
      let index = msg.get("index").and_then(|v| v.as_u64())? as usize;
      let value = msg.get("value").and_then(|v| v.as_f64())? as f32;
      let clamped = value.clamp(0.0, 1.0);
      let hb = preview.heartbeats.get(index)?;
      *hb.override_value.write().unwrap() = Some(clamped);
      hb.metric.set_value(clamped);
      let _ = preview.broadcast_tx.send(
        json!({
          "type": "override_changed",
          "index": index,
          "value": clamped,
          "overridden": true,
        })
        .to_string(),
      );
      None
    }

    "clear_override" => {
      let index = msg.get("index").and_then(|v| v.as_u64())? as usize;
      let hb = preview.heartbeats.get(index)?;
      *hb.override_value.write().unwrap() = None;
      let _ = preview.broadcast_tx.send(
        json!({
          "type": "override_changed",
          "index": index,
          "value": null,
          "overridden": false,
        })
        .to_string(),
      );
      None
    }

    "trigger_heartbeat" => {
      let index = msg.get("index").and_then(|v| v.as_u64())? as usize;
      preview.trigger_immediate_play(index);
      None
    }

    "play_patch" => {
      let name = msg.get("patch_name").and_then(|v| v.as_str())?;
      preview.play_patch_immediate(name);
      None
    }

    "set_muted" => {
      let muted = msg.get("muted").and_then(|v| v.as_bool())?;
      preview.muted.store(muted, Ordering::Relaxed);
      preview.update_all_effective_volumes();
      let _ = preview
        .broadcast_tx
        .send(json!({"type": "mute_changed", "muted": muted}).to_string());
      None
    }

    "set_master_volume" => {
      let vol = msg.get("volume").and_then(|v| v.as_f64())? as f32;
      preview.master_volume.set_value(vol.clamp(0.0, 1.0));
      preview.update_all_effective_volumes();
      let _ = preview.broadcast_tx.send(
        json!({
          "type": "volume_changed",
          "layer": "master",
          "volume": vol,
        })
        .to_string(),
      );
      None
    }

    "set_cycle_offset" => set_heartbeat_field(
      preview,
      &msg,
      |cfg, v| cfg.cycle_offset_secs = v,
      "cycle_offset_changed",
    ),

    "set_crossfade_ms" => set_heartbeat_field(
      preview,
      &msg,
      |cfg, v| cfg.crossfade_ms = v,
      "crossfade_ms_changed",
    ),

    "set_playback" => {
      let index = msg.get("index").and_then(|v| v.as_u64())? as usize;
      let raw = msg.get("value").and_then(|v| v.as_str())?;
      let playback: sonify_health_lib::Playback =
        serde_json::from_value(serde_json::Value::String(raw.to_string()))
          .ok()?;
      {
        let mut configs = preview.heartbeat_configs.write().unwrap();
        configs.get_mut(index)?.playback = playback;
      }
      let _ = preview.broadcast_tx.send(
        json!({
          "type": "playback_changed",
          "index": index,
          "value": raw,
        })
        .to_string(),
      );
      None
    }

    "revert_all" => {
      preview.revert();
      let snapshot = preview.state_snapshot();
      let _ = preview.broadcast_tx.send(snapshot);
      None
    }

    "export_config" => {
      let lib = preview.library.read().unwrap();
      let ovr = preview.overrides.read().unwrap();
      let lib_json = serde_json::to_value(&*lib).unwrap_or_default();
      let ovr_json = preview.overrides_json();
      drop(ovr);
      Some(
        json!({
          "type": "config_export",
          "library": lib_json,
          "overrides": ovr_json,
        })
        .to_string(),
      )
    }

    "import_config" => {
      let text = msg.get("text").and_then(|v| v.as_str())?;
      match parse_import(text) {
        Ok(patches) => {
          let mut lib = preview.library.write().unwrap();
          for (name, patch) in patches {
            lib.insert(name, patch);
          }
          drop(lib);
          broadcast_library(preview);
          let snapshot = preview.state_snapshot();
          let _ = preview.broadcast_tx.send(snapshot);
          None
        }
        Err(e) => Some(
          json!({
            "type": "import_error",
            "message": e,
          })
          .to_string(),
        ),
      }
    }

    "create_patch" => {
      let name = msg.get("name").and_then(|v| v.as_str())?;
      {
        let lib = preview.library.read().unwrap();
        if lib.contains_key(name) {
          return None;
        }
      }
      {
        let mut lib = preview.library.write().unwrap();
        lib.insert(name.to_string(), Patch::default());
      }
      let snapshot = preview.state_snapshot();
      let _ = preview.broadcast_tx.send(snapshot);
      None
    }

    "create_override" => {
      let base = msg.get("base").and_then(|v| v.as_str())?;
      let name = msg.get("name").and_then(|v| v.as_str())?;
      {
        let lib = preview.library.read().unwrap();
        // Name must not be taken.
        if lib.contains_key(name) {
          return None;
        }
        // Base must exist.
        if !lib.contains_key(base) {
          return None;
        }
      }
      // Base must not itself be an override.
      {
        let ovr = preview.overrides.read().unwrap();
        if ovr.contains_key(base) {
          return None;
        }
      }
      // Clone the base into the library and register the override.
      {
        let mut lib = preview.library.write().unwrap();
        let cloned = lib[base].clone();
        lib.insert(name.to_string(), cloned);
      }
      {
        let mut ovr = preview.overrides.write().unwrap();
        ovr.insert(
          name.to_string(),
          OverrideInfo {
            base: base.to_string(),
            delta: HashMap::new(),
          },
        );
      }
      let snapshot = preview.state_snapshot();
      let _ = preview.broadcast_tx.send(snapshot);
      None
    }

    "rename_patch" => {
      let old_name = msg.get("old_name").and_then(|v| v.as_str())?;
      let new_name = msg.get("new_name").and_then(|v| v.as_str())?;
      if old_name == new_name || new_name.is_empty() {
        return None;
      }
      {
        let lib = preview.library.read().unwrap();
        if !lib.contains_key(old_name) || lib.contains_key(new_name) {
          return None;
        }
      }
      // Rename in library.
      {
        let mut lib = preview.library.write().unwrap();
        if let Some(patch) = lib.remove(old_name) {
          lib.insert(new_name.to_string(), patch);
        }
      }
      // Update heartbeat configs: transition patch references.
      {
        let mut configs = preview.heartbeat_configs.write().unwrap();
        for cfg in configs.iter_mut() {
          for note in &mut cfg.notes {
            rename_in_transition(&mut note.transition, old_name, new_name);
          }
        }
      }
      // Update overrides map.
      {
        let mut ovr = preview.overrides.write().unwrap();
        // If any override has old_name as its base, update it.
        for info in ovr.values_mut() {
          if info.base == old_name {
            info.base = new_name.to_string();
          }
        }
        // If the override itself was renamed, move the key.
        if let Some(info) = ovr.remove(old_name) {
          ovr.insert(new_name.to_string(), info);
        }
      }
      let snapshot = preview.state_snapshot();
      let _ = preview.broadcast_tx.send(snapshot);
      None
    }

    "reset_override_param" => {
      let patch_name = msg.get("patch_name").and_then(|v| v.as_str())?;
      let param = msg.get("param").and_then(|v| v.as_str())?;
      let base_name;
      {
        let mut ovr = preview.overrides.write().unwrap();
        let info = ovr.get_mut(patch_name)?;
        info.delta.remove(param);
        base_name = info.base.clone();
      }
      // Copy the base's current value for this param into the
      // resolved override patch.
      {
        let mut lib = preview.library.write().unwrap();
        let base_val = lib.get(&base_name)?.get_param(param)?;
        lib.get_mut(patch_name)?.set_param(param, base_val);
      }
      let _ = preview.broadcast_tx.send(
        json!({
          "type": "patch_param_changed",
          "patch_name": patch_name,
          "param": param,
          "value": preview.library.read().unwrap()
            .get(patch_name)
            .and_then(|p| p.get_param(param))
            .unwrap_or(0.0),
        })
        .to_string(),
      );
      broadcast_library(preview);
      broadcast_overrides(preview);
      None
    }

    "save_config" => {
      let config_path = match (&preview.config_path, preview.config_writable) {
        (Some(p), true) => p.clone(),
        _ => {
          return Some(
            json!({
              "type": "save_error",
              "message": "No writable config file available.",
            })
            .to_string(),
          );
        }
      };

      let lib = preview.library.read().unwrap();
      let ovr = preview.overrides.read().unwrap();
      let hb_configs = preview.heartbeat_configs.read().unwrap();

      let toml_str = match build_save_toml(
        &lib,
        &ovr,
        &hb_configs,
        &preview.slider_ranges,
      ) {
        Ok(s) => s,
        Err(e) => {
          return Some(
            json!({
              "type": "save_error",
              "message": format!("Serialization failed: {e}"),
            })
            .to_string(),
          );
        }
      };
      drop(lib);
      drop(ovr);
      drop(hb_configs);

      // Atomic write: temp file + rename.
      let dir = config_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
      match tempfile::NamedTempFile::new_in(dir) {
        Ok(mut tmp) => {
          use std::io::Write;
          if let Err(e) = tmp.write_all(toml_str.as_bytes()) {
            return Some(
              json!({
                "type": "save_error",
                "message": format!("Write failed: {e}"),
              })
              .to_string(),
            );
          }
          if let Err(e) = tmp.persist(&config_path) {
            return Some(
              json!({
                "type": "save_error",
                "message": format!("Rename failed: {e}"),
              })
              .to_string(),
            );
          }
          tracing::info!(path = %config_path.display(), "Config saved");
          let _ = preview
            .broadcast_tx
            .send(json!({"type": "config_saved"}).to_string());
          None
        }
        Err(e) => Some(
          json!({
            "type": "save_error",
            "message": format!("Failed to create temp file: {e}"),
          })
          .to_string(),
        ),
      }
    }

    _ => None,
  }
}

/// Extract index and value from a message, clamp, apply a field
/// setter on the heartbeat config, and broadcast the change.
fn set_heartbeat_field(
  preview: &PreviewState,
  msg: &serde_json::Value,
  setter: impl FnOnce(&mut sonify_health_lib::HeartbeatConfig, f64),
  changed_type: &str,
) -> Option<String> {
  let index = msg.get("index").and_then(|v| v.as_u64())? as usize;
  let value = msg.get("value").and_then(|v| v.as_f64())?;
  let clamped = value.max(0.0);
  {
    let mut configs = preview.heartbeat_configs.write().unwrap();
    setter(configs.get_mut(index)?, clamped);
  }
  let _ = preview.broadcast_tx.send(
    json!({
      "type": changed_type,
      "index": index,
      "value": clamped,
    })
    .to_string(),
  );
  None
}

/// Extract index, note, and value from a message, apply a field
/// setter on the note config, and broadcast the change.
fn set_note_field(
  preview: &PreviewState,
  msg: &serde_json::Value,
  setter: impl FnOnce(&mut NoteConfig, f64),
  changed_type: &str,
) -> Option<String> {
  let index = msg.get("index").and_then(|v| v.as_u64())? as usize;
  let note_idx = msg.get("note").and_then(|v| v.as_u64())? as usize;
  let value = msg.get("value").and_then(|v| v.as_f64())?;
  {
    let mut configs = preview.heartbeat_configs.write().unwrap();
    setter(configs.get_mut(index)?.notes.get_mut(note_idx)?, value);
  }
  let _ = preview.broadcast_tx.send(
    json!({
      "type": changed_type,
      "index": index,
      "note": note_idx,
      "value": value,
    })
    .to_string(),
  );
  None
}

/// Serialize a notes list to JSON values.
fn notes_to_json(notes: &[NoteConfig]) -> Vec<serde_json::Value> {
  notes
    .iter()
    .map(|nc| {
      json!({
        "volume": nc.volume,
        "offset": nc.offset,
        "transition": serde_json::to_value(&nc.transition).unwrap_or_default(),
      })
    })
    .collect()
}

/// Broadcast the full library to all connected clients.
fn broadcast_library(preview: &PreviewState) {
  let lib = preview.library.read().unwrap();
  let lib_json: serde_json::Map<String, serde_json::Value> = lib
    .iter()
    .map(|(name, patch)| {
      (name.clone(), serde_json::to_value(patch).unwrap_or_default())
    })
    .collect();
  let _ = preview.broadcast_tx.send(
    json!({
      "type": "library_changed",
      "library": lib_json,
    })
    .to_string(),
  );
}

/// Broadcast the overrides map to all connected clients.
fn broadcast_overrides(preview: &PreviewState) {
  let _ = preview.broadcast_tx.send(
    json!({
      "type": "overrides_changed",
      "overrides": preview.overrides_json(),
    })
    .to_string(),
  );
}

/// When a base patch parameter changes, propagate it to override
/// patches that have not overridden that specific parameter.
fn propagate_base_change(
  preview: &PreviewState,
  base_name: &str,
  param: &str,
  value: f64,
) {
  let ovr = preview.overrides.read().unwrap();
  let dependents: Vec<String> = ovr
    .iter()
    .filter(|(_, info)| {
      info.base == base_name && !info.delta.contains_key(param)
    })
    .map(|(name, _)| name.clone())
    .collect();
  drop(ovr);

  if dependents.is_empty() {
    return;
  }

  let mut lib = preview.library.write().unwrap();
  for dep_name in &dependents {
    if let Some(patch) = lib.get_mut(dep_name) {
      patch.set_param(param, value);
    }
  }
  drop(lib);

  for dep_name in &dependents {
    let _ = preview.broadcast_tx.send(
      json!({
        "type": "patch_param_changed",
        "patch_name": dep_name,
        "param": param,
        "value": value,
      })
      .to_string(),
    );
  }
}

/// Rename a patch reference inside a transition.
fn rename_in_transition(
  transition: &mut Transition,
  old_name: &str,
  new_name: &str,
) {
  match transition {
    Transition::Gradient {
      patches,
      segments: _,
    } => {
      for p in patches.iter_mut() {
        if p == old_name {
          *p = new_name.to_string();
        }
      }
    }
    Transition::Discrete { states } => {
      for s in states.iter_mut() {
        if s.patch == old_name {
          s.patch = new_name.to_string();
        }
      }
    }
  }
}

/// Auto-detect format and parse patches from imported text.
fn parse_import(text: &str) -> Result<Vec<(String, Patch)>, String> {
  let trimmed = text.trim();
  if trimmed.starts_with('{') {
    let map: std::collections::HashMap<String, Patch> =
      serde_json::from_str(trimmed)
        .map_err(|e| format!("JSON parse error: {e}"))?;
    Ok(map.into_iter().collect())
  } else {
    let map: std::collections::HashMap<String, Patch> =
      toml::from_str(trimmed).map_err(|e| format!("TOML parse error: {e}"))?;
    Ok(map.into_iter().collect())
  }
}
