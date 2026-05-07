// Tests are the only exemption from the workspace's no-unwrap /
// no-expect / no-panic policy.  `cfg_attr(test, ...)` triggers
// only when this crate is compiled with `cfg(test)` set —
// `cargo test`, `cargo clippy --tests` — leaving normal
// `cargo build` / `cargo clippy` linting production code under
// the workspace's `deny`.  See workspace `[lints.clippy]` in the
// root `Cargo.toml`.
#![cfg_attr(
  test,
  allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
  )
)]

pub mod audio;
pub mod continuous;
pub mod heartbeat;
pub mod heartbeat_config;
pub mod library;
pub mod logging;
pub mod patch;
pub mod probe;
pub mod timing;
pub mod transition;

pub use continuous::{
  continuous_graph, continuous_graph_with_notes, ContinuousControls,
  StructuralParams,
};
pub use heartbeat::ResolvedNote;
pub use heartbeat_config::{HeartbeatConfig, NoteConfig, Playback, TierConfig};
pub use library::{builtin_library, PatchLibrary};
pub use logging::{LogFormat, LogLevel};
pub use patch::{Patch, PatchOverrides, PatchParamMeta};
pub use probe::ResultMode;
pub use timing::seconds_until_next;
pub use transition::Transition;
