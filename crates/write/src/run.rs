//! One git invocation in, git's exit status and output back.

use std::borrow::Cow;
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;

#[cfg(windows)]
use crate::MAX_WINDOWS_REPO_PATH;
use crate::{Git, RunError, StartGate, Stop, git_command};

/// What a Job is, as far as spawning git goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobClass {
    /// Only looks at the repository. Passes `--no-optional-locks`, so it never takes a lock.
    Read,
    /// Changes the local repository.
    LocalWrite,
    /// Talks to a remote.
    Network,
}

/// How Windows starts git, which decides what console git and the programs it runs get.
///
/// Cairn ships [`NoWindow`](Self::NoWindow). `docs/detached-process.md` holds the evidence
/// for a later decision on [`Detached`](Self::Detached), and `crates/write/tests/windows_console.rs`
/// is what gathers it. Nothing else sets this.
#[cfg(windows)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindowsProcess {
    /// `CREATE_NO_WINDOW`: a console with no window, which git's children share.
    #[default]
    NoWindow,
    /// `DETACHED_PROCESS`: no console at all, which saves about 8 ms a call.
    Detached,
}

#[cfg(windows)]
impl WindowsProcess {
    pub(crate) fn creation_flag(self) -> u32 {
        match self {
            WindowsProcess::NoWindow => 0x0800_0000,
            WindowsProcess::Detached => 0x0000_0008,
        }
    }
}

/// One run of git: where, with what arguments and stdin, and for which class of Job.
///
/// Commit messages and patches belong in [`stdin`](Self::stdin), never in arguments, so
/// that no character in them can be read as an option or mangled by a shell.
#[derive(Debug, Clone)]
pub struct Invocation {
    class: JobClass,
    repo: PathBuf,
    args: Vec<OsString>,
    stdin: Option<Vec<u8>>,
    stop: Option<Stop>,
    gate: Option<StartGate>,
    #[cfg(windows)]
    windows_process: WindowsProcess,
}

impl Invocation {
    pub fn new(class: JobClass, repo: impl Into<PathBuf>) -> Self {
        Self {
            class,
            repo: repo.into(),
            args: Vec::new(),
            stdin: None,
            stop: None,
            gate: None,
            #[cfg(windows)]
            windows_process: WindowsProcess::default(),
        }
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I>(mut self, args: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Bytes written to git's stdin. Without this, git's stdin is empty.
    pub fn stdin(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(bytes.into());
        self
    }

    /// Lets `stop` end this run. A [`JobClass::LocalWrite`] invocation can't be stopped
    /// (ADR 0002): it is refused with [`RunError::NotStoppable`] before git starts.
    pub fn stop_on(mut self, stop: &Stop) -> Self {
        self.stop = Some(stop.clone());
        self
    }

    /// Starts git only while `gate` is open. Every Local write Job passes through the Repo
    /// service's gate, so that none starts once a Force quit has decided to exit.
    pub fn through(mut self, gate: &StartGate) -> Self {
        self.gate = Some(gate.clone());
        self
    }

    /// Starts git under `process` instead of the shipped [`WindowsProcess::NoWindow`].
    #[cfg(windows)]
    pub fn windows_process(mut self, process: WindowsProcess) -> Self {
        self.windows_process = process;
        self
    }
}

/// What git did: its exit status, its stdout, and its stderr exactly as written.
#[derive(Debug, Clone)]
pub struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Output {
    /// The NUL-terminated records of `-z` output, without the terminators. Paths come back
    /// as git stored them, never octal-escaped, whatever `core.quotepath` says.
    pub fn records(&self) -> impl Iterator<Item = &[u8]> {
        let body = self.stdout.strip_suffix(b"\0").unwrap_or(&self.stdout);
        // Empty output holds no records, not one empty one.
        let records = (!body.is_empty()).then(|| body.split(|&b| b == 0));
        records.into_iter().flatten()
    }

    /// stderr as text, for showing to the user. Invalid UTF-8 is replaced, nothing else.
    pub fn stderr_text(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.stderr)
    }

    /// This output if git exited 0, otherwise [`RunError::Failed`] carrying git's stderr.
    pub fn success(self) -> Result<Output, RunError> {
        if self.status.success() {
            Ok(self)
        } else {
            Err(RunError::Failed {
                status: self.status,
                stderr: self.stderr,
            })
        }
    }
}

impl Git {
    /// Runs git once and waits for it. A non-zero exit is not an error here: it comes back in
    /// [`Output::status`], and [`Output::success`] turns it into one.
    pub fn run(&self, invocation: &Invocation) -> Result<Output, RunError> {
        self.run_streaming(invocation, |_| {})
    }

    /// Like [`run`](Self::run), and also hands `on_stderr` each piece of stderr as git
    /// writes it, so that hook output and progress can be shown while git runs. Hooks write
    /// to git's stderr, their stdout included. The returned [`Output::stderr`] still holds
    /// all of it.
    pub fn run_streaming(
        &self,
        invocation: &Invocation,
        mut on_stderr: impl FnMut(&[u8]),
    ) -> Result<Output, RunError> {
        #[cfg(windows)]
        check_windows_repo_path(&invocation.repo)?;
        if invocation.stop.is_some() && invocation.class == JobClass::LocalWrite {
            return Err(RunError::NotStoppable {
                class: invocation.class,
            });
        }

        let mut cmd = git_command(self.path());
        cmd.arg("-C")
            .arg(&invocation.repo)
            .args(["-c", "core.longpaths=true", "--no-pager"]);
        if invocation.class == JobClass::Read {
            cmd.arg("--no-optional-locks");
        }
        cmd.args(&invocation.args)
            .stdin(if invocation.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // A stoppable git leads its own process group, so that stopping it reaches the
        // processes it starts too.
        #[cfg(unix)]
        if invocation.stop.is_some() {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(invocation.windows_process.creation_flag());
        }

        let spawn_error = |e: io::Error| RunError::Spawn {
            path: self.path().to_owned(),
            source: Arc::new(e),
        };
        let spawned = match (&invocation.stop, &invocation.gate) {
            (Some(stop), _) => match stop.spawn_unless_stopped(|| cmd.spawn()) {
                Some(spawned) => spawned,
                None => return Err(RunError::Stopped { stderr: Vec::new() }),
            },
            (None, Some(gate)) => match gate.spawn_unless_closed(|| cmd.spawn()) {
                Some(spawned) => spawned,
                None => return Err(RunError::GateClosed),
            },
            (None, None) => cmd.spawn(),
        };
        let mut child = spawned.map_err(spawn_error)?;
        let pipe = child.stdin.take();
        let mut stdout = child.stdout.take().expect("stdout is piped");
        let mut stderr = child.stderr.take().expect("stderr is piped");
        // Feed stdin and drain stdout from their own threads while this one reads stderr, so
        // a large patch can't deadlock against a full output pipe.
        let read = std::thread::scope(|scope| {
            if let (Some(mut pipe), Some(bytes)) = (pipe, invocation.stdin.as_deref()) {
                scope.spawn(move || {
                    // git may exit before reading everything, for example on a bad option.
                    // Its exit status and stderr say why; a broken pipe adds nothing.
                    let _ = pipe.write_all(bytes);
                });
            }
            let stdout = scope.spawn(move || {
                let mut bytes = Vec::new();
                stdout.read_to_end(&mut bytes).map(|_| bytes)
            });
            let mut err = Vec::new();
            let mut chunk = [0u8; 8192];
            let stderr_read = loop {
                match stderr.read(&mut chunk) {
                    Ok(0) => break Ok(()),
                    Ok(n) => {
                        on_stderr(&chunk[..n]);
                        err.extend_from_slice(&chunk[..n]);
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => {
                        // Keep the pipe drained, or git could block writing to it while
                        // the stdout reader waits for git to exit.
                        let _ = io::copy(&mut stderr, &mut io::sink());
                        break Err(e);
                    }
                }
            };
            let out = stdout.join().expect("the stdout reader doesn't panic");
            stderr_read.and(out).map(|out| (out, err))
        });
        if let Some(stop) = &invocation.stop {
            stop.finished();
        }
        // Always wait, even when reading failed: git is never left running unwatched.
        let status = child.wait().map_err(spawn_error)?;
        let (stdout, stderr) = read.map_err(spawn_error)?;
        // A git that ended on its own before the stop reached it reports its own outcome.
        if !status.success() && invocation.stop.as_ref().is_some_and(Stop::reached_git) {
            return Err(RunError::Stopped { stderr });
        }
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }
}

#[cfg(windows)]
fn check_windows_repo_path(repo: &std::path::Path) -> Result<(), RunError> {
    use std::os::windows::ffi::OsStrExt;
    let absolute = std::path::absolute(repo).unwrap_or_else(|_| repo.to_owned());
    let chars = absolute.as_os_str().encode_wide().count();
    if chars > MAX_WINDOWS_REPO_PATH {
        return Err(RunError::RepoPathTooLong {
            path: absolute,
            chars,
        });
    }
    Ok(())
}
