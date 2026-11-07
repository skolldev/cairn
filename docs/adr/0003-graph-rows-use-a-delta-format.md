# Graph rows use a delta format, not a per-row lane store

The plan stored every row's full lane state compactly: a node column, plus incoming and outgoing edges for each lane, in `u16` columns. On the Linux kernel that can't fit. A greedy lane walk over all refs averages 294 active edges per row and reaches 804 columns at worst, so the uncapped store is about 2.6 GB. Even capped at 32 lanes, it measured 301 MB, which is over the 300 MB memory Floor before any framework cost. So the Phase 0 memory miss (#9) condemned this format, not GPUI.

We store one event per row instead, plus a full keyframe every 256 rows, and rebuild the visible rows' lanes from the nearest keyframe when painting. On the kernel that's about 12.4 MB, and building and painting stayed at about 0.4 ms a frame. It passed the memory Floor at 248 MB on macOS and 37 MB on Windows.

## Consequences

- Random access to a row now means replaying from its keyframe. Sha jump, search hits and scrolling past row 100k all depend on that replay staying cheap.
- macOS headroom is about 52 MB at 1728x1117, before gix, redb or commit metadata are loaded. That makes memory a live risk for the budget rewrite, not a solved problem.
- The 32-lane cap was a prototype assumption, not a decision. The delta format doesn't need it, so lane overflow on very wide histories is still an open Phase 2 question.
