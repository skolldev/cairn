#!/usr/bin/env bash
# Lists the files a build of the benches was compiled from, relative to the workspace root, one per
# line: every source rustc read for a workspace crate (from its dep-info), and the files that pick
# dependencies and the toolchain. check-bench-binaries.sh diffs them between two commits. Run it
# from the workspace root, after the build, with the same CARGO_TARGET_DIR. Needs jq.
#
# Usage: bench-sources.sh <cargo bench arguments>
# Fails if a workspace crate's artifact has no dep-info beside it, so a list can't silently shrink.
set -euo pipefail

# a/b/../c -> a/c. Paths outside the workspace (registry crates, OUT_DIR) are absolute and dropped.
normalize() {
  # shellcheck disable=SC1003 # tr reads '\\' as one backslash
  tr -d '\r' | tr '\\' '/' | jq -Rr '
    select(test("^(/|[A-Za-z]:/)") | not) |
    split("/") | reduce .[] as $part ([];
      if $part == "." or $part == "" then .
      elif $part == ".." then .[:-1]
      else . + [$part] end) |
    join("/")'
}

depinfo_for() {
  local dir stem
  dir="$(dirname "$1")"
  stem="$(basename "$1")"
  stem="${stem%.*}"
  for candidate in "$dir/$stem.d" "$dir/${stem#lib}.d"; do
    [ -f "$candidate" ] && { printf '%s\n' "$candidate"; return; }
  done
  # An executable has no extension on Unix, so its name may contain none to strip.
  [ -f "$1.d" ] && { printf '%s\n' "$1.d"; return; }
  # A binary is copied up out of deps/, where its dep-info stays under the crate name and a hash.
  # Every hash found is listed: another profile's build of the same binary only adds sources.
  local found=no
  for candidate in "$dir/deps/${stem//-/_}"-*.d; do
    [ -f "$candidate" ] && { printf '%s\n' "$candidate"; found=yes; }
  done
  [ "$found" = yes ] && return
  echo "error: no dep-info for $1" >&2
  return 1
}

built="$(cargo bench "$@" --no-run --message-format=json |
  jq -c 'select(.reason == "compiler-artifact" and (.package_id | test("^path\\+")))')"
artifacts="$(jq -r '(.filenames[] | select(test("\\.(rmeta|pdb|dSYM)$") | not)), (.executable // empty)' \
  <<<"$built" | tr -d '\r' | sort -u)"
[ -n "$artifacts" ] || { echo "error: cargo reported no workspace artifacts" >&2; exit 2; }

{
  while IFS= read -r artifact; do
    depinfos="$(depinfo_for "$artifact")" || exit 2
    while IFS= read -r depinfo; do
      # Each prerequisite also appears on a line of its own, as "path:".
      tr -d '\r' <"$depinfo" | grep -v '^#' | sed -n 's/^\(.*[^ ]\):$/\1/p'
    done <<<"$depinfos"
  done <<<"$artifacts"
  root="$(cargo metadata --format-version 1 --no-deps --locked | jq -r .workspace_root)"
  jq -r --arg root "$root" '.manifest_path | ltrimstr($root) | ltrimstr("/") | ltrimstr("\\")' <<<"$built"
  printf '%s\n' Cargo.toml Cargo.lock rust-toolchain.toml .cargo/config.toml
} | normalize | sort -u
