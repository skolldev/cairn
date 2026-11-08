#!/usr/bin/env bash
set -euo pipefail

# A Force quit can leave git's hook or remote wrapper behind when a test exits.
# Give a released process a moment to finish before calling it a leak.
for _ in 1 2 3 4 5; do
  if pids=$(pgrep -f '[.]git/hooks/post-checkout|slow-remote[.]sh'); then
    sleep 1
  else
    status=$?
    if [ "$status" -eq 1 ]; then
      exit 0
    fi
    exit "$status"
  fi
done

printf 'Test hook processes are still running:\n%s\n' "$pids" >&2
exit 1
