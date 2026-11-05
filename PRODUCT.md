# Product

<!-- impeccable:product-schema 1 -->

## Platform

desktop

Native desktop app rendered with GPUI, not a web view. macOS and Windows are equal first-class targets. Linux is best effort. This value is outside Impeccable's `web`/`ios`/`android`/`adaptive` set because none of them describes the product.

## Stack

Rust workspace of seven crates. Only `app` depends on the UI framework.

- UI: gpui, pinned to the rev gpui-component pins, plus gpui-component 0.5.x.
- Reads: gix on background threads, returned as immutable snapshots.
- Writes: the `git` CLI, as cancellable jobs.
- Diff and highlighting: imara-diff and syntect. Cache: redb. Watching: notify. Async: GPUI executors only, no tokio in the MVP.
- Kill switch: if GPUI text input, scrolling or focus block for more than two weeks in Phase 0, keep every crate except `app` and rebuild the UI in Tauri 2 + Angular.

Full plan: `Git Client (GPUI + gix) Project Plan.md`.

## Users

**Now:** one developer, the author, using Cairn as a daily driver on GitHub and Azure DevOps repos, including a work monorepo. Their job is to browse history, stage precisely, commit, sync and move between branches all day without waiting on the tool.

**Later:** other developers, through an open source release after the MVP gate. Serving them must not weaken the daily-driver work, and it is not a reason to build first-run or onboarding before the gate.

## Product Purpose

Cairn is a git client that replaces GitKraken in daily work. It keeps the parts worth copying: a branch and ref sidebar, a commit graph, a commit and WIP panel, a diff viewer, and fetch, pull and push.

Success has one test: Cairn fully replaces GitKraken for two weeks of real work on GitHub and Azure DevOps repos, on both macOS and Windows, with conflicts as the only reason to open another tool. A feature count proves nothing.

## Positioning

Cairn is the fast one. Speed is defined by published budgets measured on the Linux kernel and a real monorepo, and checked with a stopwatch against GitKraken and Sublime Merge before each phase gate. It reads through gix, writes through the real `git` CLI, and never has AI, telemetry or accounts.

When speed, absence of bloat and git fidelity conflict, speed wins. A feature that misses its budget gets cut or deferred, even if it's useful. Two limits apply: the plan's correctness gates (golden tests against git, round-trip tests on line staging) must still pass, and the Never list below is not open to trade.

## Operating Context

- Runs in long stretches beside a terminal. Changes made in the terminal must appear in the app, so the two are used together.
- Repos range from small projects to very large histories. The fixtures are a small repo, the Linux kernel (1M+ commits) and the work monorepo.
- Remotes are GitHub and Azure DevOps, reached through whatever git already has: Git Credential Manager, SSH keys, ssh-agent. App-managed tokens are a second layer.
- Merge conflicts go to the user's configured `git mergetool`. Hooks, signing and credential helpers behave exactly as they do in the terminal.
- Windows-touched repos are normal, so autocrlf, `.gitattributes` and clean filters are everyday cases.
- A client's Azure DevOps org may restrict PATs.

## Capabilities and Constraints

**MVP scope**

- Sidebar: local branches, remote branches, tags and stashes. Double-click checks out, and a remote branch becomes a tracking branch.
- Commit graph: lanes, ref badges, a WIP row on top, instant scrolling through 100k+ commits.
- Commit and WIP panel: changed files, hunk- and line-level staging, commit message box.
- Diff viewer: unified and side-by-side, with syntax highlighting.
- Network: fetch, pull and push, with token auth for GitHub and Azure DevOps.
- Common git operations, each a thin UI over one `git` command. Merge, rebase, cherry-pick and revert show an in-progress banner with Open mergetool, Mark resolved, Continue and Abort.

**Performance budgets** (targets set up front, not yet measured; revise after the Phase 0 spikes)

| Interaction | Target |
| --- | --- |
| Launch to first window | < 150 ms |
| Open repo to first graph screen | < 200 ms warm, < 1 s cold (Linux kernel) |
| Graph scrolling | no frame over 8 ms |
| Select commit to file list | < 50 ms |
| Select file to first diff screen | < 50 ms under 10k lines |
| Worktree save to updated status | < 150 ms, debounce included |
| Git command finishes to UI updated | < 100 ms |
| Memory with graph loaded | < 300 MB (Linux kernel) |

**Hard constraints**

- The UI thread never touches git.
- Anything that mutates the repo is done by the `git` CLI. Porcelain is never reimplemented.
- Tokens never appear in remote URLs, arguments, logs or `.git/config`.
- Minimum git version is 2.23.

**Not in the MVP:** PRs, issue trackers, provider avatars, a built-in conflict editor, an interactive rebase UI, drag-and-drop merge or rebase, and submodule or LFS management UI.

**Never:** AI commit messages, AI anything, telemetry, accounts.

**Open decisions**

- Name: Cairn comes from the repo name and is not yet confirmed as the public name.
- Open source licence and release timing.
- Accessibility target. The plan names IME and accessibility as GPUI widget gaps, and no standard has been set. This needs a decision before public release.
- Whether Windows parity survives the Phase 0 spike. Every phase gate and budget now applies to both OSes, and a Windows failure counts toward the kill switch.

## Brand Commitments

- Working name: Cairn (lowercase `cairn` in the repo).
- "No AI, no telemetry, no accounts" is a permanent product commitment and the stated reason the project exists.
- No logo, voice guide or identity assets exist yet.

## Evidence on Hand

- `Git Client (GPUI + gix) Project Plan.md` holds the scope, architecture, phases, budgets and risks.
- No code, benchmarks or users exist yet. Budgets are targets. Never present them as measured results or as proven wins over GitKraken or Sublime Merge.
- No testimonials, screenshots, licence, pricing or adoption figures exist. Do not invent them.

## Product Principles

1. **Speed is the feature.** Every interaction has a budget. Missing it is a defect, and the feature gives way rather than the number.
2. **Never disagree with git.** Diffs match `git diff`, writes run the real CLI, and errors show git's own words. Cairn adds visibility, not new semantics.
3. **Protect the user's work.** Destructive actions confirm, pull defaults to `--ff-only`, and a tool that loses work loses all trust, however fast it is.
4. **Nothing stands between developers and their repo.** No AI, telemetry, accounts or cloud, ever. Adding a dependency on someone else's service needs a very good reason.
5. **Daily use is the only test.** Scope follows what is missed while dogfooding, not what competitors list.
