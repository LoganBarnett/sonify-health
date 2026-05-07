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

pub mod auth;
pub mod config;
pub mod daemon;
pub mod metrics;
pub mod preview_state;
pub mod remote_source;
pub mod web_base;
pub mod websocket;
