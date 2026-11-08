#!/usr/bin/env bash
# Shows that `fixtures.sh check` passes fixtures that are what their names claim, reports each way
# one isn't with a fix, changes nothing, and lets the local-only kernel fixtures be absent.
# Copies the real small fixture, so `fixtures.sh small` must have run.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
tool="$here/fixtures.sh"
source_root="${CAIRN_FIXTURES:-$HOME/cairn-fixtures}"
failures=0
fail() { echo "FAIL: $*"; failures=$((failures + 1)); }

root="$(mktemp -d)"
trap 'rm -rf "${root:?}"' EXIT
export CAIRN_FIXTURES="$root"

run_check() {
  set +e
  out="$("$tool" check 2>&1)"
  status=$?
  set -e
}

expect_problem() {
  run_check
  [ "$status" -eq 1 ] || fail "$1: expected exit 1, got $status: $out"
  grep -q "^wrong: .*$1" <<<"$out" || fail "$1: not reported: $out"
  grep -q "^  fix: " <<<"$out" || fail "$1: no fix printed: $out"
}

remove_graph() {
  rm -rf "${1:?}/.git/objects/info/commit-graph" "${1:?}/.git/objects/info/commit-graphs"
}

# Everything missing fails, because the tests need the small fixtures.
expect_problem "ripgrep is missing"
expect_problem "filters is missing"

git clone --quiet --no-hardlinks "$source_root/ripgrep" "$root/ripgrep"
remove_graph "$root/ripgrep"
pin="$(git -C "$source_root/ripgrep" rev-parse HEAD)"
git -C "$root/ripgrep" -c advice.detachedHead=false checkout --quiet --detach "$pin"
"$tool" filters >/dev/null

# Kernel fixtures absent: a pass, saying so.
run_check
[ "$status" -eq 0 ] || fail "absent kernel fixtures should pass, got $status: $out"
grep -q "^absent: .*linux-graph" <<<"$out" || fail "didn't say linux-graph is absent: $out"
grep -q "^absent: .*linux-nograph" <<<"$out" || fail "didn't say linux-nograph is absent: $out"

# The small fixture with a graph, then at the wrong commit.
git -C "$root/ripgrep" commit-graph write --reachable
expect_problem "ripgrep has a commit-graph"
[ -e "$root/ripgrep/.git/objects/info/commit-graph" ] || [ -d "$root/ripgrep/.git/objects/info/commit-graphs" ] ||
  fail "check changed the fixture"
remove_graph "$root/ripgrep"
git -C "$root/ripgrep" -c advice.detachedHead=false checkout --quiet --detach "$pin~1"
expect_problem "ripgrep is at "
git -C "$root/ripgrep" -c advice.detachedHead=false checkout --quiet --detach "$pin"

# The filters fixture without its config.
git -C "$root/filters" config --unset core.autocrlf
expect_problem "filters has lost"
git -C "$root/filters" config core.autocrlf true

# Kernel stand-ins: small repositories, set up as #5 sets up the real ones.
kernel() {
  git clone --quiet --no-hardlinks "$root/ripgrep" "$root/$1"
  remove_graph "$root/$1"
  git -C "$root/$1" config gc.auto 0
  git -C "$root/$1" config maintenance.auto false
  git -C "$root/$1" config gc.writeCommitGraph false
  git -C "$root/$1" config fetch.writeCommitGraph false
}
kernel linux-graph
kernel linux-nograph
expect_problem "linux-graph has no commit-graph"
git -C "$root/linux-graph" commit-graph write --reachable
run_check
[ "$status" -eq 0 ] || fail "good kernel fixtures should pass, got $status: $out"

git -C "$root/linux-nograph" commit-graph write --reachable
expect_problem "linux-nograph has a commit-graph"
remove_graph "$root/linux-nograph"

git -C "$root/linux-nograph" config --unset gc.auto
expect_problem "linux-nograph has gc.auto"
git -C "$root/linux-nograph" config gc.auto 0

git -C "$root/linux-nograph" -c advice.detachedHead=false checkout --quiet --detach "$pin~1"
expect_problem "the kernel fixtures differ"

[ "$failures" -eq 0 ] || { echo "$failures failure(s)" >&2; exit 1; }
echo "ok: fixtures.sh check reports each wrong fixture and passes good ones"
