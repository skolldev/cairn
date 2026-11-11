# Cairn

A native desktop git client built to replace GitKraken in daily work. This glossary fixes the language used across the plan, issues and code.

## Language

**Job**:
A single piece of git work done on the user's behalf, either a read of the repository or a run of the `git` CLI.
_Avoid_: Task, operation, command

**Repo service**:
The UI-independent heart of Cairn for one open repository. It accepts Intents and publishes Snapshots, and owns everything between them.
_Avoid_: Backend, core

**Intent**:
Something the user wants from the repository, such as selecting a commit or pulling. The UI speaks in Intents and never in Jobs.
_Avoid_: Action, request

**Snapshot**:
An immutable view of repository state published by the Repo service for the UI to show.

**Job scheduler**:
The private part of the Repo service that decides which Jobs run, in what order, and when one stops.
_Avoid_: Executor, runtime

**Read Job**:
A Job that only looks at the repository, whose result nobody needs once a newer one replaces it.

**Interactive lane**:
The Read Jobs answering something the user just did. They have capacity that background work can never take.

**Background lane**:
The Read Jobs with no one directly waiting on them, such as the history walk or a message search.

**Slot**:
One purpose a Read Job serves, such as "file list for the selected commit". A Slot holds at most one running and one pending Read Job.

**Network Job**:
A Job that talks to a remote. It stops only when the user asks.

**Local write Job**:
A Job that changes the local repository. Once started it always runs to completion.
_Avoid_: Write (alone, because a push also writes)

**Prompt**:
A question that git, or a program git runs such as `ssh`, asks the user before a Job can go on. Cairn either shows it or makes it fail. A Job never waits on one.
_Avoid_: Dialog, popup (those are Cairn's own windows)

**Quit**:
Leaving Cairn without risk to the repository: Read Jobs are dropped, Network Jobs stopped, and Local write Jobs run to their end first. It is itself an Intent.

**Force quit**:
Leaving Cairn without waiting for Local write Jobs, after a warning, because exiting can kill git mid-step. It is the only way out while one runs, such as under a hung hook.

**Pull**:
One user action made of two Jobs: a Network Job that fetches the upstream, then a Local write Job that integrates it. The second starts only if the first succeeds.

**Stash and switch**:
One user action made of two Local write Jobs: one that stashes the local changes, then the switch. The second starts only if the first succeeds. It is offered only when git refused a switch because local changes would be overwritten.

**External change**:
A change to the repository made outside Cairn, such as a file saved in an editor or a `git` command run in a terminal. Cairn learns of it through the watcher, never from a Job of its own. Cairn's own Jobs wake the watcher too, and the extra refresh coalesces in the Slot.

**Recheck**:
Reading again, when the user comes back to the window, every Slot whose last Snapshot was an error. It answers the user looking again, not the failure, so it is not a retry.
_Avoid_: Retry, refresh

**Supersede**:
To drop a Read Job because a newer Read Job for the same purpose has made its result unwanted.
_Avoid_: Cancel

**Abort**:
Git's own exit from an in-progress merge, rebase, cherry-pick or revert. It is a Local write Job, not a way of stopping one.
_Avoid_: Cancel

**Budget**:
The performance one interaction must reach for a phase gate to pass. A Budget is set from measurements, and a Budget with no measurement behind it is unproven and gates nothing until a phase measures it.
_Avoid_: Target

**Floor**:
The worst result that still passes a Phase 0 gate, fixed before any measurement. Each Floor names the one component that a miss condemns, and it never moves once results are in.
_Avoid_: Budget, threshold
