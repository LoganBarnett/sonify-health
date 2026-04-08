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

/// Format voice parameters as a TOML `[voice]` block suitable for
/// pasting into `config.toml`.
pub(crate) fn format_toml(
  voice: &Voice,
  scale_key: &str,
  boops: &[BoopSpec],
) -> String {
  let mut lines = vec![
    "[voice]".to_string(),
    format!("scale_key = \"{scale_key}\""),
    format!("base_freq = {}", float_lit(voice.base_freq)),
    format!("sine_ratio = {}", float_lit(voice.sine_ratio)),
    format!("tri_ratio = {}", float_lit(voice.tri_ratio)),
    format!("saw_ratio = {}", float_lit(voice.saw_ratio)),
    format!("attack_ms = {}", float_lit(voice.attack_ms)),
    format!("release_ms = {}", float_lit(voice.release_ms)),
    format!("chirp_ratio = {}", float_lit(voice.chirp_ratio)),
    format!("stereo_pan = {}", float_lit(voice.stereo_pan)),
    format!("reverb_mix = {}", float_lit(voice.reverb_mix)),
    format!("note_seed = {}", float_lit(voice.note_seed)),
    format!("echo_delay = {}", float_lit(voice.echo_delay)),
    format!("echo_mix = {}", float_lit(voice.echo_mix)),
    format!("brightness = {}", float_lit(voice.brightness)),
    format!("resonance = {}", float_lit(voice.resonance)),
    format!("sub_octave = {}", float_lit(voice.sub_octave)),
  ];
  for spec in boops {
    lines.push(String::new());
    lines.push("[[boops]]".to_string());
    lines.push(format!("freq = {}", float_lit(spec.freq)));
    lines.push(format!("duration = {}", float_lit(spec.duration)));
  }
  lines.join("\n")
}

/// Format voice parameters as a Nix attribute set.
pub(crate) fn format_nix(
  voice: &Voice,
  scale_key: &str,
  boops: &[BoopSpec],
) -> String {
  let mut lines = vec![
    "voice = {".to_string(),
    format!("  scale_key = \"{scale_key}\";"),
    format!("  base_freq = {};", float_lit(voice.base_freq)),
    format!("  sine_ratio = {};", float_lit(voice.sine_ratio)),
    format!("  tri_ratio = {};", float_lit(voice.tri_ratio)),
    format!("  saw_ratio = {};", float_lit(voice.saw_ratio)),
    format!("  attack_ms = {};", float_lit(voice.attack_ms)),
    format!("  release_ms = {};", float_lit(voice.release_ms)),
    format!("  chirp_ratio = {};", float_lit(voice.chirp_ratio)),
    format!("  stereo_pan = {};", float_lit(voice.stereo_pan)),
    format!("  reverb_mix = {};", float_lit(voice.reverb_mix)),
    format!("  note_seed = {};", float_lit(voice.note_seed)),
    format!("  echo_delay = {};", float_lit(voice.echo_delay)),
    format!("  echo_mix = {};", float_lit(voice.echo_mix)),
    format!("  brightness = {};", float_lit(voice.brightness)),
    format!("  resonance = {};", float_lit(voice.resonance)),
    format!("  sub_octave = {};", float_lit(voice.sub_octave)),
    "};".to_string(),
  ];
  if !boops.is_empty() {
    lines.push("boops = [".to_string());
    for spec in boops {
      lines.push(format!(
        "  {{ freq = {}; duration = {}; }}",
        float_lit(spec.freq),
        float_lit(spec.duration)
      ));
    }
    lines.push("];".to_string());
  }
  lines.join("\n")
}

/// Format voice parameters as a pretty-printed JSON object.
pub(crate) fn format_json(
  voice: &Voice,
  scale_key: &str,
  boops: &[BoopSpec],
) -> String {
  let mut obj = json!({
    "voice": {
      "scale_key": scale_key,
      "base_freq": voice.base_freq,
      "sine_ratio": voice.sine_ratio,
      "tri_ratio": voice.tri_ratio,
      "saw_ratio": voice.saw_ratio,
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
    }
  });
  if !boops.is_empty() {
    let boop_arr: Vec<_> = boops
      .iter()
      .map(|s| json!({"freq": s.freq, "duration": s.duration}))
      .collect();
    obj["boops"] = json!(boop_arr);
  }
  serde_json::to_string_pretty(&obj).unwrap()
}

/// Format voice parameters as CLI flags for round-tripping into
/// `preview` or `print`.
pub(crate) fn format_cli(voice: &Voice, scale_key: &str) -> String {
  [
    format!("--scale-key {scale_key}"),
    format!("--base-freq {}", voice.base_freq),
    format!("--sine-ratio {}", voice.sine_ratio),
    format!("--tri-ratio {}", voice.tri_ratio),
    format!("--saw-ratio {}", voice.saw_ratio),
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
  ]
  .join(" ")
}
