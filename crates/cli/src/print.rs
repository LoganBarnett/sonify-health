use sonify_health_lib::{NoteSpec, Patch};

/// Ensure a float string contains a decimal point so TOML and Nix
/// parse it as a float, not an integer.
fn float_lit(v: f64) -> String {
  let s = v.to_string();
  if s.contains('.') || s.contains('e') || s.contains('E') {
    s
  } else {
    format!("{s}.0")
  }
}

/// Append TOML patch fields under a given table header.
fn patch_toml_lines(lines: &mut Vec<String>, header: &str, patch: &Patch) {
  lines.push(format!("[{header}]"));
  for meta in Patch::PARAMS {
    let val = patch.get_param(meta.name).unwrap_or(0.0);
    lines.push(format!("{} = {}", meta.name, float_lit(val)));
  }
}

/// Format all patches as a TOML document suitable for pasting into
/// `config.toml`.
pub(crate) fn format_toml(
  heartbeat_patch: &Patch,
  drone_profiles: &[(String, Patch, Patch)],
  boops: &[NoteSpec],
  drone_notes: &[(String, Vec<NoteSpec>)],
) -> String {
  let mut lines = Vec::new();
  patch_toml_lines(&mut lines, "heartbeat.patch", heartbeat_patch);
  for spec in boops {
    lines.push(String::new());
    lines.push("[[heartbeat.notes]]".to_string());
    lines.push(format!("freq = {}", float_lit(spec.freq)));
    lines.push(format!("duration = {}", float_lit(spec.duration)));
  }
  for (name, specs) in drone_notes {
    for spec in specs {
      lines.push(String::new());
      lines.push(format!("[[drone_notes.{name}]]"));
      lines.push(format!("freq = {}", float_lit(spec.freq)));
      lines.push(format!("duration = {}", float_lit(spec.duration)));
    }
  }
  for (name, lo, hi) in drone_profiles {
    lines.push(String::new());
    patch_toml_lines(&mut lines, &format!("drone_profiles.{name}.lo"), lo);
    lines.push(String::new());
    patch_toml_lines(&mut lines, &format!("drone_profiles.{name}.hi"), hi);
  }
  lines.join("\n")
}

/// Append Nix patch attribute set lines.
fn patch_nix_lines(lines: &mut Vec<String>, prefix: &str, patch: &Patch) {
  lines.push(format!("{prefix} = {{"));
  for meta in Patch::PARAMS {
    let val = patch.get_param(meta.name).unwrap_or(0.0);
    lines.push(format!("  {} = {};", meta.name, float_lit(val)));
  }
  lines.push("};".to_string());
}

/// Format all patches as Nix attribute sets.
pub(crate) fn format_nix(
  heartbeat_patch: &Patch,
  drone_profiles: &[(String, Patch, Patch)],
  boops: &[NoteSpec],
  drone_notes: &[(String, Vec<NoteSpec>)],
) -> String {
  let mut lines = Vec::new();
  patch_nix_lines(&mut lines, "heartbeat.patch", heartbeat_patch);
  if !boops.is_empty() {
    lines.push("heartbeat.notes = [".to_string());
    for spec in boops {
      lines.push(format!(
        "  {{ freq = {}; duration = {}; }}",
        float_lit(spec.freq),
        float_lit(spec.duration)
      ));
    }
    lines.push("];".to_string());
  }
  for (name, specs) in drone_notes {
    if !specs.is_empty() {
      lines.push(format!("drone_notes.{name} = ["));
      for spec in specs {
        lines.push(format!(
          "  {{ freq = {}; duration = {}; }}",
          float_lit(spec.freq),
          float_lit(spec.duration)
        ));
      }
      lines.push("];".to_string());
    }
  }
  for (name, lo, hi) in drone_profiles {
    patch_nix_lines(&mut lines, &format!("drone_profiles.{name}.lo"), lo);
    patch_nix_lines(&mut lines, &format!("drone_profiles.{name}.hi"), hi);
  }
  lines.join("\n")
}

/// Format all patches as a pretty-printed JSON object.
pub(crate) fn format_json(
  heartbeat_patch: &Patch,
  drone_profiles: &[(String, Patch, Patch)],
  boops: &[NoteSpec],
  drone_notes: &[(String, Vec<NoteSpec>)],
) -> String {
  use serde_json::json;

  let patch_json = serde_json::to_value(heartbeat_patch).unwrap_or_default();
  let mut heartbeat_obj = json!({ "patch": patch_json });
  if !boops.is_empty() {
    let notes_arr: Vec<_> = boops
      .iter()
      .map(|s| json!({"freq": s.freq, "duration": s.duration}))
      .collect();
    heartbeat_obj["notes"] = json!(notes_arr);
  }
  let mut drone_profiles_obj = json!({});
  for (name, lo, hi) in drone_profiles {
    drone_profiles_obj[name] = json!({
      "lo": serde_json::to_value(lo).unwrap_or_default(),
      "hi": serde_json::to_value(hi).unwrap_or_default(),
    });
  }
  let mut drone_notes_obj = json!({});
  for (name, specs) in drone_notes {
    let notes_arr: Vec<_> = specs
      .iter()
      .map(|s| json!({"freq": s.freq, "duration": s.duration}))
      .collect();
    drone_notes_obj[name] = json!(notes_arr);
  }
  let obj = json!({
    "heartbeat": heartbeat_obj,
    "drone_profiles": drone_profiles_obj,
    "drone_notes": drone_notes_obj,
  });
  serde_json::to_string_pretty(&obj).unwrap()
}

/// Format patch parameters as CLI flags for round-tripping into
/// `preview` or `print`.
pub(crate) fn format_cli(patch: &Patch) -> String {
  Patch::PARAMS
    .iter()
    .map(|meta| {
      let val = patch.get_param(meta.name).unwrap_or(0.0);
      let flag = meta.name.replace('_', "-");
      format!("--{flag} {val}")
    })
    .collect::<Vec<_>>()
    .join(" ")
}
