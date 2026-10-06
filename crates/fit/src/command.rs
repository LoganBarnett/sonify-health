//! Top-level subcommand definitions.  Each variant carries the argument struct
//! its own module defines.

use crate::compare::CompareArgs;
use crate::fit::FitArgs;
use crate::render::RenderArgs;

#[derive(Clone, Debug, clap::Subcommand)]
pub enum Command {
  /// Render one patch to a WAV file through the graph the daemon plays.
  Render(RenderArgs),

  /// Search for the patch parameters that best reproduce a recorded tone.
  Fit(FitArgs),

  /// Tabulate how closely rendered audio matches a recorded tone.
  Compare(CompareArgs),
}
