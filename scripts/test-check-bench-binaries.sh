#!/usr/bin/env bash
# Shows that check-bench-binaries.sh asks for a timing comparison when a bench source changed,
# calls it noise when none did, fails when a source changed but the executables didn't, and fails
# closed on missing input.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
guard="$here/check-bench-binaries.sh"
failures=0
fail() { echo "FAIL: $*"; failures=$((failures + 1)); }

run_guard() {
  set +e
  out="$(cd "$repo" && "$guard" "$@" 2>&1)"
  status=$?
  set -e
}

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
repo="$work/repo"
mkdir -p "$repo/crates/repo/src" "$repo/crates/app/src"
git -C "$repo" init --quiet
commit() { git -C "$repo" add -A && git -C "$repo" -c user.name=t -c user.email=t@t commit --quiet -m "$1"; }
echo 'pub fn a() {}' >"$repo/crates/repo/src/lib.rs"
echo 'pub fn gone() {}' >"$repo/crates/repo/src/gone.rs"
echo 'fn main() {}' >"$repo/crates/app/src/main.rs"
commit base
base="$(git -C "$repo" rev-parse HEAD)"

printf '%s\n' crates/repo/src/lib.rs crates/repo/src/gone.rs >"$work/base-sources"
printf '%s\n' crates/repo/src/lib.rs >"$work/head-sources"
echo 'reads 1111' >"$work/hashes-a"
echo 'reads 2222' >"$work/hashes-b"

# 1. Only a file the benches aren't built from changed: noise, whatever the executables are.
echo 'fn main() { println!(); }' >"$repo/crates/app/src/main.rs"
commit app
run_guard "$base" "$work/base-sources" "$work/head-sources" "$work/hashes-a" "$work/hashes-b"
[ "$status" -eq 3 ] || fail "expected 3 when no bench source changed, got $status: $out"
grep -q "^unchanged: none of the 2 files" <<<"$out" || fail "didn't say nothing changed: $out"

# 2. A bench source changed and the executables differ: compare the timings.
echo 'pub fn a() { let _ = 1; }' >"$repo/crates/repo/src/lib.rs"
commit repo
run_guard "$base" "$work/base-sources" "$work/head-sources" "$work/hashes-a" "$work/hashes-b"
[ "$status" -eq 0 ] || fail "expected 0 when a bench source changed, got $status: $out"
grep -q "^changed: crates/repo/src/lib.rs$" <<<"$out" || fail "didn't name the changed source: $out"

# 3. The same change with identical executables is a reused build, and fails.
run_guard "$base" "$work/base-sources" "$work/head-sources" "$work/hashes-a" "$work/hashes-a"
[ "$status" -eq 1 ] || fail "expected 1 on identical executables, got $status: $out"
grep -q "reused the base's" <<<"$out" || fail "didn't say the build was reused: $out"

# 4. A source only the base was built from counts too: deleting it changes the benches.
git -C "$repo" checkout --quiet "$base" -- crates/repo/src/lib.rs
rm "$repo/crates/repo/src/gone.rs"
commit delete
run_guard "$base" "$work/base-sources" "$work/head-sources" "$work/hashes-a" "$work/hashes-a"
[ "$status" -eq 1 ] || fail "expected a deleted base source to count as a change, got $status: $out"
grep -q "^changed: crates/repo/src/gone.rs$" <<<"$out" || fail "didn't name the deleted source: $out"

# 5. Missing input fails closed rather than calling anything noise.
: >"$work/empty"
run_guard "$base" "$work/empty" "$work/head-sources" "$work/hashes-a" "$work/hashes-b"
[ "$status" -eq 2 ] || fail "expected 2 on an empty source list, got $status: $out"
run_guard "$base" "$work/base-sources" "$work/head-sources" "$work/hashes-a" "$work/missing"
[ "$status" -eq 2 ] || fail "expected 2 on a missing hash file, got $status: $out"
run_guard 0000000000000000000000000000000000000000 "$work/base-sources" "$work/head-sources" "$work/hashes-a" "$work/hashes-b"
[ "$status" -eq 2 ] || fail "expected 2 on an unknown base commit, got $status: $out"

[ "$failures" -eq 0 ] || { echo "$failures failure(s)" >&2; exit 1; }
echo "ok: check-bench-binaries.sh skips unchanged benches, catches reused builds and fails closed"
