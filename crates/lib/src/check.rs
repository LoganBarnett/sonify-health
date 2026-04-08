use crate::severity::Severity;
use serde::Deserialize;
use std::process::Command;
use thiserror::Error;

/// How the daemon reads a check command's result.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ResultMode {
  /// Process exit code maps directly to the value.
  ExitCode,
  /// Command prints the value to stdout.
  Stdout,
}

/// Configuration for a single heartbeat check (one boop).
#[derive(Debug, Clone, Deserialize)]
pub struct HeartbeatCheckConfig {
  pub name: String,
  pub command: String,
  pub result_mode: ResultMode,
}

/// Configuration for a single drone metric poll.
#[derive(Debug, Clone, Deserialize)]
pub struct DroneMetricConfig {
  pub name: String,
  pub command: String,
  pub result_mode: ResultMode,
  /// Override base frequency for note selection (Hz).
  pub base_freq: Option<f64>,
  /// Number of boops per drone phrase.
  pub boops: Option<usize>,
}

#[derive(Debug, Error)]
pub enum CheckError {
  #[error("Heartbeat check '{name}' failed to execute: {source}")]
  HeartbeatCheckExecution {
    name: String,
    #[source]
    source: std::io::Error,
  },

  #[error("Drone poll '{name}' failed to execute: {source}")]
  DronePollExecution {
    name: String,
    #[source]
    source: std::io::Error,
  },

  #[error("Drone poll '{name}' produced invalid stdout: {output}")]
  DronePollInvalidStdout { name: String, output: String },

  #[error(
    "Heartbeat check '{name}' produced invalid stdout \
     severity: {output}"
  )]
  HeartbeatCheckInvalidStdout { name: String, output: String },
}

/// Run a heartbeat check command and interpret the result as a
/// severity.
pub fn run_heartbeat_check(
  config: &HeartbeatCheckConfig,
) -> Result<Severity, CheckError> {
  let output = Command::new("sh")
    .args(["-c", &config.command])
    .output()
    .map_err(|source| CheckError::HeartbeatCheckExecution {
      name: config.name.clone(),
      source,
    })?;

  match config.result_mode {
    ResultMode::ExitCode => {
      let code = output.status.code().unwrap_or(2) as u8;
      Severity::try_from(code.min(2)).map_err(|e| {
        CheckError::HeartbeatCheckInvalidStdout {
          name: config.name.clone(),
          output: e.to_string(),
        }
      })
    }
    ResultMode::Stdout => {
      let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
      text.parse::<Severity>().map_err(|_| {
        CheckError::HeartbeatCheckInvalidStdout {
          name: config.name.clone(),
          output: text,
        }
      })
    }
  }
}

/// Run a drone poll command and interpret the result as a
/// normalized float (0.0..=1.0).
pub fn run_drone_poll(config: &DroneMetricConfig) -> Result<f32, CheckError> {
  let output = Command::new("sh")
    .args(["-c", &config.command])
    .output()
    .map_err(|source| CheckError::DronePollExecution {
      name: config.name.clone(),
      source,
    })?;

  match config.result_mode {
    ResultMode::ExitCode => {
      let code = output.status.code().unwrap_or(0) as f32 / 255.0;
      Ok(code.clamp(0.0, 1.0))
    }
    ResultMode::Stdout => {
      let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
      text
        .parse::<f32>()
        .map_err(|_| CheckError::DronePollInvalidStdout {
          name: config.name.clone(),
          output: text,
        })
        .map(|v| v.clamp(0.0, 1.0))
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn exit_code_zero_is_healthy() {
    let cfg = HeartbeatCheckConfig {
      name: "test".into(),
      command: "true".into(),
      result_mode: ResultMode::ExitCode,
    };
    assert_eq!(run_heartbeat_check(&cfg).unwrap(), Severity::Healthy);
  }

  #[test]
  fn exit_code_one_is_degraded() {
    let cfg = HeartbeatCheckConfig {
      name: "test".into(),
      command: "exit 1".into(),
      result_mode: ResultMode::ExitCode,
    };
    assert_eq!(run_heartbeat_check(&cfg).unwrap(), Severity::Degraded);
  }

  #[test]
  fn stdout_severity_parsing() {
    let cfg = HeartbeatCheckConfig {
      name: "test".into(),
      command: "echo 2".into(),
      result_mode: ResultMode::Stdout,
    };
    assert_eq!(run_heartbeat_check(&cfg).unwrap(), Severity::Down);
  }

  #[test]
  fn stdout_drone_poll() {
    let cfg = DroneMetricConfig {
      name: "test".into(),
      command: "echo 0.75".into(),
      result_mode: ResultMode::Stdout,
      base_freq: None,
      boops: None,
    };
    let val = run_drone_poll(&cfg).unwrap();
    assert!((val - 0.75).abs() < 0.001);
  }

  #[test]
  fn invalid_command_returns_error() {
    let cfg = HeartbeatCheckConfig {
      name: "bad".into(),
      command: "/nonexistent/binary".into(),
      result_mode: ResultMode::ExitCode,
    };
    // The shell itself will run, returning a non-zero exit
    // code for a missing binary.  This should map to Down.
    let result = run_heartbeat_check(&cfg).unwrap();
    assert_eq!(result, Severity::Down);
  }

  #[test]
  fn drone_clamps_to_unit_range() {
    let cfg = DroneMetricConfig {
      name: "test".into(),
      command: "echo 5.0".into(),
      result_mode: ResultMode::Stdout,
      base_freq: None,
      boops: None,
    };
    let val = run_drone_poll(&cfg).unwrap();
    assert!(val <= 1.0);
  }
}
