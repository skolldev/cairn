# PROTOTYPE: git process cost spike (issue #12)

Throwaway. Not part of Cairn. Answers one question: what does running `git` cost
on Windows, and does it break anything?

No Floor hangs on this (see #8). Spawn cost goes to the budget rewrite, and
breakage is a checklist of bugs for the git CLI runner.

Stack: Rust 1.99.1, std only. Builds unchanged on macOS and Windows.

## Run

From this folder, in release mode, with nothing heavy running:

```bash
cargo run --release -- bench
```

```bash
cargo run --release -- paths
```

Flags: `--iters 100` spawns per run, `--runs 3`, `--warmup 5`,
`--fixtures ~/cairn-fixtures`, `--only <case>`, `--git <exe>` to add another git,
`--root <dir>` for scratch repos (default: the OS temp dir).

`bench` uses the `ripgrep` and `linux-graph` fixtures read-only. Every write goes
to a scratch repo in the temp dir, which is deleted afterwards.

## What it measures

**Round trip:** `Command::spawn` to `wait_with_output` returning, with stdout and
stderr piped and stdin null (or piped, for `hash-object`). That is what the CLI
runner pays per call.

**Spawners.** On Windows:

- `direct`: `Git\mingw64\bin\git.exe`, the real git.
- `shim`: `Git\cmd\git.exe`, a launcher that starts the real git as a second
  process. A GUI app started from Explorer gets the registry `PATH`, which on a
  default Git for Windows install has only `Git\cmd`. So resolving `git` from
  `PATH` lands on the shim.
- `no-window`: `CREATE_NO_WINDOW`, which a GUI app must pass so no console
  flashes up. The child gets a fresh hidden console.
- `detached`: `DETACHED_PROCESS`, no console at all.
- `inherit`: shares the bench's console. Reference only: a GUI parent has no
  console, so this would open a visible window.
- `self-noop`: this exe with `noop`, which exits at once. It's the OS cost of
  making a process, with no git in it.

On macOS: `direct` is `xcrun --find git`, and `shim` is `/usr/bin/git`, which is
Apple's xcrun shim. Both inherit, since macOS has no console flags.

**Cases:**

| Case | What | Why |
| --- | --- | --- |
| `noop` | the bench itself, exiting at once | OS process creation floor |
| `version` | `git --version` | git startup with no repo |
| `rev-parse small` | `rev-parse HEAD` in `ripgrep` | the cheapest repo read |
| `status small` | `status --porcelain=v2 -z` in `ripgrep` | a real refresh on a small repo |
| `rev-parse kernel` | `rev-parse HEAD` in `linux-graph` | repo discovery on a big repo |
| `diff-index kernel` | `diff-index --cached --quiet HEAD` | reads the 97k-entry index and HEAD's tree, the read half of a commit, read-only |
| `hash-object stdin` | `hash-object -w --stdin` in scratch | writing through a stdin pipe |
| `update-ref` | `update-ref` in scratch | the smallest mutation |
| `commit scratch` | `commit --allow-empty` in scratch | a whole commit on a tiny repo |
| `parallel` | 8 threads of `rev-parse small` | background refresh firing reads at once |

`first` is the first spawn of each run, before warmup. Summary rows are the
worst p50, p95 and max across runs.

**`paths`:** a pass/fail checklist on scratch repos:

- a repo under `dir with spaces/Ümläut ß 日本/`, with a file named
  `file with spaces ä 🎉.txt`, through `-C` and through the child's working
  directory: init, add, commit, status, log, restore, a branch named
  `feature/ümlaut`
- a repo whose own path is over 260 characters, through `-C` and the working
  directory, with and without `core.longpaths`
- a file path over 260 characters in a short repo: add, commit, status, restore
  and clone, with and without `core.longpaths`
- argument quoting through both exes: commit messages with quotes, backslashes,
  a trailing backslash, `%PATH%`, cmd metacharacters, newlines, tabs, emoji and
  a leading dash; a message on stdin; pathspecs such as `-dash.txt` and
  `semi;colon & amp.txt`

**`console`:** a post-commit hook runs this exe, which reports whether git's own
child got a console and whether that console is visible. It runs once per
spawner. Then it times commit with auto-maintenance on and off.

## macOS checklist

- [ ] `cargo run --release -- bench > results/macos-bench.log`
- [ ] `cargo run --release -- paths > results/macos-paths.log`
- [ ] `cargo run --release -- console --iters 50 > results/macos-console.log`
- [ ] Paste the three logs into #12.

Expected on macOS: `shim` against `direct` shows what the xcrun shim costs. The
long-path rows should pass, since `PATH_MAX` is 1024.

## Windows results

Recorded 2026-11-07 on the physical PC from #5: Ryzen 7 7800X3D, Windows 11 Home
26200, Git for Windows 2.52.0, fixtures on the SATA drive. Defender was in its
normal state: real-time, on-access, behaviour monitoring and tamper protection all
on, with neither the fixtures nor `git.exe` excluded. `LongPathsEnabled=0`, the
Windows default. 3 runs of 100 timed spawns per case, after 5 warmup spawns.
Raw logs are in `results/`, which is gitignored.

### Spawn cost, worst p50 of 3 runs, ms

| Case | shim, no-window | direct, no-window | direct, detached |
| --- | ---: | ---: | ---: |
| `noop` (OS floor, no git) | | 12.8 | 4.5 (inherit) |
| `version` | 24.1 | 15.8 | 7.6 |
| `rev-parse small` | 25.4 | 17.2 | 9.2 |
| `rev-parse kernel` | 25.4 | 17.2 | |
| `status small` | 30.3 | 22.0 | |
| `diff-index kernel` | 35.1 | 26.8 | |
| `hash-object stdin` | 25.3 | 17.2 | |
| `update-ref` | 26.0 | 17.8 | |
| `commit scratch` | 52.2 | 44.1 | 36.7 |

p95 sits 1 to 5 ms above p50 in every row. Six single spawns out of about 7,000
took 110 to 190 ms: one each in six different runs, never two in a row, and
mostly on the shim. That looks like Defender or the scheduler, but it wasn't
isolated.

The `first` column, the first spawn of each run, is at most 20 ms over p50 (60.7
against 26.8 for `diff-index kernel`, first run). git had run since boot, so this
isn't a true cold start.

### Where the time goes, p50

| Part | Cost | Evidence |
| --- | ---: | --- |
| Making a process | 4.5 ms | `noop` inherit |
| Hidden console from `CREATE_NO_WINDOW` | +8 ms | `noop` 4.5 → 12.8, `version` 7.5 → 15.8 |
| git starting, no repo | +3 ms | `version` against `noop` |
| Finding the repo | +1.5 ms | `rev-parse` against `version`. The kernel costs the same as ripgrep |
| The `Git\cmd` shim | +8 ms | shim against direct, in every row |
| Auto-maintenance after commit | +13 ms | `commit` 44.5 → 31.2 with `maintenance.auto=false` |

`CREATE_NO_WINDOW` doesn't skip the console. It creates a hidden one, which means
a `conhost.exe` per spawn. `DETACHED_PROCESS` skips it, and so does sharing an
existing console. The shim is a second process with its own console, so it pays
both costs again.

**Typical GUI app** (finds `git` on `PATH`, passes `CREATE_NO_WINDOW`): about
25 ms per read and 52 ms per commit. **Tuned** (real exe, detached, no
auto-maintenance): about 9 ms per read and 23 ms per commit.

**Parallel:** 8 threads each running `rev-parse small` (direct, no-window)
reached 316 to 327 spawns a second. Per-spawn p50 rose from 17 to 24 ms, and the
worst spawn took 39 ms.

### Does `DETACHED_PROCESS` flash windows?

A detached git has no console, so the worry is that each of its console children
opens a visible one. The `console` probe ran a post-commit hook (git → `sh` →
probe exe):

| Spawner | Hook's console |
| --- | --- |
| direct or shim, no-window | none visible (the hidden console has no window) |
| direct, detached | a console exists, not visible |

Git for Windows gives its own children a hidden console, so hooks don't flash.
This was a single probe of one hook path. Credential helpers, `ssh`, `gpg` and
askpass prompts weren't tested.

### Breakage checklist

75 checks, 8 failures, all from paths over 260 characters. Full log:
`results/windows-paths.log`.

**Passes:**

- **Spaces, umlauts, CJK and emoji** in the repo path, file names and branch
  names. Both `-C` and the child's working directory work.
- **Argument quoting:** every commit message round-tripped through both the shim
  and the real exe. That includes `"`, `\`, a trailing `\`, `\"`, `%PATH%`,
  `!`, `^`, `&`, `|`, `<`, `>`, newlines, tabs, emoji and a leading `-`. A
  message on stdin (`-F -`) round-tripped too. Rust's std quoting plus the shim's
  re-quoting broke nothing.
- **Pathspecs:** `-dash.txt`, `semi;colon & amp.txt`, `%PATH%.txt`,
  `[brackets].txt` and `quote'single.txt` all worked after `--`.
- **Long file paths inside a short repo,** with `-c core.longpaths=true`: add,
  commit, status, `checkout -- .` and clone all worked, at 307 characters.

**Bugs for the runner:**

1. **A repo whose own path is over 260 characters can't be used by git at all.**
   `-C` fails with `cannot change to '…': Filename too long`. Setting the child's
   working directory fails at `CreateProcessW` with os error 267. Setting
   `core.longpaths` doesn't help, whether through `-c` before `-C`, through
   `GIT_CONFIG_COUNT` or through a global config file. Neither does
   `--git-dir`/`--work-tree`. The limit is the process working directory, which
   `core.longpaths` doesn't cover. Cairn can't fix this, so it needs to detect it
   and say so. Untested: whether `LongPathsEnabled=1` lifts it, which needs an
   admin registry change.
2. **Long file paths fail silently without `core.longpaths`.** `add` exits 128.
   Worse, `status -uall` exits 0 with empty output, so the untracked file just
   isn't there. With the file tracked, `status` reports it as deleted (`.D`),
   even though it's on disk. `clone` fails. The runner should pass
   `-c core.longpaths=true` on every call, because the user's config can't be
   relied on: it's unset on this machine.
3. **Paths in output are octal-escaped unless `-z`** (`core.quotepath`). Parse
   `-z` output everywhere.

### For the CLI runner spec

- **Run the real exe, not the shim.** Find `mingw64\bin\git.exe` next to
  `cmd\git.exe`, or use `git --exec-path`. Saves 8 ms a call.
- **Prefer `DETACHED_PROCESS` to `CREATE_NO_WINDOW`.** Saves 8 ms a call. Hooks
  stay hidden, but check helpers and prompts before relying on it.
- **Own maintenance.** `-c maintenance.auto=false` on writes saves 13 ms a
  commit, but then Cairn has to schedule maintenance itself as a Job.
- **Always pass `-c core.longpaths=true`, and always use `-z`.**
- **Use `-C`** rather than the working directory. Both work, and `-C` gives a
  readable error at the limit.

### For the budget rewrite and #13

A commit as Cairn would run it is about four spawns: `add`, `commit`, then
`status` and a log refresh. That's about 125 ms the typical way and about 50 ms
tuned, before any git work on a big repo. So the write path can meet a 100 ms
interaction budget on Windows only if it's tuned. A CLI fallback for reads costs
at least 9 ms a call, so it can't sit on a per-keystroke or per-frame path, but a
single read per refresh is cheap.

macOS numbers are pending (see the checklist), so the cross-OS comparison the
ticket asks for isn't done.
