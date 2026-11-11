#!/usr/bin/env bash
# Removes this workspace's own crates from the release build in CARGO_TARGET_DIR, keeping its
# dependencies. Cargo decides a workspace crate is fresh by comparing mtimes, so a build left by
# another checkout of the repo can pass for this one's. Run it from the workspace root. Needs jq.
set -euo pipefail

packages=()
while IFS= read -r name; do
  packages+=(-p "$name")
done < <(cargo metadata --format-version 1 --no-deps --locked | jq -r '.packages[].name' | tr -d '\r')
[ "${#packages[@]}" -gt 0 ] || { echo "error: no workspace crates found" >&2; exit 2; }
cargo clean --release "${packages[@]}"
