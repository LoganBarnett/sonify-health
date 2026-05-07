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
# production code; running this with `--all-targets` also covers
# integration tests (which are exempted via per-file `#![allow]`)
# and benches.  Policy lints (deny in workspace config) become
# hard errors; non-policy clippy warnings stay as warnings rather
# than blanket-failing the build via `-D warnings` — those tend
# to be domain-specific false positives (e.g. clippy::precedence
# on fundsp's `>>` graph-composition operator).
lint-rust:
    cargo clippy --workspace --all-targets

# Build Elm then run via cargo, forwarding all arguments.
run *args: build-elm
    cargo run {{args}}
