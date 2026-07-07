//! sonify-health-server library.
//!
//! Owns the daemon binary's application-specific pieces: the audio
//! engine (the thread-supervised mixer + poll/play threads that
//! actually produce sound), the WebSocket route, the runtime
//! preview state, the remote-source connector, the sonify-specific
//! Prometheus metrics, and the application state shape consumed by
//! foundation's `Server` runner.  The binary entry point in
//! `main.rs` wires everything together through `#[foundation_main]`.

pub mod audio_engine;
pub mod config;
pub mod frontend;
pub mod metrics;
pub mod preview_state;
pub mod remote_source;
pub mod web_base;
pub mod websocket;
