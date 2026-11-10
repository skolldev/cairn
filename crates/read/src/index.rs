//! The index as a read saw it, so that the next read can tell what an index write changed
//! (#54).
//!
//! A `git switch` on the Linux kernel rewrites all 97k entries' worth of index, but changes
//! only the entries of the files that differ between the branches. Unstaged's answer for an
//! entry depends only on that entry and its file, and Untracked's only on which paths are
//! tracked, so a read that knows the index the last read saw needs to read only the entries
//! that differ. Loading the kernel's index took about 10 ms on an M3 Max, and comparing two
//! of them entry by entry about 0.6 ms.

use std::collections::BTreeSet;

use gix::bstr::BStr;

use crate::{ReadError, err};

/// One load of the index, or an empty one where there is no index file yet.
#[derive(Clone)]
pub struct Index(pub(crate) gix::worktree::Index);

/// What changed between two loads of the index. Paths are relative to the worktree root,
/// with `/` separators.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct IndexChanges {
    /// Every path with an entry, at any stage, that was added, removed, or differs in mode,
    /// id, stat data or flags.
    pub entries: BTreeSet<Vec<u8>>,
    /// The paths tracked in only one of the two: no entry at any stage in the other.
    pub tracked: BTreeSet<Vec<u8>>,
}

impl Index {
    pub(crate) fn load(repo: &gix::Repository) -> Result<Self, ReadError> {
        repo.index_or_empty().map(Self).map_err(err)
    }

    pub(crate) fn state(&self) -> &gix::index::State {
        &self.0
    }

    /// What changed in this index since `before` was loaded.
    pub fn changes_since(&self, before: &Index) -> IndexChanges {
        let (old, new) = (before.state(), self.state());
        // The checksum covers every entry, so equal checksums mean nothing to compare. With
        // `index.skipHash` there is none.
        if let (Some(a), Some(b)) = (before.0.checksum(), self.0.checksum())
            && a == b
        {
            return IndexChanges::default();
        }
        let mut changes = IndexChanges::default();

        // Entries are sorted by path, then stage.
        let (mut a, mut b) = (
            old.entries().iter().peekable(),
            new.entries().iter().peekable(),
        );
        loop {
            match (a.peek(), b.peek()) {
                (None, None) => break,
                (Some(x), None) => {
                    changes.entries.insert(x.path(old).to_vec());
                    a.next();
                }
                (None, Some(y)) => {
                    changes.entries.insert(y.path(new).to_vec());
                    b.next();
                }
                (Some(x), Some(y)) => match x
                    .path(old)
                    .cmp(y.path(new))
                    .then(x.stage_raw().cmp(&y.stage_raw()))
                {
                    std::cmp::Ordering::Less => {
                        changes.entries.insert(x.path(old).to_vec());
                        a.next();
                    }
                    std::cmp::Ordering::Greater => {
                        changes.entries.insert(y.path(new).to_vec());
                        b.next();
                    }
                    std::cmp::Ordering::Equal => {
                        if x.mode != y.mode
                            || x.id != y.id
                            || x.stat != y.stat
                            || x.flags != y.flags
                        {
                            changes.entries.insert(x.path(old).to_vec());
                        }
                        a.next();
                        b.next();
                    }
                },
            }
        }
        // Only a path with an entry added or removed can be tracked in one and not the other.
        changes.tracked = changes
            .entries
            .iter()
            .filter(|path| {
                let path: &BStr = path.as_slice().into();
                old.entry_by_path(path).is_some() != new.entry_by_path(path).is_some()
            })
            .cloned()
            .collect();
        changes
    }
}
