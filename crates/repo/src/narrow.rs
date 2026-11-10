//! Reads narrowed to what an External change touched (#40).
//!
//! On the Linux kernel, Unstaged's `git diff-files` takes about 130 ms and Untracked's
//! directory walk about 210 ms, each well over the External change Budget on its own. So a
//! change the watcher saw at known worktree paths reads only those paths again, starting from
//! the list the Slot read last. An index write reads only the entries it changed (#54): each
//! read keeps the index it read against, and the next compares it with the index it loads.
//! Anything that can change an answer for paths neither names reads everything again: an
//! Open, a Local write Job, config, the excludes, or an event the watcher lost.
//!
//! What changed is recorded before the refresh is submitted, and a read takes it as it
//! starts. A change whose event arrives after that is taken by the next read, which the same
//! event queues behind this one (ADR 0004).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use cairn_read::Index;

/// More changed paths than this read everything: past it a narrowed read costs about as much,
/// and git's command line has a length limit.
const MOST_PATHS: usize = 256;

/// One Slot's narrowing: what changed since its last read began, and that read's list and
/// index.
pub(crate) struct Narrowed<T> {
    changes: Mutex<Changes>,
    last: Mutex<Option<Last<T>>>,
}

#[derive(Default)]
struct Changes {
    everything: bool,
    /// The index may have changed, so a read compares it even if no path changed.
    index: bool,
    /// Relative to the worktree root, with `/` separators.
    paths: BTreeSet<Vec<u8>>,
}

struct Last<T> {
    workdir: PathBuf,
    list: Vec<T>,
    /// The index loaded after that read took its changes and before it read.
    index: Index,
}

/// What a read starting now has to read.
pub(crate) enum Scope<T> {
    Everything,
    /// Nothing changed since `last` was read against `index`: the read that began last took
    /// every change, such as both events of one save, and the one queued behind it has none
    /// left.
    Unchanged {
        last: Vec<T>,
        index: Index,
    },
    /// Only these paths, and whatever changed in the index since `before`, changed since
    /// `last` was read against `before`.
    Paths {
        last: Vec<T>,
        before: Index,
        paths: Vec<Vec<u8>>,
    },
}

impl<T: Clone> Narrowed<T> {
    pub(crate) fn new() -> Self {
        Self {
            changes: Mutex::new(Changes {
                everything: true,
                ..Changes::default()
            }),
            last: Mutex::new(None),
        }
    }

    /// The worktree changed at these paths.
    pub(crate) fn touch(&self, paths: impl IntoIterator<Item = Vec<u8>>) {
        self.changes.lock().unwrap().paths.extend(paths);
    }

    /// The index may have changed. HEAD alone can't change these Slots' answers.
    pub(crate) fn index(&self) {
        self.changes.lock().unwrap().index = true;
    }

    /// Something changed that a narrowed read can't account for.
    pub(crate) fn everything(&self) {
        self.changes.lock().unwrap().everything = true;
    }

    /// Takes what changed since the last read began, as what the read starting now reads.
    /// Until [`finish`](Reading::finish), the changes it took are held by the returned
    /// [`Reading`], which hands them back as "read everything" if the read never finishes,
    /// such as when it panics.
    pub(crate) fn begin<'a>(&'a self, workdir: &'a Path) -> (Scope<T>, Reading<'a, T>) {
        let reading = Reading {
            narrowed: self,
            workdir,
            finished: false,
        };
        (self.take(workdir), reading)
    }

    fn take(&self, workdir: &Path) -> Scope<T> {
        let changes = std::mem::take(&mut *self.changes.lock().unwrap());
        let last = self.last.lock().unwrap();
        match &*last {
            Some(last)
                if !changes.everything
                    && changes.paths.len() <= MOST_PATHS
                    && last.workdir == workdir =>
            {
                if changes.paths.is_empty() && !changes.index {
                    Scope::Unchanged {
                        last: last.list.clone(),
                        index: last.index.clone(),
                    }
                } else {
                    Scope::Paths {
                        last: last.list.clone(),
                        before: last.index.clone(),
                        paths: changes.paths.into_iter().collect(),
                    }
                }
            }
            _ => Scope::Everything,
        }
    }

    fn keep(&self, workdir: &Path, result: &Result<Vec<T>, String>, index: Option<Index>) {
        let mut last = self.last.lock().unwrap();
        match (result, index) {
            (Ok(list), Some(index)) => {
                *last = Some(Last {
                    workdir: workdir.to_path_buf(),
                    list: list.clone(),
                    index,
                })
            }
            // Without the index it was read against, the next read can't tell what an index
            // write changed.
            _ => {
                *last = None;
                self.everything();
            }
        }
    }
}

/// A read that has taken its changes from a [`Narrowed`].
pub(crate) struct Reading<'a, T: Clone> {
    narrowed: &'a Narrowed<T>,
    workdir: &'a Path,
    finished: bool,
}

impl<T: Clone> Reading<'_, T> {
    /// Keeps what the read answered, and the index it read against, for the next one to
    /// start from. A read that failed, or was stopped, may have taken changes it never read:
    /// the next reads all. So does the next after a read whose index couldn't be loaded.
    pub(crate) fn finish(mut self, result: &Result<Vec<T>, String>, index: Option<Index>) {
        self.finished = true;
        self.narrowed.keep(self.workdir, result, index);
    }
}

impl<T: Clone> Drop for Reading<'_, T> {
    fn drop(&mut self) {
        if !self.finished {
            self.narrowed.keep(
                self.workdir,
                &Err("the read never finished".to_owned()),
                None,
            );
        }
    }
}

/// The paths a narrowed read reads: those the watcher saw, and `more` that an index write
/// changed. `None` if there are too many, so that the read reads everything.
pub(crate) fn paths_with(paths: Vec<Vec<u8>>, more: BTreeSet<Vec<u8>>) -> Option<Vec<Vec<u8>>> {
    let mut all = more;
    all.extend(paths);
    (all.len() <= MOST_PATHS).then(|| all.into_iter().collect())
}

/// `relative`, a path under the worktree root, as git names it: `/` between components.
/// `None` if it can't be named, so that its change reads everything.
pub(crate) fn git_path(relative: &Path) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    for (i, part) in relative.components().enumerate() {
        let std::path::Component::Normal(part) = part else {
            return None;
        };
        if i > 0 {
            bytes.push(b'/');
        }
        #[cfg(unix)]
        bytes.extend_from_slice(std::os::unix::ffi::OsStrExt::as_bytes(part));
        #[cfg(not(unix))]
        bytes.extend_from_slice(part.to_str()?.as_bytes());
    }
    (!bytes.is_empty()).then_some(bytes)
}
