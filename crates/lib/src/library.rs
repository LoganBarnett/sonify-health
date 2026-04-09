use crate::patch::Patch;
use std::collections::HashMap;

/// A collection of named patches.  Built-in presets are compiled in;
/// user patches from config or `--extra-patches-file` merge on top
/// (user wins on collision).
pub type PatchLibrary = HashMap<String, Patch>;

/// Return the built-in preset library.
pub fn builtin_library() -> PatchLibrary {
  let mut lib = PatchLibrary::new();

  lib.insert("sine".to_string(), Patch::default());

  lib.insert(
    "bell".to_string(),
    Patch {
      freq: 880.0,
      duration: 0.3,
      sine_ratio: 0.6,
      tri_ratio: 0.4,
      attack_ms: 5.0,
      release_ms: 400.0,
      reverb_mix: 0.4,
      fm_ratio: 2.0,
      fm_depth: 1.5,
      chirp_ratio: 2.0,
      ..Default::default()
    },
  );

  lib.insert(
    "warm".to_string(),
    Patch {
      freq: 330.0,
      sine_ratio: 0.8,
      tri_ratio: 0.2,
      attack_ms: 40.0,
      release_ms: 200.0,
      brightness: 0.6,
      sub_octave: 0.2,
      ..Default::default()
    },
  );

  lib.insert(
    "sharp".to_string(),
    Patch {
      freq: 660.0,
      sine_ratio: 0.2,
      saw_ratio: 0.8,
      attack_ms: 5.0,
      release_ms: 100.0,
      brightness: 1.5,
      resonance: 2.0,
      drive: 1.5,
      chirp_ratio: 0.7,
      ..Default::default()
    },
  );

  lib.insert(
    "hollow".to_string(),
    Patch {
      freq: 440.0,
      sine_ratio: 0.0,
      tri_ratio: 1.0,
      attack_ms: 30.0,
      release_ms: 300.0,
      brightness: 0.4,
      ..Default::default()
    },
  );

  lib.insert(
    "breath".to_string(),
    Patch {
      freq: 440.0,
      sine_ratio: 0.3,
      noise_mix: 0.5,
      attack_ms: 80.0,
      release_ms: 200.0,
      brightness: 0.5,
      ..Default::default()
    },
  );

  lib.insert(
    "pluck".to_string(),
    Patch {
      freq: 440.0,
      sine_ratio: 0.5,
      tri_ratio: 0.3,
      saw_ratio: 0.2,
      attack_ms: 2.0,
      release_ms: 300.0,
      duration: 0.3,
      brightness: 1.2,
      chirp_ratio: 1.5,
      ..Default::default()
    },
  );

  lib.insert(
    "pad".to_string(),
    Patch {
      freq: 220.0,
      sine_ratio: 0.5,
      tri_ratio: 0.5,
      attack_ms: 200.0,
      release_ms: 500.0,
      duration: 2.0,
      reverb_mix: 0.5,
      vibrato_rate: 3.0,
      vibrato_depth: 0.1,
      ..Default::default()
    },
  );

  lib.insert(
    "crunch".to_string(),
    Patch {
      freq: 440.0,
      sine_ratio: 0.3,
      saw_ratio: 0.7,
      attack_ms: 5.0,
      release_ms: 100.0,
      drive: 5.0,
      crush: 0.3,
      downsample: 0.2,
      brightness: 1.5,
      ..Default::default()
    },
  );

  lib.insert(
    "chirp".to_string(),
    Patch {
      freq: 1100.0,
      sine_ratio: 0.7,
      tri_ratio: 0.3,
      attack_ms: 5.0,
      release_ms: 25.0,
      duration: 0.15,
      chirp_ratio: 1.8,
      reverb_mix: 0.7,
      echo_mix: 0.3,
      echo_delay: 0.12,
      ..Default::default()
    },
  );

  lib.insert(
    "alarm".to_string(),
    Patch {
      freq: 880.0,
      sine_ratio: 0.0,
      saw_ratio: 1.0,
      attack_ms: 2.0,
      release_ms: 50.0,
      duration: 0.2,
      brightness: 2.0,
      resonance: 3.0,
      tremolo_rate: 8.0,
      tremolo_depth: 0.5,
      ..Default::default()
    },
  );

  lib
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn builtin_library_has_expected_presets() {
    let lib = builtin_library();
    for name in [
      "sine", "bell", "warm", "sharp", "hollow", "breath", "pluck", "pad",
      "crunch", "chirp", "alarm",
    ] {
      assert!(lib.contains_key(name), "missing preset: {name}");
    }
    assert_eq!(lib.len(), 11);
  }

  #[test]
  fn sine_preset_matches_default() {
    let lib = builtin_library();
    let sine = &lib["sine"];
    let default = Patch::default();
    assert_eq!(sine.freq, default.freq);
    assert_eq!(sine.sine_ratio, default.sine_ratio);
  }
}
