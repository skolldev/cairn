//! A repository's status: the Staged and Unstaged lists and the untracked files.

use std::borrow::Cow;
use std::fmt;

/// A path inside the repository, relative to its top level and `/`-separated, exactly as
/// git stores it. Not always UTF-8, so it is kept as bytes.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RepoPath(Vec<u8>);

impl RepoPath {
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self(bytes.into())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// The path as text, for display. Invalid UTF-8 is replaced.
    pub fn to_str_lossy(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.0)
    }
}

impl fmt::Debug for RepoPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.to_str_lossy(), f)
    }
}

impl fmt::Display for RepoPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_str_lossy())
    }
}

/// How one path differs between two sides, in the letters of git's raw diff format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Deleted,
    Modified,
    /// The kind of file changed, such as a file becoming a symlink.
    TypeChanged,
    /// The path has conflicts waiting to be resolved.
    Unmerged,
    /// Moved from `from`, with git's similarity score in percent.
    Renamed {
        from: RepoPath,
        score: u8,
    },
}

impl ChangeKind {
    /// The letter git's raw diff format and `git status` use for this kind.
    pub fn letter(&self) -> char {
        match self {
            ChangeKind::Added => 'A',
            ChangeKind::Deleted => 'D',
            ChangeKind::Modified => 'M',
            ChangeKind::TypeChanged => 'T',
            ChangeKind::Unmerged => 'U',
            ChangeKind::Renamed { .. } => 'R',
        }
    }
}

/// One changed path in the Staged or Unstaged list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    /// The path on the newer side: the destination of a rename.
    pub path: RepoPath,
    pub kind: ChangeKind,
}
