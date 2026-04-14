use crate::lock_util::RecoverPoison;
use crate::preview_state::{metric_label, PreviewState};
use serde_json::json;
use sonify_health_lib::{
  audio::{AudioError, AudioMixer, MixerHandle},
  continuous_graph_with_notes, heartbeat, probe, seconds_until_next, Patch,
  Playback, ResolvedNote, StructuralParams,
};
use std::any::Any;
use std::sync::{
  atomic::{AtomicBool, Ordering},
  Arc,
};
use std::thread;
use std::time::{Duration, Instant};
use thiserror::Error;
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// Supervision constants
// ---------------------------------------------------------------------------

/// How often (in ~100 ms ticks) the main loop checks thread health.
const SUPERVISION_CHECK_INTERVAL: u32 = 10; // ~1 s

/// Maximum panics per thread within `FAILURE_WINDOW` before the
/// daemon gives up and shuts down.
const MAX_FAILURES_PER_THREAD: usize = 5;

/// Rolling window for counting thread failures.
const FAILURE_WINDOW: Duration = Duration::from_secs(300); // 5 min

/// How often (in ~100 ms ticks) audio health stats are logged and
/// pushed to Prometheus.
const HEALTH_LOG_INTERVAL: u32 = 300; // ~30 s

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum DaemonError {
  #[error("Audio playback failed: {0}")]
  Audio(#[from] AudioError),

  #[error(
    "Thread failure budget exhausted for heartbeat {heartbeat} ({role})"
  )]
  ThreadBudgetExhausted { heartbeat: usize, role: String },
}

// ---------------------------------------------------------------------------
// Thread supervision types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadRole {
  Poll,
  Play,
}

impl std::fmt::Display for ThreadRole {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      ThreadRole::Poll => write!(f, "poll"),
      ThreadRole::Play => write!(f, "play"),
    }
  }
}

pub struct SupervisedThread {
  pub handle: Option<thread::JoinHandle<()>>,
  pub heartbeat_index: usize,
  pub role: ThreadRole,
  pub failures: Vec<Instant>,
}

impl SupervisedThread {
  fn new(
    handle: thread::JoinHandle<()>,
    heartbeat_index: usize,
    role: ThreadRole,
  ) -> Self {
    Self {
      handle: Some(handle),
      heartbeat_index,
      role,
      failures: Vec::new(),
    }
  }

  /// Record a failure, pruning old entries outside the window.
  /// Returns `true` if the failure budget is exhausted.
  pub fn record_failure(&mut self) -> bool {
    let now = Instant::now();
    self.failures.push(now);
    self
      .failures
      .retain(|t| now.duration_since(*t) < FAILURE_WINDOW);
    self.failures.len() > MAX_FAILURES_PER_THREAD
  }
}

// ---------------------------------------------------------------------------
// Panic payload extraction
// ---------------------------------------------------------------------------

/// Extract a human-readable message from a panic payload.
pub fn extract_panic_message(payload: &Box<dyn Any + Send>) -> String {
  if let Some(s) = payload.downcast_ref::<&str>() {
    (*s).to_string()
  } else if let Some(s) = payload.downcast_ref::<String>() {
    s.clone()
  } else {
    format!("<non-string panic: {:?}>", (**payload).type_id())
  }
}

// ---------------------------------------------------------------------------
// DaemonContext + run_daemon
// ---------------------------------------------------------------------------

/// Everything the daemon thread needs from main.
pub struct DaemonContext<'a> {
  pub audio_device: Option<&'a str>,
  pub preview: Arc<PreviewState>,
}

/// Run the daemon's main loop: spawn per-heartbeat poll/play threads,
/// supervise them, and respond to preview-UI actions.  Shuts down
/// when `running` becomes false or a thread exhausts its failure
/// budget.
pub fn run_daemon(ctx: DaemonContext<'_>) -> Result<(), DaemonError> {
  let DaemonContext {
    audio_device,
    preview,
  } = ctx;

  let mut mixer = AudioMixer::new(audio_device)?;
  preview.set_mixer_handle(mixer.handle());

  let mut supervised: Vec<SupervisedThread> = Vec::new();

  // Snapshot heartbeat configs for thread setup; transitions are
  // re-read at runtime so live edits take effect.
  let hb_count = preview.heartbeat_configs.read().unwrap_or_recover().len();
  for i in 0..hb_count {
    let poll_h = spawn_poll_thread(&preview, i);
    let play_h = spawn_play_thread(&preview, i);
    supervised.push(SupervisedThread::new(poll_h, i, ThreadRole::Poll));
    supervised.push(SupervisedThread::new(play_h, i, ThreadRole::Play));
  }

  info!(heartbeats = hb_count, "Daemon started");

  // Stream recovery state (exponential backoff).
  const MAX_RECOVERY_BACKOFF: Duration = Duration::from_secs(60);
  let mut recovery_backoff = Duration::from_secs(1);
  let mut next_recovery_at: Option<Instant> = None;
  let mut recovery_attempts: u64 = 0;

  // Main loop: handle mute transitions + supervision + health.
  let mut was_muted = preview.muted.load(Ordering::Relaxed);
  let mut tick: u32 = 0;

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

    // -- Supervision check (~every 1 s).
    if tick.is_multiple_of(SUPERVISION_CHECK_INTERVAL) {
      let mut budget_exhausted: Option<(usize, String)> = None;

      for st in supervised.iter_mut() {
        let finished =
          st.handle.as_ref().map(|h| h.is_finished()).unwrap_or(false);
        if !finished {
          continue;
        }

        let handle = st.handle.take().unwrap();
        match handle.join() {
          Ok(()) => {
            debug!(
              heartbeat = st.heartbeat_index,
              role = %st.role,
              "Thread exited cleanly"
            );
          }
          Err(payload) => {
            let msg = extract_panic_message(&payload);
            error!(
              heartbeat = st.heartbeat_index,
              role = %st.role,
              panic = msg,
              "Thread panicked"
            );
            if st.record_failure() {
              error!(
                heartbeat = st.heartbeat_index,
                role = %st.role,
                "Failure budget exhausted, shutting down"
              );
              budget_exhausted =
                Some((st.heartbeat_index, st.role.to_string()));
              break;
            }

            // Respawn.
            info!(
              heartbeat = st.heartbeat_index,
              role = %st.role,
              recent_failures = st.failures.len(),
              "Respawning thread"
            );
            let new_handle = match st.role {
              ThreadRole::Poll => {
                spawn_poll_thread(&preview, st.heartbeat_index)
              }
              ThreadRole::Play => {
                spawn_play_thread(&preview, st.heartbeat_index)
              }
            };
            st.handle = Some(new_handle);
          }
        }
      }

      if let Some((heartbeat, role)) = budget_exhausted {
        preview.running.store(false, Ordering::Relaxed);
        for st in supervised.iter_mut() {
          if let Some(h) = st.handle.take() {
            if let Err(p) = h.join() {
              let m = extract_panic_message(&p);
              warn!(
                heartbeat = st.heartbeat_index,
                role = %st.role,
                panic = m,
                "Thread panicked during shutdown"
              );
            }
          }
        }
        mixer.clear();
        return Err(DaemonError::ThreadBudgetExhausted { heartbeat, role });
      }

      // -- Stream recovery: if the audio stream has failed, attempt
      //    to rebuild it with exponential backoff.
      if mixer.stream_failed() {
        let should_try = match next_recovery_at {
          Some(t) => Instant::now() >= t,
          None => true,
        };
        if should_try {
          recovery_attempts += 1;
          preview
            .metrics
            .audio_recovery_attempts
            .set(recovery_attempts as i64);
          match mixer.try_recover() {
            Ok(()) => {
              info!(
                attempts = recovery_attempts,
                "Audio stream recovered successfully"
              );
              recovery_backoff = Duration::from_secs(1);
              next_recovery_at = None;
            }
            Err(e) => {
              let next = Instant::now() + recovery_backoff;
              error!(
                error = %e,
                next_retry_secs = recovery_backoff.as_secs(),
                attempts = recovery_attempts,
                "Audio stream recovery failed"
              );
              recovery_backoff =
                (recovery_backoff * 2).min(MAX_RECOVERY_BACKOFF);
              next_recovery_at = Some(next);
            }
          }
        }
      }
    }

    // -- Health logging (~every 30 s).
    if tick > 0 && tick.is_multiple_of(HEALTH_LOG_INTERVAL) {
      let lock_fail = mixer.lock_failures();
      let nan = mixer.nan_frames();
      let peak_us = mixer.peak_callback_us();
      let stream_errs = mixer.stream_errors();
      let stream_fail = mixer.stream_failed();

      preview.metrics.audio_lock_failures.set(lock_fail as i64);
      preview.metrics.audio_nan_frames.set(nan as i64);
      preview.metrics.audio_peak_callback_us.set(peak_us as i64);
      preview.metrics.audio_stream_errors.set(stream_errs as i64);
      preview
        .metrics
        .audio_stream_failed
        .set(i64::from(stream_fail));
      mixer.reset_peak_callback_us();

      if lock_fail > 0 || nan > 0 || stream_fail {
        warn!(
          lock_failures = lock_fail,
          nan_frames = nan,
          peak_callback_us = peak_us,
          stream_errors = stream_errs,
          stream_failed = stream_fail,
          "Audio health"
        );
      } else {
        debug!(
          lock_failures = lock_fail,
          nan_frames = nan,
          peak_callback_us = peak_us,
          stream_errors = stream_errs,
          stream_failed = stream_fail,
          "Audio health"
        );
      }
    }

    tick = tick.wrapping_add(1);
    thread::sleep(Duration::from_millis(100));
  }

  info!("Waiting for heartbeat threads to finish");
  for st in supervised.iter_mut() {
    if let Some(h) = st.handle.take() {
      match h.join() {
        Ok(()) => {}
        Err(payload) => {
          let msg = extract_panic_message(&payload);
          warn!(
            heartbeat = st.heartbeat_index,
            role = %st.role,
            panic = msg,
            "Thread panicked during shutdown"
          );
        }
      }
    }
  }
  mixer.clear();
  info!("Daemon stopped");
  Ok(())
}

// ---------------------------------------------------------------------------
// Thread spawning
// ---------------------------------------------------------------------------

/// Spawn a poll thread for heartbeat at `index`.  Re-reads config
/// each iteration so live UI edits take effect.
pub fn spawn_poll_thread(
  preview: &Arc<PreviewState>,
  i: usize,
) -> thread::JoinHandle<()> {
  let poll_running = Arc::clone(&preview.running);
  let poll_preview = Arc::clone(preview);
  let cfg = preview.heartbeat_configs.read().unwrap_or_recover()[i].clone();
  let poll_counter = preview
    .metrics
    .probes_completed
    .with_label_values(&[&cfg.name]);
  let poll_gauge = preview.metrics.probe_value.with_label_values(&[&cfg.name]);

  thread::spawn(move || {
    while poll_running.load(Ordering::Relaxed) {
      let (cfg_name, command, mode, tiers, interval) = {
        let configs = poll_preview.heartbeat_configs.read().unwrap_or_recover();
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
        let hbs = poll_preview.heartbeats.read().unwrap_or_recover();
        let val = *hbs[i].override_value.read().unwrap_or_recover();
        val
      };

      let resolved = if let Some(metric) = overridden {
        let clamped = metric.clamp(0.0, 1.0);
        {
          let hbs = poll_preview.heartbeats.read().unwrap_or_recover();
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
          Ok(output) => {
            if !output.stderr.is_empty() {
              debug!(
                heartbeat = cfg_name,
                stderr = output.stderr,
                "Probe stderr"
              );
            }
            let label = metric_label(output.metric, &tiers);
            info!(heartbeat = cfg_name, result = label, "Probe completed");
            {
              let hbs = poll_preview.heartbeats.read().unwrap_or_recover();
              hbs[i].metric.set_value(output.metric);
            }
            send_probe_log(&poll_preview, &cfg_name, &label, false);
            poll_counter.inc();
            poll_gauge.set(output.metric as f64);
            Some(output.metric)
          }
          Err(e) => {
            let stderr = match &e {
              probe::ProbeError::ProbeSignaled { stderr, .. }
              | probe::ProbeError::ProbeInvalidStdout { stderr, .. } => {
                Some(stderr.as_str())
              }
              probe::ProbeError::ProbeExecution { .. } => None,
            };
            if let Some(text) = stderr.filter(|s| !s.is_empty()) {
              warn!(
                heartbeat = cfg_name,
                error = %e,
                stderr = text,
                "Probe failed, retaining previous metric"
              );
            } else {
              warn!(
                heartbeat = cfg_name,
                error = %e,
                "Probe failed, retaining previous metric"
              );
            }
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

      let metric_val = {
        let hbs = poll_preview.heartbeats.read().unwrap_or_recover();
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
  })
}

/// Spawn a play thread for heartbeat at `index`.  Extracted from
/// `spawn_heartbeat_threads` so both poll and play can be respawned
/// independently by the supervisor.
pub fn spawn_play_thread(
  preview: &Arc<PreviewState>,
  i: usize,
) -> thread::JoinHandle<()> {
  let play_running = Arc::clone(&preview.running);
  let play_preview = Arc::clone(preview);
  let play_mix = preview
    .mixer_handle
    .read()
    .unwrap_or_recover()
    .clone()
    .expect("Mixer handle must be set before spawning threads");
  let cfg_name = preview.heartbeat_configs.read().unwrap_or_recover()[i]
    .name
    .clone();
  let play_counter = preview
    .metrics
    .heartbeats_played
    .with_label_values(&[&cfg_name]);

  thread::spawn(move || {
    // Align to the wall-clock grid before the first play so that
    // clock-mode heartbeats with different offsets start staggered.
    // Loop and Continuous modes skip this to start playing immediately.
    {
      let cfg = &play_preview.heartbeat_configs.read().unwrap_or_recover()[i];
      if cfg.playback == Playback::Clock {
        let wait = seconds_until_next(cfg.cycle_secs, cfg.cycle_offset_secs);
        if wait > 0.005 {
          sleep_checking(&play_running, Duration::from_secs_f64(wait));
        }
      }
    }

    while play_running.load(Ordering::Relaxed) {
      let mode =
        play_preview.heartbeat_configs.read().unwrap_or_recover()[i].playback;
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
  })
}

/// Spawn poll and play threads for heartbeat at `index`.  Reads
/// the config snapshot and prometheus labels at spawn time.
pub fn spawn_heartbeat_threads(
  preview: &Arc<PreviewState>,
  i: usize,
) -> (thread::JoinHandle<()>, thread::JoinHandle<()>) {
  let poll_handle = spawn_poll_thread(preview, i);
  let play_handle = spawn_play_thread(preview, i);
  (poll_handle, play_handle)
}

// ---------------------------------------------------------------------------
// Audio playback helpers
// ---------------------------------------------------------------------------

/// Resolve all notes for heartbeat `i` from the current metric
/// and transition config.  Shared by loop, oneshot, and continuous
/// playback.
fn resolve_notes(preview: &PreviewState, i: usize) -> Vec<ResolvedNote> {
  let metric = {
    let hbs = preview.heartbeats.read().unwrap_or_recover();
    hbs[i].metric.value() as f64
  };
  let note_configs = {
    let cfg = &preview.heartbeat_configs.read().unwrap_or_recover()[i];
    cfg.notes.clone()
  };
  let lib = preview.library.read().unwrap_or_recover();
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
  preview.heartbeats.read().unwrap_or_recover()[i]
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
    if preview.heartbeat_configs.read().unwrap_or_recover()[i].playback
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
    let cfg = &preview.heartbeat_configs.read().unwrap_or_recover()[i];
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

    if preview.heartbeat_configs.read().unwrap_or_recover()[i].playback
      != Playback::Continuous
    {
      break;
    }

    let crossfade_ms = {
      let cfg = &preview.heartbeat_configs.read().unwrap_or_recover()[i];
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
    if preview.heartbeat_configs.read().unwrap_or_recover()[i].playback
      != Playback::Loop
    {
      break;
    }

    let crossfade_ms = {
      let cfg = &preview.heartbeat_configs.read().unwrap_or_recover()[i];
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
    let cfg = &preview.heartbeat_configs.read().unwrap_or_recover()[i];
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
  let deadline = Instant::now() + dur;
  while Instant::now() < deadline && running.load(Ordering::Relaxed) {
    let remaining = deadline.saturating_duration_since(Instant::now());
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn extract_panic_message_str() {
    let payload: Box<dyn Any + Send> = Box::new("boom");
    assert_eq!(extract_panic_message(&payload), "boom");
  }

  #[test]
  fn extract_panic_message_string() {
    let payload: Box<dyn Any + Send> = Box::new(String::from("kaboom"));
    assert_eq!(extract_panic_message(&payload), "kaboom");
  }

  #[test]
  fn extract_panic_message_unknown() {
    let payload: Box<dyn Any + Send> = Box::new(42_i32);
    let msg = extract_panic_message(&payload);
    assert!(msg.starts_with("<non-string panic:"), "Unexpected message: {msg}");
  }

  #[test]
  fn supervised_thread_records_failures_in_window() {
    // Use a dummy handle (spawn a no-op thread).
    let h = thread::spawn(|| {});
    let mut st = SupervisedThread::new(h, 0, ThreadRole::Poll);
    for _ in 0..MAX_FAILURES_PER_THREAD {
      assert!(!st.record_failure(), "should not be exhausted yet");
    }
    assert_eq!(st.failures.len(), MAX_FAILURES_PER_THREAD);
  }

  #[test]
  fn supervised_thread_budget_exhaustion() {
    let h = thread::spawn(|| {});
    let mut st = SupervisedThread::new(h, 0, ThreadRole::Play);
    for _ in 0..MAX_FAILURES_PER_THREAD {
      st.record_failure();
    }
    // One more should exhaust the budget.
    assert!(st.record_failure(), "budget should be exhausted");
  }

  #[test]
  fn supervised_thread_prunes_old_failures() {
    let h = thread::spawn(|| {});
    let mut st = SupervisedThread::new(h, 0, ThreadRole::Poll);

    // Insert old failures that would be outside the window.
    let old = Instant::now() - FAILURE_WINDOW - Duration::from_secs(1);
    for _ in 0..MAX_FAILURES_PER_THREAD {
      st.failures.push(old);
    }
    assert_eq!(st.failures.len(), MAX_FAILURES_PER_THREAD);

    // Recording a new failure should prune the old ones.
    let exhausted = st.record_failure();
    assert!(!exhausted, "old failures should be pruned");
    assert_eq!(st.failures.len(), 1);
  }
}
