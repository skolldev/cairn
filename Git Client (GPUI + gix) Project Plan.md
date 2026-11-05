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

One rule shapes the whole design: the UI thread never touches git. Reads use gix on background threads and come back as immutable snapshots. Writes are cancellable jobs that run the `git` CLI. A file watcher invalidates the snapshots that are affected.

&#91;embedded content: architecture · UI, repo service, outside world\]

UI intent reaches git only through the job scheduler. Everything that flows back up to the UI is an immutable snapshot.

**Crates**

| Crate | Owns | Depends on |
| --- | --- | --- |
| `core` | Domain types (`Oid`, `RefName`, `BranchInfo`, `CommitRow`, `FileStatus`), errors, job trait | nothing |
| `read` | gix queries: refs, revwalk, status, tree and blob diff | core, gix |
| `write` | git CLI runner: spawn, environment, porcelain parsing, progress | core |
| `graph` | Lane layout from a commit stream, incremental and cacheable | core |
| `diff` | Line and word diff, highlighting of visible lines | core, imara-diff, syntect |
| `auth` | Token store and git credential-helper mode | core, keyring |
| `app` | GPUI binary: windows, panels, actions, keymaps | all of the above, gpui, gpui-component |

Only `app` depends on GPUI. Every other crate can be tested headless, and they all survive a UI framework swap. That's the hedge against GPUI risk (see Risks).

**Key decisions**

| Decision | Choice | Why |
| --- | --- | --- |
| Read path | gix (gitoxide) | Pure Rust, `ThreadSafeRepository` handles that can be shared across threads, fast commit-graph walks |
| Write path | `git` CLI | Hooks, signing, credential helpers and rebase/merge semantics come free and match the user's config |
| Not used | libgit2 / git2 | Adds a C dependency, has slower status on large trees, and lags behind upstream git |
| UI | gpui, pinned to the rev that gpui-component pins, plus gpui-component 0.5.x | gpui has no standalone text input. gpui-component supplies Input, VirtualList, DataTable and Tree |
| Diff algorithm | imara-diff (histogram) | The same engine gix uses. Fast and git-compatible |
| Highlighting | syntect for the MVP, visible lines only, cached per blob oid | Broad grammar coverage with no setup. Move to tree-sitter only if profiling demands it |
| Watching | notify + notify-debouncer-full | Cross-platform; debouncing collapses event bursts |
| Cache | redb | Embedded pure-Rust KV store for graph layout and commit metadata |
| Async | GPUI executors only, no tokio in the MVP | One scheduler. Git processes run on background tasks |

**Data flow**

1. A user action becomes a GPUI action, and the action becomes a job.
2. The job runs on the background executor and carries a generation token. A newer job of the same kind (for example, selecting another commit) cancels the older one.
3. The result is an `Arc` snapshot. It's swapped into a GPUI entity, followed by `cx.notify()`.
4. The watcher observes `.git/` (HEAD, refs, packed-refs, index, FETCH\_HEAD) and the worktree. After a 100–200 ms debounce, it refreshes only the affected snapshot: refs, status or both.
5. Every write job ends with a targeted refresh. There are no full reloads.

## Phase 0: Spike and foundations

Phase 0 tests whether GPUI and gix carry their weight on both macOS and Windows before anything else gets built. Size: about 8–12 focused days, because each spike runs on both OSes.

- [ ] Cargo workspace with the seven crates. CI runs fmt, clippy and test on macOS and Windows.
- [ ] Pin gpui and gpui-component. Build a window with a 1M-row VirtualList and confirm smooth scrolling on both OSes.
- [ ] Commit message spike: gpui-component's multi-line Input with IME, undo and copy/paste, tested separately on each OS because IME handling is platform-specific. A git client is unusable without it.
- [ ] gix spike: open the Linux kernel repo, walk all of its history (with and without a commit-graph file), and time it. Run status on your work monorepo. Do both on macOS and Windows.
- [ ] git CLI runner: `GIT_TERMINAL_PROMPT=0`, `LC_ALL=C`, `--no-pager`, porcelain formats wherever they exist, stderr captured for errors and `--progress`.
- [ ] Windows process handling: spawn git with `CREATE_NO_WINDOW` so no console windows flash, measure process spawn cost, and test paths with spaces and paths over 260 characters.
- [ ] Job scheduler with generation tokens and cancellation.
- [ ] Benchmark harness: criterion for the core crates, `tracing` spans in the app, and three fixture repos (small, Linux kernel, work monorepo) available on both OSes.

**Gate:** on both macOS and Windows, the 1M-row list scrolls without dropped frames, the commit box handles real typing, and the history walk fits the budget in Performance budgets.

**Kill switch:** if GPUI basics (text input, scrolling, focus) block you for more than two weeks on either OS, keep every crate except `app` and build the UI in Tauri 2 + Angular instead.

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

**Gate:** a branch switched in the terminal appears in the sidebar within 200 ms, and fetch works against your GitHub and Azure DevOps repos.

## Phase 2: Commit graph

The graph is the hardest UI piece and the main reason people stay with GitKraken, so it gets the largest share of time. Size: about 10–15 days.

**Data (`graph` crate)**

- [ ] Revwalk from all refs, HEAD and (optionally) stashes, in topological order with commit date as the tie-breaker.
- [ ] Use the commit-graph file for generation numbers. If it's missing, offer to run `git commit-graph write --reachable` once.
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

**Gate:** the Linux kernel graph shows its first screen within budget, scrolling never drops a frame, and selecting a commit feels instant.

## Phase 3: Diff viewer

A diff has to agree with `git diff` exactly, so correctness tests come before visual polish. Size: about 8–10 days.

- [ ] File lists. Commit: tree-to-tree diff with rename detection. WIP: staged (HEAD vs index) and unstaged (index vs worktree) from gix status.
- [ ] Worktree comparisons go through gix's filter pipeline (autocrlf, `.gitattributes`, clean filters such as LFS). Without it, every Windows-touched repo shows phantom changes.
- [ ] Line diff with imara-diff (histogram), plus word-level highlights inside changed line pairs.
- [ ] Unified and side-by-side views, collapsible unchanged regions, virtualized for 50k-line files.
- [ ] Syntax highlighting with syntect for visible lines only, cached per blob oid.
- [ ] Binary and oversized files get a stub ("binary, 2.4 MB"). Respect `-diff` in `.gitattributes`.
- [ ] Ignore-whitespace toggle.
- [ ] Golden tests: for each fixture repo, `--numstat` totals must match `git diff --numstat`.

**Gate:** golden tests pass on all fixtures, and a 10k-line file shows its first screen within budget.

## Phase 4: Common git operations

Every mutation in this phase is a thin UI over one `git` command, and the work is mostly state handling and error display. Size: about 10–14 days.

| Operation | Command | Notes |
| --- | --- | --- |
| Stage / unstage file | `git add -- <path>` / `git restore --staged -- <path>` | Multi-select in the file list |
| Stage / unstage hunk or line | `git apply --cached [--reverse]` with a patch built from the diff | The same approach lazygit uses. Test the patch builder heavily |
| Discard changes | `git restore -- <path>` or a reverse patch on the worktree | Always asks for confirmation |
| Commit / amend | `git commit -F -` (message on stdin) / `--amend` | Hooks run as normal. Hook output shows on failure |
| Push | `git push [-u <remote> <branch>]`, `--force-with-lease` behind a confirm | Uses inherited credentials until Phase 5 |
| Pull | `git pull --ff-only` by default, rebase as an option | No surprise merge commits |
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

These numbers are the definition of "blazing fast" for this project. They're targets set up front, not measurements, so revise them after the Phase 0 spikes, not after the features ship. Every budget applies on both macOS and Windows. Fixtures: a small repo, the Linux kernel (1M+ commits) and your work monorepo.

| Interaction | Target | Fixture |
| --- | --- | --- |
| Launch to first window | < 150 ms | any |
| Open repo to first graph screen, warm cache | < 200 ms | Linux kernel |
| Open repo to first graph screen, cold | < 1 s | Linux kernel |
| Graph scrolling | no frame over 8 ms | Linux kernel |
| Select commit to file list | < 50 ms | Linux kernel |
| Select file to first diff screen | < 50 ms for files under 10k lines | any |
| Worktree save to updated status | < 150 ms (watcher debounce included) | work monorepo |
| Git command finishes to UI updated | < 100 ms | any |
| Memory with graph loaded | < 300 MB | Linux kernel |

**How to measure:** criterion benches for the core crates, run in CI with a regression threshold. `tracing` spans plus a debug-build frame-time overlay in the app. Before each phase gate, do a side-by-side stopwatch run against GitKraken and Sublime Merge on the same fixtures, on both OSes.

## Risks and mitigations

The biggest risk isn't technical. It's a side project that never gets good enough to become your daily driver. Most mitigations below exist to get you dogfooding early.

| Risk | Impact | Mitigation |
| --- | --- | --- |
| Scope creep, lost momentum | Never replaces GitKraken | Strict MVP gate. Use it read-only from Phase 1, next to the terminal |
| GPUI API churn, or Zed deprioritising GPUI ([community fork gpui-ce exists](https://github.com/gpui-ce/gpui-ce/)) | Breakage on every upgrade | Pin the rev through gpui-component. Keep every crate except `app` UI-free. Upgrade deliberately, not continuously |
| GPUI widget gaps (IME, accessibility) | A commit box that fights you | Use gpui-component's Input. Phase 0 kill switch to Tauri 2 + Angular |
| GPUI is less mature on Windows than on macOS | A second-class Windows build, or Windows-only blockers late in the project | Run every Phase 0 spike and gate on both OSes. A Windows failure counts toward the kill switch |
| Windows file and process overhead (NTFS, antivirus scanning, process spawn cost) | Status and write budgets missed on Windows only | Measure status and git process round trips on the work monorepo on Windows in Phase 0, then revise the budgets from real numbers |
| gix behaves differently from git (filters, autocrlf, sparse index, fsmonitor) | Phantom changes, wrong status | Golden tests against `git status --porcelain=v2` and `git diff --numstat`. Per-repo fallback to CLI status |
| CLI output differs across git versions | Broken write path | Porcelain formats, `LC_ALL=C`, a minimum git version (2.23+ for `switch`/`restore`), CI against the oldest version you support |
| Patch builder for line staging corrupts the index | Lost work, lost trust | Property tests: stage then unstage must round-trip to the original diff |
| Your client's Azure DevOps org restricts PATs | Layer 2 auth unusable there | Layer 1 (GCM with Entra sign-in) needs no PAT |
| Lane explosion in repos with hundreds of branches | Unreadable, slow graph | Lane cap with collapse, hide-remote toggle, filter graph by branch |

## Sources

- [gpui-component on docs.rs](https://docs.rs/crate/gpui-component/latest) (VirtualList, Input, DataTable)
- [gitoxide crate status](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)
- [GPUI book: licensing and missing text input](https://vanuan.github.io/gpui-book/appendix_a.html)
- [gpui-ce community fork](https://github.com/gpui-ce/gpui-ce/)
