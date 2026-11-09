# Watcher coalesces through the Slot, not a debounce timer

The plan had the watcher wait for a 100–200 ms debounce before refreshing, using `notify-debouncer-full`. We dropped the timer. A trailing debounce adds its whole length to every External change, so it can't fit the 100 ms External change Budget at all, while the read itself measured about 25–30 ms on both OSes (#11). Instead, the first watcher event starts a refresh of the affected Slot straight away. Events that arrive while it runs leave one pending refresh in that Slot, which already holds at most one running and one pending Read Job. A burst, such as a checkout writing thousands of files, costs at most two refreshes back to back.

## Consequences

- An atomic editor save (a temp file, then a rename) can briefly show the intermediate state. The pending refresh corrects it one read later. We accept that.
- A refresh never Supersedes the running read, only the pending one: the running read's answer is newer than what is shown, so it still publishes. A stream of changes longer than one read, such as a build writing for minutes, then publishes once per read instead of nothing until it stops (#21). Only a new question, such as an Open, Supersedes and stops the running read.
- There is no tunable quiet window. If Phase 1 finds refresh storms anyway, the fix belongs in the Slot or in how events map to Slots, not in a timer in front of them.
- A change to a file git ignores and doesn't track refreshes nothing (#32): the watcher keeps gix's exclude stack and the index, and asks them per path. It rebuilds them, and refreshes status, when `index`, `.git/info/exclude`, `config` or any `.gitignore` changes. `core.excludesFile` and the global git config live outside the watched root, so a pattern removed there leaves matching changes skipped until the next rebuild or Open. We accept that. Linked worktrees and submodules, whose index isn't watched, skip nothing.
