#!/usr/bin/env bash
# Creates Cairn's fixture repos under $CAIRN_FIXTURES (default ~/cairn-fixtures).
# Existing fixtures are left alone. The commits match #5, so numbers stay comparable.
#
# Usage: fixtures.sh small    ripgrep, for tests (CI runs this)
#
# The Linux kernel fixtures are local only. #5 has their setup.
set -euo pipefail

root="${CAIRN_FIXTURES:-$HOME/cairn-fixtures}"
mkdir -p "$root"

small() {
  local dir="$root/ripgrep"
  local pin=91f5d22ef48bde459d90a8c7dbeab416e97d0f2b
  if [ -d "$dir/.git" ]; then
    echo "ok: $dir exists"
    return
  fi
  git clone --quiet https://github.com/BurntSushi/ripgrep.git "$dir"
  git -C "$dir" -c advice.detachedHead=false checkout --quiet --detach "$pin"
  echo "ok: $dir at $pin"
}

case "${1:-}" in
  small) small ;;
  *) echo "usage: $0 small" >&2; exit 2 ;;
esac
