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
fn voice_subcommand() {
  let output = Command::new(binary_path()).arg("voice").output();
  match output {
    Ok(output) => {
      assert!(
        output.status.success(),
        "voice subcommand failed: {}",
        String::from_utf8_lossy(&output.stderr)
      );
      let stdout = String::from_utf8_lossy(&output.stdout);
      assert!(
        stdout.contains("base_freq:"),
        "Expected voice output, got: {}",
        stdout
      );
    }
    Err(e) => {
      panic!("Failed to execute binary: {}", e);
    }
  }
}

#[test]
fn voice_with_hostname_flag() {
  let output = Command::new(binary_path())
    .args(["voice", "--hostname", "silicon"])
    .output();
  match output {
    Ok(output) => {
      assert!(
        output.status.success(),
        "voice --hostname failed: {}",
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
fn preview_requires_three_severities() {
  let output = Command::new(binary_path())
    .args(["preview", "0", "0"])
    .output();
  match output {
    Ok(output) => {
      assert!(!output.status.success(), "preview with 2 args should fail");
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

#[test]
fn preview_drone_help_shows_register() {
  let output = Command::new(binary_path())
    .args(["preview", "--help"])
    .output();
  match output {
    Ok(output) => {
      assert!(output.status.success());
      let stdout = String::from_utf8_lossy(&output.stdout);
      assert!(
        stdout.contains("--register"),
        "Expected --register in help, got: {}",
        stdout
      );
      assert!(
        stdout.contains("drone"),
        "Expected 'drone' in help, got: {}",
        stdout
      );
    }
    Err(e) => {
      panic!("Failed to execute binary: {}", e);
    }
  }
}
