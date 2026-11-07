# Coding standards

Judgement calls a reviewer applies to every change. Rules a script can check live in `scripts/` and run in CI instead: GPUI only in `cairn-app` (`check-gpui-free.sh`), and git spawned only by `cairn-write` (`check-single-git-spawn.sh`). Domain terms come from `CONTEXT.md`; decisions from `docs/adr/`.

## Tests

From #14's testing decisions.

- **Seams.** A test drives the system through a public seam and asserts what a caller could observe. There are two seams:
  - **`cairn-repo`'s interface** (Intents in, Snapshots out) is the main one. Assert Snapshot contents, the order and generation of Snapshots, Job outcomes, git's error text, and the repository on disk afterwards.
  - **`cairn-write`'s runner** (an `Invocation` in, `Output` or git's error out) is for rules awkward to reach through Intents.
- **Observable behaviour.** Tests stay out of the scheduler's queues, the runner's argument list and gix internals. When a runner rule needs proving, prove it through what git itself reports (`git config --get`, an `!env` alias, the index on disk).
- **Real git.** Tests run real git against real repositories. Where timing matters, control it through git's own extension points (hooks, a wrapper around `upload-pack`), never by injecting fakes into private modules.
- **Hermetic config.** Tests run git with an empty user config and no system config, so a developer's `commit.gpgsign` or `core.longpaths` can't change the result. `crates/write/tests/common` shows the pattern: set `HOME`, `XDG_CONFIG_HOME` and `GIT_CONFIG_NOSYSTEM` once per test binary, before anything spawns git.
- **Oldest git.** Runner tests pass on git 2.23. Use only git features that 2.23 has (no `init -b`, no `--pathspec-from-file`). Check with `scripts/oldest-git.sh`.
- **Controls.** A test that asserts something *didn't* happen pairs it with a control showing the setup would make it happen otherwise. `reads_never_take_the_index_lock` is the example.
- **Benchmarks are not tests.** A criterion regression threshold is a CI signal. Budget measurements come from a local harness run before a phase gate.
