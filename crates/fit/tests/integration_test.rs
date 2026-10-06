// Helpers in this file sit outside `#[test]` functions, so
// clippy.toml's `allow-{unwrap,expect,panic}-in-tests` does not
// reach them.  Opt the whole file in explicitly.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use fundsp::wave::Wave;
use sonify_health_lib::{heartbeat, Patch, ResolvedNote};
use std::path::Path;
use std::process::{Command, Output};

fn fit(scratch: &Path, args: &[&str]) -> Output {
  Command::new(env!("CARGO_BIN_EXE_sonify-health-fit"))
    .args(args)
    // A developer's own `config.toml` or shell would otherwise leak into the
    // result: the tool shares the daemon's config location and environment
    // prefix.  Confining the config search to `scratch` and clearing the
    // variables keeps each run to what the test passes.
    .env("XDG_CONFIG_HOME", scratch)
    .env_remove("sonify_health_config")
    .env_remove("sonify_health_log_level")
    .env_remove("sonify_health_log_format")
    .output()
    .expect("failed to run sonify-health-fit")
}

fn stderr(output: &Output) -> String {
  String::from_utf8_lossy(&output.stderr).into_owned()
}

fn peak(samples: &[f32]) -> f32 {
  samples
    .iter()
    .fold(0.0, |peak, sample| peak.max(sample.abs()))
}

#[test]
fn help_lists_the_render_subcommand() {
  let scratch = tempfile::tempdir().unwrap();
  let output = fit(scratch.path(), &["--help"]);
  assert!(output.status.success(), "stderr: {}", stderr(&output));
  let stdout = String::from_utf8_lossy(&output.stdout);
  assert!(stdout.contains("Usage:"), "stdout: {stdout}");
  assert!(stdout.contains("render"), "stdout: {stdout}");
}

#[test]
fn version_flag() {
  let scratch = tempfile::tempdir().unwrap();
  let output = fit(scratch.path(), &["--version"]);
  assert!(output.status.success(), "stderr: {}", stderr(&output));
  // clap prints the app name (the `merge_config(app_name = ...)` value), not
  // the binary name, so the version line reads "sonify-health <version>".
  let stdout = String::from_utf8_lossy(&output.stdout);
  assert!(stdout.contains("sonify-health"), "stdout: {stdout}");
}

#[test]
fn render_writes_the_frames_the_daemon_graph_produces() {
  let scratch = tempfile::tempdir().unwrap();
  let wav = scratch.path().join("note.wav");
  let output = fit(
    scratch.path(),
    &[
      "render",
      "--output",
      wav.to_str().unwrap(),
      "--seconds",
      "0.5",
      "--sample-rate",
      "22050",
      "--param",
      "freq=220",
      "--param",
      "stereo_pan=-0.5",
    ],
  );
  assert!(output.status.success(), "stderr: {}", stderr(&output));

  let wave = Wave::load(&wav).unwrap();
  assert_eq!(wave.channels(), 2);
  assert_eq!(wave.sample_rate(), 22_050.0);
  assert_eq!(wave.len(), 11_025);
  assert!(peak(wave.channel(0)) > 0.05, "left channel is silent");
  assert!(peak(wave.channel(1)) > 0.05, "right channel is silent");

  let mut graph = heartbeat::heartbeat_graph_with_notes(
    &[ResolvedNote {
      patch: Patch {
        freq: 220.0,
        stereo_pan: -0.5,
        ..Default::default()
      },
      volume: 1.0,
      offset: 0.0,
    }],
    None,
    22_050.0,
  );
  graph.set_sample_rate(22_050.0);
  graph.allocate();
  let (left, right): (Vec<f32>, Vec<f32>) =
    (0..wave.len()).map(|_| graph.get_stereo()).unzip();
  // The off-centre pan makes the channels differ, so a file with them swapped
  // or duplicated cannot pass the comparison below.
  assert!(left != right, "the reference channels should differ");
  assert!(wave.channel(0) == &left, "left channel differs from the graph");
  assert!(wave.channel(1) == &right, "right channel differs from the graph");
}

#[test]
fn render_accepts_the_daemon_config_file() {
  let scratch = tempfile::tempdir().unwrap();
  let wav = scratch.path().join("note.wav");
  let config = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/connectivity-and-cpu-star-trek.toml"
  );
  let output = fit(
    scratch.path(),
    &[
      "--config",
      config,
      "render",
      "--output",
      wav.to_str().unwrap(),
    ],
  );
  assert!(output.status.success(), "stderr: {}", stderr(&output));
  assert!(Wave::load(&wav).unwrap().amplitude() > 0.05);
}

#[test]
fn render_rejects_an_unknown_parameter() {
  let scratch = tempfile::tempdir().unwrap();
  let wav = scratch.path().join("note.wav");
  let output = fit(
    scratch.path(),
    &[
      "render",
      "--output",
      wav.to_str().unwrap(),
      "--param",
      "wobble=1",
    ],
  );
  assert!(!output.status.success());
  assert!(
    stderr(&output).contains("\"wobble\""),
    "stderr: {}",
    stderr(&output)
  );
  assert!(!wav.exists(), "nothing should be written for a bad patch");
}

#[test]
fn render_rejects_a_malformed_assignment() {
  let scratch = tempfile::tempdir().unwrap();
  let wav = scratch.path().join("note.wav");
  let output = fit(
    scratch.path(),
    &[
      "render",
      "--output",
      wav.to_str().unwrap(),
      "--param",
      "freq",
    ],
  );
  assert!(!output.status.success());
  assert!(
    stderr(&output).contains("expected NAME=VALUE"),
    "stderr: {}",
    stderr(&output)
  );
}

#[test]
fn render_reports_an_unwritable_output() {
  let scratch = tempfile::tempdir().unwrap();
  let wav = scratch.path().join("missing-directory").join("note.wav");
  let output =
    fit(scratch.path(), &["render", "--output", wav.to_str().unwrap()]);
  assert!(!output.status.success());
  assert!(
    stderr(&output).contains("missing-directory"),
    "stderr: {}",
    stderr(&output)
  );
}

fn stdout(output: &Output) -> String {
  String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Renders a steady 110 Hz saw into `scratch` and returns the file's path.
fn rendered_tone(scratch: &Path) -> String {
  let wav = scratch.join("tone.wav").to_str().unwrap().to_string();
  let output = fit(
    scratch,
    &[
      "render",
      "--output",
      &wav,
      "--seconds",
      "2.5",
      "--param",
      "freq=110",
      "--param",
      "sine_ratio=0",
      "--param",
      "saw_ratio=1",
      "--param",
      "duration=4",
    ],
  );
  assert!(output.status.success(), "stderr: {}", stderr(&output));
  wav
}

#[test]
fn help_lists_the_fit_and_compare_subcommands() {
  let scratch = tempfile::tempdir().unwrap();
  let output = fit(scratch.path(), &["--help"]);
  assert!(output.status.success(), "stderr: {}", stderr(&output));
  let stdout = stdout(&output);
  assert!(stdout.contains("fit"), "stdout: {stdout}");
  assert!(stdout.contains("compare"), "stdout: {stdout}");
}

#[test]
fn compare_scores_a_recording_against_itself_as_identical() {
  let scratch = tempfile::tempdir().unwrap();
  let wav = rendered_tone(scratch.path());
  let output = fit(
    scratch.path(),
    &[
      "compare",
      "--target",
      &wav,
      "--fundamental",
      "110",
      "--settle-seconds",
      "0",
      &wav,
    ],
  );
  assert!(output.status.success(), "stderr: {}", stderr(&output));
  let stdout = stdout(&output);
  let distances: Vec<&str> = stdout
    .lines()
    .find(|line| line.starts_with("distance"))
    .unwrap_or_else(|| panic!("no distance row in: {stdout}"))
    .split_whitespace()
    .skip(1)
    .collect();
  assert_eq!(distances, ["0.00", "0.00"], "stdout: {stdout}");
}

#[test]
fn compare_names_a_recording_it_cannot_load() {
  let scratch = tempfile::tempdir().unwrap();
  let wav = rendered_tone(scratch.path());
  let missing = scratch.path().join("absent.wav");
  let output = fit(
    scratch.path(),
    &[
      "compare",
      "--target",
      &wav,
      "--fundamental",
      "110",
      missing.to_str().unwrap(),
    ],
  );
  assert!(!output.status.success());
  assert!(
    stderr(&output).contains("absent.wav"),
    "stderr: {}",
    stderr(&output)
  );
}

// The budgets here are far too small to find a good match; the test is that a
// search runs end to end and prints a fragment that pastes into a config.
#[test]
fn fit_prints_a_patch_fragment_with_its_pins() {
  let scratch = tempfile::tempdir().unwrap();
  let wav = rendered_tone(scratch.path());
  let output = fit(
    scratch.path(),
    &[
      "fit",
      "--target",
      &wav,
      "--fundamental",
      "110",
      "--param",
      "freq=110",
      "--sample-rate",
      "22050",
      "--measured-frames",
      "4096",
      "--random-samples",
      "10",
      "--climb-steps",
      "2",
      "--results",
      "1",
    ],
  );
  assert!(output.status.success(), "stderr: {}", stderr(&output));
  let stdout = stdout(&output);
  assert!(stdout.starts_with("# distance "), "stdout: {stdout}");
  assert!(stdout.contains("\nfreq = 110.000\n"), "stdout: {stdout}");
  assert!(stdout.contains("\nsaw_ratio = "), "stdout: {stdout}");
}

#[test]
fn fit_rejects_an_unknown_pin_before_searching() {
  let scratch = tempfile::tempdir().unwrap();
  let wav = rendered_tone(scratch.path());
  let output = fit(
    scratch.path(),
    &[
      "fit",
      "--target",
      &wav,
      "--fundamental",
      "110",
      "--param",
      "wobbliness=1",
    ],
  );
  assert!(!output.status.success());
  assert!(
    stderr(&output).contains("wobbliness"),
    "stderr: {}",
    stderr(&output)
  );
}
