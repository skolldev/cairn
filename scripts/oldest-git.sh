#!/usr/bin/env bash
# Builds the oldest git Cairn supports from source, so the tests that run git can run against it
# locally exactly as the `oldest-git` CI job does. Skips the build if the prefix already
# holds that version. Prints the bin directory to put first on PATH.
#
# Usage: scripts/oldest-git.sh [version] [prefix]
# Defaults: 2.23.0, and ~/git-<version>. Needs make, a C compiler, curl and zlib headers.
#
#   PATH="$(scripts/oldest-git.sh):$PATH" CAIRN_EXPECT_GIT_VERSION=2.23.0 \
#     cargo test -p cairn-write -p cairn-repo --no-fail-fast
#
# Create the fixtures first with your usual git: 2.23 is built without curl.
set -euo pipefail

version="${1:-2.23.0}"
prefix="${2:-$HOME/git-$version}"
git="$prefix/bin/git"

if [ -x "$git" ] && [ "$("$git" --version)" = "git version $version" ]; then
  echo "$prefix/bin"
  exit 0
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
curl -fsSL "https://mirrors.edge.kernel.org/pub/software/scm/git/git-$version.tar.gz" | tar xz -C "$work"
jobs="$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 2)"
# Current compilers default to C23, where thread_local is a keyword git 2.23 uses as a name,
# and reject some pre-C99 idioms outright. Build it as the C it was written in.
make -C "$work/git-$version" -j"$jobs" prefix="$prefix" \
  CFLAGS="-O2 -std=gnu11 -fpermissive" \
  NO_GETTEXT=1 NO_TCLTK=1 NO_PERL=1 NO_PYTHON=1 NO_CURL=1 NO_EXPAT=1 NO_OPENSSL=1 \
  install >&2
[ "$("$git" --version)" = "git version $version" ] || { echo "error: built git is not $version" >&2; exit 1; }
echo "$prefix/bin"
