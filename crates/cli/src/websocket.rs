use crate::preview_state::{PatchOwner, PreviewState};
use axum::{
  extract::{
    ws::{Message, WebSocket, WebSocketUpgrade},
    State,
  },
  response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use serde_json::json;
use sonify_health_lib::{NoteSpec, Patch, PatchOverrides};
use std::sync::{atomic::Ordering, Arc};
use tokio::sync::broadcast;
use tracing::debug;

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
  let mut log_rx = preview.check_log_tx.subscribe();

  // Channel for direct replies (get_state, export_toml).
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

  // Forward broadcasts, log entries, and direct replies to the
  // WebSocket client.
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

  // Parse incoming client messages and dispatch.
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

  debug!("WebSocket client disconnected");
}

/// Broadcast current drone specs + pins for a specific drone to all
/// connected clients.
fn broadcast_drone_specs(preview: &PreviewState, index: usize) {
  let all_specs = preview.drone_boop_specs.read().unwrap();
  let all_pins = preview.drone_boop_pins.read().unwrap();
  let specs = all_specs.get(index).cloned().unwrap_or_default();
  let pins = all_pins.get(index).cloned().unwrap_or_default();
  let specs_json: Vec<_> = specs
    .iter()
    .enumerate()
    .map(|(j, spec)| {
      json!({
        "freq": spec.freq,
        "duration": spec.duration,
        "pinned": pins.get(j).copied().unwrap_or(false),
      })
    })
    .collect();
  let _ = preview.broadcast_tx.send(
    json!({
      "type": "drone_specs_changed",
      "index": index,
      "specs": specs_json,
    })
    .to_string(),
  );
}

/// Broadcast current boop specs + pins to all connected clients.
fn broadcast_boop_specs(preview: &PreviewState) {
  let specs = preview.boop_specs.read().unwrap();
  let pins = preview.boop_pins.read().unwrap();
  let specs_json: Vec<_> = specs
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
  let _ = preview.broadcast_tx.send(
    json!({
      "type": "boop_specs_changed",
      "specs": specs_json,
    })
    .to_string(),
  );
}

/// Broadcast locked params for a specific entity to all connected
/// clients.
fn broadcast_locked_params(preview: &PreviewState, owner: &PatchOwner) {
  let locked = preview.locked_params.read().unwrap();
  let params: Vec<_> = locked
    .get(owner)
    .map(|s| s.iter().collect())
    .unwrap_or_default();
  let (layer, index) = match owner {
    PatchOwner::Heartbeat => ("heartbeat", None),
    PatchOwner::DroneLo(i) => ("drone_lo", Some(*i)),
    PatchOwner::DroneHi(i) => ("drone_hi", Some(*i)),
  };
  let mut msg = json!({
    "type": "locked_params_changed",
    "layer": layer,
    "params": params,
  });
  if let Some(i) = index {
    msg["index"] = json!(i);
  }
  let _ = preview.broadcast_tx.send(msg.to_string());
}

/// Broadcast current locked drone indices to all connected clients.
fn broadcast_locked_drones(preview: &PreviewState) {
  let locked = preview.locked_drones.read().unwrap();
  let locked_json: Vec<_> = locked.iter().collect();
  let _ = preview.broadcast_tx.send(
    json!({
      "type": "locked_drones_changed",
      "indices": locked_json,
    })
    .to_string(),
  );
}

/// Parse a `PatchOwner` from the `layer` and optional `index`
/// fields of a WebSocket message.
fn parse_patch_owner(msg: &serde_json::Value) -> Option<PatchOwner> {
  let layer = msg.get("layer").and_then(|v| v.as_str())?;
  let index = msg
    .get("index")
    .and_then(|v| v.as_u64())
    .map(|v| v as usize);
  PatchOwner::from_layer_index(layer, index)
}

/// Dispatch a single client message.  Returns `Some(reply)` for
/// messages that should go only to the requesting client.
/// Broadcast side-effects are fired inline.
fn handle_client_message(preview: &PreviewState, text: &str) -> Option<String> {
  let msg: serde_json::Value = serde_json::from_str(text).ok()?;
  let msg_type = msg.get("type").and_then(|v| v.as_str())?;

  match msg_type {
    "get_state" => Some(preview.state_snapshot()),

    "set_patch_param" => {
      let owner = parse_patch_owner(&msg)?;
      let param = msg.get("param").and_then(|v| v.as_str())?;
      let value = msg.get("value").and_then(|v| v.as_f64())?;
      {
        let mut patches = preview.patches.write().unwrap();
        let patch = patches.get_mut(&owner)?;
        if !patch.set_param(param, value) {
          return None;
        }
      }
      let (layer, index) = match &owner {
        PatchOwner::Heartbeat => ("heartbeat", None),
        PatchOwner::DroneLo(i) => ("drone_lo", Some(*i)),
        PatchOwner::DroneHi(i) => ("drone_hi", Some(*i)),
      };
      let mut broadcast = json!({
        "type": "param_changed",
        "layer": layer,
        "param": param,
        "value": value,
      });
      if let Some(i) = index {
        broadcast["index"] = json!(i);
      }
      let _ = preview.broadcast_tx.send(broadcast.to_string());
      match &owner {
        PatchOwner::Heartbeat if matches!(param, "note_seed" | "base_freq") => {
          preview.recompute_boop_specs();
          broadcast_boop_specs(preview);
        }
        PatchOwner::DroneLo(i)
          if matches!(param, "note_seed" | "base_freq") =>
        {
          preview.recompute_drone_specs(*i);
          broadcast_drone_specs(preview, *i);
        }
        _ => {}
      }
      None
    }

    "set_muted" => {
      let muted = msg.get("muted").and_then(|v| v.as_bool())?;
      preview.muted.store(muted, Ordering::Relaxed);
      preview.update_all_combined_volumes();
      preview.update_effective_heartbeat_volume();
      let _ = preview
        .broadcast_tx
        .send(json!({"type": "mute_changed", "muted": muted}).to_string());
      None
    }

    "set_master_volume" => {
      let vol = msg.get("volume").and_then(|v| v.as_f64())? as f32;
      preview.master_volume.set_value(vol.clamp(0.0, 1.0));
      preview.update_all_combined_volumes();
      preview.update_effective_heartbeat_volume();
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

    "set_heartbeat_volume" => {
      let vol = msg.get("volume").and_then(|v| v.as_f64())? as f32;
      preview.heartbeat_volume.set_value(vol.clamp(0.0, 1.0));
      preview.update_effective_heartbeat_volume();
      let _ = preview.broadcast_tx.send(
        json!({
          "type": "volume_changed",
          "layer": "heartbeat",
          "volume": vol,
        })
        .to_string(),
      );
      None
    }

    "set_drone_interp_curve" => {
      let index = msg.get("index").and_then(|v| v.as_u64())? as usize;
      let curve = msg.get("curve").and_then(|v| v.as_f64())? as f32;
      let clamped = curve.clamp(0.1, 5.0);
      preview.drone_interp_curves.get(index)?.set_value(clamped);
      let _ = preview.broadcast_tx.send(
        json!({
          "type": "drone_interp_curve_changed",
          "index": index,
          "curve": clamped,
        })
        .to_string(),
      );
      None
    }

    "override_check" => {
      let layer = msg.get("layer").and_then(|v| v.as_str())?;
      let index = msg.get("index").and_then(|v| v.as_u64())? as usize;

      match layer {
        "heartbeat" => {
          let val = msg.get("value").and_then(|v| v.as_f64())? as f32;
          let clamped = val.clamp(0.0, 1.0);
          {
            let mut ov = preview.heartbeat_overrides.write().unwrap();
            *ov.get_mut(index)? = Some(clamped);
          }
          preview.heartbeat_state.set(index, clamped);
          let _ = preview.broadcast_tx.send(
            json!({
              "type": "override_changed",
              "layer": "heartbeat",
              "index": index,
              "value": clamped,
              "overridden": true,
            })
            .to_string(),
          );
        }
        "drone" => {
          let val = msg.get("value").and_then(|v| v.as_f64())? as f32;
          let clamped = val.clamp(0.0, 1.0);
          {
            let mut ov = preview.drone_overrides.write().unwrap();
            *ov.get_mut(index)? = Some(clamped);
          }
          preview.drone_state.set(index, clamped);
          let _ = preview.broadcast_tx.send(
            json!({
              "type": "override_changed",
              "layer": "drone",
              "index": index,
              "value": clamped,
              "overridden": true,
            })
            .to_string(),
          );
        }
        _ => {}
      }
      None
    }

    "clear_override" => {
      let layer = msg.get("layer").and_then(|v| v.as_str())?;
      let index = msg.get("index").and_then(|v| v.as_u64())? as usize;

      match layer {
        "heartbeat" => {
          let mut ov = preview.heartbeat_overrides.write().unwrap();
          *ov.get_mut(index)? = None;
        }
        "drone" => {
          let mut ov = preview.drone_overrides.write().unwrap();
          *ov.get_mut(index)? = None;
        }
        _ => {}
      }
      let _ = preview.broadcast_tx.send(
        json!({
          "type": "override_changed",
          "layer": layer,
          "index": index,
          "value": null,
          "overridden": false,
        })
        .to_string(),
      );
      None
    }

    "set_heartbeat_loop" => {
      let enabled = msg.get("enabled").and_then(|v| v.as_bool())?;
      preview.heartbeat_loop.store(enabled, Ordering::Relaxed);
      let _ = preview.broadcast_tx.send(
        json!({
          "type": "heartbeat_loop_changed",
          "enabled": enabled,
        })
        .to_string(),
      );
      None
    }

    "set_boop_count" => {
      let count = msg.get("count").and_then(|v| v.as_u64())? as usize;
      let clamped = count.clamp(1, 8);
      preview.boop_count.store(clamped, Ordering::Relaxed);
      let _ = preview.broadcast_tx.send(
        json!({
          "type": "boop_count_changed",
          "count": clamped,
        })
        .to_string(),
      );
      preview.recompute_boop_specs();
      broadcast_boop_specs(preview);
      None
    }

    "trigger_heartbeat" => {
      preview.heartbeat_trigger.store(true, Ordering::Relaxed);
      None
    }

    "revert_all" => {
      preview.revert();
      let snapshot = preview.state_snapshot();
      let _ = preview.broadcast_tx.send(snapshot);
      None
    }

    "set_drone_freq" => {
      let index = msg.get("index").and_then(|v| v.as_u64())? as usize;
      let freq = msg.get("freq").and_then(|v| v.as_f64())?;
      {
        let mut patches = preview.patches.write().unwrap();
        if let Some(p) = patches.get_mut(&PatchOwner::DroneLo(index)) {
          p.base_freq = freq;
        }
        if let Some(p) = patches.get_mut(&PatchOwner::DroneHi(index)) {
          p.base_freq = freq;
        }
      }
      for layer in &["drone_lo", "drone_hi"] {
        let _ = preview.broadcast_tx.send(
          json!({
            "type": "param_changed",
            "layer": layer,
            "index": index,
            "param": "base_freq",
            "value": freq,
          })
          .to_string(),
        );
      }
      preview.recompute_drone_specs(index);
      broadcast_drone_specs(preview, index);
      None
    }

    "set_drone_boops" => {
      let index = msg.get("index").and_then(|v| v.as_u64())? as usize;
      let boops = msg.get("boops").and_then(|v| v.as_u64())? as usize;
      let clamped = boops.clamp(1, 8);
      {
        let mut infos = preview.drone_infos.write().unwrap();
        infos.get_mut(index)?.boops = clamped;
      }
      let info = preview.drone_infos.read().unwrap()[index].clone();
      let _ = preview.broadcast_tx.send(
        json!({
          "type": "drone_config_changed",
          "index": index,
          "boops": info.boops,
        })
        .to_string(),
      );
      preview.recompute_drone_specs(index);
      broadcast_drone_specs(preview, index);
      None
    }

    "lock_param" => {
      let owner = parse_patch_owner(&msg)?;
      let param = msg.get("param").and_then(|v| v.as_str())?;
      if !Patch::PARAMS.iter().any(|p| p.name == param) {
        return None;
      }
      preview
        .locked_params
        .write()
        .unwrap()
        .entry(owner.clone())
        .or_default()
        .insert(param.to_string());
      broadcast_locked_params(preview, &owner);
      None
    }

    "unlock_param" => {
      let owner = parse_patch_owner(&msg)?;
      let param = msg.get("param").and_then(|v| v.as_str())?;
      {
        let mut locked = preview.locked_params.write().unwrap();
        if let Some(set) = locked.get_mut(&owner) {
          set.remove(param);
        }
      }
      broadcast_locked_params(preview, &owner);
      None
    }

    "unlock_all" => {
      let owners: Vec<PatchOwner> = preview
        .locked_params
        .read()
        .unwrap()
        .keys()
        .cloned()
        .collect();
      preview.locked_params.write().unwrap().clear();
      preview.locked_drones.write().unwrap().clear();
      for owner in &owners {
        broadcast_locked_params(preview, owner);
      }
      broadcast_locked_drones(preview);
      None
    }

    "lock_drone" => {
      let index = msg.get("index").and_then(|v| v.as_u64())? as usize;
      if index >= preview.combined_volumes.len() {
        return None;
      }
      preview.locked_drones.write().unwrap().insert(index);
      broadcast_locked_drones(preview);
      None
    }

    "unlock_drone" => {
      let index = msg.get("index").and_then(|v| v.as_u64())? as usize;
      preview.locked_drones.write().unwrap().remove(&index);
      broadcast_locked_drones(preview);
      None
    }

    "set_boop_spec" => {
      let index = msg.get("index").and_then(|v| v.as_u64())? as usize;
      {
        let mut specs = preview.boop_specs.write().unwrap();
        let mut pins = preview.boop_pins.write().unwrap();
        let spec = specs.get_mut(index)?;
        if let Some(freq) = msg.get("freq").and_then(|v| v.as_f64()) {
          spec.freq = freq;
        }
        if let Some(duration) = msg.get("duration").and_then(|v| v.as_f64()) {
          spec.duration = duration;
        }
        if let Some(pin) = pins.get_mut(index) {
          *pin = true;
        }
      }
      broadcast_boop_specs(preview);
      None
    }

    "clear_boop_pin" => {
      let index = msg.get("index").and_then(|v| v.as_u64())? as usize;
      {
        let mut pins = preview.boop_pins.write().unwrap();
        if let Some(pin) = pins.get_mut(index) {
          *pin = false;
        }
      }
      preview.recompute_boop_specs();
      broadcast_boop_specs(preview);
      None
    }

    "set_drone_spec" => {
      let index = msg.get("index").and_then(|v| v.as_u64())? as usize;
      let note_index = msg.get("note_index").and_then(|v| v.as_u64())? as usize;
      {
        let mut all_specs = preview.drone_boop_specs.write().unwrap();
        let mut all_pins = preview.drone_boop_pins.write().unwrap();
        let specs = all_specs.get_mut(index)?;
        let spec = specs.get_mut(note_index)?;
        if let Some(freq) = msg.get("freq").and_then(|v| v.as_f64()) {
          spec.freq = freq;
        }
        if let Some(duration) = msg.get("duration").and_then(|v| v.as_f64()) {
          spec.duration = duration;
        }
        let pins = all_pins.get_mut(index)?;
        if let Some(pin) = pins.get_mut(note_index) {
          *pin = true;
        }
      }
      broadcast_drone_specs(preview, index);
      None
    }

    "clear_drone_pin" => {
      let index = msg.get("index").and_then(|v| v.as_u64())? as usize;
      let note_index = msg.get("note_index").and_then(|v| v.as_u64())? as usize;
      {
        let mut all_pins = preview.drone_boop_pins.write().unwrap();
        if let Some(pins) = all_pins.get_mut(index) {
          if let Some(pin) = pins.get_mut(note_index) {
            *pin = false;
          }
        }
      }
      preview.recompute_drone_specs(index);
      broadcast_drone_specs(preview, index);
      None
    }

    "export_toml" => {
      let toml = preview.export_toml();
      let json_str = preview.export_json();
      let nix = preview.export_nix();
      Some(
        json!({
          "type": "patch_export",
          "toml": toml,
          "json": json_str,
          "nix": nix,
        })
        .to_string(),
      )
    }

    "import_config" => {
      let text = msg.get("text").and_then(|v| v.as_str())?;
      match parse_import(text) {
        Ok(data) => {
          apply_import(preview, &data);
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

    _ => None,
  }
}

// Import uses the generated PatchOverrides for deserialization.

#[derive(serde::Deserialize)]
struct ImportBoop {
  freq: f64,
  duration: f64,
}

#[derive(Default, serde::Deserialize)]
struct ImportHeartbeatSection {
  patch: Option<PatchOverrides>,
  notes: Option<Vec<ImportBoop>>,
}

#[derive(Default, serde::Deserialize)]
struct ImportDroneProfile {
  lo: Option<PatchOverrides>,
  hi: Option<PatchOverrides>,
}

#[derive(serde::Deserialize)]
struct ImportToml {
  patch: Option<PatchOverrides>,
  heartbeat: Option<ImportHeartbeatSection>,
  drone_profiles: Option<std::collections::HashMap<String, ImportDroneProfile>>,
  drone_notes: Option<std::collections::HashMap<String, Vec<ImportBoop>>>,
}

#[derive(serde::Deserialize)]
struct ImportJson {
  patch: Option<PatchOverrides>,
  heartbeat: Option<ImportHeartbeatSection>,
  drone_profiles: Option<std::collections::HashMap<String, ImportDroneProfile>>,
  drone_notes: Option<std::collections::HashMap<String, Vec<ImportBoop>>>,
}

/// Per-entity patch overrides plus heartbeat note specs.
struct ImportData {
  heartbeat_params: Vec<(&'static str, f64)>,
  drone_lo_params: std::collections::HashMap<String, Vec<(&'static str, f64)>>,
  drone_hi_params: std::collections::HashMap<String, Vec<(&'static str, f64)>>,
  boops: Vec<NoteSpec>,
  drone_notes: std::collections::HashMap<String, Vec<NoteSpec>>,
}

type ImportResult = Result<ImportData, String>;

/// Auto-detect format and parse into per-entity patch param overrides
/// + note specs.
fn parse_import(text: &str) -> ImportResult {
  let trimmed = text.trim();
  if trimmed.starts_with('{') {
    parse_import_json(trimmed)
  } else {
    parse_import_toml(trimmed)
  }
}

fn boops_from_import(raw: &[ImportBoop]) -> Vec<NoteSpec> {
  raw
    .iter()
    .map(|b| NoteSpec {
      freq: b.freq,
      duration: b.duration,
    })
    .collect()
}

fn build_import_data(
  legacy_patch: Option<&PatchOverrides>,
  heartbeat: Option<ImportHeartbeatSection>,
  drone_profiles: Option<std::collections::HashMap<String, ImportDroneProfile>>,
  drone_notes_raw: Option<std::collections::HashMap<String, Vec<ImportBoop>>>,
) -> ImportData {
  // Heartbeat patch: prefer heartbeat.patch, fall back to bare [patch]
  // for backwards compatibility.
  let heartbeat_params = heartbeat
    .as_ref()
    .and_then(|hb| hb.patch.as_ref())
    .or(legacy_patch)
    .map(|p| p.to_fields())
    .unwrap_or_default();

  let boops = heartbeat
    .and_then(|hb| hb.notes)
    .as_deref()
    .map(boops_from_import)
    .unwrap_or_default();

  let profiles = drone_profiles.unwrap_or_default();
  let drone_lo_params = profiles
    .iter()
    .filter_map(|(name, p)| {
      p.lo.as_ref().map(|v| (name.clone(), v.to_fields()))
    })
    .collect();
  let drone_hi_params = profiles
    .iter()
    .filter_map(|(name, p)| {
      p.hi.as_ref().map(|v| (name.clone(), v.to_fields()))
    })
    .collect();

  let drone_notes = drone_notes_raw
    .unwrap_or_default()
    .into_iter()
    .map(|(name, raw)| (name, boops_from_import(&raw)))
    .collect();

  ImportData {
    heartbeat_params,
    drone_lo_params,
    drone_hi_params,
    boops,
    drone_notes,
  }
}

fn parse_import_json(text: &str) -> ImportResult {
  let parsed: ImportJson =
    serde_json::from_str(text).map_err(|e| format!("JSON parse error: {e}"))?;
  Ok(build_import_data(
    parsed.patch.as_ref(),
    parsed.heartbeat,
    parsed.drone_profiles,
    parsed.drone_notes,
  ))
}

fn parse_import_toml(text: &str) -> ImportResult {
  let parsed: ImportToml =
    toml::from_str(text).map_err(|e| format!("TOML parse error: {e}"))?;
  Ok(build_import_data(
    parsed.patch.as_ref(),
    parsed.heartbeat,
    parsed.drone_profiles,
    parsed.drone_notes,
  ))
}

/// Apply imported per-entity patch params and note specs to preview
/// state.  Per-entity locked params are respected.  Imported notes
/// are pinned.
fn apply_import(preview: &PreviewState, data: &ImportData) {
  let locked = preview.locked_params.read().unwrap().clone();

  let is_locked = |owner: &PatchOwner, name: &str| -> bool {
    locked.get(owner).map(|s| s.contains(name)).unwrap_or(false)
  };

  {
    let mut patches = preview.patches.write().unwrap();

    // Heartbeat patch params.
    if let Some(patch) = patches.get_mut(&PatchOwner::Heartbeat) {
      for &(name, value) in &data.heartbeat_params {
        if !is_locked(&PatchOwner::Heartbeat, name) {
          patch.set_param(name, value);
        }
      }
    }

    // Drone profile params (matched by name).
    let drone_infos = preview.drone_infos.read().unwrap();
    for (i, info) in drone_infos.iter().enumerate() {
      if let Some(params) = data.drone_lo_params.get(&info.name) {
        let owner = PatchOwner::DroneLo(i);
        if let Some(patch) = patches.get_mut(&owner) {
          for &(name, value) in params {
            if !is_locked(&owner, name) {
              patch.set_param(name, value);
            }
          }
        }
      }
      if let Some(params) = data.drone_hi_params.get(&info.name) {
        let owner = PatchOwner::DroneHi(i);
        if let Some(patch) = patches.get_mut(&owner) {
          for &(name, value) in params {
            if !is_locked(&owner, name) {
              patch.set_param(name, value);
            }
          }
        }
      }
    }
  }

  if !data.boops.is_empty() {
    let mut specs = preview.boop_specs.write().unwrap();
    let mut pins = preview.boop_pins.write().unwrap();
    for (i, boop) in data.boops.iter().enumerate() {
      if i < specs.len() {
        specs[i] = boop.clone();
        if i < pins.len() {
          pins[i] = true;
        }
      }
    }
  }

  // Apply imported drone notes as pinned specs.
  if !data.drone_notes.is_empty() {
    let drone_infos = preview.drone_infos.read().unwrap();
    let mut all_specs = preview.drone_boop_specs.write().unwrap();
    let mut all_pins = preview.drone_boop_pins.write().unwrap();
    for (i, info) in drone_infos.iter().enumerate() {
      if let Some(notes) = data.drone_notes.get(&info.name) {
        if let (Some(specs), Some(pins)) =
          (all_specs.get_mut(i), all_pins.get_mut(i))
        {
          *specs = notes.clone();
          *pins = vec![true; notes.len()];
        }
      }
    }
  }

  preview.recompute_boop_specs();
  let drone_count = preview.drone_infos.read().unwrap().len();
  for i in 0..drone_count {
    preview.recompute_drone_specs(i);
  }

  let snapshot = preview.state_snapshot();
  let _ = preview.broadcast_tx.send(snapshot);
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::print;
  use sonify_health_lib::Patch;

  #[test]
  fn toml_export_import_round_trips_notes() {
    let patch = Patch::from_hostname("test");
    let notes = vec![
      NoteSpec {
        freq: 440.0,
        duration: 0.25,
      },
      NoteSpec {
        freq: 880.0,
        duration: 0.15,
      },
    ];
    let toml = print::format_toml(&patch, &[], &notes, &[]);
    let data = parse_import(&toml).expect("round-trip TOML should parse");
    assert_eq!(data.boops.len(), notes.len());
    for (orig, imp) in notes.iter().zip(data.boops.iter()) {
      assert!(
        (orig.freq - imp.freq).abs() < f64::EPSILON,
        "freq mismatch: {} vs {}",
        orig.freq,
        imp.freq
      );
      assert!(
        (orig.duration - imp.duration).abs() < f64::EPSILON,
        "duration mismatch: {} vs {}",
        orig.duration,
        imp.duration
      );
    }
  }

  #[test]
  fn json_export_import_round_trips_notes() {
    let patch = Patch::from_hostname("test");
    let notes = vec![
      NoteSpec {
        freq: 440.0,
        duration: 0.25,
      },
      NoteSpec {
        freq: 880.0,
        duration: 0.15,
      },
    ];
    let json = print::format_json(&patch, &[], &notes, &[]);
    let data = parse_import(&json).expect("round-trip JSON should parse");
    assert_eq!(data.boops.len(), notes.len());
    for (orig, imp) in notes.iter().zip(data.boops.iter()) {
      assert!(
        (orig.freq - imp.freq).abs() < f64::EPSILON,
        "freq mismatch: {} vs {}",
        orig.freq,
        imp.freq
      );
      assert!(
        (orig.duration - imp.duration).abs() < f64::EPSILON,
        "duration mismatch: {} vs {}",
        orig.duration,
        imp.duration
      );
    }
  }
}
