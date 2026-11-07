#!/usr/bin/env bash
# Shows that check-single-git-spawn.sh catches each way of starting a process outside the
# allowed paths, ignores the allowed ones, and fails closed on a tree it can't check.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
guard="$here/check-single-git-spawn.sh"
failures=0
fail() { echo "FAIL: $*"; failures=$((failures + 1)); }

# Runs the guard, leaving its output in $out and its exit code in $status.
run_guard() {
  set +e
  out="$("$guard" "$@" 2>&1)"
  status=$?
  set -e
}

tree="$(mktemp -d)"
trap 'rm -rf "$tree"' EXIT
mkdir -p "$tree/crates/write/src" "$tree/crates/repo/src" "$tree/crates/repo/tests" "$tree/crates/app/src"
echo 'fn spawn() { std::process::Command::new("git"); }' >"$tree/crates/write/src/lib.rs"
echo 'fn clean() {}' >"$tree/crates/app/src/main.rs"
echo 'fn f() { std::process::Command::new("git"); }' >"$tree/crates/repo/src/full.rs"
printf 'use std::process::Command;\nfn f() { Command::new("git"); }\n' >"$tree/crates/repo/tests/imported.rs"

# 1. Violations in product code and tests are reported; the allowed crate and clean files aren't.
run_guard "$tree"
[ "$status" -eq 1 ] || fail "expected exit 1 with violations, got $status"
grep -q "^violation: crates/repo/src/full.rs:1:" <<<"$out" || fail "missed a full-path Command::new"
grep -q "^violation: crates/repo/tests/imported.rs:1:" <<<"$out" || fail "missed an imported Command"
grep -q "^violation: crates/repo/tests/imported.rs:2:" <<<"$out" || fail "missed a bare Command::new"
if grep -qE "^violation: crates/(write|app)/" <<<"$out"; then fail "reported an allowed or clean file: $out"; fi
case1="$out"

# 2. With the offenders allowed, the guard passes.
run_guard "$tree" crates/write/src/ crates/repo/
[ "$status" -eq 0 ] || fail "guard failed with every offender allowed (exit $status): $out"

# 3. A root with no crates/ is an error, never a pass.
run_guard "$tree/crates/app"
[ "$status" -eq 2 ] || fail "expected exit 2 for a root without crates/, got $status"

if [ "$failures" -ne 0 ]; then
  echo "--- guard output for case 1:"
  echo "$case1"
  exit 1
fi
echo "ok: check-single-git-spawn.sh catches every spawn outside cairn-write and fails closed"
