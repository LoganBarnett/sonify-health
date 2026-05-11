use crate::command::Command;
use rust_template_foundation::logging::{LogFormat, LogLevel};
use rust_template_foundation::MergeConfig;
use serde::{Deserialize, Serialize};
use sonify_health_lib::config::ConfigError as LibConfigError;
use sonify_health_lib::{
  builtin_library, HeartbeatConfig, Patch, PatchLibrary, PatchOverrides,
};
use std::collections::HashMap;
use std::path::PathBuf;
use thiserror::Error;
use tokio_listener::ListenerAddress;

// Shared leaf types live in the lib so the cli and server can
// agree on them without one binary depending on the other.
// Re-exported here for the websocket / preview_state / remote_source
// modules that read these types out of `crate::config`.  The lib's
// `ConfigError` is intentionally *not* re-exported under this name
// because the `MergeConfig` derive on `Config` (below) emits its own
// `ConfigError` type in this same module; the lib's rich error type
// is reachable as `sonify_health_lib::config::ConfigError` (aliased
// `LibConfigError` inside this module).
pub use sonify_health_lib::config::{OverrideInfo, SliderRange, SliderRanges};

/// Static configuration for a single Remote Source — the entry the
/// user writes in their config file or passes on the CLI to declare
/// "subscribe to this other instance and mirror its state."  The
/// runtime `Source` (in `preview_state::Source`) is constructed
/// from this descriptor at startup; the connector then populates
/// the runtime's library/heartbeats from the wire.
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

/// Fully resolved OIDC configuration.  `base_url` is captured here
/// (rather than on `Config` directly) because the OIDC code in the
/// daemon path consumes the full struct and we don't gain anything
/// by splitting the four fields across two scopes.
#[derive(Debug, Clone)]
pub struct OidcConfig {
  pub base_url: String,
  pub issuer: String,
  pub client_id: String,
  pub client_secret: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct OidcSectionRaw {
  pub base_url: Option<String>,
  pub issuer: Option<String>,
  pub client_id: Option<String>,
  pub client_secret_file: Option<PathBuf>,
}

/// CLI-only arguments that are inputs to skip-field resolvers
/// (`resolve_library`, `resolve_remote_sources`, `resolve_oidc`)
/// rather than direct fields on `Config`.  Flattened into the
/// macro-generated `CliRaw` via `extra_cli`.
#[derive(Debug, clap::Args)]
pub struct SonifyCliFields {
  /// Path to a TOML file of patch definitions.  May be repeated;
  /// last-in wins for overlapping patch names.  The main config
  /// file always wins over CLI-supplied patch libraries.
  #[arg(long)]
  pub patch_library: Vec<PathBuf>,

  /// Declare a Remote Source as `name=url`.  May be repeated.
  /// Sources from the config file's `[[sources]]` array and these
  /// CLI flags are merged; names must be unique across the merged
  /// set, and `localhost` is reserved for the Local Source.
  #[arg(long, value_name = "NAME=URL")]
  pub source: Vec<String>,

  /// Base URL of this service (e.g. https://sonify.example.com),
  /// used to construct the OIDC redirect URI.
  #[arg(long, env = "sonify_health_base_url")]
  pub base_url: Option<String>,

  /// OIDC issuer URL for provider discovery.
  #[arg(long, env = "sonify_health_oidc_issuer")]
  pub oidc_issuer: Option<String>,

  /// OIDC client ID.
  #[arg(long, env = "sonify_health_oidc_client_id")]
  pub oidc_client_id: Option<String>,

  /// Path to a file containing the OIDC client secret.
  #[arg(long, env = "sonify_health_oidc_client_secret_file")]
  pub oidc_client_secret_file: Option<PathBuf>,
}

/// Config-file-only fields that don't appear on `Config` as
/// merged scalars (because they're collections or composite shapes
/// that have no useful CLI form, or are inputs to a skip-field
/// resolver).  Flattened into the macro-generated `ConfigFileRaw`
/// via `extra_file`.
#[derive(Debug, Deserialize, Default)]
pub struct SonifyFileFields {
  /// User-defined patches.  `toml::Value` because each entry may
  /// be either a full `Patch` table or an override
  /// (`overrides = "base"` + delta fields), resolved in
  /// `resolve_library` / `resolve_overrides`.
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

#[derive(Debug, Clone, MergeConfig)]
#[merge_config(
  app_name = "sonify-health",
  extra_cli = "SonifyCliFields",
  extra_file = "SonifyFileFields",
  extra_error = "sonify_health_lib::config::ConfigError"
)]
pub struct Config {
  #[merge_config(common)]
  pub log_level: LogLevel,
  #[merge_config(common)]
  pub log_format: LogFormat,

  /// Address to listen on: host:port for TCP, /path/to.sock for
  /// Unix socket, or sd-listen to inherit from systemd.
  #[merge_config(
    name = "listen",
    env,
    default = "\"127.0.0.1:3000\".to_string()",
    parse
  )]
  pub listen_address: ListenerAddress,

  /// Path to compiled frontend static assets.
  #[merge_config(env, default = "PathBuf::from(\"frontend/public\")")]
  pub frontend_path: PathBuf,

  /// Audio device substring for output device selection.  Match is
  /// case-insensitive against both cpal's device ID and description.
  #[merge_config(env, default = "None")]
  pub audio_device: Option<String>,

  /// Run without opening an audio device.  Pollers, WebSocket
  /// state, frontend, and metrics keep working; intended for
  /// speakerless servers whose state will be rendered by another
  /// instance subscribed to this one.
  #[merge_config(env, default = "false")]
  pub headless: bool,

  #[merge_config(skip)]
  pub library: PatchLibrary,

  #[merge_config(skip)]
  pub overrides: HashMap<String, OverrideInfo>,

  #[merge_config(skip)]
  pub heartbeats: Vec<HeartbeatConfig>,

  #[merge_config(skip)]
  pub slider_ranges: SliderRanges,

  #[merge_config(skip)]
  pub oidc: Option<OidcConfig>,

  /// Path the config was actually loaded from (either the explicit
  /// `--config` argument or the XDG fallback).  Distinct from the
  /// input `--config` flag; this is the *resolved output* that the
  /// save-back UI flow writes to.
  #[merge_config(skip)]
  pub config_path_resolved: Option<PathBuf>,

  #[merge_config(skip)]
  pub remote_sources: Vec<RemoteSourceConfig>,

  #[merge_config(subcommand)]
  pub command: Command,
}

impl Config {
  fn resolve_library(
    cli: &CliRaw,
    file: &ConfigFileRaw,
  ) -> Result<PatchLibrary, LibConfigError> {
    build_library_and_overrides(cli, file).map(|(lib, _)| lib)
  }

  fn resolve_overrides(
    cli: &CliRaw,
    file: &ConfigFileRaw,
  ) -> Result<HashMap<String, OverrideInfo>, LibConfigError> {
    build_library_and_overrides(cli, file).map(|(_, ovr)| ovr)
  }

  fn resolve_heartbeats(
    _cli: &CliRaw,
    file: &ConfigFileRaw,
  ) -> Result<Vec<HeartbeatConfig>, LibConfigError> {
    let mut heartbeats = file.extra.heartbeats.clone();
    for hb in &mut heartbeats {
      hb.resolve_legacy_continuous();
    }
    Ok(heartbeats)
  }

  fn resolve_slider_ranges(
    _cli: &CliRaw,
    file: &ConfigFileRaw,
  ) -> Result<SliderRanges, LibConfigError> {
    Ok(file.extra.slider_ranges.clone())
  }

  fn resolve_config_path_resolved(
    cli: &CliRaw,
    _file: &ConfigFileRaw,
  ) -> Result<Option<PathBuf>, LibConfigError> {
    Ok(rust_template_foundation::config::find_config_file(
      "sonify-health",
      cli.config.as_deref(),
    ))
  }

  fn resolve_oidc(
    cli: &CliRaw,
    file: &ConfigFileRaw,
  ) -> Result<Option<OidcConfig>, LibConfigError> {
    let file_oidc = file.extra.oidc.as_ref();
    let base = cli
      .extra
      .base_url
      .clone()
      .or_else(|| file_oidc.and_then(|o| o.base_url.clone()));
    let issuer = cli
      .extra
      .oidc_issuer
      .clone()
      .or_else(|| file_oidc.and_then(|o| o.issuer.clone()));
    let client_id = cli
      .extra
      .oidc_client_id
      .clone()
      .or_else(|| file_oidc.and_then(|o| o.client_id.clone()));
    let secret_file = cli
      .extra
      .oidc_client_secret_file
      .clone()
      .or_else(|| file_oidc.and_then(|o| o.client_secret_file.clone()));

    match (&base, &issuer, &client_id) {
      (None, None, None) if secret_file.is_none() => Ok(None),
      (Some(base_url), Some(iss), Some(cid)) => {
        let secret_file =
          secret_file.or_else(credential_secret_path).ok_or_else(|| {
            LibConfigError::Validation(
              "oidc_client_secret_file is required when oidc_issuer and \
               oidc_client_id are set (set it explicitly or run under \
               systemd with LoadCredential)"
                .to_string(),
            )
          })?;

        let client_secret = std::fs::read_to_string(&secret_file)
          .map(|s| s.trim().to_string())
          .map_err(|source| LibConfigError::OidcSecretFileRead {
            path: secret_file,
            source,
          })?;

        Ok(Some(OidcConfig {
          base_url: base_url.clone(),
          issuer: iss.clone(),
          client_id: cid.clone(),
          client_secret,
        }))
      }
      _ => {
        let mut present = Vec::new();
        let mut missing = Vec::new();
        for (name, val) in [
          ("base_url", base.is_some()),
          ("oidc_issuer", issuer.is_some()),
          ("oidc_client_id", client_id.is_some()),
          (
            "oidc_client_secret_file",
            secret_file.is_some() || credential_secret_path().is_some(),
          ),
        ] {
          if val {
            present.push(name);
          } else {
            missing.push(name);
          }
        }
        Err(LibConfigError::Validation(format!(
          "partial OIDC configuration: set all four fields or none. \
           present: [{}], missing: [{}]",
          present.join(", "),
          missing.join(", ")
        )))
      }
    }
  }

  fn resolve_remote_sources(
    cli: &CliRaw,
    file: &ConfigFileRaw,
  ) -> Result<Vec<RemoteSourceConfig>, LibConfigError> {
    // File entries first, CLI `--source name=url` appended.  The
    // wire syntax is `name=url`, splitting on the first `=` so URLs
    // containing `=` (e.g. token query params) survive intact.
    let mut remote_sources = file.extra.sources.clone();
    for raw in &cli.extra.source {
      let (name, url) = raw.split_once('=').ok_or_else(|| {
        LibConfigError::Validation(format!(
          "--source argument {raw:?} must be of the form name=url"
        ))
      })?;
      if name.is_empty() || url.is_empty() {
        return Err(LibConfigError::Validation(format!(
          "--source argument {raw:?} has empty name or url"
        )));
      }
      remote_sources.push(RemoteSourceConfig {
        name: name.to_string(),
        url: url.to_string(),
        playback_enabled: false,
      });
    }
    // Inline rather than referencing preview_state::LOCAL_SOURCE_NAME
    // — duplicating a never-changing string saves a cross-module
    // dependency and keeps `config.rs` from pulling in runtime types.
    const RESERVED_LOCAL_NAME: &str = "localhost";
    let mut seen = std::collections::HashSet::new();
    for s in &remote_sources {
      if s.name == RESERVED_LOCAL_NAME {
        return Err(LibConfigError::Validation(format!(
          "remote source name {:?} is reserved for the Local Source",
          s.name
        )));
      }
      if !seen.insert(s.name.as_str()) {
        return Err(LibConfigError::Validation(format!(
          "remote source name {:?} is declared more than once",
          s.name
        )));
      }
    }
    Ok(remote_sources)
  }
}

/// Build the patch library and override map from the merged config
/// inputs.  Called twice during `from_cli_and_file` (once for the
/// library resolver, once for the overrides resolver) — the walk is
/// pure compute over already-deserialized data, so the duplication is
/// cheap and avoids inventing a shared state cell to memoize across
/// two macro-driven resolver calls.
fn build_library_and_overrides(
  cli: &CliRaw,
  file: &ConfigFileRaw,
) -> Result<(PatchLibrary, HashMap<String, OverrideInfo>), LibConfigError> {
  let mut library = builtin_library();
  let mut override_entries: Vec<(String, String, toml::Value)> = Vec::new();

  // First pass: separate standalone patches from override patches.
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

  // Patch library files (CLI `--patch-library`, repeatable).
  // Last-in wins for overlapping names.  Config-file patches are
  // re-inserted in the override pass below, so the main config
  // always wins over CLI-supplied libraries.
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

  // Second pass: resolve override patches against the now-populated
  // library.
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

/// Serialize the current runtime state to a TOML config string that
/// can be loaded back via `Config::from_cli_and_file`.  Override
/// patches emit the compact `overrides = "base"` form with only delta
/// fields; standalone patches serialize as full `Patch` tables.
/// Builtin patches are omitted.
pub fn build_save_toml(
  library: &PatchLibrary,
  overrides: &HashMap<String, OverrideInfo>,
  heartbeats: &[HeartbeatConfig],
  slider_ranges: &SliderRanges,
  remote_sources: &[RemoteSourceConfig],
) -> Result<String, ConfigSaveError> {
  let builtins = builtin_library();
  let mut doc = toml::Table::new();

  // Patches: only user-defined (non-builtin) entries.
  let mut patches_table = toml::Table::new();
  for (name, patch) in library {
    if builtins.contains_key(name) && !overrides.contains_key(name) {
      continue;
    }
    if let Some(info) = overrides.get(name) {
      let mut tbl = toml::Table::new();
      tbl.insert(
        "overrides".to_string(),
        toml::Value::String(info.base.clone()),
      );
      for (param, val) in &info.delta {
        tbl.insert(param.clone(), toml::Value::Float(*val));
      }
      patches_table.insert(name.clone(), toml::Value::Table(tbl));
    } else {
      let val = toml::Value::try_from(patch)
        .map_err(|e| ConfigSaveError::PatchSerialize(name.clone(), e))?;
      patches_table.insert(name.clone(), val);
    }
  }
  if !patches_table.is_empty() {
    doc.insert("patches".to_string(), toml::Value::Table(patches_table));
  }

  // Heartbeats.
  let hb_val = toml::Value::try_from(heartbeats)
    .map_err(ConfigSaveError::HeartbeatSerialize)?;
  if let toml::Value::Array(ref arr) = hb_val {
    if !arr.is_empty() {
      doc.insert("heartbeats".to_string(), hb_val);
    }
  }

  // Slider ranges (only if non-default).
  let default_ranges = SliderRanges::default();
  let sr_val = toml::Value::try_from(slider_ranges)
    .map_err(ConfigSaveError::SliderRangesSerialize)?;
  let default_sr_val = toml::Value::try_from(&default_ranges)
    .map_err(ConfigSaveError::SliderRangesSerialize)?;
  if sr_val != default_sr_val {
    doc.insert("slider_ranges".to_string(), sr_val);
  }

  // Remote sources (only if any are declared).
  if !remote_sources.is_empty() {
    let val = toml::Value::try_from(remote_sources)
      .map_err(ConfigSaveError::RemoteSourcesSerialize)?;
    doc.insert("sources".to_string(), val);
  }

  toml::to_string_pretty(&doc).map_err(ConfigSaveError::Serialize)
}

/// Build an intermediate JSON value representing the full config.
/// Shared by `build_save_json` and `build_save_nix`.
fn build_save_value(
  library: &PatchLibrary,
  overrides: &HashMap<String, OverrideInfo>,
  heartbeats: &[HeartbeatConfig],
  slider_ranges: &SliderRanges,
  remote_sources: &[RemoteSourceConfig],
) -> Result<serde_json::Value, ConfigSaveError> {
  let builtins = builtin_library();
  let mut doc = serde_json::Map::new();

  let mut patches = serde_json::Map::new();
  for (name, patch) in library {
    if builtins.contains_key(name) && !overrides.contains_key(name) {
      continue;
    }
    if let Some(info) = overrides.get(name) {
      let mut obj = serde_json::Map::new();
      obj.insert(
        "overrides".into(),
        serde_json::Value::String(info.base.clone()),
      );
      for (param, val) in &info.delta {
        obj.insert(param.clone(), serde_json::Value::from(*val));
      }
      patches.insert(name.clone(), serde_json::Value::Object(obj));
    } else {
      patches.insert(
        name.clone(),
        serde_json::to_value(patch).map_err(ConfigSaveError::JsonSerialize)?,
      );
    }
  }
  if !patches.is_empty() {
    doc.insert("patches".into(), serde_json::Value::Object(patches));
  }

  let hb_val =
    serde_json::to_value(heartbeats).map_err(ConfigSaveError::JsonSerialize)?;
  if let serde_json::Value::Array(ref arr) = hb_val {
    if !arr.is_empty() {
      doc.insert("heartbeats".into(), hb_val);
    }
  }

  let default_ranges = SliderRanges::default();
  if slider_ranges != &default_ranges {
    let sr_val = serde_json::to_value(slider_ranges)
      .map_err(ConfigSaveError::JsonSerialize)?;
    doc.insert("slider_ranges".into(), sr_val);
  }

  if !remote_sources.is_empty() {
    let val = serde_json::to_value(remote_sources)
      .map_err(ConfigSaveError::JsonSerialize)?;
    doc.insert("sources".into(), val);
  }

  Ok(serde_json::Value::Object(doc))
}

/// Serialize the current runtime state to a JSON config string.
pub fn build_save_json(
  library: &PatchLibrary,
  overrides: &HashMap<String, OverrideInfo>,
  heartbeats: &[HeartbeatConfig],
  slider_ranges: &SliderRanges,
  remote_sources: &[RemoteSourceConfig],
) -> Result<String, ConfigSaveError> {
  let val = build_save_value(
    library,
    overrides,
    heartbeats,
    slider_ranges,
    remote_sources,
  )?;
  serde_json::to_string_pretty(&val).map_err(ConfigSaveError::JsonSerialize)
}

/// Serialize the current runtime state to Nix attribute set body.
/// The output assumes it is already inside the `sonify-health`
/// config section — no top-level module wrapper is emitted.
pub fn build_save_nix(
  library: &PatchLibrary,
  overrides: &HashMap<String, OverrideInfo>,
  heartbeats: &[HeartbeatConfig],
  slider_ranges: &SliderRanges,
  remote_sources: &[RemoteSourceConfig],
) -> Result<String, ConfigSaveError> {
  let val = build_save_value(
    library,
    overrides,
    heartbeats,
    slider_ranges,
    remote_sources,
  )?;
  Ok(nix_body(&val))
}

/// Ensure a float string contains a decimal point so Nix parses
/// it as a float, not an integer.
fn nix_float(v: f64) -> String {
  let s = v.to_string();
  if s.contains('.') || s.contains('e') || s.contains('E') {
    s
  } else {
    format!("{s}.0")
  }
}

/// Escape a string for Nix double-quoted literals.
fn nix_escape(s: &str) -> String {
  s.replace('\\', "\\\\")
    .replace('"', "\\\"")
    .replace("${", "\\${")
}

/// Format a string as a Nix attribute name, quoting if needed.
fn nix_attr(name: &str) -> String {
  let valid = !name.is_empty()
    && name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
    && name
      .chars()
      .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '\'');
  if valid {
    name.to_string()
  } else {
    format!("\"{}\"", nix_escape(name))
  }
}

/// Convert a serde_json value to Nix expression syntax.
fn nix_value(val: &serde_json::Value, indent: usize) -> String {
  let pad = "  ".repeat(indent);
  let inner_pad = "  ".repeat(indent + 1);
  match val {
    serde_json::Value::Null => "null".to_string(),
    serde_json::Value::Bool(b) => b.to_string(),
    serde_json::Value::Number(n) => {
      n.as_f64().map_or_else(|| n.to_string(), nix_float)
    }
    serde_json::Value::String(s) => {
      format!("\"{}\"", nix_escape(s))
    }
    serde_json::Value::Array(arr) if arr.is_empty() => "[ ]".to_string(),
    serde_json::Value::Array(arr) => {
      let items: Vec<String> = arr
        .iter()
        .map(|v| format!("{inner_pad}{}", nix_value(v, indent + 1)))
        .collect();
      format!("[\n{}\n{pad}]", items.join("\n"))
    }
    serde_json::Value::Object(map) if map.is_empty() => "{ }".to_string(),
    serde_json::Value::Object(map) => {
      let items: Vec<String> = map
        .iter()
        .map(|(k, v)| {
          format!("{inner_pad}{} = {};", nix_attr(k), nix_value(v, indent + 1),)
        })
        .collect();
      format!("{{\n{}\n{pad}}}", items.join("\n"))
    }
  }
}

/// Convert a top-level JSON object to Nix attribute set body
/// (without outer braces).
fn nix_body(val: &serde_json::Value) -> String {
  match val {
    serde_json::Value::Object(map) => map
      .iter()
      .map(|(k, v)| format!("{} = {};", nix_attr(k), nix_value(v, 0)))
      .collect::<Vec<_>>()
      .join("\n"),
    _ => nix_value(val, 0),
  }
}

#[derive(Debug, Error)]
pub enum ConfigSaveError {
  #[error("Failed to serialize patch {0:?}: {1}")]
  PatchSerialize(String, toml::ser::Error),

  #[error("Failed to serialize heartbeats: {0}")]
  HeartbeatSerialize(toml::ser::Error),

  #[error("Failed to serialize slider ranges: {0}")]
  SliderRangesSerialize(toml::ser::Error),

  #[error("Failed to serialize remote sources: {0}")]
  RemoteSourcesSerialize(toml::ser::Error),

  #[error("Failed to serialize config: {0}")]
  Serialize(toml::ser::Error),

  #[error("Failed to serialize config to JSON: {0}")]
  JsonSerialize(serde_json::Error),
}

/// Returns the path to the `oidc-client-secret` credential file inside
/// systemd's `CREDENTIALS_DIRECTORY`, if the directory is set and the
/// file exists.
fn credential_secret_path() -> Option<PathBuf> {
  let dir = std::env::var("CREDENTIALS_DIRECTORY").ok()?;
  let path = PathBuf::from(dir).join("oidc-client-secret");
  path.exists().then_some(path)
}

#[cfg(test)]
mod tests {
  #![allow(clippy::unwrap_used)]
  use super::*;
  use clap::Parser;
  use sonify_health_lib::heartbeat_config::Playback;
  use sonify_health_lib::probe::ResultMode;

  /// Build a `Config` by writing the given TOML to a tempfile and
  /// invoking the macro-generated `from_cli_and_file` with
  /// `--config <path>`.  Subcommand defaults to `daemon` because
  /// the `Command` field is required and the choice is irrelevant
  /// for these tests.
  fn config_from_toml(toml_str: &str) -> Result<Config, ConfigError> {
    let tmp = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
    std::fs::write(tmp.path(), toml_str).unwrap();
    let cli = CliRaw::try_parse_from([
      "sonify-health",
      "--config",
      tmp.path().to_str().unwrap(),
      "daemon",
    ])
    .unwrap();
    Config::from_cli_and_file(cli)
  }

  /// Build a `Config` with no config file and no extra CLI args
  /// beyond the bare subcommand.
  fn empty_config() -> Result<Config, ConfigError> {
    let cli = CliRaw::try_parse_from(["sonify-health", "daemon"]).unwrap();
    Config::from_cli_and_file(cli)
  }

  /// Build a `Config` from the given TOML plus a list of
  /// `--source name=url` CLI flags.
  fn config_from_toml_and_sources(
    toml_str: &str,
    sources: &[&str],
  ) -> Result<Config, ConfigError> {
    let tmp = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
    std::fs::write(tmp.path(), toml_str).unwrap();
    let mut argv = vec![
      "sonify-health".to_string(),
      "--config".to_string(),
      tmp.path().to_str().unwrap().to_string(),
    ];
    for s in sources {
      argv.push("--source".to_string());
      argv.push((*s).to_string());
    }
    argv.push("daemon".to_string());
    let cli = CliRaw::try_parse_from(argv).unwrap();
    Config::from_cli_and_file(cli)
  }

  /// Build a `Config` from CLI args only (no config file).
  fn config_from_sources(sources: &[&str]) -> Result<Config, ConfigError> {
    let mut argv = vec!["sonify-health".to_string()];
    for s in sources {
      argv.push("--source".to_string());
      argv.push((*s).to_string());
    }
    argv.push("daemon".to_string());
    let cli = CliRaw::try_parse_from(argv).unwrap();
    Config::from_cli_and_file(cli)
  }

  #[test]
  fn heartbeats_section_parses() {
    let toml_str = r#"
      [[heartbeats]]
      name = "gateway"
      command = "ping -c 1 8.8.8.8"
      result_mode = "exit-code"

      [[heartbeats.notes]]
      volume = 0.3
      offset = 0.0

      [heartbeats.notes.transition]
      type = "discrete"

      [[heartbeats.notes.transition.states]]
      threshold = 0.5
      patch = "sine"

      [[heartbeats.notes.transition.states]]
      threshold = 1.01
      patch = "alarm"

      [[heartbeats]]
      name = "cpu"
      command = "echo 0.5"
      result_mode = "stdout"
      playback = "continuous"

      [[heartbeats.notes]]
      volume = 0.2

      [heartbeats.notes.transition]
      type = "gradient"
      patches = ["warm", "sharp"]
    "#;

    let config = config_from_toml(toml_str).unwrap();
    assert_eq!(config.heartbeats.len(), 2);
    assert_eq!(config.heartbeats[0].name, "gateway");
    assert_eq!(config.heartbeats[0].result_mode, ResultMode::ExitCode);
    assert_eq!(config.heartbeats[0].playback, Playback::Clock);
    assert_eq!(config.heartbeats[0].notes.len(), 1);
    assert_eq!(config.heartbeats[1].name, "cpu");
    assert_eq!(config.heartbeats[1].result_mode, ResultMode::Stdout);
    assert_eq!(config.heartbeats[1].playback, Playback::Continuous);
    assert!((config.heartbeats[1].notes[0].volume - 0.2).abs() < f64::EPSILON);
  }

  #[test]
  fn continuous_backcompat() {
    let toml_str = r#"
      [[heartbeats]]
      name = "drone"
      command = "echo 0.5"
      result_mode = "stdout"
      continuous = true

      [[heartbeats.notes]]
      volume = 0.3

      [heartbeats.notes.transition]
      type = "discrete"

      [[heartbeats.notes.transition.states]]
      threshold = 1.01
      patch = "sine"
    "#;

    let config = config_from_toml(toml_str).unwrap();
    assert_eq!(config.heartbeats[0].playback, Playback::Continuous);
  }

  #[test]
  fn playback_defaults_to_clock() {
    let toml_str = r#"
      [[heartbeats]]
      name = "hb"
      command = "echo 0"
      result_mode = "stdout"

      [[heartbeats.notes]]
      volume = 0.3

      [heartbeats.notes.transition]
      type = "discrete"

      [[heartbeats.notes.transition.states]]
      threshold = 1.01
      patch = "sine"
    "#;

    let config = config_from_toml(toml_str).unwrap();
    assert_eq!(config.heartbeats[0].playback, Playback::Clock);
  }

  #[test]
  fn patches_section_parses() {
    let toml_str = r#"
      [patches.my-tone]
      freq = 523.0
      duration = 0.4
      saw_ratio = 1.0
      sine_ratio = 0.0
    "#;

    let config = config_from_toml(toml_str).unwrap();
    let p = &config.library["my-tone"];
    assert_eq!(p.freq, 523.0);
    assert_eq!(p.duration, 0.4);
    assert_eq!(p.saw_ratio, 1.0);
    assert_eq!(p.sine_ratio, 0.0);
    // Unspecified fields use defaults.
    assert_eq!(p.amplitude, 0.3);
  }

  #[test]
  fn override_patch_resolves() {
    let toml_str = r#"
      [patches.base-tone]
      freq = 440.0
      duration = 0.5

      [patches.hi-tone]
      overrides = "base-tone"
      freq = 880.0
    "#;

    let config = config_from_toml(toml_str).unwrap();
    let hi = &config.library["hi-tone"];
    assert_eq!(hi.freq, 880.0);
    // Inherited from base.
    assert_eq!(hi.duration, 0.5);
    assert!(config.overrides.contains_key("hi-tone"));
    assert_eq!(config.overrides["hi-tone"].base, "base-tone");
    assert!(config.overrides["hi-tone"].delta.contains_key("freq"));
  }

  #[test]
  fn round_trip_remote_sources() {
    let toml_str = r#"
[[sources]]
name = "prod-db-1"
url = "wss://db1.example/ws"

[[sources]]
name = "edge-node-2"
url = "ws://edge2.example/ws"
playback_enabled = true
"#;
    let original = config_from_toml(toml_str).unwrap();

    let saved = build_save_toml(
      &original.library,
      &original.overrides,
      &original.heartbeats,
      &original.slider_ranges,
      &original.remote_sources,
    )
    .unwrap();
    let reloaded = config_from_toml(&saved).unwrap();
    assert_eq!(original.remote_sources, reloaded.remote_sources);
  }

  #[test]
  fn config_file_sources_section_parses() {
    let toml_str = r#"
[[sources]]
name = "prod-db-1"
url = "wss://db1.example/ws"

[[sources]]
name = "edge-node-2"
url = "ws://edge2.example/ws"
playback_enabled = true
"#;
    let config = config_from_toml(toml_str).unwrap();
    assert_eq!(config.remote_sources.len(), 2);
    assert_eq!(config.remote_sources[0].name, "prod-db-1");
    assert!(!config.remote_sources[0].playback_enabled);
    assert_eq!(config.remote_sources[1].name, "edge-node-2");
    assert!(config.remote_sources[1].playback_enabled);
  }

  #[test]
  fn cli_and_file_sources_are_merged_and_validated_together() {
    let toml_str = r#"
[[sources]]
name = "from-file"
url = "ws://file/ws"
"#;
    let err =
      config_from_toml_and_sources(toml_str, &["from-file=ws://cli/ws"])
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("more than once"), "{msg}");
  }

  #[test]
  fn cli_source_flag_appends_remote_source() {
    let config = config_from_sources(&["prod-db-1=ws://db1/ws"]).unwrap();
    assert_eq!(config.remote_sources.len(), 1);
    let s = &config.remote_sources[0];
    assert_eq!(s.name, "prod-db-1");
    assert_eq!(s.url, "ws://db1/ws");
    assert!(!s.playback_enabled);
  }

  #[test]
  fn cli_source_flag_rejects_missing_equals() {
    let err = config_from_sources(&["malformed"]).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("name=url"), "{msg}");
  }

  #[test]
  fn remote_source_named_localhost_is_rejected() {
    let err =
      config_from_sources(&["localhost=ws://elsewhere/ws"]).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("reserved"), "{msg}");
  }

  #[test]
  fn remote_source_duplicate_name_is_rejected() {
    let err =
      config_from_sources(&["dup=ws://a/ws", "dup=ws://b/ws"]).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("more than once"), "{msg}");
  }

  #[test]
  fn cli_source_url_with_equals_survives_split() {
    // splitn(2, '=') keeps the rest of the URL intact even when it
    // contains '=' (e.g. a query parameter).
    let config = config_from_sources(&["q=ws://host/ws?token=abc"]).unwrap();
    assert_eq!(config.remote_sources.len(), 1);
    assert_eq!(config.remote_sources[0].name, "q");
    assert_eq!(config.remote_sources[0].url, "ws://host/ws?token=abc");
  }

  #[test]
  fn missing_heartbeats_defaults() {
    let config = empty_config().unwrap();
    assert!(config.heartbeats.is_empty());
  }

  #[test]
  fn library_includes_builtins() {
    let config = empty_config().unwrap();
    assert!(config.library.contains_key("sine"));
    assert!(config.library.contains_key("alarm"));
  }

  #[test]
  fn example_configs_parse() {
    let examples_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
      .parent()
      .unwrap()
      .parent()
      .unwrap()
      .join("examples");
    if !examples_dir.exists() {
      return;
    }
    for entry in
      std::fs::read_dir(&examples_dir).expect("examples directory should exist")
    {
      let path = entry.unwrap().path();
      if path.extension().is_some_and(|e| e == "toml") {
        let contents = std::fs::read_to_string(&path).unwrap();
        let _raw: ConfigFileRaw = toml::from_str(&contents)
          .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
      }
    }
  }

  /// Load a TOML config, serialize via build_save_toml, reload, and
  /// assert that every serializable field survives the round-trip.
  fn assert_config_round_trip(toml_str: &str) {
    let original = config_from_toml(toml_str).unwrap();

    let saved = build_save_toml(
      &original.library,
      &original.overrides,
      &original.heartbeats,
      &original.slider_ranges,
      &original.remote_sources,
    )
    .unwrap();

    let reloaded = config_from_toml(&saved).unwrap();

    // Compare user-defined patches (exclude builtins that weren't
    // in the original config and thus aren't exported).
    let user_patches: PatchLibrary = reloaded
      .library
      .iter()
      .filter(|(name, _)| original.library.contains_key(*name))
      .map(|(k, v)| (k.clone(), v.clone()))
      .collect();
    let orig_patches: PatchLibrary = original.library.clone();
    assert_eq!(
      orig_patches, user_patches,
      "Patch library mismatch after round-trip.\nSaved TOML:\n{saved}"
    );
    assert_eq!(
      original.overrides, reloaded.overrides,
      "Overrides mismatch after round-trip.\nSaved TOML:\n{saved}"
    );
    assert_eq!(
      original.heartbeats, reloaded.heartbeats,
      "Heartbeats mismatch after round-trip.\nSaved TOML:\n{saved}"
    );
    assert_eq!(
      original.slider_ranges, reloaded.slider_ranges,
      "Slider ranges mismatch after round-trip.\nSaved TOML:\n{saved}"
    );
  }

  #[test]
  fn round_trip_discrete_transitions() {
    assert_config_round_trip(
      r#"
        [patches.low-beep]
        freq = 330.0
        duration = 0.3

        [patches.hi-alert]
        freq = 880.0
        duration = 0.2
        saw_ratio = 2.0

        [[heartbeats]]
        name = "net"
        command = "ping -c1 localhost"
        result_mode = "exit-code"
        cycle_secs = 10
        cycle_offset_secs = 2.5

        [[heartbeats.notes]]
        volume = 0.5

        [heartbeats.notes.transition]
        type = "discrete"

        [[heartbeats.notes.transition.states]]
        threshold = 0.5
        patch = "low-beep"

        [[heartbeats.notes.transition.states]]
        threshold = 1.01
        patch = "hi-alert"
      "#,
    );
  }

  #[test]
  fn round_trip_gradient_transitions() {
    assert_config_round_trip(
      r#"
        [patches.my-warm]
        freq = 220.0
        sine_ratio = 2.0
        brightness = 0.5

        [patches.my-sharp]
        freq = 660.0
        saw_ratio = 3.0
        brightness = 1.5

        [[heartbeats]]
        name = "cpu"
        command = "echo 0.5"
        result_mode = "stdout"
        playback = "loop"
        crossfade_ms = 100.0

        [[heartbeats.notes]]
        volume = 0.6
        offset = 0.1

        [heartbeats.notes.transition]
        type = "gradient"
        patches = ["my-warm", "my-sharp"]

        [[heartbeats.notes.transition.segments]]
        strategy = "ease-in"
        intensity = 3.0
      "#,
    );
  }

  #[test]
  fn round_trip_multiple_heartbeats_and_overrides() {
    assert_config_round_trip(
      r#"
        [patches.base-tone]
        freq = 440.0
        amplitude = 0.5
        reverb_mix = 0.3

        [patches.variant]
        overrides = "base-tone"
        freq = 550.0

        [[heartbeats]]
        name = "hb-one"
        command = "echo 0"
        result_mode = "exit-code"
        playback = "continuous"
        poll_interval_secs = 5.0

        [[heartbeats.notes]]
        volume = 0.4

        [heartbeats.notes.transition]
        type = "discrete"

        [[heartbeats.notes.transition.states]]
        threshold = 1.01
        patch = "base-tone"

        [[heartbeats]]
        name = "hb-two"
        command = "exit 0"
        result_mode = "exit-code"
        cycle_secs = 30

        [[heartbeats.notes]]
        volume = 0.3
        offset = 0.2

        [heartbeats.notes.transition]
        type = "discrete"

        [[heartbeats.notes.transition.states]]
        threshold = 0.5
        patch = "variant"

        [[heartbeats.notes.transition.states]]
        threshold = 1.01
        patch = "alarm"

        [[heartbeats.notes]]
        volume = 0.2

        [heartbeats.notes.transition]
        type = "discrete"

        [[heartbeats.notes.transition.states]]
        threshold = 1.01
        patch = "sine"
      "#,
    );
  }

  #[test]
  fn round_trip_custom_slider_ranges() {
    assert_config_round_trip(
      r#"
        [slider_ranges.master_volume]
        min = 0.0
        max = 3.0
        step = 0.1

        [slider_ranges.note_offset]
        min = 0.0
        max = 10.0
        step = 0.5

        [[heartbeats]]
        name = "hb"
        command = "echo 0"
        result_mode = "stdout"

        [[heartbeats.notes]]

        [heartbeats.notes.transition]
        type = "discrete"

        [[heartbeats.notes.transition.states]]
        threshold = 1.01
        patch = "sine"
      "#,
    );
  }

  #[test]
  fn crossfade_ms_parses_from_toml() {
    let toml_str = r#"
      [[heartbeats]]
      name = "hb"
      command = "echo 0"
      result_mode = "stdout"
      crossfade_ms = 200.0

      [[heartbeats.notes]]
      volume = 0.3

      [heartbeats.notes.transition]
      type = "discrete"

      [[heartbeats.notes.transition.states]]
      threshold = 1.01
      patch = "sine"
    "#;

    let config = config_from_toml(toml_str).unwrap();
    assert!((config.heartbeats[0].crossfade_ms - 200.0).abs() < f64::EPSILON);
  }

  #[test]
  fn crossfade_ms_defaults() {
    let toml_str = r#"
      [[heartbeats]]
      name = "hb"
      command = "echo 0"
      result_mode = "stdout"

      [[heartbeats.notes]]
      volume = 0.3

      [heartbeats.notes.transition]
      type = "discrete"

      [[heartbeats.notes.transition.states]]
      threshold = 1.01
      patch = "sine"
    "#;

    let config = config_from_toml(toml_str).unwrap();
    assert!((config.heartbeats[0].crossfade_ms - 6.0).abs() < f64::EPSILON);
  }

  #[test]
  fn config_writable_flag() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = std::env::temp_dir().join("sonify_writable_test.toml");
    std::fs::write(&tmp, "").unwrap();

    // Make read-only.
    let mut perms = std::fs::metadata(&tmp).unwrap().permissions();
    perms.set_mode(0o444);
    std::fs::set_permissions(&tmp, perms).unwrap();

    let readonly_flag =
      std::fs::metadata(&tmp).is_ok_and(|m| !m.permissions().readonly());
    assert!(!readonly_flag, "Should be non-writable");

    // Make writable again.
    let mut perms = std::fs::metadata(&tmp).unwrap().permissions();
    perms.set_mode(0o644);
    std::fs::set_permissions(&tmp, perms).unwrap();

    let writable_flag =
      std::fs::metadata(&tmp).is_ok_and(|m| !m.permissions().readonly());
    assert!(writable_flag, "Should be writable");

    std::fs::remove_file(&tmp).ok();
  }
}
