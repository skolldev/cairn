# The minimum git is 2.37

The minimum was 2.23, because `switch` and `restore` arrived there, but nothing weighed that against the gits people have, and 2.23 dates from August 2019. We raised it to 2.37 (June 2022) before the first public release, while raising it locks nobody out (#46).

2.37 is the step users would notice. From 2.37, `fetch --all` fetches a lone remote directly, so `--progress` works, and every repo with one remote shows fetch progress. Below it, none does. The floor also makes `fetch.writeCommitGraph`, `--pathspec-from-file`, `git init -b` and sparse indexes always available.

- **macOS:** the Command Line Tools for Xcode 14 are reported to have git 2.37 and to need macOS 12.5. That comes from Apple forum replies and a CI image listing, not Apple's release notes. Older tools are refused until they are updated.
- **Windows:** Git for Windows is installed from its own releases, which are far ahead of 2.37 (2.56 in October 2026). We have no figures for what is actually installed.
- **Linux:** best effort, so its distros didn't set the floor. Ubuntu 22.04 ships 2.34.1 and is refused. Debian 12 ships 2.39.5.

## Consequences

- A Mac below macOS 12.5 can't get those tools, so it needs git from elsewhere, such as Homebrew. macOS is a first-class target and Cairn states no minimum macOS, so we can't say how many people this reaches. We accept it to get fetch progress on every repo with one remote.
- Someone on Ubuntu 22.04 needs a newer git from outside the distro's own archive. We accept that because Linux is best effort. If Linux becomes a target, this floor has to be weighed again.
- We chose this without usage data. The refusal names the version and the path, so someone below the floor can see why and what to change.
- `fetch --all` still drops `--progress` with several remotes, and partial clones still differ between 2.39 and 2.56. No floor fixes those.
- The oldest-git CI job, `scripts/git-matrix.sh` and the startup check still apply, now at 2.37. `scripts/oldest-git.sh` keeps `-std=gnu11` because 2.37 still uses `thread_local` as a name. It also keeps `-fpermissive`, untested: 2.37 built without it on Apple clang 15, but GCC 14 and newer weren't tried.
