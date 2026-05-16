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
/// audio engine gives up and shuts down.
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
pub enum AudioEngineError {
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
  pub source_name: String,
  pub hb_idx: usize,
  pub role: ThreadRole,
  pub failures: Vec<Instant>,
}

impl SupervisedThread {
  fn new(
    handle: thread::JoinHandle<()>,
    source_name: String,
    hb_idx: usize,
    role: ThreadRole,
  ) -> Self {
    Self {
      handle: Some(handle),
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
//
// `option_if_let_else` would suggest collapsing the &str/String/Other
// chain into a nested `map_or_else`, which obscures the linear "try
// these types in order" logic.  The if/else-if/else chain reads
// straight down; keep it.
#[allow(clippy::option_if_let_else)]
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
// AudioEngineContext + run_audio_engine
// ---------------------------------------------------------------------------

/// Everything the audio engine thread needs from main.
pub struct AudioEngineContext<'a> {
  pub audio_device: Option<&'a str>,
  /// When true, the audio engine does not open an audio device and does
  /// not spawn play threads.  Poll threads, supervision, and the
  /// rest of the run-loop continue normally.
  pub headless: bool,
  pub preview: Arc<PreviewState>,
}

/// Exponential-backoff state for re-establishing a failed audio
/// stream.  Carried separately from the rest of the engine so the
/// stream-recovery tick has one cohesive piece of state to mutate
/// and reset.
struct AudioRecoveryState {
  backoff: Duration,
  next_at: Option<Instant>,
  attempts: u64,
}

impl AudioRecoveryState {
  /// Cap on exponential backoff between recovery attempts.
  const MAX_BACKOFF: Duration = Duration::from_secs(60);

  fn new() -> Self {
    Self {
      backoff: Duration::from_secs(1),
      next_at: None,
      attempts: 0,
    }
  }

  /// True when enough time has elapsed since the last failed
  /// attempt that another try is warranted.
  fn should_try(&self) -> bool {
    self.next_at.is_none_or(|t| Instant::now() >= t)
  }

  /// Reset to "stream healthy" after a successful recovery.
  fn on_success(&mut self) {
    self.backoff = Duration::from_secs(1);
    self.next_at = None;
  }

  /// Schedule the next retry, doubling the backoff up to the cap.
  fn on_failure(&mut self) {
    self.next_at = Some(Instant::now() + self.backoff);
    self.backoff = (self.backoff * 2).min(Self::MAX_BACKOFF);
  }
}

/// The audio engine: owns the mixer (when not headless), the
/// supervised worker thread handles, and the loop-local state for
/// stream recovery / mute tracking / tick counting.  Constructed
/// once via `new`, driven via `run`, torn down via `shutdown`
/// (which `run` calls on every exit path).
pub struct AudioEngine {
  preview: Arc<PreviewState>,
  mixer: Option<AudioMixer>,
  supervised: Vec<SupervisedThread>,
  audio_recovery: AudioRecoveryState,
  was_muted: bool,
  tick: u32,
}

impl AudioEngine {
  /// Build the engine: open the audio device (when not headless),
  /// spawn the initial poll/play threads for every (source,
  /// heartbeat) pair, and log a startup summary.
  pub fn new(ctx: AudioEngineContext<'_>) -> Result<Self, AudioEngineError> {
    let AudioEngineContext {
      audio_device,
      headless,
      preview,
    } = ctx;

    // Headless instances skip the audio device entirely.  The mixer,
    // play threads, audio recovery, and audio health logging are all
    // gated on this `Option`; everything else (pollers, supervision,
    // the WebSocket-driven mute/volume hooks) runs unconditionally.
    let mixer = if headless {
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
    let initial_sources = preview.sources_snapshot();
    let source_count = initial_sources.len();
    // `Some(handle)` and `!headless` are equivalent invariants;
    // gating play-thread spawns on `if let Some(...)` lets the type
    // system carry the "non-headless implies handle exists" claim
    // instead of a separate `if !headless` check followed by an
    // `.expect()`.
    let mixer_handle = mixer.as_ref().map(AudioMixer::handle);
    for source in &initial_sources {
      let hb_count = source.heartbeats.read().len();
      total_heartbeats += hb_count;
      for hb_idx in 0..hb_count {
        // Poll threads run only for Local sources — remote
        // heartbeats are probed by the remote instance and arrive
        // over the wire.
        if source.kind.is_local() {
          let poll_h = spawn_poll_thread(&preview, source, hb_idx);
          supervised.push(SupervisedThread::new(
            poll_h,
            source.name.clone(),
            hb_idx,
            ThreadRole::Poll,
          ));
        }
        if let Some(handle) = mixer_handle.as_ref() {
          let play_h =
            spawn_play_thread(&preview, source, hb_idx, handle.clone());
          supervised.push(SupervisedThread::new(
            play_h,
            source.name.clone(),
            hb_idx,
            ThreadRole::Play,
          ));
        }
      }
    }

    info!(
      sources = source_count,
      heartbeats = total_heartbeats,
      headless,
      "Audio engine started"
    );

    let was_muted = preview.muted.load(Ordering::Relaxed);
    Ok(Self {
      preview,
      mixer,
      supervised,
      audio_recovery: AudioRecoveryState::new(),
      was_muted,
      tick: 0,
    })
  }

  /// Drive the loop until `preview.running` becomes false or a
  /// thread exhausts its failure budget.  Always tears the engine
  /// down (`shutdown`) before returning, regardless of which exit
  /// path was taken.
  pub fn run(mut self) -> Result<(), AudioEngineError> {
    let result = self.run_loop();
    self.shutdown();
    result
  }

  fn run_loop(&mut self) -> Result<(), AudioEngineError> {
    while self.preview.running.load(Ordering::Relaxed) {
      self.mute_tick();
      if self.tick.is_multiple_of(SUPERVISION_CHECK_INTERVAL) {
        self.supervision_tick()?;
        self.rebalance_play_threads();
        self.stream_recovery_tick();
      }
      if self.tick > 0 && self.tick.is_multiple_of(HEALTH_LOG_INTERVAL) {
        self.health_log_tick();
      }
      self.tick = self.tick.wrapping_add(1);
      thread::sleep(Duration::from_millis(100));
    }
    Ok(())
  }

  /// `Some` iff the engine is non-headless and a mixer is open.
  /// Play threads need this to schedule audio.
  fn mixer_handle(&self) -> Option<MixerHandle> {
    self.mixer.as_ref().map(AudioMixer::handle)
  }

  /// Detect a mute transition and propagate volume updates.
  fn mute_tick(&mut self) {
    let is_muted = self.preview.muted.load(Ordering::Relaxed);
    if is_muted == self.was_muted {
      return;
    }
    if is_muted {
      info!("Audio muted via API");
    } else {
      info!("Audio unmuted via API");
    }
    self.preview.update_all_effective_volumes();
    self.was_muted = is_muted;
  }

  /// Reap finished threads, count failures, and either respawn or
  /// return `Err(ThreadBudgetExhausted)` when a thread crashes too
  /// often.  Called at the supervision cadence (~1 s).
  fn supervision_tick(&mut self) -> Result<(), AudioEngineError> {
    let mixer_handle = self.mixer_handle();
    for st in self.supervised.iter_mut() {
      // Combine the "is this thread finished?" check and the
      // `take()` into a single conditional take so the borrow
      // checker carries the invariant — no separate predicate
      // followed by an `.unwrap()` that could rot under
      // refactoring.
      let Some(handle) = st.handle.take_if(|h| h.is_finished()) else {
        continue;
      };
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
            return Err(AudioEngineError::ThreadBudgetExhausted {
              source_name: st.source_name.clone(),
              heartbeat: st.hb_idx,
              role: st.role.to_string(),
            });
          }

          // Respawn.  Resolve the source by name first — if it has
          // been removed in the interim, log and skip instead of
          // asserting.  For play threads, the same structural rule
          // as initial spawn applies: a handle exists IFF we are
          // not headless.
          let Some(source) = self.preview.source_by_name(&st.source_name)
          else {
            warn!(
              source = %st.source_name,
              heartbeat = st.hb_idx,
              role = %st.role,
              "Cannot respawn thread: source no longer exists",
            );
            continue;
          };
          info!(
            source = %st.source_name,
            heartbeat = st.hb_idx,
            role = %st.role,
            recent_failures = st.failures.len(),
            "Respawning thread"
          );
          let new_handle = match (&st.role, mixer_handle.as_ref()) {
            (ThreadRole::Poll, _) => {
              spawn_poll_thread(&self.preview, &source, st.hb_idx)
            }
            (ThreadRole::Play, Some(handle)) => spawn_play_thread(
              &self.preview,
              &source,
              st.hb_idx,
              handle.clone(),
            ),
            (ThreadRole::Play, None) => {
              // Should be unreachable: Play roles only enter
              // `supervised` when a mixer handle exists, and the
              // handle is set-once at audio-engine start.  Log
              // structurally rather than panic so the bug — if it
              // ever happens — surfaces in operator logs.
              error!(
                source = %st.source_name,
                heartbeat = st.hb_idx,
                "Cannot respawn play thread without mixer handle; \
                 this is an audio-engine-startup invariant violation",
              );
              continue;
            }
          };
          st.handle = Some(new_handle);
        }
      }
    }
    Ok(())
  }

  /// Ensure every (source, heartbeat) pair has a live play thread.
  /// Catches Remote Sources whose heartbeats arrive after startup
  /// and respawns play threads that exited cleanly because a
  /// heartbeat was removed and then re-added.  No-op in headless
  /// mode (no mixer means no play threads).
  fn rebalance_play_threads(&mut self) {
    let Some(play_mix) = self.mixer_handle() else {
      return;
    };

    // Build the desired set of (Arc<Source>, hb_idx) play targets.
    // Carrying the Arc<Source> rather than just the name lets the
    // spawn call below skip a re-lookup that would otherwise have
    // to assert the source still exists.
    let snapshot = self.preview.sources_snapshot();
    let desired: Vec<(Arc<Source>, usize)> = snapshot
      .iter()
      .flat_map(|source| {
        let s = Arc::clone(source);
        let hb_count = s.heartbeats.read().len();
        (0..hb_count).map(move |hb_idx| (Arc::clone(&s), hb_idx))
      })
      .collect();

    // Drop play-thread entries whose Source no longer exists.  The
    // threads themselves see `kind.is_alive() == false` and exit,
    // but their `SupervisedThread` slots would otherwise pile up
    // forever.
    self.supervised.retain(|st| {
      if st.role != ThreadRole::Play {
        return true;
      }
      desired
        .iter()
        .any(|(source, _)| source.name == st.source_name)
    });

    for (source, hb_idx) in &desired {
      let live = self.supervised.iter().any(|st| {
        st.role == ThreadRole::Play
          && st.source_name == source.name
          && st.hb_idx == *hb_idx
          && st.handle.as_ref().is_some_and(|h| !h.is_finished())
      });
      if live {
        continue;
      }
      let play_h =
        spawn_play_thread(&self.preview, source, *hb_idx, play_mix.clone());
      let existing = self.supervised.iter_mut().find(|st| {
        st.role == ThreadRole::Play
          && st.source_name == source.name
          && st.hb_idx == *hb_idx
      });
      match existing {
        Some(st) => {
          st.handle = Some(play_h);
        }
        None => {
          self.supervised.push(SupervisedThread::new(
            play_h,
            source.name.clone(),
            *hb_idx,
            ThreadRole::Play,
          ));
        }
      }
    }
  }

  /// Attempt to rebuild the audio stream after a failure, with
  /// exponential backoff between tries.  No-op in headless mode or
  /// when the stream is healthy.
  fn stream_recovery_tick(&mut self) {
    let Some(mixer) = self.mixer.as_mut() else {
      return;
    };
    if !mixer.stream_failed() {
      return;
    }
    if !self.audio_recovery.should_try() {
      return;
    }
    self.audio_recovery.attempts += 1;
    self
      .preview
      .metrics
      .audio_recovery_attempts
      .set(self.audio_recovery.attempts as i64);
    match mixer.try_recover() {
      Ok(()) => {
        info!(
          attempts = self.audio_recovery.attempts,
          "Audio stream recovered successfully"
        );
        self.audio_recovery.on_success();
      }
      Err(e) => {
        error!(
          error = %e,
          next_retry_secs = self.audio_recovery.backoff.as_secs(),
          attempts = self.audio_recovery.attempts,
          "Audio stream recovery failed"
        );
        self.audio_recovery.on_failure();
      }
    }
  }

  /// Pull current mixer stats and push them to the Prometheus
  /// metrics + a warning/debug log line.  No-op in headless mode.
  fn health_log_tick(&mut self) {
    let Some(mixer) = self.mixer.as_mut() else {
      return;
    };
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

    let metrics = &self.preview.metrics;
    metrics.audio_lock_failures.set(lock_fail as i64);
    metrics.audio_nan_frames.set(nan as i64);
    metrics.audio_peak_callback_us.set(peak_us as i64);
    metrics.audio_stream_errors.set(stream_errs as i64);
    metrics.audio_stream_failed.set(i64::from(stream_fail));
    mixer.reset_peak_callback_us();

    metrics.audio_output_peak_amplitude.set(out_peak as f64);
    for i in 0..MAX_MIXER_SLOTS {
      let label = i.to_string();
      metrics
        .audio_slot_peak_amplitude
        .with_label_values(&[&label])
        .set(slot_peaks[i] as f64);
      metrics
        .audio_slot_rms_amplitude
        .with_label_values(&[&label])
        .set(slot_rms[i] as f64);
    }
    metrics
      .audio_callback_buffer_frames_min
      .set(if buf_min == u32::MAX {
        0
      } else {
        buf_min as i64
      });
    metrics.audio_callback_buffer_frames_max.set(buf_max as i64);

    // Format non-zero slot amplitudes as "0:0.123/0.089 1:0.456/0.234".
    let slot_amplitudes: String = (0..MAX_MIXER_SLOTS)
      .filter(|&i| slot_peaks[i] > 0.0)
      .map(|i| format!("{}:{:.4}/{:.4}", i, slot_peaks[i], slot_rms[i]))
      .collect::<Vec<_>>()
      .join(" ");

    let clipping = out_peak > 1.0;
    let period_renego =
      buf_min != buf_max && buf_min != u32::MAX && buf_max != 0;
    let buf_min_logged = if buf_min == u32::MAX { 0 } else { buf_min };

    if lock_fail > 0 || nan > 0 || stream_fail || clipping || period_renego {
      warn!(
        lock_failures = lock_fail,
        nan_frames = nan,
        peak_callback_us = peak_us,
        stream_errors = stream_errs,
        stream_failed = stream_fail,
        output_peak = out_peak,
        slot_amplitudes = slot_amplitudes.as_str(),
        callback_buffer_min = buf_min_logged,
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
        callback_buffer_min = buf_min_logged,
        callback_buffer_max = buf_max,
        "Audio health"
      );
    }
  }

  /// Join every supervised thread and clear the mixer.  `run` calls
  /// this on every exit path so the engine never leaves dangling
  /// threads or a live audio stream behind.
  fn shutdown(&mut self) {
    self.preview.running.store(false, Ordering::Relaxed);
    info!("Waiting for heartbeat threads to finish");
    for st in self.supervised.iter_mut() {
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
    if let Some(mixer) = self.mixer.as_mut() {
      mixer.clear();
    }
    info!("Audio engine stopped");
  }
}

/// Construct an `AudioEngine` from the given context and run it to
/// completion.  Convenience wrapper for callers that don't need to
/// hold the engine to mutate it from outside the loop.
pub fn run_audio_engine(
  ctx: AudioEngineContext<'_>,
) -> Result<(), AudioEngineError> {
  AudioEngine::new(ctx)?.run()
}

// ---------------------------------------------------------------------------
// Thread spawning
// ---------------------------------------------------------------------------

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

/// Spawn a poll thread for the heartbeat at `hb_idx` within the
/// source named `source_name`.  Captures an `Arc<Source>` at spawn
/// time; the thread checks `kind.is_alive()` each iteration and
/// exits cleanly when the Source is removed at runtime.  Re-reads
/// the heartbeat config on every iteration so live UI edits take
/// effect.
pub fn spawn_poll_thread(
  preview: &Arc<PreviewState>,
  source: &Arc<Source>,
  hb_idx: usize,
) -> thread::JoinHandle<()> {
  let poll_running = Arc::clone(&preview.running);
  let poll_preview = Arc::clone(preview);
  let source = Arc::clone(source);
  let source_name = source.name.clone();
  let cfg = source.heartbeat_configs.read()[hb_idx].clone();
  let poll_counter = preview
    .metrics
    .probes_completed
    .with_label_values(&[&cfg.name]);
  let poll_gauge = preview.metrics.probe_value.with_label_values(&[&cfg.name]);

  thread::spawn(move || {
    while poll_running.load(Ordering::Relaxed) {
      if !source.kind.is_alive() {
        return;
      }
      let (cfg_name, command, mode, tiers, interval) = {
        let configs = source.heartbeat_configs.read();
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
        let hbs = source.heartbeats.read();
        let val = *hbs[hb_idx].override_value.read();
        val
      };

      // option_if_let_else would fold this 60-line override/probe
      // branch into a single map_or_else call, which destroys the
      // linear "either we have an override OR we run the probe"
      // structure.  Keep the if/else.
      #[allow(clippy::option_if_let_else)]
      let resolved = if let Some(metric) = overridden {
        let clamped = metric.clamp(0.0, 1.0);
        {
          let hbs = source.heartbeats.read();
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
              let hbs = source.heartbeats.read();
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
        let hbs = source.heartbeats.read();
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

/// Spawn a play thread for the heartbeat at `hb_idx` within the
/// source named `source_name`.  Captures an `Arc<Source>` at spawn
/// time; the thread checks `kind.is_alive()` each iteration so a
/// runtime-removed Source's play threads exit cleanly.  Re-reads
/// the heartbeat config on every iteration so live edits and remote
/// state mirror updates take effect.
pub fn spawn_play_thread(
  preview: &Arc<PreviewState>,
  source: &Arc<Source>,
  hb_idx: usize,
  play_mix: MixerHandle,
) -> thread::JoinHandle<()> {
  let play_running = Arc::clone(&preview.running);
  let play_preview = Arc::clone(preview);
  let source = Arc::clone(source);
  let cfg_name = source.heartbeat_configs.read()[hb_idx].name.clone();
  let play_counter = preview
    .metrics
    .heartbeats_played
    .with_label_values(&[&cfg_name]);

  thread::spawn(move || {
    // Align to the wall-clock grid before the first play so that
    // clock-mode heartbeats with different offsets start staggered.
    // Loop and Continuous modes skip this to start playing immediately.
    {
      let configs = source.heartbeat_configs.read();
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
      // Exit cleanly when the Source is removed at runtime.
      if !source.kind.is_alive() {
        return;
      }
      // Exit cleanly when the heartbeat is removed (e.g. a remote
      // shape change that shrinks the list past `hb_idx`).
      let mode = match source.heartbeat_configs.read().get(hb_idx) {
        Some(cfg) => cfg.playback,
        None => return,
      };

      // For Remote sources, idle while the user has playback off.
      // The thread stays alive; it just doesn't produce audio.
      if !source_should_play(&source) {
        sleep_checking(&play_running, Duration::from_millis(200));
        continue;
      }

      match mode {
        Playback::Continuous => play_continuous_tick(
          &play_running,
          &play_preview,
          &source,
          &play_mix,
          hb_idx,
          &play_counter,
        ),
        Playback::Loop => play_loop(
          &play_running,
          &play_preview,
          &source,
          &play_mix,
          hb_idx,
          &play_counter,
        ),
        Playback::Clock => play_oneshot_once(
          &play_running,
          &play_preview,
          &source,
          &play_mix,
          hb_idx,
          true,
          &play_counter,
        ),
      }
    }
  })
}

/// Spawn poll and (conditionally) play threads for the heartbeat
/// at `hb_idx` within `source`.  Used by the runtime
/// `add_heartbeat` path to wire fresh threads for a newly-added
/// Local heartbeat.  The play thread is only spawned when
/// `play_mix` is `Some`; in headless deployments the caller
/// passes `None` and only the poll thread runs.
pub fn spawn_heartbeat_threads(
  preview: &Arc<PreviewState>,
  source: &Arc<Source>,
  hb_idx: usize,
  play_mix: Option<MixerHandle>,
) -> (thread::JoinHandle<()>, Option<thread::JoinHandle<()>>) {
  let poll_handle = spawn_poll_thread(preview, source, hb_idx);
  let play_handle =
    play_mix.map(|h| spawn_play_thread(preview, source, hb_idx, h));
  (poll_handle, play_handle)
}

// ---------------------------------------------------------------------------
// Audio playback helpers
// ---------------------------------------------------------------------------

/// Resolve all notes for the heartbeat at `hb_idx` within
/// `source`.  Returns an empty Vec when `hb_idx` is out of range
/// (e.g. a remote shape change shrunk the list past `hb_idx`).
fn resolve_notes(source: &Source, hb_idx: usize) -> Vec<ResolvedNote> {
  let metric = {
    let hbs = source.heartbeats.read();
    match hbs.get(hb_idx) {
      Some(hb) => hb.metric.value() as f64,
      None => return vec![],
    }
  };
  let note_configs = {
    let configs = source.heartbeat_configs.read();
    match configs.get(hb_idx) {
      Some(cfg) => cfg.notes.clone(),
      None => return vec![],
    }
  };
  let lib = source.library.read();
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

/// Clone the effective_volume Shared for the heartbeat at `hb_idx`
/// within `source`.  The clone shares the same underlying atomic,
/// so updates via `update_effective_volume` are immediately visible.
/// Returns a default `Shared(1.0)` if `hb_idx` is out of range —
/// callers in that case will see no notes to resolve and exit.
fn clone_effective_volume(
  source: &Source,
  hb_idx: usize,
) -> fundsp::shared::Shared {
  source.heartbeats.read().get(hb_idx).map_or_else(
    || fundsp::prelude32::shared(1.0),
    |hb| hb.effective_volume.clone(),
  )
}

/// Continuous morph playback: build a multi-note graph with
/// `Shared` controls, then update those controls as the metric
/// changes.  Rebuilds the graph when structural parameters or
/// note count change.  Returns when the audio engine is shutting down
/// or the playback mode changes away from `Continuous`.
fn play_continuous_tick(
  running: &AtomicBool,
  preview: &PreviewState,
  source: &Source,
  play_mix: &MixerHandle,
  hb_idx: usize,
  counter: &prometheus::IntCounter,
) {
  // Wait for at least one valid note.
  let initial_notes = loop {
    if !running.load(Ordering::Relaxed) {
      return;
    }
    let configs = source.heartbeat_configs.read();
    let Some(cfg) = configs.get(hb_idx) else {
      return;
    };
    if cfg.playback != Playback::Continuous {
      return;
    }
    drop(configs);
    let notes = resolve_notes(source, hb_idx);
    if !notes.is_empty() {
      break notes;
    }
    thread::sleep(Duration::from_secs(1));
  };

  let crossfade_ms = match source.heartbeat_configs.read().get(hb_idx) {
    Some(cfg) => cfg.crossfade_ms,
    None => return,
  };
  let smoothing = crossfade_ms / 1000.0;

  let pairs = notes_to_continuous_pairs(&initial_notes);
  let eff_vol = clone_effective_volume(source, hb_idx);
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

    let crossfade_ms = match source.heartbeat_configs.read().get(hb_idx) {
      Some(cfg) if cfg.playback == Playback::Continuous => cfg.crossfade_ms,
      Some(_) => break,
      None => break,
    };

    let notes = resolve_notes(source, hb_idx);
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
/// Returns when the audio engine is shutting down or the playback mode
/// changes away from `Loop`.
fn play_loop(
  running: &AtomicBool,
  preview: &PreviewState,
  source: &Source,
  play_mix: &MixerHandle,
  hb_idx: usize,
  counter: &prometheus::IntCounter,
) {
  let notes = resolve_notes(source, hb_idx);
  if notes.is_empty() {
    thread::sleep(Duration::from_secs(1));
    return;
  }

  let eff_vol = clone_effective_volume(source, hb_idx);
  preview.update_effective_volume(source, hb_idx);
  let graph = heartbeat::heartbeat_graph_with_notes(&notes, Some(&eff_vol));
  let content_dur = heartbeat::heartbeat_notes_content_duration(&notes);
  let sid = play_mix.add(graph);
  counter.inc();

  sleep_checking(running, content_dur);

  while running.load(Ordering::Relaxed) {
    let crossfade_ms = match source.heartbeat_configs.read().get(hb_idx) {
      Some(cfg) if cfg.playback == Playback::Loop => cfg.crossfade_ms,
      _ => break,
    };
    let notes = resolve_notes(source, hb_idx);
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
  source: &Source,
  play_mix: &MixerHandle,
  hb_idx: usize,
  wait_for_clock: bool,
  counter: &prometheus::IntCounter,
) {
  let (cycle_secs, cycle_offset) =
    match source.heartbeat_configs.read().get(hb_idx) {
      Some(cfg) => (cfg.cycle_secs, cfg.cycle_offset_secs),
      None => return,
    };
  let notes = resolve_notes(source, hb_idx);
  if notes.is_empty() {
    thread::sleep(Duration::from_secs(1));
    return;
  }

  let eff_vol = clone_effective_volume(source, hb_idx);
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
      SupervisedThread::new(h, "localhost".to_string(), 0, ThreadRole::Poll);
    for _ in 0..MAX_FAILURES_PER_THREAD {
      assert!(!st.record_failure(), "should not be exhausted yet");
    }
    assert_eq!(st.failures.len(), MAX_FAILURES_PER_THREAD);
  }

  #[test]
  fn supervised_thread_budget_exhaustion() {
    let h = thread::spawn(|| {});
    let mut st =
      SupervisedThread::new(h, "localhost".to_string(), 0, ThreadRole::Play);
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
      SupervisedThread::new(h, "localhost".to_string(), 0, ThreadRole::Poll);

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
