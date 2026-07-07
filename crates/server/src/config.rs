//! Server-side configuration.  Plugs into foundation's
//! `MergeConfig` derive, exposes `ServerApp` so the foundation
//! `Server` runner can stand up listeners + OIDC discovery from
//! this struct, and owns the runtime-state-to-config-file
//! serialization helpers used by the WebSocket "save_config"
//! handler.

use rust_template_foundation::auth::OidcConfig;
use rust_template_foundation::logging::{LogFormat, LogLevel};
use rust_template_foundation::server::runner::{ServerApp, ServerRunConfig};
use rust_template_foundation::{CliApp, MergeConfig};
use sonify_health_lib::config::{
  ConfigError as LibConfigError, OverrideInfo, RemoteSourceConfig, SliderRanges,
};
use sonify_health_lib::{
  builtin_library, HeartbeatConfig, Patch, PatchLibrary, PatchOverrides,
};
use std::collections::HashMap;
use std::path::PathBuf;
use thiserror::Error;
use tokio_listener::ListenerAddress;

// ── extra_cli / extra_file plug-ins ──────────────────────────────────

/// CLI-only arguments that are inputs to skip-field resolvers
/// (`resolve_library`, `resolve_remote_sources`, `resolve_oidc`)
/// rather than direct fields on `Config`.  Flattened into the
/// macro-generated `CliRaw` via `extra_cli`.
#[derive(Debug, clap::Args)]
pub struct ServerCliFields {
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

  /// OIDC issuer URL for provider discovery.
  #[arg(long, env)]
  pub oidc_issuer: Option<String>,

  /// OIDC client ID.
  #[arg(long, env)]
  pub oidc_client_id: Option<String>,

  /// Path to a file containing the OIDC client secret.
  #[arg(long, env)]
  pub oidc_client_secret_file: Option<PathBuf>,
}

// ── Config ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, MergeConfig)]
#[merge_config(
  app_name = "sonify-health",
  extra_cli = "ServerCliFields",
  extra_file = "sonify_health_lib::config::SonifyFileFields",
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

  /// Base URL of this service (e.g. https://sonify.example.com),
  /// used by foundation to construct the OIDC redirect URI and
  /// shared with the OIDC handlers.
  #[merge_config(env, default = "\"http://localhost:3000\".to_string()")]
  pub base_url: String,

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

    match (&issuer, &client_id) {
      (None, None) if secret_file.is_none() => Ok(None),
      (Some(iss), Some(cid)) => {
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
          issuer: iss.clone(),
          client_id: cid.clone(),
          client_secret,
        }))
      }
      _ => {
        let mut present = Vec::new();
        let mut missing = Vec::new();
        for (name, val) in [
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
          "partial OIDC configuration: set all three fields or none. \
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

impl ServerApp for Config {
  fn server_run_configs(&self) -> Vec<ServerRunConfig> {
    vec![ServerRunConfig {
      app_name: Self::app_name().to_string(),
      listen_address: self.listen_address.clone(),
      base_url: self.base_url.clone(),
      oidc: self.oidc.clone(),
    }]
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

// ── save-back serializers used by the websocket save_config handler ─

/// Serialize the current runtime state to a TOML config string that
/// can be loaded back via `Config::from_cli_and_file`.
pub fn build_save_toml(
  library: &PatchLibrary,
  overrides: &HashMap<String, OverrideInfo>,
  heartbeats: &[HeartbeatConfig],
  slider_ranges: &SliderRanges,
  remote_sources: &[RemoteSourceConfig],
) -> Result<String, ConfigSaveError> {
  let builtins = builtin_library();
  let mut doc = toml::Table::new();

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

  let hb_val = toml::Value::try_from(heartbeats)
    .map_err(ConfigSaveError::HeartbeatSerialize)?;
  if let toml::Value::Array(ref arr) = hb_val {
    if !arr.is_empty() {
      doc.insert("heartbeats".to_string(), hb_val);
    }
  }

  let default_ranges = SliderRanges::default();
  let sr_val = toml::Value::try_from(slider_ranges)
    .map_err(ConfigSaveError::SliderRangesSerialize)?;
  let default_sr_val = toml::Value::try_from(&default_ranges)
    .map_err(ConfigSaveError::SliderRangesSerialize)?;
  if sr_val != default_sr_val {
    doc.insert("slider_ranges".to_string(), sr_val);
  }

  if !remote_sources.is_empty() {
    let val = toml::Value::try_from(remote_sources)
      .map_err(ConfigSaveError::RemoteSourcesSerialize)?;
    doc.insert("sources".to_string(), val);
  }

  toml::to_string_pretty(&doc).map_err(ConfigSaveError::Serialize)
}

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

fn nix_float(v: f64) -> String {
  let s = v.to_string();
  if s.contains('.') || s.contains('e') || s.contains('E') {
    s
  } else {
    format!("{s}.0")
  }
}

fn nix_escape(s: &str) -> String {
  s.replace('\\', "\\\\")
    .replace('"', "\\\"")
    .replace("${", "\\${")
}

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

/// Returns the path to the `oidc-client-secret` credential file
/// inside systemd's `CREDENTIALS_DIRECTORY`, if the directory is
/// set and the file exists.
fn credential_secret_path() -> Option<PathBuf> {
  let dir = std::env::var("CREDENTIALS_DIRECTORY").ok()?;
  let path = PathBuf::from(dir).join("oidc-client-secret");
  path.exists().then_some(path)
}
