# PROTOTYPE: gix timings spike (issue #11)

Throwaway. Not part of Cairn. Answers one question: does gix meet the gix Floors in
[#8](https://github.com/skolldev/cairn/issues/8#issuecomment-6685134359) for history walks
and status on macOS and Windows, and does it agree with `git`?

Stack: gix `main` at `bc4d813` (no release after 0.90.0 yet, and 0.90.0 lacks the topo walk
and status fixes), Rust 1.98.1. Opened with `git_binary` config on, as the plan says.

## Run

From this folder, in release mode, with GitKraken closed.

```bash
cargo run --release -- all | tee results/macos-full.log
```

That runs the walks, the kernel status timings and parity, and the filters fixture. It takes
about 8 minutes on the M3 Max. `walk`, `status` and `filters` run one part. Flags:
`--runs 3`, `--only <case>`, `--cold`, `--repo <path>`, `--git <path>`, and
`--object-cache-mb <n>` for diagnostics only.

`status` changes the `linux-graph` worktree, then restores it. It refuses to start unless
status shows only the 13 baseline paths, and it writes `results/kernel-mutation.manifest`
first. If a run dies, `cargo run --release -- restore` puts the worktree back.

Windows is a checklist for the human: [CHECKLIST.md](CHECKLIST.md).

## How it measures

- **Timing:** one warm-up run, then three timed runs, worst reported. The OS file cache is
  warm. Cold runs are recorded only, with `--cold` after a reboot or `sudo purge`.
- **gix:** each run opens the repo fresh, untimed (0.3 ms), then times the work. Status
  cases include reading the 97,358-entry index.
- **git:** the same case through the CLI, process spawn included, and with
  `--no-optional-locks` for status so it doesn't rewrite the index between runs. This is the
  CLI fallback's time, which matters when gix misses.

| Case | gix | `git` |
| --- | --- | --- |
| Walks | `rev_walk(tips).sorting(TopoOrder)`, `use_commit_graph(true or false)` | `rev-list --topo-order`, with `-c core.commitGraph=false` for no graph |
| All refs | HEAD plus every ref that peels to a commit (950 tags, 2 tag-to-tree skipped) | `--all` |
| HEAD vs index | `tree_index_status`, renames as configured | `diff-index --cached -M` |
| HEAD vs index, tree index held, recorded only | `index_from_tree` once per HEAD, untimed. Each check reads `.git/index` fresh and runs `gix::diff::index`, default renames | - |
| Watcher-scoped | `status().into_index_worktree_iter(<10 paths>)`, no untracked | `diff-files -- <10 paths>` |
| Full status | `status().untracked_files(Collapsed).into_iter()` | `status --porcelain=v2` |

The walk compares gix's commit order with `git`'s, which isn't a Floor because topo order
allows ties to differ. It was identical in every case.

**Worktree changes for status:** 10 edited `.c` files, one staged then edited again (`MM`),
a staged rename (`R100`), a staged deletion, a worktree deletion, four untracked files in
three places (one collapsing to a directory), and an ignored `.o`. Each edit replaces a middle
line and inserts one a quarter of the way in, so hunks land mid-file. The stat cache is
refreshed after the changes and past the racy-git second.

**Parity:** gix's full status is rendered as `git status --porcelain=v2` lines, modes and
hashes included, and compared as a set with `git`'s output. The 13 baseline paths are counted
separately and excluded from the result.

**Hunks:** gix's unified hunks (the blob platform in `ToGit` mode, the repo's diff algorithm,
slider heuristics, 3 lines of context) are compared with `git diff -U3` byte for byte. Line
terminators are kept, so a CRLF to LF change can't read as no change.

**Filters fixture:** rebuilt on each run in `results/filters-fixture`. `core.autocrlf=true`,
and `.gitattributes` with `text=auto`, `eol=crlf`, `eol=lf`, `-text`, `binary`, and a clean
filter (`tr a-z A-Z`). It covers files whose only change the filters undo, real edits under
each rule, a CRLF to LF change under `-text`, a binary edit, a staged CRLF edit, and untracked
files. Parity is checked twice, before and after `git update-index --refresh`.

## macOS results, 2026-11-07

M3 Max, Apple Git 2.39.3, 14 threads, nothing else from Cairn running. Raw output is in
`results/macos-full.log`, which is git-ignored, so these figures are the record.

**Fixture state, recorded, not repaired:** `linux-graph` has a two-layer commit-graph chain
(the 90 MB base plus a 25 KB layer GitKraken added), a `multi-pack-index` and two packs.
`linux-nograph` now has a full commit-graph and its own `multi-pack-index`, so it's no longer a
no-graph fixture. Every no-graph figure here turns the graph off per walk on `linux-graph`
instead, which is how the `git` reference in #8 was taken. HEAD is `05441d6`, 1,505,332 commits
across 955 refs.

### Walks, layout excluded

| Floor | Pass | gix worst | `git` worst | Result |
| --- | --- | --- | --- | --- |
| HEAD, first 5,000, graph | 100 ms | 38.6 ms | 34.2 ms | Pass |
| All refs, first 5,000, graph | 1.5 s | 1.38 s | 1.08 s | Pass |
| All refs, everything, graph | 6 s | 2.68 s | 2.01 s | Pass |
| All refs, first 5,000, no graph | 12 s | 23.22 s | 8.06 s | **Fail** |

Recorded only: HEAD first 5,000 without a graph took 23.54 s (`git` 8.24 s). HEAD everything
took 2.45 s with a graph (`git` 2.00 s) and 39.12 s without (`git` 9.13 s).

- **The no-graph miss is structural:** without a graph, gix's topo walk explores all history
  before yielding, and it does that about three times slower than `git`. HEAD alone costs the
  same as all refs. Emitting the rest costs gix another 16 s, against about 1 s for `git`.
- **All refs, first 5,000 with a graph has 8% headroom.** That's the case the cold-open
  Budget leans on.

### Status

| Floor | Pass | gix worst | `git` worst | Result |
| --- | --- | --- | --- | --- |
| HEAD vs index | 50 ms | 59.5 ms | 27.4 ms | **Fail** |
| Watcher-scoped index vs worktree, 10 paths | 30 ms | 33.4 ms | 24.7 ms | **Fail** |
| Full status with untracked files | 500 ms | 317.6 ms | 363.9 ms | Pass |

Recorded only:

| Case | gix worst | `git` worst |
| --- | --- | --- |
| Index vs worktree, whole tree, no untracked | 117.9 ms | 121.9 ms |
| Full status without untracked files | 112.6 ms | 134.7 ms |
| HEAD vs index, part: reading the index | 8.6 ms | - |
| HEAD vs index, part: building an index from `HEAD^{tree}` | 54.1 ms | - |
| HEAD vs index, HEAD tree index held (rerun) | 10.6 ms | - |
| Watcher-scoped, index already in memory (separate run) | 22.8 ms | - |

The held tree index row comes from a later full `all` rerun, logged in
`results/macos-rerun.log`, with GitKraken closed and the fixture at the same baseline. The rerun
moved no result, and parity passed everywhere again. Most rows came within 10% of the first
run. The exceptions: HEAD, first 5,000 with a graph took 31.0 ms (first run 38.6 ms), building
the index from `HEAD^{tree}` took 45.8 ms (54.1 ms), and full status with untracked files took
287.6 ms (317.6 ms). HEAD vs index was 58.6 ms and the watcher case 31.1 ms, still misses.

- **HEAD vs index misses on its tree-to-index build.** gix turns the whole `HEAD^{tree}` into a
  97,000-entry index on every call, which takes 54 ms alone. `git diff-index --cached` finishes
  in 27 ms including spawn. Its cache-tree extension is the likely reason, but that wasn't
  verified here.
- **Holding the HEAD tree index brings HEAD vs index to 10.6 ms,** under the 50 ms Floor and
  faster than `git`'s 26.1 ms in the same run. As on Windows, that includes reading
  `.git/index` fresh, and the tree index is only rebuilt when HEAD moves.
- **The watcher case misses on fixed cost, not on the 10 files.** Even with the index already
  in memory it takes 22.8 ms for 10 paths, against 117.9 ms for all 97,000. The remainder is
  setup, such as pathspec, attribute and ignore stacks, before any file is compared.
- **Full status beats `git`.** gix runs the index check and the directory walk in parallel. The
  odd result is that the whole passes while two of its parts miss on their own.

### Parity

| Check | Result |
| --- | --- |
| Kernel status, 17 lines, untracked included | Pass. 13 of 13 baseline lines also identical |
| Kernel hunks, 11 unstaged files and 1 staged | Pass. 24 hunks byte for byte |
| Filters fixture status, before refresh | Pass. 10 lines |
| Filters fixture status, after refresh | Pass. 10 lines |
| Filters fixture hunks, 9 unstaged files and 1 staged | Pass. 5 hunks, 4 empty diffs, 1 binary |

- **git's "dirty with no diff" is reproduced too.** The three files whose only change the
  filters undo (`b.crlf` LF in the worktree, `c.lf` CRLF, `e.up` lowercase under the clean
  filter) hash the same as the index and show no hunks. `git` still reports them `.M`, even
  after a refresh, and gix reports exactly the same. Cairn will show them modified with an
  empty diff, as `git` does.
- **Not covered:** renames below 100% similarity (#3 sends those to the CLI), conflicts,
  submodules, sparse index, LFS (no `git-lfs` installed), `core.safecrlf=true` and
  `working-tree-encoding`. #3 named the last two as gix's exceptions.

### Diagnostics, not results

Run separately so the Floor figures stay on gix's defaults. Log:
`results/macos-object-cache-DIAGNOSTIC.log`.

| Case | Default | 64 MB object cache | 256 MB object cache | `git` |
| --- | --- | --- | --- | --- |
| All refs, first 5,000, no graph | 23.22 s | 15.89 s | 16.29 s | 7.93 s |
| HEAD vs index | 59.5 ms | 61.0 ms | 64.2 ms | 26.5 ms |

No cache size rescues either miss. The walk improves a third and still fails, while HEAD vs
index doesn't move.

### What the misses mean

By the #8 rules, a gix miss sends that path to the CLI, and a `git` miss too would be a design
problem. `git` passes all three, so:

- **No-graph walks go to `git rev-list`,** or Cairn makes sure a graph exists. Writing one is a
  Local write Job that changes the user's repo, so that's a call for #13.
- **HEAD vs index goes to `git diff-index --cached`.** The CLI's 27 ms includes a spawn, which
  #12 measures. #13 should read these together.
- **The watcher-scoped check goes to `git diff-files`.** A spawn per watcher event is the case
  most exposed to Windows process cost, so the Windows numbers for both tools decide whether
  that's viable.

The misses are 19% (HEAD vs index) and 11% (watcher) over their Floors on macOS. Floors don't
move and gix gets no fix window, so they're misses. Both causes are in gix's call path rather
than in I/O, so a later gix may close them, but that's for the Budget rewrite, not this gate.

Not run on macOS: cold runs (need `sudo purge`) and anything with Defender or NTFS, which is
the Windows half.

## Windows results, 2026-11-07

Ryzen 7 7800X3D (8 cores, 16 threads), 64 GB, Windows 11 build 26200. Fixtures on `C:`, a
SATA SSD, NTFS. Git for Windows 2.52.0.windows.1, Rust 1.98.1, the same gix pin. Defender
real-time protection on in its normal state. Its exclusions (recorded in the GitKraken baseline
spike) don't cover the fixtures or this repo. GitKraken was closed before the run. Steam,
Stream Deck and the Claude app were running, idle. Raw output is in `results/windows-full.log`,
which is git-ignored, so these figures are the record.

**Fixture state, recorded, not repaired:** `linux-graph` has a single commit-graph file
(90 MB, no chain), one pack and a `multi-pack-index` dated Nov 6, from GitKraken. HEAD is
`05441d6`, 1,505,332 commits. Status showed only the 13 baseline paths, all `1 .M` with equal
hashes, before and after the run, and the graph and `multi-pack-index` were unchanged.
`core.autocrlf=true` and `core.fscache=true` both come from Git for Windows' system
`etc/gitconfig`. gix reads the same config, so the kernel figures are taken with autocrlf on.

### Walks, layout excluded

| Floor | Pass | gix worst | `git` worst | Result |
| --- | --- | --- | --- | --- |
| HEAD, first 5,000, graph | 100 ms | 45.7 ms | 46.0 ms | Pass |
| All refs, first 5,000, graph | 1.5 s | 1.32 s | 999 ms | Pass |
| All refs, everything, graph | 6 s | 2.27 s | 2.23 s | Pass |
| All refs, first 5,000, no graph | 12 s | 22.64 s | 7.56 s | **Fail** |

Recorded only: HEAD first 5,000 without a graph took 22.50 s (`git` 7.54 s). HEAD everything
took 2.03 s with a graph (`git` 2.14 s) and 36.88 s without (`git` 8.77 s). Every walk had
the same order as `git`.

- **Same picture as macOS.** The no-graph walk misses by the same structural margin, about
  three times `git`. The graph walks pass. All refs, first 5,000 has 12% headroom, against 8%
  on the Mac.

### Status

| Floor | Pass | gix worst | `git` worst | Result |
| --- | --- | --- | --- | --- |
| HEAD vs index | 50 ms | 78.2 ms | 18.6 ms | **Fail** |
| Watcher-scoped index vs worktree, 10 paths | 30 ms | 24.7 ms | 28.6 ms | Pass |
| Full status with untracked files | 1 s | 358.7 ms | 93.6 ms | Pass |

Recorded only:

| Case | gix worst | `git` worst |
| --- | --- | --- |
| Index vs worktree, whole tree, no untracked | 84.9 ms | 60.2 ms |
| Full status without untracked files | 127.2 ms | 64.8 ms |
| HEAD vs index, part: reading the index | 12.0 ms | - |
| HEAD vs index, part: building an index from `HEAD^{tree}` | 58.6 ms | - |
| HEAD vs index, HEAD tree index held (rerun) | 16.2 ms | - |
| Watcher-scoped, index already in memory | 11.9 ms | - |

The held tree index row comes from a later `status` rerun, logged in
`results/windows-status-rerun.log`. The rerun moved no result. Most rows came within 2 ms of
the first run, but gix's full status without untracked files took 116.2 ms (first run 127.2 ms),
and `git diff-files` took 23.8 ms (first run 28.6 ms).

- **HEAD vs index misses by more than on macOS,** 56% over the Floor against 19%. The
  tree-to-index build alone is 58.6 ms, already over the Floor. `git diff-index --cached` is
  18.6 ms including spawn, faster than on the Mac.
- **Holding the HEAD tree index brings HEAD vs index to 16.2 ms,** under the 50 ms Floor. That
  includes reading `.git/index` fresh and rename tracking, and it found the same 3 changes,
  rename included. A long-running app only needs to rebuild that tree index when HEAD moves, which
  costs the 58 ms once per commit or checkout, not once per check. gix's API doesn't do this
  itself: `tree_index_status` rebuilds the tree index on every call, and nothing in gix reads the
  index's cache-tree (TREE extension), which is how `git` skips unchanged folders. On this fixture
  the cache-tree is fully valid and matches `HEAD^{tree}`.
- **The watcher case passes on Windows,** with 18% headroom, where it missed on macOS. With the
  index in memory it's 11.9 ms, half the Mac's 22.8 ms. `git diff-files` passes with only 5%
  headroom, spawn included, so neither tool has much room here.
- **`git` is much faster than gix at full status on Windows,** 93.6 ms against 358.7 ms. That's
  the opposite of macOS, where gix was faster. Both pass the 1 s Floor. `git` has `fscache` on
  here, which batches the `lstat` calls, and that's the likely reason, but it wasn't verified.

### Parity

| Check | Result |
| --- | --- |
| Kernel status, 17 lines, untracked included, autocrlf on | Pass. 13 of 13 baseline lines also identical |
| Kernel hunks, 11 unstaged files and 1 staged | Pass. 24 hunks byte for byte |
| Filters fixture status, before refresh | Pass. 10 lines |
| Filters fixture status, after refresh | Pass. 10 lines |
| Filters fixture hunks, 9 unstaged files and 1 staged | Pass. 5 hunks, 4 empty diffs, 1 binary |

- **The `upper` clean filter ran.** gix found `sh` and `tr` from Git for Windows, with no
  error.
- The "dirty with no diff" files (`b.crlf`, `c.lf`, `e.up`) behave as on macOS: `git` reports
  them `.M` with no hunks, before and after a refresh, and gix matches.

### What the Windows run changes

- **No-graph walks:** a miss on both OSes, so they go to `git rev-list` or Cairn makes sure a
  graph exists. That's still #13's call.
- **HEAD vs index:** a miss on both OSes as measured, worse on Windows, so by the #8 rules it goes
  to `git diff-index --cached`, which is 18.6 ms on Windows with the spawn. The held tree index
  passes at 16.2 ms on Windows and 10.6 ms on macOS, though. Whether that design counts for the
  gate is for #13.
- **Watcher-scoped check:** passes on Windows, misses on macOS. If the gate is per path on both
  OSes, it still goes to `git diff-files`. A per-OS split is for #13. Either way, `git`'s 28.6 ms
  on Windows leaves 1.4 ms of headroom, so a CLI fallback on every watcher event is tight, and
  #12's spawn numbers should be read with this.
- **Full status:** passes with both tools on both OSes. On Windows `git` is about four times
  faster, which matters for the Budget rewrite more than for this gate.

Not run on Windows: cold runs, which need a reboot with the fixture unopened. They're optional
and recorded only.
