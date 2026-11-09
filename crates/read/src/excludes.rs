//! Which worktree paths the watcher may skip: excluded by git's ignore rules, and untracked.
//!
//! Ignore rules apply only to untracked files, so a path is skipped only when nothing tracked
//! could be affected by a change there. When in doubt it isn't skipped: a needless refresh
//! costs one read, while a wrongly skipped change leaves a stale state on screen.

use std::collections::HashSet;
use std::path::Path;

use gix::bstr::{BStr, BString, ByteSlice as _};
use gix::index::entry::Mode;
use gix::worktree::stack::state::ignore::Source;

use crate::{ReadError, err};

/// The exclude stack and the index of one repository, read once when built.
pub struct Excludes {
    /// Reads each directory's `.gitignore` as a lookup first reaches it, and keeps those of
    /// the directories the last lookup passed through. Consecutive events in one directory,
    /// as a build writes them, read nothing more. A lookup of a directory itself drops what
    /// was kept for it, so the event for a directory moved or replaced, which may arrive
    /// without one for its `.gitignore`, has the next lookup below it read the new one.
    stack: gix::worktree::Stack,
    workdir: std::path::PathBuf,
    tracked: Tracked,
}

/// The index's paths, looked up as `core.ignorecase` says.
enum Tracked {
    Sensitive(Box<gix::index::State>),
    /// ASCII-folded entries, and every directory with an entry below it.
    Folded {
        entries: HashSet<BString>,
        directories: HashSet<BString>,
    },
}

impl Excludes {
    /// Reads every exclude source git reads (each `.gitignore`, `info/exclude` and
    /// `core.excludesFile`) and the index of the repository at `workdir`.
    pub fn build(workdir: &Path) -> Result<Self, ReadError> {
        let repo = crate::Repo::open(workdir)?.0.to_thread_local();
        let index = repo.index_or_empty().map_err(err)?;
        let stack = repo
            .excludes(&index, None, Source::WorktreeThenIdMappingIfNotSkipped)
            .map_err(err)?
            .detach();
        let ignore_case = repo.config_snapshot().boolean("core.ignoreCase") == Some(true);
        let index = gix::index::State::clone(&index);
        let tracked = if ignore_case {
            let mut entries = HashSet::new();
            let mut directories = HashSet::new();
            for entry in index.entries() {
                let path = entry.path(&index).to_ascii_lowercase();
                for directory in ancestors(path.as_bstr()) {
                    // A directory seen before had its own ancestors added with it.
                    if !directories.insert(directory.to_owned()) {
                        break;
                    }
                }
                entries.insert(BString::from(path));
            }
            Tracked::Folded {
                entries,
                directories,
            }
        } else {
            Tracked::Sensitive(Box::new(index))
        };
        Ok(Self {
            stack,
            workdir: workdir.to_path_buf(),
            tracked,
        })
    }

    /// Whether a change at `relative`, a path inside the worktree, can't change status:
    /// it is excluded and nothing tracked lies at, below or above it.
    pub fn skips(&mut self, relative: &Path) -> bool {
        let Ok(path) = gix::path::into_bstr(relative) else {
            return false;
        };
        let path = gix::path::to_unix_separators_on_windows(path);
        if path.is_empty() {
            return false;
        }
        // A directory that still exists matches directory-only patterns such as `target/`.
        let mode = self.workdir.join(relative).is_dir().then_some(Mode::DIR);
        // Looked up even for a tracked path, so the stack drops what it kept for a directory
        // (see `stack`). A lookup that would need an object, such as a skip-worktree
        // `.gitignore`, errors and counts as not excluded.
        let excluded = self
            .stack
            .at_entry(path.as_ref(), mode, &gix::objs::find::Never)
            .is_ok_and(|platform| platform.is_excluded());
        excluded && !self.tracked.covers(path.as_ref())
    }
}

impl Tracked {
    /// Whether `path` is an index entry, a directory with entries below it, or lies inside
    /// an entry, as a file in a submodule does.
    fn covers(&self, path: &BStr) -> bool {
        let folded;
        let path = match self {
            Tracked::Sensitive(_) => path,
            Tracked::Folded { .. } => {
                folded = BString::from(path.to_ascii_lowercase());
                folded.as_ref()
            }
        };
        self.is_entry(path) || self.is_directory(path) || ancestors(path).any(|a| self.is_entry(a))
    }

    fn is_entry(&self, path: &BStr) -> bool {
        match self {
            Tracked::Sensitive(index) => index.entry_by_path(path).is_some(),
            Tracked::Folded { entries, .. } => entries.contains(path),
        }
    }

    fn is_directory(&self, path: &BStr) -> bool {
        match self {
            Tracked::Sensitive(index) => index.path_is_directory(path),
            Tracked::Folded { directories, .. } => directories.contains(path),
        }
    }
}

/// `a/b/c`'s ancestors: `a/b`, then `a`.
fn ancestors(path: &BStr) -> impl Iterator<Item = &BStr> {
    let mut end = path.len();
    std::iter::from_fn(move || {
        end = path[..end].rfind_byte(b'/')?;
        Some(path[..end].as_bstr())
    })
}
