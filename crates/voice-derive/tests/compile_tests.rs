// Tests-only exemption from the workspace's no-unwrap policy.
// See workspace `[lints.clippy]` in the root Cargo.toml.
#![allow(
  clippy::unwrap_used,
  clippy::expect_used,
  clippy::panic,
  clippy::unreachable,
  clippy::todo,
  clippy::unimplemented
)]

#[test]
fn compile_fail_tests() {
  let t = trybuild::TestCases::new();
  t.compile_fail("tests/compile_fail/*.rs");
}
