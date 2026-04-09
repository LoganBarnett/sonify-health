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
fn patch_subcommand() {
  let output = Command::new(binary_path()).arg("patch").output();
  match output {
    Ok(output) => {
      assert!(
        output.status.success(),
        "patch subcommand failed: {}",
        String::from_utf8_lossy(&output.stderr)
      );
      let stdout = String::from_utf8_lossy(&output.stdout);
      assert!(
        stdout.contains("freq:"),
        "Expected patch output, got: {}",
        stdout
      );
    }
    Err(e) => {
      panic!("Failed to execute binary: {}", e);
    }
  }
}

#[test]
fn patch_with_hostname_flag() {
  let output = Command::new(binary_path())
    .args(["patch", "--hostname", "silicon"])
    .output();
  match output {
    Ok(output) => {
      assert!(
        output.status.success(),
        "patch --hostname failed: {}",
        String::from_utf8_lossy(&output.stderr)
      );
      let stdout = String::from_utf8_lossy(&output.stdout);
      assert!(stdout.contains("silicon"));
    }
    Err(e) => {
      panic!("Failed to execute binary: {}", e);
    }
  }
}

#[test]
fn preview_requires_at_least_one_severity() {
  let output = Command::new(binary_path()).args(["preview"]).output();
  match output {
    Ok(output) => {
      assert!(!output.status.success(), "preview with 0 args should fail");
    }
    Err(e) => {
      panic!("Failed to execute binary: {}", e);
    }
  }
}

#[test]
fn preview_drone_requires_one_metric() {
  let output = Command::new(binary_path())
    .args(["preview", "--drone", "0.5", "0.8"])
    .output();
  match output {
    Ok(output) => {
      assert!(
        !output.status.success(),
        "drone preview with 2 values should fail"
      );
    }
    Err(e) => {
      panic!("Failed to execute binary: {}", e);
    }
  }
}
