# Local write Jobs always run to completion

The plan said writes are cancellable and that a newer job of the same kind cancels the older one. We rejected both. Superseding loses work: staging file B would cancel staging file A. Killing git skips its cleanup, because `Child::kill` is SIGKILL on Unix and `TerminateProcess` on Windows, so it can leave a stale `index.lock` or a half-applied rebase. Protecting the user's work outranks responsiveness here.

Only Read Jobs can be Superseded. Network Jobs stop only when the user asks, and a refresh follows. Local write Jobs are queued per repo and never stopped; leaving a merge or rebase is an Abort, which is itself a Local write Job. Pull is split into a Network Job and a Local write Job, so a dead connection can't freeze local work. This makes Pull the one exception to "one operation, one `git` command", so its behaviour needs golden tests against the real `git pull`.

Quit obeys the same rule. Cairn stays alive until Local write Jobs finish, because exiting closes git's pipes and can kill it mid-step. A warned Force quit is the only deliberate escape, so a hung hook has no everyday stop button: its output is shown live, and it can be killed from the terminal.

## Consequences

- Network Jobs and Local write Jobs use separate queues and may overlap, so lock collisions are a normal outcome, as they already are beside a terminal. Cairn shows git's own error and doesn't retry.
- Read Jobs must never take a lock, so CLI reads use `--no-optional-locks`. Index refreshes and `git commit-graph write` count as Local write Jobs even though users don't see them as writes.
- Cairn never deletes a lock file it finds, because it may belong to the user's terminal.
