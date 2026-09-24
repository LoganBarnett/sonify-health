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
# production code, and `--deny warnings` promotes every other
# clippy warning to a hard error.  Steady state is zero warnings:
# warnings are treated like errors we can temporarily ship when
# things are dicey (via a targeted `#[allow(clippy::...)]` with a
# justifying comment), not background noise to ignore.
# `--all-targets` also lints integration tests (exempted from the
# unwrap policy via per-file `#![allow]`) and benches.
lint-rust:
    cargo clippy --workspace --all-targets -- --deny warnings

# Build Elm then run via cargo, forwarding all arguments.
run *args: build-elm
    cargo run {{args}}

# Combine all passing open Dependabot PRs into one PR and merge it.
#
# One-shot catch-up for a Dependabot backlog: takes the versions Dependabot
# already resolved, bundles the passing bumps onto one branch, opens a single
# PR, and merges it once green (failing bumps are left for a human).  Per-PR
# auto-merge handles the steady-state trickle.  Pass --dry-run or --no-merge to
# hold back.
dependabot-combine *args:
    dependabot-combine {{args}}

# Bump every dependency the workspace's constraints allow, with changelog.
#
# The working-tree half of the scheduled dependency-bump flow: runs `cargo
# update` across the workspace, classifies each bump against `cargo audit`,
# and composes the CHANGELOG entries — then stops.  Nothing is committed or
# pushed; review the diff and commit yourself.  The scheduled workflow
# (.github/workflows/dependency-bump.yml) runs the same engine and owns the
# branch/PR/merge half.  Pass --dry-run true to preview.
dependency-bump *args:
    dependency-bump {{args}}

# Review the current change set against the project's conventions.
#
# Sends the change set to a nested reviewer that judges it against the
# project's convention documents, and reports what stands.  Verdicts are
# recorded per file in review.json so addressing one file does not re-review
# the rest; pass --priors clear to judge those files afresh.  The reviewer is
# your own `claude` install, which must be on PATH.
#
# Exit code 1 means the review ran and found something; 2 means it could not
# judge the tree in full.  `just` reports either as a failed recipe, so read
# the code: a 1 with findings printed above it is the tool working, not
# breaking.
review *args:
    review {{args}}
