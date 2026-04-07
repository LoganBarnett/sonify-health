use crate::config::DaemonConfig;
use crate::metrics::Metrics;
use fundsp::prelude32::shared;
use sonify_health_lib::{
  audio::{AudioError, AudioOutput},
  check, drone, heartbeat,
  state::HeartbeatState,
  BoopSpec, DroneState, PentatonicScale, Voice,
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
  scale: &PentatonicScale,
  audio_device: Option<&str>,
  muted: Arc<AtomicBool>,
  running: Arc<AtomicBool>,
  metrics: Metrics,
) -> Result<(), DaemonError> {
  let boop_count = config.heartbeat_checks.len();
  let heartbeat_state = Arc::new(HeartbeatState::new(boop_count));
  let boop_specs =
    voice.boop_specs(scale, boop_count, heartbeat::TOTAL_BOOP_TIME);

  // Spawn a check thread for each heartbeat check.
  let check_handles: Vec<_> = config
    .heartbeat_checks
    .iter()
    .enumerate()
    .map(|(i, check_cfg)| {
      let cfg = check_cfg.clone();
      let st = Arc::clone(&heartbeat_state);
      let run = Arc::clone(&running);
      let m = metrics.clone();
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
              m.check_severity
                .with_label_values(&[&cfg.name])
                .set(severity as i64);
              m.check_runs
                .with_label_values(&[&cfg.name, &severity.to_string()])
                .inc();
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
          thread::sleep(interval);
        }
      })
    })
    .collect();

  // Drone layer: continuous audio driven by metric polls.
  let drone_state = Arc::new(DroneState::new(config.drone_metrics.len()));
  let mute_volume = shared(if muted.load(Ordering::Relaxed) {
    0.0
  } else {
    1.0
  });

  // Start a persistent audio stream for each drone metric.
  let _drone_outputs: Vec<AudioOutput> = config
    .drone_metrics
    .iter()
    .enumerate()
    .filter_map(|(i, cfg)| {
      let graph = drone::drone_graph_with_volume(
        voice,
        cfg.register,
        &drone_state.metrics[i],
        Some(&mute_volume),
      );
      match AudioOutput::play(graph, audio_device) {
        Ok(output) => {
          info!(metric = cfg.name, "Drone audio stream started");
          Some(output)
        }
        Err(e) => {
          warn!(
            metric = cfg.name,
            error = %e,
            "Failed to start drone audio stream"
          );
          None
        }
      }
    })
    .collect();

  // Spawn a poll thread for each drone metric.
  let drone_handles: Vec<_> = config
    .drone_metrics
    .iter()
    .enumerate()
    .map(|(i, drone_cfg)| {
      let cfg = drone_cfg.clone();
      let st = Arc::clone(&drone_state);
      let run = Arc::clone(&running);
      let m = metrics.clone();
      let interval = Duration::from_secs_f64(config.drone_poll_interval_secs);
      thread::spawn(move || {
        while run.load(Ordering::Relaxed) {
          match check::run_drone_poll(&cfg) {
            Ok(value) => {
              info!(metric = cfg.name, value, "Drone poll completed");
              st.set(i, value);
              m.drone_metric_value
                .with_label_values(&[&cfg.name])
                .set(value as f64);
              m.drone_polls.with_label_values(&[&cfg.name, "ok"]).inc();
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
        mute_volume.set_value(0.0);
      } else {
        info!("Audio unmuted via API");
        mute_volume.set_value(1.0);
      }
      was_muted = is_muted;
    }

    if !is_muted && !boop_specs.is_empty() {
      play_heartbeat(voice, &heartbeat_state, &boop_specs, audio_device)?;
      metrics.heartbeats_played.inc();
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
  for h in drone_handles {
    let _ = h.join();
  }

  info!("Daemon stopped");
  Ok(())
}

fn play_heartbeat(
  voice: &Voice,
  state: &HeartbeatState,
  specs: &[BoopSpec],
  audio_device: Option<&str>,
) -> Result<(), AudioError> {
  let severities: Vec<_> = state
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

  let graph = heartbeat::heartbeat_graph(voice, &severities, specs);
  AudioOutput::play_for(
    graph,
    heartbeat::heartbeat_duration(specs),
    audio_device,
  )
}

fn severity_from_shared(value: f32) -> sonify_health_lib::Severity {
  use sonify_health_lib::Severity;
  match value.round() as u8 {
    0 => Severity::Healthy,
    1 => Severity::Degraded,
    _ => Severity::Down,
  }
}
