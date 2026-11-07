# PROTOTYPE: graph scroll spike (issue #9)

Throwaway. Not part of Cairn. Answers one question: can GPUI scroll a 1.5M-row
list with painted graph lanes at the Floors in #8, on macOS and Windows?

Stack: `gpui-pre =0.3.11`, Rust 1.98.1, `uniform_list` plus `canvas`. No gix, no
redb, no gpui-kit. Built on a pure GPUI window, so it measures framework cost only.

## Run

From this folder, in release mode. Unlock the screen and keep the window in
front. GPUI throttles inactive windows to 30 Hz, and macOS stops the display link
for a locked or covered one, so a run that loses focus is marked INVALID.

```bash
cargo run --release -- bench --format rows
```

Other commands:

```bash
cargo run --release -- bench --format delta
```

```bash
cargo run --release -- launch
```

```bash
cargo run --release -- empty
```

```bash
cargo run --release
```

The last one is a free window to scroll by hand.

Flags: `--cap 32` lanes, `--rows 1500000`, `--runs 3`, `--seconds 8`,
`--only <name>`, `--paint edge`, `--window 1280x800`, `--settle 10`.

Set `CAIRN_IDLE_SETTLE=1` for memory runs. It stops driving frames once the
scenarios finish, reads memory after `--settle`, prints `footprint` and exits.
Without it the settle phase keeps rendering, and the reading includes a transient
Metal driver pool of about 194 MB. Scroll figures are the same either way, but the
summary table isn't printed.

## Windows

1. Set the primary display to 120 Hz.
2. Get the PresentMon console exe. Admin isn't needed if your account is in
   Performance Log Users, because PresentMon then traces its own user's processes.
3. Set `CAIRN_PRESENTMON` to the PresentMon exe, then run the commands above.

Windows launch numbers are an in-process stand-in, not presented frames. Memory
is private working set. GPUI's graphics memory sits in VRAM, which that counter
doesn't see, so read it separately from the `GPU Process Memory` counters. It
doesn't count toward the Floor.

## What the fake data is

Calibrated on `linux-graph`, all refs, topo order, with a greedy lane walk: 7.5%
merges, 6.5% lanes closing, mean 294 active edges per row, 804 columns at worst.
That is far above any lane cap, so every lane is occupied on every row. The cap of
32 is a prototype assumption, not a decision.

- `rows`: the plan's format. u16 columns and an edge list per row, 6 bytes an edge.
- `delta`: one event per row plus a keyframe of lane colours every 256 rows. The
  lever, not the plan.

## How it measures

- **Frames:** on macOS, Metal's presented times. GPUI doesn't expose them, so the
  two drawable getters are hooked at runtime and GPUI stays stock. On Windows,
  PresentMon.
- **Scroll input:** `ScrollEvent`s dispatched into the window each frame. These
  skip AppKit's queue but count as high-rate input in GPUI, as real input does.
- **Held input:** real OS key events at 30 Hz through the platform queue, plus a
  notify every 8 ms. A run with no keys received is INVALID.
- **Memory:** `phys_footprint` or private working set, checked against `footprint`.
  Read it with `CAIRN_IDLE_SETTLE=1`, or it isn't a settled figure.
- **Control:** same input path, a plain repaint and 2 ms of paint work. See
  `CAIRN_CONTROL_SPIN_US`. With no work it misses about 2% of frames, while the
  real graph doesn't.

## macOS results, 2026-11-06

M3 Max, built-in display at 120 Hz, on battery, window 1728x1117 at 2x, cap 32,
1.5M rows, worst of three runs. Raw output is in `results/`, which is git-ignored,
so these figures are the record.

| Floor | Limit | `rows` (plan) | `delta` | Result |
| --- | --- | --- | --- | --- |
| Scroll, top, wheel | 99%, no gap over 50 ms | 99.58%, 16.7 ms | 99.37%, 33.3 ms | Pass, provisional |
| Scroll, top, flick | same | 99.16%, 16.7 ms | 99.16%, 16.7 ms | Pass, provisional |
| Scroll, row 100k, wheel | same | 99.26%, 16.7 ms | 99.48%, 16.7 ms | Pass, provisional |
| Scroll, row 100k, flick | same | 99.37%, 16.7 ms | 99.37%, 16.7 ms | Pass, provisional |
| Scroll, held input, notifies | same | 99.69%, 16.7 ms | 99.79%, 16.7 ms | Pass, provisional |
| Launch, 20 warm | median 400, max 800 ms | 276, 293 ms | 239, 247 ms | Pass |
| Memory, settled, idle | 300 MB | 538 MB, peak 757 | 248 MB, peak 467 | **Fail**, plan's format |

Recorded only, past row 100k: 99.05% to 99.69%, worst gap 16.7 ms, on both formats.

Scroll is provisional because the control read 98.52%, under the 99% it should
clear, on both formats. Its misses are 16.7 ms gaps like those in the scroll
runs. Nothing independent has checked Metal's presented times yet, such as
Instruments. Launch was measured with the first 5,000 rows only, since the rest
arrive in the background.

Memory is the idle re-measure below. The rest of the table is from the first
runs, on battery.

Memory by window size while still rendering, so each figure includes the driver
pool. `rows` / `delta`, top-wheel with a 3 s settle, one run:

| Window | Empty | `rows` | `delta` |
| --- | --- | --- | --- |
| 1728x1117 | 295 MB | 750 MB | 462 MB |
| 1440x900 | 275 MB | 688 MB | 397 MB |
| 1280x800 | 265 MB | 658 MB | 367 MB |

The Floor doesn't name a window size.

### Idle re-measure, 2026-11-06

The first memory runs were wrong: `tick` re-arms `on_next_frame` unconditionally,
so settle never went idle. Same machine and window on mains power, three separate
processes per format, `CAIRN_IDLE_SETTLE=1`, worst of three. The empty window read
85 MB idle, peak 296 MB.

The driver pool is about 193 MB of `Owned physical footprint (unmapped)
(graphics)`. A bare Metal program with no GPUI shows it too, so it isn't GPUI's. In
11 trials it appeared within a second of rendering starting and left within a
second of stopping, at 10 Hz to 1,000 Hz, with or without a window. That it's a
driver working pool is inferred from this behaviour, not documented.

After scrolling, GPUI holds 5 drawables at 28.3 MB each, 142 MB in total, against
3 in a bare Metal window. The cause isn't known, and the frame hooks haven't been
ruled out.

Scroll from these six processes, worst of three, `rows` / `delta`: top wheel
99.79% / 99.16%, top flick 99.58% / 99.90%, row 100k wheel 99.79% / **98.84%**, row
100k flick 99.37% / 99.90%, held input 99.58% / 99.79%. Worst gap 16.7 ms. The
control read 97.10% to 98.84%, so it failed in all six and these can't separate a
real miss from noise.

## Windows results, 2026-11-07

Ryzen 7 7800X3D, Radeon RX 7900 XT (driver 32.0.32015.7029), 64 GB, Windows 11
build 26200, display 3840x2160 at 150% set to 120 Hz. The window fills the
display: 2560x1440 logical, 3840x2160 px. Cap 32, 1.5M rows, PresentMon 2.6.0.
Three separate processes per format, one run of each scenario per process,
`CAIRN_IDLE_SETTLE=1`, worst of three.

| Floor | Limit | `rows` (plan) | `delta` | Result |
| --- | --- | --- | --- | --- |
| Scroll, top, wheel | 99%, no gap over 50 ms | 100%, 8.4 ms | 99.90%, 16.7 ms | Pass |
| Scroll, top, flick | same | 99.90%, 16.7 ms | 99.90%, 16.7 ms | Pass |
| Scroll, row 100k, wheel | same | 99.90%, 16.6 ms | 99.90%, 16.7 ms | Pass |
| Scroll, row 100k, flick | same | 99.79%, 16.7 ms | 99.90%, 16.7 ms | Pass |
| Scroll, held input, notifies | same | 99.90%, 16.7 ms | 100%, 8.4 ms | Pass |
| Launch, 20 warm | median 400, max 800 ms | 122, 133 ms | 121, 126 ms | Pass, stand-in |
| Memory, settled, idle | 300 MB | 326 MB, peak 333 | 37 MB, peak 44 | **Fail**, plan's format |

Recorded only, past row 100k: 99.79% to 100%, worst gap 16.7 ms, on both formats.

The control read 100% in five of six processes and 98.95% in one, so unlike on
the Mac these scroll results can be trusted. Every miss was one skipped refresh,
at most two per run, and every frame was `Composed: Flip`. Held runs received 240
keys and no freeze, so the zed#61469 watchdog isn't needed.

The empty window reads 17 MB. Dedicated GPU memory, recorded only: 50 MB empty
and 309 MB after scrolling at full screen, and it stays at 309 MB when idle. It
scales with window area: 201 MB at 1728x1117 and 119 MB at 1280x800 logical.

When the window doesn't cover the display, display intervals alternate about
7.0 / 9.65 ms, a mean of 8.33 ms. That puts the median at 9.6 ms, so the "presenting
at 120 Hz" check uses the mean interval. Full-display runs didn't alternate.

The first Windows run also needed a fix in `src/sys.rs`: the `SendInput` struct
was 32 bytes, but x64 needs 40.
