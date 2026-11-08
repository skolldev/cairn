#!/usr/bin/env bash
# Runs the tests that run git against the oldest git Cairn supports and the newest git released,
# building either if needed, so a test leaning on one git's incidental behaviour fails here and
# not in CI. CI's macOS and Windows runners follow the newest git without saying so.
#
# Usage: git-matrix.sh [cargo test arguments]
# Defaults: -p cairn-write -p cairn-repo -p cairn-app --no-fail-fast
# Environment: CAIRN_GIT_VERSIONS overrides the versions to run, as a space-separated list. Builds
# go to ~/git-<version>, as with oldest-git.sh. Create the fixtures first with your usual git.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"

newest_released() {
  git ls-remote --tags --refs https://github.com/git/git 'v[0-9]*' |
    sed 's|.*refs/tags/v||' | grep -E '^[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | tail -1
}

versions="${CAIRN_GIT_VERSIONS:-2.23.0 $(newest_released)}"
[ "$#" -gt 0 ] || set -- -p cairn-write -p cairn-repo -p cairn-app --no-fail-fast

failed=()
for version in $versions; do
  bin="$("$here/oldest-git.sh" "$version")"
  echo "== git $version"
  if ! PATH="$bin:$PATH" CAIRN_EXPECT_GIT_VERSION="$version" cargo test "$@"; then
    failed+=("$version")
  fi
done

if [ "${#failed[@]}" -ne 0 ]; then
  echo "error: tests failed on git ${failed[*]}" >&2
  exit 1
fi
echo "ok: tests pass on git $versions"
