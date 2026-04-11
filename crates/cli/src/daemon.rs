use crate::metrics::Metrics;
use crate::preview_state::{metric_label, PreviewState};
use serde_json::json;
use sonify_health_lib::{
  audio::{AudioError, AudioMixer, MixerHandle},
  continuous_graph, heartbeat, probe, seconds_until_next, ContinuousControls,
  Playback, ResolvedNote, StructuralParams,
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
  pub metrics: Metrics,
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
    metrics,
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
    handles.push(thread::spawn(move || {
      // Align to the wall-clock grid before the first play so that
      // heartbeats with different offsets start staggered.
      {
        let cfg = &play_preview.heartbeat_configs.read().unwrap()[i];
        let wait = seconds_until_next(cfg.cycle_secs, cfg.cycle_offset_secs);
        if wait > 0.005 {
          sleep_checking(&play_running, Duration::from_secs_f64(wait));
        }
      }

      while play_running.load(Ordering::Relaxed) {
        let mode = play_preview.heartbeat_configs.read().unwrap()[i].playback;
        match mode {
          Playback::Continuous => {
            play_continuous_tick(&play_running, &play_preview, &play_mix, i);
          }
          Playback::Loop => {
            play_loop(&play_running, &play_preview, &play_mix, i);
          }
          Playback::Clock => {
            play_oneshot_once(&play_running, &play_preview, &play_mix, i, true);
          }
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

/// Resolve the first note's patch from the current metric and
/// transition config.  Returns `None` if no notes resolve.
fn resolve_first_patch(
  preview: &PreviewState,
  i: usize,
) -> Option<(sonify_health_lib::Patch, f64)> {
  let metric = preview.heartbeats[i].metric.value() as f64;
  let note_configs = {
    let cfg = &preview.heartbeat_configs.read().unwrap()[i];
    cfg.notes.clone()
  };
  let lib = preview.library.read().unwrap();
  let nc = note_configs.first()?;
  let mut patch = nc.transition.resolve(metric, &lib)?;
  patch.amplitude *= nc.volume;
  Some((patch, metric))
}

/// Continuous morph playback: build the graph once with `Shared`
/// controls, then update those controls as the metric changes.
/// Only rebuilds the graph when structural parameters change.
/// Returns when the daemon is shutting down or the playback mode
/// changes away from `Continuous`.
fn play_continuous_tick(
  running: &AtomicBool,
  preview: &PreviewState,
  play_mix: &MixerHandle,
  i: usize,
) {
  // Wait for a valid initial patch.
  let (patch, _) = loop {
    if !running.load(Ordering::Relaxed) {
      return;
    }
    if preview.heartbeat_configs.read().unwrap()[i].playback
      != Playback::Continuous
    {
      return;
    }
    if let Some(result) = resolve_first_patch(preview, i) {
      break result;
    }
    thread::sleep(Duration::from_secs(1));
  };

  let crossfade_ms = {
    let cfg = &preview.heartbeat_configs.read().unwrap()[i];
    cfg.crossfade_ms
  };
  let smoothing = crossfade_ms / 1000.0;

  let controls = ContinuousControls::from_patch(&patch);
  let mut structural = StructuralParams::from_patch(&patch);
  preview.update_effective_volume(i);
  let graph = continuous_graph(
    &controls,
    smoothing,
    &structural,
    Some(&preview.heartbeats[i].effective_volume),
  );
  let sid = play_mix.add(graph);

  while running.load(Ordering::Relaxed) {
    sleep_checking(running, Duration::from_millis(50));

    if !running.load(Ordering::Relaxed) {
      break;
    }

    // Break out if the user switched away from continuous mode.
    if preview.heartbeat_configs.read().unwrap()[i].playback
      != Playback::Continuous
    {
      break;
    }

    // Re-read crossfade_ms in case the user changed it.
    let crossfade_ms = {
      let cfg = &preview.heartbeat_configs.read().unwrap()[i];
      cfg.crossfade_ms
    };

    if let Some((patch, _)) = resolve_first_patch(preview, i) {
      controls.update_from_patch(&patch);
      preview.update_effective_volume(i);

      // Rebuild graph only if structural params changed.
      let new_structural = StructuralParams::from_patch(&patch);
      if new_structural != structural {
        let smoothing = crossfade_ms / 1000.0;
        let graph = continuous_graph(
          &controls,
          smoothing,
          &new_structural,
          Some(&preview.heartbeats[i].effective_volume),
        );
        let cf =
          ((crossfade_ms / 1000.0) * play_mix.sample_rate()).ceil() as usize;
        play_mix.replace(sid, graph, cf);
        structural = new_structural;
      }
    }
  }

  play_mix.remove(sid);
}

/// Loop playback: keep a persistent mixer slot and crossfade each
/// iteration into the next via `replace()`.  Sleeps for the
/// content duration (excluding release/echo tail) so the crossfade
/// overlaps sustaining audio with the attack of the new graph.
/// Returns when the daemon is shutting down or the playback mode
/// changes away from `Loop`.
fn play_loop(
  running: &AtomicBool,
  preview: &PreviewState,
  play_mix: &MixerHandle,
  i: usize,
) {
  let metric = preview.heartbeats[i].metric.value() as f64;
  let note_configs = {
    let cfg = &preview.heartbeat_configs.read().unwrap()[i];
    cfg.notes.clone()
  };
  let lib = preview.library.read().unwrap();
  let notes: Vec<ResolvedNote> = note_configs
    .iter()
    .filter_map(|nc| {
      let patch = nc.transition.resolve(metric, &lib)?;
      Some(ResolvedNote {
        patch,
        volume: nc.volume,
        offset: nc.offset,
      })
    })
    .collect();
  drop(lib);

  if notes.is_empty() {
    thread::sleep(Duration::from_secs(1));
    return;
  }

  preview.update_effective_volume(i);
  let graph = heartbeat::heartbeat_graph_with_notes(
    &notes,
    Some(&preview.heartbeats[i].effective_volume),
  );
  let content_dur = heartbeat::heartbeat_notes_content_duration(&notes);
  let sid = play_mix.add(graph);

  sleep_checking(running, content_dur);

  while running.load(Ordering::Relaxed) {
    // Break out if the user switched away from loop mode.
    if preview.heartbeat_configs.read().unwrap()[i].playback != Playback::Loop {
      break;
    }

    // Check for manual trigger — break so the outer loop
    // re-enters and fires immediately.
    if preview.heartbeat_trigger.swap(false, Ordering::Relaxed) {
      break;
    }

    let metric = preview.heartbeats[i].metric.value() as f64;
    let (note_configs, crossfade_ms) = {
      let cfg = &preview.heartbeat_configs.read().unwrap()[i];
      (cfg.notes.clone(), cfg.crossfade_ms)
    };
    let lib = preview.library.read().unwrap();
    let notes: Vec<ResolvedNote> = note_configs
      .iter()
      .filter_map(|nc| {
        let patch = nc.transition.resolve(metric, &lib)?;
        Some(ResolvedNote {
          patch,
          volume: nc.volume,
          offset: nc.offset,
        })
      })
      .collect();
    drop(lib);

    if notes.is_empty() {
      break;
    }

    preview.update_effective_volume(i);
    let graph = heartbeat::heartbeat_graph_with_notes(
      &notes,
      Some(&preview.heartbeats[i].effective_volume),
    );
    let content_dur = heartbeat::heartbeat_notes_content_duration(&notes);
    let cf = ((crossfade_ms / 1000.0) * play_mix.sample_rate()).ceil() as usize;
    play_mix.replace(sid, graph, cf);

    sleep_checking(running, content_dur);
  }

  play_mix.remove(sid);
}

/// One-shot playback: build and play a single heartbeat, then
/// either wait for the wall-clock grid (`wait_for_clock = true`)
/// or sleep briefly before looping (`wait_for_clock = false`).
fn play_oneshot_once(
  running: &AtomicBool,
  preview: &PreviewState,
  play_mix: &MixerHandle,
  i: usize,
  wait_for_clock: bool,
) {
  let metric = preview.heartbeats[i].metric.value() as f64;
  let (note_configs, cycle_secs, cycle_offset) = {
    let cfg = &preview.heartbeat_configs.read().unwrap()[i];
    (cfg.notes.clone(), cfg.cycle_secs, cfg.cycle_offset_secs)
  };
  let lib = preview.library.read().unwrap();
  let notes: Vec<ResolvedNote> = note_configs
    .iter()
    .filter_map(|nc| {
      let patch = nc.transition.resolve(metric, &lib)?;
      Some(ResolvedNote {
        patch,
        volume: nc.volume,
        offset: nc.offset,
      })
    })
    .collect();
  drop(lib);

  if notes.is_empty() {
    thread::sleep(Duration::from_secs(1));
    return;
  }

  preview.update_effective_volume(i);
  let graph = heartbeat::heartbeat_graph_with_notes(
    &notes,
    Some(&preview.heartbeats[i].effective_volume),
  );
  let dur = heartbeat::heartbeat_notes_duration(&notes);

  let sid = play_mix.add(graph);
  sleep_checking(running, dur);
  play_mix.remove(sid);

  if !running.load(Ordering::Relaxed) {
    return;
  }

  if preview.heartbeat_trigger.swap(false, Ordering::Relaxed) {
    return;
  }

  if wait_for_clock {
    let wait = seconds_until_next(cycle_secs, cycle_offset);
    sleep_checking(running, Duration::from_secs_f64(wait));
  } else {
    thread::sleep(Duration::from_millis(50));
  }
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
