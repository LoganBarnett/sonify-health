use serde::Deserialize;
use sonify_health_lib::{
  check::HeartbeatCheckConfig, timing::TimingConfig, LogFormat, LogLevel,
  Voice, VoiceOverrides,
};
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
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct ConfigFileRaw {
  log_level: Option<String>,
  log_format: Option<String>,
  listen: Option<String>,
  voice: Option<VoiceOverrides>,
  heartbeat: Option<HeartbeatSectionRaw>,
}

#[derive(Debug, Deserialize, Default)]
struct HeartbeatSectionRaw {
  #[serde(flatten)]
  timing: Option<TimingConfig>,
  #[serde(default)]
  checks: Vec<HeartbeatCheckConfig>,
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
  voice_overrides: VoiceOverrides,
  pub daemon: DaemonConfig,
}

/// Configuration specific to daemon mode.
#[derive(Debug, Clone, Default)]
pub struct DaemonConfig {
  pub timing: TimingConfig,
  pub heartbeat_checks: Vec<HeartbeatCheckConfig>,
}

impl Config {
  /// Build a validated configuration by merging CLI
  /// arguments with an optional config file.
  pub fn from_args(
    log_level: Option<&str>,
    log_format: Option<&str>,
    listen: Option<&str>,
    config_path: Option<&Path>,
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

    let daemon = file
      .heartbeat
      .map(|hb| DaemonConfig {
        timing: hb.timing.unwrap_or_default(),
        heartbeat_checks: hb.checks,
      })
      .unwrap_or_default();

    Ok(Config {
      log_level,
      log_format,
      listen_address,
      voice_overrides: file.voice.unwrap_or_default(),
      daemon,
    })
  }

  /// Resolve the machine's voice: hostname-derived defaults
  /// with any configured overrides applied.
  pub fn voice(&self) -> Voice {
    Voice::from_current_host().with_overrides(&self.voice_overrides)
  }
}
