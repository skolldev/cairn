//! History walks through gix, in the order `git rev-list --topo-order --date-order --all`
//! prints. (Of those two options git keeps the later, which still implies `--topo-order`:
//! no parent before its children, and commit date otherwise.)
//!
//! Commits with the same commit date can come out in another order than git's, because gix's
//! queue doesn't break those ties as git's does. Measured on 2026-11-08: none on the small
//! fixture, where 615 of 2343 commits share a date, and 134 positions of the kernel's
//! 1,505,332, all between commits of equal date. The set of commits is the same.

use std::sync::atomic::{AtomicBool, Ordering};

use cairn_core::Oid;
use gix::revision::walk::Sorting;

use crate::{ReadError, err};

/// Whether gix's walk would really use a commit-graph: one exists, and neither
/// `core.commitGraph=false`, replace refs, grafts nor a shallow boundary rules it out. git
/// won't use a graph in those repositories either, so its history is what they show. gix
/// itself drops the graph at a shallow boundary, which would leave it walking with none.
pub(crate) fn has_commit_graph(repo: &gix::Repository) -> Result<bool, ReadError> {
    if repo.common_dir().join("info/grafts").exists() || repo.is_shallow().map_err(err)? {
        return Ok(false);
    }
    Ok(repo.commit_graph_if_enabled().map_err(err)?.is_some())
}

pub(crate) fn read(repo: &gix::Repository, interrupt: &AtomicBool) -> Result<Vec<Oid>, ReadError> {
    let tips = tips(repo)?;
    // The predicate is what reaches the walk's first stretch, which goes down to the lowest
    // tip's generation before it yields anything. Rejecting a commit cuts the walk short, so
    // a walk that was interrupted never reports its result.
    let walk = repo
        .rev_walk(tips)
        .sorting(Sorting::DateOrder)
        .use_commit_graph(true)
        .selected(|_| !interrupt.load(Ordering::Relaxed))
        .map_err(err)?;
    let mut commits = Vec::new();
    for info in walk {
        commits.push(Oid::from_hex(info.map_err(err)?.id.to_string()));
    }
    if interrupt.load(Ordering::Relaxed) {
        return Err(ReadError("interrupted".to_owned()));
    }
    Ok(commits)
}

/// What `--all` starts from, in its order, which decides between commits of equal date:
/// every ref by name, this worktree's HEAD, then every other worktree's. A tag counts as what
/// it peels to, and a ref to anything but a commit doesn't count.
fn tips(repo: &gix::Repository) -> Result<Vec<gix::ObjectId>, ReadError> {
    let mut tips = Vec::new();
    for reference in repo.references().map_err(err)?.all().map_err(err)? {
        let mut reference = reference.map_err(err)?;
        tips.push(reference.peel_to_id().map_err(err)?.detach());
    }
    tips.extend(head(repo)?);
    let main = repo.main_repo().map_err(err)?;
    if main.git_dir() != repo.git_dir() {
        tips.extend(head(&main)?);
    }
    for worktree in main.worktrees().map_err(err)? {
        let other = worktree
            .into_repo_with_possibly_inaccessible_worktree()
            .map_err(err)?;
        if other.git_dir() != repo.git_dir() {
            tips.extend(head(&other)?);
        }
    }
    let mut commits = Vec::with_capacity(tips.len());
    for tip in tips {
        if repo.find_header(tip).map_err(err)?.kind() == gix::object::Kind::Commit {
            commits.push(tip);
        }
    }
    Ok(commits)
}

/// HEAD's commit, unless its branch is unborn. Any other failure is an error, for the Repo
/// service to retry through git.
fn head(repo: &gix::Repository) -> Result<Option<gix::ObjectId>, ReadError> {
    let mut head = repo.head().map_err(err)?;
    let id = head.try_peel_to_id().map_err(err)?;
    Ok(id.map(|id| id.detach()))
}
