//! gix queries: refs, revwalk, status, tree and blob diff, and what the watcher needs: where
//! git keeps its files, and the excludes it skips ignored files by.
//!
//! gix rules, from the plan. Keep them where the read code lives:
//! - Open with `permissions.config.git_binary = true`, so gix sees the same system config
//!   as the `git` binary.
//! - Never use `repo.status()` on the fast path: it rebuilds an index from `HEAD^{tree}`.
//! - Never call `Outcome::write_changes()`: it drops index extensions.
//! - Keep the default `FastEq` comparison. `HashEq` disagrees with git.
//!
//! What the pinned gix does that its signatures don't show: `docs/gix-notes.md`.
//!
//! Untracked files come from the directory walk alone. Staged and Unstaged are read by the
//! CLI (#13), so an index-vs-worktree pass here would only repeat their work.

mod excludes;
mod index;
mod refs;
mod untracked;
mod walk;

pub use excludes::Excludes;
pub use index::{Index, IndexChanges};

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

pub use cairn_core::{Oid, Refs, RepoPath};

/// A gix error, as text. The Repo service can retry through git.
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

    /// Whether gix opened this repository with restricted config permissions.
    pub fn reduced_trust(&self) -> bool {
        self.0.to_thread_local().git_dir_trust() == gix::sec::Trust::Reduced
    }

    /// Where git keeps this worktree's own files, such as HEAD and the index. Wherever a
    /// `.git` file points, in a linked worktree or a submodule.
    pub fn git_dir(&self) -> PathBuf {
        self.0.git_dir().to_path_buf()
    }

    /// Where git keeps what every worktree of the repository shares, such as refs and
    /// config. The same as [`git_dir`](Self::git_dir) except in a linked worktree.
    pub fn common_dir(&self) -> PathBuf {
        self.0.to_thread_local().common_dir().to_path_buf()
    }

    /// Every ref Cairn shows: local and remote branches, tags, stashes and HEAD.
    pub fn refs(&self) -> Result<Refs, ReadError> {
        refs::read(&self.0.to_thread_local())
    }

    /// Untracked, unignored paths, sorted, as `git status` lists them. An all-untracked
    /// directory is one entry ending in `/` unless `status.showUntrackedFiles=all`.
    /// Setting `interrupt` ends the walk early with an error.
    pub fn untracked(&self, interrupt: &AtomicBool) -> Result<Vec<RepoPath>, ReadError> {
        self.untracked_in(&self.index()?, interrupt)
    }

    /// The index as it is now, or an empty one if there is no index file yet.
    pub fn index(&self) -> Result<Index, ReadError> {
        Index::load(&self.0.to_thread_local())
    }

    /// [`untracked`](Self::untracked), walked against `index`, a load of this repository's.
    pub fn untracked_in(
        &self,
        index: &Index,
        interrupt: &AtomicBool,
    ) -> Result<Vec<RepoPath>, ReadError> {
        untracked::read(&self.0.to_thread_local(), index, interrupt)
    }

    /// [`untracked_in`](Self::untracked_in) again after the worktree changed only at
    /// `changed`, relative paths with `/` separators, starting from `last`, the list it gave
    /// against the index `before`. `changed` must include every path tracked in only one of
    /// `before` and `index` ([`IndexChanges::tracked`]). Only right if the excludes are as
    /// they were when `last` was read.
    pub fn untracked_within(
        &self,
        interrupt: &AtomicBool,
        index: &Index,
        before: &Index,
        last: &[RepoPath],
        changed: &[&[u8]],
    ) -> Result<Vec<RepoPath>, ReadError> {
        untracked::read_within(
            &self.0.to_thread_local(),
            interrupt,
            index,
            before,
            last,
            changed,
        )
    }

    /// Whether [`walk`](Self::walk) can use a commit-graph, as git would. Only then does gix
    /// walk history: without one, `git rev-list` is the engine that passed (#13).
    pub fn has_commit_graph(&self) -> Result<bool, ReadError> {
        walk::has_commit_graph(&self.0.to_thread_local())
    }

    /// History from all refs and HEAD, in the order of `git rev-list --topo-order
    /// --date-order --all`, except that commits of equal date may swap places. Setting
    /// `interrupt` ends the walk early with an error.
    pub fn walk(&self, interrupt: &AtomicBool) -> Result<Vec<Oid>, ReadError> {
        walk::read(&self.0.to_thread_local(), interrupt)
    }
}
