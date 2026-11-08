#!/usr/bin/env bash
# Shows that check-test-gates.sh catches a check-then-create gate, leaves a waiting loop alone,
# and fails closed on a tree it can't check.
set -euo pipefail
guard="$(cd "$(dirname "$0")" && pwd)/check-test-gates.sh"
failures=0
fail() { echo "FAIL: $*"; failures=$((failures + 1)); }

run_guard() {
  set +e
  out="$("$guard" "$@" 2>&1)"
  status=$?
  set -e
}

tree="$(mktemp -d)"
trap 'rm -rf "${tree:?}"' EXIT
mkdir -p "$tree/crates/repo/tests"
echo "let wait = \"while [ ! -f 'release' ]; do sleep 0.05; done\";" >"$tree/crates/repo/tests/wait.rs"

run_guard "$tree"
[ "$status" -eq 0 ] || fail "a waiting loop isn't a gate, got $status: $out"

echo "let gate = \"if [ ! -f 'started' ]; then touch 'started'; fi\";" >"$tree/crates/repo/tests/gate.rs"
run_guard "$tree"
[ "$status" -eq 1 ] || fail "expected exit 1 on a gate, got $status"
grep -q "^violation: crates/repo/tests/gate.rs:1:" <<<"$out" || fail "didn't name the gate: $out"

run_guard "$tree/missing"
[ "$status" -eq 2 ] || fail "expected exit 2 with no crates/, got $status"

[ "$failures" -eq 0 ] || { echo "$failures failure(s)" >&2; exit 1; }
echo "ok: check-test-gates.sh catches a gate and fails closed"
