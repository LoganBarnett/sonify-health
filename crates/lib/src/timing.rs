use serde::Deserialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Configuration for multi-machine time-slot coordination.
///
/// Multiple machines share a repeating cycle.  Each machine gets
/// a fixed slot within the cycle, determined by wall-clock time
/// modulo the cycle duration.  NTP synchronization provides the
/// ~10 ms precision needed for second-level slots.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimingConfig {
  /// Total cycle duration covering all machines.
  #[serde(default = "default_cycle_secs")]
  pub cycle_duration_secs: f64,

  /// Time budget per machine within the cycle.
  #[serde(default = "default_slot_secs")]
  pub slot_duration_secs: f64,

  /// This machine's zero-indexed position in the cycle.
  #[serde(default)]
  pub slot: u8,
}

fn default_cycle_secs() -> f64 {
  16.0
}

fn default_slot_secs() -> f64 {
  4.0
}

impl Default for TimingConfig {
  fn default() -> Self {
    Self {
      cycle_duration_secs: default_cycle_secs(),
      slot_duration_secs: default_slot_secs(),
      slot: 0,
    }
  }
}

impl TimingConfig {
  /// Seconds elapsed within the current cycle.
  fn cycle_offset_secs(&self) -> f64 {
    let now = SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap_or_default()
      .as_secs_f64();
    now % self.cycle_duration_secs
  }

  /// Start offset of this machine's slot within a cycle.
  fn slot_start(&self) -> f64 {
    self.slot as f64 * self.slot_duration_secs
  }

  /// Whether the current wall-clock time falls within this
  /// machine's slot.
  pub fn is_in_slot(&self) -> bool {
    let offset = self.cycle_offset_secs();
    let start = self.slot_start();
    let end = start + self.slot_duration_secs;
    offset >= start && offset < end
  }

  /// Duration until the next occurrence of this machine's slot.
  /// Returns zero if already inside the slot.
  pub fn duration_until_next_slot(&self) -> Duration {
    let offset = self.cycle_offset_secs();
    let start = self.slot_start();

    let wait = if offset < start {
      start - offset
    } else if offset < start + self.slot_duration_secs {
      0.0
    } else {
      self.cycle_duration_secs - offset + start
    };

    Duration::from_secs_f64(wait)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn default_config_values() {
    let cfg = TimingConfig::default();
    assert_eq!(cfg.cycle_duration_secs, 16.0);
    assert_eq!(cfg.slot_duration_secs, 4.0);
    assert_eq!(cfg.slot, 0);
  }

  #[test]
  fn duration_until_slot_is_bounded() {
    let cfg = TimingConfig {
      cycle_duration_secs: 16.0,
      slot_duration_secs: 4.0,
      slot: 3,
    };
    let wait = cfg.duration_until_next_slot();
    // Wait can never exceed a full cycle.
    assert!(wait <= Duration::from_secs_f64(16.0));
  }

  #[test]
  fn in_slot_and_wait_are_consistent() {
    let cfg = TimingConfig::default();
    if cfg.is_in_slot() {
      assert_eq!(cfg.duration_until_next_slot(), Duration::ZERO);
    } else {
      assert!(cfg.duration_until_next_slot() > Duration::ZERO);
    }
  }
}
