use serde::{Deserialize, Serialize};
use std::process::Command;
use thiserror::Error;

/// How the daemon reads a probe command's result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ResultMode {
  /// Exit code 0 maps to 0.0 (healthy), non-zero maps to 1.0 (down).
  ExitCode,
  /// Command prints a float (0.0–1.0) to stdout.
  Stdout,
}

#[derive(Debug, Error)]
pub enum ProbeError {
  #[error("Probe '{heartbeat}' failed to execute: {source}")]
  ProbeExecution {
    heartbeat: String,
    #[source]
    source: std::io::Error,
  },

  #[error("Probe '{heartbeat}' killed by signal (no exit code)")]
  ProbeSignaled { heartbeat: String },

  #[error("Probe '{heartbeat}' produced invalid stdout: {output}")]
  ProbeInvalidStdout { heartbeat: String, output: String },
}

/// Run a probe command and return a normalized metric (0.0..=1.0).
pub fn run_probe(
  name: &str,
  command: &str,
  result_mode: &ResultMode,
) -> Result<f32, ProbeError> {
  let output =
    Command::new("sh")
      .args(["-c", command])
      .output()
      .map_err(|source| ProbeError::ProbeExecution {
        heartbeat: name.to_string(),
        source,
      })?;

  match result_mode {
    ResultMode::ExitCode => {
      let code =
        output
          .status
          .code()
          .ok_or_else(|| ProbeError::ProbeSignaled {
            heartbeat: name.to_string(),
          })?;
      Ok(if code == 0 { 0.0 } else { 1.0 })
    }
    ResultMode::Stdout => {
      let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
      text
        .parse::<f32>()
        .map_err(|_| ProbeError::ProbeInvalidStdout {
          heartbeat: name.to_string(),
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
    let val = run_probe("test", "true", &ResultMode::ExitCode).unwrap();
    assert!((val - 0.0).abs() < 0.001);
  }

  #[test]
  fn exit_code_nonzero_is_down() {
    let val = run_probe("test", "exit 1", &ResultMode::ExitCode).unwrap();
    assert!((val - 1.0).abs() < 0.001);
  }

  #[test]
  fn exit_code_high_is_down() {
    let val = run_probe("test", "exit 127", &ResultMode::ExitCode).unwrap();
    assert!((val - 1.0).abs() < 0.001);
  }

  #[test]
  fn exit_code_missing_binary_is_down() {
    let val =
      run_probe("test", "/nonexistent/binary", &ResultMode::ExitCode).unwrap();
    assert!((val - 1.0).abs() < 0.001);
  }

  #[test]
  fn stdout_float_parsing() {
    let val = run_probe("test", "echo 0.75", &ResultMode::Stdout).unwrap();
    assert!((val - 0.75).abs() < 0.001);
  }

  #[test]
  fn stdout_clamps_to_unit_range() {
    let val = run_probe("test", "echo 5.0", &ResultMode::Stdout).unwrap();
    assert!(val <= 1.0);
  }
}
