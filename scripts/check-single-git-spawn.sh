#!/usr/bin/env bash
# Fails if any Rust file under crates/ outside the allowed paths starts a process. Only
# cairn-write's runner may spawn git (#14), and it carries the rules every spawn needs: the
# real binary, -C, core.longpaths, no prompts, no console window. Tests included: they go
# through cairn-write too, so they run the same git the product does. The Budget harness has to
# start `cairn` and PresentMon, so it has one file of its own, and that file refuses git.
#
# Usage: check-single-git-spawn.sh [root] [allowed path prefix...]
# Defaults: the current directory, `crates/write/src/` and `crates/budget/src/spawn.rs`.
set -euo pipefail

root="${1:-.}"
shift || true
allowed=("$@")
[ "${#allowed[@]}" -gt 0 ] || allowed=(crates/write/src/ crates/budget/src/spawn.rs)

[ -d "$root/crates" ] || { echo "error: no crates/ directory under $root" >&2; exit 2; }

is_allowed() {
  local prefix
  for prefix in "${allowed[@]}"; do [[ "$1" == "$prefix"* ]] && return 0; done
  return 1
}

# Spawning means naming std's Command type: Command::new, or importing it to call it.
pattern='(process::Command|Command::new)'
violations=0
checked=0
while IFS= read -r -d '' file; do
  rel="${file#"$root"/}"
  is_allowed "$rel" && continue
  checked=$((checked + 1))
  while IFS= read -r hit; do
    echo "violation: $rel:$hit"
    violations=$((violations + 1))
  done < <(grep -nE "$pattern" "$file" || true)
done < <(find "$root/crates" -name '*.rs' -not -path '*/target/*' -print0)

[ "$checked" -gt 0 ] || { echo "error: no Rust files checked under $root/crates" >&2; exit 2; }
if [ "$violations" -ne 0 ]; then
  echo "error: only ${allowed[*]} may start processes ($violations violation(s)). Run git through cairn_write::Git::run." >&2
  exit 1
fi
echo "ok: $checked file(s) start no processes (allowed: ${allowed[*]})"
