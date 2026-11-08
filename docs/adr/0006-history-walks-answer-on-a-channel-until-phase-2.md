# History walks answer on a channel until Phase 2

The Repo service is Intents in, Snapshots out, but #24 builds history walks while saying there's no history Snapshot until the graph Snapshot arrives in Phase 2. We decided that `RepoService::walk_history` is the one exception: a method that runs the walk as a Read Job in the Background lane and answers on a channel it returns. `app` doesn't call it, and nothing starts a walk yet. The graph Snapshot replaces it, with an Intent, in Phase 2.

## Considered Options

- **An Intent and a `Snapshot` variant holding the commits:** keeps the seam whole, but it is the history Snapshot the ticket rules out, and `app` would have to handle a variant only to ignore it. Its shape would also be guessed ahead of the delta format in ADR 0003.
- **Tests inside the scheduler module:** adds nothing to the public interface, but the coding standards keep tests out of the scheduler's queues, and they couldn't show that a walk and an Interactive read share a service without interfering.

## Consequences

- The walk's order, engine and stops are tested from outside the crate, as the graph Snapshot's will be.
- Delete the method, and move its tests to the new Snapshot, when Phase 2 adds the graph Snapshot.
