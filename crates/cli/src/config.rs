use serde::{Deserialize, Serialize};
use sonify_health_lib::{
  builtin_library, HeartbeatConfig, LogFormat, LogLevel, Patch, PatchLibrary,
  PatchOverrides,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;
use tokio_listener::ListenerAddress;

// Shared leaf types live in the lib so the cli and server can
// agree on them without one binary depending on the other.
// Re-exported here so existing call sites inside cli (and other
// crates that reach into cli's config) keep compiling unchanged
// during the workspace split.
pub use sonify_health_lib::config::{
  ConfigError, OverrideInfo, SliderRange, SliderRanges,
};

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

/// Fully resolved OIDC configuration.
#[derive(Debug, Clone)]
pub struct OidcConfig {
  pub base_url: String,
  pub issuer: String,
  pub client_id: String,
  pub client_secret: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfigFileRaw {
  log_level: Option<String>,
  log_format: Option<String>,
  listen: Option<String>,
  audio_device: Option<String>,
  headless: Option<bool>,
  frontend_path: Option<PathBuf>,
  #[serde(default)]
  patches: HashMap<String, toml::Value>,
  #[serde(default)]
  heartbeats: Vec<HeartbeatConfig>,
  #[serde(default)]
  slider_ranges: SliderRanges,
  oidc: Option<OidcSectionRaw>,
  #[serde(default)]
  sources: Vec<RemoteSourceConfig>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct OidcSectionRaw {
  base_url: Option<String>,
  issuer: Option<String>,
  client_id: Option<String>,
  client_secret_file: Option<PathBuf>,
}

impl ConfigFileRaw {
  fn from_file(path: &Path) -> Result<Self, ConfigError> {
    let contents = std::fs::read_to_string(path).map_err(|source| {
      ConfigError::FileRead {
        path: path.to_path_buf(),
        source,
      }
    })?;

    toml::from_str(&contents).map_err(|source| ConfigError::Parse {
      path: path.to_path_buf(),
      source: Box::new(source),
    })
  }
}

#[derive(Debug)]
pub struct Config {
  pub log_level: LogLevel,
  pub log_format: LogFormat,
  pub listen_address: ListenerAddress,
  pub audio_device: Option<String>,
  /// When true, the daemon never opens an audio device and never
  /// spawns play threads.  Pollers, the WebSocket server, the
  /// frontend, and metrics keep working — the instance is a pure
  /// state producer, intended for speakerless servers.
  pub headless: bool,
  pub frontend_path: PathBuf,
  pub library: PatchLibrary,
  pub overrides: HashMap<String, OverrideInfo>,
  pub heartbeats: Vec<HeartbeatConfig>,
  pub slider_ranges: SliderRanges,
  pub oidc: Option<OidcConfig>,
  pub config_path: Option<PathBuf>,
  /// Remote source declarations.  CLI `--source` flags are merged
  /// with the file's `[[sources]]` array (CLI entries appended
  /// after file entries); name uniqueness is enforced across the
  /// merged set.
  pub remote_sources: Vec<RemoteSourceConfig>,
}

impl Config {
  /// Build a validated configuration by merging CLI
  /// arguments with an optional config file.
  #[allow(clippy::too_many_arguments)]
  pub fn from_args(
    log_level: Option<&str>,
    log_format: Option<&str>,
    listen: Option<&str>,
    frontend_path: Option<&Path>,
    config_path: Option<&Path>,
    patch_libraries: &[PathBuf],
    base_url: Option<&str>,
    oidc_issuer: Option<&str>,
    oidc_client_id: Option<&str>,
    oidc_client_secret_file: Option<&Path>,
    headless: Option<bool>,
    remote_source_args: &[String],
  ) -> Result<Self, ConfigError> {
    let (file, resolved_config_path) = if let Some(p) = config_path {
      (ConfigFileRaw::from_file(p)?, Some(p.to_path_buf()))
    } else {
      let default = PathBuf::from("config.toml");
      if default.exists() {
        (ConfigFileRaw::from_file(&default)?, Some(default))
      } else {
        (ConfigFileRaw::default(), None)
      }
    };

    let log_level = log_level
      .or(file.log_level.as_deref())
      .unwrap_or("info")
      .parse::<LogLevel>()
      .map_err(|e| ConfigError::Validation(e.to_string()))?;

    let log_format = log_format
      .or(file.log_format.as_deref())
      .unwrap_or("text")
      .parse::<LogFormat>()
      .map_err(|e| ConfigError::Validation(e.to_string()))?;

    let listen_address = listen
      .or(file.listen.as_deref())
      .unwrap_or("127.0.0.1:3000")
      .parse::<ListenerAddress>()
      .map_err(|e| ConfigError::Validation(e.to_string()))?;

    let frontend_path = frontend_path
      .map(PathBuf::from)
      .or(file.frontend_path)
      .unwrap_or_else(|| PathBuf::from("frontend/public"));

    // Build patch library: builtins, then config patches, then extra
    // file patches (each layer wins on collision).  Two-pass: first
    // standalone patches, then override patches that reference them.
    let mut library = builtin_library();
    let mut override_entries: Vec<(String, String, toml::Value)> = Vec::new();

    for (name, mut table) in file.patches {
      if let Some(base_val) =
        table.as_table_mut().and_then(|t| t.remove("overrides"))
      {
        let base = base_val
          .as_str()
          .ok_or_else(|| {
            ConfigError::Validation(format!(
              "patch {name:?}: 'overrides' must be a string"
            ))
          })?
          .to_string();
        override_entries.push((name, base, table));
      } else {
        let patch: Patch =
          table.try_into().map_err(|source| ConfigError::PatchParse {
            name: name.clone(),
            source: Box::new(source),
          })?;
        library.insert(name, patch);
      }
    }

    // Patch library files (CLI flag, repeatable).  Last-in wins
    // for overlapping names; the main config file's patches (above)
    // have already been inserted and will be overwritten by library
    // files.  Config-file patches are re-inserted in the override
    // pass below, so the main config always wins.
    for path in patch_libraries {
      let contents = std::fs::read_to_string(path).map_err(|source| {
        ConfigError::PatchLibraryRead {
          path: path.clone(),
          source,
        }
      })?;
      let extra: HashMap<String, Patch> =
        toml::from_str(&contents).map_err(|source| {
          ConfigError::PatchLibraryParse {
            path: path.clone(),
            source: Box::new(source),
          }
        })?;
      for (name, patch) in extra {
        library.insert(name, patch);
      }
    }

    // Second pass: resolve override patches.
    let mut overrides = HashMap::new();
    for (name, base, table) in override_entries {
      if !library.contains_key(&base) {
        return Err(ConfigError::OverrideBaseMissing { name, base });
      }
      if overrides.contains_key(&base) {
        return Err(ConfigError::OverrideChained { name, base });
      }
      let parsed: PatchOverrides =
        table.try_into().map_err(|source| ConfigError::PatchParse {
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

    // OIDC.
    let oidc_file = file.oidc.unwrap_or_default();
    let oidc_base = base_url.map(String::from).or(oidc_file.base_url);
    let oidc_iss = oidc_issuer.map(String::from).or(oidc_file.issuer);
    let oidc_cid = oidc_client_id.map(String::from).or(oidc_file.client_id);
    let oidc_sf = oidc_client_secret_file
      .map(PathBuf::from)
      .or(oidc_file.client_secret_file);

    let oidc = match (&oidc_base, &oidc_iss, &oidc_cid) {
      (None, None, None) if oidc_sf.is_none() => None,
      (Some(base), Some(iss), Some(cid)) => {
        let secret_file =
          oidc_sf.or_else(credential_secret_path).ok_or_else(|| {
            ConfigError::Validation(
              "oidc_client_secret_file is required when oidc_issuer and \
               oidc_client_id are set (set it explicitly or run under \
               systemd with LoadCredential)"
                .to_string(),
            )
          })?;

        let secret = std::fs::read_to_string(&secret_file)
          .map(|s| s.trim().to_string())
          .map_err(|source| ConfigError::OidcSecretFileRead {
            path: secret_file,
            source,
          })?;
        Some(OidcConfig {
          base_url: base.clone(),
          issuer: iss.clone(),
          client_id: cid.clone(),
          client_secret: secret,
        })
      }
      _ => {
        let mut present = Vec::new();
        let mut missing = Vec::new();
        for (name, val) in [
          ("base_url", oidc_base.is_some()),
          ("oidc_issuer", oidc_iss.is_some()),
          ("oidc_client_id", oidc_cid.is_some()),
          (
            "oidc_client_secret_file",
            oidc_sf.is_some() || credential_secret_path().is_some(),
          ),
        ] {
          if val {
            present.push(name);
          } else {
            missing.push(name);
          }
        }
        return Err(ConfigError::Validation(format!(
          "partial OIDC configuration: set all four fields or none. \
           present: [{}], missing: [{}]",
          present.join(", "),
          missing.join(", ")
        )));
      }
    };

    let mut heartbeats = file.heartbeats;
    for hb in &mut heartbeats {
      hb.resolve_legacy_continuous();
    }

    // Remote sources: file's [[sources]] first, then any CLI
    // --source flags appended.  Each CLI flag is `name=url`,
    // splitting on the first `=` so URLs containing `=` survive.
    let mut remote_sources = file.sources;
    for raw in remote_source_args {
      let (name, url) = raw.split_once('=').ok_or_else(|| {
        ConfigError::Validation(format!(
          "--source argument {raw:?} must be of the form name=url"
        ))
      })?;
      if name.is_empty() || url.is_empty() {
        return Err(ConfigError::Validation(format!(
          "--source argument {raw:?} has empty name or url"
        )));
      }
      remote_sources.push(RemoteSourceConfig {
        name: name.to_string(),
        url: url.to_string(),
        playback_enabled: false,
      });
    }
    // Mirrors preview_state::LOCAL_SOURCE_NAME — kept inline to
    // avoid a cross-module constant reference for a value that
    // will never change.
    const RESERVED_LOCAL_NAME: &str = "localhost";
    let mut seen = std::collections::HashSet::new();
    for s in &remote_sources {
      if s.name == RESERVED_LOCAL_NAME {
        return Err(ConfigError::Validation(format!(
          "remote source name {:?} is reserved for the Local Source",
          s.name
        )));
      }
      if !seen.insert(s.name.as_str()) {
        return Err(ConfigError::Validation(format!(
          "remote source name {:?} is declared more than once",
          s.name
        )));
      }
    }

    Ok(Config {
      log_level,
      log_format,
      listen_address,
      audio_device: file.audio_device,
      headless: headless.or(file.headless).unwrap_or(false),
      frontend_path,
      library,
      overrides,
      heartbeats,
      slider_ranges: file.slider_ranges,
      oidc,
      config_path: resolved_config_path,
      remote_sources,
    })
  }
}

/// Serialize the current runtime state to a TOML config string that
/// can be loaded back via `Config::from_args`.  Override patches emit
/// the compact `overrides = "base"` form with only delta fields;
/// standalone patches serialize as full `Patch` tables.  Builtin
/// patches are omitted.
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
      // Override patch: emit compact form.
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
      // Standalone patch: full serialization.
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
  use super::*;
  use sonify_health_lib::heartbeat_config::Playback;
  use sonify_health_lib::probe::ResultMode;
  use sonify_health_lib::transition::{DiscreteState, Transition};
  use sonify_health_lib::NoteConfig;

  #[test]
  fn heartbeats_section_parses() {
    let toml = r#"
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

    let raw: ConfigFileRaw = toml::from_str(toml).unwrap();
    assert_eq!(raw.heartbeats.len(), 2);
    assert_eq!(raw.heartbeats[0].name, "gateway");
    assert_eq!(raw.heartbeats[0].result_mode, ResultMode::ExitCode);
    assert_eq!(raw.heartbeats[0].playback, Playback::Clock);
    assert_eq!(raw.heartbeats[0].notes.len(), 1);
    assert_eq!(raw.heartbeats[1].name, "cpu");
    assert_eq!(raw.heartbeats[1].result_mode, ResultMode::Stdout);
    assert_eq!(raw.heartbeats[1].playback, Playback::Continuous);
    assert!((raw.heartbeats[1].notes[0].volume - 0.2).abs() < f64::EPSILON);
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

    let mut raw: ConfigFileRaw = toml::from_str(toml_str).unwrap();
    assert_eq!(raw.heartbeats[0].playback, Playback::Clock);
    raw.heartbeats[0].resolve_legacy_continuous();
    assert_eq!(raw.heartbeats[0].playback, Playback::Continuous);
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

    let raw: ConfigFileRaw = toml::from_str(toml_str).unwrap();
    assert_eq!(raw.heartbeats[0].playback, Playback::Clock);
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

    let raw: ConfigFileRaw = toml::from_str(toml_str).unwrap();
    assert_eq!(raw.patches.len(), 1);
    let p: Patch = raw.patches["my-tone"].clone().try_into().unwrap();
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

    let raw: ConfigFileRaw = toml::from_str(toml_str).unwrap();
    assert_eq!(raw.patches.len(), 2);

    // Verify via full config parsing that the override resolves.
    let cfg_path = std::env::temp_dir().join("sonify_test_override.toml");
    std::fs::write(&cfg_path, toml_str).unwrap();
    let config = Config::from_args(
      None,
      None,
      None,
      None,
      Some(cfg_path.as_path()),
      &[],
      None,
      None,
      None,
      None,
      None,
      &[],
    )
    .unwrap();
    std::fs::remove_file(&cfg_path).ok();
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
    let tmp = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
    let cfg_path = tmp.path().to_path_buf();
    std::fs::write(
      &cfg_path,
      r#"
[[sources]]
name = "prod-db-1"
url = "wss://db1.example/ws"

[[sources]]
name = "edge-node-2"
url = "ws://edge2.example/ws"
playback_enabled = true
"#,
    )
    .unwrap();
    let original = Config::from_args(
      None,
      None,
      None,
      None,
      Some(cfg_path.as_path()),
      &[],
      None,
      None,
      None,
      None,
      None,
      &[],
    )
    .unwrap();
    std::fs::remove_file(&cfg_path).ok();

    let saved = build_save_toml(
      &original.library,
      &original.overrides,
      &original.heartbeats,
      &original.slider_ranges,
      &original.remote_sources,
    )
    .unwrap();

    let tmp2 = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
    let cfg2 = tmp2.path().to_path_buf();
    std::fs::write(&cfg2, saved).unwrap();
    let reloaded = Config::from_args(
      None,
      None,
      None,
      None,
      Some(cfg2.as_path()),
      &[],
      None,
      None,
      None,
      None,
      None,
      &[],
    )
    .unwrap();
    std::fs::remove_file(&cfg2).ok();

    assert_eq!(original.remote_sources, reloaded.remote_sources);
  }

  #[test]
  fn config_file_sources_section_parses() {
    let tmp = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
    let cfg_path = tmp.path().to_path_buf();
    std::fs::write(
      &cfg_path,
      r#"
[[sources]]
name = "prod-db-1"
url = "wss://db1.example/ws"

[[sources]]
name = "edge-node-2"
url = "ws://edge2.example/ws"
playback_enabled = true
"#,
    )
    .unwrap();
    let config = Config::from_args(
      None,
      None,
      None,
      None,
      Some(cfg_path.as_path()),
      &[],
      None,
      None,
      None,
      None,
      None,
      &[],
    )
    .unwrap();
    std::fs::remove_file(&cfg_path).ok();
    assert_eq!(config.remote_sources.len(), 2);
    assert_eq!(config.remote_sources[0].name, "prod-db-1");
    assert!(!config.remote_sources[0].playback_enabled);
    assert_eq!(config.remote_sources[1].name, "edge-node-2");
    assert!(config.remote_sources[1].playback_enabled);
  }

  #[test]
  fn cli_and_file_sources_are_merged_and_validated_together() {
    let tmp = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
    let cfg_path = tmp.path().to_path_buf();
    std::fs::write(
      &cfg_path,
      r#"
[[sources]]
name = "from-file"
url = "ws://file/ws"
"#,
    )
    .unwrap();
    let err = Config::from_args(
      None,
      None,
      None,
      None,
      Some(cfg_path.as_path()),
      &[],
      None,
      None,
      None,
      None,
      None,
      &["from-file=ws://cli/ws".to_string()],
    )
    .unwrap_err();
    std::fs::remove_file(&cfg_path).ok();
    let msg = format!("{err}");
    assert!(msg.contains("more than once"), "{msg}");
  }

  #[test]
  fn cli_source_flag_appends_remote_source() {
    let config = Config::from_args(
      None,
      None,
      None,
      None,
      None,
      &[],
      None,
      None,
      None,
      None,
      None,
      &["prod-db-1=ws://db1/ws".to_string()],
    )
    .unwrap();
    assert_eq!(config.remote_sources.len(), 1);
    let s = &config.remote_sources[0];
    assert_eq!(s.name, "prod-db-1");
    assert_eq!(s.url, "ws://db1/ws");
    assert!(!s.playback_enabled);
  }

  #[test]
  fn cli_source_flag_rejects_missing_equals() {
    let err = Config::from_args(
      None,
      None,
      None,
      None,
      None,
      &[],
      None,
      None,
      None,
      None,
      None,
      &["malformed".to_string()],
    )
    .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("name=url"), "{msg}");
  }

  #[test]
  fn remote_source_named_localhost_is_rejected() {
    let err = Config::from_args(
      None,
      None,
      None,
      None,
      None,
      &[],
      None,
      None,
      None,
      None,
      None,
      &["localhost=ws://elsewhere/ws".to_string()],
    )
    .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("reserved"), "{msg}");
  }

  #[test]
  fn remote_source_duplicate_name_is_rejected() {
    let err = Config::from_args(
      None,
      None,
      None,
      None,
      None,
      &[],
      None,
      None,
      None,
      None,
      None,
      &["dup=ws://a/ws".to_string(), "dup=ws://b/ws".to_string()],
    )
    .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("more than once"), "{msg}");
  }

  #[test]
  fn cli_source_url_with_equals_survives_split() {
    // splitn(2, '=') keeps the rest of the URL intact even when it
    // contains '=' (e.g. a query parameter).
    let config = Config::from_args(
      None,
      None,
      None,
      None,
      None,
      &[],
      None,
      None,
      None,
      None,
      None,
      &["q=ws://host/ws?token=abc".to_string()],
    )
    .unwrap();
    assert_eq!(config.remote_sources.len(), 1);
    assert_eq!(config.remote_sources[0].name, "q");
    assert_eq!(config.remote_sources[0].url, "ws://host/ws?token=abc");
  }

  #[test]
  fn missing_heartbeats_defaults() {
    let config = Config::from_args(
      None,
      None,
      None,
      None,
      None,
      &[],
      None,
      None,
      None,
      None,
      None,
      &[],
    )
    .unwrap();
    assert!(config.heartbeats.is_empty());
  }

  #[test]
  fn library_includes_builtins() {
    let config = Config::from_args(
      None,
      None,
      None,
      None,
      None,
      &[],
      None,
      None,
      None,
      None,
      None,
      &[],
    )
    .unwrap();
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

  #[test]
  fn save_round_trip() {
    // Build a config with a standalone patch, an override, a
    // heartbeat, and non-default slider ranges.
    let toml_str = r#"
      [patches.my-base]
      freq = 440.0
      duration = 0.5

      [patches.my-override]
      overrides = "my-base"
      freq = 880.0

      [slider_ranges.master_volume]
      min = 0.0
      max = 2.0
      step = 0.05

      [[heartbeats]]
      name = "test-hb"
      command = "echo 0"
      result_mode = "exit-code"
      playback = "continuous"

      [[heartbeats.notes]]
      volume = 0.4

      [heartbeats.notes.transition]
      type = "discrete"

      [[heartbeats.notes.transition.states]]
      threshold = 0.5
      patch = "my-base"

      [[heartbeats.notes.transition.states]]
      threshold = 1.01
      patch = "my-override"
    "#;

    let tmp = std::env::temp_dir().join("sonify_save_rt.toml");
    std::fs::write(&tmp, toml_str).unwrap();

    let config = Config::from_args(
      None,
      None,
      None,
      None,
      Some(tmp.as_path()),
      &[],
      None,
      None,
      None,
      None,
      None,
      &[],
    )
    .unwrap();

    // Mutate: change a param on the override delta and on a
    // standalone patch.
    let mut library = config.library.clone();
    let mut overrides = config.overrides.clone();
    library.get_mut("my-base").unwrap().duration = 0.8;
    library.get_mut("my-override").unwrap().duration = 0.8;
    overrides
      .get_mut("my-override")
      .unwrap()
      .delta
      .insert("amplitude".to_string(), 0.9);
    library.get_mut("my-override").unwrap().amplitude = 0.9;

    // Serialize and reload.
    let saved = build_save_toml(
      &library,
      &overrides,
      &config.heartbeats,
      &config.slider_ranges,
      &config.remote_sources,
    )
    .unwrap();

    let tmp2 = std::env::temp_dir().join("sonify_save_rt2.toml");
    std::fs::write(&tmp2, &saved).unwrap();

    let reloaded = Config::from_args(
      None,
      None,
      None,
      None,
      Some(tmp2.as_path()),
      &[],
      None,
      None,
      None,
      None,
      None,
      &[],
    )
    .unwrap();

    // Verify patches.
    assert_eq!(reloaded.library["my-base"].freq, 440.0);
    assert_eq!(reloaded.library["my-base"].duration, 0.8);
    assert_eq!(reloaded.library["my-override"].freq, 880.0);
    assert_eq!(reloaded.library["my-override"].amplitude, 0.9);
    // Inherited duration from mutated base.
    assert_eq!(reloaded.library["my-override"].duration, 0.8);

    // Verify override delta.
    assert!(reloaded.overrides.contains_key("my-override"));
    assert_eq!(reloaded.overrides["my-override"].base, "my-base");
    assert!(reloaded.overrides["my-override"].delta.contains_key("freq"));
    assert!(reloaded.overrides["my-override"]
      .delta
      .contains_key("amplitude"));

    // Verify heartbeat.
    assert_eq!(reloaded.heartbeats.len(), 1);
    assert_eq!(reloaded.heartbeats[0].name, "test-hb");
    assert_eq!(reloaded.heartbeats[0].playback, Playback::Continuous);

    // Verify slider ranges.
    assert_eq!(reloaded.slider_ranges.master_volume.max, 2.0);
    assert_eq!(reloaded.slider_ranges.master_volume.step, 0.05);

    std::fs::remove_file(&tmp).ok();
    std::fs::remove_file(&tmp2).ok();
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

    let raw: ConfigFileRaw = toml::from_str(toml_str).unwrap();
    assert!((raw.heartbeats[0].crossfade_ms - 200.0).abs() < f64::EPSILON);
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

    let raw: ConfigFileRaw = toml::from_str(toml_str).unwrap();
    assert!((raw.heartbeats[0].crossfade_ms - 6.0).abs() < f64::EPSILON);
  }

  /// Load a TOML config, serialize via build_save_toml, reload, and
  /// assert that every serializable field survives the round-trip.
  fn assert_config_round_trip(toml_str: &str) {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), toml_str).unwrap();

    let original = Config::from_args(
      None,
      None,
      None,
      None,
      Some(tmp.path()),
      &[],
      None,
      None,
      None,
      None,
      None,
      &[],
    )
    .unwrap();

    let saved = build_save_toml(
      &original.library,
      &original.overrides,
      &original.heartbeats,
      &original.slider_ranges,
      &original.remote_sources,
    )
    .unwrap();

    let tmp2 = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp2.path(), &saved).unwrap();

    let reloaded = Config::from_args(
      None,
      None,
      None,
      None,
      Some(tmp2.path()),
      &[],
      None,
      None,
      None,
      None,
      None,
      &[],
    )
    .unwrap();

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
  fn round_trip_with_mutations() {
    let toml_str = r#"
      [patches.my-patch]
      freq = 440.0
      duration = 0.5

      [[heartbeats]]
      name = "test"
      command = "echo 0"
      result_mode = "exit-code"

      [[heartbeats.notes]]
      volume = 0.4

      [heartbeats.notes.transition]
      type = "discrete"

      [[heartbeats.notes.transition.states]]
      threshold = 1.01
      patch = "my-patch"
    "#;

    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), toml_str).unwrap();

    let config = Config::from_args(
      None,
      None,
      None,
      None,
      Some(tmp.path()),
      &[],
      None,
      None,
      None,
      None,
      None,
      &[],
    )
    .unwrap();

    // Mutate: change patch params, note volume, add a note.
    let mut library = config.library.clone();
    library.get_mut("my-patch").unwrap().freq = 550.0;
    library.get_mut("my-patch").unwrap().detune = 10.0;

    let mut heartbeats = config.heartbeats.clone();
    heartbeats[0].notes[0].volume = 0.8;
    heartbeats[0].notes.push(NoteConfig {
      transition: Transition::Discrete {
        states: vec![DiscreteState {
          threshold: 1.01,
          patch: "sine".to_string(),
        }],
      },
      volume: 0.2,
      offset: 0.3,
    });

    let saved = build_save_toml(
      &library,
      &config.overrides,
      &heartbeats,
      &config.slider_ranges,
      &config.remote_sources,
    )
    .unwrap();

    let tmp2 = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp2.path(), &saved).unwrap();

    let reloaded = Config::from_args(
      None,
      None,
      None,
      None,
      Some(tmp2.path()),
      &[],
      None,
      None,
      None,
      None,
      None,
      &[],
    )
    .unwrap();

    assert_eq!(reloaded.library["my-patch"].freq, 550.0);
    assert_eq!(reloaded.library["my-patch"].detune, 10.0);
    assert_eq!(reloaded.heartbeats[0].notes.len(), 2);
    assert_eq!(reloaded.heartbeats[0].notes[0].volume, 0.8);
    assert_eq!(reloaded.heartbeats[0].notes[1].volume, 0.2);
    assert_eq!(reloaded.heartbeats[0].notes[1].offset, 0.3);
  }
}
