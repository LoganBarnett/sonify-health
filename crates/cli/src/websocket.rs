use crate::preview_state::{self, PreviewState, VoiceOwner};
use axum::{
  extract::{
    ws::{Message, WebSocket, WebSocketUpgrade},
    State,
  },
  response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use serde_json::json;
use sonify_health_lib::{BoopSpec, Severity};
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
fn broadcast_locked_params(preview: &PreviewState, owner: &VoiceOwner) {
  let locked = preview.locked_params.read().unwrap();
  let params: Vec<_> = locked
    .get(owner)
    .map(|s| s.iter().collect())
    .unwrap_or_default();
  let mut msg = json!({
    "type": "locked_params_changed",
    "layer": match owner {
      VoiceOwner::Heartbeat => "heartbeat",
      VoiceOwner::Drone(_) => "drone",
    },
    "params": params,
  });
  if let VoiceOwner::Drone(i) = owner {
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

/// Parse a `VoiceOwner` from the `layer` and optional `index`
/// fields of a WebSocket message.
fn parse_voice_owner(msg: &serde_json::Value) -> Option<VoiceOwner> {
  let layer = msg.get("layer").and_then(|v| v.as_str())?;
  let index = msg
    .get("index")
    .and_then(|v| v.as_u64())
    .map(|v| v as usize);
  VoiceOwner::from_layer_index(layer, index)
}

/// Dispatch a single client message.  Returns `Some(reply)` for
/// messages that should go only to the requesting client.
/// Broadcast side-effects are fired inline.
fn handle_client_message(preview: &PreviewState, text: &str) -> Option<String> {
  let msg: serde_json::Value = serde_json::from_str(text).ok()?;
  let msg_type = msg.get("type").and_then(|v| v.as_str())?;

  match msg_type {
    "get_state" => Some(preview.state_snapshot()),

    "set_voice_param" => {
      let owner = parse_voice_owner(&msg)?;
      let param = msg.get("param").and_then(|v| v.as_str())?;
      let value = msg.get("value").and_then(|v| v.as_f64())?;
      {
        let mut voices = preview.voices.write().unwrap();
        let voice = voices.get_mut(&owner)?;
        if !preview_state::set_voice_param(voice, param, value) {
          return None;
        }
      }
      let mut broadcast = json!({
        "type": "param_changed",
        "layer": match &owner {
          VoiceOwner::Heartbeat => "heartbeat",
          VoiceOwner::Drone(_) => "drone",
        },
        "param": param,
        "value": value,
      });
      if let VoiceOwner::Drone(i) = &owner {
        broadcast["index"] = json!(i);
      }
      let _ = preview.broadcast_tx.send(broadcast.to_string());
      if owner == VoiceOwner::Heartbeat
        && matches!(param, "note_seed" | "base_freq")
      {
        preview.recompute_boop_specs();
        broadcast_boop_specs(preview);
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

    "set_drone_volume" => {
      let index = msg.get("index").and_then(|v| v.as_u64())? as usize;
      let vol = msg.get("volume").and_then(|v| v.as_f64())? as f32;
      let dv = preview.drone_volumes.get(index)?;
      dv.set_value(vol.clamp(0.0, 1.0));
      preview.update_combined_volume(index);
      let _ = preview.broadcast_tx.send(
        json!({
          "type": "volume_changed",
          "layer": "drone",
          "index": index,
          "volume": vol,
        })
        .to_string(),
      );
      None
    }

    "set_drone_repeat_rate" => {
      let index = msg.get("index").and_then(|v| v.as_u64())? as usize;
      let rate = msg.get("rate").and_then(|v| v.as_f64())? as f32;
      let clamped = rate.clamp(0.1, 10.0);
      preview.drone_repeat_rates.get(index)?.set_value(clamped);
      let _ = preview.broadcast_tx.send(
        json!({
          "type": "drone_repeat_rate_changed",
          "index": index,
          "rate": clamped,
        })
        .to_string(),
      );
      None
    }

    "set_drone_repeat_curve" => {
      let index = msg.get("index").and_then(|v| v.as_u64())? as usize;
      let curve = msg.get("curve").and_then(|v| v.as_f64())? as f32;
      let clamped = curve.clamp(0.1, 5.0);
      preview.drone_repeat_curves.get(index)?.set_value(clamped);
      let _ = preview.broadcast_tx.send(
        json!({
          "type": "drone_repeat_curve_changed",
          "index": index,
          "curve": clamped,
        })
        .to_string(),
      );
      None
    }

    "set_drone_phrase_gap" => {
      let index = msg.get("index").and_then(|v| v.as_u64())? as usize;
      let gap = msg.get("gap").and_then(|v| v.as_f64())? as f32;
      let clamped = gap.clamp(0.0, 16.0);
      preview.drone_phrase_gaps.get(index)?.set_value(clamped);
      let _ = preview.broadcast_tx.send(
        json!({
          "type": "drone_phrase_gap_changed",
          "index": index,
          "gap": clamped,
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
          let val_str = msg.get("value").and_then(|v| v.as_str())?;
          let severity: Severity = val_str.parse().ok()?;
          {
            let mut ov = preview.heartbeat_overrides.write().unwrap();
            *ov.get_mut(index)? = Some(severity);
          }
          preview.heartbeat_state.set(index, severity);
          let _ = preview.broadcast_tx.send(
            json!({
              "type": "override_changed",
              "layer": "heartbeat",
              "index": index,
              "value": severity.to_string(),
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
        let mut infos = preview.drone_infos.write().unwrap();
        infos.get_mut(index)?.base_freq = Some(freq);
      }
      let info = preview.drone_infos.read().unwrap()[index].clone();
      let _ = preview.broadcast_tx.send(
        json!({
          "type": "drone_config_changed",
          "index": index,
          "base_freq": info.base_freq,
          "boops": info.boops,
        })
        .to_string(),
      );
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
          "base_freq": info.base_freq,
          "boops": info.boops,
        })
        .to_string(),
      );
      None
    }

    "lock_param" => {
      let owner = parse_voice_owner(&msg)?;
      let param = msg.get("param").and_then(|v| v.as_str())?;
      if !preview_state::VOICE_PARAMS.iter().any(|p| p.name == param) {
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
      let owner = parse_voice_owner(&msg)?;
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
      let owners: Vec<VoiceOwner> = preview
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
      if index >= preview.drone_volumes.len() {
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

    "export_toml" => {
      let toml = preview.export_toml();
      let json_str = preview.export_json();
      let nix = preview.export_nix();
      Some(
        json!({
          "type": "voice_export",
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

/// Intermediate structure for deserializing voice params from import.
#[derive(Default, serde::Deserialize)]
struct ImportVoice {
  base_freq: Option<f64>,
  sine_ratio: Option<f64>,
  tri_ratio: Option<f64>,
  saw_ratio: Option<f64>,
  attack_ms: Option<f64>,
  release_ms: Option<f64>,
  chirp_ratio: Option<f64>,
  stereo_pan: Option<f64>,
  reverb_mix: Option<f64>,
  note_seed: Option<f64>,
  echo_delay: Option<f64>,
  echo_mix: Option<f64>,
  brightness: Option<f64>,
  resonance: Option<f64>,
  sub_octave: Option<f64>,
  note_spread: Option<f64>,
  vibrato_rate: Option<f64>,
  vibrato_depth: Option<f64>,
  tremolo_rate: Option<f64>,
  tremolo_depth: Option<f64>,
  amplitude: Option<f64>,
  square_ratio: Option<f64>,
}

#[derive(serde::Deserialize)]
struct ImportBoop {
  freq: f64,
  duration: f64,
}

#[derive(Default, serde::Deserialize)]
struct ImportHeartbeatSection {
  voice: Option<ImportVoice>,
  notes: Option<Vec<ImportBoop>>,
}

#[derive(serde::Deserialize)]
struct ImportToml {
  voice: Option<ImportVoice>,
  heartbeat: Option<ImportHeartbeatSection>,
  drone_voices: Option<std::collections::HashMap<String, ImportVoice>>,
}

#[derive(serde::Deserialize)]
struct ImportJson {
  voice: Option<ImportVoice>,
  heartbeat: Option<ImportHeartbeatSection>,
  drone_voices: Option<std::collections::HashMap<String, ImportVoice>>,
}

/// Per-entity voice overrides plus heartbeat boop specs.
struct ImportData {
  heartbeat_params: Vec<(&'static str, f64)>,
  drone_params: std::collections::HashMap<String, Vec<(&'static str, f64)>>,
  boops: Vec<BoopSpec>,
}

type ImportResult = Result<ImportData, String>;

/// Auto-detect format and parse into per-entity voice param overrides
/// + boop specs.
fn parse_import(text: &str) -> ImportResult {
  let trimmed = text.trim();
  if trimmed.starts_with('{') {
    parse_import_json(trimmed)
  } else {
    parse_import_toml(trimmed)
  }
}

fn voice_fields(v: &ImportVoice) -> Vec<(&'static str, f64)> {
  let mut out = Vec::new();
  if let Some(x) = v.base_freq {
    out.push(("base_freq", x));
  }
  if let Some(x) = v.sine_ratio {
    out.push(("sine_ratio", x));
  }
  if let Some(x) = v.tri_ratio {
    out.push(("tri_ratio", x));
  }
  if let Some(x) = v.saw_ratio {
    out.push(("saw_ratio", x));
  }
  if let Some(x) = v.attack_ms {
    out.push(("attack_ms", x));
  }
  if let Some(x) = v.release_ms {
    out.push(("release_ms", x));
  }
  if let Some(x) = v.chirp_ratio {
    out.push(("chirp_ratio", x));
  }
  if let Some(x) = v.stereo_pan {
    out.push(("stereo_pan", x));
  }
  if let Some(x) = v.reverb_mix {
    out.push(("reverb_mix", x));
  }
  if let Some(x) = v.note_seed {
    out.push(("note_seed", x));
  }
  if let Some(x) = v.echo_delay {
    out.push(("echo_delay", x));
  }
  if let Some(x) = v.echo_mix {
    out.push(("echo_mix", x));
  }
  if let Some(x) = v.brightness {
    out.push(("brightness", x));
  }
  if let Some(x) = v.resonance {
    out.push(("resonance", x));
  }
  if let Some(x) = v.sub_octave {
    out.push(("sub_octave", x));
  }
  if let Some(x) = v.note_spread {
    out.push(("note_spread", x));
  }
  if let Some(x) = v.vibrato_rate {
    out.push(("vibrato_rate", x));
  }
  if let Some(x) = v.vibrato_depth {
    out.push(("vibrato_depth", x));
  }
  if let Some(x) = v.tremolo_rate {
    out.push(("tremolo_rate", x));
  }
  if let Some(x) = v.tremolo_depth {
    out.push(("tremolo_depth", x));
  }
  if let Some(x) = v.amplitude {
    out.push(("amplitude", x));
  }
  if let Some(x) = v.square_ratio {
    out.push(("square_ratio", x));
  }
  out
}

fn boops_from_import(raw: &[ImportBoop]) -> Vec<BoopSpec> {
  raw
    .iter()
    .map(|b| BoopSpec {
      freq: b.freq,
      duration: b.duration,
    })
    .collect()
}

fn build_import_data(
  legacy_voice: Option<&ImportVoice>,
  heartbeat: Option<ImportHeartbeatSection>,
  drone_voices: Option<std::collections::HashMap<String, ImportVoice>>,
) -> ImportData {
  // Heartbeat voice: prefer heartbeat.voice, fall back to bare [voice]
  // for backwards compatibility.
  let heartbeat_params = heartbeat
    .as_ref()
    .and_then(|hb| hb.voice.as_ref())
    .or(legacy_voice)
    .map(voice_fields)
    .unwrap_or_default();

  let boops = heartbeat
    .and_then(|hb| hb.notes)
    .as_deref()
    .map(boops_from_import)
    .unwrap_or_default();

  let drone_params = drone_voices
    .unwrap_or_default()
    .iter()
    .map(|(name, v)| (name.clone(), voice_fields(v)))
    .collect();

  ImportData {
    heartbeat_params,
    drone_params,
    boops,
  }
}

fn parse_import_json(text: &str) -> ImportResult {
  let parsed: ImportJson =
    serde_json::from_str(text).map_err(|e| format!("JSON parse error: {e}"))?;
  Ok(build_import_data(
    parsed.voice.as_ref(),
    parsed.heartbeat,
    parsed.drone_voices,
  ))
}

fn parse_import_toml(text: &str) -> ImportResult {
  let parsed: ImportToml =
    toml::from_str(text).map_err(|e| format!("TOML parse error: {e}"))?;
  Ok(build_import_data(
    parsed.voice.as_ref(),
    parsed.heartbeat,
    parsed.drone_voices,
  ))
}

/// Apply imported per-entity voice params and boop specs to preview
/// state.  Per-entity locked params are respected.  Imported boops
/// are pinned.
fn apply_import(preview: &PreviewState, data: &ImportData) {
  let locked = preview.locked_params.read().unwrap().clone();

  let is_locked = |owner: &VoiceOwner, name: &str| -> bool {
    locked.get(owner).map(|s| s.contains(name)).unwrap_or(false)
  };

  {
    let mut voices = preview.voices.write().unwrap();

    // Heartbeat voice params.
    if let Some(voice) = voices.get_mut(&VoiceOwner::Heartbeat) {
      for &(name, value) in &data.heartbeat_params {
        if !is_locked(&VoiceOwner::Heartbeat, name) {
          preview_state::set_voice_param(voice, name, value);
        }
      }
    }

    // Drone voice params (matched by name).
    let drone_infos = preview.drone_infos.read().unwrap();
    for (i, info) in drone_infos.iter().enumerate() {
      if let Some(params) = data.drone_params.get(&info.name) {
        let owner = VoiceOwner::Drone(i);
        if let Some(voice) = voices.get_mut(&owner) {
          for &(name, value) in params {
            if !is_locked(&owner, name) {
              preview_state::set_voice_param(voice, name, value);
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

  preview.recompute_boop_specs();

  let snapshot = preview.state_snapshot();
  let _ = preview.broadcast_tx.send(snapshot);
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::print;
  use sonify_health_lib::Voice;

  #[test]
  fn toml_export_import_round_trips_notes() {
    let voice = Voice::from_hostname("test");
    let notes = vec![
      BoopSpec {
        freq: 440.0,
        duration: 0.25,
      },
      BoopSpec {
        freq: 880.0,
        duration: 0.15,
      },
    ];
    let toml = print::format_toml(&voice, &[], "C", &notes);
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
    let voice = Voice::from_hostname("test");
    let notes = vec![
      BoopSpec {
        freq: 440.0,
        duration: 0.25,
      },
      BoopSpec {
        freq: 880.0,
        duration: 0.15,
      },
    ];
    let json = print::format_json(&voice, &[], "C", &notes);
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
