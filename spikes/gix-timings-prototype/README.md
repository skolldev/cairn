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
| Watcher-scoped, index already in memory (separate run) | 22.8 ms | - |

- **HEAD vs index misses on its tree-to-index build.** gix turns the whole `HEAD^{tree}` into a
  97,000-entry index on every call, which takes 54 ms alone. `git diff-index --cached` finishes
  in 27 ms including spawn. Its cache-tree extension is the likely reason, but that wasn't
  verified here.
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

Not run on macOS: cold runs (need `sudo purge`), and anything with Defender or NTFS, which is
the Windows half.

## Windows results

Not run yet. See [CHECKLIST.md](CHECKLIST.md).
