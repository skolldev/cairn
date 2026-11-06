# GitKraken baseline: Windows half of issue 7

GitKraken misses every budget on Windows that it can be compared on, with one exception: revisiting a commit, at 34 ms settled against 50 ms. Selecting a file comes close, with first paint at 52 ms, but it takes 244 ms to settle.

These are stopwatch-style reference numbers for GitKraken, not Cairn results. The macOS half is in `RESULTS.md`.

## Setup

- GitKraken 12.6.2, signed in, with `commits.maxCommitsInGraph` unset, so at its default.
- Ryzen 7 7800X3D, 64 GB, Radeon RX 7900 XT, Windows 11 build 26200, High performance power plan
- Fixtures on `C:`, a SATA SSD, under `C:\Users\guita\cairn-fixtures`
- Primary display 3840×2160 at 150%, set to 120 Hz for the run. GitKraken was maximized on it. The two side displays stayed on at 60 Hz.
- Defender real-time protection on, in its normal state
- Taken 2026-11-06, with the user's 15 repo tabs open in other tabs

## Results

Medians unless noted. All times are in ms unless marked as seconds.

| Interaction | Budget | Windows | macOS | Samples | Fixture |
| --- | --- | --- | --- | --- | --- |
| Launch to first window (splash) | < 150 ms | 804 (738–820) | 800 | 8 | `ripgrep` tab |
| Launch to main window | none | 3.77 s (3.72–3.81) | 3.57 s | 8 | `ripgrep` tab |
| Open repo, warm, usable | < 200 ms | 3.00 s (2.89–3.11) | 1.24 s | 10 | `linux-graph` |
| Open repo, cold, first commits | < 1 s | 6.13 s (6.10–6.39) | 2.73 s | 5 | `linux-graph` |
| Open repo, cold, settled | none | 6.59 s (6.56–6.86) | 4.51 s | 5 | `linux-graph` |
| Open repo, cold, no commit-graph, first commits | < 1 s | 19.3 s (19.2–19.4) | 12.0 s, 1 sample | 5 | `linux-nograph` |
| Graph scrolling | no frame over 8 ms | 36 fps. Median 25 ms, p99 58 ms, worst 117 ms (PresentMon) | 39% of gaps over 8.4 ms, worst 58 ms | 681 frames | `linux-graph` |
| Select commit to file list, first visit | < 50 ms | 30 first change, 93 settled | 62, 109 | 21 | `linux-graph` |
| Select commit to file list, revisit | < 50 ms | 33 first change, 34 settled | 70 | 21 | `linux-graph` |
| Select file to first diff | < 50 ms | 52 first paint, 244 settled, p95 302 | 87, 254, 717 | 21 | `linux-graph` |
| Select file to first diff | < 50 ms | 53 first paint, 224 settled, p95 264 | 87, 191, 670 | 21 | `ripgrep` |
| Worktree save to updated status | < 150 ms | 255 (226–280) | 502 | 12 | `linux-graph` |
| External git command to UI updated, tag created | < 100 ms | 2.02 s (245 ms–2.10 s) | 490, pooled | 10 | `ripgrep` |
| External git command to UI updated, tag deleted | < 100 ms | 538 (502–1,798) | 490, pooled | 10 | `ripgrep` |
| Memory with graph loaded | < 300 MB | 1,046 MiB private working set, largest process 530 MiB, plus 297 MiB dedicated GPU memory | 1.24 GB | 1 | `linux-graph` |

This isn't an OS comparison. The machines differ, and the Windows fixtures sit on SATA while the Mac's are on internal NVMe, which is likely to affect the open rows.

## Method

`windows/src/main.rs` is the counterpart of `probe.swift`. It posts real input with `SendInput`, records the screen with DXGI desktop duplication, and timestamps frames with their QPC present time. `windows/run.py` drives the same steps and row numbers as `run.py`. `windows/summarize.py` reduces `results/windows/raw.jsonl` to `results/windows/summary.txt`.

To rerun, build with `cargo build --release` and start `windows/helper.ps1` elevated and hidden. An elevated window with focus blocks all synthetic input. `windows/bin/PresentMon.exe` is PresentMon 2.6.0 and isn't committed. Coordinates in `run.py` are for this display and scale.

- **Commit, file, status, git to UI, warm and cold open, launch:** as on macOS. `exec:` actions start the clock before `cmd.exe` runs, and status writes in-process.
- **Warm usable:** the last change, when the dimmed graph clears. The dimmed graph first shows at 1.55 s. Timed screenshots confirm both.
- **Cold open:** quit, purge, relaunch, wait 9 s, open the repo. The loader appears at about 1.6 s and doesn't count as first commits.
- **Cache purge:** `probe purge` flushes the modified list and purges standby. Reading 400 MB of a pack took 75 ms before it and 966 ms after it. GitKraken's own caches were left alone.
- **No-graph cold open:** the fixture had no commit-graph, chain or `multi-pack-index` before each sample, and still had none afterwards.
- **Scrolling:** wheel events of 14 units every 4 ms, which moved the graph 11.7 CSS px per event, against 12 px on macOS. Ten runs of 2 s, five each way, with PresentMon and desktop duplication recording together.
- **Memory:** `WorkingSetPrivate` summed across the 7 GitKraken processes, as the Floors specify, plus `DedicatedUsage`.
- **Launch:** the root `gitkraken.exe` stub, so the row includes it. GitKraken's own log puts the splash at 328 ms from when Node starts.

## Limits on these numbers

- **The 120 Hz control failed.** A window repainting every refresh averaged 60 fps with the primary at 120 Hz, repeating 8, 8, 8, 33, 25 ms. It stayed near 60 fps at 240 Hz and with duplication pointed at another display. The cause is unknown. The side displays at 60 Hz are unconfirmed as a cause, because they were never turned off, and moving GitKraken between displays didn't change the average. So this run can't show what share of GitKraken's frames would pass a strict budget, and the Windows scroll row can't be compared directly with the Mac's.
- **The scroll result still stands as a miss.** GitKraken's 36 fps is well under the 60 fps the control reached, and PresentMon and desktop duplication agree on it, at a 25.0 and 25.7 ms median.
- **PresentMon has no control baseline.** On the three control runs in `raw.jsonl` it logged only 2 to 8 frames from the control window, while earlier ad-hoc runs logged about 240 to 380. That isn't explained. On GitKraken it logged 681 frames against desktop duplication's 715.
- **Click floor:** a window repainting inside `WM_LBUTTONDOWN` measured a median of 6.7 ms (1.5–14.1), so click rows include that. Taken off, file first paint would sit just inside budget.
- **Commit limit:** GitKraken loads 2000 of the 1.5M commits, as on macOS.
- **Memory** is one sample with the user's other tabs open, so it's an upper bound. Private working set and `phys_footprint` are different counters.
- **Git to UI** is two clusters, not one number, so it isn't pooled. Creating a tag usually takes about 2 s, and deleting one about 540 ms. One sample of each fell outside its cluster, at 245 ms for a create and 1.8 s for a delete. The reason wasn't investigated.
- **Cold opens** ran on a relaunched app, with the file cache purged but GitKraken's own caches intact.

## What GitKraken does here

- **Window position:** moving the window and quitting left `windowSettings` in `%APPDATA%\.gitkraken\config` unchanged, so it reopened on the old display. Pointing that at the primary was needed for launch and cold rows, and it has been put back.
- **Fixtures:** with fetches redirected, pins held on every check. It wrote a `multi-pack-index` to `linux-graph` and `ripgrep`, and a commit-graph chain to `ripgrep`. `linux-nograph` stayed clean, unlike on the Mac, probably because its tab was never open for more than about 40 s.

## Set aside

- `results/windows/invalid-cold-graph-short-quiet.jsonl`: five cold `linux-graph` samples whose capture ended before the graph appeared. The 4 s quiet window the Mac run used is too short here.

## Fixture state after this run

- `linux-graph`: pin correct, single `commit-graph`, `multi-pack-index`, fetches redirected to itself
- `linux-nograph`: pin correct, no commit-graph or `multi-pack-index`, fetches redirected to itself
- `ripgrep`: pin correct, refs unchanged, commit-graph chain and `multi-pack-index`, fetches from `C:\Users\guita\cairn-fixtures\ripgrep-mirror.git`
