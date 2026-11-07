//! The Repo service: Intents in, Snapshots out.
//!
//! This is the only seam `app` uses. Jobs, the Job scheduler and gix stay private: `app`
//! sends [`Intent`]s, is woken when Snapshots are ready, and drains them with
//! [`RepoService::drain`]. Read Jobs run on threads this crate owns, with no async runtime
//! and no UI executor, so the whole service runs headless in tests.

mod scheduler;
mod status;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub use cairn_core::{ChangeKind, FileChange, Head, Oid, RefEntry, RefName, Refs, RepoPath, Stash};

use scheduler::{Lane, Slot};

/// Interactive lane workers. Enough for the Read Jobs one click starts to run side by side.
const INTERACTIVE_THREADS: usize = 2;

/// Which submission of a Slot a Snapshot answers. Within a Slot, a larger generation is
/// always the newer answer, so a smaller one is stale. Generations of different Slots are
/// unrelated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Generation(u64);

impl Generation {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Something the user wants from the repository.
#[derive(Debug, Clone)]
pub enum Intent {
    /// Open the repository at this path, replacing any open one, and read its refs and status.
    Open(PathBuf),
}

/// An immutable view of repository state, shared by `Arc`. Each variant comes from its
/// own Slot, so each is published, superseded and generation-numbered on its own.
#[derive(Debug, Clone)]
pub enum Snapshot {
    Refs(Arc<RefsSnapshot>),
    /// HEAD vs index.
    Staged(Arc<ChangesSnapshot>),
    /// Index vs worktree.
    Unstaged(Arc<ChangesSnapshot>),
    /// Files in the worktree that git doesn't track and doesn't ignore.
    Untracked(Arc<UntrackedSnapshot>),
}

/// One Slot's answer: what it read, or why it couldn't.
#[derive(Debug)]
pub struct SlotSnapshot<T> {
    pub generation: Generation,
    /// The path the Open Intent named.
    pub workdir: PathBuf,
    /// What was read, or the error text from git or gix.
    pub result: Result<T, String>,
    /// The span of the Intent this answers, so the UI can record when it was shown.
    pub span: tracing::Span,
}

/// Every ref Cairn shows.
pub type RefsSnapshot = SlotSnapshot<Refs>;
/// The Staged or Unstaged list, sorted by path.
pub type ChangesSnapshot = SlotSnapshot<Vec<FileChange>>;
/// Untracked paths, sorted. A directory whose contents are all untracked is one entry
/// ending in `/`, as `git status` shows it, unless `status.showUntrackedFiles=all`.
pub type UntrackedSnapshot = SlotSnapshot<Vec<RepoPath>>;

/// The Repo service for one window. Dropping it stops its threads and drops queued Read Jobs.
pub struct RepoService {
    // Dropping the lane joins its threads. Kept as a field for that, not read.
    _interactive: Lane,
    refs_slot: Slot,
    staged_slot: Slot,
    unstaged_slot: Slot,
    untracked_slot: Slot,
    outbox: Arc<Outbox>,
}

impl RepoService {
    /// Starts the service. `wake` is called from a service thread when Snapshots become
    /// ready to drain, at most once until the next [`drain`](Self::drain).
    pub fn start(wake: impl Fn() + Send + Sync + 'static) -> Self {
        let interactive = Lane::new("interactive", INTERACTIVE_THREADS);
        Self {
            refs_slot: Slot::new("refs", &interactive),
            staged_slot: Slot::new("staged", &interactive),
            unstaged_slot: Slot::new("unstaged", &interactive),
            untracked_slot: Slot::new("untracked", &interactive),
            _interactive: interactive,
            outbox: Arc::new(Outbox {
                pending: Mutex::new(Pending::default()),
                wake: Box::new(wake),
            }),
        }
    }

    pub fn send(&self, intent: Intent) {
        match intent {
            Intent::Open(path) => {
                let span = tracing::info_span!("intent", kind = "open", path = %path.display());
                let _entered = span.enter();
                self.submit(&self.refs_slot, &path, &span, read_refs, |p| &mut p.refs);
                self.submit(&self.staged_slot, &path, &span, status::staged, |p| {
                    &mut p.staged
                });
                self.submit(&self.unstaged_slot, &path, &span, status::unstaged, |p| {
                    &mut p.unstaged
                });
                self.submit(&self.untracked_slot, &path, &span, read_untracked, |p| {
                    &mut p.untracked
                });
            }
        }
    }

    /// Takes the Snapshots published since the last drain: the newest one per Slot.
    pub fn drain(&self) -> Vec<Snapshot> {
        let mut pending = self.outbox.pending.lock().unwrap();
        [
            pending.refs.take().map(Snapshot::Refs),
            pending.staged.take().map(Snapshot::Staged),
            pending.unstaged.take().map(Snapshot::Unstaged),
            pending.untracked.take().map(Snapshot::Untracked),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    /// Submits a Read Job to `slot` that runs `read` on the repository at `path` and
    /// publishes its answer into the outbox cell `cell` picks.
    fn submit<T: Send + Sync + 'static>(
        &self,
        slot: &Slot,
        path: &Path,
        span: &tracing::Span,
        read: fn(&Path) -> Result<T, String>,
        cell: fn(&mut Pending) -> &mut Latest<T>,
    ) {
        let outbox = self.outbox.clone();
        let path = path.to_path_buf();
        let span = span.clone();
        slot.submit(move |job| {
            let result = read(&path);
            if !job.is_current() {
                tracing::debug!("superseded while running; result dropped");
                return;
            }
            outbox.publish(
                cell,
                Arc::new(SlotSnapshot {
                    generation: job.generation(),
                    workdir: path,
                    result,
                    span,
                }),
            );
        });
    }
}

/// Opens the repository with gix and reads its refs.
fn read_refs(path: &Path) -> Result<Refs, String> {
    cairn_read::Repo::open(path)
        .and_then(|repo| repo.refs())
        .map_err(|e| e.message().to_owned())
}

/// Opens the repository with gix and walks the worktree for untracked files.
fn read_untracked(path: &Path) -> Result<Vec<RepoPath>, String> {
    cairn_read::Repo::open(path)
        .and_then(|repo| repo.untracked())
        .map_err(|e| e.message().to_owned())
}

/// Snapshots waiting for the UI, coalesced to the newest per Slot.
struct Outbox {
    pending: Mutex<Pending>,
    wake: Box<dyn Fn() + Send + Sync>,
}

#[derive(Default)]
struct Pending {
    refs: Latest<Refs>,
    staged: Latest<Vec<FileChange>>,
    unstaged: Latest<Vec<FileChange>>,
    untracked: Latest<Vec<RepoPath>>,
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.refs.pending.is_none()
            && self.staged.pending.is_none()
            && self.unstaged.pending.is_none()
            && self.untracked.pending.is_none()
    }
}

/// One Slot's undrained Snapshot, and the newest generation it ever accepted.
struct Latest<T> {
    pending: Option<Arc<SlotSnapshot<T>>>,
    newest: Option<Generation>,
}

impl<T> Default for Latest<T> {
    fn default() -> Self {
        Self {
            pending: None,
            newest: None,
        }
    }
}

impl<T> Latest<T> {
    fn take(&mut self) -> Option<Arc<SlotSnapshot<T>>> {
        self.pending.take()
    }
}

impl Outbox {
    fn publish<T>(&self, cell: fn(&mut Pending) -> &mut Latest<T>, snapshot: Arc<SlotSnapshot<T>>) {
        let _span = tracing::debug_span!("publish", generation = snapshot.generation.0).entered();
        let was_empty = {
            let mut pending = self.pending.lock().unwrap();
            let was_empty = pending.is_empty();
            let latest = cell(&mut pending);
            // A newer generation may have been published, even drained, while this one
            // was checking whether it was still current. Never let it follow that one.
            if latest
                .newest
                .is_some_and(|newest| newest >= snapshot.generation)
            {
                tracing::debug!("stale; dropped");
                return;
            }
            latest.newest = Some(snapshot.generation);
            latest.pending = Some(snapshot);
            was_empty
        };
        if was_empty {
            (self.wake)();
        }
    }
}
