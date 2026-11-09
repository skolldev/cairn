# git behaviour by version

Cairn runs git 2.37 and newer (`GitVersion::MINIMUM`, ADR 0007). These are the git behaviours that set the minimum, changed after it, or surprise a caller, each checked in git's own source. Check a git command in a spec, product code or a test against this list, and add every new fact you confirm.

## Checking a fact

Read git's source at a release tag rather than its manual, which describes only the newest git:

```bash
gh api "repos/git/git/contents/builtin/fetch.c?ref=v2.37.0" -H "Accept: application/vnd.github.raw"
```

Release notes live at `Documentation/RelNotes/<version>.adoc`. To find the first version with a behaviour, grep the file at a few tags and narrow down.

Check the newest tag as well as 2.37 when a test leans on git's incidental behaviour rather than a documented promise, such as what makes git block or which step fetches. `scripts/git-matrix.sh` then runs the tests on both.

## Facts

| Behaviour | Since | Where it shows | Checked in |
|---|---|---|---|
| `fetch --all` with one remote fetches it directly | 2.37 | why 2.37 is the minimum: the fetch honours `--progress`. Before 2.37 even a lone remote goes through a sub-fetch | `builtin/fetch.c` at 2.36 and 2.37: "do not do fetch_multiple() of one" |

## Gotchas in every version

- **`fetch --all` drops `--progress`.** `--all` hands each remote to a sub-fetch, and `add_options_to_argv` never passes `--progress` down to it (checked from 2.23 through master). So progress reports appear only with one remote. Everything else git and the remote write still streams. See `crates/repo/src/network.rs`.
- **`--date-order` and `--topo-order` overwrite each other.** Both set `revs->sort_order`, so the later one wins, and either implies a topological sort. `git rev-list --topo-order --date-order` is `--date-order`.
- **`--all` starts from every ref, HEAD and every other worktree's HEAD.** Refs come first, by name (`handle_revision_pseudo_opt` in `revision.c`). So gix's walk gathers them itself, in that order. They aren't all branches: a stash or a custom ref counts.
- **`rev-list` in a partial clone fetches a tag's missing target, but not always a missing parent.** Peeling a tag from `--all` fetches on 2.39 and 2.56, through `remote.<name>.uploadpack`. A missing parent fetches on 2.39, while on 2.56 the walk fails with `Could not read`. The first version to change wasn't narrowed down. `--missing` and `--exclude-promisor-objects` turn fetching off. `held_walk` in `crates/repo/tests/lanes.rs` blocks a real walk through a tag for this reason. Measured by running each, with `commit.c` read at 2.39 and 2.56.
- **`-c` goes before the subcommand.** `git -c key=value fetch` sets config for that run. `git fetch -c key=value` doesn't set config: git parses the `-c` as one of `fetch`'s own options.
- **`remote.<name>.uploadpack` runs through a shell** for a local remote, with the repository path appended. `sh '<script>'` therefore works as a wrapper, and `remote.<name>.receivepack` does the same for a push. `slow_remote` in `crates/repo/tests/common` uses both to slow a remote down.
