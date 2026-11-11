#!/usr/bin/env bash
# Fetches the PresentMon console exe the Budget harness times Windows frames with, and checks it
# against a pinned hash. Other versions name their CSV columns differently. Skips the download if
# the prefix already holds it. Prints the exe's path, for CAIRN_PRESENTMON. budget.sh runs it on
# Windows when CAIRN_PRESENTMON is unset.
#
# Usage: scripts/presentmon.sh [prefix]
# Default: ~/presentmon-2.6.0. Needs curl and sha256sum, as Git Bash has.
set -euo pipefail

version="2.6.0"
sha256="b2a706bc6ad475749e3b7e3409263aa1e6906d45bdcf993f6dbc0f660188f1af"
prefix="${1:-$HOME/presentmon-$version}"
exe="$prefix/PresentMon-$version-x64.exe"

matches() { [ "$(sha256sum "$1" | cut -d' ' -f1)" = "$sha256" ]; }

if [ -f "$exe" ] && matches "$exe"; then
  echo "$exe"
  exit 0
fi

mkdir -p "$prefix"
part="$exe.part"
trap 'rm -f "$part"' EXIT
curl -fsSL -o "$part" \
  "https://github.com/GameTechDev/PresentMon/releases/download/v$version/PresentMon-$version-x64.exe"
matches "$part" || { echo "error: the downloaded PresentMon $version doesn't match its pinned hash" >&2; exit 1; }
mv "$part" "$exe"
echo "$exe"
