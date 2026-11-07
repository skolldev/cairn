# The runner runs the real git binary, not the shim on PATH

Both OSes put a shim on `PATH` that costs a few milliseconds a call: about 8 ms for Git for Windows' `Git\cmd\git.exe` and about 7 ms for macOS's `/usr/bin/git` (#12). Every Local write Job is a git process, so the runner resolves the real binary once per run and spawns that.

- **Windows:** the real binary is `Git\<environment>\bin\git.exe`, beside the `cmd` and `bin` shims, or found through `git --exec-path`. The environment was `mingw64` when #12 measured and #17 was written. Current Git for Windows ships `ucrt64`, and ARM builds ship `clangarm64`. The runner accepts all three, and its test does too.
- **macOS:** the first git on `PATH` that isn't `/usr/bin/git`, such as Homebrew's, or else what `xcrun --find git` names. A machine with no Command Line Tools gets a not-found error, not a spawn failure.
- **Elsewhere:** the first git on `PATH`. The git 2.23 CI job runs on Linux and relies on this.

## Consequences

- When Git for Windows changes its layout again, the `--exec-path` fallback still finds the binary, but the Windows resolution test fails until it learns the new environment name. That failure is deliberate: it is how the next layout change gets noticed.
- On Windows, a repository whose own path is over 260 characters is refused before git is spawned, because `core.longpaths` doesn't lift that limit (#12). The working-directory limit that `-C` meets is really about 258. We kept 260, as #17 specifies, and accept that a 259 or 260 character path fails inside git instead, with git's own error.
