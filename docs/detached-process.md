# `DETACHED_PROCESS` on Windows: the evidence

Cairn starts git with `CREATE_NO_WINDOW`. `DETACHED_PROCESS` saves about 7 ms a call, but #14 adopts it only once prompts are tested under it, because a hidden prompt would hang a push. This is that test (#26). The switch itself is a later decision, and nothing here changes the shipped default.

## Result

**`DETACHED_PROCESS` is safe to adopt on this evidence, with one check left.** Every prompt tested ended the same way under both flags, and no console window appeared under either. Nothing tested got worse.

- **Still blocking:** a person needs to see Git Credential Manager (GCM) ask for a sign-in under `DETACHED_PROCESS`. GCM never reached its prompt in these runs, under either flag (see below). It is the helper Git for Windows installs, so it is the prompt most users will meet.
- **Not a blocker, but needs its own fix:** Windows' own `ssh.exe` hangs on a prompt under *both* flags, so Cairn has this hang today. Switching flags neither helps nor harms it.

## What was measured

| Prompt | Under `CREATE_NO_WINDOW` (asserted) | Under `DETACHED_PROCESS` (recorded) |
|---|---|---|
| Hook | ran, exit 0 | same |
| Hook reading `/dev/tty` | finds no terminal, carries on | same |
| Credential helper | its answer reached the remote | same |
| Askpass program | its answer reached the remote | same |
| No helper and no askpass | fails at once: `terminal prompts disabled` | same |
| `ssh`, as git starts it | ran, and its ProxyCommand ran | same |
| `gpg` signing | signed commit | same |
| `gpg` with a passphrase, answered by a scripted pinentry | signed commit | same |

Run by hand, because they need GitHub or can open a dialog (`--ignored`), under both flags with the same outcome:

| Prompt | Outcome |
|---|---|
| Git for Windows' `ssh` asking about an unknown host | refuses in about 400 ms: `Host key verification failed.` |
| Windows' `ssh.exe` asking about an unknown host | **hangs**, silently, until stopped (killed after 20 s) |
| Control: Windows' `ssh.exe` with nothing to ask | ends in about 600 ms: `Permission denied (publickey)` |
| Windows' `ssh.exe` with `-o BatchMode=yes` | refuses in about 350 ms: `Host key verification failed.` |
| gpg's own pinentry | a visible dialog (`#32770`) waits for a person, as it should |
| GCM with no stored credential | fails in about 500 ms: `Cannot prompt because user interactivity has been disabled.` |
| GCM, as above, with `GIT_TERMINAL_PROMPT=1` | the same GCM error, so that setting isn't the cause |

Median time for one call, from 200 calls in each of 3 rounds, in a release build:

| | `CREATE_NO_WINDOW` | `DETACHED_PROCESS` | Saved |
|---|---|---|---|
| `git --version` | 14.4 ms | 7.0 ms | 7.4 ms |
| A git alias that runs a shell | 27.3 ms | 19.7 ms | 7.6 ms |

## The one visible difference

Under `CREATE_NO_WINDOW` the programs git runs through its shell get a console with no window. Under `DETACHED_PROCESS` they get a console with a real window that is hidden. No test saw that window on the desktop. The watcher that looks for it polls about every 2 ms, so a shorter flash could slip past it.

## How to read this

- **"Out of sight"** rests on two checks: a probe run by the prompt program, which asks Windows about its console, and a watcher on the desktop for new console windows. With `CREATE_NEW_CONSOLE` swapped in, both report the window.
- **Environment:** one Windows 11 machine (build 26200), Git for Windows 2.52.0, OpenSSH 10.2p1 from Git for Windows, OpenSSH for Windows 9.5p2, GCM 2.6.1, gpg 2.4.8. CI's `windows-latest` runs the unignored tests. The hand-run tables come from this one machine.
- **Which `ssh`:** git puts its own `usr/bin` first on `PATH`, so plain `ssh` is Git for Windows' copy, even with `System32\OpenSSH` on `PATH`. Windows' `ssh.exe` runs only when someone sets `core.sshCommand` or `GIT_SSH_COMMAND` to it, which is common among people using the Windows ssh-agent.

## Not covered

- **GCM's sign-in dialog.** Why GCM reports interactivity disabled is not known. It isn't `GIT_TERMINAL_PROMPT` or the flag, and whether Cairn's users would hit it outside these tests is an open question.
- **Git 2.23 on Windows.** These tests are Windows-only and the oldest-git job runs on Linux, so they have only run on the git above. They use nothing newer than 2.23.
- **`ssh` passphrase and password prompts.** Only the host-key prompt was run, since asking for a passphrase needs a server that accepts the key.

## Reproducing

```bash
cargo test -p cairn-write --test windows_console -- --nocapture
cargo test -p cairn-write --test windows_console record_ssh_prompts -- --ignored --nocapture
cargo test -p cairn-write --test windows_console record_real_dialogs -- --ignored --nocapture --test-threads=1
cargo test --release -p cairn-write --test windows_console record_spawn_cost -- --ignored --nocapture
```

To adopt the flag, move `#[default]` on `WindowsProcess` to `Detached`, and update `the_shipped_default_is_a_console_without_a_window`, which pins today's default.
