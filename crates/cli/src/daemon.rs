use crate::config::DaemonConfig;
use crate::metrics::Metrics;
use crate::preview_state::{severity_from_shared, PreviewState};
use serde_json::json;
use sonify_health_lib::{
  audio::{AudioError, AudioMixer},
  check, drone, heartbeat, Severity, Voice,
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
  pub audio_device: Option<&'a str>,
  pub muted: Arc<AtomicBool>,
  pub running: Arc<AtomicBool>,
  pub metrics: Metrics,
  pub preview: Arc<PreviewState>,
}

/// Run the daemon's main loop: spawn check threads, drone play
/// threads, play heartbeat boops at the configured time slot, and
/// respond to preview-UI actions.  Shuts down when `running` becomes
/// false.
pub fn run_daemon(ctx: DaemonContext<'_>) -> Result<(), DaemonError> {
  let DaemonContext {
    config,
    audio_device,
    muted,
    running,
    metrics,
    preview,
  } = ctx;
  {
    let voices = preview.voices.read().unwrap();
    let voice = &voices[&crate::preview_state::VoiceOwner::Heartbeat];
    debug!(?voice, "Resolved voice");
    log_voice_derivation(voice);
  }

  // Log initial boop specs from materialized state.
  {
    let boops_per_check = preview.boop_count.load(Ordering::Relaxed);
    let specs = preview.boop_specs.read().unwrap();
    for (i, spec) in specs.iter().enumerate() {
      let check_idx = if boops_per_check > 0 {
        i / boops_per_check
      } else {
        0
      };
      let check_name = config
        .heartbeat_checks
        .get(check_idx)
        .map(|c| c.name.as_str())
        .unwrap_or("?");
      debug!(
        boop = i,
        freq = format_args!("{:.1} Hz", spec.freq),
        duration = format_args!("{:.3}s", spec.duration),
        check = check_name,
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

  // -- Drone play threads -----------------------------------------------------
  // Each drone gets its own thread that loops: build a heartbeat
  // phrase from current voice + drone config, play it, then sleep
  // for a metric-driven gap before repeating.

  let drone_play_handles: Vec<_> = (0..config.drone_metrics.len())
    .map(|i| {
      let run = Arc::clone(&running);
      let prev = Arc::clone(&preview);
      let ds = Arc::clone(&drone_state);
      let mix = mixer.handle();
      thread::spawn(move || {
        let mut slot_id: Option<usize> = None;
        while run.load(Ordering::Relaxed) {
          if prev.drone_infos.read().unwrap().get(i).is_none() {
            break;
          }
          let metric = ds.metrics[i].value();

          // Interpolate between lo and hi profiles based on metric.
          let voice = prev.effective_drone_voice(i, metric);

          let specs = prev.drone_boop_specs.read().unwrap()[i].clone();
          let severities: Vec<Severity> =
            (0..specs.len()).map(|_| Severity::Healthy).collect();

          let graph = heartbeat::heartbeat_graph_with_volume(
            &voice,
            &severities,
            &specs,
            Some(&prev.combined_volumes[i]),
          );

          let attack_secs = voice.attack_ms / 1000.0;
          let release_secs = voice.release_ms / 1000.0;
          let phrase_dur = heartbeat::heartbeat_duration(
            &specs,
            attack_secs,
            release_secs,
            voice.echo_delay,
            voice.echo_mix,
          );

          // First iteration uses add; subsequent iterations use
          // replace for seamless graph swap.
          let sid = match slot_id {
            Some(id) => {
              mix.replace(id, graph);
              id
            }
            None => {
              let id = mix.add(graph);
              slot_id = Some(id);
              id
            }
          };
          sleep_checking(&run, phrase_dur);

          if !run.load(Ordering::Relaxed) {
            mix.remove(sid);
            break;
          }

          // Gap between phrases, shaped by repeat_curve and scaled
          // by repeat_rate.
          let base_gap = prev.drone_phrase_gaps[i].value() as f64;
          let curve = prev.drone_repeat_curves[i].value();
          let rate = prev.drone_repeat_rates[i].value();
          let gap = drone::phrase_gap_secs(base_gap, metric, curve, rate);
          if gap > 0.0 {
            mix.remove(sid);
            slot_id = None;
            sleep_checking(&run, Duration::from_secs_f64(gap));
          }
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
    // Handle mute transitions.
    let is_muted = muted.load(Ordering::Relaxed);
    if is_muted != was_muted {
      if is_muted {
        info!("Audio muted via API");
      } else {
        info!("Audio unmuted via API");
      }
      preview.update_all_combined_volumes();
      preview.update_effective_heartbeat_volume();
      was_muted = is_muted;
    }

    // Heartbeat trigger (one-shot, immediate).
    if preview.heartbeat_trigger.swap(false, Ordering::Relaxed)
      && !preview.heartbeat_state.boops.is_empty()
    {
      play_heartbeat_preview(&mixer, &preview)?;
      metrics.heartbeats_played.inc();
      thread::sleep(Duration::from_millis(50));
      continue;
    }

    // Heartbeat loop (continuous).
    if preview.heartbeat_loop.load(Ordering::Relaxed)
      && !preview.heartbeat_state.boops.is_empty()
    {
      play_heartbeat_preview(&mixer, &preview)?;
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
    if !is_muted && !preview.heartbeat_state.boops.is_empty() {
      play_heartbeat_preview(&mixer, &preview)?;
      metrics.heartbeats_played.inc();
    }

    // Sleep until the next slot.  Use the full cycle duration minus
    // a small margin so we land just before the next slot rather
    // than re-entering the current one.
    let remaining = Duration::from_secs_f64(
      config.timing.cycle_duration_secs - config.timing.slot_duration_secs,
    )
    .max(Duration::from_millis(500));
    let end = std::time::Instant::now() + remaining;
    while std::time::Instant::now() < end && running.load(Ordering::Relaxed) {
      if preview.heartbeat_trigger.load(Ordering::Relaxed)
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
  for h in drone_play_handles {
    let _ = h.join();
  }

  // Clear all mixer slots so audio stops before we log.
  mixer.clear();
  info!("Daemon stopped");
  Ok(())
}

// -- Heartbeat ---------------------------------------------------------------

fn play_heartbeat_preview(
  mixer: &AudioMixer,
  preview: &PreviewState,
) -> Result<(), DaemonError> {
  let specs = preview.boop_specs.read().unwrap().clone();
  let total = specs.len();
  let boops_per_check = preview.boop_count.load(Ordering::Relaxed);

  let voices = preview.voices.read().unwrap();
  let voice = &voices[&crate::preview_state::VoiceOwner::Heartbeat];

  // Each check's severity repeats for its phrase of boops.
  let severities: Vec<_> = (0..total)
    .map(|i| {
      let check_idx = if boops_per_check > 0 {
        i / boops_per_check
      } else {
        0
      };
      severity_from_shared(
        preview
          .heartbeat_state
          .boops
          .get(check_idx)
          .map(|b| b.value())
          .unwrap_or(0.0),
      )
    })
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
    Some(&preview.effective_heartbeat_volume),
  );
  let attack_secs = voice.attack_ms / 1000.0;
  let release_secs = voice.release_ms / 1000.0;
  let slot = mixer.add(graph);
  std::thread::sleep(heartbeat::heartbeat_duration(
    &specs,
    attack_secs,
    release_secs,
    voice.echo_delay,
    voice.echo_mix,
  ));
  mixer.remove(slot);
  Ok(())
}

// -- Helpers -----------------------------------------------------------------

/// Sleep for `dur` in ~100 ms increments, checking `running` flag.
fn sleep_checking(running: &AtomicBool, dur: Duration) {
  let deadline = std::time::Instant::now() + dur;
  while std::time::Instant::now() < deadline && running.load(Ordering::Relaxed)
  {
    let remaining =
      deadline.saturating_duration_since(std::time::Instant::now());
    thread::sleep(remaining.min(Duration::from_millis(100)));
  }
}

fn log_voice_derivation(voice: &Voice) {
  use sha2::{Digest, Sha256};

  let hostname = gethostname::gethostname().to_string_lossy().to_string();
  let host_hash = Sha256::digest(hostname.as_bytes());
  debug!(
    hostname = %hostname,
    hostname_sha256_prefix = %host_hash[..8]
      .iter()
      .map(|b| format!("{:02x}", b))
      .collect::<String>(),
    note_seed = voice.note_seed,
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
