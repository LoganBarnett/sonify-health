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

/// Build the OS-appropriate shell invocation for a probe command.
///
/// Probe commands are shell one-liners rather than bare executables, so
/// they run through an interpreter instead of being exec'd directly.  On
/// Unix that is always POSIX `sh`.  On Windows it depends on how the
/// process was launched — see `in_posix_shell` — because the same native
/// binary is started both from a stock PowerShell session (the
/// `install.ps1` route, whose preset is authored in PowerShell) and from a
/// MinGW/MSYS2/Cygwin shell (the `install.sh` route, whose preset is
/// authored in POSIX `sh`).
#[cfg(not(windows))]
fn shell_command(command: &str) -> Command {
  let mut shell = Command::new("sh");
  shell.args(["-c", command]);
  shell
}

#[cfg(windows)]
fn shell_command(command: &str) -> Command {
  if in_posix_shell() {
    let mut shell = Command::new("sh");
    shell.args(["-c", command]);
    shell
  } else {
    // `-Command` runs the text as an expression rather than a script file,
    // so the default Restricted execution policy — which governs script
    // *files* on disk — does not apply, the same reason `install.ps1` can
    // be piped through `iex`.
    let mut shell = Command::new("powershell.exe");
    shell.args(["-NoProfile", "-NonInteractive", "-Command", command]);
    shell
  }
}

/// Whether this Windows process is running inside a POSIX shell layer (Git
/// Bash, MSYS2/MinGW, or Cygwin), in which case probe commands are POSIX
/// and must run through `sh` rather than PowerShell.
///
/// The launching environment is the proxy for which dialect the config was
/// authored in: `install.sh` recognises exactly the
/// `MINGW*`/`MSYS*`/`CYGWIN*` environments and lays down the POSIX preset
/// there, while `install.ps1` targets a stock PowerShell session and lays
/// down the PowerShell preset.  Git Bash and MSYS2/MinGW export `MSYSTEM`
/// (e.g. `MINGW64`); Cygwin does not, but every one of these layers — and
/// no stock PowerShell/cmd session — puts `sh` on `PATH`.  The environment
/// is fixed for the process lifetime, so the answer is computed once.
#[cfg(windows)]
fn in_posix_shell() -> bool {
  use std::sync::LazyLock;
  static POSIX_SHELL: LazyLock<bool> = LazyLock::new(|| {
    std::env::var_os("MSYSTEM").is_some()
      || std::env::var_os("PATH")
        .map(|path| {
          std::env::split_paths(&path)
            .any(|dir| dir.join("sh.exe").is_file() || dir.join("sh").is_file())
        })
        .unwrap_or(false)
  });
  *POSIX_SHELL
}

/// Run a probe command and return a normalized metric (0.0..=1.0)
/// along with any stderr the command produced.
pub fn run_probe(
  name: &str,
  command: &str,
  result_mode: &ResultMode,
) -> Result<ProbeOutput, ProbeError> {
  let output = shell_command(command).output().map_err(|source| {
    ProbeError::ProbeExecution {
      heartbeat: name.to_string(),
      source,
    }
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

// POSIX `sh` probe syntax, so these run wherever `shell_command` selects
// `sh`: every non-Windows host, and a Windows host inside a POSIX shell
// layer.  Gating them off Windows keeps a stock-Windows `cargo test` —
// which would route them through PowerShell — from failing on syntax it
// cannot parse.
#[cfg(all(test, not(windows)))]
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

// PowerShell mirrors of the probe contract, exercising the Windows branch
// of `shell_command`.  These are not run in CI — the Windows targets are
// cross-compiled and the `windowsSmoke` check runs the binary under wine,
// not `cargo test` — but they document the expected behavior and pass on a
// real Windows host.  Each skips when a POSIX layer is present, since
// `shell_command` would then route the PowerShell syntax through `sh`.
#[cfg(all(test, windows))]
mod windows_tests {
  use super::*;

  #[test]
  fn exit_code_zero_is_healthy() {
    if in_posix_shell() {
      return;
    }
    let out = run_probe("test", "exit 0", &ResultMode::ExitCode).unwrap();
    assert!((out.metric - 0.0).abs() < 0.001);
  }

  #[test]
  fn exit_code_nonzero_is_down() {
    if in_posix_shell() {
      return;
    }
    let out = run_probe("test", "exit 1", &ResultMode::ExitCode).unwrap();
    assert!((out.metric - 1.0).abs() < 0.001);
  }

  #[test]
  fn stdout_float_parsing() {
    if in_posix_shell() {
      return;
    }
    let out =
      run_probe("test", "Write-Output 0.75", &ResultMode::Stdout).unwrap();
    assert!((out.metric - 0.75).abs() < 0.001);
  }
}
