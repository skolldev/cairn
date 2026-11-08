#!/usr/bin/env bash
# Fails if a criterion bench got slower than its baseline by more than the threshold. Both runs
# must have been saved on the same machine, moments apart: numbers from different CI runners
# say nothing about a change. A regression here is a CI signal, not a Budget measurement.
#
# Usage: check-bench-regressions.sh [criterion dir] [threshold percent] [baseline] [new]
# Defaults: target/criterion, 30, base, head. Compares each bench's median. A bench only in
# the new run has nothing to regress from and passes, saying so, as does a bench only in the
# baseline. Needs jq.
set -euo pipefail

root="${1:-target/criterion}"
threshold="${2:-30}"
baseline="${3:-base}"
new="${4:-head}"

[ -d "$root" ] || { echo "error: no criterion results at $root" >&2; exit 2; }

compared=0
regressions=0
while IFS= read -r -d '' estimates; do
  bench="$(dirname "$(dirname "$estimates")")"
  name="${bench#"$root"/}"
  before="$bench/$baseline/estimates.json"
  if [ ! -f "$before" ]; then
    echo "new: $name has no $baseline run"
    continue
  fi
  compared=$((compared + 1))
  verdict="$(jq -rn \
    --slurpfile a "$before" --slurpfile b "$estimates" --argjson limit "$threshold" '
    ($a[0].median.point_estimate) as $before | ($b[0].median.point_estimate) as $after |
    (($after - $before) / $before * 100) as $change |
    "\(if $change > $limit then "regressed" else "ok" end) \($before / 1000000) \($after / 1000000) \($change)"' | tr -d '\r')"
  read -r state before_ms after_ms change <<<"$verdict"
  printf '%s: %s %.3f ms -> %.3f ms (%+.1f%%)\n' "$state" "$name" "$before_ms" "$after_ms" "$change"
  [ "$state" = ok ] || regressions=$((regressions + 1))
done < <(find "$root" -type f -path "*/$new/estimates.json" -print0 | sort -z)

# A bench that was removed or renamed passes, because a branch removing one must be able to
# land. So it's reported here, for a reader to tell the two apart. A run that compared nothing
# at all, below, must not read as a pass.
while IFS= read -r -d '' estimates; do
  bench="$(dirname "$(dirname "$estimates")")"
  [ -f "$bench/$new/estimates.json" ] || echo "gone: ${bench#"$root"/} has no $new run"
done < <(find "$root" -type f -path "*/$baseline/estimates.json" -print0 | sort -z)
if [ "$compared" -eq 0 ]; then
  echo "error: no bench under $root has both a $baseline and a $new run" >&2
  exit 2
fi

if [ "$regressions" -ne 0 ]; then
  echo "error: $regressions of $compared bench(es) slowed by more than $threshold%" >&2
  exit 1
fi
echo "ok: $compared bench(es) within $threshold% of $baseline"
