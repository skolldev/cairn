#!/usr/bin/env bash
# Decides whether two bench builds are worth timing against each other, and fails if the newer
# build is the older one reused. Each list comes from bench-sources.sh, each hash file from
# bench-binary-hashes.sh. Base and head build into separate target directories, whose absolute
# paths end up in the executables, so two real builds never produce the same bytes: identical
# executables after a source changed mean one build was never made.
#
# Usage: check-bench-binaries.sh <base commit> <base sources> <head sources> <base hashes> <head hashes>
# Run inside the head commit's checkout.
# Exit: 0 a bench source changed, so compare the timings; 3 none did, so timing differences are
# runner noise; 1 a source changed but the executables are identical; 2 bad input.
set -euo pipefail

[ "$#" -eq 5 ] || { echo "usage: $0 <base commit> <base sources> <head sources> <base hashes> <head hashes>" >&2; exit 2; }
base="$1"
for list in "$2" "$3" "$4" "$5"; do
  [ -s "$list" ] || { echo "error: $list is missing or empty" >&2; exit 2; }
done
git rev-parse --verify --quiet "$base^{commit}" >/dev/null ||
  { echo "error: $base is not a commit" >&2; exit 2; }

sources=()
while IFS= read -r path; do
  [ -n "$path" ] && sources+=("$path")
done < <(cat "$2" "$3" | tr -d '\r' | sort -u)

changed="$(git -c core.quotepath=off --literal-pathspecs diff --name-only "$base" HEAD -- "${sources[@]}")"
if [ -z "$changed" ]; then
  echo "unchanged: none of the ${#sources[@]} files the benches are built from changed since $base; timing differences are runner noise"
  exit 3
fi
# shellcheck disable=SC2001 # prefixes every line, which ${var//} can't
sed 's/^/changed: /' <<<"$changed"

if cmp -s "$4" "$5"; then
  echo "error: bench sources changed, yet the executables are identical: the head build reused the base's" >&2
  exit 1
fi
echo "ok: the executables differ, so their timings are worth comparing"
