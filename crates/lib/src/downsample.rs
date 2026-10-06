//! Sample-and-hold whose average rate is exact at any sample rate.

use fundsp::prelude32::{An, AudioNode, Frame, DEFAULT_SR, U1};

/// Sample-and-hold whose average rate is exact at any sample rate.
///
/// fundsp's `hold_hz` realises a different rate on every device: a 1120 Hz hold
/// runs at 1102.5 Hz on a 44.1 kHz device and at 1116.3 Hz on a 48 kHz one, and
/// a 31 kHz hold collapses to a 2:1 decimation on both.  It schedules each hold
/// from the sample at which the previous one fired, so every hold lasts a whole
/// number of samples and the realised rate is `sample_rate / ceil(sample_rate /
/// rate)`.  Carrying the fractional remainder forward makes the average rate
/// exactly `rate_hz` everywhere.
#[derive(Clone)]
pub(crate) struct ExactHold {
  rate_hz: f64,
  /// Phase advance per sample: `rate_hz / sample_rate`.
  step: f64,
  /// Accumulates towards one, which triggers a hold.
  phase: f64,
  held: f32,
}

impl ExactHold {
  fn new(rate_hz: f32) -> Self {
    Self {
      rate_hz: f64::from(rate_hz),
      step: f64::from(rate_hz) / DEFAULT_SR,
      phase: 1.0,
      held: 0.0,
    }
  }
}

impl AudioNode for ExactHold {
  const ID: u64 = 0x736f_6e69_6679_0001;
  type Inputs = U1;
  type Outputs = U1;

  fn reset(&mut self) {
    // Starting at one fires a hold on the first sample, as fundsp's does.
    self.phase = 1.0;
    self.held = 0.0;
  }

  fn set_sample_rate(&mut self, sample_rate: f64) {
    self.step = self.rate_hz / sample_rate;
  }

  fn tick(
    &mut self,
    input: &Frame<f32, Self::Inputs>,
  ) -> Frame<f32, Self::Outputs> {
    self.phase += self.step;
    if self.phase >= 1.0 {
      self.phase -= self.phase.floor();
      self.held = input[0];
    }
    [self.held].into()
  }
}

/// Holds its input at `rate_hz`.  At or above the sample rate every sample is
/// held afresh, so the signal passes through unchanged.
pub(crate) fn exact_hold_hz(rate_hz: f32) -> An<ExactHold> {
  An(ExactHold::new(rate_hz))
}

#[cfg(test)]
mod tests {
  use super::*;

  /// Holds per second that `exact_hold_hz(rate_hz)` realises at `sample_rate`,
  /// counted over ten seconds of a ramp that never repeats a value.
  fn realised_rate(rate_hz: f32, sample_rate: f64) -> f64 {
    let mut hold = exact_hold_hz(rate_hz);
    hold.set_sample_rate(sample_rate);
    hold.reset();
    let frames = (10.0 * sample_rate) as usize;
    let changes = (0..frames)
      .map(|i| hold.filter_mono(i as f32))
      .fold((f32::NAN, 0usize), |(last, count), value| {
        (value, count + usize::from(value != last))
      })
      .1;
    changes as f64 / 10.0
  }

  #[test]
  fn rate_is_exact_on_every_device() {
    for sample_rate in [22_050.0, 44_100.0, 48_000.0, 96_000.0] {
      let realised = realised_rate(1120.0, sample_rate);
      assert!(
        (realised - 1120.0).abs() < 0.5,
        "at {sample_rate} Hz a 1120 Hz hold realised {realised} Hz"
      );
    }
  }

  // fundsp's hold rounds 31 200 Hz down to a 2:1 decimation (22 050 Hz) on a
  // 44.1 kHz device; the exact hold keeps the requested rate.
  #[test]
  fn rate_near_the_sample_rate_is_not_rounded_to_a_decimation() {
    let realised = realised_rate(31_200.0, 44_100.0);
    assert!(
      (realised - 31_200.0).abs() < 1.0,
      "a 31 200 Hz hold realised {realised} Hz"
    );
  }

  #[test]
  fn rate_at_or_above_the_sample_rate_passes_the_signal_through() {
    let mut hold = exact_hold_hz(100_000.0);
    hold.set_sample_rate(44_100.0);
    hold.reset();
    let passed: Vec<f32> =
      (0..1000).map(|i| hold.filter_mono(i as f32)).collect();
    let expected: Vec<f32> = (0..1000).map(|i| i as f32).collect();
    assert_eq!(passed, expected);
  }
}
