#!/usr/bin/env bash
# Creates Cairn's fixture repos under $CAIRN_FIXTURES (default ~/cairn-fixtures).
# Existing fixtures are left alone. The commits match #5, so numbers stay comparable.
#
# Usage: fixtures.sh small    ripgrep, for tests (CI runs this)
#        fixtures.sh filters  a small repo with autocrlf, eol attributes and a clean
#                             filter, for status tests (CI runs this)
#        fixtures.sh check    confirms each fixture is what its name claims, and prints the
#                             fix for any that isn't. It changes nothing.
#
# The Linux kernel fixtures are local only. #5 has their setup.
set -euo pipefail

root="${CAIRN_FIXTURES:-$HOME/cairn-fixtures}"
mkdir -p "$root"

small_pin=91f5d22ef48bde459d90a8c7dbeab416e97d0f2b

small() {
  local dir="$root/ripgrep"
  local pin="$small_pin"
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

has_commit_graph() {
  local info="$1/.git/objects/info"
  [ -e "$info/commit-graph" ] || [ -e "$info/commit-graphs/commit-graph-chain" ]
}

problems=0
problem() {
  echo "wrong: $1"
  echo "  fix: $2"
  problems=$((problems + 1))
}

# Printed for the user to run, because these fixtures are shared with other tools.
how_to_drop_graph() {
  echo "delete commit-graph and commit-graphs from $1/.git/objects/info"
}

# Each of these is what the kernel clones were set up with (#5), so that runs stay comparable.
check_kernel_config() {
  local dir="$1" key want got
  for key in gc.auto=0 maintenance.auto=false gc.writeCommitGraph=false fetch.writeCommitGraph=false; do
    want="${key#*=}"
    key="${key%=*}"
    got="$(git -C "$dir" config --get "$key" || true)"
    [ "$got" = "$want" ] || problem "$dir has $key='$got', not '$want'" "git -C '$dir' config $key $want"
  done
}

check() {
  local dir="$root/ripgrep"
  if [ ! -d "$dir/.git" ]; then
    problem "$dir is missing" "scripts/fixtures.sh small"
  else
    local head
    head="$(git -C "$dir" rev-parse HEAD)"
    [ "$head" = "$small_pin" ] ||
      problem "$dir is at $head, not $small_pin" "git -C '$dir' checkout --detach $small_pin"
    # CI's copy has none, and walks on a local one would start from another state.
    if has_commit_graph "$dir"; then
      problem "$dir has a commit-graph, which CI's copy doesn't" "$(how_to_drop_graph "$dir")"
    fi
  fi

  dir="$root/filters"
  if [ ! -d "$dir/.git" ]; then
    problem "$dir is missing" "scripts/fixtures.sh filters"
  elif [ "$(git -C "$dir" config --get core.autocrlf || true)" != true ] ||
    [ -z "$(git -C "$dir" config --get filter.upper.clean || true)" ]; then
    problem "$dir has lost its autocrlf or filter config" "delete $dir, then run scripts/fixtures.sh filters"
  fi

  local graph="$root/linux-graph" no_graph="$root/linux-nograph" setup="#5 has the setup"
  if [ -d "$graph/.git" ]; then
    has_commit_graph "$graph" ||
      problem "$graph has no commit-graph" "git -C '$graph' commit-graph write --reachable"
    check_kernel_config "$graph"
  else
    echo "absent: $graph, so its benches skip. $setup."
  fi
  if [ -d "$no_graph/.git" ]; then
    if has_commit_graph "$no_graph"; then
      problem "$no_graph has a commit-graph, so its benches skip" "$(how_to_drop_graph "$no_graph")"
    fi
    check_kernel_config "$no_graph"
  else
    echo "absent: $no_graph, so its benches skip. $setup."
  fi
  if [ -d "$graph/.git" ] && [ -d "$no_graph/.git" ]; then
    local a b
    a="$(git -C "$graph" rev-parse HEAD)"
    b="$(git -C "$no_graph" rev-parse HEAD)"
    [ "$a" = "$b" ] || problem "the kernel fixtures differ: $graph at $a, $no_graph at $b" "$setup"
  fi

  if [ "$problems" -ne 0 ]; then
    echo "error: $problems fixture problem(s) under $root" >&2
    exit 1
  fi
  echo "ok: the fixtures under $root are what their names claim"
}

case "${1:-}" in
  small) small ;;
  filters) filters ;;
  check) check ;;
  *) echo "usage: $0 small|filters|check" >&2; exit 2 ;;
esac
