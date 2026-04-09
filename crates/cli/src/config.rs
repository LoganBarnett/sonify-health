use serde::Deserialize;
use sonify_health_lib::{
  check::CheckConfig, timing::TimingConfig, LogFormat, LogLevel, NoteSpec,
  Patch, PatchOverrides,
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

/// A check profile as it appears in the config file under
/// `[profiles.<name>]`, containing `lo` and `hi` patch overrides
/// for metric-driven interpolation.
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProfileRaw {
  #[serde(default)]
  pub lo: PatchOverrides,
  #[serde(default)]
  pub hi: PatchOverrides,
}

/// A note specification as it appears in the config file under
/// `[[check_notes.<name>]]`.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct RawNoteSpec {
  freq: f64,
  duration: f64,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfigFileRaw {
  log_level: Option<String>,
  log_format: Option<String>,
  listen: Option<String>,
  audio_device: Option<String>,
  frontend_path: Option<PathBuf>,
  patch: Option<PatchOverrides>,
  timing: Option<TimingSectionRaw>,
  #[serde(default)]
  checks: Vec<CheckConfig>,
  profiles: Option<HashMap<String, ProfileRaw>>,
  check_notes: Option<HashMap<String, Vec<RawNoteSpec>>>,
  oidc: Option<OidcSectionRaw>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct TimingSectionRaw {
  cycle_duration_secs: Option<f64>,
  slot_duration_secs: Option<f64>,
  slot: Option<u8>,
  poll_interval_secs: Option<f64>,
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
  patch_overrides: PatchOverrides,
  pub daemon: DaemonConfig,
  pub oidc: Option<OidcConfig>,
}

/// Configuration specific to daemon mode.
#[derive(Debug, Clone)]
pub struct DaemonConfig {
  pub timing: TimingConfig,
  pub checks: Vec<CheckConfig>,
  pub poll_interval_secs: f64,
  /// Per-check profile overrides from `[profiles.<name>]`, keyed by
  /// check name.  Each entry holds (lo, hi) patch overrides for
  /// metric-driven interpolation.
  pub profile_overrides: HashMap<String, (PatchOverrides, PatchOverrides)>,
  /// Per-check note specs from `[[check_notes.<name>]]`, keyed by
  /// check name.  When present, these override algorithmic generation
  /// and start pinned.
  pub check_notes: HashMap<String, Vec<NoteSpec>>,
}

impl Default for DaemonConfig {
  fn default() -> Self {
    Self {
      timing: TimingConfig::default(),
      checks: Vec::new(),
      poll_interval_secs: 5.0,
      profile_overrides: HashMap::new(),
      check_notes: HashMap::new(),
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

    let timing_raw = file.timing.unwrap_or_default();

    let profile_overrides: HashMap<String, (PatchOverrides, PatchOverrides)> =
      file
        .profiles
        .unwrap_or_default()
        .into_iter()
        .map(|(name, p)| (name, (p.lo, p.hi)))
        .collect();

    let check_notes: HashMap<String, Vec<NoteSpec>> = file
      .check_notes
      .unwrap_or_default()
      .into_iter()
      .map(|(name, specs)| {
        let notes = specs
          .iter()
          .map(|n| NoteSpec {
            freq: n.freq,
            duration: n.duration,
          })
          .collect();
        (name, notes)
      })
      .collect();

    let daemon = DaemonConfig {
      timing: TimingConfig {
        cycle_duration_secs: timing_raw.cycle_duration_secs.unwrap_or(16.0),
        slot_duration_secs: timing_raw.slot_duration_secs.unwrap_or(4.0),
        slot: timing_raw.slot.unwrap_or(0),
      },
      checks: file.checks,
      poll_interval_secs: timing_raw.poll_interval_secs.unwrap_or(5.0),
      profile_overrides,
      check_notes,
    };

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
      patch_overrides: file.patch.unwrap_or_default(),
      daemon,
      oidc,
    })
  }

  /// Resolve the machine's voice: hostname-derived defaults with any
  /// configured overrides applied.
  pub fn patch(&self) -> Patch {
    Patch::from_hostname(&gethostname::gethostname().to_string_lossy())
      .with_overrides(&self.patch_overrides)
  }

  /// Return the config file's patch overrides.
  pub fn patch_overrides_ref(&self) -> &PatchOverrides {
    &self.patch_overrides
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
  use sonify_health_lib::check::ResultMode;

  #[test]
  fn checks_section_parses() {
    let toml = r#"
      [timing]
      poll_interval_secs = 10

      [[checks]]
      name = "gpu"
      command = "echo 0.5"
      result_mode = "stdout"

      [[checks]]
      name = "gateway"
      command = "ping -c 1 8.8.8.8"
      result_mode = "exit-code-severity"
    "#;

    let raw: ConfigFileRaw = toml::from_str(toml).unwrap();
    let timing = raw.timing.unwrap();
    assert_eq!(timing.poll_interval_secs, Some(10.0));
    assert_eq!(raw.checks.len(), 2);
    assert_eq!(raw.checks[0].name, "gpu");
    assert_eq!(raw.checks[0].result_mode, ResultMode::Stdout);
    assert_eq!(raw.checks[1].name, "gateway");
    assert_eq!(raw.checks[1].result_mode, ResultMode::ExitCodeSeverity);
  }

  #[test]
  fn check_optional_fields_parse() {
    let toml = r#"
      [[checks]]
      name = "cpu"
      command = "echo 0.5"
      result_mode = "stdout"
      interp_curve = 2.0
      boops = 3
    "#;

    let raw: ConfigFileRaw = toml::from_str(toml).unwrap();
    let c = &raw.checks[0];
    assert_eq!(c.interp_curve, Some(2.0));
    assert_eq!(c.boops, Some(3));
  }

  #[test]
  fn missing_checks_defaults() {
    let config =
      Config::from_args(None, None, None, None, None, None, None, None, None)
        .unwrap();
    assert!(config.daemon.checks.is_empty());
    assert!((config.daemon.poll_interval_secs - 5.0).abs() < f64::EPSILON);
  }

  #[test]
  fn check_notes_parse() {
    let toml = r#"
      [[checks]]
      name = "cpu"
      command = "echo 0.5"
      result_mode = "stdout"

      [[check_notes.cpu]]
      freq = 460.0
      duration = 0.5

      [[check_notes.cpu]]
      freq = 920.0
      duration = 0.25
    "#;

    let raw: ConfigFileRaw = toml::from_str(toml).unwrap();
    let notes = raw.check_notes.unwrap();
    assert_eq!(notes.len(), 1);
    let cpu_notes = &notes["cpu"];
    assert_eq!(cpu_notes.len(), 2);
    assert!((cpu_notes[0].freq - 460.0).abs() < f64::EPSILON);
    assert!((cpu_notes[0].duration - 0.5).abs() < f64::EPSILON);
    assert!((cpu_notes[1].freq - 920.0).abs() < f64::EPSILON);
    assert!((cpu_notes[1].duration - 0.25).abs() < f64::EPSILON);
  }

  #[test]
  fn profiles_section_parses() {
    let toml = r#"
      [profiles.cpu.lo]
      freq = 220.0
      sine_ratio = 0.5

      [profiles.cpu.hi]
      freq = 440.0
      sine_ratio = 1.0

      [profiles.mem.lo]
      freq = 330.0

      [profiles.mem.hi]
      freq = 660.0
    "#;

    let raw: ConfigFileRaw = toml::from_str(toml).unwrap();
    let dp = raw.profiles.unwrap();
    assert_eq!(dp.len(), 2);
    assert_eq!(dp["cpu"].lo.freq, Some(220.0));
    assert_eq!(dp["cpu"].lo.sine_ratio, Some(0.5));
    assert_eq!(dp["cpu"].hi.freq, Some(440.0));
    assert_eq!(dp["cpu"].hi.sine_ratio, Some(1.0));
    assert_eq!(dp["mem"].lo.freq, Some(330.0));
    assert_eq!(dp["mem"].hi.freq, Some(660.0));
    assert_eq!(dp["mem"].lo.sine_ratio, None);
  }

  /// The exported TOML format must round-trip through config loading.
  /// `format_toml` produces `[patch]` and `[profiles.<name>.lo/hi]`
  /// sections; these must be parsed by `ConfigFileRaw` so users can
  /// paste an export into a config file.
  #[test]
  fn export_toml_round_trips_through_config_parser() {
    let patch = Patch::from_hostname("test");
    let drone_lo = Patch::from_hostname("drone-lo");
    let drone_hi = Patch::from_hostname("drone-hi");

    // Simulate what format_toml produces: base patch, notes, and
    // check profiles.
    let exported = format!(
      r#"[patch]
freq = {hb_base}
sine_ratio = {hb_sine}
tri_ratio = {hb_tri}
saw_ratio = {hb_saw}
attack_ms = {hb_attack}
release_ms = {hb_release}
chirp_ratio = {hb_chirp}
stereo_pan = {hb_pan}
reverb_mix = {hb_reverb}
note_seed = {hb_seed}
echo_delay = {hb_echo_delay}
echo_mix = {hb_echo_mix}
brightness = {hb_brightness}
resonance = {hb_resonance}
sub_octave = {hb_sub}
vibrato_rate = {hb_vib_rate}
vibrato_depth = {hb_vib_depth}
tremolo_rate = {hb_trem_rate}
tremolo_depth = {hb_trem_depth}
amplitude = {hb_amp}

[[check_notes.gateway]]
freq = 440.0
duration = 0.25

[[check_notes.gateway]]
freq = 880.0
duration = 0.15

[[check_notes.cpu]]
freq = 460.0
duration = 0.5

[[check_notes.cpu]]
freq = 920.0
duration = 0.25

[profiles.cpu.lo]
freq = {lo_base}
sine_ratio = {lo_sine}
amplitude = {lo_amp}

[profiles.cpu.hi]
freq = {hi_base}
sine_ratio = {hi_sine}
amplitude = {hi_amp}
"#,
      hb_base = patch.freq,
      hb_sine = patch.sine_ratio,
      hb_tri = patch.tri_ratio,
      hb_saw = patch.saw_ratio,
      hb_attack = patch.attack_ms,
      hb_release = patch.release_ms,
      hb_chirp = patch.chirp_ratio,
      hb_pan = patch.stereo_pan,
      hb_reverb = patch.reverb_mix,
      hb_seed = patch.note_seed,
      hb_echo_delay = patch.echo_delay,
      hb_echo_mix = patch.echo_mix,
      hb_brightness = patch.brightness,
      hb_resonance = patch.resonance,
      hb_sub = patch.sub_octave,
      hb_vib_rate = patch.vibrato_rate,
      hb_vib_depth = patch.vibrato_depth,
      hb_trem_rate = patch.tremolo_rate,
      hb_trem_depth = patch.tremolo_depth,
      hb_amp = patch.amplitude,
      lo_base = drone_lo.freq,
      lo_sine = drone_lo.sine_ratio,
      lo_amp = drone_lo.amplitude,
      hi_base = drone_hi.freq,
      hi_sine = drone_hi.sine_ratio,
      hi_amp = drone_hi.amplitude,
    );

    let raw: ConfigFileRaw = toml::from_str(&exported)
      .expect("Exported TOML must parse as ConfigFileRaw");

    // Patch params should survive.
    let patch_ovr = raw
      .patch
      .as_ref()
      .expect("Export should produce a [patch] section");
    assert!(
      (patch_ovr.freq.unwrap() - patch.freq).abs() < f64::EPSILON,
      "freq did not round-trip",
    );
    assert!(
      (patch_ovr.amplitude.unwrap() - patch.amplitude).abs() < f64::EPSILON,
      "amplitude did not round-trip",
    );

    // Profile params should survive.
    let dp = raw
      .profiles
      .as_ref()
      .expect("Export should produce a [profiles] section");
    let cpu_profile = dp.get("cpu").expect("Export should include cpu profile");
    assert!(
      (cpu_profile.lo.freq.unwrap() - drone_lo.freq).abs() < f64::EPSILON,
      "Profile lo freq did not round-trip",
    );
    assert!(
      (cpu_profile.hi.freq.unwrap() - drone_hi.freq).abs() < f64::EPSILON,
      "Profile hi freq did not round-trip",
    );

    // Check notes should survive.
    let cn = raw
      .check_notes
      .as_ref()
      .expect("Export should produce a [check_notes] section");
    let gw_notes = cn
      .get("gateway")
      .expect("Export should include gateway notes");
    assert_eq!(gw_notes.len(), 2);
    assert!((gw_notes[0].freq - 440.0).abs() < f64::EPSILON);
    assert!((gw_notes[0].duration - 0.25).abs() < f64::EPSILON);

    let cpu_notes = cn.get("cpu").expect("Export should include cpu notes");
    assert_eq!(cpu_notes.len(), 2);
    assert!((cpu_notes[0].freq - 460.0).abs() < f64::EPSILON);
    assert!((cpu_notes[0].duration - 0.5).abs() < f64::EPSILON);
  }

  /// The example config files must parse without error, including
  /// their `[patch]` and `[profiles.*]` sections.
  #[test]
  fn example_configs_parse() {
    let examples_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
      .parent()
      .unwrap()
      .parent()
      .unwrap()
      .join("examples");
    for entry in
      std::fs::read_dir(&examples_dir).expect("examples directory should exist")
    {
      let path = entry.unwrap().path();
      if path.extension().map(|e| e == "toml").unwrap_or(false) {
        let contents = std::fs::read_to_string(&path).unwrap();
        let raw: ConfigFileRaw = toml::from_str(&contents)
          .unwrap_or_else(|e| panic!("{}: {e}", path.display()));

        // If the example has [patch], it must parse.
        if contents.contains("[patch]") {
          assert!(
            raw.patch.is_some(),
            "{}: [patch] should parse",
            path.display()
          );
        }

        // If the example has [profiles.…], it must parse.
        if contents.contains("[profiles.") {
          assert!(
            raw
              .profiles
              .as_ref()
              .map(|dp| !dp.is_empty())
              .unwrap_or(false),
            "{}: [profiles] should parse",
            path.display()
          );
        }

        // If the example has [[check_notes.…]], it must parse.
        if contents.contains("[[check_notes.") {
          assert!(
            raw
              .check_notes
              .as_ref()
              .map(|dn| !dn.is_empty())
              .unwrap_or(false),
            "{}: [check_notes] should parse",
            path.display()
          );
        }
      }
    }
  }
}
