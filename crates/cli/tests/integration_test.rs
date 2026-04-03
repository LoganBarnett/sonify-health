use std::{path::PathBuf, process::Command};

fn get_binary_path() -> PathBuf {
  let mut path =
    std::env::current_exe().expect("Failed to get current executable path");

  // Navigate from the test executable to the binary
  path.pop(); // remove test executable name
  path.pop(); // remove deps dir
  path.push("sonify-health-cli");

  // If the binary doesn't exist in release, try debug
  if !path.exists() {
    path.pop();
    path.pop();
    path.push("debug");
    path.push("sonify-health-cli");
  }

  path
}

#[test]
fn test_help_flag() {
  let output = Command::new(get_binary_path()).arg("--help").output();

  match output {
    Ok(output) => {
      assert!(
        output.status.success(),
        "Expected success exit code, got: {:?}",
        output.status.code()
      );
      let stdout = String::from_utf8_lossy(&output.stdout);
      assert!(
        stdout.contains("Usage:"),
        "Expected help text to contain 'Usage:', got: {}",
        stdout
      );
    }
    Err(e) => {
      if e.kind() == std::io::ErrorKind::NotFound {
        eprintln!(
                    "Binary not found. Please build the project first with: cargo build -p sonify-health-cli"
                );
      }
      panic!("Failed to execute binary: {}", e);
    }
  }
}

#[test]
fn test_version_flag() {
  let output = Command::new(get_binary_path()).arg("--version").output();

  match output {
    Ok(output) => {
      assert!(
        output.status.success(),
        "Expected success exit code, got: {:?}",
        output.status.code()
      );
      let stdout = String::from_utf8_lossy(&output.stdout);
      assert!(
        stdout.contains("sonify-health-cli"),
        "Expected version text to contain 'sonify-health-cli', got: {}",
        stdout
      );
    }
    Err(e) => {
      if e.kind() == std::io::ErrorKind::NotFound {
        eprintln!(
                    "Binary not found. Please build the project first with: cargo build -p sonify-health-cli"
                );
      }
      panic!("Failed to execute binary: {}", e);
    }
  }
}

#[test]
fn test_basic_execution() {
  let output = Command::new(get_binary_path()).output();

  match output {
    Ok(output) => {
      assert!(
        output.status.success(),
        "Expected success exit code, got: {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
      );
    }
    Err(e) => {
      if e.kind() == std::io::ErrorKind::NotFound {
        eprintln!(
                    "Binary not found. Please build the project first with: cargo build -p sonify-health-cli"
                );
      }
      panic!("Failed to execute binary: {}", e);
    }
  }
}

#[test]
fn test_with_name_argument() {
  let output = Command::new(get_binary_path())
    .arg("--name")
    .arg("Rust")
    .output();

  match output {
    Ok(output) => {
      assert!(
        output.status.success(),
        "Expected success exit code, got: {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
      );
    }
    Err(e) => {
      if e.kind() == std::io::ErrorKind::NotFound {
        eprintln!(
                    "Binary not found. Please build the project first with: cargo build -p sonify-health-cli"
                );
      }
      panic!("Failed to execute binary: {}", e);
    }
  }
}
