//! Domain types and errors shared by every Cairn crate.

pub mod refs;
pub mod status;

pub use refs::{Head, Oid, RefEntry, RefName, Refs, Stash};
pub use status::{ChangeKind, FileChange, RepoPath};
