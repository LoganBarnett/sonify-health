use serde::Deserialize;
use sonify_health_lib::{
  check::HeartbeatCheckConfig, timing::TimingConfig, DroneMetricConfig,
  LogFormat, LogLevel, Voice, VoiceOverrides,
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
  audio_device: Option<String>,
  voice: Option<VoiceOverrides>,
  heartbeat: Option<HeartbeatSectionRaw>,
  drone: Option<DroneSectionRaw>,
}

#[derive(Debug, Deserialize, Default)]
struct HeartbeatSectionRaw {
  #[serde(flatten)]
  timing: Option<TimingConfig>,
  #[serde(default)]
  checks: Vec<HeartbeatCheckConfig>,
}

#[derive(Debug, Deserialize, Default)]
struct DroneSectionRaw {
  poll_interval_secs: Option<f64>,
  #[serde(default)]
  metrics: Vec<DroneMetricConfig>,
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
  voice_overrides: VoiceOverrides,
  pub daemon: DaemonConfig,
}

/// Configuration specific to daemon mode.
#[derive(Debug, Clone)]
pub struct DaemonConfig {
  pub timing: TimingConfig,
  pub heartbeat_checks: Vec<HeartbeatCheckConfig>,
  pub drone_poll_interval_secs: f64,
  pub drone_metrics: Vec<DroneMetricConfig>,
}

impl Default for DaemonConfig {
  fn default() -> Self {
    Self {
      timing: TimingConfig::default(),
      heartbeat_checks: Vec::new(),
      drone_poll_interval_secs: 5.0,
      drone_metrics: Vec::new(),
    }
  }
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

    let (drone_poll_interval_secs, drone_metrics) = file
      .drone
      .map(|d| (d.poll_interval_secs.unwrap_or(5.0), d.metrics))
      .unwrap_or((5.0, Vec::new()));

    let daemon = file
      .heartbeat
      .map(|hb| DaemonConfig {
        timing: hb.timing.unwrap_or_default(),
        heartbeat_checks: hb.checks,
        drone_poll_interval_secs,
        drone_metrics: drone_metrics.clone(),
      })
      .unwrap_or(DaemonConfig {
        drone_poll_interval_secs,
        drone_metrics,
        ..DaemonConfig::default()
      });

    Ok(Config {
      log_level,
      log_format,
      listen_address,
      audio_device: file.audio_device,
      voice_overrides: file.voice.unwrap_or_default(),
      daemon,
    })
  }

  /// Resolve the machine's voice: hostname-derived defaults with any
  /// configured overrides and pentatonic scale snapping applied.
  pub fn voice(&self) -> Voice {
    let scale_key = self.scale_key();
    Voice::from_hostname(&gethostname::gethostname().to_string_lossy())
      .with_overrides(&self.voice_overrides)
      .with_scale(&scale_key)
  }

  /// Build the pentatonic scale for this machine's domain.
  pub fn scale(&self) -> sonify_health_lib::PentatonicScale {
    sonify_health_lib::PentatonicScale::from_key(&self.scale_key())
  }

  /// Return the config file's voice overrides.
  pub fn voice_overrides_ref(&self) -> &VoiceOverrides {
    &self.voice_overrides
  }

  /// Determine the scale key for a given hostname: config override if
  /// set, otherwise the domain extracted from the hostname.
  pub fn scale_key_for(&self, hostname: &str) -> String {
    self.voice_overrides.scale_key.clone().unwrap_or_else(|| {
      sonify_health_lib::scale::domain_from_hostname(hostname)
    })
  }

  fn scale_key(&self) -> String {
    self.scale_key_for(&gethostname::gethostname().to_string_lossy())
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use sonify_health_lib::DroneRegister;

  #[test]
  fn drone_section_parses() {
    let toml = r#"
      [drone]
      poll_interval_secs = 10

      [[drone.metrics]]
      name = "gpu"
      command = "echo 0.5"
      result_mode = "stdout"
      register = "low"

      [[drone.metrics]]
      name = "mem"
      command = "echo 0.3"
      result_mode = "exit-code"
      register = "high"
    "#;

    let raw: ConfigFileRaw = toml::from_str(toml).unwrap();
    let drone = raw.drone.unwrap();
    assert_eq!(drone.poll_interval_secs, Some(10.0));
    assert_eq!(drone.metrics.len(), 2);
    assert_eq!(drone.metrics[0].name, "gpu");
    assert_eq!(drone.metrics[0].register, DroneRegister::Low);
    assert_eq!(drone.metrics[1].name, "mem");
    assert_eq!(drone.metrics[1].register, DroneRegister::High);
  }

  #[test]
  fn missing_drone_section_defaults() {
    let config = Config::from_args(None, None, None, None).unwrap();
    assert!(config.daemon.drone_metrics.is_empty());
    assert!(
      (config.daemon.drone_poll_interval_secs - 5.0).abs() < f64::EPSILON
    );
  }

  #[test]
  fn drone_register_deserializes() {
    #[derive(Deserialize)]
    struct Wrapper {
      register: DroneRegister,
    }
    for (input, expected) in [
      ("register = \"low\"", DroneRegister::Low),
      ("register = \"mid\"", DroneRegister::Mid),
      ("register = \"high\"", DroneRegister::High),
    ] {
      let w: Wrapper = toml::from_str(input).unwrap();
      assert_eq!(w.register, expected);
    }
  }
}
