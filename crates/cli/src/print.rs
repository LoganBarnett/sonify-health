use serde_json::json;
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
  lines.push(format!("base_freq = {}", float_lit(patch.base_freq)));
  lines.push(format!("sine_ratio = {}", float_lit(patch.sine_ratio)));
  lines.push(format!("tri_ratio = {}", float_lit(patch.tri_ratio)));
  lines.push(format!("saw_ratio = {}", float_lit(patch.saw_ratio)));
  lines.push(format!("square_ratio = {}", float_lit(patch.square_ratio)));
  lines.push(format!("attack_ms = {}", float_lit(patch.attack_ms)));
  lines.push(format!("release_ms = {}", float_lit(patch.release_ms)));
  lines.push(format!("chirp_ratio = {}", float_lit(patch.chirp_ratio)));
  lines.push(format!("stereo_pan = {}", float_lit(patch.stereo_pan)));
  lines.push(format!("reverb_mix = {}", float_lit(patch.reverb_mix)));
  lines.push(format!("note_seed = {}", float_lit(patch.note_seed)));
  lines.push(format!("echo_delay = {}", float_lit(patch.echo_delay)));
  lines.push(format!("echo_mix = {}", float_lit(patch.echo_mix)));
  lines.push(format!("brightness = {}", float_lit(patch.brightness)));
  lines.push(format!("resonance = {}", float_lit(patch.resonance)));
  lines.push(format!("sub_octave = {}", float_lit(patch.sub_octave)));
  lines.push(format!("vibrato_rate = {}", float_lit(patch.vibrato_rate)));
  lines.push(format!("vibrato_depth = {}", float_lit(patch.vibrato_depth)));
  lines.push(format!("tremolo_rate = {}", float_lit(patch.tremolo_rate)));
  lines.push(format!("tremolo_depth = {}", float_lit(patch.tremolo_depth)));
  lines.push(format!("amplitude = {}", float_lit(patch.amplitude)));
  lines.push(format!("drive = {}", float_lit(patch.drive)));
  lines.push(format!("noise_mix = {}", float_lit(patch.noise_mix)));
  lines.push(format!("crush = {}", float_lit(patch.crush)));
  lines.push(format!("fm_ratio = {}", float_lit(patch.fm_ratio)));
  lines.push(format!("fm_depth = {}", float_lit(patch.fm_depth)));
  lines.push(format!("downsample = {}", float_lit(patch.downsample)));
  lines.push(format!("sustain = {}", float_lit(patch.sustain)));
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
  lines.push(format!("  base_freq = {};", float_lit(patch.base_freq)));
  lines.push(format!("  sine_ratio = {};", float_lit(patch.sine_ratio)));
  lines.push(format!("  tri_ratio = {};", float_lit(patch.tri_ratio)));
  lines.push(format!("  saw_ratio = {};", float_lit(patch.saw_ratio)));
  lines.push(format!("  square_ratio = {};", float_lit(patch.square_ratio)));
  lines.push(format!("  attack_ms = {};", float_lit(patch.attack_ms)));
  lines.push(format!("  release_ms = {};", float_lit(patch.release_ms)));
  lines.push(format!("  chirp_ratio = {};", float_lit(patch.chirp_ratio)));
  lines.push(format!("  stereo_pan = {};", float_lit(patch.stereo_pan)));
  lines.push(format!("  reverb_mix = {};", float_lit(patch.reverb_mix)));
  lines.push(format!("  note_seed = {};", float_lit(patch.note_seed)));
  lines.push(format!("  echo_delay = {};", float_lit(patch.echo_delay)));
  lines.push(format!("  echo_mix = {};", float_lit(patch.echo_mix)));
  lines.push(format!("  brightness = {};", float_lit(patch.brightness)));
  lines.push(format!("  resonance = {};", float_lit(patch.resonance)));
  lines.push(format!("  sub_octave = {};", float_lit(patch.sub_octave)));
  lines.push(format!("  vibrato_rate = {};", float_lit(patch.vibrato_rate)));
  lines.push(format!("  vibrato_depth = {};", float_lit(patch.vibrato_depth)));
  lines.push(format!("  tremolo_rate = {};", float_lit(patch.tremolo_rate)));
  lines.push(format!("  tremolo_depth = {};", float_lit(patch.tremolo_depth)));
  lines.push(format!("  amplitude = {};", float_lit(patch.amplitude)));
  lines.push(format!("  drive = {};", float_lit(patch.drive)));
  lines.push(format!("  noise_mix = {};", float_lit(patch.noise_mix)));
  lines.push(format!("  crush = {};", float_lit(patch.crush)));
  lines.push(format!("  fm_ratio = {};", float_lit(patch.fm_ratio)));
  lines.push(format!("  fm_depth = {};", float_lit(patch.fm_depth)));
  lines.push(format!("  downsample = {};", float_lit(patch.downsample)));
  lines.push(format!("  sustain = {};", float_lit(patch.sustain)));
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

fn patch_to_json_value(patch: &Patch) -> serde_json::Value {
  json!({
    "base_freq": patch.base_freq,
    "sine_ratio": patch.sine_ratio,
    "tri_ratio": patch.tri_ratio,
    "saw_ratio": patch.saw_ratio,
    "square_ratio": patch.square_ratio,
    "attack_ms": patch.attack_ms,
    "release_ms": patch.release_ms,
    "chirp_ratio": patch.chirp_ratio,
    "stereo_pan": patch.stereo_pan,
    "reverb_mix": patch.reverb_mix,
    "note_seed": patch.note_seed,
    "echo_delay": patch.echo_delay,
    "echo_mix": patch.echo_mix,
    "brightness": patch.brightness,
    "resonance": patch.resonance,
    "sub_octave": patch.sub_octave,
    "vibrato_rate": patch.vibrato_rate,
    "vibrato_depth": patch.vibrato_depth,
    "tremolo_rate": patch.tremolo_rate,
    "tremolo_depth": patch.tremolo_depth,
    "amplitude": patch.amplitude,
    "drive": patch.drive,
    "noise_mix": patch.noise_mix,
    "crush": patch.crush,
    "fm_ratio": patch.fm_ratio,
    "fm_depth": patch.fm_depth,
    "downsample": patch.downsample,
    "sustain": patch.sustain,
  })
}

/// Format all patches as a pretty-printed JSON object.
pub(crate) fn format_json(
  heartbeat_patch: &Patch,
  drone_profiles: &[(String, Patch, Patch)],
  boops: &[NoteSpec],
  drone_notes: &[(String, Vec<NoteSpec>)],
) -> String {
  let mut heartbeat_obj =
    json!({ "patch": patch_to_json_value(heartbeat_patch) });
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
      "lo": patch_to_json_value(lo),
      "hi": patch_to_json_value(hi),
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
  [
    format!("--base-freq {}", patch.base_freq),
    format!("--sine-ratio {}", patch.sine_ratio),
    format!("--tri-ratio {}", patch.tri_ratio),
    format!("--saw-ratio {}", patch.saw_ratio),
    format!("--square-ratio {}", patch.square_ratio),
    format!("--attack-ms {}", patch.attack_ms),
    format!("--release-ms {}", patch.release_ms),
    format!("--chirp-ratio {}", patch.chirp_ratio),
    format!("--stereo-pan {}", patch.stereo_pan),
    format!("--reverb-mix {}", patch.reverb_mix),
    format!("--note-seed {}", patch.note_seed),
    format!("--echo-delay {}", patch.echo_delay),
    format!("--echo-mix {}", patch.echo_mix),
    format!("--brightness {}", patch.brightness),
    format!("--resonance {}", patch.resonance),
    format!("--sub-octave {}", patch.sub_octave),
    format!("--vibrato-rate {}", patch.vibrato_rate),
    format!("--vibrato-depth {}", patch.vibrato_depth),
    format!("--tremolo-rate {}", patch.tremolo_rate),
    format!("--tremolo-depth {}", patch.tremolo_depth),
    format!("--amplitude {}", patch.amplitude),
    format!("--drive {}", patch.drive),
    format!("--noise-mix {}", patch.noise_mix),
    format!("--crush {}", patch.crush),
    format!("--fm-ratio {}", patch.fm_ratio),
    format!("--fm-depth {}", patch.fm_depth),
    format!("--downsample {}", patch.downsample),
    format!("--sustain {}", patch.sustain),
  ]
  .join(" ")
}
