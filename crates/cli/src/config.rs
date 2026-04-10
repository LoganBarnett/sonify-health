use serde::{Deserialize, Serialize};
use sonify_health_lib::{
  builtin_library, HeartbeatConfig, LogFormat, LogLevel, Patch, PatchLibrary,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;
use tokio_listener::ListenerAddress;

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
    source: toml::de::Error,
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
    "Failed to read extra patches file at {path:?}: \
     {source}"
  )]
  ExtraPatchesRead {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },

  #[error(
    "Failed to parse extra patches file at {path:?}: \
     {source}"
  )]
  ExtraPatchesParse {
    path: PathBuf,
    #[source]
    source: toml::de::Error,
  },
}

/// Fully resolved OIDC configuration.
#[derive(Debug, Clone)]
pub struct OidcConfig {
  pub base_url: String,
  pub issuer: String,
  pub client_id: String,
  pub client_secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SliderRange {
  pub min: f64,
  pub max: f64,
  pub step: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SliderRanges {
  pub master_volume: SliderRange,
  pub cycle_offset: SliderRange,
  pub override_metric: SliderRange,
  pub note_volume: SliderRange,
  pub note_offset: SliderRange,
  pub gradient_curve: SliderRange,
  pub discrete_threshold: SliderRange,
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
      gradient_curve: SliderRange {
        min: 0.1,
        max: 10.0,
        step: 0.1,
      },
      discrete_threshold: SliderRange {
        min: 0.0,
        max: 1.0,
        step: 0.01,
      },
    }
  }
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfigFileRaw {
  log_level: Option<String>,
  log_format: Option<String>,
  listen: Option<String>,
  audio_device: Option<String>,
  frontend_path: Option<PathBuf>,
  extra_patches_file: Option<PathBuf>,
  #[serde(default)]
  patches: HashMap<String, Patch>,
  #[serde(default)]
  heartbeats: Vec<HeartbeatConfig>,
  #[serde(default)]
  slider_ranges: SliderRanges,
  oidc: Option<OidcSectionRaw>,
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
      source,
    })
  }
}

#[derive(Debug)]
pub struct Config {
  pub log_level: LogLevel,
  pub log_format: LogFormat,
  pub listen_address: ListenerAddress,
  pub audio_device: Option<String>,
  pub frontend_path: PathBuf,
  pub library: PatchLibrary,
  pub heartbeats: Vec<HeartbeatConfig>,
  pub slider_ranges: SliderRanges,
  pub oidc: Option<OidcConfig>,
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
    extra_patches_file: Option<&Path>,
    base_url: Option<&str>,
    oidc_issuer: Option<&str>,
    oidc_client_id: Option<&str>,
    oidc_client_secret_file: Option<&Path>,
  ) -> Result<Self, ConfigError> {
    let file = match config_path {
      Some(p) => ConfigFileRaw::from_file(p)?,
      None => {
        let default = PathBuf::from("config.toml");
        if default.exists() {
          ConfigFileRaw::from_file(&default)?
        } else {
          ConfigFileRaw::default()
        }
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
    // file patches (each layer wins on collision).
    let mut library = builtin_library();
    for (name, patch) in &file.patches {
      library.insert(name.clone(), patch.clone());
    }

    // Extra patches file (CLI flag or config).
    let epf = extra_patches_file
      .map(PathBuf::from)
      .or(file.extra_patches_file);
    if let Some(path) = &epf {
      let contents = std::fs::read_to_string(path).map_err(|source| {
        ConfigError::ExtraPatchesRead {
          path: path.clone(),
          source,
        }
      })?;
      let extra: HashMap<String, Patch> =
        toml::from_str(&contents).map_err(|source| {
          ConfigError::ExtraPatchesParse {
            path: path.clone(),
            source,
          }
        })?;
      for (name, patch) in extra {
        library.insert(name, patch);
      }
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

    Ok(Config {
      log_level,
      log_format,
      listen_address,
      audio_device: file.audio_device,
      frontend_path,
      library,
      heartbeats: file.heartbeats,
      slider_ranges: file.slider_ranges,
      oidc,
    })
  }
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
  use sonify_health_lib::probe::ResultMode;

  #[test]
  fn heartbeats_section_parses() {
    let toml = r#"
      [[heartbeats]]
      name = "gateway"
      command = "ping -c 1 8.8.8.8"
      result_mode = "exit-code-severity"

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
      continuous = true

      [[heartbeats.notes]]
      volume = 0.2

      [heartbeats.notes.transition]
      type = "gradient"
      patches = ["warm", "sharp"]
    "#;

    let raw: ConfigFileRaw = toml::from_str(toml).unwrap();
    assert_eq!(raw.heartbeats.len(), 2);
    assert_eq!(raw.heartbeats[0].name, "gateway");
    assert_eq!(raw.heartbeats[0].result_mode, ResultMode::ExitCodeSeverity);
    assert!(!raw.heartbeats[0].continuous);
    assert_eq!(raw.heartbeats[0].notes.len(), 1);
    assert_eq!(raw.heartbeats[1].name, "cpu");
    assert_eq!(raw.heartbeats[1].result_mode, ResultMode::Stdout);
    assert!(raw.heartbeats[1].continuous);
    assert!((raw.heartbeats[1].notes[0].volume - 0.2).abs() < f64::EPSILON);
  }

  #[test]
  fn patches_section_parses() {
    let toml = r#"
      [patches.my-tone]
      freq = 523.0
      duration = 0.4
      saw_ratio = 1.0
      sine_ratio = 0.0
    "#;

    let raw: ConfigFileRaw = toml::from_str(toml).unwrap();
    assert_eq!(raw.patches.len(), 1);
    let p = &raw.patches["my-tone"];
    assert_eq!(p.freq, 523.0);
    assert_eq!(p.duration, 0.4);
    assert_eq!(p.saw_ratio, 1.0);
    assert_eq!(p.sine_ratio, 0.0);
    // Unspecified fields use defaults.
    assert_eq!(p.amplitude, 0.3);
  }

  #[test]
  fn missing_heartbeats_defaults() {
    let config = Config::from_args(
      None, None, None, None, None, None, None, None, None, None,
    )
    .unwrap();
    assert!(config.heartbeats.is_empty());
  }

  #[test]
  fn library_includes_builtins() {
    let config = Config::from_args(
      None, None, None, None, None, None, None, None, None, None,
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
      if path.extension().map(|e| e == "toml").unwrap_or(false) {
        let contents = std::fs::read_to_string(&path).unwrap();
        let _raw: ConfigFileRaw = toml::from_str(&contents)
          .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
      }
    }
  }
}
