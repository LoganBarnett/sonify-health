use serde::Deserialize;
use std::process::Command;
use thiserror::Error;

/// How the daemon reads a probe command's result.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ResultMode {
  /// Process exit code maps directly to 0.0–1.0 (code / 255).
  ExitCode,
  /// Command prints a float (0.0–1.0) to stdout.
  Stdout,
  /// Exit code maps to severity: 0→0.0, 1→0.5, 2→1.0.
  ExitCodeSeverity,
}

#[derive(Debug, Error)]
pub enum ProbeError {
  #[error("Probe '{name}' failed to execute: {source}")]
  ProbeExecution {
    name: String,
    #[source]
    source: std::io::Error,
  },

  #[error("Probe '{name}' produced invalid stdout: {output}")]
  ProbeInvalidStdout { name: String, output: String },
}

/// Run a probe command and return a normalized metric (0.0..=1.0).
pub fn run_probe(
  name: &str,
  command: &str,
  result_mode: &ResultMode,
) -> Result<f32, ProbeError> {
  let output = Command::new("sh")
    .args(["-c", command])
    .output()
    .map_err(|source| ProbeError::ProbeExecution {
      name: name.to_string(),
      source,
    })?;

  match result_mode {
    ResultMode::ExitCode => {
      let code = output.status.code().unwrap_or(0) as f32 / 255.0;
      Ok(code.clamp(0.0, 1.0))
    }
    ResultMode::ExitCodeSeverity => {
      let code = output.status.code().unwrap_or(2).min(2) as u8;
      Ok(match code {
        0 => 0.0,
        1 => 0.5,
        _ => 1.0,
      })
    }
    ResultMode::Stdout => {
      let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
      text
        .parse::<f32>()
        .map_err(|_| ProbeError::ProbeInvalidStdout {
          name: name.to_string(),
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
  fn exit_code_severity_zero_is_healthy() {
    let val =
      run_probe("test", "true", &ResultMode::ExitCodeSeverity).unwrap();
    assert!((val - 0.0).abs() < 0.001);
  }

  #[test]
  fn exit_code_severity_one_is_degraded() {
    let val =
      run_probe("test", "exit 1", &ResultMode::ExitCodeSeverity).unwrap();
    assert!((val - 0.5).abs() < 0.001);
  }

  #[test]
  fn exit_code_severity_two_is_down() {
    let val =
      run_probe("test", "exit 2", &ResultMode::ExitCodeSeverity).unwrap();
    assert!((val - 1.0).abs() < 0.001);
  }

  #[test]
  fn exit_code_severity_high_clamps_to_down() {
    let val =
      run_probe("test", "exit 127", &ResultMode::ExitCodeSeverity).unwrap();
    assert!((val - 1.0).abs() < 0.001);
  }

  #[test]
  fn exit_code_maps_to_unit_range() {
    let val = run_probe("test", "true", &ResultMode::ExitCode).unwrap();
    assert!((val - 0.0).abs() < 0.001);
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

  #[test]
  fn missing_binary_returns_down_for_severity_mode() {
    let val = run_probe(
      "test",
      "/nonexistent/binary",
      &ResultMode::ExitCodeSeverity,
    )
    .unwrap();
    assert!((val - 1.0).abs() < 0.001);
  }
}
