// See workspace `[lints.clippy]` and the matching attribute on
// `crates/lib/src/lib.rs` — same test-exemption shape.
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

//! Server crate for sonify-health.  Phase-1 scaffold: the daemon /
//! web / OIDC / WebSocket modules currently live in the cli crate
//! and will move here in the next refactor phase.  See `tasks.org`
//! for the migration plan.
