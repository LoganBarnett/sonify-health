// Helpers in this file sit outside `#[test]` functions, so
// clippy.toml's `allow-{unwrap,expect,panic}-in-tests` does not
// reach them.  Opt the whole file in explicitly.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use clap::Parser;
use rust_template_foundation::server::runner::ServerApp;
use sonify_health_server::config::{CliRaw, Config};
use std::ffi::OsString;
use std::io::Write;
use tempfile::NamedTempFile;

fn temp_file_containing(contents: &str) -> NamedTempFile {
  let mut file = NamedTempFile::new().unwrap();
  file.write_all(contents.as_bytes()).unwrap();
  file
}

/// The redirect base and the OIDC client settings are resolved separately,
/// so nothing but this proves they arrive at the server runner together.
#[test]
fn oidc_run_config_carries_the_base_url() {
  // An explicit, empty config file keeps the host's own
  // ~/.config/sonify-health/config.toml out of the result.
  let config_file = temp_file_containing("");
  let secret_file = temp_file_containing("s3cret\n");
  let args: Vec<OsString> = vec![
    "sonify-health-server".into(),
    "--config".into(),
    config_file.path().into(),
    "--base-url".into(),
    "https://sonify.example.com".into(),
    "--oidc-issuer".into(),
    "https://sso.example.com".into(),
    "--oidc-client-id".into(),
    "sonify".into(),
    "--oidc-client-secret-file".into(),
    secret_file.path().into(),
  ];

  let config =
    Config::from_cli_and_file(CliRaw::try_parse_from(args).unwrap()).unwrap();
  let run_configs = config.server_run_configs();
  let oidc = run_configs[0].oidc.as_ref().unwrap();

  assert_eq!(oidc.base_url, "https://sonify.example.com");
  assert_eq!(oidc.issuer, "https://sso.example.com");
  assert_eq!(oidc.client_id, "sonify");
  assert_eq!(oidc.client_secret, "s3cret");
}

/// The config resolved from `contents`, with `extra` arguments after it.
fn config_from(contents: &str, extra: &[&str]) -> Result<Config, String> {
  let config_file = temp_file_containing(contents);
  let args: Vec<OsString> = ["sonify-health-server", "--config"]
    .into_iter()
    .map(OsString::from)
    .chain([config_file.path().as_os_str().to_owned()])
    .chain(extra.iter().map(OsString::from))
    .collect();
  Config::from_cli_and_file(CliRaw::try_parse_from(args).unwrap())
    .map_err(|error| error.to_string())
}

const PAST_THE_LIMITS: &str = r#"
[patches.slow]
attack_ms = -5.0
freq = 15.0

[patches.slower]
overrides = "slow"
reverb_mix = 1.5
"#;

#[test]
fn a_value_past_a_hard_limit_loads_held_at_the_limit() {
  let config = config_from(PAST_THE_LIMITS, &[]).unwrap();
  let slow = &config.library["slow"];
  assert_eq!(slow.attack_ms, 0.0);
  let slower = &config.library["slower"];
  assert_eq!(slower.reverb_mix, 1.0);
  assert_eq!(slower.attack_ms, 0.0);
}

#[test]
fn a_value_past_the_slider_loads_as_written() {
  let config = config_from(PAST_THE_LIMITS, &[]).unwrap();
  assert_eq!(config.library["slow"].freq, 15.0);
}

#[test]
fn strict_limits_turns_a_hard_limit_into_an_error() {
  let flagged =
    config_from(PAST_THE_LIMITS, &["--strict-limits", "true"]).unwrap_err();
  assert!(flagged.contains("attack_ms"), "{flagged}");
  assert!(flagged.contains("--strict-limits"), "{flagged}");

  let keyed = format!("strict_limits = true\n{PAST_THE_LIMITS}");
  assert!(config_from(&keyed, &[]).is_err());
}

#[test]
fn strict_limits_accepts_a_value_past_the_slider() {
  let config = config_from(
    "strict_limits = true\n[patches.slow]\nattack_ms = 3000.0\n",
    &[],
  )
  .unwrap();
  assert_eq!(config.library["slow"].attack_ms, 3000.0);
}
