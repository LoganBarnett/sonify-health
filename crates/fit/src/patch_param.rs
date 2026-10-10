//! Patch parameter assignments given on the command line as `NAME=VALUE`.

use sonify_health_lib::Patch;
use std::num::ParseFloatError;
use std::str::FromStr;
use tap::Tap;
use thiserror::Error;
use tracing::warn;

/// One `NAME=VALUE` pair that is well formed but not yet checked against the
/// patch model: whether the name is a real parameter is only known once the
/// assignment is applied to a patch.
#[derive(Clone, Debug, PartialEq)]
pub struct ParamAssignment {
  pub name: String,
  pub value: f64,
}

#[derive(Debug, Error)]
pub enum ParamAssignmentParseError {
  #[error("expected NAME=VALUE, got {argument:?}")]
  MissingSeparator { argument: String },

  #[error(
    "the value {value:?} given for patch parameter {name:?} is not a \
     number: {source}"
  )]
  ValueNotANumber {
    name: String,
    value: String,
    #[source]
    source: ParseFloatError,
  },
}

impl FromStr for ParamAssignment {
  type Err = ParamAssignmentParseError;

  fn from_str(argument: &str) -> Result<Self, Self::Err> {
    argument
      .split_once('=')
      .ok_or_else(|| ParamAssignmentParseError::MissingSeparator {
        argument: argument.to_string(),
      })
      .and_then(|(name, value)| {
        value
          .parse()
          .map(|value| ParamAssignment {
            name: name.to_string(),
            value,
          })
          .map_err(|source| ParamAssignmentParseError::ValueNotANumber {
            name: name.to_string(),
            value: value.to_string(),
            source,
          })
      })
  }
}

#[derive(Debug, Error)]
pub enum PatchParamError {
  #[error(
    "Unknown patch parameter {name:?}; the known parameters are: {}",
    known_parameter_names()
  )]
  UnknownParameter { name: String },

  #[error(
    "The value {value} given for patch parameter {name:?} is not finite"
  )]
  NonFiniteValue { name: String, value: f64 },
}

fn known_parameter_names() -> String {
  Patch::PARAMS
    .iter()
    .map(|meta| meta.name)
    .collect::<Vec<_>>()
    .join(", ")
}

/// `base` with every assignment applied in order, so a later assignment to
/// the same parameter wins.  Each value is held to its parameter's hard
/// limits; a value past the slider range is kept.
pub fn patch_with_params(
  base: Patch,
  params: &[ParamAssignment],
) -> Result<Patch, PatchParamError> {
  params.iter().try_fold(base, with_param)
}

fn with_param(
  mut patch: Patch,
  param: &ParamAssignment,
) -> Result<Patch, PatchParamError> {
  // `Patch::set_param` answers an unknown name and a non-finite value with the
  // same `false`.  `get_param` tells the two apart, so the error names the
  // actual problem.
  if patch.set_param(&param.name, param.value) {
    Ok(patch.tap(|patch| warn_if_clamped(patch, param)))
  } else if patch.get_param(&param.name).is_none() {
    Err(PatchParamError::UnknownParameter {
      name: param.name.clone(),
    })
  } else {
    Err(PatchParamError::NonFiniteValue {
      name: param.name.clone(),
      value: param.value,
    })
  }
}

fn warn_if_clamped(patch: &Patch, param: &ParamAssignment) {
  if let Some(applied) = patch
    .get_param(&param.name)
    .filter(|applied| *applied != param.value)
  {
    warn!(
      name = %param.name,
      requested = param.value,
      applied,
      "Patch parameter held at its hard limit"
    );
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn assignment(name: &str, value: f64) -> ParamAssignment {
    ParamAssignment {
      name: name.to_string(),
      value,
    }
  }

  #[test]
  fn parses_name_and_value() {
    let parsed: ParamAssignment = "freq=220.5".parse().unwrap();
    assert_eq!(parsed, assignment("freq", 220.5));
  }

  #[test]
  fn parses_negative_value() {
    let parsed: ParamAssignment = "stereo_pan=-0.5".parse().unwrap();
    assert_eq!(parsed, assignment("stereo_pan", -0.5));
  }

  #[test]
  fn rejects_argument_without_separator() {
    let error = "freq".parse::<ParamAssignment>().unwrap_err();
    assert!(
      matches!(
        &error,
        ParamAssignmentParseError::MissingSeparator { argument }
          if argument == "freq"
      ),
      "unexpected error: {error:?}"
    );
  }

  #[test]
  fn rejects_non_numeric_value() {
    let error = "freq=loud".parse::<ParamAssignment>().unwrap_err();
    assert!(
      matches!(
        &error,
        ParamAssignmentParseError::ValueNotANumber { name, value, .. }
          if name == "freq" && value == "loud"
      ),
      "unexpected error: {error:?}"
    );
  }

  #[test]
  fn applies_assignments_over_the_base() {
    let patch = patch_with_params(
      Patch::default(),
      &[assignment("freq", 220.0), assignment("saw_ratio", 0.5)],
    )
    .unwrap();
    assert_eq!(patch.freq, 220.0);
    assert_eq!(patch.saw_ratio, 0.5);
    assert_eq!(patch.sine_ratio, Patch::default().sine_ratio);
  }

  #[test]
  fn later_assignment_wins() {
    let patch = patch_with_params(
      Patch::default(),
      &[assignment("freq", 220.0), assignment("freq", 330.0)],
    )
    .unwrap();
    assert_eq!(patch.freq, 330.0);
  }

  #[test]
  fn holds_to_the_hard_limits_but_not_the_slider() {
    let patch = patch_with_params(
      Patch::default(),
      &[assignment("freq", 50_000.0), assignment("attack_ms", -5.0)],
    )
    .unwrap();
    assert_eq!(patch.freq, 50_000.0);
    assert_eq!(patch.attack_ms, 0.0);
  }

  #[test]
  fn unknown_parameter_error_names_it_and_lists_the_known_ones() {
    let error =
      patch_with_params(Patch::default(), &[assignment("wobble", 1.0)])
        .unwrap_err();
    assert!(
      matches!(
        &error,
        PatchParamError::UnknownParameter { name } if name == "wobble"
      ),
      "unexpected error: {error:?}"
    );
    let message = error.to_string();
    assert!(message.contains("\"wobble\""), "message: {message}");
    assert!(message.contains("freq, duration"), "message: {message}");
  }

  #[test]
  fn non_finite_value_is_told_apart_from_an_unknown_name() {
    let error =
      patch_with_params(Patch::default(), &[assignment("freq", f64::NAN)])
        .unwrap_err();
    assert!(
      matches!(
        &error,
        PatchParamError::NonFiniteValue { name, .. } if name == "freq"
      ),
      "unexpected error: {error:?}"
    );
  }
}
