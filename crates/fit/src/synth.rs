//! Offline synthesis through the graph the daemon plays.

use sonify_health_lib::{heartbeat, Patch, ResolvedNote};

fn single_note(patch: Patch) -> [ResolvedNote; 1] {
  [ResolvedNote {
    patch,
    volume: 1.0,
    offset: 0.0,
  }]
}

/// An endless stream of stereo frames that plays `patch` once: the note, its
/// tail, then silence.  The frames are pulled from the graph the daemon builds
/// for a clock or loop heartbeat holding this one note, so a measurement taken
/// here describes what the daemon plays.
pub fn note_frames(
  patch: Patch,
  sample_rate: f64,
) -> impl Iterator<Item = (f32, f32)> {
  let mut graph = heartbeat::heartbeat_graph_with_notes(
    &single_note(patch),
    None,
    sample_rate,
  );
  graph.set_sample_rate(sample_rate);
  graph.allocate();
  std::iter::repeat_with(move || graph.get_stereo())
}

/// `frames` mono samples of `patch`, taken after `settle` frames have been
/// discarded so the attack and the filters' start-up transients are over.
pub fn settled_mono(
  patch: &Patch,
  sample_rate: f64,
  settle: usize,
  frames: usize,
) -> Vec<f64> {
  note_frames(patch.clone(), sample_rate)
    .skip(settle)
    .take(frames)
    .map(|(left, right)| f64::from(0.5 * (left + right)))
    .collect()
}

/// Seconds one note of `patch` lasts, through its release and echo tail.
pub fn note_seconds(patch: &Patch) -> f64 {
  heartbeat::heartbeat_notes_duration(&single_note(patch.clone())).as_secs_f64()
}

#[cfg(test)]
mod tests {
  use super::*;

  const SAMPLE_RATE: f64 = 44_100.0;

  fn frames(patch: Patch, count: usize) -> Vec<(f32, f32)> {
    note_frames(patch, SAMPLE_RATE).take(count).collect()
  }

  #[test]
  fn default_patch_is_audible_on_both_channels() {
    let rendered = frames(Patch::default(), 8_192);
    let peak = |channel: fn(&(f32, f32)) -> f32| {
      rendered
        .iter()
        .map(channel)
        .fold(0.0f32, |peak, sample| peak.max(sample.abs()))
    };
    assert!(peak(|frame| frame.0) > 0.01, "left channel is silent");
    assert!(peak(|frame| frame.1) > 0.01, "right channel is silent");
  }

  // A search over patch parameters compares renders by their loss, which only
  // means something when the same patch always renders the same samples --
  // including through the noise generator.
  #[test]
  fn the_same_patch_renders_the_same_frames() {
    let patch = Patch {
      noise_mix: 0.5,
      hiss: 0.3,
      ..Default::default()
    };
    assert_eq!(frames(patch.clone(), 8_192), frames(patch, 8_192));
  }

  #[test]
  fn note_seconds_covers_the_envelope() {
    let patch = Patch::default();
    let envelope = (patch.attack_ms + patch.decay_ms + patch.release_ms)
      / 1000.0
      + patch.duration;
    assert!(note_seconds(&patch) >= envelope);
  }
}
