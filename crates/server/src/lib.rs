//! sonify-health-server library.
//!
//! Owns the daemon implementation: the audio supervision loop, the
//! WebSocket route, the runtime preview state, the remote-source
//! connector, the sonify-specific Prometheus metrics, and the
//! application state shape consumed by foundation's `Server`
//! runner.  The binary entry point in `main.rs` wires everything
//! together through `#[foundation_main]`.

pub mod config;
pub mod daemon;
pub mod metrics;
pub mod preview_state;
pub mod remote_source;
pub mod web_base;
pub mod websocket;
