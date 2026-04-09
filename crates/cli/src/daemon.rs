use crate::preview_state::{metric_label, PreviewState};
use serde_json::json;
use sonify_health_lib::{
  audio::{AudioError, AudioMixer},
  heartbeat, probe, seconds_until_next, Patch,
};
use std::sync::{
  atomic::{AtomicBool, Ordering},
  Arc,
};
use std::thread;
use std::time::Duration;
use thiserror::Error;
use tracing::{info, warn};

#[derive(Debug, Error)]
pub enum DaemonError {
  #[error("Audio playback failed: {0}")]
  Audio(#[from] AudioError),
}

/// Everything the daemon thread needs from main.
pub struct DaemonContext<'a> {
  pub audio_device: Option<&'a str>,
  pub muted: Arc<AtomicBool>,
  pub running: Arc<AtomicBool>,
  pub preview: Arc<PreviewState>,
}

/// Run the daemon's main loop: spawn per-heartbeat poll/play threads,
/// respond to preview-UI actions.  Shuts down when `running` becomes
/// false.
pub fn run_daemon(ctx: DaemonContext<'_>) -> Result<(), DaemonError> {
  let DaemonContext {
    audio_device,
    muted,
    running,
    preview,
  } = ctx;

  let mixer = AudioMixer::new(audio_device)?;

  let mut handles = Vec::new();

  // Snapshot heartbeat configs for thread setup; transitions are
  // re-read at runtime so live edits take effect.
  let hb_configs = preview.heartbeat_configs.read().unwrap().clone();
  for (i, cfg) in hb_configs.iter().enumerate() {
    // -- Poll thread: runs probe command, updates metric.
    let poll_cfg_name = cfg.name.clone();
    let poll_command = cfg.command.clone();
    let poll_mode = cfg.result_mode.clone();
    let poll_interval = Duration::from_secs_f64(cfg.poll_interval_secs);
    let poll_running = Arc::clone(&running);
    let poll_preview = Arc::clone(&preview);
    handles.push(thread::spawn(move || {
      while poll_running.load(Ordering::Relaxed) {
        let overridden = poll_preview.heartbeats[i]
          .override_value
          .read()
          .unwrap()
          .clone();

        if let Some(metric) = overridden {
          poll_preview.heartbeats[i]
            .metric
            .set_value(metric.clamp(0.0, 1.0));
          send_probe_log(
            &poll_preview,
            &poll_cfg_name,
            &format!("{metric:.3}"),
            true,
          );
        } else {
          match probe::run_probe(&poll_cfg_name, &poll_command, &poll_mode) {
            Ok(metric) => {
              let label = metric_label(metric);
              info!(
                heartbeat = poll_cfg_name,
                result = label,
                "Probe completed"
              );
              poll_preview.heartbeats[i].metric.set_value(metric);
              send_probe_log(&poll_preview, &poll_cfg_name, label, false);
            }
            Err(e) => {
              warn!(
                heartbeat = poll_cfg_name,
                error = %e,
                "Probe failed, retaining previous metric"
              );
            }
          }
        }

        // Broadcast metric change.
        let _ = poll_preview.broadcast_tx.send(
          json!({
            "type": "metric_changed",
            "index": i,
            "value": poll_preview.heartbeats[i].metric.value(),
          })
          .to_string(),
        );

        sleep_checking(&poll_running, poll_interval);
      }
    }));

    // -- Play thread.
    let play_running = Arc::clone(&running);
    let play_preview = Arc::clone(&preview);
    let play_mix = mixer.handle();
    let continuous = cfg.continuous;
    let phrase_gap = cfg.phrase_gap;
    let repeat_rate = cfg.repeat_rate;
    let cycle_secs = cfg.cycle_secs;
    let cycle_offset = cfg.cycle_offset_secs;
    handles.push(thread::spawn(move || {
      let mut slot_id: Option<usize> = None;
      while play_running.load(Ordering::Relaxed) {
        // Resolve transition → patch.  Clone the transition under a
        // brief read lock so live edits take effect immediately.
        let metric = play_preview.heartbeats[i].metric.value() as f64;
        let transition = play_preview.heartbeat_configs.read().unwrap()[i]
          .transition
          .clone();
        let lib = play_preview.library.read().unwrap();
        let patch = transition.resolve(metric, &lib);
        drop(lib);

        let patch = match patch {
          Some(p) => p,
          None => {
            thread::sleep(Duration::from_secs(1));
            continue;
          }
        };

        // Build single-note audio graph with effective volume.
        play_preview.update_effective_volume(i);
        let graph = heartbeat::heartbeat_graph_with_volume(
          &[patch.clone()],
          Some(&play_preview.heartbeats[i].effective_volume),
        );
        let dur = heartbeat::heartbeat_duration(&[patch]);

        if continuous {
          // Continuous: loop with phrase_gap / repeat_rate sleep.
          let sid = match slot_id {
            Some(id) => {
              play_mix.replace(id, graph);
              id
            }
            None => {
              let id = play_mix.add(graph);
              slot_id = Some(id);
              id
            }
          };

          let gap = phrase_gap / repeat_rate.max(0.1);
          let sleep_dur = if gap == 0.0 {
            heartbeat::heartbeat_content_duration(&[Patch {
              duration: dur.as_secs_f64(),
              ..Patch::default()
            }])
          } else {
            dur
          };

          sleep_checking(&play_running, sleep_dur);

          if !play_running.load(Ordering::Relaxed) {
            play_mix.remove(sid);
            break;
          }

          if gap > 0.0 {
            play_mix.remove(sid);
            slot_id = None;
            sleep_checking(&play_running, Duration::from_secs_f64(gap));

            // Align phrase restart to the wall-clock grid.
            let wait = seconds_until_next(cycle_secs, cycle_offset);
            if wait > 0.005 {
              sleep_checking(&play_running, Duration::from_secs_f64(wait));
            }
          }
        } else {
          // One-shot: play, then sleep for cycle_secs.
          let sid = play_mix.add(graph);
          sleep_checking(&play_running, dur);
          play_mix.remove(sid);

          if !play_running.load(Ordering::Relaxed) {
            break;
          }

          // Check for trigger/loop mode.
          if play_preview
            .heartbeat_trigger
            .swap(false, Ordering::Relaxed)
          {
            continue;
          }
          if play_preview.heartbeat_loop.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(50));
            continue;
          }

          let wait = seconds_until_next(cycle_secs, cycle_offset);
          sleep_checking(&play_running, Duration::from_secs_f64(wait));
        }
      }
    }));
  }

  info!(heartbeats = hb_configs.len(), "Daemon started");

  // Main loop: handle mute transitions and global triggers.
  let mut was_muted = muted.load(Ordering::Relaxed);
  while running.load(Ordering::Relaxed) {
    let is_muted = muted.load(Ordering::Relaxed);
    if is_muted != was_muted {
      if is_muted {
        info!("Audio muted via API");
      } else {
        info!("Audio unmuted via API");
      }
      preview.update_all_effective_volumes();
      was_muted = is_muted;
    }

    // Global heartbeat trigger: wake all one-shot heartbeats.
    if preview.heartbeat_trigger.load(Ordering::Relaxed) {
      // The per-heartbeat play threads will pick this up.
    }

    thread::sleep(Duration::from_millis(100));
  }

  info!("Waiting for heartbeat threads to finish");
  for h in handles {
    let _ = h.join();
  }
  mixer.clear();
  info!("Daemon stopped");
  Ok(())
}

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

fn send_probe_log(
  preview: &PreviewState,
  name: &str,
  result: &str,
  overridden: bool,
) {
  let ts = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|d| d.as_secs_f64())
    .unwrap_or(0.0);
  let _ = preview.probe_log_tx.send(
    json!({
      "type": "probe_log",
      "timestamp": ts,
      "name": name,
      "result": result,
      "overridden": overridden,
    })
    .to_string(),
  );
}
