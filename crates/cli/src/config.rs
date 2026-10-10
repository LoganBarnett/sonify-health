//! CLI-side configuration: just enough to drive the `preview` and
//! `print` one-shots.  Daemon-side fields (listen address,
//! heartbeats, OIDC, remote sources, slider ranges, …) live on the
//! sibling server crate's `Config` because they're meaningless
//! outside the long-running daemon.  Both binaries plug into the
//! same on-disk config file via `SonifyFileFields` from the lib, so
//! a user's `config.toml` works against either binary unchanged.

use crate::command::Command;
use rust_template_foundation::logging::{LogFormat, LogLevel};
use rust_template_foundation::MergeConfig;
use sonify_health_lib::config::{
  ConfigError as LibConfigError, PatchLimitViolation,
};
use sonify_health_lib::{load_library, LoadedLibrary, PatchLibrary};
use std::path::PathBuf;

/// CLI-only arguments that feed the library resolver but don't
/// belong on `Config` as direct fields.
#[derive(Debug, clap::Args)]
pub struct CliExtraFields {
  /// Path to a TOML file of patch definitions.  May be repeated;
  /// last-in wins for overlapping patch names.  The main config
  /// file always wins over CLI-supplied patch libraries.
  #[arg(long)]
  pub patch_library: Vec<PathBuf>,
}

#[derive(Debug, Clone, MergeConfig)]
#[merge_config(
  app_name = "sonify-health",
  extra_cli = "CliExtraFields",
  extra_file = "sonify_health_lib::config::SonifyFileFields",
  extra_error = "sonify_health_lib::config::ConfigError"
)]
pub struct Config {
  #[merge_config(common)]
  pub log_level: LogLevel,
  #[merge_config(common)]
  pub log_format: LogFormat,

  /// Audio device substring for output device selection.  Used by
  /// the `preview` subcommand when it opens an `AudioOutput`.
  #[merge_config(default = "None")]
  pub audio_device: Option<String>,

  /// Fail when a patch value breaks a hard limit, instead of holding it at
  /// the limit with a warning.  Covers the config file, patch libraries, and
  /// the patch override flags.
  #[merge_config(default = "false")]
  pub strict_limits: bool,

  #[merge_config(skip)]
  pub library: PatchLibrary,

  /// Patch values the hard limits moved while loading.  Configuration is
  /// read before logging starts, so `main` logs these once it has.
  #[merge_config(skip)]
  pub limit_violations: Vec<PatchLimitViolation>,

  #[merge_config(subcommand)]
  pub command: Command,
}

impl Config {
  fn resolve_library(
    cli: &CliRaw,
    file: &ConfigFileRaw,
  ) -> Result<PatchLibrary, LibConfigError> {
    loaded_library(cli, file).map(|loaded| loaded.library)
  }

  fn resolve_limit_violations(
    cli: &CliRaw,
    file: &ConfigFileRaw,
  ) -> Result<Vec<PatchLimitViolation>, LibConfigError> {
    loaded_library(cli, file).map(|loaded| loaded.limit_violations)
  }
}

/// The patch library this config describes.
fn loaded_library(
  cli: &CliRaw,
  file: &ConfigFileRaw,
) -> Result<LoadedLibrary, LibConfigError> {
  // The library resolves before the merged `Config` exists, so the flag is
  // read from the raw layers in the merge's own order: the CLI wins.
  load_library(
    &file.extra.patches,
    &cli.extra.patch_library,
    cli.strict_limits.or(file.strict_limits).unwrap_or(false),
  )
}
