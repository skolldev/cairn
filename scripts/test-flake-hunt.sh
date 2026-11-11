#!/usr/bin/env bash
# Shows that flake-hunt.py counts each failure against the git it ran on, groups failures by test
# and panic location, and survives hangs and leaked processes. Uses a small crate of its own whose
# tests fail, hang or leak on cue, and stands in for each built git, so it needs only cargo.
set -euo pipefail
tool="$(cd "$(dirname "$0")" && pwd)/flake-hunt.py"
failures=0
fail() { echo "FAIL: $*"; failures=$((failures + 1)); }

work="$(mktemp -d)"
trap 'rm -rf "${work:?}"' EXIT

# oldest-git.sh skips its build when ~/git-<version> already holds that version, built with curl.
for version in 9.9.8 9.9.9; do
  mkdir -p "$work/home/git-$version/bin" "$work/home/git-$version/libexec/git-core"
  printf '#!/bin/sh\necho "git version %s"\n' "$version" >"$work/home/git-$version/bin/git"
  : >"$work/home/git-$version/libexec/git-core/git-remote-http"
  chmod +x "$work/home/git-$version/bin/git" "$work/home/git-$version/libexec/git-core/git-remote-http"
done

crate="$work/crate"
mkdir -p "$crate/src" "$crate/tests"
cd "$crate"
cat >Cargo.toml <<'TOML'
[package]
name = "flake-hunt-fixture"
version = "0.0.0"
edition = "2021"

[workspace]
members = ["sub"]
TOML
mkdir -p sub/src sub/tests
cat >sub/Cargo.toml <<'TOML'
[package]
name = "flake-hunt-fixture-sub"
version = "0.0.0"
edition = "2021"
TOML
: >sub/src/lib.rs
cat >sub/tests/other.rs <<'RUST'
#[test]
fn other_steady() {
    assert!(std::env::var_os("STANDIN_OTHER_FAILS").is_none(), "told to fail");
}
RUST
echo target >.gitignore
# Without a default toolchain, as on a machine that only builds Cairn, Cairn's own, which rustup
# finds only from the crate's directory. And cargo's own homes, which HOME below would move.
rustup default >/dev/null 2>&1 || cp "$(dirname "$tool")/../rust-toolchain.toml" .
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
: >src/lib.rs
mkdir src/bin
echo 'fn main() {}' >src/bin/helper.rs
# Each test does what STANDIN_<NAME> asks: fail every second run, hang, leak a process, or take
# STANDIN_SLOW seconds. A run's count is kept in $STANDIN_DIR, and the git it saw is what
# `git --version` prints.
cat >tests/standin.rs <<'RUST'
use std::{env, fs, process::Command, thread, time::Duration};

fn count(name: &str) -> u32 {
    let file = format!("{}/{name}", env::var("STANDIN_DIR").unwrap());
    let n = fs::read_to_string(&file).map(|s| s.trim().parse().unwrap()).unwrap_or(0) + 1;
    fs::write(&file, n.to_string()).unwrap();
    n
}

#[test]
fn steady() {}

// As cairn-write's runner finds cairn-askpass: one directory above a test binary.
#[test]
fn finds_the_helper_above() {
    let exe = env::current_exe().unwrap();
    let helper = exe.parent().unwrap().parent().unwrap().join("helper");
    assert!(helper.exists(), "no helper at {}", helper.display());
}

#[test]
fn aborts() {
    if env::var_os("STANDIN_ABORT").is_some() {
        std::process::abort();
    }
}

#[test]
fn multiline() {
    if env::var_os("STANDIN_MULTILINE").is_some() {
        panic!("the first line\nthe second line");
    }
}

#[test]
fn flaky() {
    if env::var_os("STANDIN_FLAKY").is_none() {
        return;
    }
    let n = count("flaky");
    let git = Command::new("git").arg("--version").output().unwrap();
    let git = String::from_utf8(git.stdout).unwrap();
    let read = match n % 4 {
        2 => r#"{"refs"}"#,
        0 => r#"{"refs", "staged"}"#,
        _ => return,
    };
    panic!("the refresh never read every Slot: {read} ({})", git.trim());
}

#[test]
fn hangs() {
    if env::var_os("STANDIN_HANG").is_some() {
        thread::sleep(Duration::from_secs(600));
    }
}

#[test]
fn leaks() {
    if env::var_os("STANDIN_LEAK").is_some() {
        Command::new("sleep").arg("2999").spawn().unwrap();
    }
}

#[test]
fn overlaps() {
    if env::var_os("STANDIN_OVERLAP").is_none() {
        return;
    }
    let dir = env::var("STANDIN_DIR").unwrap();
    let mine = format!("{dir}/running-{}", std::process::id());
    fs::write(&mine, "").unwrap();
    thread::sleep(Duration::from_millis(1500));
    let running = fs::read_dir(&dir).unwrap().filter(|e| {
        e.as_ref().unwrap().file_name().to_str().unwrap().starts_with("running-")
    });
    if running.count() > 1 {
        fs::write(format!("{dir}/overlapped"), "").unwrap();
    }
    fs::remove_file(&mine).unwrap();
}

#[test]
fn slow() {
    if let Some(secs) = env::var_os("STANDIN_SLOW") {
        thread::sleep(Duration::from_secs_f64(secs.to_str().unwrap().parse().unwrap()));
    }
}
RUST
cargo generate-lockfile -q
# With no user config, so a developer's commit.gpgsign can't stop the commit.
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
git init -q
git add .
git -c user.name=t -c user.email=t@t commit -qm fixture
mkdir "$work/counts"

# hunt <expected exit status> <what it means> <flake-hunt.py options>
# Cargo arguments replace the default suite, which names Cairn's crates.
hunt() {
  local want="$1" what="$2"
  shift 2
  set +e
  out="$(HOME="$work/home" CAIRN_GIT_VERSIONS="9.9.8 9.9.9" STANDIN_DIR="$work/counts" \
    python3 "$tool" "$@" -- --workspace 2>&1)"
  status=$?
  set -e
  [ "$status" -eq "$want" ] || fail "$what: expected exit $want, got $status: $out"
}

# A sweep alternates the gits, and counts each failure against the git it ran on. Runs 2 and 4 fail,
# both on 9.9.9, at one place with two messages: one group.
STANDIN_FLAKY=1 hunt 1 "a sweep with a flaky test" --iterations 4
grep -q "^4 iterations (git 9.9.8: 2, git 9.9.9: 2)" <<<"$out" || fail "no iteration count: $out"
grep -qE "^FAILED standin flaky at tests/standin.rs:[0-9]+: 2 / 4 \(50.0%\); git 9.9.8: 0 / 2, git 9.9.9: 2 / 2$" <<<"$out" ||
  fail "no failure group with its rates: $out"
grep -qF '  1x the refresh never read every Slot: {"refs"} (git version 9.9.9)' <<<"$out" ||
  fail "lost the first message: $out"
grep -qF '  1x the refresh never read every Slot: {"refs", "staged"} (git version 9.9.9)' <<<"$out" ||
  fail "lost the second message: $out"
grep -qxF "  issue: gh issue list --label flaky --search flaky" <<<"$out" || fail "no issue to update: $out"
[ "$(grep -c '^FAILED' <<<"$out")" -eq 1 ] || fail "a passing test was reported: $out"
run_dir="$(sed -n 's/^report: //p' <<<"$out")"
[ -f "$run_dir/summary.txt" ] || fail "no summary in the run directory: $out"
[ "$(find "$run_dir/logs" -name '*.log' | wc -l | tr -d ' ')" -eq 2 ] || fail "expected a log per failed run: $out"
grep -q "commit $(git rev-parse --short HEAD)$" <<<"$out" || fail "no commit, or called a clean tree dirty: $out"

# A hung binary is killed when its time is up, named with the tests that hadn't finished, and the
# hunt carries on with the next binary and the next iteration.
STANDIN_HANG=1 hunt 1 "a hang" --iterations 2 --timeout 3
grep -q "^2 iterations" <<<"$out" || fail "a hang stopped the hunt: $out"
grep -qx "HUNG standin: 2 / 2 (100.0%); git 9.9.8: 1 / 1, git 9.9.9: 1 / 1" <<<"$out" || fail "no hang reported: $out"
grep -qx "  unfinished: hangs" <<<"$out" || fail "didn't name the hung test: $out"
grep -q "^HUNG sub/other" <<<"$out" && fail "blamed the next binary: $out"
pgrep -f "$work/crate/target/flake/.*/bin/" >/dev/null && fail "left a hung binary running"

# A process a test leaves running is reported against its binary, even one that passed, and killed.
STANDIN_LEAK=1 hunt 1 "a leak" --iterations 1
grep -qx "LEAKED standin: 1 / 1 (100.0%); git 9.9.8: 1 / 1, git 9.9.9: 0 / 0" <<<"$out" || fail "no leak reported: $out"
grep -qx "  sleep 2999" <<<"$out" || fail "didn't name the leaked process: $out"
pgrep -f "sleep 2999" >/dev/null && fail "left the leaked process running"
pkill -f "sleep 2999" || true

# A binary that passes in more than half its time is flagged, so a timeout about to bite shows
# before it does. Slow isn't failed.
STANDIN_SLOW=2 hunt 0 "a slow binary" --iterations 1 --timeout 3
grep -qE "^SLOW standin: 1 / 1 \(100.0%\); git 9.9.8: 1 / 1, git 9.9.9: 0 / 0; longest 2\.[0-9]s of 3s$" <<<"$out" ||
  fail "no slow binary reported: $out"
grep -q "^SLOW sub/other" <<<"$out" && fail "flagged a quick binary: $out"

# Targeting one test runs only the binaries that have it, and --until-fail stops at its first
# failure: the second run.
rm -f "$work/counts/flaky"
STANDIN_FLAKY=1 hunt 1 "--until-fail" --filter flaky --until-fail --iterations 10
grep -qx "hunting 1 test in 1 binary" <<<"$out" || fail "ran more than the target: $out"
grep -q "^2 iterations" <<<"$out" || fail "didn't stop at the first failure: $out"
grep -qx "after a fix, about 6 iterations in a row must pass to show it worked (95% confidence)" <<<"$out" ||
  fail "no runs needed for a fix: $out"
hunt 0 "a target that never fails" --filter steady --iterations 6
grep -qx "nothing failed in 6 iterations: its failure rate is likely under 50.0% (95% confidence)" <<<"$out" ||
  fail "no bound for a clean target: $out"
hunt 2 "a filter matching nothing" --filter no_such_test --iterations 1

# --jobs runs iterations side by side, and still counts every one against its git.
STANDIN_OVERLAP=1 hunt 0 "--jobs" --jobs 3 --iterations 6 --filter overlaps
grep -q "^6 iterations (git 9.9.8: 3, git 9.9.9: 3)" <<<"$out" || fail "--jobs lost count: $out"
[ -f "$work/counts/overlapped" ] || fail "--jobs ran iterations one at a time: $out"

# --for stops the hunt when its time is up.
hunt 0 "--for" --for 2s --filter steady
grep -qE "^[1-9][0-9]* iterations" <<<"$out" || fail "--for ran no iteration: $out"

# Ctrl-C stops the hunt midway, kills what's running and still writes the report. A shell starts a
# background job with SIGINT ignored, so this also shows the hunt doesn't rely on inheriting it.
HOME="$work/home" CAIRN_GIT_VERSIONS="9.9.8 9.9.9" STANDIN_DIR="$work/counts" STANDIN_HANG=1 \
  python3 "$tool" --filter hangs --timeout 60 -- --workspace >"$work/interrupted.out" 2>&1 &
pid=$!
for _ in $(seq 1 100); do
  grep -q "^hunting" "$work/interrupted.out" && break
  sleep 0.1
done
# Time for the binary to start, so Ctrl-C has something running to kill.
sleep 1
kill -INT "$pid"
# A hunt that ignores Ctrl-C would run for its whole hour.
for _ in $(seq 1 200); do
  kill -0 "$pid" 2>/dev/null || break
  sleep 0.1
done
if kill -0 "$pid" 2>/dev/null; then
  fail "Ctrl-C didn't stop the hunt"
  pkill -KILL -f "$work/crate/target/flake/.*/bin/" || true
  kill -KILL "$pid"
fi
set +e
wait "$pid"
set -e
out="$(cat "$work/interrupted.out")"
grep -qx "interrupted" <<<"$out" || fail "didn't say it was interrupted: $out"
grep -q "^0 iterations" <<<"$out" || fail "counted the iteration Ctrl-C stopped: $out"
run_dir="$(sed -n 's/^report: //p' <<<"$out")"
[ -n "$run_dir" ] && [ -f "$run_dir/summary.txt" ] || fail "no report after Ctrl-C: $out"
pgrep -f "$work/crate/target/flake/.*/bin/" >/dev/null && fail "left a binary running after Ctrl-C"

# A `cargo test` in the worktree in the middle of a hunt doesn't change the binaries it runs. The
# rebuilt `slow` fails; the hunted one doesn't. And a second hunt in the same worktree refuses to
# start, rather than rebuild them.
HOME="$work/home" CAIRN_GIT_VERSIONS="9.9.8 9.9.9" STANDIN_DIR="$work/counts" STANDIN_SLOW=1 \
  python3 "$tool" --filter slow --iterations 4 -- --workspace >"$work/rebuilt.out" 2>&1 &
pid=$!
for _ in $(seq 1 100); do
  grep -q "^hunting" "$work/rebuilt.out" && break
  sleep 0.1
done
hunt 2 "a second hunt" --iterations 1
grep -q "another hunt is running" <<<"$out" || fail "didn't say why a second hunt refused: $out"
sed -i.orig 's/^fn slow() {$/fn slow() { panic!("rebuilt");/' tests/standin.rs
cargo test -q --no-run 2>/dev/null
set +e
wait "$pid"
status=$?
set -e
# Seen red: building into the worktree's target directory turns this red.
[ "$status" -eq 0 ] || fail "a rebuild changed the hunted binaries: $(cat "$work/rebuilt.out")"
# Started on a changed tree, it says so.
hunt 0 "a changed tree" --filter steady --iterations 1
# Seen red: never noting the changes turns this red.
grep -q "commit $(git rev-parse --short HEAD) (uncommitted changes)$" <<<"$out" || fail "didn't flag the changed tree: $out"
mv tests/standin.rs.orig tests/standin.rs

# A message of several lines shows on one, as an assert_eq!'s values must to tell its failures apart.
STANDIN_MULTILINE=1 hunt 1 "a multi-line message" --filter multiline --iterations 1
grep -qx "  1x the first line | the second line" <<<"$out" || fail "didn't join the message's lines: $out"

# A binary in a crate of its own is named by its crate's directory.
STANDIN_OTHER_FAILS=1 hunt 1 "a crate in a directory" --filter other_steady --iterations 1
grep -q "^FAILED sub/other other_steady at sub/tests/other.rs:" <<<"$out" || fail "didn't name the crate: $out"
grep -q "^  log: .*sub-other.log$" <<<"$out" || fail "no log for a crate's binary: $out"

# A binary that dies without a test failing, as an abort does, is reported as crashed.
STANDIN_ABORT=1 hunt 1 "an abort" --filter aborts --iterations 1
grep -qE "^CRASHED standin, exit -?[0-9]+: 1 / 1 \(100.0%\)" <<<"$out" || fail "no crash reported: $out"

[ "$failures" -eq 0 ] || { echo "$failures failure(s)" >&2; exit 1; }
echo "ok: flake-hunt.py counts, groups and survives what flaky tests do"
