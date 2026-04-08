use crate::preview_state::{self, PreviewState};
use axum::{
  extract::{
    ws::{Message, WebSocket, WebSocketUpgrade},
    State,
  },
  response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use serde_json::json;
use sonify_health_lib::Severity;
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

/// Dispatch a single client message.  Returns `Some(reply)` for
/// messages that should go only to the requesting client.
/// Broadcast side-effects are fired inline.
fn handle_client_message(preview: &PreviewState, text: &str) -> Option<String> {
  let msg: serde_json::Value = serde_json::from_str(text).ok()?;
  let msg_type = msg.get("type").and_then(|v| v.as_str())?;

  match msg_type {
    "get_state" => Some(preview.state_snapshot()),

    "set_voice_param" => {
      let param = msg.get("param").and_then(|v| v.as_str())?;
      let value = msg.get("value").and_then(|v| v.as_f64())?;
      {
        let mut voice = preview.voice.write().unwrap();
        if !preview_state::set_voice_param(&mut voice, param, value) {
          return None;
        }
      }
      preview
        .drone_rebuild_requested
        .store(true, Ordering::Relaxed);
      let _ = preview.broadcast_tx.send(
        json!({
          "type": "param_changed",
          "param": param,
          "value": value,
        })
        .to_string(),
      );
      None
    }

    "set_muted" => {
      let muted = msg.get("muted").and_then(|v| v.as_bool())?;
      preview.muted.store(muted, Ordering::Relaxed);
      preview.update_all_combined_volumes();
      let _ = preview
        .broadcast_tx
        .send(json!({"type": "mute_changed", "muted": muted}).to_string());
      None
    }

    "set_heartbeat_volume" => {
      let vol = msg.get("volume").and_then(|v| v.as_f64())? as f32;
      preview.heartbeat_volume.set_value(vol.clamp(0.0, 1.0));
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

    "set_drone_texture" => {
      let index = msg.get("index").and_then(|v| v.as_u64())? as usize;
      let value = msg.get("texture").and_then(|v| v.as_str())?;
      let texture = preview_state::texture_from_str(value)?;
      {
        let mut infos = preview.drone_infos.write().unwrap();
        infos.get_mut(index)?.texture = texture;
      }
      preview
        .drone_rebuild_requested
        .store(true, Ordering::Relaxed);
      let register = {
        let infos = preview.drone_infos.read().unwrap();
        preview_state::register_str(infos[index].register)
      };
      let _ = preview.broadcast_tx.send(
        json!({
          "type": "drone_config_changed",
          "index": index,
          "texture": value,
          "register": register,
        })
        .to_string(),
      );
      None
    }

    "set_drone_register" => {
      let index = msg.get("index").and_then(|v| v.as_u64())? as usize;
      let value = msg.get("register").and_then(|v| v.as_str())?;
      let register = preview_state::register_from_str(value)?;
      {
        let mut infos = preview.drone_infos.write().unwrap();
        infos.get_mut(index)?.register = register;
      }
      preview
        .drone_rebuild_requested
        .store(true, Ordering::Relaxed);
      let texture = {
        let infos = preview.drone_infos.read().unwrap();
        preview_state::texture_str(infos[index].texture)
      };
      let _ = preview.broadcast_tx.send(
        json!({
          "type": "drone_config_changed",
          "index": index,
          "texture": texture,
          "register": value,
        })
        .to_string(),
      );
      None
    }

    "export_toml" => {
      let toml = preview.export_toml();
      Some(json!({"type": "toml_export", "content": toml}).to_string())
    }

    _ => None,
  }
}
