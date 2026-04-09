use serde::Deserialize;
use std::process::Command;
use thiserror::Error;

/// How the daemon reads a check command's result.
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

/// Configuration for a single check.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckConfig {
  pub name: String,
  pub command: String,
  pub result_mode: ResultMode,
  /// Number of boops per phrase.
  pub boops: Option<usize>,
  /// Power-curve exponent for lo/hi patch interpolation.
  pub interp_curve: Option<f64>,
}

#[derive(Debug, Error)]
pub enum CheckError {
  #[error("Check '{name}' failed to execute: {source}")]
  CheckExecution {
    name: String,
    #[source]
    source: std::io::Error,
  },

  #[error("Check '{name}' produced invalid stdout: {output}")]
  CheckInvalidStdout { name: String, output: String },
}

/// Run a check command and return a normalized metric (0.0..=1.0).
pub fn run_check(config: &CheckConfig) -> Result<f32, CheckError> {
  let output = Command::new("sh")
    .args(["-c", &config.command])
    .output()
    .map_err(|source| CheckError::CheckExecution {
      name: config.name.clone(),
      source,
    })?;

  match config.result_mode {
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
        .map_err(|_| CheckError::CheckInvalidStdout {
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

  fn check(command: &str, mode: ResultMode) -> CheckConfig {
    CheckConfig {
      name: "test".into(),
      command: command.into(),
      result_mode: mode,
      boops: None,
      interp_curve: None,
    }
  }

  #[test]
  fn exit_code_severity_zero_is_healthy() {
    let val = run_check(&check("true", ResultMode::ExitCodeSeverity)).unwrap();
    assert!((val - 0.0).abs() < 0.001);
  }

  #[test]
  fn exit_code_severity_one_is_degraded() {
    let val =
      run_check(&check("exit 1", ResultMode::ExitCodeSeverity)).unwrap();
    assert!((val - 0.5).abs() < 0.001);
  }

  #[test]
  fn exit_code_severity_two_is_down() {
    let val =
      run_check(&check("exit 2", ResultMode::ExitCodeSeverity)).unwrap();
    assert!((val - 1.0).abs() < 0.001);
  }

  #[test]
  fn exit_code_severity_high_clamps_to_down() {
    let val =
      run_check(&check("exit 127", ResultMode::ExitCodeSeverity)).unwrap();
    assert!((val - 1.0).abs() < 0.001);
  }

  #[test]
  fn exit_code_maps_to_unit_range() {
    let val = run_check(&check("true", ResultMode::ExitCode)).unwrap();
    assert!((val - 0.0).abs() < 0.001);
  }

  #[test]
  fn stdout_float_parsing() {
    let val = run_check(&check("echo 0.75", ResultMode::Stdout)).unwrap();
    assert!((val - 0.75).abs() < 0.001);
  }

  #[test]
  fn stdout_clamps_to_unit_range() {
    let val = run_check(&check("echo 5.0", ResultMode::Stdout)).unwrap();
    assert!(val <= 1.0);
  }

  #[test]
  fn missing_binary_returns_down_for_severity_mode() {
    // The shell runs but the binary doesn't exist → non-zero exit.
    let val =
      run_check(&check("/nonexistent/binary", ResultMode::ExitCodeSeverity))
        .unwrap();
    assert!((val - 1.0).abs() < 0.001);
  }
}
