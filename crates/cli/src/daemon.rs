use crate::lock_util::RecoverPoison;
use crate::preview_state::{metric_label, PreviewState, Source, SourceKind};
use serde_json::json;
use sonify_health_lib::{
  audio::{AudioError, AudioMixer, MixerHandle, MAX_MIXER_SLOTS},
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

/// How often the continuous graph is forcibly rebuilt to reset IIR
/// filter state and prevent numerical drift (e.g. Moog ladder
/// instability).
const CONTINUOUS_REBUILD_INTERVAL: Duration = Duration::from_secs(3600);

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum DaemonError {
  #[error("Audio playback failed: {0}")]
  Audio(#[from] AudioError),

  // `source` would collide with thiserror's reserved name for the
  // underlying error chain, so use `source_name` instead.
  #[error(
    "Thread failure budget exhausted for source {source_name:?} heartbeat {heartbeat} ({role})"
  )]
  ThreadBudgetExhausted {
    source_name: String,
    heartbeat: usize,
    role: String,
  },
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
  pub source_idx: usize,
  pub source_name: String,
  pub hb_idx: usize,
  pub role: ThreadRole,
  pub failures: Vec<Instant>,
}

impl SupervisedThread {
  fn new(
    handle: thread::JoinHandle<()>,
    source_idx: usize,
    source_name: String,
    hb_idx: usize,
    role: ThreadRole,
  ) -> Self {
    Self {
      handle: Some(handle),
      source_idx,
      source_name,
      hb_idx,
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
  /// When true, the daemon does not open an audio device and does
  /// not spawn play threads.  Poll threads, supervision, and the
  /// rest of the run-loop continue normally.
  pub headless: bool,
  pub preview: Arc<PreviewState>,
}

/// Run the daemon's main loop: spawn per-heartbeat poll/play threads,
/// supervise them, and respond to preview-UI actions.  Shuts down
/// when `running` becomes false or a thread exhausts its failure
/// budget.
pub fn run_daemon(ctx: DaemonContext<'_>) -> Result<(), DaemonError> {
  let DaemonContext {
    audio_device,
    headless,
    preview,
  } = ctx;

  // Headless instances skip the audio device entirely.  The mixer,
  // play threads, audio recovery, and audio health logging are all
  // gated on this `Option`; everything else (pollers, supervision,
  // the WebSocket-driven mute/volume hooks) runs unconditionally.
  let mut mixer = if headless {
    None
  } else {
    let m = AudioMixer::new(audio_device)?;
    preview.set_mixer_handle(m.handle());
    Some(m)
  };

  let mut supervised: Vec<SupervisedThread> = Vec::new();
  let mut total_heartbeats = 0usize;

  // Snapshot heartbeat configs for thread setup; transitions are
  // re-read at runtime so live edits take effect.  Iterate every
  // Source even though today only the Local Source exists, so the
  // poll/play thread topology already understands per-Source
  // addressing when remote Sources arrive.
  for (source_idx, source) in preview.sources.iter().enumerate() {
    let hb_count = source.heartbeats.read().unwrap_or_recover().len();
    total_heartbeats += hb_count;
    for hb_idx in 0..hb_count {
      let poll_h = spawn_poll_thread(&preview, source_idx, hb_idx);
      supervised.push(SupervisedThread::new(
        poll_h,
        source_idx,
        source.name.clone(),
        hb_idx,
        ThreadRole::Poll,
      ));
      if !headless {
        let play_h = spawn_play_thread(&preview, source_idx, hb_idx);
        supervised.push(SupervisedThread::new(
          play_h,
          source_idx,
          source.name.clone(),
          hb_idx,
          ThreadRole::Play,
        ));
      }
    }
  }

  info!(
    sources = preview.sources.len(),
    heartbeats = total_heartbeats,
    headless,
    "Daemon started"
  );

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
      let mut budget_exhausted: Option<(String, usize, String)> = None;

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
              source = %st.source_name,
              heartbeat = st.hb_idx,
              role = %st.role,
              "Thread exited cleanly"
            );
          }
          Err(payload) => {
            let msg = extract_panic_message(&payload);
            error!(
              source = %st.source_name,
              heartbeat = st.hb_idx,
              role = %st.role,
              panic = msg,
              "Thread panicked"
            );
            if st.record_failure() {
              error!(
                source = %st.source_name,
                heartbeat = st.hb_idx,
                role = %st.role,
                "Failure budget exhausted, shutting down"
              );
              budget_exhausted =
                Some((st.source_name.clone(), st.hb_idx, st.role.to_string()));
              break;
            }

            // Respawn.
            info!(
              source = %st.source_name,
              heartbeat = st.hb_idx,
              role = %st.role,
              recent_failures = st.failures.len(),
              "Respawning thread"
            );
            let new_handle = match st.role {
              ThreadRole::Poll => {
                spawn_poll_thread(&preview, st.source_idx, st.hb_idx)
              }
              ThreadRole::Play => {
                spawn_play_thread(&preview, st.source_idx, st.hb_idx)
              }
            };
            st.handle = Some(new_handle);
          }
        }
      }

      if let Some((source_name, heartbeat, role)) = budget_exhausted {
        preview.running.store(false, Ordering::Relaxed);
        for st in supervised.iter_mut() {
          if let Some(h) = st.handle.take() {
            if let Err(p) = h.join() {
              let m = extract_panic_message(&p);
              warn!(
                source = %st.source_name,
                heartbeat = st.hb_idx,
                role = %st.role,
                panic = m,
                "Thread panicked during shutdown"
              );
            }
          }
        }
        if let Some(m) = mixer.as_mut() {
          m.clear();
        }
        return Err(DaemonError::ThreadBudgetExhausted {
          source_name,
          heartbeat,
          role,
        });
      }

      // -- Ensure play threads exist for every (source, hb) pair.
      //    Catches Remote Sources whose heartbeats arrive after
      //    startup, and respawns any that exited cleanly because
      //    a heartbeat was removed and then re-added.
      if !headless {
        rebalance_play_threads(&preview, &mut supervised);
      }

      // -- Stream recovery: if the audio stream has failed, attempt
      //    to rebuild it with exponential backoff.  Skipped entirely
      //    in headless mode (no mixer to recover).
      if let Some(mixer) = mixer.as_mut() {
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
    }

    // -- Health logging (~every 30 s).  Skipped in headless mode —
    //    no mixer means no audio metrics to publish.
    if tick > 0 && tick.is_multiple_of(HEALTH_LOG_INTERVAL) {
      if let Some(mixer) = mixer.as_mut() {
        let lock_fail = mixer.lock_failures();
        let nan = mixer.nan_frames();
        let peak_us = mixer.peak_callback_us();
        let stream_errs = mixer.stream_errors();
        let stream_fail = mixer.stream_failed();

        let out_peak = mixer.output_peak_amplitude();
        let slot_peaks = mixer.slot_peak_amplitudes();
        let slot_rms = mixer.slot_rms_amplitudes();
        let (buf_min, buf_max) = mixer.callback_buffer_range();
        mixer.reset_amplitude_stats();

        preview.metrics.audio_lock_failures.set(lock_fail as i64);
        preview.metrics.audio_nan_frames.set(nan as i64);
        preview.metrics.audio_peak_callback_us.set(peak_us as i64);
        preview.metrics.audio_stream_errors.set(stream_errs as i64);
        preview
          .metrics
          .audio_stream_failed
          .set(i64::from(stream_fail));
        mixer.reset_peak_callback_us();

        preview
          .metrics
          .audio_output_peak_amplitude
          .set(out_peak as f64);
        for i in 0..MAX_MIXER_SLOTS {
          let label = i.to_string();
          preview
            .metrics
            .audio_slot_peak_amplitude
            .with_label_values(&[&label])
            .set(slot_peaks[i] as f64);
          preview
            .metrics
            .audio_slot_rms_amplitude
            .with_label_values(&[&label])
            .set(slot_rms[i] as f64);
        }
        preview.metrics.audio_callback_buffer_frames_min.set(
          if buf_min == u32::MAX {
            0
          } else {
            buf_min as i64
          },
        );
        preview
          .metrics
          .audio_callback_buffer_frames_max
          .set(buf_max as i64);

        // Format non-zero slot amplitudes as "0:0.123/0.089 1:0.456/0.234".
        let slot_amplitudes: String = (0..MAX_MIXER_SLOTS)
          .filter(|&i| slot_peaks[i] > 0.0)
          .map(|i| format!("{}:{:.4}/{:.4}", i, slot_peaks[i], slot_rms[i]))
          .collect::<Vec<_>>()
          .join(" ");

        let clipping = out_peak > 1.0;
        let period_renego =
          buf_min != buf_max && buf_min != u32::MAX && buf_max != 0;

        if lock_fail > 0 || nan > 0 || stream_fail || clipping || period_renego
        {
          warn!(
            lock_failures = lock_fail,
            nan_frames = nan,
            peak_callback_us = peak_us,
            stream_errors = stream_errs,
            stream_failed = stream_fail,
            output_peak = out_peak,
            slot_amplitudes = slot_amplitudes.as_str(),
            callback_buffer_min = if buf_min == u32::MAX { 0 } else { buf_min },
            callback_buffer_max = buf_max,
            "Audio health"
          );
        } else {
          debug!(
            lock_failures = lock_fail,
            nan_frames = nan,
            peak_callback_us = peak_us,
            stream_errors = stream_errs,
            stream_failed = stream_fail,
            output_peak = out_peak,
            slot_amplitudes = slot_amplitudes.as_str(),
            callback_buffer_min = if buf_min == u32::MAX { 0 } else { buf_min },
            callback_buffer_max = buf_max,
            "Audio health"
          );
        }
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
            source = %st.source_name,
            heartbeat = st.hb_idx,
            role = %st.role,
            panic = msg,
            "Thread panicked during shutdown"
          );
        }
      }
    }
  }
  if let Some(mixer) = mixer.as_mut() {
    mixer.clear();
  }
  info!("Daemon stopped");
  Ok(())
}

// ---------------------------------------------------------------------------
// Thread spawning
// ---------------------------------------------------------------------------

/// Borrow the Source at `source_idx` from `preview`.  Panics if the
/// index is out of bounds — the supervisor only constructs valid
/// indices.
fn source_at(preview: &PreviewState, source_idx: usize) -> &Source {
  &preview.sources[source_idx]
}

/// Whether the play threads for a Source should currently emit
/// audio.  Local sources always play (the global `headless` flag
/// already gates whether play threads exist at all).  Remote
/// sources play only when their `playback_enabled` toggle is on,
/// so a listener can mirror state silently without sounding it.
fn source_should_play(source: &Source) -> bool {
  match &source.kind {
    SourceKind::Local => true,
    SourceKind::Remote {
      playback_enabled, ..
    } => playback_enabled.load(Ordering::Relaxed),
  }
}

/// Spawn a poll thread for `(source_idx, hb_idx)`.  Re-reads config
/// each iteration so live UI edits take effect.
pub fn spawn_poll_thread(
  preview: &Arc<PreviewState>,
  source_idx: usize,
  hb_idx: usize,
) -> thread::JoinHandle<()> {
  let poll_running = Arc::clone(&preview.running);
  let poll_preview = Arc::clone(preview);
  let cfg = source_at(preview, source_idx)
    .heartbeat_configs
    .read()
    .unwrap_or_recover()[hb_idx]
    .clone();
  let poll_counter = preview
    .metrics
    .probes_completed
    .with_label_values(&[&cfg.name]);
  let poll_gauge = preview.metrics.probe_value.with_label_values(&[&cfg.name]);

  let source_name = source_at(preview, source_idx).name.clone();

  thread::spawn(move || {
    while poll_running.load(Ordering::Relaxed) {
      let source = source_at(&poll_preview, source_idx);
      let (cfg_name, command, mode, tiers, interval) = {
        let configs = source.heartbeat_configs.read().unwrap_or_recover();
        let cfg = &configs[hb_idx];
        (
          cfg.name.clone(),
          cfg.command.clone(),
          cfg.result_mode.clone(),
          cfg.tiers.clone(),
          Duration::from_secs_f64(cfg.poll_interval_secs),
        )
      };

      let overridden = {
        let hbs = source.heartbeats.read().unwrap_or_recover();
        let val = *hbs[hb_idx].override_value.read().unwrap_or_recover();
        val
      };

      let resolved = if let Some(metric) = overridden {
        let clamped = metric.clamp(0.0, 1.0);
        {
          let hbs = source.heartbeats.read().unwrap_or_recover();
          hbs[hb_idx].metric.set_value(clamped);
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
                source = %source_name,
                heartbeat = cfg_name,
                stderr = output.stderr,
                "Probe stderr"
              );
            }
            let label = metric_label(output.metric, &tiers);
            info!(
              source = %source_name,
              heartbeat = cfg_name,
              result = label,
              "Probe completed"
            );
            {
              let hbs = source.heartbeats.read().unwrap_or_recover();
              hbs[hb_idx].metric.set_value(output.metric);
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
                source = %source_name,
                heartbeat = cfg_name,
                error = %e,
                stderr = text,
                "Probe failed, retaining previous metric"
              );
            } else {
              warn!(
                source = %source_name,
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
          source = %source_name,
          heartbeat = cfg_name,
          result = label,
          overridden = overridden.is_some(),
          "Metric resolved"
        );
      }

      let metric_val = {
        let hbs = source.heartbeats.read().unwrap_or_recover();
        hbs[hb_idx].metric.value()
      };
      // The wire protocol does not yet carry source identifiers, so
      // emit only `index` (= hb_idx).  When the protocol grows a
      // source field, include source_idx here too.
      let _ = poll_preview.broadcast_tx.send(
        json!({
          "type": "metric_changed",
          "index": hb_idx,
          "value": metric_val,
        })
        .to_string(),
      );

      sleep_checking(&poll_running, interval);
    }
  })
}

/// Spawn a play thread for `(source_idx, hb_idx)`.  Extracted from
/// `spawn_heartbeat_threads` so both poll and play can be respawned
/// independently by the supervisor.
pub fn spawn_play_thread(
  preview: &Arc<PreviewState>,
  source_idx: usize,
  hb_idx: usize,
) -> thread::JoinHandle<()> {
  let play_running = Arc::clone(&preview.running);
  let play_preview = Arc::clone(preview);
  let play_mix = preview
    .mixer_handle
    .read()
    .unwrap_or_recover()
    .clone()
    .expect("Mixer handle must be set before spawning threads");
  let cfg_name = source_at(preview, source_idx)
    .heartbeat_configs
    .read()
    .unwrap_or_recover()[hb_idx]
    .name
    .clone();
  let play_counter = preview
    .metrics
    .heartbeats_played
    .with_label_values(&[&cfg_name]);

  thread::spawn(move || {
    let source = source_at(&play_preview, source_idx);

    // Align to the wall-clock grid before the first play so that
    // clock-mode heartbeats with different offsets start staggered.
    // Loop and Continuous modes skip this to start playing immediately.
    {
      let configs = source.heartbeat_configs.read().unwrap_or_recover();
      let Some(cfg) = configs.get(hb_idx) else {
        return;
      };
      if cfg.playback == Playback::Clock {
        let wait = seconds_until_next(cfg.cycle_secs, cfg.cycle_offset_secs);
        if wait > 0.005 {
          sleep_checking(&play_running, Duration::from_secs_f64(wait));
        }
      }
    }

    while play_running.load(Ordering::Relaxed) {
      // Exit cleanly when the heartbeat is removed (e.g. a remote
      // shape change that shrinks the list past `hb_idx`).
      let mode = match source
        .heartbeat_configs
        .read()
        .unwrap_or_recover()
        .get(hb_idx)
      {
        Some(cfg) => cfg.playback,
        None => return,
      };

      // For Remote sources, idle while the user has playback off.
      // The thread stays alive; it just doesn't produce audio.
      if !source_should_play(source) {
        sleep_checking(&play_running, Duration::from_millis(200));
        continue;
      }

      match mode {
        Playback::Continuous => {
          play_continuous_tick(
            &play_running,
            &play_preview,
            &play_mix,
            source_idx,
            hb_idx,
            &play_counter,
          );
        }
        Playback::Loop => {
          play_loop(
            &play_running,
            &play_preview,
            &play_mix,
            source_idx,
            hb_idx,
            &play_counter,
          );
        }
        Playback::Clock => {
          play_oneshot_once(
            &play_running,
            &play_preview,
            &play_mix,
            source_idx,
            hb_idx,
            true,
            &play_counter,
          );
        }
      }
    }
  })
}

/// Ensure a play thread is alive for every `(source_idx, hb_idx)`
/// pair that exists in the current `preview.sources`.  Called on
/// each supervision tick so a Remote Source whose heartbeat list
/// arrives or grows after startup picks up play threads without
/// requiring a daemon restart.
///
/// The check considers a pair "live" only when its `SupervisedThread`
/// has a `handle: Some(h)` and `!h.is_finished()`.  Existing entries
/// whose handle has been taken (clean exit) get a fresh thread;
/// pairs that were never spawned get a new `SupervisedThread`.
///
/// Skipped entirely when the daemon is headless — headless instances
/// never spawn play threads, by design.
fn rebalance_play_threads(
  preview: &Arc<PreviewState>,
  supervised: &mut Vec<SupervisedThread>,
) {
  for (source_idx, source) in preview.sources.iter().enumerate() {
    let hb_count = source.heartbeats.read().unwrap_or_recover().len();
    for hb_idx in 0..hb_count {
      let live = supervised.iter().any(|st| {
        st.source_idx == source_idx
          && st.hb_idx == hb_idx
          && st.role == ThreadRole::Play
          && st
            .handle
            .as_ref()
            .map(|h| !h.is_finished())
            .unwrap_or(false)
      });
      if live {
        continue;
      }
      let play_h = spawn_play_thread(preview, source_idx, hb_idx);
      let existing = supervised.iter_mut().find(|st| {
        st.source_idx == source_idx
          && st.hb_idx == hb_idx
          && st.role == ThreadRole::Play
      });
      match existing {
        Some(st) => {
          st.handle = Some(play_h);
        }
        None => {
          supervised.push(SupervisedThread::new(
            play_h,
            source_idx,
            source.name.clone(),
            hb_idx,
            ThreadRole::Play,
          ));
        }
      }
    }
  }
}

/// Spawn poll and play threads for the heartbeat at `hb_idx` within
/// the source named `source_name`.  Looks up the source's index
/// once at spawn time; threads then keep the index for the rest of
/// their lifetime.
pub fn spawn_heartbeat_threads(
  preview: &Arc<PreviewState>,
  source_name: &str,
  hb_idx: usize,
) -> (thread::JoinHandle<()>, thread::JoinHandle<()>) {
  let source_idx = preview
    .sources
    .iter()
    .position(|s| s.name == source_name)
    .unwrap_or_else(|| {
      panic!("source {source_name:?} not found in PreviewState::sources")
    });
  let poll_handle = spawn_poll_thread(preview, source_idx, hb_idx);
  let play_handle = spawn_play_thread(preview, source_idx, hb_idx);
  (poll_handle, play_handle)
}

// ---------------------------------------------------------------------------
// Audio playback helpers
// ---------------------------------------------------------------------------

/// Resolve all notes for `(source_idx, hb_idx)` from the current
/// metric and transition config.  Shared by loop, oneshot, and
/// continuous playback.
fn resolve_notes(
  preview: &PreviewState,
  source_idx: usize,
  hb_idx: usize,
) -> Vec<ResolvedNote> {
  let source = source_at(preview, source_idx);
  let metric = {
    let hbs = source.heartbeats.read().unwrap_or_recover();
    hbs[hb_idx].metric.value() as f64
  };
  let note_configs = {
    let cfg = &source.heartbeat_configs.read().unwrap_or_recover()[hb_idx];
    cfg.notes.clone()
  };
  let lib = source.library.read().unwrap_or_recover();
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

/// Clone the effective_volume Shared for `(source_idx, hb_idx)`.
/// The clone shares the same underlying atomic, so updates via
/// `update_effective_volume` are immediately visible.
fn clone_effective_volume(
  preview: &PreviewState,
  source_idx: usize,
  hb_idx: usize,
) -> fundsp::shared::Shared {
  source_at(preview, source_idx)
    .heartbeats
    .read()
    .unwrap_or_recover()[hb_idx]
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
  source_idx: usize,
  hb_idx: usize,
  counter: &prometheus::IntCounter,
) {
  let source = source_at(preview, source_idx);

  // Wait for at least one valid note.
  let initial_notes = loop {
    if !running.load(Ordering::Relaxed) {
      return;
    }
    if source.heartbeat_configs.read().unwrap_or_recover()[hb_idx].playback
      != Playback::Continuous
    {
      return;
    }
    let notes = resolve_notes(preview, source_idx, hb_idx);
    if !notes.is_empty() {
      break notes;
    }
    thread::sleep(Duration::from_secs(1));
  };

  let crossfade_ms = {
    let cfg = &source.heartbeat_configs.read().unwrap_or_recover()[hb_idx];
    cfg.crossfade_ms
  };
  let smoothing = crossfade_ms / 1000.0;

  let pairs = notes_to_continuous_pairs(&initial_notes);
  let eff_vol = clone_effective_volume(preview, source_idx, hb_idx);
  preview.update_effective_volume(source, hb_idx);
  let (graph, mut all_controls, mut all_structural) =
    continuous_graph_with_notes(&pairs, smoothing, Some(&eff_vol));
  let mut note_count = pairs.len();
  let sid = play_mix.add(graph);
  counter.inc();
  let mut last_rebuild = Instant::now();

  while running.load(Ordering::Relaxed) {
    sleep_checking(running, Duration::from_millis(50));

    if !running.load(Ordering::Relaxed) {
      break;
    }

    if source.heartbeat_configs.read().unwrap_or_recover()[hb_idx].playback
      != Playback::Continuous
    {
      break;
    }

    let crossfade_ms = {
      let cfg = &source.heartbeat_configs.read().unwrap_or_recover()[hb_idx];
      cfg.crossfade_ms
    };

    let notes = resolve_notes(preview, source_idx, hb_idx);
    if notes.is_empty() {
      continue;
    }

    preview.update_effective_volume(source, hb_idx);
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
      last_rebuild = Instant::now();
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
      last_rebuild = Instant::now();
    }

    // Periodic rebuild to reset IIR filter state and prevent
    // long-running numerical drift (Moog ladder instability).
    if !needs_rebuild && last_rebuild.elapsed() >= CONTINUOUS_REBUILD_INTERVAL {
      info!(
        source = %source.name,
        heartbeat = hb_idx,
        elapsed_secs = last_rebuild.elapsed().as_secs(),
        "Periodic continuous graph rebuild to reset filter state"
      );
      let smoothing = crossfade_ms / 1000.0;
      let (graph, new_controls, new_structural) =
        continuous_graph_with_notes(&pairs, smoothing, Some(&eff_vol));
      let cf =
        ((crossfade_ms / 1000.0) * play_mix.sample_rate()).ceil() as usize;
      play_mix.replace(sid, graph, cf);
      all_controls = new_controls;
      all_structural = new_structural;
      last_rebuild = Instant::now();
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
  source_idx: usize,
  hb_idx: usize,
  counter: &prometheus::IntCounter,
) {
  let source = source_at(preview, source_idx);
  let notes = resolve_notes(preview, source_idx, hb_idx);
  if notes.is_empty() {
    thread::sleep(Duration::from_secs(1));
    return;
  }

  let eff_vol = clone_effective_volume(preview, source_idx, hb_idx);
  preview.update_effective_volume(source, hb_idx);
  let graph = heartbeat::heartbeat_graph_with_notes(&notes, Some(&eff_vol));
  let content_dur = heartbeat::heartbeat_notes_content_duration(&notes);
  let sid = play_mix.add(graph);
  counter.inc();

  sleep_checking(running, content_dur);

  while running.load(Ordering::Relaxed) {
    if source.heartbeat_configs.read().unwrap_or_recover()[hb_idx].playback
      != Playback::Loop
    {
      break;
    }

    let crossfade_ms = {
      let cfg = &source.heartbeat_configs.read().unwrap_or_recover()[hb_idx];
      cfg.crossfade_ms
    };
    let notes = resolve_notes(preview, source_idx, hb_idx);
    if notes.is_empty() {
      break;
    }

    preview.update_effective_volume(source, hb_idx);
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
  source_idx: usize,
  hb_idx: usize,
  wait_for_clock: bool,
  counter: &prometheus::IntCounter,
) {
  let source = source_at(preview, source_idx);
  let (cycle_secs, cycle_offset) = {
    let cfg = &source.heartbeat_configs.read().unwrap_or_recover()[hb_idx];
    (cfg.cycle_secs, cfg.cycle_offset_secs)
  };
  let notes = resolve_notes(preview, source_idx, hb_idx);
  if notes.is_empty() {
    thread::sleep(Duration::from_secs(1));
    return;
  }

  let eff_vol = clone_effective_volume(preview, source_idx, hb_idx);
  preview.update_effective_volume(source, hb_idx);
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
    let mut st =
      SupervisedThread::new(h, 0, "localhost".to_string(), 0, ThreadRole::Poll);
    for _ in 0..MAX_FAILURES_PER_THREAD {
      assert!(!st.record_failure(), "should not be exhausted yet");
    }
    assert_eq!(st.failures.len(), MAX_FAILURES_PER_THREAD);
  }

  #[test]
  fn supervised_thread_budget_exhaustion() {
    let h = thread::spawn(|| {});
    let mut st =
      SupervisedThread::new(h, 0, "localhost".to_string(), 0, ThreadRole::Play);
    for _ in 0..MAX_FAILURES_PER_THREAD {
      st.record_failure();
    }
    // One more should exhaust the budget.
    assert!(st.record_failure(), "budget should be exhausted");
  }

  #[test]
  fn supervised_thread_prunes_old_failures() {
    let h = thread::spawn(|| {});
    let mut st =
      SupervisedThread::new(h, 0, "localhost".to_string(), 0, ThreadRole::Poll);

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
