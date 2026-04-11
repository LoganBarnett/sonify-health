use sonify_health_lib::{Patch, PatchLibrary, PatchOverrides};

/// CLI patch overrides shared by the `preview` and `print`
/// subcommands.
#[derive(Debug, clap::Args)]
pub(crate) struct CliPatchOverrides {
  /// Name of a library patch to use as the base.
  #[arg(long, default_value = "sine", help_heading = "Patch")]
  pub patch_name: String,

  /// Override frequency (Hz).
  #[arg(long, help_heading = "Patch overrides")]
  freq: Option<f64>,

  /// Override note duration (seconds).
  #[arg(long, help_heading = "Patch overrides")]
  duration: Option<f64>,

  /// Override sine oscillator ratio.
  #[arg(long, help_heading = "Patch overrides")]
  sine_ratio: Option<f64>,

  /// Override triangle oscillator ratio.
  #[arg(long, help_heading = "Patch overrides")]
  tri_ratio: Option<f64>,

  /// Override saw oscillator ratio.
  #[arg(long, help_heading = "Patch overrides")]
  saw_ratio: Option<f64>,

  /// Override square oscillator ratio.
  #[arg(long, help_heading = "Patch overrides")]
  square_ratio: Option<f64>,

  /// Override attack time (ms).
  #[arg(long, help_heading = "Patch overrides")]
  attack_ms: Option<f64>,

  /// Override decay time (ms).
  #[arg(long, help_heading = "Patch overrides")]
  decay_ms: Option<f64>,

  /// Override release time (ms).
  #[arg(long, help_heading = "Patch overrides")]
  release_ms: Option<f64>,

  /// Override sustain level (0.0–1.0).
  #[arg(long, help_heading = "Patch overrides")]
  sustain: Option<f64>,

  /// Override chirp frequency ratio.
  #[arg(long, help_heading = "Patch overrides")]
  chirp_ratio: Option<f64>,

  /// Override stereo pan position (-1.0 to 1.0).
  #[arg(long, allow_hyphen_values = true, help_heading = "Patch overrides")]
  stereo_pan: Option<f64>,

  /// Override reverb mix level.
  #[arg(long, help_heading = "Patch overrides")]
  reverb_mix: Option<f64>,

  /// Override echo delay time (seconds).
  #[arg(long, help_heading = "Patch overrides")]
  echo_delay: Option<f64>,

  /// Override echo wet/dry mix.
  #[arg(long, help_heading = "Patch overrides")]
  echo_mix: Option<f64>,

  /// Override brightness (lowpass cutoff scaler).
  #[arg(long, help_heading = "Patch overrides")]
  brightness: Option<f64>,

  /// Override resonance (filter Q scaler).
  #[arg(long, help_heading = "Patch overrides")]
  resonance: Option<f64>,

  /// Override highpass filter cutoff (Hz).
  #[arg(long, help_heading = "Patch overrides")]
  highpass: Option<f64>,

  /// Override sub-octave mix.
  #[arg(long, help_heading = "Patch overrides")]
  sub_octave: Option<f64>,

  /// Override vibrato rate (Hz).
  #[arg(long, help_heading = "Patch overrides")]
  vibrato_rate: Option<f64>,

  /// Override vibrato depth (semitones).
  #[arg(long, help_heading = "Patch overrides")]
  vibrato_depth: Option<f64>,

  /// Override tremolo rate (Hz).
  #[arg(long, help_heading = "Patch overrides")]
  tremolo_rate: Option<f64>,

  /// Override tremolo depth (fraction).
  #[arg(long, help_heading = "Patch overrides")]
  tremolo_depth: Option<f64>,

  /// Override amplitude (0.0–1.0).
  #[arg(long, help_heading = "Patch overrides")]
  amplitude: Option<f64>,

  /// Override drive (pre-filter saturation).
  #[arg(long, help_heading = "Patch overrides")]
  drive: Option<f64>,

  /// Override noise mix (pink noise blend).
  #[arg(long, help_heading = "Patch overrides")]
  noise_mix: Option<f64>,

  /// Override crush (bitcrush intensity).
  #[arg(long, help_heading = "Patch overrides")]
  crush: Option<f64>,

  /// Override FM modulator ratio.
  #[arg(long, help_heading = "Patch overrides")]
  fm_ratio: Option<f64>,

  /// Override FM modulation depth.
  #[arg(long, help_heading = "Patch overrides")]
  fm_depth: Option<f64>,

  /// Override downsample (lo-fi sample rate reduction).
  #[arg(long, help_heading = "Patch overrides")]
  downsample: Option<f64>,

  /// Override loop repeat gap (seconds added to content duration).
  #[arg(long, help_heading = "Patch overrides")]
  gap: Option<f64>,

  /// Override detune offset in cents.
  #[arg(long, help_heading = "Patch overrides")]
  detune: Option<f64>,

  /// Override harshness offset (-1 to 1).
  #[arg(long, help_heading = "Patch overrides")]
  harshness_offset: Option<f64>,
}

impl CliPatchOverrides {
  fn patch_overrides(&self) -> PatchOverrides {
    PatchOverrides {
      freq: self.freq,
      duration: self.duration,
      sine_ratio: self.sine_ratio,
      tri_ratio: self.tri_ratio,
      saw_ratio: self.saw_ratio,
      square_ratio: self.square_ratio,
      attack_ms: self.attack_ms,
      decay_ms: self.decay_ms,
      release_ms: self.release_ms,
      sustain: self.sustain,
      chirp_ratio: self.chirp_ratio,
      stereo_pan: self.stereo_pan,
      reverb_mix: self.reverb_mix,
      echo_delay: self.echo_delay,
      echo_mix: self.echo_mix,
      brightness: self.brightness,
      resonance: self.resonance,
      highpass: self.highpass,
      sub_octave: self.sub_octave,
      vibrato_rate: self.vibrato_rate,
      vibrato_depth: self.vibrato_depth,
      tremolo_rate: self.tremolo_rate,
      tremolo_depth: self.tremolo_depth,
      amplitude: self.amplitude,
      drive: self.drive,
      noise_mix: self.noise_mix,
      crush: self.crush,
      fm_ratio: self.fm_ratio,
      fm_depth: self.fm_depth,
      downsample: self.downsample,
      gap: self.gap,
      detune: self.detune,
      harshness_offset: self.harshness_offset,
    }
  }

  /// Resolve the named patch from the library and apply CLI
  /// overrides.
  pub(crate) fn resolve_patch(&self, library: &PatchLibrary) -> Patch {
    library
      .get(&self.patch_name)
      .cloned()
      .unwrap_or_default()
      .with_overrides(&self.patch_overrides())
  }
}
