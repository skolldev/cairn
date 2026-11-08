//! The git CLI runner: the only place Cairn spawns git.
//!
//! [`Git::shared`] resolves the real git binary once per run. [`Git::run`] takes an
//! [`Invocation`] (a repository, arguments, optional stdin and the [`JobClass`] it serves)
//! and returns git's exit status, its stdout ready to split into `-z` records, and its
//! stderr exactly as git wrote it. The runner never rewrites an error and never retries.
//!
//! Every call runs as `git -C <repo> -c core.longpaths=true --no-pager [--no-optional-locks]
//! <args>` with `LC_ALL=C` and `GIT_TERMINAL_PROMPT=0`, so output parses the same everywhere
//! and git never waits on a prompt. Maintenance stays at git's default (#14).
//!
//! A Network invocation can carry a [`Stop`], which kills git and what it started.

mod resolve;
mod run;
mod stop;

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::Arc;

pub use resolve::{Git, GitVersion};
pub use run::{Invocation, JobClass, Output};
pub use stop::Stop;

/// Why Cairn can't run git on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// No git is installed where Cairn looks, including macOS without the Command Line Tools.
    NotFound { reason: String },
    /// A git was found but it is older than [`GitVersion::MINIMUM`].
    TooOld { path: PathBuf, version: GitVersion },
    /// A git was found but didn't run or didn't report a version.
    Unusable { path: PathBuf, reason: String },
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveError::NotFound { reason } => write!(f, "git was not found: {reason}"),
            ResolveError::TooOld { path, version } => write!(
                f,
                "Cairn needs git {} or newer, but {} is git {version}",
                GitVersion::MINIMUM,
                path.display()
            ),
            ResolveError::Unusable { path, reason } => {
                write!(f, "git at {} can't be used: {reason}", path.display())
            }
        }
    }
}

impl std::error::Error for ResolveError {}

/// On Windows git can't use a repository whose own path is over this many characters, even
/// with `core.longpaths` (#12). Counted in UTF-16 units,
/// the unit Windows path limits use.
pub const MAX_WINDOWS_REPO_PATH: usize = 260;

/// Why a git invocation didn't produce the output asked for.
#[derive(Debug, Clone)]
pub enum RunError {
    /// git exited non-zero. `stderr` is exactly what git wrote.
    Failed { status: ExitStatus, stderr: Vec<u8> },
    /// Windows only: the repository's own path is over [`MAX_WINDOWS_REPO_PATH`] characters,
    /// which git can't use. Checked before git is spawned.
    RepoPathTooLong { path: PathBuf, chars: usize },
    /// The operating system couldn't start git or collect its output.
    Spawn {
        path: PathBuf,
        source: Arc<std::io::Error>,
    },
    /// The invocation's [`Stop`] was used. `stderr` is what git wrote before it was killed,
    /// empty if it never started.
    Stopped { stderr: Vec<u8> },
    /// A [`Stop`] was attached to an invocation that isn't a Network Job. Only Network Jobs
    /// can be stopped (ADR 0002), so git wasn't started.
    NotStoppable { class: JobClass },
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Failed { stderr, .. } => f.write_str(&String::from_utf8_lossy(stderr)),
            RunError::RepoPathTooLong { path, chars } => write!(
                f,
                "git can't use a repository whose path is over {MAX_WINDOWS_REPO_PATH} \
                 characters on Windows, and {} is {chars}. Move or clone it to a shorter path.",
                path.display()
            ),
            RunError::Spawn { path, source } => {
                write!(f, "couldn't run git at {}: {source}", path.display())
            }
            RunError::Stopped { .. } => f.write_str("stopped"),
            RunError::NotStoppable { class } => {
                write!(f, "only a Network Job can be stopped, not a {class:?} Job")
            }
        }
    }
}

impl std::error::Error for RunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RunError::Spawn { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}

/// A command for `program` with the environment every git spawn gets: C locale, no
/// terminal prompts, and on Windows no console window.
fn git_command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.env("LC_ALL", "C").env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}
