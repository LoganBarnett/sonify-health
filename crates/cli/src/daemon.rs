use crate::preview_state::{metric_label, PreviewState};
use serde_json::json;
use sonify_health_lib::{
  audio::{AudioError, AudioMixer, MixerHandle},
  continuous_graph_with_notes, heartbeat, probe, seconds_until_next, Patch,
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
  pub preview: Arc<PreviewState>,
}

/// Run the daemon's main loop: spawn per-heartbeat poll/play threads,
/// respond to preview-UI actions.  Shuts down when `running` becomes
/// false.
pub fn run_daemon(ctx: DaemonContext<'_>) -> Result<(), DaemonError> {
  let DaemonContext {
    audio_device,
    preview,
  } = ctx;

  let mixer = AudioMixer::new(audio_device)?;
  preview.set_mixer_handle(mixer.handle());

  let mut handles = Vec::new();

  // Snapshot heartbeat configs for thread setup; transitions are
  // re-read at runtime so live edits take effect.
  let hb_count = preview.heartbeat_configs.read().unwrap().len();
  for i in 0..hb_count {
    let (poll_h, play_h) = spawn_heartbeat_threads(&preview, i);
    handles.push(poll_h);
    handles.push(play_h);
  }

  info!(heartbeats = hb_count, "Daemon started");

  // Main loop: handle mute transitions.
  let mut was_muted = preview.muted.load(Ordering::Relaxed);
  while preview.running.load(Ordering::Relaxed) {
    let is_muted = preview.muted.load(Ordering::Relaxed);
    if is_muted != was_muted {
      if is_muted {
        info!("Audio muted via API");
      } else {
        info!("Audio unmuted via API");
      }
      preview.update_all_effective_volumes();
      was_muted = is_muted;
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

/// Spawn poll and play threads for heartbeat at `index`.  Reads
/// the config snapshot and prometheus labels at spawn time.
pub fn spawn_heartbeat_threads(
  preview: &Arc<PreviewState>,
  i: usize,
) -> (thread::JoinHandle<()>, thread::JoinHandle<()>) {
  let cfg = preview.heartbeat_configs.read().unwrap()[i].clone();

  // -- Poll thread: runs probe command, updates metric.
  let poll_running = Arc::clone(&preview.running);
  let poll_preview = Arc::clone(preview);
  let poll_counter = preview
    .metrics
    .probes_completed
    .with_label_values(&[&cfg.name]);
  let poll_gauge = preview.metrics.probe_value.with_label_values(&[&cfg.name]);

  let poll_handle = thread::spawn(move || {
    while poll_running.load(Ordering::Relaxed) {
      // Re-read config each iteration so live UI edits take effect.
      let (cfg_name, command, mode, tiers, interval) = {
        let configs = poll_preview.heartbeat_configs.read().unwrap();
        let cfg = &configs[i];
        (
          cfg.name.clone(),
          cfg.command.clone(),
          cfg.result_mode.clone(),
          cfg.tiers.clone(),
          Duration::from_secs_f64(cfg.poll_interval_secs),
        )
      };

      let overridden = {
        let hbs = poll_preview.heartbeats.read().unwrap();
        let val = hbs[i].override_value.read().unwrap().clone();
        val
      };

      let resolved = if let Some(metric) = overridden {
        let clamped = metric.clamp(0.0, 1.0);
        {
          let hbs = poll_preview.heartbeats.read().unwrap();
          hbs[i].metric.set_value(clamped);
        }
        send_probe_log(
          &poll_preview,
          &cfg_name,
          &metric_label(clamped, &tiers),
          true,
        );
        poll_counter.inc();
        poll_gauge.set(clamped as f64);
        Some(clamped)
      } else {
        match probe::run_probe(&cfg_name, &command, &mode) {
          Ok(metric) => {
            let label = metric_label(metric, &tiers);
            info!(heartbeat = cfg_name, result = label, "Probe completed");
            {
              let hbs = poll_preview.heartbeats.read().unwrap();
              hbs[i].metric.set_value(metric);
            }
            send_probe_log(&poll_preview, &cfg_name, &label, false);
            poll_counter.inc();
            poll_gauge.set(metric as f64);
            Some(metric)
          }
          Err(e) => {
            warn!(
              heartbeat = cfg_name,
              error = %e,
              "Probe failed, retaining previous metric"
            );
            None
          }
        }
      };

      if let Some(val) = resolved {
        let label = metric_label(val, &tiers);
        info!(
          heartbeat = cfg_name,
          result = label,
          overridden = overridden.is_some(),
          "Metric resolved"
        );
      }

      // Broadcast metric change.
      let metric_val = {
        let hbs = poll_preview.heartbeats.read().unwrap();
        hbs[i].metric.value()
      };
      let _ = poll_preview.broadcast_tx.send(
        json!({
          "type": "metric_changed",
          "index": i,
          "value": metric_val,
        })
        .to_string(),
      );

      sleep_checking(&poll_running, interval);
    }
  });

  // -- Play thread.
  let play_running = Arc::clone(&preview.running);
  let play_preview = Arc::clone(preview);
  let play_mix = preview
    .mixer_handle
    .read()
    .unwrap()
    .clone()
    .expect("Mixer handle must be set before spawning threads");
  let play_counter = preview
    .metrics
    .heartbeats_played
    .with_label_values(&[&cfg.name]);

  let play_handle = thread::spawn(move || {
    // Align to the wall-clock grid before the first play so that
    // clock-mode heartbeats with different offsets start staggered.
    // Loop and Continuous modes skip this to start playing immediately.
    {
      let cfg = &play_preview.heartbeat_configs.read().unwrap()[i];
      if cfg.playback == Playback::Clock {
        let wait = seconds_until_next(cfg.cycle_secs, cfg.cycle_offset_secs);
        if wait > 0.005 {
          sleep_checking(&play_running, Duration::from_secs_f64(wait));
        }
      }
    }

    while play_running.load(Ordering::Relaxed) {
      let mode = play_preview.heartbeat_configs.read().unwrap()[i].playback;
      match mode {
        Playback::Continuous => {
          play_continuous_tick(
            &play_running,
            &play_preview,
            &play_mix,
            i,
            &play_counter,
          );
        }
        Playback::Loop => {
          play_loop(&play_running, &play_preview, &play_mix, i, &play_counter);
        }
        Playback::Clock => {
          play_oneshot_once(
            &play_running,
            &play_preview,
            &play_mix,
            i,
            true,
            &play_counter,
          );
        }
      }
    }
  });

  (poll_handle, play_handle)
}

/// Resolve all notes for heartbeat `i` from the current metric
/// and transition config.  Shared by loop, oneshot, and continuous
/// playback.
fn resolve_notes(preview: &PreviewState, i: usize) -> Vec<ResolvedNote> {
  let metric = {
    let hbs = preview.heartbeats.read().unwrap();
    hbs[i].metric.value() as f64
  };
  let note_configs = {
    let cfg = &preview.heartbeat_configs.read().unwrap()[i];
    cfg.notes.clone()
  };
  let lib = preview.library.read().unwrap();
  note_configs
    .iter()
    .filter_map(|nc| {
      let patch = nc.transition.resolve(metric, &lib)?;
      Some(ResolvedNote {
        patch,
        volume: nc.volume,
        offset: nc.offset,
      })
    })
    .collect()
}

/// Build `(Patch, f64)` pairs for `continuous_graph_with_notes`
/// from resolved notes (patch with volume baked in is handled
/// inside `continuous_graph_with_notes`, so we pass raw volume).
fn notes_to_continuous_pairs(notes: &[ResolvedNote]) -> Vec<(Patch, f64)> {
  notes.iter().map(|n| (n.patch.clone(), n.volume)).collect()
}

/// Clone the effective_volume Shared for heartbeat `i`.  The clone
/// shares the same underlying atomic, so updates via
/// `update_effective_volume` are immediately visible.
fn clone_effective_volume(
  preview: &PreviewState,
  i: usize,
) -> fundsp::shared::Shared {
  preview.heartbeats.read().unwrap()[i]
    .effective_volume
    .clone()
}

/// Continuous morph playback: build a multi-note graph with
/// `Shared` controls, then update those controls as the metric
/// changes.  Rebuilds the graph when structural parameters or
/// note count change.  Returns when the daemon is shutting down
/// or the playback mode changes away from `Continuous`.
fn play_continuous_tick(
  running: &AtomicBool,
  preview: &PreviewState,
  play_mix: &MixerHandle,
  i: usize,
  counter: &prometheus::IntCounter,
) {
  // Wait for at least one valid note.
  let initial_notes = loop {
    if !running.load(Ordering::Relaxed) {
      return;
    }
    if preview.heartbeat_configs.read().unwrap()[i].playback
      != Playback::Continuous
    {
      return;
    }
    let notes = resolve_notes(preview, i);
    if !notes.is_empty() {
      break notes;
    }
    thread::sleep(Duration::from_secs(1));
  };

  let crossfade_ms = {
    let cfg = &preview.heartbeat_configs.read().unwrap()[i];
    cfg.crossfade_ms
  };
  let smoothing = crossfade_ms / 1000.0;

  let pairs = notes_to_continuous_pairs(&initial_notes);
  let eff_vol = clone_effective_volume(preview, i);
  preview.update_effective_volume(i);
  let (graph, mut all_controls, mut all_structural) =
    continuous_graph_with_notes(&pairs, smoothing, Some(&eff_vol));
  let mut note_count = pairs.len();
  let sid = play_mix.add(graph);
  counter.inc();

  while running.load(Ordering::Relaxed) {
    sleep_checking(running, Duration::from_millis(50));

    if !running.load(Ordering::Relaxed) {
      break;
    }

    if preview.heartbeat_configs.read().unwrap()[i].playback
      != Playback::Continuous
    {
      break;
    }

    let crossfade_ms = {
      let cfg = &preview.heartbeat_configs.read().unwrap()[i];
      cfg.crossfade_ms
    };

    let notes = resolve_notes(preview, i);
    if notes.is_empty() {
      continue;
    }

    preview.update_effective_volume(i);
    let pairs = notes_to_continuous_pairs(&notes);

    // Full rebuild if note count changed.
    if pairs.len() != note_count {
      let smoothing = crossfade_ms / 1000.0;
      let (graph, new_controls, new_structural) =
        continuous_graph_with_notes(&pairs, smoothing, Some(&eff_vol));
      let cf =
        ((crossfade_ms / 1000.0) * play_mix.sample_rate()).ceil() as usize;
      play_mix.replace(sid, graph, cf);
      all_controls = new_controls;
      all_structural = new_structural;
      note_count = pairs.len();
      continue;
    }

    // Update each note's controls; check for structural changes.
    let mut needs_rebuild = false;
    for (j, (patch, volume)) in pairs.iter().enumerate() {
      let mut p = patch.clone();
      p.amplitude *= volume;
      all_controls[j].update_from_patch(&p);
      let new_structural = StructuralParams::from_patch(&p);
      if new_structural != all_structural[j] {
        all_structural[j] = new_structural;
        needs_rebuild = true;
      }
    }

    if needs_rebuild {
      let smoothing = crossfade_ms / 1000.0;
      let (graph, new_controls, new_structural) =
        continuous_graph_with_notes(&pairs, smoothing, Some(&eff_vol));
      let cf =
        ((crossfade_ms / 1000.0) * play_mix.sample_rate()).ceil() as usize;
      play_mix.replace(sid, graph, cf);
      all_controls = new_controls;
      all_structural = new_structural;
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
  counter: &prometheus::IntCounter,
) {
  let notes = resolve_notes(preview, i);
  if notes.is_empty() {
    thread::sleep(Duration::from_secs(1));
    return;
  }

  let eff_vol = clone_effective_volume(preview, i);
  preview.update_effective_volume(i);
  let graph = heartbeat::heartbeat_graph_with_notes(&notes, Some(&eff_vol));
  let content_dur = heartbeat::heartbeat_notes_content_duration(&notes);
  let sid = play_mix.add(graph);
  counter.inc();

  sleep_checking(running, content_dur);

  while running.load(Ordering::Relaxed) {
    if preview.heartbeat_configs.read().unwrap()[i].playback != Playback::Loop {
      break;
    }

    let crossfade_ms = {
      let cfg = &preview.heartbeat_configs.read().unwrap()[i];
      cfg.crossfade_ms
    };
    let notes = resolve_notes(preview, i);
    if notes.is_empty() {
      break;
    }

    preview.update_effective_volume(i);
    let graph = heartbeat::heartbeat_graph_with_notes(&notes, Some(&eff_vol));
    let content_dur = heartbeat::heartbeat_notes_content_duration(&notes);
    let cf = ((crossfade_ms / 1000.0) * play_mix.sample_rate()).ceil() as usize;
    play_mix.replace(sid, graph, cf);
    counter.inc();

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
  counter: &prometheus::IntCounter,
) {
  let (cycle_secs, cycle_offset) = {
    let cfg = &preview.heartbeat_configs.read().unwrap()[i];
    (cfg.cycle_secs, cfg.cycle_offset_secs)
  };
  let notes = resolve_notes(preview, i);
  if notes.is_empty() {
    thread::sleep(Duration::from_secs(1));
    return;
  }

  let eff_vol = clone_effective_volume(preview, i);
  preview.update_effective_volume(i);
  let graph = heartbeat::heartbeat_graph_with_notes(&notes, Some(&eff_vol));
  let dur = heartbeat::heartbeat_notes_duration(&notes);

  let sid = play_mix.add(graph);
  sleep_checking(running, dur);
  play_mix.remove(sid);
  counter.inc();

  if !running.load(Ordering::Relaxed) {
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
