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

## Windows

`src/sys.rs` has a Windows path, but it has never been compiled or run. Expect
to fix it on the first run.

1. Set the primary display to 120 Hz.
2. Install PresentMon and open an elevated shell.
3. Set `CAIRN_PRESENTMON` to the PresentMon exe, then run the commands above.

Windows launch numbers are an in-process stand-in, not presented frames. Read
dedicated GPU memory from Task Manager. If `top-held` freezes, record that first,
then add the zed#61469 watchdog. It isn't in this prototype.

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
| Memory, settled | 300 MB | 738 MB, peak 757 | 466 MB, peak 468 | **Fail** |

Recorded only, past row 100k: 99.05% to 99.69%, worst gap 16.7 ms, on both formats.

Scroll is provisional because the control read 98.52%, under the 99% it should
clear, on both formats. Its misses are 16.7 ms gaps like those in the scroll
runs. Nothing independent has checked Metal's presented times yet, such as
Instruments. Launch was measured with the first 5,000 rows only, since the rest
arrive in the background.

Memory by window size, `rows` / `delta`, top-wheel with a 3 s settle, one run:

| Window | Empty | `rows` | `delta` |
| --- | --- | --- | --- |
| 1728x1117 | 295 MB | 750 MB | 462 MB |
| 1440x900 | 275 MB | 688 MB | 397 MB |
| 1280x800 | 265 MB | 658 MB | 367 MB |

`delta` misses the Floor at every size, so the miss isn't only the row store. On
the full-size window, `footprint` showed about 380 MB of 439 MB in graphics
memory. The Floor doesn't name a window size.
