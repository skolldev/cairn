#!/usr/bin/env bash
# Shows that check-gpui-free.sh fails on deliberate violations, fails when it can't read the
# dependency graph, and passes when there is no violation.
# Runs offline against scripts/gpui-guard-fixture, whose "GPUI" crates are local stubs.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
fixture="$here/gpui-guard-fixture"
guard="$here/check-gpui-free.sh"
offenders=(direct mid transitive devonly builddep underscore)
failures=0

fail() { echo "FAIL: $*"; failures=$((failures + 1)); }

# Runs the guard, leaving its output in $out and its exit code in $status.
run_guard() {
  set +e
  out="$("$guard" "$@" 2>&1)"
  status=$?
  set -e
}

# 1. Violations: direct, transitive, dev-only and build-only dependencies, and crates named
#    gpui-<x> and gpui_<x>, are all caught, and the guard exits 1.
run_guard "$fixture/Cargo.toml" app
[ "$status" -eq 1 ] || fail "expected exit 1 for a workspace with violations, got $status"
for bad in "${offenders[@]}"; do
  grep -q "^violation: $bad " <<<"$out" || fail "guard did not report '$bad'"
done
for good in app clean; do
  if grep -q "^violation: $good " <<<"$out"; then fail "guard wrongly reported '$good'"; fi
done
case1="$out"

# 2. No violations once every offender is allowed: the guard passes.
run_guard "$fixture/Cargo.toml" app "${offenders[@]}"
[ "$status" -eq 0 ] || fail "guard failed with every offender allowed (exit $status): $out"

# 3. If cargo fails while reading one member's graph, the guard must not count that member as
#    clean. A cargo wrapper on PATH fails every per-member (`-p`) call.
shim="$(mktemp -d)"
trap 'rm -rf "$shim"' EXIT
real_cargo="$(command -v cargo)"
cat >"$shim/cargo" <<SHIM
#!/usr/bin/env bash
for arg in "\$@"; do [ "\$arg" = "-p" ] && { echo "shim: cargo tree failed" >&2; exit 101; }; done
exec "$real_cargo" "\$@"
SHIM
chmod +x "$shim/cargo"
PATH="$shim:$PATH" run_guard "$fixture/Cargo.toml" app "${offenders[@]}"
[ "$status" -ne 0 ] || fail "guard passed although cargo tree failed for a member"
if grep -q "^ok:" <<<"$out"; then fail "guard printed ok although cargo tree failed for a member"; fi

if [ "$failures" -ne 0 ]; then
  echo "--- guard output for case 1:"
  echo "$case1"
  exit 1
fi
echo "ok: check-gpui-free.sh catches every violation kind and fails closed when cargo fails"
