# A failed read waits for the user to look again

A failed Read Job publishes git's error like any other answer, and nothing reads that Slot again until an External change, a Job's refresh or an Open (#43). The error sticks when its cause is fixed somewhere the watcher can't see, such as the global git config, or when the failure outlasts the last watcher event. Probes showed both: on macOS and Windows every Slot kept the error after the global config was fixed, and on a physical Windows PC with Defender real-time protection on, 2 of 4,635 git reads failed with `unable to access '.git/config': Permission denied` after the last event of a `.git/config` replacement, with every core busy (0 of 8,726 when idle).

We chose a Recheck. When the window gains focus, the UI sends `Intent::Recheck`, and the Repo service reads again, in full, every Slot whose last Snapshot was an error. With no such Slot it does nothing. Counts never publish an error: a failed count shows as no count and is tried again by the next count, which a rechecked Refs read that succeeds starts. A Recheck queues behind a running read as a watcher refresh does, so a read already under way still publishes and the Recheck's read follows it. A failed Open is rechecked too, since the path stays open. The usual way to fix a failure is outside Cairn, in a terminal or an editor, and coming back to the window is the moment the user looks again.

## Considered options

- **A Refresh command.** Rejected: switching away and back does the same, and a button suggests the view can be stale.
- **Retrying inside the Read Job before publishing.** Rejected: it delays every real error, telling transient failures apart means matching git's error text, and it would reverse ADR 0002's "Cairn shows git's own error and doesn't retry".
- **Keeping the last good list with the error above it.** Rejected: it shows stale data as current, which is its own way of disagreeing with git.
- **One delayed re-read after a failed read.** Held. It would cover the race while Cairn keeps focus, but that needs a program other than Cairn to replace `.git/config` under load, since no Cairn Job writes it, and the next focus gain clears it anyway. It also brings back a timer, which ADR 0004 avoided. Build it if an error is seen to stick while Cairn had focus.

## Consequences

- Recheck is not a retry: it answers the user looking again, not the failure, and fires no more than focus does. ADR 0002's rule stands.
- An error whose cause the watcher can't see stays on screen until the user leaves the window and comes back, or something else reads that Slot. We accept that.
- Recheck only re-reads Slots showing an error. A watcher blind spot that leaves wrong data with no error, such as a pattern removed from `core.excludesFile` (ADR 0004), is not covered.
- A failed refresh of Refs says `Couldn't read refs: <error>`; `Couldn't open` is kept for a failed Open.
