# The `repo` crate owns the Repo service and runs Jobs on its own threads

The plan said "GPUI executors only" and also that every crate except `app` survives a UI swap. Those can't both hold for a scheduler outside `app`, because `BackgroundExecutor` needs a GPUI `PlatformDispatcher`. We decided that an eighth crate, `repo`, owns the Repo service: the Job scheduler (as a private module), the watcher, Snapshots, `redb` access and the CLI fallback. Jobs run on plain threads that `repo` owns. `app` sends Intents, drains Snapshots through a coalesced bridge, and never sees a Job. GPUI executors are for UI-side async only, and there is still no tokio.

Git work blocks rather than awaits: gix calls are synchronous, and a git Job mostly waits on a process. Dropping a GPUI `Task` can't stop a future that never yields, so stopping needs a cooperative flag or a process kill either way, and GPUI would add only a thread pool. Owning the threads lets us cap them against the 8 ms budget and test `repo` headless.

## Considered Options

- **Scheduler in `app`:** the plan's literal wording. It rewrites all git behaviour on a no-go and can't be tested headless.
- **Scheduler in a GPUI-free crate with an injected GPUI spawner:** adds an abstraction and keeps the blocking problem.
- **Generic `jobs` crate:** leaves the watcher, cache, Pull sequencing and refresh behaviour to land in `app`, which hollows out the hedge. A ninth crate alongside `repo` adds a public interface with one user. Split it out later only if the private module proves too tangled.

## Consequences

- A move away from GPUI costs the whole UI layer but no git behaviour. Panels, the custom graph element, the diff view, keymaps and the bridge are rewritten; every crate except `app` is kept.
- `repo` becomes the largest non-UI crate, so the scheduler needs a tight internal boundary.
- We own thread shutdown and priorities, including macOS QoS. Check GPUI's dispatcher behaviour on the `gpui-pre` pin before assuming what we must replicate.
