#!/usr/bin/env bash
# Shows that check-gix-pathspec-top.sh fails on pathspecs without `top`, passes with it, and
# fails closed when there is nothing to check.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
guard="$here/check-gix-pathspec-top.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
failures=0

fail() { echo "FAIL: $*"; failures=$((failures + 1)); }

run_guard() {
  set +e
  out="$("$guard" "$@" 2>&1)"
  status=$?
  set -e
}

mkdir -p "$tmp/bad/crates/read/src" "$tmp/good/crates/read/src" "$tmp/empty/crates/read"
cat >"$tmp/bad/crates/read/src/lib.rs" <<'RS'
let a = BString::from(":(literal)");
let b = format!(":(icase,literal){path}");
let c = ":(literal,top-ish)";
RS
cat >"$tmp/good/crates/read/src/lib.rs" <<'RS'
let a = BString::from(":(top,literal)");
let b = format!(":(literal,top){path}");
let c = ":(top)";
let d = "not a pathspec: (x)";
RS

# 1. Each pathspec without `top` is reported, and the guard exits 1.
run_guard "$tmp/bad"
[ "$status" -eq 1 ] || fail "expected exit 1 for pathspecs without top, got $status: $out"
for line in 1 2 3; do
  grep -q "^violation: crates/read/src/lib.rs:$line:" <<<"$out" || fail "line $line not reported: $out"
done

# 2. `top` anywhere in the magic passes, and other strings are ignored.
run_guard "$tmp/good"
[ "$status" -eq 0 ] || fail "guard failed on anchored pathspecs (exit $status): $out"

# 3. No Rust files at all is an error, never a pass.
run_guard "$tmp/empty"
[ "$status" -eq 2 ] || fail "expected exit 2 with nothing to check, got $status: $out"

[ "$failures" -eq 0 ] || exit 1
echo "ok: check-gix-pathspec-top.sh catches pathspecs without top and fails closed"
