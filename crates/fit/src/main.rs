//! sonify-health-fit -- entry point.
//!
//! The `#[foundation_main]` macro handles CLI parsing, config resolution, and
//! logging init.  This file only dispatches the chosen subcommand to its
//! module.

mod command;
mod compare;
mod config;
mod fit;
mod patch_param;
mod recording;
mod render;
mod search;
mod spectrum;
mod synth;

use command::Command;
use compare::CompareError;
use config::Config;
use fit::FitError;
use render::RenderError;
use rust_template_foundation::main as foundation_main;
use std::process::ExitCode;
use thiserror::Error;

#[derive(Debug, Error)]
enum ApplicationError {
  #[error("Failed to render the patch: {0}")]
  Render(#[from] RenderError),

  #[error("Failed to fit a patch to the target: {0}")]
  Fit(#[from] FitError),

  #[error("Failed to compare the renders with the target: {0}")]
  Compare(#[from] CompareError),
}

#[foundation_main]
pub fn main(config: Config) -> Result<ExitCode, ApplicationError> {
  match &config.command {
    Command::Render(args) => render::run(args)?,
    Command::Fit(args) => fit::run(args)?,
    Command::Compare(args) => compare::run(args)?,
  }

  Ok(ExitCode::SUCCESS)
}
