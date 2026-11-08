#!/usr/bin/env bash
# Fails if a Rust file under crates/ builds a shell script that picks a first run by testing for
# a file and then creating it. Two processes can both pass that test. `block_first_run` in
# crates/repo/tests/common uses `mkdir`, which only one of them can win.
#
# Usage: check-test-gates.sh [root]
set -euo pipefail

root="${1:-.}"
[ -d "$root/crates" ] || { echo "error: no crates/ directory under $root" >&2; exit 2; }

violations=0
checked=0
while IFS= read -r -d '' file; do
  checked=$((checked + 1))
  while IFS= read -r hit; do
    echo "violation: ${file#"$root"/}:$hit"
    violations=$((violations + 1))
  done < <(grep -nE 'if \[ ! -[ef] ' "$file" || true)
done < <(find "$root/crates" -name '*.rs' -not -path '*/target/*' -print0)

[ "$checked" -gt 0 ] || { echo "error: no Rust files checked under $root/crates" >&2; exit 2; }
if [ "$violations" -ne 0 ]; then
  echo "error: $violations check-then-create gate(s). Use block_first_run from crates/repo/tests/common." >&2
  exit 1
fi
echo "ok: $checked file(s) have no check-then-create gate"
