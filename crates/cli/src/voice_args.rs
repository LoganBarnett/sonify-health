use crate::config::Config;
use sha2::{Digest, Sha256};
use sonify_health_lib::{scale, PentatonicScale, Voice, VoiceOverrides};
use tracing::debug;

/// CLI voice overrides shared by the `preview` and `print` subcommands.
#[derive(Debug, clap::Args)]
pub(crate) struct CliVoiceOverrides {
  /// Override hostname for voice derivation.
  #[arg(long, help_heading = "Voice overrides")]
  hostname: Option<String>,

  /// Override the pentatonic scale key.
  #[arg(long, help_heading = "Voice overrides")]
  scale_key: Option<String>,

  /// Override base frequency (Hz).
  #[arg(long, help_heading = "Voice overrides")]
  base_freq: Option<f64>,

  /// Override sine oscillator ratio.
  #[arg(long, help_heading = "Voice overrides")]
  sine_ratio: Option<f64>,

  /// Override triangle oscillator ratio.
  #[arg(long, help_heading = "Voice overrides")]
  tri_ratio: Option<f64>,

  /// Override saw oscillator ratio.
  #[arg(long, help_heading = "Voice overrides")]
  saw_ratio: Option<f64>,

  /// Override attack time (ms).
  #[arg(long, help_heading = "Voice overrides")]
  attack_ms: Option<f64>,

  /// Override release time (ms).
  #[arg(long, help_heading = "Voice overrides")]
  release_ms: Option<f64>,

  /// Override chirp frequency ratio.
  #[arg(long, help_heading = "Voice overrides")]
  chirp_ratio: Option<f64>,

  /// Override stereo pan position (-1.0 to 1.0).
  #[arg(long, allow_hyphen_values = true, help_heading = "Voice overrides")]
  stereo_pan: Option<f64>,

  /// Override reverb mix level.
  #[arg(long, help_heading = "Voice overrides")]
  reverb_mix: Option<f64>,

  /// Override note seed (0.0 to 1.0).
  #[arg(long, help_heading = "Voice overrides")]
  note_seed: Option<f64>,

  /// Override echo delay time (seconds).
  #[arg(long, help_heading = "Voice overrides")]
  echo_delay: Option<f64>,

  /// Override echo wet/dry mix.
  #[arg(long, help_heading = "Voice overrides")]
  echo_mix: Option<f64>,

  /// Override brightness (lowpass cutoff scaler, 0.05–1.0).
  #[arg(long, help_heading = "Voice overrides")]
  brightness: Option<f64>,

  /// Override resonance (filter Q scaler, 0.1–3.0).
  #[arg(long, help_heading = "Voice overrides")]
  resonance: Option<f64>,

  /// Override sub-octave mix (0.0–1.0).
  #[arg(long, help_heading = "Voice overrides")]
  sub_octave: Option<f64>,

  /// Override note spread (0.0–1.0, octaves around base frequency).
  #[arg(long, help_heading = "Voice overrides")]
  note_spread: Option<f64>,
}

impl CliVoiceOverrides {
  /// Return the CLI-provided hostname or the current machine's hostname.
  pub(crate) fn effective_hostname(&self) -> String {
    self.hostname.clone().unwrap_or_else(|| {
      gethostname::gethostname().to_string_lossy().to_string()
    })
  }

  /// Convert CLI fields into a `VoiceOverrides`.
  fn voice_overrides(&self) -> VoiceOverrides {
    VoiceOverrides {
      scale_key: self.scale_key.clone(),
      base_freq: self.base_freq,
      sine_ratio: self.sine_ratio,
      tri_ratio: self.tri_ratio,
      saw_ratio: self.saw_ratio,
      attack_ms: self.attack_ms,
      release_ms: self.release_ms,
      chirp_ratio: self.chirp_ratio,
      stereo_pan: self.stereo_pan,
      reverb_mix: self.reverb_mix,
      note_seed: self.note_seed,
      echo_delay: self.echo_delay,
      echo_mix: self.echo_mix,
      brightness: self.brightness,
      resonance: self.resonance,
      sub_octave: self.sub_octave,
      note_spread: self.note_spread,
    }
  }

  /// Determine the effective scale key: CLI flag, then config file,
  /// then domain derived from the effective hostname.
  pub(crate) fn effective_scale_key(&self, config: &Config) -> String {
    self
      .scale_key
      .clone()
      .unwrap_or_else(|| config.scale_key_for(&self.effective_hostname()))
  }

  /// Fully resolve the voice: hostname derivation, config overrides,
  /// CLI overrides, and pentatonic scale snap.
  pub(crate) fn resolve_voice(&self, config: &Config) -> Voice {
    let hostname = self.effective_hostname();
    let scale_key = self.effective_scale_key(config);

    let voice = Voice::from_hostname(&hostname)
      .with_overrides(config.voice_overrides_ref())
      .with_overrides(&self.voice_overrides())
      .with_scale(&scale_key);

    let domain = scale::domain_from_hostname(&hostname);
    let host_hash = Sha256::digest(hostname.as_bytes());
    let domain_hash = Sha256::digest(domain.as_bytes());
    debug!(
      hostname = %hostname,
      hostname_sha256_prefix = %host_hash[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>(),
      domain = %domain,
      domain_sha256_prefix = %domain_hash[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>(),
      note_seed = voice.note_seed,
      "Voice seed derivation"
    );

    voice
  }

  /// Build the pentatonic scale for the resolved voice.
  pub(crate) fn resolve_scale(&self, config: &Config) -> PentatonicScale {
    PentatonicScale::from_key(&self.effective_scale_key(config))
  }
}
