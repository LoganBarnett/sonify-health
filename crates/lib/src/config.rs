//! Shared configuration leaf types and the on-disk config-file
//! shape.  Both the cli (preview/print) and server (daemon)
//! binaries read the same `config.toml`; the shape lives here so
//! it has exactly one definition that both binaries' `MergeConfig`
//! derives plug in via `extra_file`.

use crate::HeartbeatConfig;
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

/// Static configuration for a single Remote Source — the entry the
/// operator writes in their config file or passes on the CLI to
/// declare "subscribe to this other instance and mirror its
/// state."  The runtime `Source` (in the server's `preview_state`)
/// is constructed from this descriptor at startup; the connector
/// then populates the runtime's library / heartbeats from the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteSourceConfig {
  /// Display name and unique identifier for the source.  Cannot be
  /// `"localhost"` (reserved for the Local Source) and must not
  /// collide with another source's name.
  pub name: String,

  /// WebSocket URL — `ws://` or `wss://`.
  pub url: String,

  /// Whether the local renderer plays audio for this source.
  /// Default false: the user opts in explicitly so adding a remote
  /// to the config never starts playing audio without consent.
  #[serde(default)]
  pub playback_enabled: bool,
}

/// File-side representation of the optional `[oidc]` section.
/// Both binaries deserialize the same TOML; the server uses these
/// to assemble the fully-resolved OIDC client, the cli ignores
/// them.
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct OidcSectionRaw {
  pub base_url: Option<String>,
  pub issuer: Option<String>,
  pub client_id: Option<String>,
  pub client_secret_file: Option<PathBuf>,
}

/// The full on-disk shape of `config.toml`.  Each binary plugs this
/// into its `#[merge_config(extra_file = "...")]` derive, so the
/// file format has exactly one definition — both binaries
/// deserialize identically and each surfaces only the subset its
/// `Config` needs.
#[derive(Debug, Deserialize, Default)]
pub struct SonifyFileFields {
  /// User-defined patches.  `toml::Value` because each entry may
  /// be either a full `Patch` table or an override
  /// (`overrides = "base"` + delta fields), resolved at config
  /// build time.
  #[serde(default)]
  pub patches: HashMap<String, toml::Value>,

  #[serde(default)]
  pub heartbeats: Vec<HeartbeatConfig>,

  #[serde(default)]
  pub slider_ranges: SliderRanges,

  pub oidc: Option<OidcSectionRaw>,

  #[serde(default)]
  pub sources: Vec<RemoteSourceConfig>,
}
