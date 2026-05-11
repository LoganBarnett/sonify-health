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
use sonify_health_lib::config::{ConfigError as LibConfigError, OverrideInfo};
use sonify_health_lib::{builtin_library, Patch, PatchLibrary, PatchOverrides};
use std::collections::HashMap;
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
  #[merge_config(env, default = "None")]
  pub audio_device: Option<String>,

  #[merge_config(skip)]
  pub library: PatchLibrary,

  #[merge_config(subcommand)]
  pub command: Command,
}

impl Config {
  fn resolve_library(
    cli: &CliRaw,
    file: &ConfigFileRaw,
  ) -> Result<PatchLibrary, LibConfigError> {
    let (library, _overrides) = build_library_and_overrides(cli, file)?;
    Ok(library)
  }
}

fn build_library_and_overrides(
  cli: &CliRaw,
  file: &ConfigFileRaw,
) -> Result<(PatchLibrary, HashMap<String, OverrideInfo>), LibConfigError> {
  let mut library = builtin_library();
  let mut override_entries: Vec<(String, String, toml::Value)> = Vec::new();

  for (name, mut table) in file.extra.patches.clone() {
    if let Some(base_val) =
      table.as_table_mut().and_then(|t| t.remove("overrides"))
    {
      let base = base_val
        .as_str()
        .ok_or_else(|| {
          LibConfigError::Validation(format!(
            "patch {name:?}: 'overrides' must be a string"
          ))
        })?
        .to_string();
      override_entries.push((name, base, table));
    } else {
      let patch: Patch =
        table
          .try_into()
          .map_err(|source| LibConfigError::PatchParse {
            name: name.clone(),
            source: Box::new(source),
          })?;
      library.insert(name, patch);
    }
  }

  for path in &cli.extra.patch_library {
    let contents = std::fs::read_to_string(path).map_err(|source| {
      LibConfigError::PatchLibraryRead {
        path: path.clone(),
        source,
      }
    })?;
    let extra: HashMap<String, Patch> =
      toml::from_str(&contents).map_err(|source| {
        LibConfigError::PatchLibraryParse {
          path: path.clone(),
          source: Box::new(source),
        }
      })?;
    for (name, patch) in extra {
      library.insert(name, patch);
    }
  }

  let mut overrides = HashMap::new();
  for (name, base, table) in override_entries {
    if !library.contains_key(&base) {
      return Err(LibConfigError::OverrideBaseMissing { name, base });
    }
    if overrides.contains_key(&base) {
      return Err(LibConfigError::OverrideChained { name, base });
    }
    let parsed: PatchOverrides =
      table
        .try_into()
        .map_err(|source| LibConfigError::PatchParse {
          name: name.clone(),
          source: Box::new(source),
        })?;
    let delta: HashMap<String, f64> = parsed
      .to_fields()
      .into_iter()
      .map(|(k, v)| (k.to_string(), v))
      .collect();
    let resolved = library[&base].clone().with_overrides(&parsed);
    library.insert(name.clone(), resolved);
    overrides.insert(name, OverrideInfo { base, delta });
  }

  Ok((library, overrides))
}
