use sonify_health_lib::{Patch, PatchLibrary};

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

/// Format the patch library as a TOML document suitable for pasting
/// into `config.toml`.
pub(crate) fn format_toml(library: &PatchLibrary) -> String {
  let mut lines = Vec::new();
  for (name, patch) in library {
    if !lines.is_empty() {
      lines.push(String::new());
    }
    patch_toml_lines(&mut lines, &format!("patches.{name}"), patch);
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

/// Format the patch library as Nix attribute sets.
pub(crate) fn format_nix(library: &PatchLibrary) -> String {
  let mut lines = vec!["patches = {".to_string()];
  for (name, patch) in library {
    patch_nix_lines(&mut lines, &format!("  {name}"), patch);
  }
  lines.push("};".to_string());
  lines.join("\n")
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
