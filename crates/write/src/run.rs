//! One git invocation in, git's exit status and output back.

use std::borrow::Cow;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;

use crate::{Git, RunError, git_command};

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
}

impl Invocation {
    pub fn new(class: JobClass, repo: impl Into<PathBuf>) -> Self {
        Self {
            class,
            repo: repo.into(),
            args: Vec::new(),
            stdin: None,
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

    pub fn repo(&self) -> &Path {
        &self.repo
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
        body.split(|&b| b == 0)
            .filter(move |_| !self.stdout.is_empty())
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

/// On Windows git can't use a repository whose own path is over this many characters, even
/// with `core.longpaths` (#12).
pub const MAX_WINDOWS_REPO_PATH: usize = 260;

impl Git {
    /// Runs git once and waits for it. A non-zero exit is not an error here: it comes back in
    /// [`Output::status`], and [`Output::success`] turns it into one.
    pub fn run(&self, invocation: &Invocation) -> Result<Output, RunError> {
        #[cfg(windows)]
        check_windows_repo_path(&invocation.repo)?;

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

        let spawn_error = |e: io::Error| RunError::Spawn {
            path: self.path().to_owned(),
            source: Arc::new(e),
        };
        let mut child = cmd.spawn().map_err(spawn_error)?;
        let pipe = child.stdin.take();
        // Feed stdin from its own thread while this one drains stdout and stderr, so a large
        // patch can't deadlock against a full output pipe.
        let out = std::thread::scope(|scope| {
            if let (Some(mut pipe), Some(bytes)) = (pipe, invocation.stdin.as_deref()) {
                scope.spawn(move || {
                    // git may exit before reading everything, for example on a bad option.
                    // Its exit status and stderr say why; a broken pipe adds nothing.
                    let _ = pipe.write_all(bytes);
                });
            }
            child.wait_with_output()
        })
        .map_err(spawn_error)?;
        Ok(Output {
            status: out.status,
            stdout: out.stdout,
            stderr: out.stderr,
        })
    }
}

#[cfg(windows)]
fn check_windows_repo_path(repo: &Path) -> Result<(), RunError> {
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
