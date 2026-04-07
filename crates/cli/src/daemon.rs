use crate::config::DaemonConfig;
use crate::metrics::Metrics;
use crate::preview_state::{severity_from_shared, PreviewState};
use serde_json::json;
use sonify_health_lib::{
  audio::{AudioError, AudioMixer},
  check, drone, heartbeat,
  state::DroneState,
  DroneTexture, PentatonicScale, Voice,
};
use std::sync::{
  atomic::{AtomicBool, Ordering},
  Arc,
};
use std::thread;
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, info, warn};

#[derive(Debug, Error)]
pub enum DaemonError {
  #[error("Audio playback failed: {0}")]
  Audio(#[from] AudioError),
}

/// Everything the daemon thread needs from main.
pub struct DaemonContext<'a> {
  pub config: &'a DaemonConfig,
  pub voice: &'a Voice,
  pub scale: &'a PentatonicScale,
  pub audio_device: Option<&'a str>,
  pub muted: Arc<AtomicBool>,
  pub running: Arc<AtomicBool>,
  pub metrics: Metrics,
  pub preview: Arc<PreviewState>,
}

/// Run the daemon's main loop: spawn check threads, build drone
/// audio streams, play heartbeat boops at the configured time slot,
/// and respond to preview-UI actions.  Shuts down when `running`
/// becomes false.
pub fn run_daemon(ctx: DaemonContext<'_>) -> Result<(), DaemonError> {
  let DaemonContext {
    config,
    voice,
    scale,
    audio_device,
    muted,
    running,
    metrics,
    preview,
  } = ctx;
  debug!(?voice, "Resolved voice");
  log_voice_derivation(voice);

  let boop_count = config.heartbeat_checks.len();

  // Log initial boop specs.
  {
    let specs = voice.boop_specs(scale, boop_count, heartbeat::TOTAL_BOOP_TIME);
    for (i, spec) in specs.iter().enumerate() {
      debug!(
        boop = i,
        freq = format_args!("{:.1} Hz", spec.freq),
        duration = format_args!("{:.3}s", spec.duration),
        check = config.heartbeat_checks[i].name,
        "Boop spec"
      );
    }
  }

  // Shared state lives inside PreviewState.
  let heartbeat_state = Arc::clone(&preview.heartbeat_state);
  let drone_state = Arc::clone(&preview.drone_state);

  // -- Heartbeat check threads ------------------------------------------------

  let check_handles: Vec<_> = config
    .heartbeat_checks
    .iter()
    .enumerate()
    .map(|(i, check_cfg)| {
      let cfg = check_cfg.clone();
      let st = Arc::clone(&heartbeat_state);
      let run = Arc::clone(&running);
      let m = metrics.clone();
      let prev = Arc::clone(&preview);
      let interval = Duration::from_secs_f64(config.timing.cycle_duration_secs);
      thread::spawn(move || {
        while run.load(Ordering::Relaxed) {
          // If an override is active, skip the shell check.
          let overridden = prev
            .heartbeat_overrides
            .read()
            .unwrap()
            .get(i)
            .copied()
            .flatten();

          if let Some(severity) = overridden {
            st.set(i, severity);
            send_check_log(
              &prev,
              "heartbeat",
              &cfg.name,
              &severity.to_string(),
              true,
            );
          } else {
            match check::run_heartbeat_check(&cfg) {
              Ok(severity) => {
                info!(
                  check = cfg.name,
                  severity = %severity,
                  "Heartbeat check completed"
                );
                st.set(i, severity);
                m.check_severity
                  .with_label_values(&[&cfg.name])
                  .set(severity as i64);
                m.check_runs
                  .with_label_values(&[&cfg.name, &severity.to_string()])
                  .inc();
                send_check_log(
                  &prev,
                  "heartbeat",
                  &cfg.name,
                  &severity.to_string(),
                  false,
                );
              }
              Err(e) => {
                warn!(
                  check = cfg.name,
                  error = %e,
                  "Heartbeat check failed, \
                   retaining previous severity"
                );
                m.check_runs.with_label_values(&[&cfg.name, "error"]).inc();
              }
            }
          }
          thread::sleep(interval);
        }
      })
    })
    .collect();

  // -- Single audio mixer stream -----------------------------------------------

  let mixer = AudioMixer::new(audio_device)?;

  let mut drone_slot_ids =
    build_drone_slots(&mixer, voice, config, scale, &drone_state, &preview);

  // -- Drone poll threads -----------------------------------------------------

  let drone_handles: Vec<_> = config
    .drone_metrics
    .iter()
    .enumerate()
    .map(|(i, drone_cfg)| {
      let cfg = drone_cfg.clone();
      let st = Arc::clone(&drone_state);
      let run = Arc::clone(&running);
      let m = metrics.clone();
      let prev = Arc::clone(&preview);
      let interval = Duration::from_secs_f64(config.drone_poll_interval_secs);
      thread::spawn(move || {
        while run.load(Ordering::Relaxed) {
          // If an override is active, skip the shell poll.
          let overridden = prev
            .drone_overrides
            .read()
            .unwrap()
            .get(i)
            .copied()
            .flatten();

          if let Some(value) = overridden {
            st.set(i, value);
            send_check_log(
              &prev,
              "drone",
              &cfg.name,
              &format!("{value:.3}"),
              true,
            );
          } else {
            match check::run_drone_poll(&cfg) {
              Ok(value) => {
                info!(metric = cfg.name, value, "Drone poll completed");
                st.set(i, value);
                m.drone_metric_value
                  .with_label_values(&[&cfg.name])
                  .set(value as f64);
                m.drone_polls.with_label_values(&[&cfg.name, "ok"]).inc();
                send_check_log(
                  &prev,
                  "drone",
                  &cfg.name,
                  &format!("{value:.3}"),
                  false,
                );
              }
              Err(e) => {
                warn!(
                  metric = cfg.name,
                  error = %e,
                  "Drone poll failed, retaining previous value"
                );
                m.drone_polls.with_label_values(&[&cfg.name, "error"]).inc();
              }
            }
          }
          thread::sleep(interval);
        }
      })
    })
    .collect();

  info!(
    slot = config.timing.slot,
    cycle_secs = config.timing.cycle_duration_secs,
    drone_metrics = config.drone_metrics.len(),
    "Daemon started"
  );

  let mut was_muted = muted.load(Ordering::Relaxed);

  // -- Main timing loop -------------------------------------------------------

  while running.load(Ordering::Relaxed) {
    // Handle pending drone rebuild.
    if preview
      .drone_rebuild_requested
      .swap(false, Ordering::Relaxed)
    {
      rebuild_drones(
        &mixer,
        &mut drone_slot_ids,
        &preview,
        config,
        scale,
        &drone_state,
      );
    }

    // Handle mute transitions.
    let is_muted = muted.load(Ordering::Relaxed);
    if is_muted != was_muted {
      if is_muted {
        info!("Audio muted via API");
      } else {
        info!("Audio unmuted via API");
      }
      preview.update_all_combined_volumes();
      was_muted = is_muted;
    }

    // Heartbeat trigger (one-shot, immediate).
    if preview.heartbeat_trigger.swap(false, Ordering::Relaxed)
      && boop_count > 0
    {
      play_heartbeat_preview(&mixer, &preview, scale, boop_count)?;
      metrics.heartbeats_played.inc();
      thread::sleep(Duration::from_millis(50));
      continue;
    }

    // Heartbeat loop (continuous).
    if preview.heartbeat_loop.load(Ordering::Relaxed) && boop_count > 0 {
      play_heartbeat_preview(&mixer, &preview, scale, boop_count)?;
      metrics.heartbeats_played.inc();
      // Brief pause before looping.
      thread::sleep(Duration::from_millis(50));
      continue;
    }

    // Wait for the configured time slot, checking flags every 100 ms.
    let wait = config.timing.duration_until_next_slot();
    if wait > Duration::ZERO {
      let deadline = std::time::Instant::now() + wait;
      while std::time::Instant::now() < deadline
        && running.load(Ordering::Relaxed)
      {
        if preview
          .drone_rebuild_requested
          .swap(false, Ordering::Relaxed)
        {
          rebuild_drones(
            &mixer,
            &mut drone_slot_ids,
            &preview,
            config,
            scale,
            &drone_state,
          );
        }
        if preview.heartbeat_trigger.load(Ordering::Relaxed)
          || preview.heartbeat_loop.load(Ordering::Relaxed)
        {
          break;
        }
        thread::sleep(Duration::from_millis(100));
      }
      if !running.load(Ordering::Relaxed) {
        break;
      }
      // If a flag interrupted the wait, handle it next iteration.
      if preview.heartbeat_trigger.load(Ordering::Relaxed)
        || preview.heartbeat_loop.load(Ordering::Relaxed)
      {
        continue;
      }
    }

    // Normal slot-based heartbeat.
    if !is_muted && boop_count > 0 {
      play_heartbeat_preview(&mixer, &preview, scale, boop_count)?;
      metrics.heartbeats_played.inc();
    }

    // Sleep through the rest of the slot.
    let remaining = config
      .timing
      .duration_until_next_slot()
      .max(Duration::from_millis(500));
    let end = std::time::Instant::now() + remaining;
    while std::time::Instant::now() < end && running.load(Ordering::Relaxed) {
      if preview.drone_rebuild_requested.load(Ordering::Relaxed)
        || preview.heartbeat_trigger.load(Ordering::Relaxed)
        || preview.heartbeat_loop.load(Ordering::Relaxed)
      {
        break;
      }
      thread::sleep(Duration::from_millis(100));
    }
  }

  info!("Waiting for check threads to finish");
  for h in check_handles {
    let _ = h.join();
  }
  for h in drone_handles {
    let _ = h.join();
  }

  // Clear all mixer slots so audio stops before we log.
  mixer.clear();
  info!("Daemon stopped");
  Ok(())
}

// -- Drone build / rebuild ---------------------------------------------------

fn build_drone_slots(
  mixer: &AudioMixer,
  voice: &Voice,
  config: &DaemonConfig,
  scale: &PentatonicScale,
  drone_state: &DroneState,
  preview: &PreviewState,
) -> Vec<usize> {
  config
    .drone_metrics
    .iter()
    .enumerate()
    .map(|(i, cfg)| {
      let texture = cfg.texture.unwrap_or_else(|| voice.drone_texture(i));
      let notes = if texture == DroneTexture::Arpeggio {
        voice.drone_notes(scale, 4)
      } else {
        vec![]
      };
      debug!(
        metric = cfg.name,
        ?texture,
        arpeggio_notes = notes.len(),
        "Drone texture resolved"
      );
      let graph = drone::drone_graph_with_volume(
        voice,
        cfg.register,
        texture,
        &drone_state.metrics[i],
        &notes,
        Some(&preview.combined_volumes[i]),
      );
      let slot = mixer.add(graph);
      info!(metric = cfg.name, slot, "Drone added to mixer");
      slot
    })
    .collect()
}

fn rebuild_drones(
  mixer: &AudioMixer,
  drone_slot_ids: &mut Vec<usize>,
  preview: &PreviewState,
  config: &DaemonConfig,
  scale: &PentatonicScale,
  drone_state: &DroneState,
) {
  for id in drone_slot_ids.drain(..) {
    mixer.remove(id);
  }
  let voice = preview.voice.read().unwrap();
  info!("Rebuilding drone audio streams");
  *drone_slot_ids =
    build_drone_slots(mixer, &voice, config, scale, drone_state, preview);
}

// -- Heartbeat ---------------------------------------------------------------

fn play_heartbeat_preview(
  mixer: &AudioMixer,
  preview: &PreviewState,
  scale: &PentatonicScale,
  boop_count: usize,
) -> Result<(), DaemonError> {
  let voice = preview.voice.read().unwrap();
  let specs = voice.boop_specs(scale, boop_count, heartbeat::TOTAL_BOOP_TIME);

  let severities: Vec<_> = preview
    .heartbeat_state
    .boops
    .iter()
    .map(|b| severity_from_shared(b.value()))
    .collect();

  info!(
    severities = ?severities
      .iter()
      .map(|s| s.to_string())
      .collect::<Vec<_>>(),
    "Playing heartbeat"
  );

  let graph = heartbeat::heartbeat_graph_with_volume(
    &voice,
    &severities,
    &specs,
    Some(&preview.heartbeat_volume),
  );
  let slot = mixer.add(graph);
  std::thread::sleep(heartbeat::heartbeat_duration(&specs));
  mixer.remove(slot);
  Ok(())
}

// -- Helpers -----------------------------------------------------------------

fn log_voice_derivation(voice: &Voice) {
  use sha2::{Digest, Sha256};
  use sonify_health_lib::scale;

  let hostname = gethostname::gethostname().to_string_lossy().to_string();
  let domain = scale::domain_from_hostname(&hostname);
  let host_hash = Sha256::digest(hostname.as_bytes());
  let domain_hash = Sha256::digest(domain.as_bytes());
  debug!(
    hostname = %hostname,
    hostname_sha256_prefix = %host_hash[..8]
      .iter()
      .map(|b| format!("{:02x}", b))
      .collect::<String>(),
    domain = %domain,
    domain_sha256_prefix = %domain_hash[..8]
      .iter()
      .map(|b| format!("{:02x}", b))
      .collect::<String>(),
    note_seed = voice.note_seed,
    base_texture_index = (voice.note_seed * 6.0).floor() as usize,
    "Voice seed derivation"
  );
}

fn send_check_log(
  preview: &PreviewState,
  layer: &str,
  name: &str,
  result: &str,
  overridden: bool,
) {
  let ts = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|d| d.as_secs_f64())
    .unwrap_or(0.0);
  let _ = preview.check_log_tx.send(
    json!({
      "type": "check_log",
      "timestamp": ts,
      "layer": layer,
      "name": name,
      "result": result,
      "overridden": overridden,
    })
    .to_string(),
  );
}
