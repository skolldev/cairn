# Git Client (GPUI + gix): Project Plan

Nov 5, 2026 · @Alexander May

## Goals and non-goals

The MVP is done when the client fully replaces GitKraken for two weeks of real work on GitHub and Azure DevOps repos, on both macOS and Windows, with conflicts as the only reason to open another tool. The test is daily use. A feature count doesn't prove anything.

**MVP scope: the GitKraken parts worth copying**

- Left panel: local branches, remote branches, tags and stashes. Double-click checks a branch out, and a remote branch becomes a tracking branch.
- Central commit graph: lanes, ref badges, a WIP row on top, and instant scrolling through 100k+ commits.
- Commit and WIP panel: changed files, hunk- and line-level staging, and a commit message box.
- Diff viewer: unified and side-by-side, with syntax highlighting.
- Network: fetch, pull and push, authenticated against GitHub and Azure DevOps with tokens.

**Non-goals for the MVP** (each goes to a later phase or never)

- AI features of any kind, telemetry, accounts or cloud sync.
- PR creation and review, issue trackers, avatars from provider APIs.
- A built-in merge conflict editor. Conflicts go to the user's configured `git mergetool`.
- Interactive rebase UI, drag-and-drop merge/rebase, submodule and LFS management UI.
- Reimplementing git porcelain. Anything that mutates the repo is done by the `git` CLI.

**Platforms:** macOS and Windows are equal targets. A phase isn't done until its gate passes on both, and every performance budget applies to both. Linux stays best effort. GPUI is most mature on macOS, so Phase 0 proves the Windows side early instead of finding gaps late. Phase sizes from Phase 1 onward were estimated for one OS, so expect them to grow.

&#91;embedded content: roadmap · 6 phases, 1 MVP gate\]

Phases run in order of dependency, not of the priority list: browsing comes before writing, and token auth comes last because inherited git credentials already cover most setups. If a machine you use has no Git Credential Manager, move Phase 5 up to right after Phase 1.

## Architecture

One rule shapes the whole design: the UI thread never touches git. Reads use gix on background threads and come back as immutable snapshots. Writes are Jobs that run the `git` CLI, and a Local write Job always runs to completion. A file watcher invalidates the snapshots that are affected.

&#91;embedded content: architecture · UI, repo service, outside world\]

UI intent reaches git only through the Repo service. Everything that flows back up to the UI is an immutable snapshot. See [ADR 0001](docs/adr/0001-repo-crate-runs-jobs-on-its-own-threads.md) and [ADR 0002](docs/adr/0002-local-write-jobs-always-run-to-completion.md).

**Crates**

| Crate | Owns | Depends on |
| --- | --- | --- |
| `core` | Domain types (`Oid`, `RefName`, `BranchInfo`, `CommitRow`, `FileStatus`), errors | nothing |
| `read` | gix queries: refs, revwalk, status, tree and blob diff | core, gix |
| `write` | git CLI runner: spawn, environment, porcelain parsing, progress | core |
| `graph` | Lane layout from a commit stream, incremental and cacheable | core |
| `diff` | Line and word diff, highlighting of visible lines | core, gix-diff, syntect |
| `auth` | Token store and git credential-helper mode | core, keyring |
| `repo` | Repo service: Intents in, Snapshots out. Privately owns the Job scheduler, watcher, CLI fallback and cache access | core, read, write, graph, diff, notify, redb |
| `app` | GPUI binary: windows, panels, actions, keymaps, and the bridge that drains Snapshots | all of the above, gpui, gpui-component |

Only `app` depends on GPUI. Every other crate can be tested headless, and they all survive a UI framework swap. That's the hedge against GPUI risk (see Risks). A swap costs the whole UI layer but no git behaviour.

**Key decisions**

| Decision | Choice | Why |
| --- | --- | --- |
| Read path | gix (gitoxide), pinned to an exact revision: `main` at `39e093f` or later, or the first release after 0.90.0 | Pure Rust, `ThreadSafeRepository` handles that can be shared across threads, fast commit-graph walks. 0.90.0 lacks `rev_walk_topo` and status fixes Cairn needs, and each 0.x release is a semver break |
| Write path | `git` CLI | Hooks, signing, credential helpers and rebase/merge semantics come free and match the user's config |
| Not used | libgit2 / git2 | Adds a C dependency, has slower status on large trees, and lags behind upstream git |
| UI | `gpui-component =0.9.0` or its umbrella crate `gpui-kit =0.9.0`, which pin `gpui-pre =0.3.11` (a snapshot of zed@f816e1f), with a committed `Cargo.lock`. Rust 1.98.1 in `rust-toolchain.toml`. Re-check the pin when the spikes start, because Zed has said automated weekly GPUI releases are coming | gpui has no standalone text input. gpui-component supplies Textarea, Input, VirtualList, DataTable and Tree. The 0.5.x line takes crates.io `gpui` 0.2.2 from October 2025, which predates Zed's Windows dead-key and IME rework |
| Diff algorithm | The repo's `diff.algorithm` (Myers by default), through `gix-diff`'s vendored imara-diff. `patience` goes to the CLI | Diffs must match `git diff`, and git and gix both default to Myers. The Myers and slider parity fixes live in that fork, and gix doesn't implement patience |
| Highlighting | syntect for the MVP, visible lines only, cached per blob oid | Broad grammar coverage with no setup. Move to tree-sitter only if profiling demands it |
| Watching | notify, with no debouncer | Cross-platform. Slots collapse event bursts, so a debounce timer would only add latency ([ADR 0004](docs/adr/0004-watcher-coalesces-through-the-slot.md)) |
| Cache | redb | Embedded pure-Rust KV store for graph layout and commit metadata |
| Async | No tokio in the MVP. Jobs run on threads owned by `repo`; GPUI executors are for UI-side async only | Git work blocks, so dropping a GPUI `Task` can't stop it, and a pool we own can be capped. Keeps the scheduler headless and out of the UI swap |

**gix rules**

- Don't use `repo.status()` for the fast path: it rebuilds an index from `HEAD^{tree}` on every call. Run HEAD vs index only when `.git/index` or HEAD changes, and scope index vs worktree to the watcher's paths.
- Never call `Outcome::write_changes()`. It drops the fsmonitor and untracked-cache extensions and dissolves a split index. Use `git update-index -q --refresh` when stat-dirty entries stay high, as a Local write Job, because it takes `index.lock`.
- Keep the default `FastEq` comparison. `HashEq` disagrees with git.
- Open with `permissions.config.git_binary = true`, or pass the config paths in, so gix sees the same system config as the `git` binary.

**CLI fallback** has three scopes instead of one per-repo switch:

| Scope | Trigger |
| --- | --- |
| Per repo, at open | Sparse index, partial clone, reduced trust or `core.safecrlf=true`. For diff only, `diff.algorithm=patience` |
| Per call | Any `Err` from open or status, with the repo latched to the CLI after repeated failures |
| Per result | A staged rename with similarity below 100%, because gix scores and pairs renames differently |
| Per repo, by measurement | gix misses its floor on a repo where `core.fsmonitor` or `core.untrackedCache` is on. gix ignores both, so the CLI may be faster. Thresholds come from the spikes |

**Data flow**

1. A user action becomes a GPUI action, and `app` turns it into an Intent for the Repo service, which turns it into one or more Jobs.
2. A Read Job carries a generation token and runs in the Interactive lane or the Background lane. A newer Read Job for the same Slot Supersedes the older one. A Slot holds at most one running and one pending Job.
3. Network Jobs and Local write Jobs each have a queue per repo, run one at a time within it, and may overlap each other. Read Jobs overlap both. A Local write Job is never stopped once started, and a Network Job stops only when the user asks.
4. The result is an `Arc` snapshot. A coalesced bridge in `app` drains results at most once per frame, drops stale generations, swaps the snapshots into GPUI entities, and calls `cx.notify()` only for entities that changed.
5. The watcher observes `.git/` (HEAD, refs, packed-refs, index, FETCH\_HEAD) and the worktree. Its first event starts a refresh of only the affected snapshot (refs, status or both) straight away. Events that arrive while that refresh runs leave one pending refresh in the same Slot, so a burst costs at most two refreshes and there is no debounce timer ([ADR 0004](docs/adr/0004-watcher-coalesces-through-the-slot.md)).
6. Every Local write Job, and any stopped Network Job, ends with a targeted refresh. There are no full reloads.
7. Quit drops Read Jobs and stops Network Jobs, asking first for a push. It waits for Local write Jobs, because exiting closes git's pipes and can kill it mid-step. A warned Force quit is the only exception.

## Phase 0: Spike and foundations

Phase 0 tests whether GPUI and gix carry their weight on both macOS and Windows before anything else gets built. Size: about 8–12 focused days, because each spike runs on both OSes.

- [ ] Cargo workspace with the eight crates. CI runs fmt, clippy and test on macOS and Windows.
- [ ] Pin Rust, gpui-component and `gpui-pre`. Build a window with 1.5M rows (the kernel fixture's size) using `uniform_list` plus `canvas`, painting graph lanes, nodes and edges, not just text. Measure presented frames, with PresentMon on Windows, because paint timers can read healthy while the screen is frozen. The scroll Floor covers the top 100k rows. Past that is recorded only: `f32` scroll offsets step by 2 px past row ~700k and 4 px past row ~1.4M at 24 px (derived, not measured). Zed's own git graph is a reference only, because `git_ui` is GPL.
- [ ] Commit message spike: gpui-component's `Textarea` (`Input` is single-line from 0.6.0) with IME, dead keys, undo and copy/paste, tested separately on each OS because IME handling is platform-specific. Include Cmd-Return during a dead key, IME near the screen bottom, CRLF and Excel paste, and a held key under background notifies, at the coalesced rate of one drain per frame as well as at the worst case. A git client is unusable without it.
- [ ] gix spike: open the Linux kernel repo and time history walks with and without a commit-graph file, separately for HEAD only and all refs, because the topo walk goes down to the lowest tip's generation before yielding anything. On its worktree, time HEAD vs index, watcher-scoped index vs worktree and full status separately, and compare against git. Do all of it on macOS and Windows.
- [ ] git CLI runner: `GIT_TERMINAL_PROMPT=0`, `LC_ALL=C`, `--no-pager`, `--no-optional-locks` on every read, porcelain formats wherever they exist, stderr captured for errors and `--progress`.
- [ ] Windows process handling: spawn git with `CREATE_NO_WINDOW` so no console windows flash, measure process spawn cost, and test paths with spaces and paths over 260 characters.
- [ ] Job scheduler inside the `repo` crate: Job classes, per-repo queues, lanes, Slots and generation tokens, as specified in ADR 0001 and ADR 0002.
- [ ] Benchmark harness: criterion for the core crates, `tracing` spans in the app, and two fixture repos (small, Linux kernel) available on both OSes.

**Gate:** on both macOS and Windows, the UI framework meets its Floors for scrolling, key input, real typing, launch and memory, and gix meets its Floors for history walks and status. The Floors are in [#8](https://github.com/skolldev/cairn/issues/8#issuecomment-6685134359) and don't move once results are in. Each one names the component a miss condemns: a framework miss counts against GPUI, while a gix miss sends that path to the CLI and leaves GPUI unaffected.

**Kill switch:** if GPUI basics (text input, scrolling, focus) block you for more than two weeks on either OS, GPUI is a no-go, and every crate except `app` is kept. A failed result starts that window, which is shared across all failures on that OS, and it becomes a Floor miss when the window runs out. Tauri 2 + Angular is not adopted automatically: it must pass the same spikes against the same floors first. Research says it probably can't meet launch or 8 ms scrolling on macOS, so adopting it would mean accepting different budgets, which is a product decision. The failure type matters: a webview fixes text input and IME failures, but is unlikely to rescue a scrolling failure on macOS.

## Phase 1: Repos, branches and checkout

After Phase 1, the app is a live branch browser you can keep open next to your terminal. Start dogfooding it here. Size: about 6–8 days.

- [ ] Open a repo from a folder picker. Keep a persisted recent-repos list.
- [ ] Sidebar sections: Local, Remotes (grouped by remote), Tags, Stashes. `feature/x` names show as folders, as in GitKraken. A filter box sits on top.
- [ ] Local branches show ahead/behind counts against their upstream. Compute them lazily for visible rows only, cached by (local oid, upstream oid).
- [ ] Double-click a local branch to run `git switch <name>`.
- [ ] Double-click a remote branch to run `git switch --track <remote>/<name>`, or check out the existing local branch if one already tracks it.
- [ ] Dirty worktree: show git's own message, plus a "Stash and switch" action.
- [ ] Detached checkout of any commit: `git switch --detach <sha>`.
- [ ] Fetch all remotes (`git fetch --all --prune --progress`) with the user's existing credentials. See Phase 5 for why this already works for most setups.
- [ ] Watcher: a `git checkout` run in the terminal shows up in the sidebar.

**Gate:** a branch switched in the terminal meets the External change Budget, Cairn's own checkouts meet the Local write Job Budget, and fetch works against your GitHub and Azure DevOps repos.

## Phase 2: Commit graph

The graph is the hardest UI piece and the main reason people stay with GitKraken, so it gets the largest share of time. Size: about 10–15 days.

**Data (`graph` crate)**

- [ ] Revwalk from all refs, HEAD and (optionally) stashes, in topological order with commit date as the tie-breaker.
- [ ] Use the commit-graph file for generation numbers. If it's missing, say so on the first screen and offer to run `git commit-graph write --reachable` once, as a Local write Job.
- [ ] First open paints HEAD's history first, then merges the other tips in behind it without moving HEAD's row on screen.
- [ ] Streaming lane assignment: each row is a node column plus incoming and outgoing edges per lane. Store it compactly (u16 columns, small edge vectors) and give each branch line a stable colour.
- [ ] Compute the first \~5,000 rows synchronously and stream the rest. In the MVP, a ref change triggers a full recompute in the background. Optimise only if it measures slow.
- [ ] Cache the layout in redb, keyed by a hash of the tip oids, so warm opens skip layout.

**Rendering (`app`)**

- [ ] A custom GPUI element that paints only the visible rows: edges as paths, commits as circles, lanes capped in width with a collapse marker.
- [ ] Columns: graph, ref badges (local, remote, tag, HEAD), message, author, date, short sha.
- [ ] A WIP row on top when the worktree is dirty.
- [ ] Selecting a commit opens the details panel: message, author, parents and changed files (tree diff against the first parent, with rename detection).
- [ ] Jump to a commit by sha prefix. Message search runs as a background job.
- [ ] Toggle to hide remote-only branches. Large repos need it.

**Gate:** the Linux kernel meets the Cached open, First open, Graph scrolling, Select commit and Memory Budgets.

## Phase 3: Diff viewer

A diff has to agree with `git diff` exactly, so correctness tests come before visual polish. Size: about 8–10 days.

- [ ] File lists. Commit: tree-to-tree diff with rename detection. WIP: staged (HEAD vs index) and unstaged (index vs worktree) from gix status.
- [ ] Worktree comparisons go through gix's filter pipeline (autocrlf, `.gitattributes`, clean filters such as LFS). Without it, every Windows-touched repo shows phantom changes.
- [ ] Line diff with the repo's `diff.algorithm` through `gix-diff`, plus word-level highlights inside changed line pairs. Keep line terminators, or a CRLF-to-LF change reads as no change.
- [ ] Unified and side-by-side views, collapsible unchanged regions, virtualized for 50k-line files.
- [ ] Syntax highlighting with syntect for visible lines only, cached per blob oid.
- [ ] Binary and oversized files get a stub ("binary, 2.4 MB"). Respect `-diff` in `.gitattributes`.
- [ ] Ignore-whitespace toggle.
- [ ] Golden tests: for each fixture repo, `--numstat` totals must match `git diff --numstat` and full hunks must match `git diff`, since numstat says nothing about hunk placement.

**Gate:** golden tests pass on all fixtures, and the Select file Budget is met.

## Phase 4: Common git operations

Every mutation in this phase is a thin UI over one `git` command, except Pull, and the work is mostly state handling and error display. Size: about 10–14 days.

| Operation | Command | Notes |
| --- | --- | --- |
| Stage / unstage file | `git add -- <path>` / `git restore --staged -- <path>` | Multi-select in the file list |
| Stage / unstage hunk or line | `git apply --cached [--reverse]` with a patch built from the diff | The same approach lazygit uses. Test the patch builder heavily |
| Discard changes | `git restore -- <path>` or a reverse patch on the worktree | Always asks for confirmation |
| Commit / amend | `git commit -F -` (message on stdin) / `--amend` | Hooks run as normal. Hook output shows on failure |
| Push | `git push [-u <remote> <branch>]`, `--force-with-lease` behind a confirm | Uses inherited credentials until Phase 5 |
| Pull | `git fetch <remote>` as a Network Job, then `git merge --ff-only @{upstream}` by default or `git rebase @{upstream}` as an option, as a Local write Job | No surprise merge commits. The second Job starts only if the fetch succeeded. Disabled with no upstream. Golden tests against the real `git pull` |
| Branch create / rename / delete | `git branch`, `git branch -m`, `git branch -d` (`-D` behind a confirm) | Right-click menus on the sidebar and the graph |
| Merge | `git merge [--no-ff] <branch>` | On conflict, show the banner (below) |
| Rebase onto | `git rebase <target>` | Non-interactive only. Continue / skip / abort |
| Cherry-pick / revert | `git cherry-pick <sha>` / `git revert <sha>` | From the graph context menu |
| Reset | `git reset --soft/--mixed/--hard <sha>` | Hard reset asks for confirmation |
| Stash | `git stash push [-u] -m`, `apply`, `pop`, `drop` | Stashes appear in the sidebar and the graph |
| Tags | `git tag -a`, `git tag -d` | Pushing tags is a checkbox on push |

**In-progress states.** Read the repository state (merge, rebase, cherry-pick, revert) through gix. While one is active, show a banner with its conflicted files and actions: Open mergetool (`git mergetool <path>`), Mark resolved (`git add`), Continue, Abort.

**Gate:** a complete feature-branch cycle with no terminal: branch, commit, rebase onto main, resolve a conflict through the mergetool, and push.

## Phase 5: Token auth for GitHub and Azure DevOps

Auth comes in two layers. The first costs nothing and ships in Phase 1. The second is a small credential helper, so this phase is short and can move up to right after Phase 1 if a machine you use has no Git Credential Manager. Size: about 3–5 days.

**Layer 1: inherit what git already has (Phase 1).** Every network operation goes through the `git` CLI, so Git Credential Manager (which handles both GitHub and Azure DevOps), SSH keys and ssh-agent work with zero code. The app sets `GIT_TERMINAL_PROMPT=0` so git never hangs waiting for a terminal. It recognises auth failures in stderr and offers "Add token".

**Layer 2: app-managed tokens.**

- [ ] Token store: the `keyring` crate (macOS Keychain, Windows Credential Manager, Secret Service). Keyed by host for GitHub and by host + organisation for Azure DevOps.
- [ ] The app binary doubles as a git credential helper (`app credential get`) that speaks git's credential protocol: it reads `protocol`/`host`/`path` on stdin and writes `username`/`password`.
- [ ] Injection per invocation, only for hosts with a stored token: `-c credential.helper=` (clears the user's helpers) then `-c credential.helper="!<app> credential"`. Add `-c credential.https://dev.azure.com.useHttpPath=true` so the helper sees the organisation in the path. On Windows, git runs `!` helpers through its bundled shell, so test app paths that contain spaces and backslashes.
- [ ] Tokens never appear in remote URLs, arguments, logs or `.git/config`.
- [ ] GitHub: a fine-grained PAT with Contents read/write. Azure DevOps: a PAT with the Code (Read & Write) scope. Over HTTPS, both take any non-empty username.
- [ ] Validate on save with `git ls-remote --heads <url>` through the helper. It's provider-agnostic and needs no HTTP client.
- [ ] Settings screen: list hosts, show which layer each one uses, delete a token.
- [ ] Auto-fetch on an interval: off by default, 5 minutes when on.

SSH remotes stay on ssh-agent. Managing SSH keys in the app is out of scope.

**Gate:** on a machine without GCM, add a GitHub token and an Azure DevOps token, then fetch and push to both. A wrong token produces a clear error within 5 seconds, with no hang.

## Later phases

Nothing below starts until the MVP gate is met. Order them by what you miss most while dogfooding.

| Phase | Scope | Notes |
| --- | --- | --- |
| 6: Workflow | Drag-and-drop branch onto branch for merge/rebase, interactive rebase editor, undo/redo, repo tabs, command palette | The interactive rebase editor sets `GIT_SEQUENCE_EDITOR` to the app, which edits the todo list in a UI. Undo is reflog-based |
| 7: Conflicts | Built-in 3-way merge editor | The biggest single feature after the graph. Until then, the external mergetool covers it |
| 8: Providers | PR list and create, CI status on commits, avatars for GitHub and Azure DevOps. GitHub OAuth device flow replaces PATs | Brings in the first HTTP client (`ureq` or `reqwest`) |
| 9: Power tools | Blame, file history, worktrees, submodules, LFS status, signing setup, sparse checkout, multi-repo workspaces | Pick à la carte |
| Never | AI commit messages, AI anything, telemetry, accounts | The reason this project exists |

## Performance budgets

These numbers are the definition of "blazing fast" for this project. Each Budget is the number an interaction must reach for a phase gate to pass, and this table is the only place Budgets are written down: `PRODUCT.md` and the phase gates name them without restating numbers. They were rewritten from the Phase 0 measurements (#7, #9, #11, #12, #13). Passing the Phase 0 gate was judged against [the Floors in #8](https://github.com/skolldev/cairn/issues/8#issuecomment-6685134359), not against these.

**Rules**

- **Evidence:** a Budget is set from measurements. A row with no measurement behind it is **unproven**: it carries a number to design toward, but gates nothing until the phase named in its row measures it on both OSes. A Budget that misses moves only on a new measurement, never in advance.
- **Both OSes, one number:** every Budget applies to macOS and Windows alike, and the slower OS on that row sets it. A row splits by OS only for a physical reason written into the row.
- **Statistic:** latency rows pass at p95, over at least 20 samples per run, worst of three runs, with the worst sample recorded. Launch and scrolling define their own statistic in the row.
- **Method:** as the Floors define it. Presented frames, never paint timers, with PresentMon on Windows. Memory is `phys_footprint` on macOS and private working set on Windows, summed across the process tree, with dedicated GPU memory recorded separately. The OS file cache is warm unless a row says otherwise.
- **Fixtures:** a small repo and the Linux kernel (about 1.5M commits, with a commit-graph).

| Interaction | Budget | Fixture | Status |
| --- | --- | --- | --- |
| Launch, process start to the first presented frame of Cairn's own UI (no repo content required) | median < 300 ms, none over 400 ms, across 20 warm launches | any | Proven on macOS (239 ms median, framework only). Windows needs a PresentMon trace (Phase 1) |
| Cached open, from the Intent to the first graph screen, including a restore at startup | < 200 ms | Linux kernel | Unproven (Phase 2) |
| First open of a repo (no Cairn cache), to the first screen of HEAD's history | < 1 s | Linux kernel | Unproven (Phase 2) |
| Graph scrolling, at 120 Hz, over the top 100k rows, in the Floor's five scenarios | at least 99.5% of presented intervals within 12.5 ms, no gap over 25 ms | Linux kernel | Proven on Windows. macOS needs an Instruments cross-check (Phase 2) |
| Select commit to file list, settled (merges diffed against the first parent) | < 50 ms | Linux kernel | Unproven (Phase 2) |
| Select file to first diff screen, settled with highlighting, for files under 10k lines | < 50 ms | any | Unproven (Phase 3) |
| External change to UI updated: from a file saved, or a `git` command run outside Cairn having exited, to the updated status, sidebar and graph | < 100 ms | Linux kernel | Unproven (Phase 1) |
| Intent to UI updated after a Local write Job, such as Commit pressed to the updated graph and status | < 100 ms | small repo | Unproven (Phase 1). Needs the tuned CLI runner: a four-spawn commit is about 50 ms tuned and 125 ms naive on Windows |
| Memory, settled after the scroll scenarios plus 10 s, at a 1728x1117 logical window, with commit metadata resident | < 500 MB | Linux kernel | Unproven (Phase 2). The framework and delta row store measured 248 MB on macOS and 37 MB on Windows |

**Recorded only, never gating:**

- Startup restore of the last repo, launch and cached open together. It keeps a skeleton launch honest.
- The full graph settling after HEAD's first screen.
- Peak memory while scrolling (about 467 MB on macOS in the delta format).
- Opens with the OS file cache purged.
- Merges touching more than N files, with N chosen in Phase 2 from the kernel's real distribution.
- A first open with no commit-graph. `git rev-list` needs about 8 s for the kernel, and no design gets topo order much faster without a graph. Cairn shows that it's building one and offers the commit-graph write on the first screen.
- Rare 110–190 ms Windows spawn outliers. p95 absorbs them while they stay under 5% of samples.

**What these Budgets require of the design:**

- The watcher has no debounce timer. Its first event starts a refresh, and the Slot coalesces the rest. See [ADR 0004](docs/adr/0004-watcher-coalesces-through-the-slot.md).
- The tuned CLI runner from #13 is a Budget requirement, not a suggestion.
- Launch does nothing slow before the first frame. Opening gix, redb and the last repo runs alongside window creation or after it.
- The first open paints HEAD's history first and merges the other tips in behind it, keeping HEAD's row in place on screen.
- Loading has no commit cap, and commit metadata stays resident for every loaded commit.

**How to measure:** criterion benches for the core crates, run in CI with a regression threshold. `tracing` spans plus a debug-build frame-time overlay in the app. Before each phase gate, do a side-by-side stopwatch run against GitKraken and Sublime Merge on the same fixtures, on both OSes.

## Risks and mitigations

The biggest risk isn't technical. It's a side project that never gets good enough to become your daily driver. Most mitigations below exist to get you dogfooding early.

| Risk | Impact | Mitigation |
| --- | --- | --- |
| Scope creep, lost momentum | Never replaces GitKraken | Strict MVP gate. Use it read-only from Phase 1, next to the terminal |
| GPUI API churn, or Zed deprioritising GPUI ([community fork gpui-ce exists](https://github.com/gpui-ce/gpui-ce/), but moving to it likely means giving up gpui-component) | Breakage on every upgrade | Pin gpui-component and `gpui-pre` exactly and commit `Cargo.lock`. Keep every crate except `app` UI-free. Upgrade deliberately, not continuously |
| `gpui-pre` is a Zed snapshot republished under one crates.io owner | The UI stack rests on one person's account | Move to Zed's own releases when they land. Re-check the pin before each upgrade |
| [zed#61469](https://github.com/zed-industries/zed/issues/61469): on Windows, sustained input plus background notifies can stop frames being presented for seconds | Freezes while typing, invisible to paint timers | Coalesce notifies through the bridge, which caps the rate but doesn't remove the bug. Measure presented frames with PresentMon. Test the reported app-level watchdog in Phase 0 |
| GPUI widget gaps (IME, accessibility) | A commit box that fights you | Use gpui-component's `Textarea`. Phase 0 kill switch, under which Tauri 2 + Angular must pass the same spikes first |
| GPUI is less mature on Windows than on macOS | A second-class Windows build, or Windows-only blockers late in the project | Run every Phase 0 spike and gate on both OSes. A Windows failure is a no-go for GPUI |
| Windows file and process overhead (NTFS, antivirus scanning, process spawn cost) | Status and write Budgets missed on Windows only | Measured in Phase 0 (#11, #12). The tuned CLI runner is required, and Windows sets the write-path Budget |
| gix behaves differently from git (filters, autocrlf, sparse index, fsmonitor) | Phantom changes, wrong status | Golden tests against `git status --porcelain=v2`, `git diff --numstat` and full `git diff` hunks. Three-scope CLI fallback (see Architecture) |
| CLI output differs across git versions | Broken write path | Porcelain formats, `LC_ALL=C`, a minimum git version (ADR 0007), CI against the oldest version you support |
| Patch builder for line staging corrupts the index | Lost work, lost trust | Property tests: stage then unstage must round-trip to the original diff |
| Your client's Azure DevOps org restricts PATs | Layer 2 auth unusable there | Layer 1 (GCM with Entra sign-in) needs no PAT |
| Lane explosion in repos with hundreds of branches | Unreadable, slow graph | Lane cap with collapse, hide-remote toggle, filter graph by branch |

## Sources

- [gpui-component on docs.rs](https://docs.rs/crate/gpui-component/latest) (VirtualList, Input, Textarea, DataTable)
- [gpui-kit](https://github.com/longbridge/gpui-kit), the renamed gpui-component repo
- Phase 0 research: [GPUI state](https://github.com/skolldev/cairn/blob/research/gpui-state/docs/research/gpui-state.md), [gix fidelity](https://github.com/skolldev/cairn/blob/research/gix-fidelity/docs/research/gix-fidelity.md), [Tauri fallback](https://github.com/skolldev/cairn/blob/research/tauri-fallback/docs/research/tauri-fallback.md)
- [gitoxide crate status](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)
- [GPUI book: licensing and missing text input](https://vanuan.github.io/gpui-book/appendix_a.html)
- [gpui-ce community fork](https://github.com/gpui-ce/gpui-ce/)
