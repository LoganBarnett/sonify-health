//! sonify-health-server — entry point.
//!
//! Phase-1 scaffold.  This binary is a placeholder until the
//! daemon / web-server / OIDC / WebSocket code migrates over from
//! the cli crate in the next refactor phase.  Running it today
//! exits immediately with a hint to use `sonify-health daemon`,
//! which still lives in the cli crate.
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
  eprintln!(
    "sonify-health-server: scaffold only — daemon code is still \
     in the sonify-health cli for now.  Use `sonify-health \
     daemon` until phase 3 of the workspace split lands."
  );
  ExitCode::from(2)
}
