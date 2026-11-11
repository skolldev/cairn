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
# It prints what cargo says only where it needs reading: compile errors and warnings, failing tests
# and their output. Cargo's full output for each version is kept in a log, named at the end beside
# one total per version, then an ok or error line. Its exit code is the result.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"

newest_released() {
  git ls-remote --tags --refs https://github.com/git/git 'v[0-9]*' |
    sed 's|.*refs/tags/v||' | grep -E '^[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | tail -1
}

versions="${CAIRN_GIT_VERSIONS:-2.37.0 $(newest_released)}"
[ "$#" -gt 0 ] || set -- -p cairn-write -p cairn-repo -p cairn-budget -p cairn-app \
  --features cairn-app/measure --no-fail-fast

tmp="${TMPDIR:-/tmp}"
logs="$(mktemp -d "${tmp%/}/git-matrix.XXXXXX")"

# Drops the lines a passing run is made of, and keeps everything else, so a kind of line nobody
# listed here still shows.
quiet() {
  awk '
    /^ *(Compiling|Checking|Finished|Running|Doc-tests|Fresh|Blocking|Updating|Locking|Downloading|Downloaded) / { next }
    /^running [0-9]+ tests?$/ || /^test .* \.\.\. (ok|ignored)$/ || /^test result: ok\./ { next }
    /^$/ { blank = 1; next }
    { if (blank && shown) print ""; blank = 0; shown = 1; print; fflush() }
  '
}

failed=()
totals=""
lognames=""
for version in $versions; do
  bin="$("$here/oldest-git.sh" "$version")"
  log="$logs/$version.log"
  echo "== git $version, all of it in $log"
  note=""
  if ! PATH="$bin:$PATH" CAIRN_EXPECT_GIT_VERSION="$version" cargo test "$@" 2>&1 | tee "$log" | quiet; then
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
  lognames="$lognames
log for git $version: $log"
done

echo "$lognames"
echo "$totals"

if [ "${#failed[@]}" -ne 0 ]; then
  echo "error: tests failed on git ${failed[*]}" >&2
  exit 1
fi
echo "ok: tests pass on git $versions"
