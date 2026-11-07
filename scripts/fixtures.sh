#!/usr/bin/env bash
# Creates Cairn's fixture repos under $CAIRN_FIXTURES (default ~/cairn-fixtures).
# Existing fixtures are left alone. The commits match #5, so numbers stay comparable.
#
# Usage: fixtures.sh small    ripgrep, for tests (CI runs this)
#        fixtures.sh filters  a small repo with autocrlf, eol attributes and a clean
#                             filter, for status tests (CI runs this)
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

# Every file is clean by git's reckoning, but none of them hashes to its blob without
# conversion: CRLF in the worktree under core.autocrlf and eol=crlf, and lowercase text
# whose clean filter uppercases it. Status that skips the filter pipeline shows each one
# as modified. The config is the repo's own, so it survives hermetic test config.
filters() {
  local dir="$root/filters"
  if [ -d "$dir/.git" ]; then
    echo "ok: $dir exists"
    return
  fi
  local tmp="$dir.tmp"
  rm -rf "$tmp"
  mkdir -p "$tmp"
  git -C "$tmp" init --quiet
  git -C "$tmp" config core.autocrlf true
  git -C "$tmp" config filter.upper.clean "tr a-z A-Z"
  printf '*.up filter=upper\n*.crlf text eol=crlf\n*.bin -text\n' > "$tmp/.gitattributes"
  printf 'one\ntwo\n' > "$tmp/lf.txt"
  printf 'one\r\ntwo\r\n' > "$tmp/crlf.txt"
  printf 'one\r\ntwo\r\n' > "$tmp/forced.crlf"
  printf 'raw\r\nbytes\r\n' > "$tmp/data.bin"
  printf 'quiet words\n' > "$tmp/shout.up"
  mkdir -p "$tmp/docs"
  printf 'nested\r\nfile\r\n' > "$tmp/docs/notes.txt"
  git -C "$tmp" add --all
  GIT_AUTHOR_DATE="2026-01-01T00:00:00Z" GIT_COMMITTER_DATE="2026-01-01T00:00:00Z" \
    git -C "$tmp" -c user.name=Cairn -c user.email=fixtures@cairn.invalid \
    commit --quiet -m "autocrlf and filters fixture"
  mv "$tmp" "$dir"
  echo "ok: $dir"
}

case "${1:-}" in
  small) small ;;
  filters) filters ;;
  *) echo "usage: $0 small|filters" >&2; exit 2 ;;
esac
