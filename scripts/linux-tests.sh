#!/usr/bin/env bash
# Runs tests in a Linux container, so inotify's behaviour (docs/filesystem-events.md) can turn a
# test red on a Mac before CI's Linux job does. Uses the container's git, not the oldest one.
#
# Usage: scripts/linux-tests.sh [cargo test arguments]
# Default: -p cairn-repo --test watch. Needs docker running and the fixtures created first
# (scripts/fixtures.sh), which are copied in so that git trusts them.
# Environment: CAIRN_LINUX_IMAGE overrides the image. Builds and toolchains persist in docker
# volumes named cairn-linux-*.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
fixtures="${CAIRN_FIXTURES:-$HOME/cairn-fixtures}"
image="${CAIRN_LINUX_IMAGE:-rust:1-bookworm}"
if [ "$#" -eq 0 ]; then
  set -- -p cairn-repo --test watch
fi

# The Linux kernel fixtures are too large to copy, and no test the default runs uses them.
exec docker run --rm \
  -v "$root":/cairn:ro \
  -v "$fixtures/ripgrep":/fixtures-host/ripgrep:ro \
  -v "$fixtures/filters":/fixtures-host/filters:ro \
  -v cairn-linux-target:/target \
  -v cairn-linux-registry:/usr/local/cargo/registry \
  -v cairn-linux-git:/usr/local/cargo/git \
  -v cairn-linux-rustup:/usr/local/rustup \
  -e CARGO_TARGET_DIR=/target \
  -e CAIRN_FIXTURES=/fixtures \
  -w /cairn \
  "$image" \
  sh -c 'cp -r /fixtures-host /fixtures && cargo test --locked --no-fail-fast "$@"' sh "$@"
