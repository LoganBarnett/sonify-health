use crate::config::{
  patch_with_violations, ConfigError, OverrideInfo, PatchLimitViolation,
};
use crate::patch::{Patch, PatchOverrides};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// A collection of named patches.  Built-in presets are compiled in;
/// user patches from config or `--patch-library` merge on top
/// (user wins on collision).
pub type PatchLibrary = HashMap<String, Patch>;

/// A config file's patch library once resolved.
#[derive(Debug)]
pub struct LoadedLibrary {
  pub library: PatchLibrary,
  pub overrides: HashMap<String, OverrideInfo>,
  pub limit_violations: Vec<PatchLimitViolation>,
}

/// One `[patches]` table as written.
enum PatchEntry {
  Whole { name: String, patch: Box<Patch> },
  Override(OverrideEntry),
}

/// An override as written: a delta over the patch `base` names.
struct OverrideEntry {
  name: String,
  base: String,
  delta: Box<PatchOverrides>,
}

/// The patches an override may name as its base: the whole patches, held to
/// their hard limits and laid over the built-in presets.
struct Bases {
  library: PatchLibrary,
  violations: Vec<PatchLimitViolation>,
}

/// An override applied to its base and held to the hard limits.
struct ResolvedOverride {
  name: String,
  patch: Patch,
  info: OverrideInfo,
  violations: Vec<PatchLimitViolation>,
}

/// Resolves a config file's `[patches]` tables and any patch-library files
/// over the built-in presets, holding every patch to its hard limits.  Later
/// sources win on a shared name: the presets, then the config file's whole
/// patches, then the library files in order, then the overrides.  Under
/// `strict`, a value past a hard limit fails the load.
pub fn load_library(
  tables: &HashMap<String, toml::Value>,
  library_files: &[PathBuf],
  strict: bool,
) -> Result<LoadedLibrary, ConfigError> {
  let entries = tables
    .iter()
    .map(|(name, table)| entry(name, table))
    .collect::<Result<Vec<_>, _>>()?;
  let bases = limited_bases(
    whole_patches(&entries).chain(library_file_patches(library_files)?),
    strict,
  )?;
  let resolved = resolved_overrides(&entries, &bases.library, strict)?;
  Ok(LoadedLibrary::new(bases, resolved))
}

impl LoadedLibrary {
  /// The bases with every resolved override laid over them.
  fn new(bases: Bases, resolved: Vec<ResolvedOverride>) -> Self {
    LoadedLibrary {
      overrides: resolved
        .iter()
        .map(|r| (r.name.clone(), r.info.clone()))
        .collect(),
      limit_violations: bases
        .violations
        .into_iter()
        .chain(resolved.iter().flat_map(|r| r.violations.iter().cloned()))
        .collect(),
      library: bases
        .library
        .into_iter()
        .chain(resolved.into_iter().map(|r| (r.name, r.patch)))
        .collect(),
    }
  }
}

/// A `[patches]` table read as an override when it names an `overrides`
/// base, and as a whole patch otherwise.
fn entry(name: &str, table: &toml::Value) -> Result<PatchEntry, ConfigError> {
  let parse_error = |source| ConfigError::PatchParse {
    name: name.to_string(),
    source: Box::new(source),
  };
  table.get("overrides").map_or_else(
    || {
      table
        .clone()
        .try_into()
        .map(|patch| PatchEntry::Whole {
          name: name.to_string(),
          patch: Box::new(patch),
        })
        .map_err(parse_error)
    },
    |base| {
      let base = base.as_str().ok_or_else(|| {
        ConfigError::Validation(format!(
          "patch {name:?}: 'overrides' must be a string"
        ))
      })?;
      without_overrides_key(table)
        .try_into()
        .map(|delta| {
          PatchEntry::Override(OverrideEntry {
            name: name.to_string(),
            base: base.to_string(),
            delta: Box::new(delta),
          })
        })
        .map_err(parse_error)
    },
  )
}

/// `table` without its `overrides` key, which names the base rather than a
/// field the delta can set.
fn without_overrides_key(table: &toml::Value) -> toml::Value {
  toml::Value::Table(table.as_table().map_or_else(toml::Table::new, |fields| {
    fields
      .iter()
      .filter(|(key, _)| key.as_str() != "overrides")
      .map(|(key, value)| (key.clone(), value.clone()))
      .collect()
  }))
}

/// The whole patches among the config file's entries.
fn whole_patches(
  entries: &[PatchEntry],
) -> impl Iterator<Item = (String, Patch)> + '_ {
  entries.iter().filter_map(|entry| match entry {
    PatchEntry::Whole { name, patch } => {
      Some((name.clone(), (**patch).clone()))
    }
    PatchEntry::Override(_) => None,
  })
}

/// The overrides among the config file's entries.
fn override_entries(
  entries: &[PatchEntry],
) -> impl Iterator<Item = &OverrideEntry> {
  entries.iter().filter_map(|entry| match entry {
    PatchEntry::Override(override_entry) => Some(override_entry),
    PatchEntry::Whole { .. } => None,
  })
}

/// The patches every patch-library file defines, in file order.
fn library_file_patches(
  paths: &[PathBuf],
) -> Result<Vec<(String, Patch)>, ConfigError> {
  paths
    .iter()
    .map(|path| library_file(path))
    .collect::<Result<Vec<_>, _>>()
    .map(|files| files.into_iter().flatten().collect())
}

/// The patches one patch-library file defines.
fn library_file(path: &Path) -> Result<HashMap<String, Patch>, ConfigError> {
  std::fs::read_to_string(path)
    .map_err(|source| ConfigError::PatchLibraryRead {
      path: path.to_path_buf(),
      source,
    })
    .and_then(|contents| {
      toml::from_str(&contents).map_err(|source| {
        ConfigError::PatchLibraryParse {
          path: path.to_path_buf(),
          source: Box::new(source),
        }
      })
    })
}

/// `patches` held to their hard limits and laid over the built-in presets, a
/// later patch replacing an earlier one of the same name.
fn limited_bases(
  patches: impl Iterator<Item = (String, Patch)>,
  strict: bool,
) -> Result<Bases, ConfigError> {
  let (limited, violations): (Vec<_>, Vec<_>) = patches
    .map(|(name, patch)| {
      patch_with_violations(&name, &patch, strict)
        .map(|(kept, found)| ((name, kept), found))
    })
    .collect::<Result<Vec<_>, _>>()?
    .into_iter()
    .unzip();
  Ok(Bases {
    library: builtin_library().into_iter().chain(limited).collect(),
    violations: violations.into_iter().flatten().collect(),
  })
}

/// Every override among `entries`, applied to its base in `bases`.
fn resolved_overrides(
  entries: &[PatchEntry],
  bases: &PatchLibrary,
  strict: bool,
) -> Result<Vec<ResolvedOverride>, ConfigError> {
  let override_names: HashSet<&str> = override_entries(entries)
    .map(|override_entry| override_entry.name.as_str())
    .collect();
  override_entries(entries)
    .map(|override_entry| {
      resolved_override(override_entry, bases, &override_names, strict)
    })
    .collect()
}

/// One override's delta applied to the whole patch its base names, held to
/// the hard limits.  An override of another override is rejected; an override
/// may share its base's name, which replaces that preset or patch in place.
fn resolved_override(
  OverrideEntry { name, base, delta }: &OverrideEntry,
  bases: &PatchLibrary,
  override_names: &HashSet<&str>,
  strict: bool,
) -> Result<ResolvedOverride, ConfigError> {
  let base_patch = if base != name && override_names.contains(base.as_str()) {
    Err(ConfigError::OverrideChained {
      name: name.clone(),
      base: base.clone(),
    })
  } else {
    bases
      .get(base)
      .ok_or_else(|| ConfigError::OverrideBaseMissing {
        name: name.clone(),
        base: base.clone(),
      })
  }?;
  patch_with_violations(name, &base_patch.clone().with_overrides(delta), strict)
    .map(|(patch, violations)| ResolvedOverride {
      name: name.clone(),
      patch,
      info: OverrideInfo {
        base: base.clone(),
        delta: delta
          .to_fields()
          .into_iter()
          .map(|(field, value)| (field.to_string(), value))
          .collect(),
      },
      violations,
    })
}

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

  fn tables(toml: &str) -> HashMap<String, toml::Value> {
    toml::from_str(toml).unwrap()
  }

  fn load(toml: &str) -> Result<LoadedLibrary, ConfigError> {
    load_library(&tables(toml), &[], false)
  }

  #[test]
  fn loads_whole_patches_and_overrides_over_the_presets() {
    let loaded = load(
      "[warm]\nfreq = 220.0\n\n\
       [warm-bright]\noverrides = \"warm\"\nbrightness = 1.6\n",
    )
    .unwrap();
    assert_eq!(loaded.library["warm"].freq, 220.0);
    assert_eq!(loaded.library["warm-bright"].freq, 220.0);
    assert_eq!(loaded.library["warm-bright"].brightness, 1.6);
    assert!(loaded.library.contains_key("sine"));
    let info = &loaded.overrides["warm-bright"];
    assert_eq!(info.base, "warm");
    assert_eq!(info.delta, HashMap::from([("brightness".to_string(), 1.6)]));
    assert!(loaded.limit_violations.is_empty());
  }

  #[test]
  fn an_override_may_replace_its_base_in_place() {
    let loaded = load("[sine]\noverrides = \"sine\"\nfreq = 880.0\n").unwrap();
    assert_eq!(loaded.library["sine"].freq, 880.0);
  }

  #[test]
  fn an_override_of_an_override_is_rejected() {
    let error = load(
      "[a]\noverrides = \"sine\"\n\n\
       [b]\noverrides = \"a\"\n",
    )
    .unwrap_err();
    assert!(
      matches!(&error, ConfigError::OverrideChained { name, base }
        if name == "b" && base == "a"),
      "{error}"
    );
  }

  #[test]
  fn an_override_of_a_missing_patch_is_rejected() {
    let error = load("[a]\noverrides = \"nowhere\"\n").unwrap_err();
    assert!(
      matches!(error, ConfigError::OverrideBaseMissing { .. }),
      "{error}"
    );
  }

  #[test]
  fn an_overrides_key_must_name_a_patch() {
    let error = load("[a]\noverrides = 3\n").unwrap_err();
    assert!(matches!(error, ConfigError::Validation(_)), "{error}");
  }

  #[test]
  fn holds_whole_patches_and_overrides_to_the_hard_limits() {
    let loaded = load(
      "[slow]\nattack_ms = -5.0\n\n\
       [wet]\noverrides = \"sine\"\nreverb_mix = 1.5\n",
    )
    .unwrap();
    assert_eq!(loaded.library["slow"].attack_ms, 0.0);
    assert_eq!(loaded.library["wet"].reverb_mix, 1.0);
    let mut moved: Vec<_> = loaded
      .limit_violations
      .iter()
      .map(|v| (v.patch.as_str(), v.violation.param))
      .collect();
    moved.sort_unstable();
    assert_eq!(moved, [("slow", "attack_ms"), ("wet", "reverb_mix")]);
  }

  #[test]
  fn strict_fails_on_a_value_past_a_hard_limit() {
    let error = load_library(&tables("[slow]\nattack_ms = -5.0\n"), &[], true)
      .unwrap_err();
    assert!(matches!(error, ConfigError::PatchLimit { .. }), "{error}");
  }

  #[test]
  fn loads_patches_from_a_library_file() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    std::io::Write::write_all(&mut file, b"[filed]\nfreq = 330.0\n").unwrap();
    let loaded =
      load_library(&HashMap::new(), &[file.path().to_path_buf()], false)
        .unwrap();
    assert_eq!(loaded.library["filed"].freq, 330.0);
  }

  #[test]
  fn reports_which_library_file_could_not_be_read() {
    let missing = PathBuf::from("/nonexistent/patches.toml");
    let error =
      load_library(&HashMap::new(), std::slice::from_ref(&missing), false)
        .unwrap_err();
    assert!(
      matches!(&error, ConfigError::PatchLibraryRead { path, .. }
        if *path == missing),
      "{error}"
    );
  }
}
