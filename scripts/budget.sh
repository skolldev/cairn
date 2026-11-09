#!/usr/bin/env bash
# The Budget harness: builds Cairn in release with its probe, then measures it. Numbers count only
# from macOS and the physical Windows PC, never from a VM or a CI runner. See docs/budget-harness.md.
#
# Usage: budget.sh run [--only launch,write,external] [options]
#        budget.sh held-key [options]
set -euo pipefail

cd "$(dirname "$0")/.."
cargo build --release --locked -p cairn-app --features measure
cargo run --release --locked -p cairn-budget -- "$@"
