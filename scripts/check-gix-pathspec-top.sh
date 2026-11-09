#!/usr/bin/env bash
# Fails if a gix pathspec in crates/read names its magic without `top`. gix reads a pattern as
# relative to the process's current directory when that lies inside the worktree (#40), so
# Cairn started from a subdirectory would match the wrong paths. `:(top,...)` anchors it at the
# worktree root, whatever the current directory. docs/gix-notes.md has the source.
#
# Only crates/read uses gix. git's own pathspecs, such as `unstaged_within`'s, run in the
# worktree and aren't affected.
#
# Usage: check-gix-pathspec-top.sh [root]
set -euo pipefail

root="${1:-.}"
dir="$root/crates/read"
[ -d "$dir" ] || { echo "error: no crates/read directory under $root" >&2; exit 2; }

violations=0
checked=0
while IFS= read -r -d '' file; do
  checked=$((checked + 1))
  # A string literal that opens with pathspec magic, `":(`, without `top` before its `)`.
  while IFS= read -r hit; do
    echo "violation: ${file#"$root"/}:$hit"
    violations=$((violations + 1))
  done < <(grep -nE '":\(' "$file" | grep -vE '":\(([a-z,]*,)?top[,)]' || true)
done < <(find "$dir" -name '*.rs' -not -path '*/target/*' -print0)

[ "$checked" -gt 0 ] || { echo "error: no Rust files checked under $dir" >&2; exit 2; }
if [ "$violations" -ne 0 ]; then
  echo "error: $violations gix pathspec(s) without \`top\`. Write \`:(top,literal)\`: see docs/gix-notes.md." >&2
  exit 1
fi
echo "ok: $checked file(s) anchor every gix pathspec at the worktree root"
