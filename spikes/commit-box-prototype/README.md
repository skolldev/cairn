# PROTOTYPE: commit box spike (issue #10)

Throwaway. Not part of Cairn. Answers one question: does gpui-component's
`Textarea` hold up as a commit box on macOS and Windows, against the Real typing and
Key input Floors in #8?

Stack: `gpui-component =0.9.0`, `gpui-pre =0.3.11`, Rust 1.98.1.

## Run

From this folder, in release mode.

```bash
cargo run --release
```

That's the window for [CHECKLIST.md](CHECKLIST.md), which only a human can run. The
right panel shows the escaped value, cursor, marked range, CR, TAB and
combining-mark counts, and a key log. Add `-- --paste-fix` for the second paste pass.

```bash
cargo run --release -- bench
```

That's the Key input Floor, plus the two scripted paste probes. It takes about
10 minutes. Keep the window in front, or the run is marked INVALID. It overwrites
the clipboard during the paste probes and restores it afterwards.

Flags: `--runs 3`, `--rates frame,vsync,worst`, `--only burst`, `--hold 10`,
`--burst 30`, `--watchdog`, `--stall-ms 120`, `--paste-fix`.

## Windows

`src/sys.rs` has only been type-checked for Windows. GPUI's Windows crate can't be
built from a Mac, so the whole program has never compiled there. Expect a fix or two
on the first run.

1. Set the primary display to 120 Hz.
2. Install PresentMon and open an elevated shell.
3. Set `CAIRN_PRESENTMON` to the PresentMon exe.
4. Run `cargo run --release -- bench` and record the result, with no watchdog.
5. If `Frame` fails, run it again with `--watchdog`. A pass then counts, but it must
   be reported as a pass with the watchdog.

If zed#61469 is real here, expect the freeze in `Frame` or `Worst`, and watch for
`renders` far above `presents`.

The #9 spike's Windows `SendInput` struct has a layout bug: it omits the padding
before the union, so `vk` lands at the wrong offset. Fixed here, not there. Its
`top-held` Windows run should be expected to send no real key until that's copied
across.

## How it measures

- **Frames:** Metal's presented times on macOS, PresentMon on Windows, as in #9.
- **Keys:** real events through the platform queue. A held key repeats at 30 Hz for
  10 s. The burst is 10 characters a second for 30 s, lowercase letters and spaces.
- **Lost or duplicated:** Backspace must delete exactly the keys sent, Left must move
  the cursor exactly that far, and the burst must match its script character for
  character.
- **Background results:** a thread produces at 1 kHz, as a stand-in for the scheduler.
  The rates differ only in how those reach the view.

| Rate | Drain | Role |
| --- | --- | --- |
| `frame` | A timer posts to the UI thread every 8.33 ms | Decides the Floor |
| `vsync` | `on_next_frame` | Recorded, design alternative |
| `worst` | A notify every 1 ms | Recorded |

`frame` decides because a timer competes with `WM_PAINT` in the message queue, which
is the mechanism zed#61469 describes. On Windows `on_next_frame` is driven by
`WM_PAINT`, so a `vsync` drain starves itself when paints starve. It can't provoke
the bug, though it may turn out to be the bridge Cairn should use.

The background rate is what keeps frames continuous. Without it, typing at
10 characters a second presents about every 100 ms, so the 50 ms limit would mean
nothing.

- **Control:** `--stall-ms 120` blocks the UI thread once mid-run. It must fail. On the
  M3 Max it read a 125 ms gap, FAIL.
- **Invalid runs:** the window lost key status, no keys were sent, or too few presents
  arrived for the gap figure to mean anything.

The floor doesn't cover input-to-photon latency, and neither does this.

## macOS results, 2026-11-06

M3 Max, built-in display, on battery, window 1180x760, worst of three runs, nothing else
from this spike open. Raw output is in `results/`, which is git-ignored, so these
figures are the record.

| Scenario | `frame` (Floor) | `vsync` | `worst` | Lost or duplicated | Result |
| --- | --- | --- | --- | --- | --- |
| Hold Backspace, 10 s | 25.0 ms | 8.3 ms | 16.7 ms | None | Pass |
| Hold Left, 10 s | 16.7 ms | 16.7 ms | 8.3 ms | None | Pass |
| Burst, 30 s | 25.0 ms | 16.7 ms | 25.0 ms | None | Pass |

No watchdog, which doesn't exist on macOS. The refresh rate wasn't read from the
system during the run, but the 8.3 ms gaps show the display was at 120 Hz.

**Contended run:** an earlier run with a second copy of this window open and being
typed in hit 58.3 ms on one `frame` burst, FAIL, and exactly 50.0 ms on one `worst`
burst. The clean run replaced it, and the rule to run once and keep the result was
set before it. It does show the margin isn't huge when another GPUI app is competing
for the machine. Log: `results/macos-full-contended.log`.

**Void:** the first run on this branch, commit ce84a4e, had no text system. Without
`gpui_platform`'s `font-kit` feature, macOS GPUI uses a no-op text system that draws no
glyphs, so those gaps measured an app with no text. Its log is
`results/macos-full-notext-INVALID.log`. The #9 spike's binary has the same fallback.

- **Not a result about real keys:** events are posted to the app, so the OS key
  repeat and the IME are bypassed.
- **`worst` is a weaker test than its name:** the timer reached about 500 notifies a
  second, not 1,000.
- **`frame` drops presents:** about 8% to 11% of drawables were never presented, and
  almost none at `vsync` or `worst`. The cause isn't confirmed.

Paste, stock: CRLF keeps 4 `\r` and Excel keeps 3, so **the Floor fails as shipped**.
`Textarea::on_paste` cleans both in about five lines of app code (`--paste-fix`), and
Excel's tabs and quoted cell are left alone. Both results were repeated with text
rendering on. Whether that counts as a framework pass is a call for #13, though the fix is shippable.

Not done: all of CHECKLIST.md on both OSes, and everything on Windows.
