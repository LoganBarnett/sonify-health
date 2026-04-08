use serde::Deserialize;
use sonify_health_lib::{
  check::HeartbeatCheckConfig, timing::TimingConfig, BoopSpec,
  DroneMetricConfig, LogFormat, LogLevel, Voice, VoiceOverrides,
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

  #[error("Failed to read OIDC client secret from {path:?}: {source}")]
  OidcSecretFileRead {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },
}

/// Fully resolved OIDC configuration.  Present only when all four
/// fields were provided via CLI args, environment, or config file.
#[derive(Debug, Clone)]
pub struct OidcConfig {
  pub base_url: String,
  pub issuer: String,
  pub client_id: String,
  pub client_secret: String,
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct ConfigFileRaw {
  log_level: Option<String>,
  log_format: Option<String>,
  listen: Option<String>,
  audio_device: Option<String>,
  frontend_path: Option<PathBuf>,
  voice: Option<VoiceOverrides>,
  heartbeat: Option<HeartbeatSectionRaw>,
  drone: Option<DroneSectionRaw>,
  oidc: Option<OidcSectionRaw>,
}

#[derive(Debug, Deserialize, Default)]
struct OidcSectionRaw {
  base_url: Option<String>,
  issuer: Option<String>,
  client_id: Option<String>,
  client_secret_file: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Default)]
struct HeartbeatSectionRaw {
  #[serde(flatten)]
  timing: Option<TimingConfig>,
  #[serde(default)]
  checks: Vec<HeartbeatCheckConfig>,
  #[serde(default)]
  notes: Vec<NoteSpec>,
}

/// A note specification as it appears in the config file under
/// `[[heartbeat.notes]]`.
#[derive(Debug, Deserialize, Clone)]
struct NoteSpec {
  freq: f64,
  duration: f64,
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
  pub frontend_path: PathBuf,
  voice_overrides: VoiceOverrides,
  pub daemon: DaemonConfig,
  pub oidc: Option<OidcConfig>,
}

/// Configuration specific to daemon mode.
#[derive(Debug, Clone)]
pub struct DaemonConfig {
  pub timing: TimingConfig,
  pub heartbeat_checks: Vec<HeartbeatCheckConfig>,
  pub heartbeat_notes: Vec<BoopSpec>,
  pub drone_poll_interval_secs: f64,
  pub drone_metrics: Vec<DroneMetricConfig>,
}

impl Default for DaemonConfig {
  fn default() -> Self {
    Self {
      timing: TimingConfig::default(),
      heartbeat_checks: Vec::new(),
      heartbeat_notes: Vec::new(),
      drone_poll_interval_secs: 5.0,
      drone_metrics: Vec::new(),
    }
  }
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

    let (drone_poll_interval_secs, drone_metrics) = file
      .drone
      .map(|d| (d.poll_interval_secs.unwrap_or(5.0), d.metrics))
      .unwrap_or((5.0, Vec::new()));

    let daemon = file
      .heartbeat
      .map(|hb| {
        let heartbeat_notes = hb
          .notes
          .iter()
          .map(|n| BoopSpec {
            freq: n.freq,
            duration: n.duration,
          })
          .collect();
        DaemonConfig {
          timing: hb.timing.unwrap_or_default(),
          heartbeat_checks: hb.checks,
          heartbeat_notes,
          drone_poll_interval_secs,
          drone_metrics: drone_metrics.clone(),
        }
      })
      .unwrap_or(DaemonConfig {
        drone_poll_interval_secs,
        drone_metrics,
        ..DaemonConfig::default()
      });

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
      voice_overrides: file.voice.unwrap_or_default(),
      daemon,
      oidc,
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

  #[test]
  fn drone_section_parses() {
    let toml = r#"
      [drone]
      poll_interval_secs = 10

      [[drone.metrics]]
      name = "gpu"
      command = "echo 0.5"
      result_mode = "stdout"

      [[drone.metrics]]
      name = "mem"
      command = "echo 0.3"
      result_mode = "exit-code"
    "#;

    let raw: ConfigFileRaw = toml::from_str(toml).unwrap();
    let drone = raw.drone.unwrap();
    assert_eq!(drone.poll_interval_secs, Some(10.0));
    assert_eq!(drone.metrics.len(), 2);
    assert_eq!(drone.metrics[0].name, "gpu");
    assert_eq!(drone.metrics[1].name, "mem");
    assert_eq!(drone.metrics[0].base_freq, None);
    assert_eq!(drone.metrics[0].boops, None);
  }

  #[test]
  fn drone_base_freq_and_boops_parse() {
    let toml = r#"
      [drone]
      [[drone.metrics]]
      name = "cpu"
      command = "echo 0.5"
      result_mode = "stdout"
      base_freq = 220.0
      boops = 3
    "#;

    let raw: ConfigFileRaw = toml::from_str(toml).unwrap();
    let drone = raw.drone.unwrap();
    assert_eq!(drone.metrics[0].base_freq, Some(220.0));
    assert_eq!(drone.metrics[0].boops, Some(3));
  }

  #[test]
  fn missing_drone_section_defaults() {
    let config =
      Config::from_args(None, None, None, None, None, None, None, None, None)
        .unwrap();
    assert!(config.daemon.drone_metrics.is_empty());
    assert!(
      (config.daemon.drone_poll_interval_secs - 5.0).abs() < f64::EPSILON
    );
  }

  #[test]
  fn heartbeat_notes_parse() {
    let toml = r#"
      [heartbeat]
      slot = 0
      cycle_duration_secs = 10
      slot_duration_secs = 2

      [[heartbeat.notes]]
      freq = 440.0
      duration = 0.25

      [[heartbeat.notes]]
      freq = 880.0
      duration = 0.15
    "#;

    let raw: ConfigFileRaw = toml::from_str(toml).unwrap();
    let hb = raw.heartbeat.unwrap();
    assert_eq!(hb.notes.len(), 2);
    assert!((hb.notes[0].freq - 440.0).abs() < f64::EPSILON);
    assert!((hb.notes[0].duration - 0.25).abs() < f64::EPSILON);
    assert!((hb.notes[1].freq - 880.0).abs() < f64::EPSILON);
    assert!((hb.notes[1].duration - 0.15).abs() < f64::EPSILON);
  }
}
