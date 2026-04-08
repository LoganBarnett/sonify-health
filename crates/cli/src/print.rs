use serde_json::json;
use sonify_health_lib::{BoopSpec, Voice};

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

/// Append TOML voice fields under a given table header.
fn voice_toml_lines(lines: &mut Vec<String>, header: &str, voice: &Voice) {
  lines.push(format!("[{header}]"));
  lines.push(format!("base_freq = {}", float_lit(voice.base_freq)));
  lines.push(format!("sine_ratio = {}", float_lit(voice.sine_ratio)));
  lines.push(format!("tri_ratio = {}", float_lit(voice.tri_ratio)));
  lines.push(format!("saw_ratio = {}", float_lit(voice.saw_ratio)));
  lines.push(format!("square_ratio = {}", float_lit(voice.square_ratio)));
  lines.push(format!("attack_ms = {}", float_lit(voice.attack_ms)));
  lines.push(format!("release_ms = {}", float_lit(voice.release_ms)));
  lines.push(format!("chirp_ratio = {}", float_lit(voice.chirp_ratio)));
  lines.push(format!("stereo_pan = {}", float_lit(voice.stereo_pan)));
  lines.push(format!("reverb_mix = {}", float_lit(voice.reverb_mix)));
  lines.push(format!("note_seed = {}", float_lit(voice.note_seed)));
  lines.push(format!("echo_delay = {}", float_lit(voice.echo_delay)));
  lines.push(format!("echo_mix = {}", float_lit(voice.echo_mix)));
  lines.push(format!("brightness = {}", float_lit(voice.brightness)));
  lines.push(format!("resonance = {}", float_lit(voice.resonance)));
  lines.push(format!("sub_octave = {}", float_lit(voice.sub_octave)));
  lines.push(format!("vibrato_rate = {}", float_lit(voice.vibrato_rate)));
  lines.push(format!("vibrato_depth = {}", float_lit(voice.vibrato_depth)));
  lines.push(format!("tremolo_rate = {}", float_lit(voice.tremolo_rate)));
  lines.push(format!("tremolo_depth = {}", float_lit(voice.tremolo_depth)));
  lines.push(format!("amplitude = {}", float_lit(voice.amplitude)));
  lines.push(format!("drive = {}", float_lit(voice.drive)));
  lines.push(format!("noise_mix = {}", float_lit(voice.noise_mix)));
  lines.push(format!("crush = {}", float_lit(voice.crush)));
  lines.push(format!("fm_ratio = {}", float_lit(voice.fm_ratio)));
  lines.push(format!("fm_depth = {}", float_lit(voice.fm_depth)));
  lines.push(format!("downsample = {}", float_lit(voice.downsample)));
}

/// Format all voices as a TOML document suitable for pasting into
/// `config.toml`.
pub(crate) fn format_toml(
  heartbeat_voice: &Voice,
  drone_profiles: &[(String, Voice, Voice)],
  boops: &[BoopSpec],
  drone_notes: &[(String, Vec<BoopSpec>)],
) -> String {
  let mut lines = Vec::new();
  voice_toml_lines(&mut lines, "heartbeat.voice", heartbeat_voice);
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
    voice_toml_lines(&mut lines, &format!("drone_profiles.{name}.lo"), lo);
    lines.push(String::new());
    voice_toml_lines(&mut lines, &format!("drone_profiles.{name}.hi"), hi);
  }
  lines.join("\n")
}

/// Append Nix voice attribute set lines.
fn voice_nix_lines(lines: &mut Vec<String>, prefix: &str, voice: &Voice) {
  lines.push(format!("{prefix} = {{"));
  lines.push(format!("  base_freq = {};", float_lit(voice.base_freq)));
  lines.push(format!("  sine_ratio = {};", float_lit(voice.sine_ratio)));
  lines.push(format!("  tri_ratio = {};", float_lit(voice.tri_ratio)));
  lines.push(format!("  saw_ratio = {};", float_lit(voice.saw_ratio)));
  lines.push(format!("  square_ratio = {};", float_lit(voice.square_ratio)));
  lines.push(format!("  attack_ms = {};", float_lit(voice.attack_ms)));
  lines.push(format!("  release_ms = {};", float_lit(voice.release_ms)));
  lines.push(format!("  chirp_ratio = {};", float_lit(voice.chirp_ratio)));
  lines.push(format!("  stereo_pan = {};", float_lit(voice.stereo_pan)));
  lines.push(format!("  reverb_mix = {};", float_lit(voice.reverb_mix)));
  lines.push(format!("  note_seed = {};", float_lit(voice.note_seed)));
  lines.push(format!("  echo_delay = {};", float_lit(voice.echo_delay)));
  lines.push(format!("  echo_mix = {};", float_lit(voice.echo_mix)));
  lines.push(format!("  brightness = {};", float_lit(voice.brightness)));
  lines.push(format!("  resonance = {};", float_lit(voice.resonance)));
  lines.push(format!("  sub_octave = {};", float_lit(voice.sub_octave)));
  lines.push(format!("  vibrato_rate = {};", float_lit(voice.vibrato_rate)));
  lines.push(format!("  vibrato_depth = {};", float_lit(voice.vibrato_depth)));
  lines.push(format!("  tremolo_rate = {};", float_lit(voice.tremolo_rate)));
  lines.push(format!("  tremolo_depth = {};", float_lit(voice.tremolo_depth)));
  lines.push(format!("  amplitude = {};", float_lit(voice.amplitude)));
  lines.push(format!("  drive = {};", float_lit(voice.drive)));
  lines.push(format!("  noise_mix = {};", float_lit(voice.noise_mix)));
  lines.push(format!("  crush = {};", float_lit(voice.crush)));
  lines.push(format!("  fm_ratio = {};", float_lit(voice.fm_ratio)));
  lines.push(format!("  fm_depth = {};", float_lit(voice.fm_depth)));
  lines.push(format!("  downsample = {};", float_lit(voice.downsample)));
  lines.push("};".to_string());
}

/// Format all voices as Nix attribute sets.
pub(crate) fn format_nix(
  heartbeat_voice: &Voice,
  drone_profiles: &[(String, Voice, Voice)],
  boops: &[BoopSpec],
  drone_notes: &[(String, Vec<BoopSpec>)],
) -> String {
  let mut lines = Vec::new();
  voice_nix_lines(&mut lines, "heartbeat.voice", heartbeat_voice);
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
    voice_nix_lines(&mut lines, &format!("drone_profiles.{name}.lo"), lo);
    voice_nix_lines(&mut lines, &format!("drone_profiles.{name}.hi"), hi);
  }
  lines.join("\n")
}

fn voice_to_json_value(voice: &Voice) -> serde_json::Value {
  json!({
    "base_freq": voice.base_freq,
    "sine_ratio": voice.sine_ratio,
    "tri_ratio": voice.tri_ratio,
    "saw_ratio": voice.saw_ratio,
    "square_ratio": voice.square_ratio,
    "attack_ms": voice.attack_ms,
    "release_ms": voice.release_ms,
    "chirp_ratio": voice.chirp_ratio,
    "stereo_pan": voice.stereo_pan,
    "reverb_mix": voice.reverb_mix,
    "note_seed": voice.note_seed,
    "echo_delay": voice.echo_delay,
    "echo_mix": voice.echo_mix,
    "brightness": voice.brightness,
    "resonance": voice.resonance,
    "sub_octave": voice.sub_octave,
    "vibrato_rate": voice.vibrato_rate,
    "vibrato_depth": voice.vibrato_depth,
    "tremolo_rate": voice.tremolo_rate,
    "tremolo_depth": voice.tremolo_depth,
    "amplitude": voice.amplitude,
    "drive": voice.drive,
    "noise_mix": voice.noise_mix,
    "crush": voice.crush,
    "fm_ratio": voice.fm_ratio,
    "fm_depth": voice.fm_depth,
    "downsample": voice.downsample,
  })
}

/// Format all voices as a pretty-printed JSON object.
pub(crate) fn format_json(
  heartbeat_voice: &Voice,
  drone_profiles: &[(String, Voice, Voice)],
  boops: &[BoopSpec],
  drone_notes: &[(String, Vec<BoopSpec>)],
) -> String {
  let mut heartbeat_obj =
    json!({ "voice": voice_to_json_value(heartbeat_voice) });
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
      "lo": voice_to_json_value(lo),
      "hi": voice_to_json_value(hi),
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

/// Format voice parameters as CLI flags for round-tripping into
/// `preview` or `print`.
pub(crate) fn format_cli(voice: &Voice) -> String {
  [
    format!("--base-freq {}", voice.base_freq),
    format!("--sine-ratio {}", voice.sine_ratio),
    format!("--tri-ratio {}", voice.tri_ratio),
    format!("--saw-ratio {}", voice.saw_ratio),
    format!("--square-ratio {}", voice.square_ratio),
    format!("--attack-ms {}", voice.attack_ms),
    format!("--release-ms {}", voice.release_ms),
    format!("--chirp-ratio {}", voice.chirp_ratio),
    format!("--stereo-pan {}", voice.stereo_pan),
    format!("--reverb-mix {}", voice.reverb_mix),
    format!("--note-seed {}", voice.note_seed),
    format!("--echo-delay {}", voice.echo_delay),
    format!("--echo-mix {}", voice.echo_mix),
    format!("--brightness {}", voice.brightness),
    format!("--resonance {}", voice.resonance),
    format!("--sub-octave {}", voice.sub_octave),
    format!("--vibrato-rate {}", voice.vibrato_rate),
    format!("--vibrato-depth {}", voice.vibrato_depth),
    format!("--tremolo-rate {}", voice.tremolo_rate),
    format!("--tremolo-depth {}", voice.tremolo_depth),
    format!("--amplitude {}", voice.amplitude),
    format!("--drive {}", voice.drive),
    format!("--noise-mix {}", voice.noise_mix),
    format!("--crush {}", voice.crush),
    format!("--fm-ratio {}", voice.fm_ratio),
    format!("--fm-depth {}", voice.fm_depth),
    format!("--downsample {}", voice.downsample),
  ]
  .join(" ")
}
