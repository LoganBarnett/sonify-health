//! The `compare` subcommand: measure renders against a target recording.

use crate::recording::{self, RecordingError};
use crate::spectrum::{
  fundamental_hz, shared_harmonics, Features, BAND_EDGES_HZ,
};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Clone, Debug, clap::Args)]
pub struct CompareArgs {
  /// Recording to compare against: WAV or MP3.
  #[arg(long)]
  pub target: PathBuf,

  /// Fundamental of the renders' harmonic series, in Hz.  For a patch that
  /// mixes in its sub-octave this is half the patch's `freq`.
  #[arg(long, value_parser = fundamental_hz)]
  pub fundamental: f64,

  /// Fundamental of the target, in Hz, when it sits slightly off the
  /// renders'.  Defaults to --fundamental.
  #[arg(long, value_parser = fundamental_hz)]
  pub target_fundamental: Option<f64>,

  /// Seconds discarded from the start of each render, to step over its
  /// attack.  The target is measured whole.
  #[arg(long, default_value_t = 0.6)]
  pub settle_seconds: f64,

  /// Harmonics listed in the table.
  #[arg(long, default_value_t = 24)]
  pub rows: usize,

  /// Audio files to measure, such as the output of `render`.
  #[arg(required = true)]
  pub renders: Vec<PathBuf>,
}

#[derive(Debug, Error)]
pub enum CompareError {
  #[error("A recording could not be loaded: {0}")]
  Recording(#[from] RecordingError),

  #[error("The recording {path:?} is silent, so there is nothing to compare")]
  Silent { path: PathBuf },
}

struct Column {
  label: String,
  features: Features,
}

pub fn run(args: &CompareArgs) -> Result<(), CompareError> {
  let target_fundamental = args.target_fundamental.unwrap_or(args.fundamental);
  let target = recording::load(&args.target)?;
  let renders = args
    .renders
    .iter()
    .map(|path| recording::load(path))
    .collect::<Result<Vec<_>, _>>()?;
  let harmonics = shared_harmonics(
    &std::iter::once((target.sample_rate, target_fundamental))
      .chain(renders.iter().map(|r| (r.sample_rate, args.fundamental)))
      .collect::<Vec<_>>(),
  );

  let measured = |path: &Path, samples: &[f64], rate: f64, fundamental: f64| {
    Features::measure(samples, rate, fundamental, harmonics)
      .map(|features| Column {
        label: label(path),
        features,
      })
      .ok_or_else(|| CompareError::Silent {
        path: path.to_path_buf(),
      })
  };
  let target_column = measured(
    &args.target,
    &target.samples,
    target.sample_rate,
    target_fundamental,
  )?;
  let render_columns = args
    .renders
    .iter()
    .zip(&renders)
    .map(|(path, render)| {
      let settle = (args.settle_seconds.max(0.0) * render.sample_rate) as usize;
      measured(
        path,
        render.samples.get(settle..).unwrap_or_default(),
        render.sample_rate,
        args.fundamental,
      )
    })
    .collect::<Result<Vec<_>, _>>()?;

  println!(
    "{}",
    table(&target_column, &render_columns, target_fundamental, args.rows)
  );
  Ok(())
}

/// A column heading: the file's stem, cut to the column width.
fn label(path: &Path) -> String {
  path
    .file_stem()
    .map(|stem| stem.to_string_lossy().chars().take(11).collect())
    .unwrap_or_default()
}

fn row(heading: &str, cells: impl Iterator<Item = String>) -> String {
  std::iter::once(format!("{heading:<18}"))
    .chain(cells.map(|cell| format!("{cell:>12}")))
    .collect()
}

fn table(
  target: &Column,
  renders: &[Column],
  target_fundamental: f64,
  rows: usize,
) -> String {
  let columns: Vec<&Column> = std::iter::once(target).chain(renders).collect();
  let per_column = |heading: String, cell: &dyn Fn(&Column) -> String| {
    row(&heading, columns.iter().map(|column| cell(column)))
  };
  let decibels = |value: Option<&f64>| {
    value.map_or_else(|| "-".to_string(), |db| format!("{db:.1}"))
  };

  let header = per_column("harmonic (dB)".to_string(), &|c| c.label.clone());
  let harmonic_rows =
    (0..rows.min(target.features.harmonic_db.len())).map(|k| {
      per_column(
        format!("h{:<3}{:>9.1} Hz", k + 1, (k + 1) as f64 * target_fundamental),
        &|c| decibels(c.features.harmonic_db.get(k)),
      )
    });
  let band_header =
    per_column("band share (dB)".to_string(), &|_| String::new());
  let band_rows = BAND_EDGES_HZ.windows(2).enumerate().map(|(band, edge)| {
    per_column(format!("{:>6.0}-{:<6.0} Hz", edge[0], edge[1]), &|c| {
      decibels(c.features.band_db.get(band))
    })
  });
  let summary = [
    per_column("crest factor".to_string(), &|c| {
      format!("{:.2}", c.features.crest)
    }),
    per_column("off-harmonic (dB)".to_string(), &|c| {
      format!("{:.1}", c.features.off_harmonic_db)
    }),
    per_column("distance".to_string(), &|c| {
      format!("{:.2}", target.features.distance(&c.features))
    }),
  ];

  std::iter::once(header)
    .chain(harmonic_rows)
    .chain(std::iter::once(band_header))
    .chain(band_rows)
    .chain(summary)
    .collect::<Vec<_>>()
    .join("\n")
}
