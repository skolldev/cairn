# The Budget harness

Measures the Phase 1 Budgets on real Cairn code, by the Budget rules in the [plan](../Git%20Client%20(GPUI%20+%20gix)%20Project%20Plan.md#performance-budgets), so that the Phase 1 gate is passed on numbers. The plan's table holds the Budgets themselves. A test keeps the harness's copy of each number in step with it.

**Numbers count only from macOS and the physical Windows PC, never from a VM or a CI runner.** The command refuses to run where `CI` is set. CI benches catch regressions and prove no Budget.

## Running it

```bash
scripts/fixtures.sh check
```

```bash
scripts/budget.sh run
```

`--only launch,write,external` picks Budgets. `--runs` and `--samples` allow a quick try, whose result can only come out Invalid below 3 runs or 20 samples. Each run writes a Markdown file to `budget-results/` (git-ignored) and exits non-zero unless everything measured passed. The record of a phase gate is that file's content, pasted into the gate's issue.

Keep the machine on mains power and idle, the screen unlocked and Cairn's window in front. GPUI throttles inactive windows to 30 Hz, and macOS stops presenting a covered one, so a run stops if the window is inactive during a kept sample. Launch has no such check, because a window can paint before it becomes active. A window that stays active but is covered isn't detected, so don't use the machine during a run.

## What each result records

The machine, OS, git version and path, Rust version, Cairn's commit (and whether it had uncommitted changes), the frame source, the fixture and its HEAD, the Budget's statistic, the p95 of the worst run, the worst sample, and every sample. A fact the harness can't find stops the run. Cairn's commit and the Rust version are read when the harness runs, so they describe the binary only if it was just built, as `scripts/budget.sh` does.

## What is measured

| Budget | Clock starts | Clock stops | Fixture |
| --- | --- | --- | --- |
| External change, file saved | just before the harness writes the file | first presented frame listing it in Unstaged, or no longer listing it | Linux kernel (`linux-graph`) |
| External change, `git switch` | when the `git switch` the harness ran has exited | first presented frame with the new HEAD, the flipped Untracked list and a newer answer from every status Slot | Linux kernel (`linux-graph`) |
| Intent to UI, Local write Job | where a double-click on a branch's row starts the switch, just before the sidebar asks for `Intent::Switch` | as for `git switch` | small (`ripgrep`) |
| Launch | just before the harness starts the process | the process's first presented frame | small (`ripgrep`), opened from the command line |

- **Statistic:** latency passes at p95 by nearest rank, over at least 20 samples per run, worst of three runs, with the worst sample recorded. Launch passes on its row's own statistic, median and none over, in each run. Its p95 is recorded too.
- **Warm cache:** each run throws away three samples first, and each Launch run one launch: warm means the binary has run since boot. Opens with a cold file cache are recorded only, and this doesn't measure them.
- **Real Cairn:** a release build of the real binary, with the `measure` feature, which `scripts/budget.sh` turns on and no shipped build has. A debug build is refused. Each run is a new process.
- **The switch:** the two branches differ in one tracked file, `budget-dir/tracked.txt`, beside an untracked `budget-dir/keep` that the sandbox keeps on both. Untracked lists `budget-dir/` on one branch and `budget-dir/keep` on the other, so the switch flips it through the index alone, and `.gitignore` is the same on both branches. A `.gitignore` change would make Untracked read everything, which is not what a switch costs (#54).
- **Fixtures:** samples run in a throwaway clone, so the fixtures stay untouched. A Budget's named fixture is checked, and never replaced by another: the run stops if it's missing, or if the kernel has no commit-graph.
- **git:** an empty user and system config, for the harness and Cairn, as in the tests, so that a machine's `core.fsmonitor` can't change a number.

## How a frame is timed

The `measure` build's probe writes a line for each painted frame with what the views hold and the time, on one clock shared by the whole machine. The harness finds the first frame showing the expected state, then asks the OS when that frame reached the screen. Paint timers read healthy while a screen is frozen (zed#61469), so a paint time never ends a sample.

- **macOS: Metal's presented times.** GPUI doesn't expose them, so the probe hooks the two getters that hand it drawables, as the #9 spike did.
- **Windows: PresentMon**, tracing the process and `dwm.exe` from before the process starts, so that Launch needs no in-process stand-in. The frame is the first `Present()` after the paint, and it reaches the screen in DWM's next flip, as [below](#how-a-windows-frame-gets-its-screen-time).

### Limits to read the numbers with

- **On macOS, the measured window draws all the time.** In an idle window, and for the first few frames after one, Metal reports a presented time of 0 even for a window on screen, so there is nothing to time. The harness keeps the window drawing. A real window that has been idle also has to wake its display link, up to about one refresh, and this doesn't include it. On Windows the window stays idle, as a real one does.
- **Launch on macOS is an upper bound,** for the same reason: the first frames report 0, so the first frame with a time can be some frames after the first one shown. It is the same method as the 239 ms in the plan.
- **The Intent starts where a double-click on the branch's row starts it** (`RefsView::switch_to`), so the harness measures the same path as the user's double-click, but not the input latency before it. Cairn has no Commit yet, so the Local write Job is a switch.
- **External `git switch` starts when git exits,** as the plan's row says, because git's own time on the kernel isn't Cairn's to spend. A frame can reach the screen before that, which counts as 0 ms.
- **There's no graph yet.** The External change row names the graph too. Nothing shows one until Phase 2, and when it does the expected state has to include it.

## Where External change's time goes

```bash
cargo run --release -p cairn-budget -- trace --only external
```

`run` says how long an External change takes. `trace` says where that time goes, Slot by Slot, as #40 had to find out by hand. It clones `linux-graph` into a sandbox and runs both External change rows' samples, with their warm-up, against the real Repo service with no window. For each sample it prints, in ms from the clock start (before the write, or once git has exited), every `read_job` span that closed after the change began and had published its Snapshot, or ended, by the time the expected Snapshots were drained: its Slot, the Refresh that asked for it, whether it was pending behind a running read, its lane wait, how long it ran, and whether it read in full or only the paths the watcher saw. A read can start before the clock does, while `git switch` is still running, and then reads as a negative time. `--samples N` sets how many samples each row keeps.

**It explains a number and never gives one.** Without a window nothing is presented, so a sample ends when the Snapshots are drained, not when a frame reaches the screen, and presenting isn't in it. It writes no result file and gives no verdict, and the Local write Job isn't traced. Like `run`, it refuses where `CI` is set and takes only a release build. It runs anywhere else, because nothing it prints counts toward a Budget.

## Where Launch's time goes

```bash
cargo build --release --locked -p cairn-app --features measure
```

```bash
cargo run --release -p cairn-budget -- trace --only launch
```

`trace --only launch` breaks a warm Launch on macOS into phases, in ms from the clock start, just before the process starts (#83). It makes the same launches as `run`, one thrown away and then `--samples N` kept, against the real binary on `ripgrep`, and prints each phase's median, min and max, then every launch:

- `main` running, GPUI's app launched, the window created, and the Repo service started with the Open sent: marks the probe writes, in a `measure` build only.
- The first painted frame, as the probe stamps it.
- Each Slot's first Snapshot, when the Repo service published it (from its `read_job` and `publish` spans) and when the window drained it. So a `measure` build with the probe on records those two spans in every run, `run` included, through a filter of its own that leaves Cairn's log as `RUST_LOG` sets it.
- The first frame painted with every Slot, the frame holding the first drawable Metal gave a presented time to, and that presented time, which is what `run` times.
- How many frames painted before that frame, and how many drawables Metal gave no time to before it.

Like External change's trace, it explains a number and never gives one: no result file, no verdict, and the Budget, its statistic and which frame ends a sample stay `run`'s. It reads Metal's presented times, so it runs on macOS only, and it needs a binary built with `measure`, as above.

## Windows

1. Set the primary display to 120 Hz.
2. Get the PresentMon 2.6.0 console exe, and set `CAIRN_PRESENTMON` to it or pass `--presentmon`. Other versions name their CSV columns differently, and a missing column stops the run. Admin isn't needed if your account is in Performance Log Users.
3. Run the commands above from Git Bash.

`scripts/budget.sh run` then records a PresentMon trace around each run, Launch included.

### The held-key test

```bash
scripts/budget.sh held-key
```

This checks zed#61469: sustained input plus background notifies can stop frames being presented for seconds. The Phase 0 runs injected their keys, so this one needs you: Windows auto-repeats only a key really held down. For each of three runs, click the Cairn window, hold an arrow key when told, and let go when told.

While you hold, the harness keeps changing a file every 8 ms, so real notifies come through the watcher and the Bridge, which delivers one coalesced drain per frame. That's the rate that decides #8's Floor. The worst-case rate, which #8 records only, would have to bypass the Bridge, so this doesn't measure it. The window draws all the time. It passes on #8's Key input Floor, because the plan's table has no row for this: no gap over 50 ms between presented frames during a 10-second hold, worst of three runs. A frozen stretch at the end of the hold counts. Arrow keys type nothing, so the Floor's other half, no lost or duplicated characters, isn't checked here, and that needs the commit box. A run with under 30 keys flagged as repeats by the OS, or with no notifies, comes out Invalid. So does a run that would otherwise pass but whose key went over a second without a repeat, because it was let go before the hold ended. A frozen window gets no keys either, so a gap over the Floor is still a Miss.

If it fails, run without changing anything first and record that, then see #13 on the watchdog.

It also runs on macOS, which is not the gate.

### What a real trace showed

Checked on the Windows PC with PresentMon 2.6.0, whose default CSV differs from its README's column table:

- Present time is `TimeInQPC`, which equals `CPUStartQPC` + `MsCPUBusy` in every row, and each Present follows its paint. Screen time is `TimeInQPC` + `MsUntilDisplayed`. `DisplayLatency` exists only under `--v2_metrics`. A missing column stops the run rather than reading as no frames.
- `--process_name` picks up a process that starts after the session does, and `--terminate_existing_session` ends the running PresentMon so that it writes its CSV.

### How a Windows frame gets its screen time

PresentMon reports almost no frame of GPUI's default swapchain as displayed. GPUI draws through DirectComposition, and every frame reads `NA`, even in a foreground window drawing without stopping (0 of 826). So a frame gets the screen time of **DWM's first flip on its display presented at or after the frame's `Present()`**. Where PresentMon has its own time for a frame, that wins.

- **Scored against PresentMon's own times.** With `GPUI_DISABLE_DIRECT_COMPOSITION=1`, the same binary draws through an HWND swapchain, whose frames PresentMon does time (825 of 826). There the rule was within 0.5 ms for 786 of 849 frames while animating and 33 of 34 while idle. It never read early, and read at most 3.63 ms late. Matching on DWM's CPU start instead read a whole frame late, every time.
- **Compared end to end.** One run of 8 samples each way, as medians on the default path and then on HWND: Launch 129 and 130 ms, Local write Job 77 and 80 ms, `git switch` 573 and 585 ms. Setting that variable stays a check to repeat by hand, not something a harness run does.
- **DWM has one chain per display.** The display is the chain that flips soonest after Cairn's `Present()`s across the session: the wrong one read about 2,100 ms where the right one read 20 ms. Another display that flips busily during a run, such as one playing a video, could be picked instead, which is one more reason not to use the machine during a run.

### Limits of a Windows number

- **A frame DWM dropped isn't known.** DWM's flip says the desktop changed, not that it held this frame. The check above is what supports the rule.
- **Launch's first frame is worked out, not reported.** PresentMon writes no row for a swapchain's first `Present()`, so its time is the first row's minus `MsBetweenPresents`, and no own time exists to score that against. GPUI shows the window before its first paint, so the frame has a window to appear in. Without this, Launch read about 70 ms later, because it timed a later frame.
- **A run's last frame has no row either,** because PresentMon writes a row only once the next `Present()` arrives. A sample ending on it has no time, which stops the run rather than giving a number.

## Recommended, not gating: Instruments on macOS scrolling

#13 judged macOS scrolling on Metal's presented times, and its control never cleared the Floor, so nothing independent has confirmed them. An Instruments run can. It's recommended, and no gate depends on it.

Cairn has no graph to scroll until Phase 2, so use the #9 prototype on `prototype/graph-scroll` until then. With the window at 120 Hz, record the process while it scrolls, in the Metal System Trace template:

```bash
xcrun xctrace record --template 'Metal System Trace' --attach cairn --time-limit 15s --output scroll.trace
```

Open the trace in Instruments and compare the display intervals with the harness's presented times for the same scenario. Record any disagreement in the issue for the gate.
