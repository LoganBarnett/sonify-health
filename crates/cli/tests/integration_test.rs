// Tests-only exemption from the workspace's no-unwrap policy.
// See workspace `[lints.clippy]` in the root Cargo.toml.
#![allow(
  clippy::unwrap_used,
  clippy::expect_used,
  clippy::panic,
  clippy::unreachable,
  clippy::todo,
  clippy::unimplemented
)]

use std::{path::PathBuf, process::Command};

fn binary_path() -> PathBuf {
  let mut path =
    std::env::current_exe().expect("Failed to get current executable path");
  path.pop(); // test executable name
  path.pop(); // deps dir
  path.push("sonify-health");

  if !path.exists() {
    path.pop();
    path.pop();
    path.push("debug");
    path.push("sonify-health");
  }
  path
}

#[test]
fn help_flag() {
  let output = Command::new(binary_path()).arg("--help").output();
  match output {
    Ok(output) => {
      assert!(
        output.status.success(),
        "Expected success, got: {:?}",
        output.status.code()
      );
      let stdout = String::from_utf8_lossy(&output.stdout);
      assert!(stdout.contains("Usage:"), "Expected help text, got: {}", stdout);
    }
    Err(e) => {
      if e.kind() == std::io::ErrorKind::NotFound {
        eprintln!(
          "Binary not found. Build first with: \
           cargo build -p sonify-health-cli"
        );
      }
      panic!("Failed to execute binary: {}", e);
    }
  }
}

#[test]
fn version_flag() {
  let output = Command::new(binary_path()).arg("--version").output();
  match output {
    Ok(output) => {
      assert!(output.status.success());
      let stdout = String::from_utf8_lossy(&output.stdout);
      assert!(
        stdout.contains("sonify-health"),
        "Expected version text, got: {}",
        stdout
      );
    }
    Err(e) => {
      panic!("Failed to execute binary: {}", e);
    }
  }
}

#[test]
fn print_toml_shows_library() {
  let output = Command::new(binary_path())
    .args(["print", "--format", "toml"])
    .output();
  match output {
    Ok(output) => {
      assert!(
        output.status.success(),
        "print subcommand failed: {}",
        String::from_utf8_lossy(&output.stderr)
      );
      let stdout = String::from_utf8_lossy(&output.stdout);
      assert!(
        stdout.contains("patches."),
        "Expected TOML patch output, got: {}",
        stdout
      );
    }
    Err(e) => {
      panic!("Failed to execute binary: {}", e);
    }
  }
}

#[test]
fn preview_unknown_patch_fails() {
  let output = Command::new(binary_path())
    .args(["preview", "--patch-name", "nonexistent-patch-xyz"])
    .output();
  match output {
    Ok(output) => {
      assert!(
        !output.status.success(),
        "preview with unknown patch should fail"
      );
    }
    Err(e) => {
      panic!("Failed to execute binary: {}", e);
    }
  }
}
