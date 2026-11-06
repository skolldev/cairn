# GitKraken baseline: macOS half of issue 7

The Windows half is in `RESULTS-windows.md`. These are stopwatch-style reference numbers for GitKraken, not Cairn results.

## Setup

- GitKraken 12.6.2, native arm64, signed in, with `commits.maxCommitsInGraph` at 2000
- MacBook Pro, Apple M3 Max, 36 GB, macOS 27.0.1, on AC power
- Built-in display at 120 Hz. GitKraken was maximized on it.
- Fixtures from issue 5: `linux-graph`, `linux-nograph`, `ripgrep`
- Taken 2026-11-06. The user's own repo tabs were open in other tabs.

## Results

Medians unless noted.

| Interaction | Budget | GitKraken | Samples | Fixture |
| --- | --- | --- | --- | --- |
| Launch to first window (splash) | < 150 ms | 800 ms (768–823) | 8 | `ripgrep` tab |
| Launch to main window | none | 3.57 s (3.50–4.47) | 8 | `ripgrep` tab |
| Open repo, warm, usable | < 200 ms | 1.24 s (1.12–1.30) | 10 | `linux-graph` |
| Open repo, cold, first commits | < 1 s | 2.73 s | 5 | `linux-graph` |
| Open repo, cold, settled | none | 4.51 s | 5 | `linux-graph` |
| Open repo, cold, no commit-graph, first commits | < 1 s | 12.0 s | **1** | `linux-nograph` |
| Graph scrolling | no frame over 8 ms | 39% of gaps over 8.4 ms, p99 25 ms, worst 58 ms | 2,047 gaps | `linux-graph` |
| Select commit to file list, first visit | < 50 ms | 62 ms first change, 109 ms settled | 21 | `linux-graph` |
| Select commit to file list, revisit | < 50 ms | 70 ms | 21 | `linux-graph` |
| Select file to first diff | < 50 ms | 87 ms first paint, 254 ms settled, p95 717 ms | 21 | `linux-graph` |
| Select file to first diff | < 50 ms | 87 ms first paint, 191 ms settled, p95 670 ms | 21 | `ripgrep` |
| Worktree save to updated status | < 150 ms | 502 ms (476–532) | 12 | `linux-graph` |
| External git command to UI updated | < 100 ms | 490 ms (458–513) | 20 | `ripgrep` |
| Memory with graph loaded | < 300 MB | 1.24 GB total, largest process 823 MB | 1 | `linux-graph` |

## Method

`probe.swift` posts a real input event, records the screen with ScreenCaptureKit, and timestamps each frame on the host clock. A sample is the time from the input to the first changed frame in a chosen region, and to the last one. `run.py` drives the interactions and appends raw samples to `results/raw.jsonl`. `summarize.py` reduces them.

To rerun, build the probe with `swiftc probe.swift -o probe`. It needs Screen Recording and Accessibility permission. Click positions and regions in `run.py` are for this display and window size.

- **Commit:** click a row, watch the file list region.
- **File:** click the first file row, watch the diff region.
- **Status:** append a byte to `README`, watch the file-count banner, then revert it.
- **Git to UI:** run `git tag` from outside, watch the tags count.
- **Warm open:** close the tab, then open a `gitkraken://repo` URL.
- **Cold open:** quit GitKraken, run `sudo purge`, relaunch, then open the repo. GitKraken's own caches were left alone.
- **Scrolling:** post wheel events in 12 px steps every 4 ms, then read gaps between presented frames.
- **Memory:** sum `phys_footprint` from `footprint` across GitKraken's processes.
- **Launch:** `open -a`, with the first window confirmed by timed screenshots.

## Limits on these numbers

- **Click floor:** a window that repaints inside `mouseDown` measured a median of about 19 ms, so click rows include that.
- **Scroll floor:** the recorder misses frames by itself. A window repainting every refresh showed 11.5% of gaps over 8.4 ms and a worst gap of 25 ms (`results/control.jsonl`), so 39% overstates GitKraken's misses. The display's 8.33 ms frame also means a strict 8 ms budget can't be proven at 120 Hz.
- **Commit limit:** GitKraken loads 2000 of the 1.5M commits, so open, scroll and memory are not like for like with the budgets.
- **Memory** is one sample, with the user's other repo tabs open, so it's an upper bound.
- **Git to UI** measures how fast GitKraken notices an external command. It doesn't cover its own buttons.
- **No-graph cold open** has one sample, because a no-graph state doesn't survive an open (below).
- **Launch** has the tab set above restored, and the first change at about 215 ms is ignored: no GitKraken window was on screen then.
- **Warm open:** a dimmed graph shows behind "Opening repo" from about 0.5 s. The 1.24 s figure is when that clears.

## What GitKraken does to fixtures

Anything measuring these fixtures needs to know this, on both OSes.

- **Auto-fetch:** it fetched `torvalds/linux` into `linux-graph` within minutes of opening, moving `origin/master` off the pin. Setting `url.<local>.insteadOf` on each fixture stops that.
- **Pruning:** its fetch prunes. Redirecting `ripgrep` to itself deleted its remote branches, so that redirect has to point at a mirror with every branch (`ripgrep-mirror.git`). The kernels have only `master`, so a self-redirect is safe for them.
- **Commit-graph:** it writes a commit-graph chain and a `multi-pack-index` itself. `linux-nograph` gained one about a minute after opening, and during the first cold open on the second try. The check in issue 5 only covers `info/commit-graph`, so it misses the chain.

## Status update quirk

Opening a repo URL on a tab that's already open toggles status updates. They worked, then stopped, then worked on each repeat open:

| Repeat open | Save, revert |
| --- | --- |
| Before | 499 ms, 498 ms |
| 1 | no update, no update |
| 2 | 868 ms, 518 ms |
| 3 | no update, no update |

The status row comes from a freshly opened tab, with 24 of 24 samples registering. Samples set aside in `results/status-no-git-activity.jsonl` are the failed state, not slow updates.

## Set aside

- `results/smoke-*.jsonl`: fixture repair and locator bug
- `results/invalid-wrong-region.jsonl`: watched the wrong screen region
- `results/invalid-cold-nograph-short-quiet.jsonl`: capture ended before the graph appeared
- `linux-nograph` commit, file and scroll samples in `raw.jsonl`: a commit-graph appeared at the start of them, so they aren't no-graph numbers
- One cold `linux-nograph` sample at 1.7 s in `raw.jsonl`: the graph already existed by then

## Fixture state after this run

- `linux-graph`: pin correct, split commit-graph, `multi-pack-index`, and a stray 7 MB unreachable pack from the fetch
- `linux-nograph`: pin correct, but has GitKraken's commit-graph chain and `multi-pack-index` again
- `ripgrep`: original refs restored, plus a commit-graph chain and `multi-pack-index`; fetches from `~/cairn-fixtures/ripgrep-mirror.git`
