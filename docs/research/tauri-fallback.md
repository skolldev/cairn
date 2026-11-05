# Can a Tauri webview credibly hit Cairn's budgets?

Research for [#4](https://github.com/skolldev/cairn/issues/4), 2026-11-05. Nothing was built or run.

## Short answer

**No, not against these budgets.** Tauri 2 + Angular is a credible way to ship a slower Cairn, but not a fallback that keeps the speed positioning. It should fail launch on both OSes and the 8 ms budget on macOS, before anyone writes a graph. The file list is the only budget it probably passes. Windows memory is the open risk.

| Budget | macOS (WKWebView) | Windows (WebView2) | Why |
| --- | --- | --- | --- |
| Launch to first window, under 150 ms | **Fail**, except as a blank native window | **Fail**, except as a blank native window | Tauri's own CI runs hello-world at a median of 0.59 s (Windows) and 1.42 s (macOS). Best single runs are 0.44 s and 0.31 s, before any Angular. |
| No frame over 8 ms, scrolling | **Fail by default** | **Unproven** | WebKit's default turns a 120 Hz display into 60 rendering updates per second, so a painted frame arrives every 16.7 ms. WebView2 has no such cap in evidence, but no one has published a worst-frame number. |
| Under 50 ms to file list | **Plausible** | **Plausible** | The payload is small. IPC isn't the limit; Rust diff and Angular render time are unmeasured. |
| Under 300 MB, 1M-commit graph | **Plausible** | **Doubtful** | An empty Tauri app measures about 96 MB on macOS and about 312 MB on Windows by process sum (third-party CI). The graph has to fit on top. |

These are plausibility readings. Under the map's evidence rule, none of the CI numbers below count as results.

**What would need to be true for it to work:**

1. **#8 sets the scroll floor at 60 Hz (16.7 ms)**, or a real test shows WKWebView presents at 120 Hz on the macOS version that counts. Otherwise macOS fails by construction.
2. **"First window" means a native window**, with webview content arriving later. That meets the wording and not the intent. If it means first real content, Tauri needs a much looser floor than GPUI.
3. **The graph lives in Rust and JS holds only the visible window.** Rows arrive in paged binary chunks, and JS keeps a typed-array overscan buffer. Scrolling never waits on IPC.
4. **The graph is a hand-written canvas or WebGL renderer, not Angular.** Angular gets the chrome and contributes nothing to the hardest UI.
5. **Scroll position is remapped.** The kernel exceeds the engines' layout limit at normal row heights (see Scrolling).
6. **The Windows memory floor uses a counter a webview can pass.** Summed process memory leaves nothing for the graph.

**The tough part:** items 1 and 2 are budget rewrites, and the map says floors don't move once results are in. So honest adoption means deciding, in advance, to ship at 60 Hz with launch in the hundreds of milliseconds. That's a different product claim, and it should be made as one.

**It matters what made GPUI fail.** If GPUI fails on text input or IME (#10), a webview fixes exactly that. If it fails on scrolling (#9), Tauri is unlikely to rescue it on macOS. The plan's kill switch treats those as the same case.

## Evidence

Tags: **[M]** measured or read directly from the source that owns the fact. **[I]** my inference. **[U]** unverified.

Sources pinned to: Tauri [`2e23492`](https://github.com/tauri-apps/tauri/tree/2e23492ab9bcec06bab041e0e47447aaa0da298c), wry [`9f351c5`](https://github.com/tauri-apps/wry/tree/9f351c5aec4888533812d63d5a003f3b3400210b), WebKit [`bb7fdb1`](https://github.com/WebKit/WebKit/tree/bb7fdb1ac1c4ad4c32a5e99a1f24415f3d765453).

### Launch

- **[M]** Tauri publishes [per-commit results](https://github.com/tauri-apps/benchmark_results/tree/8c94ba28b224222c4c0dfa577729def3bb9f8a10) on GitHub-hosted `windows-latest` and `macos-latest` runners. The [hello-world test](https://github.com/tauri-apps/tauri/blob/2e23492ab9bcec06bab041e0e47447aaa0da298c/bench/tests/helloworld/public/index.html) exits when `DOMContentLoaded` fires and IPC reaches Rust. [hyperfine](https://github.com/tauri-apps/tauri/blob/2e23492ab9bcec06bab041e0e47447aaa0da298c/bench/src/run_benchmark.rs#L279) runs it with 3 warmup runs, so disk caches are warm.
- **[M]** Last 20 published runs, in seconds:

| App | Windows median | Windows best single run | macOS median | macOS best single run |
| --- | --- | --- | --- | --- |
| Tauri hello world ([Windows](https://github.com/tauri-apps/benchmark_results/blob/8c94ba28b224222c4c0dfa577729def3bb9f8a10/tauri-recent-windows.json), [macOS](https://github.com/tauri-apps/benchmark_results/blob/8c94ba28b224222c4c0dfa577729def3bb9f8a10/tauri-recent-macos.json)) | 0.589 | 0.439 | 1.422 | 0.311 |
| Bare wry hello world ([Windows](https://github.com/tauri-apps/benchmark_results/blob/8c94ba28b224222c4c0dfa577729def3bb9f8a10/wry-recent-windows.json), [macOS](https://github.com/tauri-apps/benchmark_results/blob/8c94ba28b224222c4c0dfa577729def3bb9f8a10/wry-recent-macos.json)) | 0.528 | 0.349 | 1.473 | 0.254 |

- **[I]** The macOS spread is wide, so read it as a noisy runner, not a stable number. Physical machines will likely beat these runners, by an amount [U]. But the best run of a bare webview with one heading is already about twice the budget, and the time sits in the webview rather than in Tauri.
- **[M]** Microsoft says each control ["launches multiple browser engine processes that add memory and startup overhead"](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/performance) and has a section titled "Don't use WebView2 for initial UI". The same page warns that with single-page-app frameworks the entire framework code must load, parse and run before anything can display.
- **[I]** A native window can appear before the webview has content, so under 150 ms is likely achievable for an empty window. Tauri's [`visible` window option](https://v2.tauri.app/reference/config/) controls when it shows. It would show nothing useful yet.
- **[U]** Neither Apple nor Microsoft publishes a cold-start figure on real hardware. I found no first-party number for either.

### Scrolling and frames

**macOS has a default 60 Hz cap on page rendering.**

- **[M]** `PreferPageRenderingUpdatesNear60FPSEnabled` [defaults to true](https://github.com/WebKit/WebKit/blob/bb7fdb1ac1c4ad4c32a5e99a1f24415f3d765453/Source/WTF/Scripts/Preferences/UnifiedWebPreferences.yaml#L5006-L5014) on every platform except visionOS.
- **[M]** With it on, a 120 Hz display [maps to the frame rate nearest 60](https://github.com/WebKit/WebKit/blob/bb7fdb1ac1c4ad4c32a5e99a1f24415f3d765453/Source/WebCore/platform/graphics/AnimationFrameRate.cpp#L35-L60). Both [page rendering updates](https://github.com/WebKit/WebKit/blob/bb7fdb1ac1c4ad4c32a5e99a1f24415f3d765453/Source/WebCore/page/Page.cpp#L3037) and [`requestAnimationFrame`](https://github.com/WebKit/WebKit/blob/bb7fdb1ac1c4ad4c32a5e99a1f24415f3d765453/Source/WebCore/dom/ScriptedAnimationController.cpp#L86) use this value.
- **[M]** [WebKit bug 294338](https://bugs.webkit.org/show_bug.cgi?id=294338), "Allow WKWebView to Support 120Hz on ProMotion Devices", is still NEW with no assignee, last modified 2026-09-17. A commenter in July 2026 says Capacitor and Tauri plugins reach 120 Hz through private WebKit APIs.
- **[M]** [Tauri #13978](https://github.com/tauri-apps/tauri/issues/13978) ("Runs at only 60fps on macOS") was closed as not planned. The maintainer's reason: WKWebView doesn't expose a setting.
- **[M]** The [GPU canvas pacing path](https://github.com/WebKit/WebKit/blob/bb7fdb1ac1c4ad4c32a5e99a1f24415f3d765453/Source/WebCore/page/Page.cpp#L3056-L3061) can only lower the rate, so WebGPU doesn't get round the cap. Animations can raise it, but [`WebAnimationsCustomFrameRateEnabled`](https://github.com/WebKit/WebKit/blob/bb7fdb1ac1c4ad4c32a5e99a1f24415f3d765453/Source/WTF/Scripts/Preferences/UnifiedWebPreferences.yaml#L7127-L7133) is off by default. **[U]** I didn't trace whether any other animation feeds that rate.
- **[U]** macOS 26 behaviour is contested. In the Tauri issue, one reporter says WKWebView runs at native refresh there and another says it's still capped. The second says scrolling and CSS transitions reach 120 Hz while `requestAnimationFrame` stays at 60. That fits WebKit driving macOS scrolling from a [display link in the UI process](https://github.com/WebKit/WebKit/blob/bb7fdb1ac1c4ad4c32a5e99a1f24415f3d765453/Source/WebKit/UIProcess/RemoteLayerTree/mac/RemoteLayerTreeEventDispatcher.mm#L59-L92), but I didn't confirm that rate from source. One test on a real ProMotion Mac settles this.
- **[I]** Even if that split holds, it doesn't save the budget. Already-painted content may slide at 120 Hz, but new rows are painted at 60. A fast scroll through a 1M-row list is mostly new rows.

**Windows has no such cap in evidence, but the 8 ms budget is very thin.**

- **[U]** The only evidence that WebView2 runs at 120 Hz is the report in Tauri #13978. The [HTML spec](https://html.spec.whatwg.org/multipage/webappapis.html#rendering-opportunity) ties rendering to hardware refresh and leaves the model to the browser, and I found no Microsoft statement.
- **[M]** Chrome's guidance is that of [16.66 ms per frame, your work must finish inside 10 ms](https://web.dev/articles/rendering-performance), because of browser overhead.
- **[I]** If that overhead of about 6.7 ms didn't shrink at 120 Hz, an 8.33 ms frame would leave under 2 ms for everything else. It's a rough guide, since overhead varies by machine, but the room is clearly small.
- **[M]** Real renderers show painting a viewport can be quick. xterm.js's canvas renderer [measured 0.4 to 4.6 ms](https://github.com/xtermjs/xterm.js/pull/938) of rendering work in Chrome 60 on a MacBook Pro. These are DevTools ranges without viewport size, and they are not worst-frame numbers.
- **[M]** [Glide Data Grid](https://github.com/glideapps/glide-data-grid/blob/0875d78cc41535a39f8b417ff88ce1ebe129c363/README.md) left virtualized DOM for canvas because of scrolling performance and claims millions of rows, but publishes no frame data.
- **[U]** I found no published worst-frame figure for any webview grid at this scale. "No frame over 8 ms" is a worst-case bound. Garbage collection and JIT pauses on the main thread are the obvious threats, and nothing above rules them out.

**The kernel exceeds the layout limit.**

- **[M]** `torvalds/linux` has 1,505,332 commits on its default branch, per the [GitHub commits API](https://api.github.com/repos/torvalds/linux/commits?per_page=1) pagination on 2026-11-05.
- **[M]** Both engines store layout positions as 32-bit fixed point with 6 fractional bits ([WebKit](https://github.com/WebKit/WebKit/blob/bb7fdb1ac1c4ad4c32a5e99a1f24415f3d765453/Source/WebCore/platform/LayoutUnit.h#L63-L65), [Chromium](https://github.com/chromium/chromium/blob/c4427ed8ebb3d7ca698b2dc5a8e9e3c74d57f7e3/third_party/blink/renderer/platform/geometry/layout_unit.h#L473)), which caps at 33,554,431 px.
- **[I]** That allows 22.29 px per row at today's commit count. A 24 px row needs 36.1M px. Past the limit, the scrollbar has to be remapped, as Glide [does by hand](https://github.com/glideapps/glide-data-grid/blob/0875d78cc41535a39f8b417ff88ce1ebe129c363/packages/core/src/internal/scrolling-data-grid/infinite-scroller.tsx#L236-L268). This works but adds code to the hottest path.

**Spike note.** **[M]** WebKit's `performance.now()` [defaults to 1 ms precision](https://github.com/WebKit/WebKit/blob/bb7fdb1ac1c4ad4c32a5e99a1f24415f3d765453/Source/WebCore/page/Performance.cpp#L73). **[I]** That's too coarse to judge an 8 ms floor from JS, so any Tauri spike needs native frame capture.

### IPC

- **[M]** `invoke` uses `fetch` to a [custom protocol](https://github.com/tauri-apps/tauri/blob/2e23492ab9bcec06bab041e0e47447aaa0da298c/crates/tauri/scripts/ipc-protocol.js#L37). Return values [are serialized to JSON](https://v2.tauri.app/develop/calling-rust/), and the docs warn this slows large returns. `tauri::ipc::Response` is the optimized path for array buffers.
- **[M]** [Channels](https://github.com/tauri-apps/tauri/blob/2e23492ab9bcec06bab041e0e47447aaa0da298c/crates/tauri/src/ipc/channel.rs#L34-L38) send small payloads by `eval` and larger ones by `fetch`. The thresholds are 8,192 bytes of JSON and 1,024 raw bytes, set from Tauri's own timing comments.
- **[M]** On Windows the path runs through WebView2's `WebResourceRequested`, and a response produced off the main thread is [dispatched back to the main thread](https://github.com/tauri-apps/wry/blob/9f351c5aec4888533812d63d5a003f3b3400210b/src/webview2/mod.rs#L1051-L1111). **[I]** A busy host main thread therefore delays IPC, so heavy work must stay on background threads.
- **[M]** WebView2 has a real shared-memory API, [`PostSharedBufferToScript`](https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/icorewebview2_17). wry doesn't use it (no reference in `src/webview2/mod.rs`).
- **[M]** A Tauri maintainer [said in December 2024](https://github.com/orgs/tauri-apps/discussions/11915) that only WebView2 supports shared memory, that users report it as slow, and that the other platforms would still lack it. He also relayed a non-scientific report of about 5 ms on macOS against about 200 ms on Windows for 10 MB of binary IPC.
- **[M]** Tauri's [3 MB test](https://github.com/tauri-apps/tauri/blob/2e23492ab9bcec06bab041e0e47447aaa0da298c/bench/tests/files_transfer/src-tauri/src/main.rs) uses `ipc::Response`. Across the last 20 Windows runs, its paired difference against hello-world has a median of -4 ms, inside the 18 ms run noise.
- **[U]** So IPC cost is known only loosely. No source measures round-trip latency in milliseconds under controlled conditions. The two data points above don't agree well.
- **[I]** For Cairn this is workable either way. A screen of rows or a file list is tens of KB, well under a millisecond even at the slow reported rate of 20 ms per MB. The cost that matters is shipping the whole graph: 1.5M rows at 32 bytes is 48 MB, about a second at that rate, and it doubles the memory. IPC is also asynchronous, so a frame can never wait for it, which is why JS needs its own overscan buffer.

### File list

- **[I]** The budget of 50 ms is three 60 Hz frames, and IPC takes a tiny part of it. What's left is the Rust diff (same as GPUI, see #11), Angular's render, and presenting. macOS loses up to 16.7 ms waiting for the next frame.
- **[U]** No source measures an Angular list update in either webview. Very large commits need a virtualized list, as they would anywhere.

### Memory

- **[M]** Tauri's harness measures memory [only on Linux](https://github.com/tauri-apps/tauri/blob/2e23492ab9bcec06bab041e0e47447aaa0da298c/bench/src/run_benchmark.rs#L377-L380). `max_memory` is empty for macOS and Windows, so there's no first-party number for either target. On Linux, hello-world peaks at 358 MB using [`mprof -C`](https://github.com/tauri-apps/tauri/blob/2e23492ab9bcec06bab041e0e47447aaa0da298c/bench/src/run_benchmark.rs#L109-L110), which sums child processes.
- **[M]** The process trees are large by design. WebView2 runs [a browser process, renderers and helpers such as GPU](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/process-model). WKWebView has [UI, WebContent and Networking processes](https://github.com/WebKit/WebKit/blob/bb7fdb1ac1c4ad4c32a5e99a1f24415f3d765453/Introduction.md), plus a GPU process. Microsoft notes GPU use [requires additional memory](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/performance).
- **Third-party, not first-party:** [Elanis's comparison](https://github.com/Elanis/web-to-desktop-framework-comparison/blob/c0529d252bfb3247955319eb8285feed4306afa3/README.md) is the only figure I found with a public method on both OSes. It's GitHub CI with undated runs, so treat it as a rough signal. Empty app, release build:

| | Process sum | Free-memory delta |
| --- | --- | --- |
| Tauri, Windows x64 | ≈312 MB | ≈187 MB |
| Tauri, macOS arm64 | ≈96 MB | ≈76 MB |

- **[I]** The graph model has to fit in what's left. On Windows that's nothing by process sum, and 113 MB by free-memory delta, or about 75 bytes per commit at 1.5M commits, with caches and a painted window still to come. On macOS the room is real.
- **[I]** The counter decides the Windows outcome. Shared pages are counted differently by each method, so #8 must name one.
- **[M]** For scale, the incumbent doesn't try. GitKraken exposes a [Max Commits in Graph](https://help.gitkraken.com/gitkraken-desktop/performance-issues/) setting and recommends lowering it to reduce rendering load. Its [8.6 notes](https://gitkraken.com/blog/gitkraken-client-v8-6) allow a floor of 500 commits.

### Other options

- **CEF runtime.** **[M]** Tauri has a [`tauri-runtime-cef`](https://github.com/tauri-apps/tauri/blob/feat/cef/crates/tauri-runtime-cef/Cargo.toml) crate at 0.1.0 on the `feat/cef` branch. Open issues show it's still maturing, and a user reports it running on [v3 alpha](https://github.com/tauri-apps/tauri/issues/14963). Stable is 2.x and v3 is at `alpha.5`. **[I]** Chromium on both OSes would remove the macOS cap, but at Electron-like size, and it worsens launch and memory. It isn't the "Tauri 2" in the plan.
- **Native graph, webview chrome.** **[M]** wry ships an [example](https://github.com/tauri-apps/wry/blob/9f351c5aec4888533812d63d5a003f3b3400210b/examples/wgpu.rs) of a transparent webview over a wgpu surface in one window. **[I]** Rust would paint the graph, so the scroll budget stops depending on the webview, and text input gets a mature web stack. This is the only shape that could meet 8 ms on macOS without private APIs. But it still has webview launch and memory costs, plus a custom GPU text and path renderer, which is what GPUI was meant to provide.
- **Private WebKit toggle.** **[I]** Possible for direct distribution, but it depends on a private preference that Apple can change on any OS update. Treat it as a last resort, not a plan.
