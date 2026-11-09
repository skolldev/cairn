#!/usr/bin/env bash
# Shows that seen-red.py passes only a break whose test goes red, and always restores the file.
# Uses a small crate of its own, so it needs only cargo.
set -euo pipefail
tool="$(cd "$(dirname "$0")" && pwd)/seen-red.py"
failures=0
fail() { echo "FAIL: $*"; failures=$((failures + 1)); }

crate="$(mktemp -d)"
trap 'rm -rf "${crate:?}"' EXIT
cd "$crate"
cat >Cargo.toml <<'TOML'
[package]
name = "seen-red-fixture"
version = "0.0.0"
edition = "2021"

[workspace]
TOML
mkdir src
cat >src/lib.rs <<'RUST'
pub fn answer() -> u32 {
    42
}

pub fn unguarded() -> u32 {
    7
}

#[test]
fn answer_is_right() {
    assert_eq!(answer(), 42, "the wrong answer");
}

#[test]
fn fails_after_a_caught_panic() {
    let caught = std::panic::catch_unwind(|| panic!("an expected panic"));
    assert!(caught.is_err());
    assert_eq!(unguarded(), 7, "the real failure");
}

#[test]
fn already_red() {
    if std::env::var_os("SEEN_RED_ALREADY_RED").is_some() {
        panic!("red before any break");
    }
}
RUST
original="$(cat src/lib.rs)"
cargo test -q >/dev/null 2>&1

# expect <exit status> <status word> <what it means> <seen-red.py arguments>
expect() {
  local want="$1" word="$2" what="$3"
  shift 3
  set +e
  out="$(python3 "$tool" "$@" 2>&1)"
  status=$?
  set -e
  [ "$status" -eq "$want" ] || fail "$what: expected exit $want, got $status: $out"
  grep -q "^$word" <<<"$out" || fail "$what: expected $word: $out"
  [ "$(cat src/lib.rs)" = "$original" ] || fail "$what: the file wasn't restored"
}

test_answer=(cargo test -q answer_is_right)
expect 0 RED "a break the test catches" src/lib.rs "    42" "    43" -- "${test_answer[@]}"
grep -q "the wrong answer" <<<"$out" || fail "didn't report why it went red: $out"
# The failure reported is the test's own, not a panic it caught on purpose first.
expect 0 RED "a break after a caught panic" src/lib.rs "    7" "    8" -- cargo test -q fails_after_a_caught_panic
grep -q "the real failure" <<<"$out" || fail "reported a caught panic instead of the failure: $out"
expect 1 STILL-GREEN "a break the test misses" src/lib.rs "    7" "    8" -- "${test_answer[@]}"
expect 1 NO-COMPILE "a break that doesn't compile" src/lib.rs "    42" "    nonsense" -- "${test_answer[@]}"
expect 2 ERROR "OLD that isn't there" src/lib.rs "    99" "    98" -- "${test_answer[@]}"
expect 2 ERROR "OLD that isn't unique" src/lib.rs "pub fn" "fn" -- "${test_answer[@]}"
expect 2 ERROR "a filter matching no test" src/lib.rs "    42" "    43" -- cargo test -q no_such_test
SEEN_RED_ALREADY_RED=1 expect 2 ERROR "a test red before the break" src/lib.rs "    42" "    43" -- cargo test -q already_red

# A hang is killed, reported and restored.
cat >>src/lib.rs <<'RUST'

#[test]
fn hangs_when_broken() {
    if unguarded() == 8 {
        std::thread::sleep(std::time::Duration::from_secs(600));
    }
}
RUST
original="$(cat src/lib.rs)"
expect 1 HUNG "a break that hangs" --timeout 20 src/lib.rs "    7" "    8" -- cargo test -q hangs_when_broken

# A list runs every break, and --only narrows it.
cat >breaks.json <<'JSON'
[
  {"name": "caught", "file": "src/lib.rs", "old": "    42", "new": "    43", "test": ["cargo", "test", "-q", "answer_is_right"]},
  {"name": "missed", "file": "src/lib.rs", "old": "    7", "new": "    9", "test": ["cargo", "test", "-q", "answer_is_right"]}
]
JSON
expect 1 RED "a list with one miss" --list breaks.json
grep -q "^STILL-GREEN *missed" <<<"$out" || fail "a list ran only part of itself: $out"
grep -q "^summary: 1 red, 1 still green, 0 unproven, 0 not tried$" <<<"$out" || fail "no summary: $out"
expect 0 RED "--only" --list breaks.json --only caught

# A break that can't be tried shows in the summary even when the lines above are filtered out.
cat >breaks.json <<'JSON'
[
  {"name": "caught", "file": "src/lib.rs", "old": "    42", "new": "    43", "test": ["cargo", "test", "-q", "answer_is_right"]},
  {"name": "missed", "file": "src/lib.rs", "old": "    7", "new": "    9", "test": ["cargo", "test", "-q", "answer_is_right"]},
  {"name": "stale", "file": "src/lib.rs", "old": "    99", "new": "    98", "test": ["cargo", "test", "-q", "answer_is_right"]}
]
JSON
expect 2 RED "a list with a stale break" --list breaks.json
tail -n 1 <<<"$out" | grep -q "^summary: 1 red, 1 still green, 0 unproven, 1 not tried$" || fail "summary isn't last or miscounts: $out"

[ "$failures" -eq 0 ] || { echo "$failures failure(s)" >&2; exit 1; }
echo "ok: seen-red.py passes only a break that goes red, and restores the file"
