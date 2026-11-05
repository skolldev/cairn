# State of GPUI and gpui-component on macOS and Windows

Research for [#2](https://github.com/skolldev/cairn/issues/2), read on 2026-11-05. Nothing was built or run, so this records what the source and trackers say, not measurements. "Derived" marks a conclusion drawn from reading code. See [Unverified](#unverified) for what this does not establish.

## Short answer

**GPUI stays in the running, but the plan's pin has to go.** Nothing here trips the kill switch on reading alone: that still takes the spikes. There is one serious open Windows bug, and it has a reported app-level workaround.

What changes:

1. **The pin is wrong and stale.** gpui-component 0.5.x pins no rev. It takes `gpui = "0.2.2"` from crates.io, Zed's commit of 2025-10-22. That predates Zed's Windows dead-key and IME rework, and the 0.5.x `VirtualList` does linear work per frame. Move to `gpui-kit = "=0.9.0"` with a committed `Cargo.lock`. It carries `gpui-pre =0.3.11`, a snapshot of zed@f816e1f.
2. **Rust: use 1.98.1.** That is Zed's toolchain at the snapshot commit. 1.82 cannot read the manifests at all, because they use edition 2024.
3. **Biggest risk: [zed#61469](https://github.com/zed-industries/zed/issues/61469).** On Windows, sustained input plus background notifies can stop `Present()` for seconds while paint keeps running. It is open and reserved for the Zed team, and I found no fix in gpui-ce or gpui-fast. A reporter says a watchdog thread in the app cures it. If that holds, this stays under the two-week kill switch.
4. **Frame timing must count presents.** Under #61469 an in-process paint timer reads healthy while the screen is frozen. On Windows, measure with PresentMon, which is how the reporter caught it. This belongs in the floors ticket (#8).
5. **Spike #9 should not use `VirtualList`.** Use `uniform_list` plus `canvas`, which is how Zed's own git graph works. It needs no per-row height table. Test at the far end of the list: scroll offsets are `f32`, so past row ~700k at 24 px they step by 2 px (derived).
6. **Spike #10 must target `Textarea`.** Since 0.6.0, `Input` is single-line only.
7. **Re-check the pin when the spikes start.** A Zed member said on 2026-10-30 that automated weekly GPUI releases are coming "soon". That could replace the `gpui-pre` route.

No macOS finding looks like a blocker. No focus issue found has a blocking shape on either OS.

## Pin and Rust version

| | gpui-component 0.5.1 (plan) | gpui-kit 0.9.0 (recommended) |
|---|---|---|
| Released | [2026-02-05](https://github.com/longbridge/gpui-kit/releases/tag/v0.5.1) | [2026-11-02](https://github.com/longbridge/gpui-kit/releases/tag/v0.9.0) |
| GPUI dependency | [`gpui = "0.2.2"`](https://github.com/longbridge/gpui-kit/blob/0f0ab35233212f8f3277028995caf0c41e13ee6c/Cargo.toml#L30), crates.io, no rev | [`gpui-pre =0.3.11`](https://github.com/longbridge/gpui-kit/blob/7591a35eced761e3613c01e71ff04c10129dae1a/Cargo.toml#L68), exact |
| Zed source | [69e2130](https://github.com/zed-industries/zed/commit/69e2130295c2649963eb639fc70b4f2ee8ea1624), 2025-10-22 | [f816e1f](https://github.com/zed-industries/zed/commit/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038), 2026-11-01 ([crate description](https://crates.io/crates/gpui-pre)) |
| Zed's toolchain at that commit | [1.90](https://github.com/zed-industries/zed/blob/69e2130295c2649963eb639fc70b4f2ee8ea1624/rust-toolchain.toml#L2) | [1.98.1](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/rust-toolchain.toml#L2) |
| Provable floor | 1.88 | Not established |

**Rust detail.** Neither gpui nor gpui-pre declares `rust-version` ([crates.io](https://crates.io/crates/gpui-pre)), so no manifest states a minimum.

- **Hard limit for both:** [edition 2024](https://github.com/longbridge/gpui-kit/blob/0f0ab35233212f8f3277028995caf0c41e13ee6c/Cargo.toml#L22) needs [Rust 1.85](https://blog.rust-lang.org/2025/02/20/Rust-1.85.0/). Rust 1.82 is out for every candidate here.
- **gpui 0.2.2:** it uses [let chains](https://github.com/zed-industries/zed/blob/69e2130295c2649963eb639fc70b4f2ee8ea1624/crates/gpui/src/element.rs#L606), stable since [1.88](https://blog.rust-lang.org/2025/06/26/Rust-1.88.0/), so 1.88 is a lower bound and 1.90 is known good.
- **gpui-pre 0.3.11:** no real minimum can be derived. The highest `rust-version` in the 0.9.0 lockfile is 1.92, on `oo7`, but that is a [Linux and FreeBSD dependency](https://crates.io/crates/gpui-pre-linux). The next highest is 1.90. Lockfile values don't bound gpui-pre's own source anyway, so 1.98.1 is the supported answer. The GPUI README says to use [the latest stable](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/crates/gpui/README.md#L8), which is [1.99.1](https://blog.rust-lang.org/releases/). [gpui-fast](https://github.com/longbridge/gpui-fast/blob/86a8228229c8646b552d747434ffd690216a872c/rust-toolchain.toml) also pins 1.98.1. gpui-kit's native CI jobs pin no version.

Commit `rust-toolchain.toml` with `channel = "1.98.1"`.

**Pin discipline.** gpui-kit moved to exact `gpui-pre` pins after [0.6.4 stopped compiling](https://github.com/longbridge/gpui-kit/blob/7591a35eced761e3613c01e71ff04c10129dae1a/script/check-gpui-pin.ts#L9) the day a new snapshot appeared. Its own sibling crates are not exact, so [`=0.7.1` could still resolve parts to 0.7.2](https://github.com/longbridge/gpui-kit/issues/3605). Commit the lockfile. Expect churn: 13 versions shipped between 2026-09-03 and 2026-11-02 ([crates.io](https://crates.io/crates/gpui-component/versions)).

**What 0.2.2 lacks.** All of these landed after it:

- Windows dead keys, AltGr and IME rework: [#41259](https://github.com/zed-industries/zed/pull/41259), merged 2025-11-05, which removes custom dead-key handling.
- More dead-key edge cases: [#42294](https://github.com/zed-industries/zed/pull/42294).
- A Windows drag-and-drop crash: [#42227](https://github.com/zed-industries/zed/pull/42227).
- Direct Manipulation touchpad handling, including pinch: [#51354](https://github.com/zed-industries/zed/pull/51354).
- AccessKit: [#56065](https://github.com/zed-industries/zed/pull/56065).

## Maintenance and Zed's commitment

**GPUI is healthy as Zed's framework. Zed makes no stability promise to outside users.**

- **Activity:** 160 commits touched [`crates/gpui`](https://github.com/zed-industries/zed/commits/main/crates/gpui) since 2026-08-05, and 64 touched [`crates/gpui_windows`](https://github.com/zed-industries/zed/commits/main/crates/gpui_windows) since 2026-05-05.
- **Stated status:** "[still pre-1.0](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/crates/gpui/README.md#L8)", with "often" breaking changes. [gpui.rs](https://www.gpui.rs/) says that "for the near future, GPUI is tied to Zed". The [1.0 post](https://zed.dev/blog/zed-1-0) makes no commitment to external users.
- **Signs of library intent:** Zed's contributor rules say GPUI is "[consumed as a library](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/.rules#L151)", and CI enforces a `[GPUI]` release-note bullet. Publishing tooling landed in [`xtask`](https://github.com/zed-industries/zed/commits/main/tooling/xtask/src/tasks/gpui) in September 2026.
- **Releases:** `gpui` on crates.io has been stuck at 0.2.2 for a year, and `gpui_platform`, which the README tells you to add, [isn't published](https://crates.io/crates/gpui_platform). On 2026-10-30 a Zed member [said](https://github.com/zed-industries/zed/discussions/55271) automated releases on Zed's weekly cadence are coming "soon", and that until then you should [pin a git revision](https://github.com/zed-industries/zed/issues/66275).
- **`gpui-pre` is not Zed's.** Longbridge republishes Zed snapshots [because Zed publishes only occasionally](https://github.com/longbridge/gpui-kit/blob/7591a35eced761e3613c01e71ff04c10129dae1a/script/bump-gpui.ts#L6), on a [weekly cron](https://github.com/longbridge/gpui-kit/blob/7591a35eced761e3613c01e71ff04c10129dae1a/.github/workflows/release-gpui.yml#L6). One [person owns the crate](https://crates.io/crates/gpui-pre), so that is a single point of failure until Zed's releases land.

gpui-kit is itself under real load: it is the UI of [Longbridge Pro](https://github.com/longbridge/gpui-kit/blob/7591a35eced761e3613c01e71ff04c10129dae1a/README.md), which [offers a Windows download](https://longbridge.com/desktop). The repo was renamed from gpui-component to `gpui-kit` in [0.6.0](https://github.com/longbridge/gpui-kit/releases/tag/v0.6.0). `gpui-component` remains as its styled layer.

## Forks

**[gpui-ce](https://github.com/gpui-ce/gpui-ce): not a fallback for this stack.** It is a community fork, created 2025-12, with [0.3.0 published 2026-10-20](https://crates.io/crates/gpui-ce).

- It is [drifting from upstream](https://github.com/gpui-ce/gpui-ce/blob/81742184cb1df090709257a5e4061eba44fa8e8a/README.md#L11), with a [new text system](https://github.com/gpui-ce/gpui-ce/blob/81742184cb1df090709257a5e4061eba44fa8e8a/CHANGELOG.md#L40) and many breaking changes.
- It needs [Rust 1.95](https://github.com/gpui-ce/gpui-ce/blob/81742184cb1df090709257a5e4061eba44fa8e8a/crates/gpui/Cargo.toml#L14).
- gpui-kit's backends are `gpui-pre` and `gpui-fast`, and it [doesn't reference gpui-ce](https://github.com/longbridge/gpui-kit/blob/7591a35eced761e3613c01e71ff04c10129dae1a/Cargo.toml#L60-L78), so choosing it likely means giving up gpui-component.
- Its [contribution policy bars LLM-written code and prose](https://github.com/gpui-ce/gpui-ce/blob/81742184cb1df090709257a5e4061eba44fa8e8a/README.md#L139).
- It keeps the mechanism behind #61469 ([paint](https://github.com/gpui-ce/gpui-ce/blob/81742184cb1df090709257a5e4061eba44fa8e8a/crates/gpui_windows/src/platform.rs#L343), [key dispatch](https://github.com/gpui-ce/gpui-ce/blob/81742184cb1df090709257a5e4061eba44fa8e8a/crates/gpui/src/window.rs#L6287)).

**[gpui-fast](https://github.com/longbridge/gpui-fast): a lever, not a baseline.** Longbridge's fork adds retained rendering and [cached scroll layers](https://github.com/longbridge/gpui-fast/blob/86a8228229c8646b552d747434ffd690216a872c/docs/scroll-layers.md), including on Direct3D 11, behind a gpui-kit feature flag. It [calls itself experimental](https://github.com/longbridge/gpui-fast/blob/86a8228229c8646b552d747434ffd690216a872c/README.md#L3) and has the [same #61469 mechanism](https://github.com/longbridge/gpui-fast/blob/86a8228229c8646b552d747434ffd690216a872c/crates/gpui/src/window.rs#L6355). Run the spikes on `gpui-pre` first.

## Open issues that matter

Status is from the trackers on 2026-11-05. Zed issues are filed against the Zed app unless labelled `area:gpui`, so some may not reach a gpui-kit widget.

### Windows

| Issue | Area | Status | Why it matters |
|---|---|---|---|
| [zed#61469](https://github.com/zed-industries/zed/issues/61469) | Input, rendering | `area:gpui`, Zed team only, S3 | Presents stop while paint continues. See below. |
| [zed#66382](https://github.com/zed-industries/zed/issues/66382) | App lifecycle | `area:gpui`, S1 | Process occasionally never exits after the last window closes, seen on gpui-pre 0.3.10. |
| [zed#64850](https://github.com/zed-industries/zed/issues/64850) | IME | S3 | Candidate window covers the composition near a screen edge, because GPUI [passes one mid-line point](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/crates/gpui_windows/src/events.rs#L645-L656). A bottom-docked commit box can hit this. |
| [zed#59636](https://github.com/zed-industries/zed/issues/59636) | IME | S2, needs repro | Composition extent reported as a caret-sized rectangle. |
| [zed#59882](https://github.com/zed-industries/zed/issues/59882) | IME | S2, needs repro | Chinese IME intermittently fails, including in Zed's commit box. |
| [zed#62404](https://github.com/zed-industries/zed/issues/62404) | Focus | `area:gpui`, S2, needs repro | [`activate()` injects a global Alt press](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/crates/gpui_windows/src/window.rs#L845-L862), visible to other apps. |
| [zed#41210](https://github.com/zed-industries/zed/issues/41210) | Scrolling | S3, needs repro | Selection drag stops auto-scrolling when the mouse is still. |
| [zed#65664](https://github.com/zed-industries/zed/issues/65664) | Rendering | S2 | Text is blurry when the window client height is odd. |
| [zed#56349](https://github.com/zed-industries/zed/issues/56349) | Rendering | `area:gpui`, needs repro | Glyph atlas memory growth, reported on 0.2.2 on ARM64. |
| [kit#2024](https://github.com/longbridge/gpui-kit/issues/2024) | Paste | Open | Text copied from Excel doesn't paste. |

**Input method on Windows.** GPUI uses IMM32 calls ([`WM_IME_COMPOSITION`](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/crates/gpui_windows/src/events.rs#L153), `ImmSetCandidateWindow`). Dead keys now behave as in other Windows apps and arrive as ordinary [`WM_CHAR` text](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/crates/gpui_windows/src/events.rs#L470). Touchpad gestures, scroll and pinch, run through [Direct Manipulation](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/crates/gpui_windows/src/direct_manipulation.rs#L21).

**#61469 in detail.**

- **Mechanism:** presents come only from `WM_PAINT`, which Windows synthesizes when the queue is idle ([vsync thread](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/crates/gpui_windows/src/platform.rs#L500)). [`dispatch_key_event`](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/crates/gpui/src/window.rs#L6268) draws without presenting and clears the dirty flag.
- **Evidence:** a PresentMon trace shows zero presents for 15 s during a key hold, with paints continuing.
- **Trigger:** it scales with background `cx.notify()` traffic, here ~20 chunks/s. A controlled test in [#63489](https://github.com/zed-industries/zed/pull/63489) found saturated posted traffic to be the deterministic trigger, and isolated 60 Hz keys healthy. A later comment reports it from scrollbar dragging at ~65 Hz on Windows 10. So Cairn's job results arriving while the user types or scrolls is the case to test (derived).
- **Fix status:** [#61632](https://github.com/zed-industries/zed/pull/61632) and #63489 were closed: "[this fix doesn't seem to be the right one](https://github.com/zed-industries/zed/pull/63489)". It is assigned to a Zed engineer.
- **Workaround:** the reporter ships a watchdog thread calling `RedrawWindow(hwnd, None, None, RDW_INVALIDATE | RDW_UPDATENOW)` every 8 ms, and reports it fully resolves the freeze. It needs no GPUI patch.

### macOS

| Issue | Area | Status | Why it matters |
|---|---|---|---|
| [kit#3456](https://github.com/longbridge/gpui-kit/issues/3456) | `Textarea` | Open | Cmd-Return during a dead key submits unfinished text. A commit shortcut would hit this. |
| [kit#3605](https://github.com/longbridge/gpui-kit/issues/3605) | IME, focus | Open | Pinyin candidate popup survives focus moving to a dialog. |
| [zed#62058](https://github.com/zed-industries/zed/issues/62058) | Dead keys | S3 | A pending dead key survives a mouse click and inserts at the old position. |
| [zed#52550](https://github.com/zed-industries/zed/issues/52550) | Keys | `area:gpui`, needs repro | `DefaultKeyBinding.dict` selectors are ignored. |
| [zed#66403](https://github.com/zed-industries/zed/issues/66403) | Performance | S2 | ~4 ms per display cycle with a customized pointer. Note the pointer setting when measuring. |

### Focus (both OSes)

[kit#3760](https://github.com/longbridge/gpui-kit/issues/3760): closing a dialog can restore focus to a removed element, so window actions drop until the next click. [kit#3612](https://github.com/longbridge/gpui-kit/issues/3612): stale tab-stop entries can trap reverse Tab. Both are small, library-level bugs.

## `Input` and `Textarea`

**Since 0.6.0 there are [three controls](https://github.com/longbridge/gpui-kit/releases/tag/v0.6.0).** `Input` is single-line with masks and validation. [`Textarea`](https://github.com/longbridge/gpui-kit/blob/7591a35eced761e3613c01e71ff04c10129dae1a/website/component/textarea.md#L8) is multi-line, with fixed rows, soft wrap and auto-grow. `Editor` is for code. In 0.5.1, multi-line was a [flag on `InputState`](https://github.com/longbridge/gpui-kit/blob/0f0ab35233212f8f3277028995caf0c41e13ee6c/crates/ui/src/input/state.rs#L420).

All three share one engine in 0.9.0:

- **Undo:** transaction-based, with [1,000 transactions](https://github.com/longbridge/gpui-kit/blob/7591a35eced761e3613c01e71ff04c10129dae1a/crates/base/src/input/base/undo_manager.rs#L6) and intent-based coalescing. An [IME undo bug](https://github.com/longbridge/gpui-kit/issues/2761) on macOS was closed in August 2026, after 0.5.1.
- **Paste:** [`insert_clipboard`](https://github.com/longbridge/gpui-kit/blob/7591a35eced761e3613c01e71ff04c10129dae1a/crates/base/src/input/base/state.rs#L3245) treats a paste as one atomic edit.
- **CRLF:** [`normalize_input`](https://github.com/longbridge/gpui-kit/blob/7591a35eced761e3613c01e71ff04c10129dae1a/crates/base/src/input/base/state.rs#L4111) strips newlines only in single-line mode, so multi-line text keeps `\r`. A [test](https://github.com/longbridge/gpui-kit/blob/7591a35eced761e3613c01e71ff04c10129dae1a/crates/base/src/input/base/state.rs#L7104) shows the `Textarea` cursor treats CRLF as one newline. [kit#2877](https://github.com/longbridge/gpui-kit/issues/2877), blank lines from CRLF paste in `Editor`, is still open. Cairn should normalize to LF before committing (derived).
- **IME:** marked text is implemented ([`unmark_text`](https://github.com/longbridge/gpui-kit/blob/7591a35eced761e3613c01e71ff04c10129dae1a/crates/base/src/input/base/state.rs#L4602)), and the repo has [IME protocol tests](https://github.com/longbridge/gpui-kit/blob/7591a35eced761e3613c01e71ff04c10129dae1a/script/test-input). These are headless, so they say nothing about real IMEs.

**Add to spike #10:** Cmd-Return during a dead key; IME with the commit box near the bottom of the screen; a pending dead key followed by a mouse click; CRLF and Excel paste on Windows; a held key while a background timer calls `cx.notify()` at ~20 Hz.

## Lists

**`VirtualList` (variable heights).** It requires an `Rc<Vec<Size<Pixels>>>` with [one entry per row](https://github.com/longbridge/gpui-kit/blob/7591a35eced761e3613c01e71ff04c10129dae1a/crates/base/src/virtual_list.rs#L351) and builds cumulative `f32` origins from it.

- In 0.9.0 a frame costs a [binary search](https://github.com/longbridge/gpui-kit/blob/7591a35eced761e3613c01e71ff04c10129dae1a/crates/base/src/virtual_list.rs#L388) plus the visible rows. The [size check](https://github.com/longbridge/gpui-kit/blob/7591a35eced761e3613c01e71ff04c10129dae1a/crates/base/src/virtual_list.rs#L478) should be a pointer comparison because `Size<Pixels>` is `Eq` (derived).
- In 0.5.1 every frame [collects bounds for all items](https://github.com/longbridge/gpui-kit/blob/0f0ab35233212f8f3277028995caf0c41e13ee6c/crates/ui/src/virtual_list.rs#L555) and [scans linearly](https://github.com/longbridge/gpui-kit/blob/0f0ab35233212f8f3277028995caf0c41e13ee6c/crates/ui/src/virtual_list.rs#L631). At 1M rows that alone threatens the budget (derived).

**`uniform_list` (fixed heights, in GPUI itself).** It keeps no per-row state and does [`f64` index math](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/crates/gpui/src/elements/uniform_list.rs#L473-L498). Commit rows are fixed height, so this is the right starting point for #9.

**The `f32` limit (derived, not measured).** The scroll offset is [`Pixels(f32)`](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/crates/gpui/src/geometry.rs#L2689) and is updated by [`scroll_offset.y += delta_y`](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/crates/gpui/src/elements/div.rs#L3440).

- `f32` has integer precision up to 2^24, or 16.8M px, which is row ~700k at 24 px. Beyond that, offsets step by 2 px, and by 4 px past 33.5M px (row ~1.4M).
- Deltas under half a step are lost, so slow trackpad scrolling can stall near the end of a large history.
- Zed's editor avoids this with [`f64` offsets](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/crates/editor/src/scroll.rs#L32-L33), and GPUI's `list` [anchors to an item index](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/crates/gpui/src/elements/list.rs#L1449).
- If the tail misbehaves, the fix is a small custom element with an anchored scroll position.

## Custom path painting

**Zed ships the exact pattern Cairn needs.** Its [git graph](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/crates/git_ui/src/git_graph.rs#L3539) is a `uniform_list` table beside a `canvas` that paints only visible rows, with edges as [`PathBuilder::stroke`](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/crates/git_ui/src/git_graph.rs#L3651) and row heights [snapped to device pixels](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/crates/git_ui/src/git_graph.rs#L1489) so the two columns don't drift. It is a technique reference only: `git_ui` is [GPL-3.0-or-later](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/crates/git_ui/Cargo.toml#L6), while [gpui is Apache-2.0](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/crates/gpui/Cargo.toml#L9). Two things not to copy: it uses [`f32` scroll math](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/crates/git_ui/src/git_graph.rs#L3557), and it [filters every line on each render](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/crates/git_ui/src/git_graph.rs#L3565-L3573). Cairn needs an interval index.

**How paths cost (from source).**

- `PathBuilder::build` [tessellates with lyon](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/crates/gpui/src/path_builder.rs#L244) on the calling thread. In a `canvas`, that is the main thread during paint.
- The scene [merges consecutive paths](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/crates/gpui/src/scene.rs#L359) into one batch, even across layers, until another primitive must draw first.
- Each batch is a full pass over a window-sized 4x MSAA texture: clear, draw, resolve, composite. This holds on both [Direct3D 11](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/crates/gpui_windows/src/directx_renderer.rs#L615-L676) and [Metal](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/crates/gpui_apple/src/metal_renderer.rs#L950-L996).

So cost is driven by batch count first, and vertex count second (derived). **Paint all edges consecutively**, then nodes. Interleaving paths with quads or text multiplies window-sized passes. Merging one builder per colour is also worth testing.

Upstream has no retained layer for scrolling. The only performance data found is old and informal: in 2025 a contributor [reported](https://github.com/zed-industries/zed/pull/34992) that 5,000 painted stars ran one frame worse than the previous renderer on Windows, and took over 70% GPU on an Intel UHD 630 Mac. Treat integrated GPUs as the risky case. Nothing public shows this pattern at 1M rows or against an 8 ms budget.

**Add to spike #9:** measure at the end of the list as well as the start; include a slow trackpad scroll there; deliver fake job results at ~20 Hz while scrolling; test with a dense lane window on the Windows PC's actual GPU.

## Accessibility (for the record)

- **gpui 0.2.2:** none, with no AccessKit dependency.
- **Current GPUI:** [AccessKit landed 2026-05-27](https://github.com/zed-industries/zed/pull/56065), with [Windows](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/crates/gpui_windows/Cargo.toml#L25) and [macOS](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/crates/gpui_macos/Cargo.toml#L28) adapters. Only elements with [an ID and a role](https://github.com/zed-industries/zed/blob/f816e1f9263e3a8ef63c4baaa02e1441ce1c6038/crates/gpui/src/_accessibility.rs#L63) are reported, so a painted graph exposes nothing by default (derived).
- **gpui-kit:** [documents AccessKit support](https://github.com/longbridge/gpui-kit/blob/7591a35eced761e3613c01e71ff04c10129dae1a/website/docs/accessibility.md), but gaps are open: [table rows](https://github.com/longbridge/gpui-kit/issues/3647), [disabled state](https://github.com/longbridge/gpui-kit/issues/3660) and [labels](https://github.com/longbridge/gpui-kit/issues/3698). Duplicate accessibility IDs can also [panic](https://github.com/longbridge/gpui-kit/issues/3295) or [abort](https://github.com/longbridge/gpui-kit/issues/3705).

## Unverified

- **Nothing compiled or ran**, so no build on macOS or Windows is confirmed.
- **The true minimum Rust** for gpui-pre 0.3.11 is unknown, and for gpui 0.2.2 whether 1.88 and 1.89 work. Reporters used [1.96.0 with 0.3.8](https://github.com/longbridge/gpui-kit/issues/3605) and [1.99.1 with 0.3.10](https://github.com/zed-industries/zed/issues/66382).
- **The #61469 watchdog** is one reporter's claim. Whether Cairn's traffic triggers the bug is unknown.
- **The `f32` stepping and path batch costs** come from reading code, not measuring.
- **Automated GPUI releases:** no date beyond "soon".
- **Zed's graph at scale:** I found no statement or benchmark at kernel scale.
- **Issues filed on Zed's app** may not reproduce in gpui-kit widgets.
- **Longbridge Pro on Windows:** a download exists. I found no evidence of how well it handles IME.
- **The plan's wording:** `Git Client (GPUI + gix) Project Plan.md` is not in the repo at this commit, so the pin wording comes from the ticket brief.
