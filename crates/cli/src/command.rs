//! Top-level CLI subcommand definitions.
//!
//! These live in the library half of the cli crate (rather than in
//! `main.rs`) so that `Config`'s `#[merge_config(subcommand)] command`
//! field can reference `Command` — the macro requires the subcommand
//! type to be in scope wherever `MergeConfig` is derived.

use crate::patch_args::CliPatchOverrides;

/// Output format for the `print` subcommand.
#[derive(Clone, Debug, clap::ValueEnum)]
pub enum PrintFormat {
  Toml,
  Nix,
  Cli,
}

#[derive(Clone, Debug, clap::Subcommand)]
pub enum Command {
  /// Preview a named patch from the library.
  Preview {
    /// Play continuously until interrupted (Ctrl-C).
    #[arg(long)]
    continuous: bool,

    #[command(flatten)]
    patch: CliPatchOverrides,
  },

  /// Print the patch library in a paste-ready format (TOML, Nix, or
  /// CLI flags).
  Print {
    /// Output format.
    #[arg(long, value_enum, default_value_t = PrintFormat::Toml)]
    format: PrintFormat,

    #[command(flatten)]
    patch: CliPatchOverrides,
  },
}
