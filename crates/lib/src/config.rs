//! Shared configuration leaf types.  Anything in this module is
//! used by *both* the cli and server crates — the bigger
//! `BaseConfig` / `ServerConfig` story (where the assembled
//! configuration values live) gets layered on top of these in a
//! later phase of the workspace split.
//!
//! Server-specific types like `RemoteSourceConfig`, `OidcConfig`,
//! and `ConfigSaveError` deliberately stay in the server crate;
//! moving them here would force the cli to drag deps it does not
//! need.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use thiserror::Error;

/// Tracks which patches are overrides (derived from a base patch
/// with a sparse delta) so the UI can display inherited vs
/// overridden parameters and exports can emit the compact form.
///
/// `Serialize` / `Deserialize` are derived to match the on-the-wire
/// shape that `state_snapshot` already emits — `{"base": "...",
/// "delta": {...}}` — so a remote-source connector can
/// deserialize it directly without a separate wire-side mirror
/// struct.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverrideInfo {
  pub base: String,
  pub delta: HashMap<String, f64>,
}

/// Errors raised while loading configuration.  Variants cover both
/// cli and server failure modes — ConfigError is shared because
/// both binaries load configuration through similar code paths
/// (file read, TOML parse, validation, optional patch-library
/// file parsing) and an operator hitting any of them gets the
/// same diagnostic.
///
/// The `toml::de::Error` payloads are boxed so the enum stays
/// small enough that callers can return `Result<T, ConfigError>`
/// without tripping `clippy::result_large_err` — the parser
/// errors are large (they carry the full source span), and the
/// box keeps the discriminant-plus-pointer layout compact.
#[derive(Debug, Error)]
pub enum ConfigError {
  #[error(
    "Failed to read configuration file at {path:?}: \
     {source}"
  )]
  FileRead {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },

  #[error(
    "Failed to parse configuration file at {path:?}: \
     {source}"
  )]
  Parse {
    path: PathBuf,
    #[source]
    source: Box<toml::de::Error>,
  },

  #[error("Configuration validation failed: {0}")]
  Validation(String),

  #[error("Failed to read OIDC client secret from {path:?}: {source}")]
  OidcSecretFileRead {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },

  #[error(
    "Failed to read patch library file at {path:?}: \
     {source}"
  )]
  PatchLibraryRead {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },

  #[error(
    "Failed to parse patch library file at {path:?}: \
     {source}"
  )]
  PatchLibraryParse {
    path: PathBuf,
    #[source]
    source: Box<toml::de::Error>,
  },

  #[error(
    "Override patch {name:?} references unknown base \
     patch {base:?}"
  )]
  OverrideBaseMissing { name: String, base: String },

  #[error(
    "Override patch {name:?} references another override \
     {base:?} (chained overrides are not supported)"
  )]
  OverrideChained { name: String, base: String },

  #[error("Failed to parse patch {name:?}: {source}")]
  PatchParse {
    name: String,
    #[source]
    source: Box<toml::de::Error>,
  },
}

/// One numeric range exposed to the UI for slider widgets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SliderRange {
  pub min: f64,
  pub max: f64,
  pub step: f64,
}

/// Slider ranges for every numeric field the UI exposes.  Carried
/// in the state snapshot so the frontend uses the same bounds the
/// operator configured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SliderRanges {
  pub master_volume: SliderRange,
  pub cycle_offset: SliderRange,
  pub override_metric: SliderRange,
  pub note_volume: SliderRange,
  pub note_offset: SliderRange,
  pub segment_intensity: SliderRange,
  pub discrete_threshold: SliderRange,
  pub step_position: SliderRange,
  pub crossfade_ms: SliderRange,
}

impl Default for SliderRanges {
  fn default() -> Self {
    Self {
      master_volume: SliderRange {
        min: 0.0,
        max: 1.0,
        step: 0.01,
      },
      cycle_offset: SliderRange {
        min: 0.0,
        max: 60.0,
        step: 0.1,
      },
      override_metric: SliderRange {
        min: 0.0,
        max: 1.0,
        step: 0.01,
      },
      note_volume: SliderRange {
        min: 0.0,
        max: 1.0,
        step: 0.01,
      },
      note_offset: SliderRange {
        min: 0.0,
        max: 60.0,
        step: 0.1,
      },
      segment_intensity: SliderRange {
        min: 0.1,
        max: 10.0,
        step: 0.1,
      },
      discrete_threshold: SliderRange {
        min: 0.0,
        max: 1.0,
        step: 0.01,
      },
      step_position: SliderRange {
        min: 0.0,
        max: 1.0,
        step: 0.01,
      },
      crossfade_ms: SliderRange {
        min: 0.0,
        max: 500.0,
        step: 1.0,
      },
    }
  }
}
