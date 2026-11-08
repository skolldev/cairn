# git behaviour by version

Cairn runs git 2.23 and newer (`GitVersion::MINIMUM`). These are the git behaviours that changed after 2.23 or that surprise a caller, each checked in git's own source. Check a git command in a spec, product code or a test against this list, and add every new fact you confirm.

## Checking a fact

Read git's source at a release tag rather than its manual, which describes only the newest git:

```bash
gh api "repos/git/git/contents/builtin/fetch.c?ref=v2.37.0" -H "Accept: application/vnd.github.raw"
```

Release notes live at `Documentation/RelNotes/<version>.adoc`. To find the first version with a behaviour, grep the file at a few tags and narrow down.

## Facts

| Behaviour | Since | Where it shows | Checked in |
|---|---|---|---|
| `git switch` and `git restore` exist | 2.23 | why 2.23 is the minimum | `GitVersion::MINIMUM` |
| `--pathspec-from-file` | 2.25 | can't be used, in product code or tests | `builtin/reset.c` |
| `git init -b` / `--initial-branch` | 2.28 | tests set the branch another way | `builtin/init-db.c` |
| `fetch.writeCommitGraph` | 2.24 | older git ignores the `-c`, so a test asserting a commit-graph is version-gated | `builtin/fetch.c`, `repo-settings.c` |
| `fetch --all` with one remote fetches it directly | 2.37 | before 2.37 even a lone remote goes through a sub-fetch | `builtin/fetch.c`: "do not do fetch_multiple() of one" |

## Gotchas in every version

- **`fetch --all` drops `--progress`.** `--all` hands each remote to a sub-fetch, and `add_options_to_argv` never passes `--progress` down to it (checked from 2.23 through master). So progress reports appear only for one remote on 2.37+. Everything else git and the remote write still streams. See `crates/repo/src/network.rs`.
- **`-c` goes before the subcommand.** `git -c key=value fetch` sets config for that run. `git fetch -c key=value` doesn't set config: git parses the `-c` as one of `fetch`'s own options.
- **`remote.<name>.uploadpack` runs through a shell** for a local remote, with the repository path appended. `sh '<script>'` therefore works as a wrapper, and `remote.<name>.receivepack` does the same for a push. `slow_remote` in `crates/repo/tests/common` uses both to slow a remote down.
