#!/usr/bin/env bash
# Shows that check-git-floor.sh passes when every place agrees, names the one that drifted, and
# fails closed on a tree it can't read.
set -euo pipefail
guard="$(cd "$(dirname "$0")" && pwd)/check-git-floor.sh"
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

write_tree() {
  mkdir -p "$tree/crates/write/src" "$tree/scripts" "$tree/.github/workflows"
  cat >"$tree/crates/write/src/resolve.rs" <<'EOF'
const OTHER: GitVersion = GitVersion {
    major: 9,
    minor: 9,
    patch: 9,
};
pub const MINIMUM: GitVersion = GitVersion {
    major: 2,
    minor: 37,
    patch: 0,
};
EOF
  # shellcheck disable=SC2016 # the fixture script gets the expansions literally
  echo 'version="${1:-2.37.0}"' >"$tree/scripts/oldest-git.sh"
  # shellcheck disable=SC2016
  echo 'versions="${CAIRN_GIT_VERSIONS:-2.37.0 $(newest_released)}"' >"$tree/scripts/git-matrix.sh"
  printf 'env:\n  GIT_VERSION: 2.37.0\n' >"$tree/.github/workflows/ci.yml"
}

write_tree
run_guard "$tree"
[ "$status" -eq 0 ] || fail "expected exit 0 when all agree, got $status: $out"

# Each place can drift on its own, and the guard has to name that one.
drifted() {
  local file="$1"
  write_tree
  sed -i.bak 's/2\.37\.0/2.38.0/' "$tree/$file"
  rm -f "$tree/$file.bak"
  run_guard "$tree"
  [ "$status" -eq 1 ] || fail "expected exit 1 with $file drifted, got $status: $out"
  grep -q "^violation: $file" <<<"$out" || fail "didn't name $file: $out"
}
drifted scripts/oldest-git.sh
drifted scripts/git-matrix.sh
drifted .github/workflows/ci.yml

write_tree
sed -i.bak 's/minor: 37/minor: 38/' "$tree/crates/write/src/resolve.rs"
rm -f "$tree/crates/write/src/resolve.rs.bak"
run_guard "$tree"
[ "$status" -eq 1 ] || fail "expected exit 1 with MINIMUM drifted, got $status: $out"

# A place the guard can't read proves nothing, so it mustn't pass.
for file in crates/write/src/resolve.rs scripts/oldest-git.sh scripts/git-matrix.sh .github/workflows/ci.yml; do
  write_tree
  echo 'nothing to read' >"$tree/$file"
  run_guard "$tree"
  [ "$status" -eq 2 ] || fail "expected exit 2 with $file unreadable, got $status: $out"
done

run_guard "$tree/missing"
[ "$status" -eq 2 ] || fail "expected exit 2 with no tree, got $status"

[ "$failures" -eq 0 ] || { echo "$failures failure(s)" >&2; exit 1; }
echo "ok: check-git-floor.sh names what drifted and fails closed"
