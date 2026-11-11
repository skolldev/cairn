#!/usr/bin/env bash
# The Budget harness: builds Cairn in release with its probe, then measures it. Numbers count only
# from macOS and the physical Windows PC, never from a VM or a CI runner. See docs/budget-harness.md.
#
# Usage: budget.sh run [--only launch,write,external] [options]
#        budget.sh held-key [options]
set -euo pipefail

cd "$(dirname "$0")/.."
# On Windows, run and held-key time frames with PresentMon: fetch it unless one is given.
case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*)
    case "${1:-}" in
      run | held-key)
        if [ -z "${CAIRN_PRESENTMON:-}" ] && [[ " $* " != *" --presentmon "* ]]; then
          exe="$(scripts/presentmon.sh)"
          CAIRN_PRESENTMON="$(cygpath -m "$exe")"
          export CAIRN_PRESENTMON
        fi
        ;;
    esac
    ;;
esac
cargo build --release --locked -p cairn-app --features measure
cargo run --release --locked -p cairn-budget -- "$@"
