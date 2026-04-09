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
use sonify_health_lib::{Patch, Transition};
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
      broadcast_library(preview);
      None
    }

    "set_heartbeat_volume" => {
      let index = msg.get("index").and_then(|v| v.as_u64())? as usize;
      let vol = msg.get("volume").and_then(|v| v.as_f64())? as f32;
      let clamped = vol.clamp(0.0, 1.0);
      preview.heartbeats.get(index)?.volume.set_value(clamped);
      preview.update_effective_volume(index);
      let _ = preview.broadcast_tx.send(
        json!({
          "type": "volume_changed",
          "layer": "heartbeat",
          "index": index,
          "volume": clamped,
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
      preview.heartbeat_trigger.store(true, Ordering::Relaxed);
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

    "set_cycle_offset" => {
      let index = msg.get("index").and_then(|v| v.as_u64())? as usize;
      let value = msg.get("value").and_then(|v| v.as_f64())?;
      let clamped = value.max(0.0);
      {
        let mut configs = preview.heartbeat_configs.write().unwrap();
        configs.get_mut(index)?.cycle_offset_secs = clamped;
      }
      let _ = preview.broadcast_tx.send(
        json!({
          "type": "cycle_offset_changed",
          "index": index,
          "value": clamped,
        })
        .to_string(),
      );
      None
    }

    "set_transition" => {
      let index = msg.get("index").and_then(|v| v.as_u64())? as usize;
      let raw = msg.get("transition")?;
      let transition: Transition = serde_json::from_value(raw.clone()).ok()?;
      {
        let mut configs = preview.heartbeat_configs.write().unwrap();
        configs.get_mut(index)?.transition = transition.clone();
      }
      let _ = preview.broadcast_tx.send(
        json!({
          "type": "transition_changed",
          "index": index,
          "transition": serde_json::to_value(&transition).unwrap_or_default(),
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
      let lib_json = serde_json::to_value(&*lib).unwrap_or_default();
      Some(
        json!({
          "type": "config_export",
          "library": lib_json,
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

    _ => None,
  }
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
