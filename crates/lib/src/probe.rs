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

/// Successful output from a probe command.
pub struct ProbeOutput {
  pub metric: f32,
  pub stderr: String,
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
  ProbeSignaled { heartbeat: String, stderr: String },

  #[error("Probe '{heartbeat}' produced invalid stdout: {output}")]
  ProbeInvalidStdout {
    heartbeat: String,
    output: String,
    stderr: String,
  },
}

/// Run a probe command and return a normalized metric (0.0..=1.0)
/// along with any stderr the command produced.
pub fn run_probe(
  name: &str,
  command: &str,
  result_mode: &ResultMode,
) -> Result<ProbeOutput, ProbeError> {
  let output =
    Command::new("sh")
      .args(["-c", command])
      .output()
      .map_err(|source| ProbeError::ProbeExecution {
        heartbeat: name.to_string(),
        source,
      })?;

  let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

  match result_mode {
    ResultMode::ExitCode => {
      let code =
        output
          .status
          .code()
          .ok_or_else(|| ProbeError::ProbeSignaled {
            heartbeat: name.to_string(),
            stderr: stderr.clone(),
          })?;
      Ok(ProbeOutput {
        metric: if code == 0 { 0.0 } else { 1.0 },
        stderr,
      })
    }
    ResultMode::Stdout => {
      let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
      let metric = text
        .parse::<f32>()
        .map_err(|_| ProbeError::ProbeInvalidStdout {
          heartbeat: name.to_string(),
          output: text,
          stderr: stderr.clone(),
        })?
        .clamp(0.0, 1.0);
      Ok(ProbeOutput { metric, stderr })
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn exit_code_zero_is_healthy() {
    let out = run_probe("test", "true", &ResultMode::ExitCode).unwrap();
    assert!((out.metric - 0.0).abs() < 0.001);
  }

  #[test]
  fn exit_code_nonzero_is_down() {
    let out = run_probe("test", "exit 1", &ResultMode::ExitCode).unwrap();
    assert!((out.metric - 1.0).abs() < 0.001);
  }

  #[test]
  fn exit_code_high_is_down() {
    let out = run_probe("test", "exit 127", &ResultMode::ExitCode).unwrap();
    assert!((out.metric - 1.0).abs() < 0.001);
  }

  #[test]
  fn exit_code_missing_binary_is_down() {
    let out =
      run_probe("test", "/nonexistent/binary", &ResultMode::ExitCode).unwrap();
    assert!((out.metric - 1.0).abs() < 0.001);
  }

  #[test]
  fn stdout_float_parsing() {
    let out = run_probe("test", "echo 0.75", &ResultMode::Stdout).unwrap();
    assert!((out.metric - 0.75).abs() < 0.001);
  }

  #[test]
  fn stdout_clamps_to_unit_range() {
    let out = run_probe("test", "echo 5.0", &ResultMode::Stdout).unwrap();
    assert!(out.metric <= 1.0);
  }

  #[test]
  fn stderr_is_captured() {
    let out =
      run_probe("test", "echo ok >&2; true", &ResultMode::ExitCode).unwrap();
    assert_eq!(out.stderr, "ok");
  }
}
