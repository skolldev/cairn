#!/usr/bin/env bash
# Turns on the tracked git hooks in scripts/hooks for this clone and its worktrees.
set -euo pipefail

git -C "$(dirname "$0")/.." config core.hooksPath scripts/hooks
echo "ok: git hooks come from scripts/hooks"
