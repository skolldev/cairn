//! A repository's refs: local and remote branches, tags, stashes and HEAD.

use std::fmt;

/// An object id as lowercase hex, whatever the repository's hash.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Oid(String);

impl Oid {
    pub fn from_hex(hex: impl Into<String>) -> Self {
        Self(hex.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The first `len` hex digits, for display.
    pub fn short(&self, len: usize) -> &str {
        &self.0[..len.min(self.0.len())]
    }
}

impl fmt::Debug for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A full ref name, such as `refs/heads/main`.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RefName(String);

impl RefName {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether it names a local branch, under `refs/heads/`.
    pub fn is_local_branch(&self) -> bool {
        self.0.starts_with("refs/heads/")
    }

    /// The name without its category prefix: `main`, `origin/main`, `v1.0`.
    pub fn short(&self) -> &str {
        ["refs/heads/", "refs/remotes/", "refs/tags/", "refs/"]
            .iter()
            .find_map(|prefix| self.0.strip_prefix(prefix))
            .unwrap_or(&self.0)
    }
}

impl fmt::Debug for RefName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for RefName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One ref, with the same fields `git for-each-ref` reports for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefEntry {
    pub name: RefName,
    /// The object the ref points at, after following symbolic refs (`%(objectname)`).
    pub target: Oid,
    /// The object a tag object points at, one level down (`%(*objectname)`).
    /// `None` unless `target` is a tag object.
    pub peeled: Option<Oid>,
    /// The ref a symbolic ref points at (`%(symref)`), such as `origin/HEAD`'s branch.
    pub symref: Option<RefName>,
    /// A local branch's upstream, as `%(upstream)` names it from `branch.<name>.remote` and
    /// `branch.<name>.merge`. `None` for a branch with none, and for every remote branch
    /// and tag.
    pub upstream: Option<Upstream>,
}

/// The ref a local branch tracks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upstream {
    /// Usually a remote-tracking branch, such as `refs/remotes/origin/main`, or a local
    /// branch for `branch.<name>.remote = .`.
    pub name: RefName,
    /// What it points at. `None` when the ref doesn't exist: the upstream is gone, as after
    /// a fetch pruned the branch the remote deleted.
    pub target: Option<Oid>,
}

/// Where HEAD points.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Head {
    /// On a branch. `target` is `None` while the branch is unborn.
    Branch {
        name: RefName,
        target: Option<Oid>,
    },
    Detached {
        target: Oid,
    },
}

/// One stash entry, newest first in [`Refs::stashes`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stash {
    pub target: Oid,
    /// The reflog subject, as `git stash list` prints it.
    pub message: String,
}

/// Every ref Cairn shows, each list sorted by full ref name as git sorts it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refs {
    pub head: Head,
    pub local: Vec<RefEntry>,
    pub remote: Vec<RefEntry>,
    pub tags: Vec<RefEntry>,
    pub stashes: Vec<Stash>,
}
