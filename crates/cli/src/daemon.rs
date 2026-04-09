use crate::config::DaemonConfig;
use crate::metrics::Metrics;
use crate::preview_state::{metric_label, PreviewState};
use serde_json::json;
use sonify_health_lib::{
  audio::{AudioError, AudioMixer},
  check::{self, ResultMode},
  heartbeat, Patch,
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
    let patches = preview.patches.read().unwrap();
    let patch = &patches[&crate::preview_state::PatchOwner::Heartbeat];
    debug!(?patch, "Resolved patch");
    log_patch_derivation(patch);
  }

  // Split checks by result mode.
  let heartbeat_checks: Vec<_> = config
    .checks
    .iter()
    .filter(|c| c.result_mode == ResultMode::ExitCodeSeverity)
    .cloned()
    .collect();
  let drone_checks: Vec<_> = config
    .checks
    .iter()
    .filter(|c| c.result_mode != ResultMode::ExitCodeSeverity)
    .cloned()
    .collect();

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
      let check_name = heartbeat_checks
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

  let check_handles: Vec<_> = heartbeat_checks
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

          if let Some(metric) = overridden {
            st.set(i, metric);
            let label = metric_label(metric);
            send_check_log(&prev, "heartbeat", &cfg.name, label, true);
          } else {
            match check::run_check(&cfg) {
              Ok(metric) => {
                let label = metric_label(metric);
                info!(
                  check = cfg.name,
                  severity = label,
                  "Heartbeat check completed"
                );
                st.set(i, metric);
                m.check_severity
                  .with_label_values(&[&cfg.name])
                  .set((metric * 2.0).round() as i64);
                m.check_runs.with_label_values(&[&cfg.name, label]).inc();
                send_check_log(&prev, "heartbeat", &cfg.name, label, false);
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

  let drone_handles: Vec<_> = drone_checks
    .iter()
    .enumerate()
    .map(|(i, drone_cfg)| {
      let cfg = drone_cfg.clone();
      let st = Arc::clone(&drone_state);
      let run = Arc::clone(&running);
      let m = metrics.clone();
      let prev = Arc::clone(&preview);
      let interval = Duration::from_secs_f64(config.poll_interval_secs);
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
            match check::run_check(&cfg) {
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

  let drone_play_handles: Vec<_> = (0..drone_checks.len())
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
          let base_patch = prev.effective_drone_patch(i, metric);

          // Update the combined volume from the interpolated patch.
          prev.update_combined_volume_with(i, base_patch.volume as f32);

          let note_specs = prev.drone_boop_specs.read().unwrap()[i].clone();
          let patches: Vec<Patch> = note_specs
            .iter()
            .map(|s| base_patch.clone().with_note(s.freq, s.duration))
            .collect();
          let graph = heartbeat::heartbeat_graph_with_volume(
            &patches,
            Some(&prev.combined_volumes[i]),
          );

          // Compute gap from the interpolated patch fields.
          let gap = base_patch.phrase_gap / (base_patch.repeat_rate.max(0.1));

          // When gap=0 the drone loops seamlessly: sleep only for
          // the note content so replace() fires while the last
          // note is still sustaining, letting the crossfade
          // overlap sound with sound instead of silence.
          let phrase_dur = if gap == 0.0 {
            heartbeat::heartbeat_content_duration(&patches)
          } else {
            heartbeat::heartbeat_duration(&patches)
          };

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
    drone_checks = drone_checks.len(),
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
      && !preview.heartbeat_state.metrics.is_empty()
    {
      play_heartbeat_preview(&mixer, &preview)?;
      metrics.heartbeats_played.inc();
      thread::sleep(Duration::from_millis(50));
      continue;
    }

    // Heartbeat loop (continuous).
    if preview.heartbeat_loop.load(Ordering::Relaxed)
      && !preview.heartbeat_state.metrics.is_empty()
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
    if !is_muted && !preview.heartbeat_state.metrics.is_empty() {
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
  let note_specs = preview.boop_specs.read().unwrap().clone();
  let total = note_specs.len();
  let boops_per_check = preview.boop_count.load(Ordering::Relaxed);

  let all_patches = preview.patches.read().unwrap();
  let base_patch = &all_patches[&crate::preview_state::PatchOwner::Heartbeat];

  let patches: Vec<Patch> = note_specs
    .iter()
    .map(|s| base_patch.clone().with_note(s.freq, s.duration))
    .collect();

  let labels: Vec<_> = (0..total)
    .map(|i| {
      let check_idx = if boops_per_check > 0 {
        i / boops_per_check
      } else {
        0
      };
      metric_label(
        preview
          .heartbeat_state
          .metrics
          .get(check_idx)
          .map(|b| b.value())
          .unwrap_or(0.0),
      )
    })
    .collect();

  info!(severities = ?labels, "Playing heartbeat");

  let graph = heartbeat::heartbeat_graph_with_volume(
    &patches,
    Some(&preview.effective_heartbeat_volume),
  );
  let slot = mixer.add(graph);
  std::thread::sleep(heartbeat::heartbeat_duration(&patches));
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

fn log_patch_derivation(patch: &Patch) {
  use sha2::{Digest, Sha256};

  let hostname = gethostname::gethostname().to_string_lossy().to_string();
  let host_hash = Sha256::digest(hostname.as_bytes());
  debug!(
    hostname = %hostname,
    hostname_sha256_prefix = %host_hash[..8]
      .iter()
      .map(|b| format!("{:02x}", b))
      .collect::<String>(),
    note_seed = patch.note_seed,
    "Patch seed derivation"
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
