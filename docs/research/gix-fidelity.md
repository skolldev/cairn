# How closely gix status, filters and revwalk match git

Answers [skolldev/cairn#3](https://github.com/skolldev/cairn/issues/3). Assessed 2026-11-05.

**Assessed version:** [gix 0.90.0][crate] (released 2026-10-22, tag commit `dd39fb6`), with gix-status 0.37.0, gix-filter 0.36.1, gix-diff 0.70.0, gix-dir 0.32.0, gix-index 0.58.0, gix-traverse 0.64.0, gix-commitgraph 0.42.0. Unreleased fixes are read from `main` at `39e093f` (215 commits past the tag). The git side is v2.56.0.

**Method limit:** every finding comes from reading source, issues and PRs. Nothing was built or run. "Matches git" below means the code does what git's code does, not that output was compared.

## Short answer

**gix fidelity does not block the Phase 0 decision, but the gix spike needs to change.** The everyday Windows cases (autocrlf, `.gitattributes`, clean filters, LFS) follow git's logic closely, and most failures surface as a hard error, which a fallback can catch. The real gaps are elsewhere:

- **Parity is young.** [#3092][pr3092] closed 24 reported parity issues on 2026-10-27, five of them in status, and three more status bugs followed ([#3128][i3128], [#3133][i3133], [#3134][i3134]). All are fixed on `main`, none in 0.90.0. Upstream compares against git case by case. I found no corpus test against `--porcelain=v2`, and gix has no porcelain v2 output: [its CLI bails][g-cli-porcelain].
- **The accelerators git uses on our two platforms are absent.** gix decodes the fsmonitor and untracked-cache extensions and then ignores them. This costs speed, not correctness. [#3153][pr3153] would add both, but it is a draft awaiting review.
- **The status budget has no published support.** The only kernel-sized figures are about 300 ms on Windows and 1.57 s cold on Linux. See [Performance](#performance).

### Changes to the gix spike

1. **Pin `main` at `39e093f` or later, or the first release after 0.90.0.** 0.90.0 lacks [`rev_walk_topo`][pr3092] and the status fixes. It also lacks [#3161][pr3161]: a file that shrinks mid-read aborts the whole status, which its reporter hit on almost one call in three while files were being rewritten. Cairn triggers status on save, so that is our exact case. Pin an exact revision: [`gix` went from 0.86 to 0.90][crate] between July and October, and each 0.x bump is a semver break.
2. **Don't use `repo.status()` as the budget case.** It [rebuilds a whole index from `HEAD^{tree}`][g-tree-index-54] on every call. Git skips matching subtrees [through the cache-tree][git-unpack]. Time three things separately: HEAD vs index (rerun only when `.git/index` or HEAD changes), [index vs worktree alone][g-iw-iter] scoped by watcher paths, and the full status. This matches the plan's "refresh only the affected snapshot".
3. **Walk the real tip set, not just HEAD.** The topo walk [explores down to the lowest tip's generation][g-topo-154] before yielding anything, [as git does][git-revision-topo]. With old kernel tags as tips, that is effectively all history, even with a commit-graph. Time HEAD-only and all-refs separately.
4. **Extend the golden tests beyond `git diff --numstat`.** Numstat parity is proven ([0 of 72,702 files differ][pr3069]), but it says nothing about hunk placement. Compare full hunks against `git diff`.

### Changes to the plan

- **"imara-diff (histogram)" conflicts with matching `git diff`.** Git's default is Myers, and gix [defaults to it too][g-diff-algo]. Use the repo's `diff.algorithm`, through `gix-diff`'s vendored `gix-imara-diff`, because the Myers and slider parity fixes live in that fork ([#2506][pr2506], [#3069][pr3069]).
- **Never call `Outcome::write_changes()`.** It [writes default extensions only][g-write-changes], and [`write_extensions`][g-index-write] emits just TREE and `sdir`. FSMN and UNTR are lost, [a split index is dissolved and V4 can't be written][crate-write]. That breaks both git's accelerators and "writes go through the CLI". Run `git update-index -q --refresh` when [`entries_to_update`][g-outcome] stays high, otherwise stat-dirty files are rehashed on every status.
- **Keep `FastEq`, the [default in `status()`][g-iter-fasteq].** `HashEq` looks more accurate but disagrees with git. Git [treats a size change as modified without cleaning the file][git-ie-modified], and [FastEq does the same][g-fasteq]. An M with an empty diff is therefore git's own behaviour, not a phantom.
- **Open with `permissions.config.git_binary = true`, or pass the paths in.** It is [off by default][g-perm-git-binary] and [only the clone helpers enable it][g-open-git-binary]. Without it, on Unix gix sees [only `/etc/gitconfig`][g-system-config] as system config, so a value set in a Homebrew or Xcode git's config is missed. Cairn's git runner knows the binary, so it can supply [the explicit config paths][g-open-paths].
- **Keep line terminators when rendering diffs.** See the blob diff rows below.

## CLI fallback triggers

The plan assumes one per-repo switch. Three scopes fit better. Thresholds for the performance row should come from the spike, not from this note.

| Scope | Trigger | Detect | Why |
| --- | --- | --- | --- |
| Per repo, at open | Sparse index | `index.is_sparse()` | Status [hard-errors in the HEAD-vs-index half][g-sparse-bail], [still on `main`][m-sparse-bail] |
| Per repo, at open | Partial clone | `extensions.partialClone` or a promisor remote | [No promisor support][crate-accel]; a [missing blob is an `Err`][g-find-blob]. Filter support is in [open PR #2375][pr2375] |
| Per repo, at open | Reduced trust | `repo.git_dir_trust() != Full` | [Repo-local filter drivers are dropped][g-is-trusted], while [gix opens anyway][g-open-trust] where [git refuses by default][git-safe] |
| Per repo, at open | `core.safecrlf=true` | Config read | Status [fails on the first affected stat-dirty file][g-eol-bail]; git [does no round-trip check here][git-conv-flags]. Setting the key to `false` in memory may avoid it (unverified) |
| Per repo, at open | `diff.algorithm=patience` | Config read | [Not implemented][g-diff-algo]: an error, or [histogram when config is lenient][g-diff-lenient]. Applies to diff, not status |
| Per call | Any `Err` from open or status | Result | Catches what the list above misses: unsupported encodings, a filter that won't start, reftable (unverified). Latch the repo to CLI after repeated failures |
| Per result | A staged rename with similarity below 100% | `Rewrite` whose ids differ | [Scoring and pairing differ from git](#status-against-porcelain-v2), so ask `git status --porcelain=v2` for that result only |
| Per repo, by measurement | gix misses the budget on a repo where `core.fsmonitor` or `core.untrackedCache` is on | Spike floors, then timing | Git uses these and gix doesn't, so the CLI may be faster |

**These should not trigger fallback:** autocrlf, `.gitattributes`, `text`, `eol`, `ident`, LFS and other clean filters, split index, skip-worktree sparse checkout, or `core.fsmonitor` and untracked cache as correctness concerns. Run fallback as `git --no-optional-locks status --porcelain=v2`.

## Evidence

Crate-status is a weak source here: its overview says it is [generated and reconciled on demand][crate-note], and some rows are stale. For example, [it lists tree-with-index diffing as missing][crate-tree-index] and [describes first-found rename matching][crate-rename], and both are out of date. The rows below rest on source.

### Filters

| Feature | State | Evidence |
| --- | --- | --- |
| autocrlf, `core.eol`, `text`, `eol` | Supported | [Config is read][g-filter-config] and passed to [status conversion][g-status-convert]. The [CRLF-in-index guard][g-eol-guard] matches [git's][git-crlf-guard]. gix reads the index blob only when there is a CRLF ([#3078][pr3078]), which changes cost, not results |
| `.gitattributes` | Supported | Read [worktree first, then index][g-status-attrs]. Also covers `ident` and the legacy `crlf` attribute through the [pipeline][g-pipeline-attrs] |
| `core.safecrlf` | Partial | gix [applies the configured check in status][g-convert-eol]. Git's [`ce_compare_data`][git-compare-data] passes no write flag, so [conv flags are 0][git-conv-flags]. `true` becomes an error; the default `warn` only traces |
| Single-file clean filters | Supported | [Input falls back when a non-required filter fails][g-filter-fallback]. The `%f` path bug that made LFS extensions show false modifications was [fixed before the tag][pr3072] |
| Long-running filters, including LFS | Supported | [Protocol v2 with clean, smudge and delay][g-filter-handshake]. Each status worker [starts its own process][g-filter-clone], only when a stat-dirty filtered file needs cleaning. Children are [reaped on drop][g-filter-drop] after [a leak of 274 zombies in 10.5 h][i2947]. Error handling has a [documented deviation][g-filter-deviation] |
| Filter launch on Windows | Supported | Commands [may run through a shell][g-filter-spawn], [using the `sh.exe` from Git for Windows][g-shell], or `sh.exe` from PATH. Discovery improved after the tag ([#3132][pr3132]) |
| `working-tree-encoding` | Partial | [encoding_rs labels only, no `-BOM` variants][g-encoding]. An unknown label is [an error when converting to git][g-encoding-err], while git [doesn't die in this path][git-encode]. UTF-16 listed in `core.checkRoundtripEncoding` fails every valid file until [#3163][pr3163] (open) lands |

### Status against porcelain v2

| Feature | State | Evidence |
| --- | --- | --- |
| Racy git and stat matching | Supported | [Ported from git][g-racy], honouring `trustctime` and `checkStat` [through stat options][g-stat]. On Windows, [dev, ino, uid and gid are zero][g-stat-from-fs]; whether that always equals Git for Windows' values is unverified |
| Assume-unchanged, skip-worktree | Supported | [Skipped by flag][g-flags], like git |
| Sparse checkout without sparse index | Supported | As above, and [the dirwalk marks excluded directories][g-dir-sparse] |
| Sparse index | Missing | [Tree-vs-index refuses][g-sparse-bail], with [a test asserting it][g-sparse-test]. No [expansion of sparse entries][crate-sparse]. Index-vs-worktree alone still runs |
| Split index | Partial | [Read and dissolved][crate-write]. [No status acceleration][crate-accel-status] |
| fsmonitor | Missing, safe | The FSMN token is [`dead_code`][g-fsmn]. [`FSMONITOR_VALID` is checked][g-flags] but nothing sets it, so every entry gets an lstat. Results stay correct. Git [marks entries valid and queries the monitor][git-fsmonitor] |
| Untracked cache | Missing, safe | UNTR is decoded but [the walk doesn't use it][crate-untr]; every status does a full directory walk |
| Staged renames | Partial | Defaults match ([50%, limit 1000][g-rewrites-default]; [git's limit][git-rename-limit]) except for empty files: [`track_empty: false`][g-rewrites-default] against git's [`rename_empty = 1`][git-rename-empty]. Similarity is [line-diff based][g-similarity], not git's, and gix [says it is less sophisticated][g-tracker-deviation] (git [keeps 4 candidates][git-candidates]). [`ChangeRef::Rewrite` carries no score][g-change-rewrite], so `R<score>` can't be produced directly. In 0.90.0 [`status.renames` reads `merge.renames`][g-status-renames] ([#3133][i3133]) |
| Worktree renames | Supported | [Off by default][g-rewrites-off]. Leave them off, since [Git has no matching config][g-rewrites-off] |
| Submodules | Supported on `main` | [Recursive status][crate-submodule], with [`diff.ignoreSubmodules` taking precedence][g-sm-ignore]. 0.90.0 [skips gitlinks missing from `.gitmodules`][g-sm-skip] ([#3114][i3114]) and [double-reports a replaced gitlink][i3113]. `S<c><m><u>` should be derivable from [`Status`][g-sm-status] (unverified) |
| Other edge cases | Supported on `main` | Fixed after the tag: [conflict double-report][i3134], [intent-to-add deletion][i3115], [unborn indirect HEAD][i3112], [executable to symlink][i3111], [boolean `showUntrackedFiles`][i3128] |
| Porcelain v2 output | Missing | Cairn must write the mapping. Status is [unordered when parallel][g-unordered]. `# branch.ab` isn't part of status; [`is_ancestor()` is still an open PR][pr3036] |

### Revwalk and commit-graph

| Feature | State | Evidence |
| --- | --- | --- |
| Date-ordered walk | Supported | [`rev_walk` sorting][g-walk-sorting]. Hidden-tip fix for skewed dates is [on `main`][i3104] |
| Topological walk | Partial in 0.90.0, supported on `main` | 0.90.0 has [plumbing only][g-topo-mod], with [wrong results for hidden tips][i3105]. [#3092][pr3092] adds `rev_walk_topo` and reports matching git on the Linux repo for both orderings and parent modes, with and without a commit-graph. That is the author's claim, not independently checked |
| Commit-graph reading | Supported | [Chains][g-graph-chain] and [generation numbers][g-graph-gen]. [Used by default][g-walk-graph], off with replace refs or `core.commitGraph=false` |
| Commit-graph writing | Missing | No write path in gix-commitgraph. The plan's `git commit-graph write --reachable` is the right tool. [`gc` writes one by default][git-gc] and [`fetch` only on opt-in][git-fetch]; nothing I read writes one at clone (unverified) |
| Without a commit-graph | Slow, as in git | Generation is [infinite without a graph][g-topo-inf], so [the indegree walk][g-topo-indegree] covers everything before the first row, and [building is documented as expensive][g-topo-expensive]. Git [also falls back to a full walk][git-revision-limited] |
| Changed-path Bloom filters | Missing | No Bloom reader in gix-commitgraph, so per-file history gets no acceleration. Not relevant to the first graph screen |

### Blob diff

Not in the ticket's list, but the principle covers it.

| Feature | State | Evidence |
| --- | --- | --- |
| Myers line counts | Supported | [0 of 72,702 real files and 0 of 216 random pairs differ][pr3069] from git's numstat. The fix is in the tag: [edit-cost numbering][g-myers-ec] |
| Hunk placement | Partial, unproven | [Slider heuristics are ported][g-sliders], but [the strict corpus is optional][g-slider-readme]; only a smoke baseline always runs strictly. The one published figure is [4.56% exact on a slider-only corpus][pr2931], from before later fixes. Treat it as out of date |
| Line terminators | Partial | [`interned_input()` strips LF and CRLF][g-interned]. A CRLF-to-LF change [counts as `+0/-0`][pr3069] where git counts every line, which is a false "no change". Use [`intern_source()`][g-intern-source] |
| `patience` | Missing | [Not implemented][g-diff-algo] |
| Hunk-header function context, whitespace-ignoring modes | Missing | No `xfuncname` or whitespace-ignore code in `gix-diff` or `gix` at the tag |

## Performance

No published figure covers macOS, a filter-heavy repo, a full-history first screen, or save-to-status latency. Hardware is unstated unless noted. None of these count as evidence under the map's rule; they only set expectations.

| Figure | Source | Caveat |
| --- | --- | --- |
| Status on the Linux repo, Windows: about 1000 ms to about 300 ms | [#2547][pr2547] | Later [rewritten on `std::fs`][pr3071], and the code [doesn't assume equal performance][g-fscache]. The author calls it about on par with libgit2, which weakens the plan's "slower status" reason for rejecting libgit2, at least on Windows |
| A separate custom status on the Linux repo: about 70 ms | [#2547][pr2547] | Not gix, and the author says its behaviour differs from git |
| Clean Linux repo, cold cache, on Linux: a gix-based tool 1.567 s, git 316.9 ms | [#1771][i1771] | January 2025. Attributed to the [single-threaded tree traversal][i1771]. Moved to [discussion #2818][d2818], with no fix recorded there |
| WebKit, gix 1.44x faster than git | [Report 24-02][r2402] | Before HEAD-vs-index existed |
| `is_dirty()` loop, 50k files: about 49 ms to about 35 ms | [Changelog][g-changelog-dirty] | Early-exit path, not a clean full status |
| `@~10..@` on the Linux repo, seven-run median: 35 ms no graph, 34 ms with graph, git 23 ms | [#3092][pr3092] | Short range, not first screen. Before the fix, uncached walks read 1.35M commits |

On budget: the plan's 100 to 200 ms debounce takes most or all of the 150 ms target that includes it, before any status work. That belongs to the budget rewrite the map already lists.

## Not verified

- Any behaviour by execution. Nothing was run, including the `core.safecrlf` failure and the fallback triggers.
- The in-memory `core.safecrlf` override as a workaround.
- Stat parity with Git for Windows. On a clean Windows fixture, [`worktree_files_read`][g-outcome] should be 0, and the spike should assert it, because otherwise every status is hashing files.
- Reftable handling on open; I found no explicit handling in the open path.
- Whether gix and git agree on UTF-16 files with a byte-order mark for the opposite endianness.
- The claims quoted from PR descriptions. Several are AI-generated and were checked against source only where linked here.

[crate]: https://crates.io/crates/gix/0.90.0
[crate-note]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/crate-status.md?plain=1#L38
[crate-accel]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/crate-status.md?plain=1#L57-L58
[crate-rename]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/crate-status.md?plain=1#L110
[crate-submodule]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/crate-status.md?plain=1#L123
[crate-tree-index]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/crate-status.md?plain=1#L125
[crate-accel-status]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/crate-status.md?plain=1#L781
[crate-untr]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/crate-status.md?plain=1#L847
[crate-sparse]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/crate-status.md?plain=1#L871
[crate-write]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/crate-status.md?plain=1#L872-L884
[g-tree-index-54]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix/src/status/tree_index.rs#L54
[g-iw-iter]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix/src/status/index_worktree.rs#L542-L551
[g-iter-fasteq]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix/src/status/iter/mod.rs#L125-L134
[g-unordered]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix/src/status/iter/mod.rs#L28-L40
[g-write-changes]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix/src/status/iter/types.rs#L86-L113
[g-index-write]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-index/src/write.rs#L121-L131
[g-outcome]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-status/src/index_as_worktree/types.rs#L48-L80
[g-fasteq]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-status/src/index_as_worktree/traits.rs#L110-L115
[g-flags]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-status/src/index_as_worktree/function.rs#L277-L285
[g-racy]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-status/src/index_as_worktree/function.rs#L451-L473
[g-find-blob]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-status/src/index_as_worktree/function.rs#L582-L591
[g-status-convert]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-status/src/index_as_worktree/function.rs#L618-L628
[g-status-attrs]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix/src/status/index_worktree.rs#L92-L97
[g-sm-skip]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix/src/status/index_worktree.rs#L222-L228
[g-sm-ignore]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix/src/status/index_worktree.rs#L241-L259
[g-rewrites-off]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix/src/status/platform.rs#L86-L93
[g-sm-status]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix/src/submodule/mod.rs#L432-L499
[g-cli-porcelain]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gitoxide-core/src/repository/status.rs#L69-L71
[g-sparse-bail]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-diff/src/index/function.rs#L42-L44
[m-sparse-bail]: https://github.com/GitoxideLabs/gitoxide/blob/39e093fb541fc7a7acf6d7fef58f30827150cb00/gix-diff/src/index/function.rs#L43-L45
[g-sparse-test]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-diff/tests/diff/index.rs#L1273
[g-dir-sparse]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-dir/src/walk/classify.rs#L452-L458
[g-fsmn]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-index/src/extension/fs_monitor.rs#L8-L16
[g-stat]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-index/src/entry/stat.rs#L38-L76
[g-stat-from-fs]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-index/src/entry/stat.rs#L90-L101
[g-filter-config]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix/src/filter.rs#L33-L59
[g-pipeline-attrs]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-filter/src/pipeline/mod.rs#L49
[g-convert-eol]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-filter/src/pipeline/convert.rs#L127-L136
[g-eol-guard]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-filter/src/eol/convert_to_git.rs#L79-L95
[g-eol-bail]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-filter/src/eol/convert_to_git.rs#L109-L143
[g-filter-fallback]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-filter/src/driver/apply.rs#L116-L125
[g-filter-deviation]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-filter/src/driver/apply.rs#L56-L61
[g-filter-handshake]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-filter/src/driver/init.rs#L30
[g-filter-spawn]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-filter/src/driver/init.rs#L75-L82
[g-filter-clone]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-filter/src/driver/mod.rs#L87-L94
[g-filter-drop]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-filter/src/driver/mod.rs#L132-L138
[g-encoding]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-filter/src/worktree/encoding.rs#L5-L11
[g-encoding-err]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-filter/src/pipeline/util.rs#L42-L64
[g-shell]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-path/src/env/mod.rs#L59-L70
[g-system-config]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-path/src/env/mod.rs#L47-L57
[g-perm-git-binary]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix/src/repository/permissions.rs#L9-L14
[g-open-git-binary]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix/src/lib.rs#L375-L380
[g-open-paths]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix/src/open/options.rs#L69-L81
[g-open-trust]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix/src/open/repository.rs#L416-L423
[g-is-trusted]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix/src/config/mod.rs#L61-L63
[g-diff-algo]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix/src/config/tree/sections/diff.rs#L8-L13
[g-diff-lenient]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix/src/config/cache/access.rs#L32-L35
[g-rewrites-default]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-diff/src/rewrites/mod.rs#L67-L75
[g-tracker-deviation]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-diff/src/rewrites/tracker.rs#L1-L7
[g-similarity]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-diff/src/rewrites/tracker.rs#L786-L793
[g-change-rewrite]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-diff/src/index/mod.rs#L72-L98
[g-status-renames]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix/src/config/tree/sections/status.rs#L14-L22
[g-walk-sorting]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix/src/revision/walk.rs#L20-L55
[g-walk-graph]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix/src/revision/walk.rs#L275-L282
[g-topo-mod]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-traverse/src/commit/topo/mod.rs#L1
[g-topo-expensive]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-traverse/src/commit/topo/init.rs#L122
[g-topo-154]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-traverse/src/commit/topo/init.rs#L154-L175
[g-topo-indegree]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-traverse/src/commit/topo/iter.rs#L65-L75
[g-topo-inf]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-traverse/src/commit/topo/iter.rs#L352
[g-graph-chain]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-commitgraph/src/init.rs#L30
[g-graph-gen]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-commitgraph/src/file/commit.rs#L39-L42
[g-sliders]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-diff/src/blob/mod.rs#L12-L38
[g-slider-readme]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-diff/tests/README.md?plain=1#L51-L52
[g-interned]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-diff/src/blob/platform.rs#L385-L390
[g-intern-source]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-diff/src/blob/platform.rs#L181-L183
[g-myers-ec]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-imara-diff/src/myers.rs#L144-L150
[g-fscache]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-status/src/fscache.rs#L15-L18
[g-changelog-dirty]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/gix-status/CHANGELOG.md?plain=1#L202-L215
[r2402]: https://github.com/GitoxideLabs/gitoxide/blob/dd39fb66e3b1551c9fe849043e39b357a9799ca1/etc/reports/24-02.md?plain=1#L53
[git-compare-data]: https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/read-cache.c#L231-L245
[git-ie-modified]: https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/read-cache.c#L460-L487
[git-crlf-guard]: https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/convert.c#L527-L539
[git-encode]: https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/convert.c#L392
[git-conv-flags]: https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/object-file.c#L36-L44
[git-fsmonitor]: https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/fsmonitor.c#L792-L822
[git-rename-empty]: https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/diff.c#L5133
[git-rename-limit]: https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/diff.c#L58
[git-safe]: https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/config/safe.adoc?plain=1#L28-L32
[git-candidates]: https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/diffcore-rename.c#L1064
[git-unpack]: https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/unpack-trees.c#L883-L899
[git-revision-topo]: https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/revision.c#L3913-L3926
[git-revision-limited]: https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/revision.c#L3195-L3196
[git-gc]: https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/config/gc.adoc?plain=1#L63-L68
[git-fetch]: https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/config/fetch.adoc?plain=1#L97-L103
[pr2375]: https://github.com/GitoxideLabs/gitoxide/pull/2375
[pr2506]: https://github.com/GitoxideLabs/gitoxide/pull/2506
[pr2547]: https://github.com/GitoxideLabs/gitoxide/pull/2547
[pr2931]: https://github.com/GitoxideLabs/gitoxide/pull/2931
[pr3036]: https://github.com/GitoxideLabs/gitoxide/pull/3036
[pr3069]: https://github.com/GitoxideLabs/gitoxide/pull/3069
[pr3071]: https://github.com/GitoxideLabs/gitoxide/pull/3071
[pr3072]: https://github.com/GitoxideLabs/gitoxide/pull/3072
[pr3078]: https://github.com/GitoxideLabs/gitoxide/pull/3078
[pr3092]: https://github.com/GitoxideLabs/gitoxide/pull/3092
[pr3132]: https://github.com/GitoxideLabs/gitoxide/pull/3132
[pr3153]: https://github.com/GitoxideLabs/gitoxide/pull/3153
[pr3161]: https://github.com/GitoxideLabs/gitoxide/pull/3161
[pr3163]: https://github.com/GitoxideLabs/gitoxide/pull/3163
[i1771]: https://github.com/GitoxideLabs/gitoxide/issues/1771
[i2947]: https://github.com/GitoxideLabs/gitoxide/issues/2947
[i3104]: https://github.com/GitoxideLabs/gitoxide/issues/3104
[i3105]: https://github.com/GitoxideLabs/gitoxide/issues/3105
[i3111]: https://github.com/GitoxideLabs/gitoxide/issues/3111
[i3112]: https://github.com/GitoxideLabs/gitoxide/issues/3112
[i3113]: https://github.com/GitoxideLabs/gitoxide/issues/3113
[i3114]: https://github.com/GitoxideLabs/gitoxide/issues/3114
[i3115]: https://github.com/GitoxideLabs/gitoxide/issues/3115
[i3128]: https://github.com/GitoxideLabs/gitoxide/issues/3128
[i3133]: https://github.com/GitoxideLabs/gitoxide/issues/3133
[i3134]: https://github.com/GitoxideLabs/gitoxide/issues/3134
[d2818]: https://github.com/GitoxideLabs/gitoxide/discussions/2818
