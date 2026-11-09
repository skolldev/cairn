//! Untracked files through gix's directory walk, as `git status` lists them.
//!
//! The walk alone, with no index-vs-worktree pass: Staged and Unstaged come from the CLI
//! (#13), so the only part of full status gix adds is the files git doesn't track.

use std::sync::atomic::AtomicBool;

use cairn_core::RepoPath;
use gix::bstr::ByteSlice as _;
use gix::dir::entry::{Kind, Status};
use gix::dir::walk::{EmissionMode, delegate::Collect};
use gix::status::UntrackedFiles;

use crate::{ReadError, err};

pub(crate) fn read(
    repo: &gix::Repository,
    interrupt: &AtomicBool,
) -> Result<Vec<RepoPath>, ReadError> {
    let Some(mode) = emission_mode(repo)? else {
        return Ok(Vec::new());
    };
    let index = repo.index_or_empty().map_err(err)?;
    walk(repo, &index, mode, None::<&[Vec<u8>]>, interrupt)
}

/// The list `last` was, after the worktree changed at `changed`, given that the index and the
/// excludes haven't changed since `last` was read.
///
/// Only what a change can affect is walked again. Whether a directory collapses into one
/// `dir/` entry depends on whether it holds any tracked file, so a change at `a/b/c` can only
/// change entries under the child of its deepest ancestor that holds one: with `a/` tracked
/// and `a/b/` not, that is `a/b`, which may appear, vanish or collapse. Every directory above
/// keeps its tracked files, so nothing outside the child changes. A tracked file itself is
/// never listed, so a change to one walks nothing.
pub(crate) fn read_within(
    repo: &gix::Repository,
    interrupt: &AtomicBool,
    last: &[RepoPath],
    changed: &[&[u8]],
) -> Result<Vec<RepoPath>, ReadError> {
    let Some(mode) = emission_mode(repo)? else {
        return Ok(Vec::new());
    };
    let index = repo.index_or_empty().map_err(err)?;
    let regions = regions(&index, changed);
    if regions.is_empty() {
        return Ok(last.to_vec());
    }
    let ignore_case = repo.filesystem_options().map_err(err)?.ignore_case;
    let walked = walk(repo, &index, mode, Some(&regions), interrupt)?;
    let mut paths: Vec<RepoPath> = last
        .iter()
        .filter(|p| !regions.iter().any(|r| covers(r, p.as_bytes(), ignore_case)))
        .cloned()
        .chain(walked)
        .collect();
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// The smallest parts of the worktree whose entries a change at each of `changed` can affect,
/// none inside another. See [`read_within`].
fn regions(index: &gix::index::State, changed: &[&[u8]]) -> Vec<Vec<u8>> {
    let mut regions: Vec<Vec<u8>> = Vec::new();
    for &path in changed {
        let path = path.strip_suffix(b"/").unwrap_or(path);
        if path.is_empty() {
            continue;
        }
        // The deepest proper ancestor holding a tracked file; the worktree root always counts.
        let slashes: Vec<usize> = path
            .iter()
            .enumerate()
            .filter(|&(_, &b)| b == b'/')
            .map(|(i, _)| i)
            .collect();
        let end = slashes
            .iter()
            .rev()
            .find(|&&i| index.path_is_directory(path[..i].as_bstr()))
            .map_or(0, |&i| i + 1);
        let child_end = path[end..]
            .iter()
            .position(|&b| b == b'/')
            .map_or(path.len(), |i| end + i);
        let child = &path[..child_end];
        let tracked_file =
            child.len() == path.len() && index.entry_by_path(path.as_bstr()).is_some();
        if !tracked_file {
            regions.push(child.to_vec());
        }
    }
    regions.sort();
    regions.dedup();
    // Drop a region inside another, whose walk already covers it.
    let all = regions.clone();
    regions.retain(|r| {
        !all.iter()
            .any(|outer| outer != r && covers(outer, r, false))
    });
    regions
}

/// Whether `entry`, a listed path that may end in `/`, lies at or under `region`.
fn covers(region: &[u8], entry: &[u8], ignore_case: bool) -> bool {
    let entry = entry.strip_suffix(b"/").unwrap_or(entry);
    let Some(head) = entry.get(..region.len()) else {
        return false;
    };
    let same = if ignore_case {
        head.eq_ignore_ascii_case(region)
    } else {
        head == region
    };
    same && (entry.len() == region.len() || entry[region.len()] == b'/')
}

/// How `status.showUntrackedFiles` says to list untracked files, or `None` for not at all.
fn emission_mode(repo: &gix::Repository) -> Result<Option<EmissionMode>, ReadError> {
    Ok(match show_untracked_files(repo)? {
        UntrackedFiles::None => None,
        UntrackedFiles::Collapsed => Some(EmissionMode::CollapseDirectory),
        UntrackedFiles::Files => Some(EmissionMode::Matching),
    })
}

/// Walks the worktree, or only `within` it: literal paths, each a file or a directory.
fn walk(
    repo: &gix::Repository,
    index: &gix::index::State,
    mode: EmissionMode,
    within: Option<&[impl AsRef<[u8]>]>,
    interrupt: &AtomicBool,
) -> Result<Vec<RepoPath>, ReadError> {
    let patterns: Vec<gix::bstr::BString> = within
        .into_iter()
        .flatten()
        .map(|p| {
            let mut pattern = gix::bstr::BString::from(":(literal)");
            pattern.extend_from_slice(p.as_ref());
            pattern
        })
        .collect();
    let options = repo
        .dirwalk_options()
        .map_err(err)?
        .emit_untracked(mode)
        .emit_ignored(None)
        .emit_tracked(false)
        .emit_pruned(false)
        .emit_empty_directories(false)
        .recurse_repositories(false);
    let mut collect = Collect::default();
    repo.dirwalk(index, &patterns, interrupt, options, &mut collect)
        .map_err(err)?;

    let mut paths: Vec<RepoPath> = collect
        .into_entries_by_path()
        .into_iter()
        .filter(|(entry, _)| entry.status == Status::Untracked)
        .map(|(entry, _)| {
            let mut path = Vec::from(entry.rela_path);
            // git marks a collapsed directory, or a nested repository, with a slash.
            if matches!(entry.disk_kind, Some(Kind::Directory | Kind::Repository)) {
                path.push(b'/');
            }
            RepoPath::new(path)
        })
        .collect();
    // Sorted with the slash, as git sorts them.
    paths.sort();
    Ok(paths)
}

/// `status.showUntrackedFiles`, read as `git status` reads it.
fn show_untracked_files(repo: &gix::Repository) -> Result<UntrackedFiles, ReadError> {
    const KEY: &gix::config::tree::status::ShowUntrackedFiles =
        &gix::config::tree::Status::SHOW_UNTRACKED_FILES;
    let config = repo.config_snapshot();
    match config.string(KEY) {
        Some(value) => KEY
            .try_into_show_untracked_files(value.as_bstr())
            .map_err(err),
        None => Ok(UntrackedFiles::default()),
    }
}
