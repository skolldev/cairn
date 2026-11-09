# git behaviour by version

Cairn runs `GitVersion::MINIMUM` and newer (ADR 0007). These are the git behaviours that set the minimum, changed after it, or surprise a caller, each checked in git's own source. Check a git command in a spec, product code or a test against this list, and add every new fact you confirm.

## Checking a fact

Read git's source at a release tag rather than its manual, which describes only the newest git:

```bash
gh api "repos/git/git/contents/builtin/fetch.c?ref=v2.37.0" -H "Accept: application/vnd.github.raw"
```

Release notes live at `Documentation/RelNotes/<version>.adoc`. To find the first version with a behaviour, grep the file at a few tags and narrow down.

Check the newest tag as well as the oldest supported one when a test leans on git's incidental behaviour rather than a documented promise, such as what makes git block or which step fetches. `scripts/git-matrix.sh` then runs the tests on both.

## Facts

| Behaviour | Since | Where it shows | Checked in |
|---|---|---|---|
| `fetch --all` with one remote fetches it directly | 2.37 | why 2.37 is the minimum: the fetch honours `--progress`. Before 2.37 even a lone remote goes through a sub-fetch | `builtin/fetch.c` at 2.36 and 2.37: "do not do fetch_multiple() of one" |
| `submodule add` from a local path needs `protocol.file.allow=always` | 2.38.1 | the default for `file` dropped from always to user, so the submodule's clone is refused. `git -c protocol.file.allow=always submodule add` works on every version. See `a_git_switch_in_a_submodule_refreshes_refs_and_status` | `get_protocol_config` in `transport.c` at 2.37.0 and 2.38.1 |

## Gotchas in every version

- **`fetch --all` drops `--progress`.** `--all` hands each remote to a sub-fetch, and `add_options_to_argv` never passes `--progress` down to it (checked from 2.23 through master). So progress reports appear only with one remote. Everything else git and the remote write still streams. See `crates/repo/src/network.rs`.
- **`--date-order` and `--topo-order` overwrite each other.** Both set `revs->sort_order`, so the later one wins, and either implies a topological sort. `git rev-list --topo-order --date-order` is `--date-order`.
- **`--all` starts from every ref, HEAD and every other worktree's HEAD.** Refs come first, by name (`handle_revision_pseudo_opt` in `revision.c`). So gix's walk gathers them itself, in that order. They aren't all branches: a stash or a custom ref counts.
- **`rev-list` in a partial clone fetches a tag's missing target, but not always a missing parent.** Peeling a tag from `--all` fetches on 2.39 and 2.56, through `remote.<name>.uploadpack`. A missing parent fetches on 2.39, while on 2.56 the walk fails with `Could not read`. The first version to change wasn't narrowed down. `--missing` and `--exclude-promisor-objects` turn fetching off. `held_walk` in `crates/repo/tests/lanes.rs` blocks a real walk through a tag for this reason. Measured by running each, with `commit.c` read at 2.39 and 2.56.
- **Racily clean entries run their clean filter on every `diff-files`.** For an entry whose stat info matches the index, `ie_match_stat` (`read-cache.c`) still checks the content if `is_racy_timestamp` says the entry's mtime is not older than the index file's. Default builds, without `USE_NSEC`, compare that in whole seconds. The check goes `ce_modified_check_fs` → `ce_compare_data` → `index_fd`, and `index_fd` applies the clean filter and line-ending conversion. So each racy file runs its filter once per `diff-files`. `run_diff_files` (`diff-lib.c`) gets there through `match_stat_with_submodule`, and `diff-files` never passes `DIFF_RACY_IS_MODIFIED`, the flag that would skip the check. Checked at 2.37.0 and 2.56.0, and measured on 2.37.0, 2.39.3 and 2.56.0. Entries are racy after a clone, checkout or `add` that writes the index in the same second as the files. They stay racy until a command writes the index at a later second: `status` or `update-index --refresh` without `--no-optional-locks`, or `add`. `diff-files` never writes the index, with or without `--no-optional-locks`. `core.checkStat` and `core.trustctime` don't change this. `assume-unchanged`, skip-worktree and fsmonitor-valid entries return before the check. A stat-dirty file is never content-checked here: its stat info already differs. Reproduce it without the fixture:

  ```bash
  git init -q r && cd r && mkdir sub && echo a > README.md && echo b > sub/README.md
  git config filter.slow.clean 'sh -c "echo \$0 >&2; cat" %f'
  echo 'README.md filter=slow' > .git/info/attributes
  git add . && git -c user.name=x -c user.email=x@x commit -qm init
  git --no-optional-locks diff-files   # names README.md and sub/README.md again, on every run
  ```

  #34's 15 runs in the ripgrep fixture came from this: the unanchored `README.md` attribute matched the 15 other READMEs, all racy after the fresh clone, while the touched README.md had changed stat info and ran no filter. In Cairn this hits the Unstaged read (`unstaged` in `crates/repo/src/status.rs`), which never writes the index, so a slow filter such as git-lfs costs one run per racy file on every read. `hash-object` then costs one more run per stat-dirty candidate. The check can't be skipped safely, since it is what catches an edit made in the same second as the index write. Running `git update-index -q --refresh` once ends the runs. But it takes `index.lock`, which Cairn's reads avoid on purpose with `--no-optional-locks`. While the lock is held, the user's own index writes fail (`git add` reports `Unable to create '.git/index.lock': File exists`). So Cairn leaves this as it is (#49). The runs stop at the next index write a second or more later: the user's own `status`, `add` or `commit`, or a Cairn branch switch. A switch to another branch at the same commit clears it too. Measured on 2.37.0.
- **A linked worktree splits git's files between two directories.** Its `.git` file names its own git dir, `<common>/worktrees/<name>`, whose `commondir` file names the common dir. `common_list` in `path.c` lists what lives in the common dir: `refs/`, `packed-refs`, `config`, `info/` (`info/exclude` included), `logs/`, `objects/`, `hooks/`, `worktrees/`, `shallow` and a few more. Everything it doesn't list belongs to each worktree on its own: `HEAD`, `index`, `FETCH_HEAD`, `ORIG_HEAD`, `config.worktree`. A few paths inside the common entries are per-worktree too: `refs/bisect`, `refs/rewritten`, `refs/worktree`, their logs under `logs/refs/`, `logs/HEAD` and `info/sparse-checkout`. So the common dir also holds the main worktree's own `HEAD`, `index` and `refs/bisect`, which mean nothing to a linked worktree. A submodule has one git dir, `<superproject>/.git/modules/<name>`, which is its own common dir too. The watcher (`crates/repo/src/watch.rs`) follows this split (#35). Checked at 2.37.0 and 2.56.0, where the list is the same.
- **`-c` goes before the subcommand.** `git -c key=value fetch` sets config for that run. `git fetch -c key=value` doesn't set config: git parses the `-c` as one of `fetch`'s own options.
- **`remote.<name>.uploadpack` runs through a shell** for a local remote, with the repository path appended. `sh '<script>'` therefore works as a wrapper, and `remote.<name>.receivepack` does the same for a push. `slow_remote` in `crates/repo/tests/common` uses both to slow a remote down.
- **`:(literal,icase)` combines.** Element magic is a comma list, and the only pair `parse_element_magic` refuses is `literal` with `glob` (`pathspec.c`, checked at 2.37.0). The global `--literal-pathspecs` with `--icase-pathspecs` is refused instead, so pass the magic per pathspec. `icase` compares with `strncasecmp` (`ps_strncmp` in `pathspec.h`), byte by byte, so whether a byte past ASCII folds depends on the C library and locale. The narrowed Unstaged read (`unstaged_within` in `crates/repo/src/status.rs`) uses it, folds only ASCII on its own side, and lets git's answer win for any entry git returns.
