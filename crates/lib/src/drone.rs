/// Compute the gap between drone phrases from a base gap, metric
/// value, power-curve exponent, and rate multiplier.
///
/// - metric=0.0 → gap = base_gap / rate (full gap)
/// - metric=1.0 → gap = 0 (back-to-back)
/// - curve < 1  → sensitive (gap drops quickly at low metrics)
/// - curve > 1  → insensitive (gap stays long until metric nears 1.0)
/// - rate > 1   → faster overall
pub fn phrase_gap_secs(
  base_gap: f64,
  metric: f32,
  curve: f32,
  rate: f32,
) -> f64 {
  let m = (metric as f64).clamp(0.0, 1.0);
  let c = (curve as f64).max(0.01);
  let r = (rate as f64).max(0.1);
  base_gap * (1.0 - m.powf(c)) / r
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn gap_at_zero_metric_equals_base_over_rate() {
    let gap = phrase_gap_secs(4.0, 0.0, 1.0, 1.0);
    assert!(
      (gap - 4.0).abs() < 1e-10,
      "Expected base gap at metric 0.0, got {gap}"
    );
  }

  #[test]
  fn gap_at_full_metric_is_zero() {
    let gap = phrase_gap_secs(4.0, 1.0, 1.0, 1.0);
    assert!(gap.abs() < 1e-10, "Expected zero gap at metric 1.0, got {gap}");
  }

  #[test]
  fn gap_monotonically_decreases() {
    let g1 = phrase_gap_secs(4.0, 0.2, 1.0, 1.0);
    let g2 = phrase_gap_secs(4.0, 0.5, 1.0, 1.0);
    let g3 = phrase_gap_secs(4.0, 0.8, 1.0, 1.0);
    assert!(g1 > g2 && g2 > g3);
  }

  #[test]
  fn gap_clamps_out_of_range_metric() {
    let at_neg = phrase_gap_secs(4.0, -0.5, 1.0, 1.0);
    let at_over = phrase_gap_secs(4.0, 1.5, 1.0, 1.0);
    assert!((at_neg - 4.0).abs() < 1e-10);
    assert!(at_over.abs() < 1e-10);
  }

  #[test]
  fn curve_below_one_is_sensitive() {
    let linear = phrase_gap_secs(4.0, 0.3, 1.0, 1.0);
    let sensitive = phrase_gap_secs(4.0, 0.3, 0.5, 1.0);
    assert!(
      sensitive < linear,
      "curve<1 should drop the gap faster: sensitive={sensitive}, linear={linear}"
    );
  }

  #[test]
  fn curve_above_one_is_insensitive() {
    let linear = phrase_gap_secs(4.0, 0.3, 1.0, 1.0);
    let insensitive = phrase_gap_secs(4.0, 0.3, 3.0, 1.0);
    assert!(
      insensitive > linear,
      "curve>1 should keep the gap longer: insensitive={insensitive}, linear={linear}"
    );
  }

  #[test]
  fn rate_scales_gap() {
    let base = phrase_gap_secs(4.0, 0.5, 1.0, 1.0);
    let fast = phrase_gap_secs(4.0, 0.5, 1.0, 2.0);
    assert!((fast - base / 2.0).abs() < 1e-10, "rate=2 should halve the gap");
  }

  #[test]
  fn endpoints_hold_for_any_curve() {
    for &c in &[0.1, 0.5, 1.0, 2.0, 5.0] {
      let at_zero = phrase_gap_secs(8.0, 0.0, c, 1.0);
      let at_one = phrase_gap_secs(8.0, 1.0, c, 1.0);
      assert!(
        (at_zero - 8.0).abs() < 1e-10,
        "curve={c}: expected 8.0 at metric 0, got {at_zero}"
      );
      assert!(
        at_one.abs() < 1e-10,
        "curve={c}: expected 0 at metric 1, got {at_one}"
      );
    }
  }
}
