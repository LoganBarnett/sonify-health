use crate::config::DaemonConfig;
use sonify_health_lib::{
  audio::{AudioError, AudioOutput},
  check, heartbeat,
  state::HeartbeatState,
  Voice,
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

/// Run the daemon's main loop: spawn check threads, play
/// heartbeat boops at the configured time slot, and shut down
/// when `running` becomes false.
pub fn run_daemon(
  config: &DaemonConfig,
  voice: &Voice,
  muted: Arc<AtomicBool>,
  running: Arc<AtomicBool>,
) -> Result<(), DaemonError> {
  let state = Arc::new(HeartbeatState::default());

  // Spawn a check thread for each heartbeat check.
  let check_handles: Vec<_> = config
    .heartbeat_checks
    .iter()
    .enumerate()
    .map(|(i, check_cfg)| {
      let cfg = check_cfg.clone();
      let st = Arc::clone(&state);
      let run = Arc::clone(&running);
      let interval = Duration::from_secs_f64(config.timing.cycle_duration_secs);
      thread::spawn(move || {
        while run.load(Ordering::Relaxed) {
          match check::run_heartbeat_check(&cfg) {
            Ok(severity) => {
              info!(
                check = cfg.name,
                severity = %severity,
                "Heartbeat check completed"
              );
              st.set(i, severity);
            }
            Err(e) => {
              warn!(
                check = cfg.name,
                error = %e,
                "Heartbeat check failed, \
                 retaining previous severity"
              );
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
    "Daemon started"
  );

  // Track previous mute state to log transitions.
  let mut was_muted = muted.load(Ordering::Relaxed);

  // Main timing loop: wait for slot, play heartbeat.
  while running.load(Ordering::Relaxed) {
    let wait = config.timing.duration_until_next_slot();
    if wait > Duration::ZERO {
      // Sleep in small increments so we can respond to
      // shutdown promptly.
      let deadline = std::time::Instant::now() + wait;
      while std::time::Instant::now() < deadline
        && running.load(Ordering::Relaxed)
      {
        thread::sleep(Duration::from_millis(100));
      }
      if !running.load(Ordering::Relaxed) {
        break;
      }
    }

    let is_muted = muted.load(Ordering::Relaxed);
    if is_muted != was_muted {
      if is_muted {
        info!("Audio muted via API");
      } else {
        info!("Audio unmuted via API");
      }
      was_muted = is_muted;
    }

    if !is_muted {
      play_heartbeat(voice, &state)?;
    }

    // Sleep through the rest of the slot to avoid
    // re-triggering.
    let remaining = config
      .timing
      .duration_until_next_slot()
      .max(Duration::from_millis(500));
    let end = std::time::Instant::now() + remaining;
    while std::time::Instant::now() < end && running.load(Ordering::Relaxed) {
      thread::sleep(Duration::from_millis(100));
    }
  }

  info!("Waiting for check threads to finish");
  for h in check_handles {
    let _ = h.join();
  }

  info!("Daemon stopped");
  Ok(())
}

fn play_heartbeat(
  voice: &Voice,
  state: &HeartbeatState,
) -> Result<(), AudioError> {
  // Read current severity values from shared state.
  let severities = [
    severity_from_shared(state.boops[0].value()),
    severity_from_shared(state.boops[1].value()),
    severity_from_shared(state.boops[2].value()),
  ];

  info!(
    s0 = %severities[0],
    s1 = %severities[1],
    s2 = %severities[2],
    "Playing heartbeat"
  );

  let durations = heartbeat::boop_durations(voice);
  let gap = Duration::from_millis(100);

  for (i, &severity) in severities.iter().enumerate() {
    let dur = durations[i];
    let graph = heartbeat::boop_graph(voice, severity, dur);
    AudioOutput::play_for(graph, Duration::from_secs_f64(dur + 0.05))?;
    if i < 2 {
      thread::sleep(gap);
    }
  }

  Ok(())
}

fn severity_from_shared(value: f32) -> sonify_health_lib::Severity {
  use sonify_health_lib::Severity;
  match value.round() as u8 {
    0 => Severity::Healthy,
    1 => Severity::Degraded,
    _ => Severity::Down,
  }
}
