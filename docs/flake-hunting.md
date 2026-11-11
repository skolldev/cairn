# Hunting flaky tests

A flaky test fails sometimes, on code nobody changed. CI runners are slower and busier than a developer's Mac, so most flakes, such as #59 and #102, show up there first. `scripts/flake-hunt.py` reproduces them locally. It runs the suite again and again, unattended, and reports every test that ever failed, hung or left a process running, with how often it happened on each git.

macOS and Linux only.

## Sweeping the suite

```bash
scripts/flake-hunt.py --for 8h
```

Run it from the repository root. Each iteration runs what CI's default suite runs: the workspace's tests, then `cairn-app`'s with its `measure` feature. Cargo arguments after `--` replace that, as with `scripts/git-matrix.sh`:

```bash
scripts/flake-hunt.py --for 2h -- -p cairn-repo
```

- **Duration.** `--for` takes `90s`, `30m`, `8h` or `1h30m`, and defaults to an hour. `--iterations N` stops after N iterations instead. Ctrl-C stops early and still writes the report. The iteration it interrupts isn't counted. The machine is kept awake while the hunt runs, by `caffeinate` on macOS and by `systemd-inhibit` on Linux if it's installed.
- **Gits.** Iterations alternate the oldest supported git and the newest released one, as CI's oldest-git job and its macOS runner do. Each is built by `scripts/oldest-git.sh` the first time. `CAIRN_GIT_VERSIONS` overrides the list.
- **Load.** `--jobs N` runs N iterations side by side, which is closer to a busy CI runner. Try 4 to 8 for a flake that never fails unloaded.
- **Hangs.** A test binary gets 120 seconds, or `--timeout`. The slowest one takes about 25 seconds unloaded, and the tests' own waits give up at 30. A binary over its time is killed with every process it started, reported as hung with the tests that hadn't finished, and the hunt carries on.
- **Leaks.** A process a test binary leaves running, such as a hook or `slow-remote.sh`, is reported against that binary and killed, so it can't load later iterations.

## Targeting one flake

```bash
scripts/flake-hunt.py --filter a_recheck_leaves_alone_the_slots_that_answered --jobs 6 --until-fail
```

`--filter` works as a libtest filter: it runs only the tests whose names contain it, and only the binaries that have one. `--until-fail` stops at the first failure and keeps its log.

After a fix, run the same target without `--until-fail` for as long as the report asks. A flake that failed 1 in 50 iterations needs about 150 clean ones before the fix is likely to have worked.

## Reading the report

The report is printed at the end and saved in `target/flake/<timestamp>/summary.txt`, beside a log of each failed run in `logs/`.

```
40 iterations (git 2.37.0: 20, git 2.56.1: 20)
commit 7f81f07
1843s
FAILED repo/recheck a_recheck_leaves_alone_the_slots_that_answered at crates/repo/tests/recheck.rs:180: 3 / 40 (7.5%); git 2.37.0: 2 / 20, git 2.56.1: 1 / 20
  issue: gh issue list --label flaky --search a_recheck_leaves_alone_the_slots_that_answered
  2x the switch's refresh never read every Slot: {"refs"}
  1x the switch's refresh never read every Slot: {"refs", "staged", "untracked"}
  log: target/flake/20261111-210000/logs/0007-git-2.37.0-repo-recheck.log
```

- **FAILED** groups failures by test and by the place they panicked, so one assertion that fails with different messages is one group. Under it come the distinct messages with their counts, each by its first line, a search for the `flaky` issue to update, and the logs. The hunt never posts anything.
- A binary is named by its crate's directory and its target, with `(measure)` for `cairn-app`'s second build.
- **HUNG**, **CRASHED** (a binary that died without a test failing, as an abort does) and **LEAKED** count a binary's runs the same way, and list the unfinished tests or the processes left behind.
- **SLOW** marks a binary that passed but took more than half its timeout. It isn't a failure, but a timeout that's about to start biting.
- The rates count runs of the test's binary, on each git. In target mode the report ends with how far the result can be trusted. With no failure in n iterations, a failure rate above 3/n would have shown 95% of the time.
- `commit` notes uncommitted changes when the tree had any.

The exit status is 0 if nothing failed, hung or leaked, 1 if something did, and 2 if the hunt couldn't start or broke partway.

## Running alongside other work

The hunt builds once into `target/flake/build`, its own target directory, and runs the test binaries there. Its first build is a full one. A `cargo test` in the same worktree builds into `target/`, so it can't change what's being hunted. The binaries aren't copied anywhere, because tests compare paths against the `CARGO_BIN_EXE_*` paths compiled into them, and `cairn-write`'s runner finds `cairn-askpass` beside the test binary. A second hunt in the same worktree would rebuild them, so it refuses to start while the first one runs.

The hunt does load the machine, so timings measured meanwhile, such as a Budget run, mean nothing.
