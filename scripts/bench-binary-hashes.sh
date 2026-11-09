#!/usr/bin/env bash
set -euo pipefail

# Compare executable contents, not timing, when Cargo reused a benchmark binary.
cargo bench "$@" --no-run --message-format=json |
  jq -r 'select(.reason == "compiler-artifact" and (.target.kind | index("bench")) and .executable != null) | .target.name, .executable' |
  while IFS= read -r name && IFS= read -r binary; do
    hash="$(git hash-object "$binary")"
    printf '%s %s\n' "$name" "$hash"
  done | sort
