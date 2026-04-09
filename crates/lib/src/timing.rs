use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds until the next play time on a wall-clock-anchored grid.
/// The grid is epoch-aligned: play times fall at
/// `offset + N * cycle` for integer N.
pub fn seconds_until_next(cycle_secs: f64, offset_secs: f64) -> f64 {
  let now = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_secs_f64();
  let aligned = now - offset_secs;
  let elapsed = aligned.rem_euclid(cycle_secs);
  let remaining = cycle_secs - elapsed;

  // Snap to zero when we're essentially at the boundary.
  if remaining < 0.005 || (cycle_secs - remaining) < 0.005 {
    0.0
  } else {
    remaining
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  /// Helper: compute seconds_until_next as if "now" were a given
  /// epoch timestamp.  Mirrors the production logic but accepts a
  /// synthetic clock value.
  fn seconds_until_next_at(now: f64, cycle_secs: f64, offset_secs: f64) -> f64 {
    let aligned = now - offset_secs;
    let elapsed = aligned.rem_euclid(cycle_secs);
    let remaining = cycle_secs - elapsed;
    if remaining < 0.005 || (cycle_secs - remaining) < 0.005 {
      0.0
    } else {
      remaining
    }
  }

  #[test]
  fn mid_cycle_offset_zero() {
    // 7.5 seconds into a 15-second cycle → 7.5 remaining.
    let r = seconds_until_next_at(7.5, 15.0, 0.0);
    assert!((r - 7.5).abs() < 1e-9);
  }

  #[test]
  fn at_boundary_snaps_to_zero() {
    // Exactly on a boundary.
    let r = seconds_until_next_at(30.0, 15.0, 0.0);
    assert_eq!(r, 0.0);
  }

  #[test]
  fn near_boundary_snaps_to_zero() {
    // Within 5 ms of boundary.
    let r = seconds_until_next_at(29.998, 15.0, 0.0);
    assert_eq!(r, 0.0);
  }

  #[test]
  fn offset_shifts_grid() {
    // Cycle 15, offset 5.  Grid points: 5, 20, 35, …
    // now=12 → next at 20 → 8 remaining.
    let r = seconds_until_next_at(12.0, 15.0, 5.0);
    assert!((r - 8.0).abs() < 1e-9);
  }

  #[test]
  fn offset_larger_than_cycle_wraps() {
    // offset 20 with cycle 15 effectively means offset 5.
    let r = seconds_until_next_at(12.0, 15.0, 20.0);
    assert!((r - 8.0).abs() < 1e-9);
  }

  #[test]
  fn live_result_is_bounded() {
    let r = seconds_until_next(15.0, 0.0);
    assert!(r >= 0.0);
    assert!(r <= 15.0);
  }
}
