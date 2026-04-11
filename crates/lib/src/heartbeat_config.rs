use crate::probe::ResultMode;
use crate::transition::Transition;
use serde::{Deserialize, Serialize};

#[derive(
  Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "kebab-case")]
pub enum Playback {
  #[default]
  Clock,
  Loop,
  Continuous,
}

pub fn default_volume() -> f64 {
  0.3
}

fn default_repeat_rate() -> f64 {
  1.0
}

fn default_poll_interval() -> f64 {
  10.0
}

fn default_cycle_secs() -> f64 {
  15.0
}

pub fn default_crossfade_ms() -> f64 {
  6.0
}

/// A labeled tier for classifying metric values with a display
/// label and color.  Tiers are ordered by threshold; the first
/// tier whose threshold exceeds the metric wins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TierConfig {
  pub threshold: f64,
  pub label: String,
  pub color: String,
}

/// A single note within a heartbeat, with its own transition,
/// volume, and time offset from heartbeat start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoteConfig {
  pub transition: Transition,

  #[serde(default = "default_volume")]
  pub volume: f64,

  #[serde(default)]
  pub offset: f64,
}

/// A heartbeat joins a probe command with one or more notes, each
/// mapping the probe's metric to patches from the library.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeartbeatConfig {
  pub name: String,
  pub command: String,
  pub result_mode: ResultMode,
  pub notes: Vec<NoteConfig>,

  #[serde(default)]
  pub playback: Playback,

  /// Legacy field kept for back-compat deserialization only.
  /// Upgraded to `Playback::Continuous` in `resolve_legacy_continuous`.
  #[serde(default, skip_serializing)]
  continuous: bool,

  /// Seconds of silence between phrase repetitions (continuous
  /// mode).
  #[serde(default)]
  pub phrase_gap: f64,

  /// Speed multiplier on phrase repetition.  Divides the gap.
  #[serde(default = "default_repeat_rate")]
  pub repeat_rate: f64,

  /// Seconds between probe command executions.
  #[serde(default = "default_poll_interval")]
  pub poll_interval_secs: f64,

  /// Seconds between plays for one-shot heartbeats.
  #[serde(default = "default_cycle_secs")]
  pub cycle_secs: f64,

  /// Offset within the cycle for wall-clock alignment, allowing
  /// heartbeats to stagger their play times.
  #[serde(default)]
  pub cycle_offset_secs: f64,

  /// Milliseconds over which `replace()` crossfades the old graph
  /// into the new one.  Higher values let release envelopes and
  /// echo tails ring out naturally during continuous playback.
  #[serde(default = "default_crossfade_ms")]
  pub crossfade_ms: f64,

  /// Optional labeled tiers for classifying metric values.  When
  /// empty, the UI displays the raw metric value instead.
  #[serde(default)]
  pub tiers: Vec<TierConfig>,
}

impl HeartbeatConfig {
  /// Build a HeartbeatConfig with the given fields and sensible
  /// defaults for legacy/internal fields.
  pub fn new(
    name: String,
    command: String,
    result_mode: ResultMode,
    notes: Vec<NoteConfig>,
    playback: Playback,
    phrase_gap: f64,
    repeat_rate: f64,
    poll_interval_secs: f64,
    cycle_secs: f64,
    cycle_offset_secs: f64,
    crossfade_ms: f64,
    tiers: Vec<TierConfig>,
  ) -> Self {
    Self {
      name,
      command,
      result_mode,
      notes,
      playback,
      continuous: false,
      phrase_gap,
      repeat_rate,
      poll_interval_secs,
      cycle_secs,
      cycle_offset_secs,
      crossfade_ms,
      tiers,
    }
  }

  /// Upgrade the legacy `continuous = true` field to
  /// `Playback::Continuous` when no explicit `playback` was set.
  pub fn resolve_legacy_continuous(&mut self) {
    if self.continuous && self.playback == Playback::Clock {
      self.playback = Playback::Continuous;
    }
  }

  /// Build a minimal heartbeat config for testing.
  pub fn test_default() -> Self {
    Self {
      name: "test".to_string(),
      command: "echo 0".to_string(),
      result_mode: crate::probe::ResultMode::Stdout,
      notes: vec![NoteConfig {
        transition: Transition::Discrete {
          states: vec![crate::transition::DiscreteState {
            threshold: 1.01,
            patch: "sine".to_string(),
          }],
        },
        volume: default_volume(),
        offset: 0.0,
      }],
      playback: Playback::default(),
      continuous: false,
      phrase_gap: 0.0,
      repeat_rate: default_repeat_rate(),
      poll_interval_secs: default_poll_interval(),
      cycle_secs: default_cycle_secs(),
      cycle_offset_secs: 0.0,
      crossfade_ms: default_crossfade_ms(),
      tiers: vec![],
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn playback_serde_round_trip() {
    // TOML requires a table at the root, so wrap in a struct.
    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
    struct Wrapper {
      playback: Playback,
    }
    for variant in [Playback::Clock, Playback::Loop, Playback::Continuous] {
      let w = Wrapper { playback: variant };
      let serialized = toml::to_string(&w).unwrap();
      let deserialized: Wrapper = toml::from_str(&serialized).unwrap();
      assert_eq!(w, deserialized);
    }
  }

  #[test]
  fn playback_default_is_clock() {
    assert_eq!(Playback::default(), Playback::Clock);
  }
}
