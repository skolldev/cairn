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
    let mode = match show_untracked_files(repo)? {
        UntrackedFiles::None => return Ok(Vec::new()),
        UntrackedFiles::Collapsed => EmissionMode::CollapseDirectory,
        UntrackedFiles::Files => EmissionMode::Matching,
    };
    let index = repo.index_or_empty().map_err(err)?;
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
    repo.dirwalk(&index, None::<&str>, interrupt, options, &mut collect)
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
