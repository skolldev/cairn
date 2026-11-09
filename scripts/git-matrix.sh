#!/usr/bin/env bash
# Runs the tests that run git against the oldest git Cairn supports and the newest git released,
# building either if needed, so a test leaning on one git's incidental behaviour fails here and
# not in CI. CI's macOS and Windows runners follow the newest git without saying so.
#
# Usage: git-matrix.sh [cargo test arguments]
# Defaults: -p cairn-write -p cairn-repo -p cairn-budget -p cairn-app --features cairn-app/measure
# --no-fail-fast. The feature is what brings in the Budget harness probe's tests, which run git too.
# Environment: CAIRN_GIT_VERSIONS overrides the versions to run, as a space-separated list. Builds
# go to ~/git-<version>, as with oldest-git.sh. Create the fixtures first with your usual git.
# It closes with one total per version, then an ok or error line. Its exit code is the result.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"

newest_released() {
  git ls-remote --tags --refs https://github.com/git/git 'v[0-9]*' |
    sed 's|.*refs/tags/v||' | grep -E '^[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | tail -1
}

versions="${CAIRN_GIT_VERSIONS:-2.37.0 $(newest_released)}"
[ "$#" -gt 0 ] || set -- -p cairn-write -p cairn-repo -p cairn-budget -p cairn-app \
  --features cairn-app/measure --no-fail-fast

log="$(mktemp)"
trap 'rm -f "$log"' EXIT

failed=()
totals=""
for version in $versions; do
  bin="$("$here/oldest-git.sh" "$version")"
  echo "== git $version"
  note=""
  if ! PATH="$bin:$PATH" CAIRN_EXPECT_GIT_VERSION="$version" cargo test "$@" 2>&1 | tee "$log"; then
    failed+=("$version")
    note=", cargo failed"
  fi
  counts="$(awk '/^test result:/ {
    for (i = 2; i <= NF; i++) {
      if ($i == "passed;") passed += $(i - 1)
      if ($i == "failed;") failed += $(i - 1)
    }
  } END { print passed + 0 " passed, " failed + 0 " failed" }' "$log")"
  # A failing test already says so in its count.
  case "$counts" in *", 0 failed") ;; *) note="" ;; esac
  totals="$totals
git $version: $counts$note"
done

echo "$totals"

if [ "${#failed[@]}" -ne 0 ]; then
  echo "error: tests failed on git ${failed[*]}" >&2
  exit 1
fi
echo "ok: tests pass on git $versions"
