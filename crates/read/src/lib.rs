//! gix queries: refs, revwalk, status, tree and blob diff.
//!
//! gix rules, from the plan. Keep them where the read code lives:
//! - Open with `permissions.config.git_binary = true`, so gix sees the same system config
//!   as the `git` binary.
//! - Never use `repo.status()` on the fast path: it rebuilds an index from `HEAD^{tree}`.
//! - Never call `Outcome::write_changes()`: it drops index extensions.
//! - Keep the default `FastEq` comparison. `HashEq` disagrees with git.
//!
//! Untracked files come from the directory walk alone. Staged and Unstaged are read by the
//! CLI (#13), so an index-vs-worktree pass here would only repeat their work.

mod refs;
mod untracked;

use std::fmt;
use std::path::Path;
use std::sync::atomic::AtomicBool;

pub use cairn_core::{Refs, RepoPath};

/// A gix error, as text. Reads never retry, so the caller only shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadError(String);

impl ReadError {
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ReadError {}

pub(crate) fn err(e: impl std::error::Error) -> ReadError {
    ReadError(e.to_string())
}

/// An open repository that can be shared across threads. Each read takes a thread-local
/// handle, so concurrent Read Jobs never contend on one.
#[derive(Clone)]
pub struct Repo(gix::ThreadSafeRepository);

impl Repo {
    /// Opens the repository at `path`, seeing the same system config as the `git` binary.
    pub fn open(path: &Path) -> Result<Self, ReadError> {
        let mut options = gix::open::Options::default();
        options.permissions.config.git_binary = true;
        gix::ThreadSafeRepository::open_opts(path, options)
            .map(Self)
            .map_err(err)
    }

    /// Every ref Cairn shows: local and remote branches, tags, stashes and HEAD.
    pub fn refs(&self) -> Result<Refs, ReadError> {
        refs::read(&self.0.to_thread_local())
    }

    /// Untracked, unignored paths, sorted, as `git status` lists them. An all-untracked
    /// directory is one entry ending in `/` unless `status.showUntrackedFiles=all`.
    /// Setting `interrupt` ends the walk early with an error.
    pub fn untracked(&self, interrupt: &AtomicBool) -> Result<Vec<RepoPath>, ReadError> {
        untracked::read(&self.0.to_thread_local(), interrupt)
    }
}
