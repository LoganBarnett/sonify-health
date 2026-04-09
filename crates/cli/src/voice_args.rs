use crate::config::Config;
use sha2::{Digest, Sha256};
use sonify_health_lib::{Voice, VoiceOverrides};
use tracing::debug;

/// CLI voice overrides shared by the `preview` and `print` subcommands.
#[derive(Debug, clap::Args)]
pub(crate) struct CliVoiceOverrides {
  /// Override hostname for voice derivation.
  #[arg(long, help_heading = "Voice overrides")]
  hostname: Option<String>,

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

  /// Override vibrato rate (0.0–20.0 Hz).
  #[arg(long, help_heading = "Voice overrides")]
  vibrato_rate: Option<f64>,

  /// Override vibrato depth (0.0–1.0 semitones).
  #[arg(long, help_heading = "Voice overrides")]
  vibrato_depth: Option<f64>,

  /// Override tremolo rate (0.0–20.0 Hz).
  #[arg(long, help_heading = "Voice overrides")]
  tremolo_rate: Option<f64>,

  /// Override tremolo depth (0.0–1.0 fraction).
  #[arg(long, help_heading = "Voice overrides")]
  tremolo_depth: Option<f64>,

  /// Override amplitude (0.0–1.0).
  #[arg(long, help_heading = "Voice overrides")]
  amplitude: Option<f64>,

  /// Override square oscillator ratio.
  #[arg(long, help_heading = "Voice overrides")]
  square_ratio: Option<f64>,

  /// Override drive (pre-filter saturation, 0.01–20.0).
  #[arg(long, help_heading = "Voice overrides")]
  drive: Option<f64>,

  /// Override noise mix (pink noise blend, 0.0–1.0).
  #[arg(long, help_heading = "Voice overrides")]
  noise_mix: Option<f64>,

  /// Override crush (bitcrush intensity, 0.0–1.0).
  #[arg(long, help_heading = "Voice overrides")]
  crush: Option<f64>,

  /// Override FM modulator ratio (0.0–8.0, ratio of carrier freq).
  #[arg(long, help_heading = "Voice overrides")]
  fm_ratio: Option<f64>,

  /// Override FM modulation depth (0.0–10.0, modulation index).
  #[arg(long, help_heading = "Voice overrides")]
  fm_depth: Option<f64>,

  /// Override downsample (lo-fi sample rate reduction, 0.0–1.0).
  #[arg(long, help_heading = "Voice overrides")]
  downsample: Option<f64>,

  /// Override sustain level (0.0–1.0). Body amplitude after attack.
  #[arg(long, help_heading = "Voice overrides")]
  sustain: Option<f64>,
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
      vibrato_rate: self.vibrato_rate,
      vibrato_depth: self.vibrato_depth,
      tremolo_rate: self.tremolo_rate,
      tremolo_depth: self.tremolo_depth,
      amplitude: self.amplitude,
      square_ratio: self.square_ratio,
      drive: self.drive,
      noise_mix: self.noise_mix,
      crush: self.crush,
      fm_ratio: self.fm_ratio,
      fm_depth: self.fm_depth,
      downsample: self.downsample,
      sustain: self.sustain,
    }
  }

  /// Fully resolve the voice: hostname derivation, config overrides,
  /// and CLI overrides.
  pub(crate) fn resolve_voice(&self, config: &Config) -> Voice {
    let hostname = self.effective_hostname();

    let voice = Voice::from_hostname(&hostname)
      .with_overrides(config.voice_overrides_ref())
      .with_overrides(&self.voice_overrides());

    let host_hash = Sha256::digest(hostname.as_bytes());
    debug!(
      hostname = %hostname,
      hostname_sha256_prefix = %host_hash[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>(),
      note_seed = voice.note_seed,
      "Voice seed derivation"
    );

    voice
  }
}
