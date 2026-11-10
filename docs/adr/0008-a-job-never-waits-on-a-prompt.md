# A Job never waits on a Prompt

On Windows, git set to use Windows' own `ssh.exe` hangs a Network Job when `ssh` asks a question, such as whether to trust an unknown host. Nobody can see the question, git writes nothing to stderr, and the Job ends only when the user stops it (#44, found in #26). `GIT_TERMINAL_PROMPT=0` stops git's own terminal prompts, but not `ssh`'s.

The rule is wider than that case: **a Job never waits on a Prompt Cairn can't show.** Until Cairn can show one, the Prompt fails, and the user sees the program's own error exactly as written, with no hint from Cairn. Showing the Prompt in Cairn's own window comes later.

The runner does this for every git run it starts, not only for Network Jobs. Signing a commit with an SSH key runs `ssh-keygen`, which can ask for a passphrase inside a Local write Job, and a Local write Job can't be stopped (ADR 0002). That would be worse than #44.

**How:** the runner sets `SSH_ASKPASS` to a small Cairn helper and `SSH_ASKPASS_REQUIRE=force`. Since OpenSSH 8.4, `force` sends every Prompt to the askpass program and never to a terminal. For now the helper refuses, so `ssh` stops with its own error, such as `Host key verification failed.` Later, the same helper is how Cairn shows the Prompt.

- **A user's own askpass is kept.** If `SSH_ASKPASS` is already set, Cairn leaves it, because that Prompt is one the user can see. Cairn sets `SSH_ASKPASS_REQUIRE=force` unless the user chose a value.
- **git's own HTTPS prompts don't change.** git falls back to `SSH_ASKPASS` for its username and password prompts. Cairn keeps those exactly as they were: refused under `GIT_TERMINAL_PROMPT=0`, or answered by a credential helper such as Git Credential Manager. The helper serves `ssh` only, until Phase 5 or the step to showing Prompts decides otherwise.
- **How git is kept apart (#44).** git asks `GIT_ASKPASS`, then `core.askPass`, then `SSH_ASKPASS`, and an askpass that fails adds `error: unable to read askpass response` to git's error. So the runner sets `GIT_ASKPASS` to empty unless the user set it. `ssh` sees only git's environment, so nothing narrower reaches `ssh` alone without rewriting the user's `ssh` command.
- **The helper is `cairn-askpass`**, a binary of `cairn-write`, found beside the running program or one directory up, where cargo puts test binaries. It takes the Prompt's text as its argument and, for now, exits non-zero with no answer.

## Considered options

- **`-o BatchMode=yes`.** It avoided the hang in #26. But git has no setting that adds an option to the user's `ssh` command, so Cairn would have to work out that command from `GIT_SSH_COMMAND`, `GIT_SSH` and `core.sshCommand` and rewrite it, and plink-style programs need `-batch` instead. It also leaves nothing to build the step to showing Prompts on.
- **Show the Job as waiting after a time with no output.** That needs a timer, which ADR 0004 avoided, and it still gives the user no way to answer.
- **Show the Prompt in Cairn now.** This is where the rule leads, but it's new UI, and it fits next to Phase 5's credential helper. The askpass helper is the piece both steps share.

## Consequences

- A user who types a key passphrase into a terminal each time can't do that inside Cairn until Cairn shows Prompts. A key held in `ssh-agent`, including Windows' OpenSSH agent service, still works, and so does an askpass the user already set.
- This has to land with or before the first control in the app that starts a Network Job. Until then, nothing in the app can reach a Prompt.
- That it stops the hang with Windows' `ssh.exe` 9.5p2 is a claim to prove on Windows, not something already measured. A [TortoiseGit report](https://gitlab.com/tortoisegit/tortoisegit/-/issues/3996) says `SSH_ASKPASS_REQUIRE` mitigated the same passphrase hang on Win32-OpenSSH 8.9p1.
- **A `core.askPass` set only in git config isn't asked inside Cairn**, because the empty `GIT_ASKPASS` comes first. git refuses that prompt with `terminal prompts disabled`, as it does with no askpass at all. A `GIT_ASKPASS` in the user's environment and credential helpers work as before. Chosen on #44 over finding `core.askPass` on every run.
- **Known gaps, out of scope:** PuTTY's plink and TortoisePlink, which show their own prompt windows, and OpenSSH older than 8.4. The `ssh` programs Cairn meets today are macOS's (9.x), Git for Windows' (10.2) and Windows' own (9.5).
