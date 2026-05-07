# Build both Rust and Elm.
build: build-elm build-rust

# Build the Elm frontend.
build-elm:
    cd frontend && elm make src/Main.elm --output public/elm.js

# Build all Rust workspace crates.
build-rust:
    cargo build --workspace

# Run all tests (Elm compile check + Rust test suite + clippy).
test: build-elm test-rust lint-rust

# Run the Rust test suite.
test-rust:
    cargo test --workspace

# Lint the workspace.  The `[workspace.lints.clippy]` block in
# Cargo.toml denies the unwrap / expect / panic family in
# production code, and `-D warnings` promotes every other clippy
# warning to a hard error.  Steady state is zero warnings:
# warnings are treated like errors we can temporarily ship when
# things are dicey (via a targeted `#[allow(clippy::...)]` with a
# justifying comment), not background noise to ignore.
# `--all-targets` also lints integration tests (exempted from the
# unwrap policy via per-file `#![allow]`) and benches.
lint-rust:
    cargo clippy --workspace --all-targets -- -D warnings

# Build Elm then run via cargo, forwarding all arguments.
run *args: build-elm
    cargo run {{args}}
