#!/usr/bin/env bash
# Fails if any workspace member outside the allowed list depends on GPUI, directly or
# transitively, through normal, build or dev dependencies, on any target.
#
# GPUI means any package whose name is `gpui` or starts with `gpui-` or `gpui_`: gpui,
# gpui-pre and its platform crates, gpui-kit, gpui-component and their siblings.
#
# Usage: check-gpui-free.sh [manifest] [allowed member...]
# Defaults: the workspace's Cargo.toml, and `cairn-app` as the only allowed member.
set -euo pipefail

manifest="${1:-Cargo.toml}"
shift || true
allowed=("$@")
[ "${#allowed[@]}" -gt 0 ] || allowed=(cairn-app)

gpui_cargo_tree() { cargo tree --manifest-path "$manifest" --locked --target all -e normal,build,dev --prefix none --format '{p}' "$@"; }

is_allowed() {
  local m
  for m in "${allowed[@]}"; do [ "$m" = "$1" ] && return 0; done
  return 1
}

members="$(gpui_cargo_tree --workspace --depth 0 | awk 'NF { print $1 }' | sort -u)"
[ -n "$members" ] || { echo "error: no workspace members found in $manifest" >&2; exit 2; }

violations=0
checked=0
for member in $members; do
  is_allowed "$member" && continue
  checked=$((checked + 1))
  # A plain assignment, so `set -e` stops the script if cargo fails: a graph the guard can't
  # read must never count as GPUI-free.
  graph="$(gpui_cargo_tree -p "$member")"
  # Skip the first line, which is the member itself. Only grep's no-match is tolerated.
  gpui="$(tail -n +2 <<<"$graph" | awk '{ print $1 }' | { grep -E '^gpui([-_]|$)' || true; } | sort -u)"
  for crate in $gpui; do
    echo "violation: $member depends on $crate"
    violations=$((violations + 1))
  done
done

if [ "$violations" -ne 0 ]; then
  echo "error: only ${allowed[*]} may depend on GPUI ($violations violation(s)). Run \`cargo tree -p <member> -i <crate>\` to see the path." >&2
  exit 1
fi
echo "ok: $checked member(s) are GPUI-free (allowed: ${allowed[*]})"
