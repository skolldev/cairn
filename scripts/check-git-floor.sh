#!/usr/bin/env bash
# Fails if the git Cairn refuses below isn't the git it tests against. `GitVersion::MINIMUM` is the
# floor. oldest-git.sh, git-matrix.sh and the oldest-git CI job each name a version to build, and
# without this they could drift while CI stays green, testing a git the floor no longer means.
#
# Usage: check-git-floor.sh [root]
set -euo pipefail

root="${1:-.}"

resolve="crates/write/src/resolve.rs"
oldest="scripts/oldest-git.sh"
matrix="scripts/git-matrix.sh"
ci=".github/workflows/ci.yml"

for file in "$resolve" "$oldest" "$matrix" "$ci"; do
  [ -f "$root/$file" ] || { echo "error: no $file under $root" >&2; exit 2; }
done

# The version from each file's one line that names it. Empty if the line has moved or changed shape.
minimum="$(awk '
  /pub const MINIMUM/ { inside = 1; next }
  inside && /major:/ { gsub(/[^0-9]/, ""); major = $0; next }
  inside && /minor:/ { gsub(/[^0-9]/, ""); minor = $0; next }
  inside && /patch:/ { gsub(/[^0-9]/, ""); print major "." minor "." $0; exit }
' "$root/$resolve")"
version='[0-9]+\.[0-9]+\.[0-9]+'
oldest_default="$(sed -nE "s/^version=\"\\\$\{1:-($version)\}\"\$/\1/p" "$root/$oldest")"
matrix_default="$(sed -nE "s/^versions=\"\\\$\{CAIRN_GIT_VERSIONS:-($version) .*/\1/p" "$root/$matrix")"
ci_version="$(sed -nE "s/^ *GIT_VERSION: ($version)\$/\1/p" "$root/$ci")"

[ -n "$minimum" ] || { echo "error: can't read GitVersion::MINIMUM from $resolve" >&2; exit 2; }
[ -n "$oldest_default" ] || { echo "error: can't read the default version from $oldest" >&2; exit 2; }
[ -n "$matrix_default" ] || { echo "error: can't read the default versions from $matrix" >&2; exit 2; }
[ -n "$ci_version" ] || { echo "error: can't read GIT_VERSION from $ci" >&2; exit 2; }

violations=0
compare() {
  if [ "$2" != "$minimum" ]; then
    echo "violation: $1 names git $2"
    violations=$((violations + 1))
  fi
}
compare "$oldest" "$oldest_default"
compare "$matrix" "$matrix_default"
compare "$ci" "$ci_version"

if [ "$violations" -ne 0 ]; then
  echo "error: GitVersion::MINIMUM is $minimum, and $violations place(s) test a different git." >&2
  exit 1
fi
echo "ok: the floor and what tests it agree on git $minimum"
