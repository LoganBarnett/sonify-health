// Helpers in this file sit outside `#[test]` functions, so
// clippy.toml's `allow-{unwrap,expect,panic}-in-tests` does not
// reach them.  Opt the whole file in explicitly.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[test]
fn compile_fail_tests() {
  let t = trybuild::TestCases::new();
  t.compile_fail("tests/compile_fail/*.rs");
}
