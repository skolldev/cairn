#!/usr/bin/env bash
# Shows that check-bench-regressions.sh fails on a slowdown over the threshold, passes on
# anything under it and on a new bench, and fails closed when there is nothing to compare.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
guard="$here/check-bench-regressions.sh"
failures=0
fail() { echo "FAIL: $*"; failures=$((failures + 1)); }

run_guard() {
  set +e
  out="$("$guard" "$@" 2>&1)"
  status=$?
  set -e
}

# estimate <bench> <run> <median in nanoseconds>
estimate() {
  mkdir -p "$tree/$1/$2"
  echo "{\"median\":{\"point_estimate\":$3}}" >"$tree/$1/$2/estimates.json"
}

tree="$(mktemp -d)"
trap 'rm -rf "$tree"' EXIT

# 1. A slowdown under the threshold, a speedup and a new bench all pass.
estimate spawn/read base 1000000
estimate spawn/read head 1200000
estimate small/refs base 2000000
estimate small/refs head 1000000
estimate small/walk head 5000000
run_guard "$tree" 30
[ "$status" -eq 0 ] || fail "expected a pass, got $status: $out"
grep -q "^new: small/walk " <<<"$out" || fail "didn't say the bench was new: $out"
grep -q "^ok: 2 bench" <<<"$out" || fail "didn't count what it compared: $out"

# 2. A slowdown over the threshold fails and names the bench, among others that pass.
estimate small/refs head 2700000
run_guard "$tree" 30
[ "$status" -eq 1 ] || fail "expected exit 1 on a regression, got $status"
grep -q "^regressed: small/refs " <<<"$out" || fail "didn't name the regression: $out"

# 3. A vanished bench is reported, and doesn't by itself fail.
estimate spawn/read head 1000000
estimate small/refs head 2000000
estimate spawn/gone base 1000000
run_guard "$tree" 30
[ "$status" -eq 0 ] || fail "a vanished bench alone shouldn't fail, got $status"
grep -q "^gone: spawn/gone " <<<"$out" || fail "didn't report the vanished bench: $out"

# 4. Nothing to compare fails closed, whichever way that happens.
run_guard "$tree/missing" 30
[ "$status" -eq 2 ] || fail "expected exit 2 with no results, got $status"
only_new="$(mktemp -d)"
mkdir -p "$only_new/spawn/read/head"
echo '{"median":{"point_estimate":1}}' >"$only_new/spawn/read/head/estimates.json"
run_guard "$only_new" 30
rm -rf "$only_new"
[ "$status" -eq 2 ] || fail "expected exit 2 with no baseline at all, got $status"

[ "$failures" -eq 0 ] || { echo "$failures failure(s)" >&2; exit 1; }
echo "ok: check-bench-regressions.sh catches regressions and fails closed"
