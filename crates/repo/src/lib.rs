//! The Repo service: Intents in, Snapshots out.
//!
//! This is the only seam `app` uses. Jobs, the Job scheduler and gix stay private: `app`
//! sends [`Intent`]s, is woken when Snapshots are ready, and drains them with
//! [`RepoService::drain`]. Read Jobs run on threads this crate owns, with no async runtime
//! and no UI executor, so the whole service runs headless in tests.

mod scheduler;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub use cairn_core::{Head, Oid, RefEntry, RefName, Refs, Stash};

use scheduler::{JobContext, Lane, Slot};

/// Interactive lane workers. Enough for the Read Jobs one click starts to run side by side.
const INTERACTIVE_THREADS: usize = 2;

/// Which submission of a Slot a Snapshot answers. Within a Slot, a larger generation is
/// always the newer answer, so a smaller one is stale.
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
    /// Open the repository at this path, replacing any open one, and read its refs.
    Open(PathBuf),
}

/// An immutable view of repository state, shared by `Arc`.
#[derive(Debug, Clone)]
pub enum Snapshot {
    Refs(Arc<RefsSnapshot>),
}

impl Snapshot {
    pub fn generation(&self) -> Generation {
        match self {
            Snapshot::Refs(refs) => refs.generation,
        }
    }
}

/// The refs of the open repository, or why they couldn't be read.
#[derive(Debug)]
pub struct RefsSnapshot {
    pub generation: Generation,
    /// The path the Open Intent named.
    pub workdir: PathBuf,
    /// The refs, or the error text from opening or reading the repository.
    pub refs: Result<Refs, String>,
    /// The span of the Intent this answers, so the UI can record when it was shown.
    pub span: tracing::Span,
}

/// The Repo service for one window. Dropping it stops its threads and drops queued Read Jobs.
pub struct RepoService {
    // Dropping the lane joins its threads. Kept as a field for that, not read.
    _interactive: Lane,
    refs_slot: Slot,
    outbox: Arc<Outbox>,
}

impl RepoService {
    /// Starts the service. `wake` is called from a service thread when Snapshots become
    /// ready to drain, at most once until the next [`drain`](Self::drain).
    pub fn start(wake: impl Fn() + Send + Sync + 'static) -> Self {
        let interactive = Lane::new("interactive", INTERACTIVE_THREADS);
        let refs_slot = Slot::new("refs", &interactive);
        Self {
            _interactive: interactive,
            refs_slot,
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
                let outbox = self.outbox.clone();
                let span = span.clone();
                self.refs_slot
                    .submit(move |job| read_refs(job, path, span, &outbox));
            }
        }
    }

    /// Takes the Snapshots published since the last drain: the newest one per Slot.
    pub fn drain(&self) -> Vec<Snapshot> {
        let mut pending = self.outbox.pending.lock().unwrap();
        pending
            .refs
            .take()
            .map(Snapshot::Refs)
            .into_iter()
            .collect()
    }
}

/// The Open Read Job: opens the repository with gix and reads its refs.
fn read_refs(job: &JobContext, path: PathBuf, span: tracing::Span, outbox: &Outbox) {
    let refs = cairn_read::Repo::open(&path)
        .and_then(|repo| repo.refs())
        .map_err(|e| e.message().to_owned());
    if !job.is_current() {
        tracing::debug!("superseded while running; result dropped");
        return;
    }
    outbox.publish_refs(Arc::new(RefsSnapshot {
        generation: job.generation(),
        workdir: path,
        refs,
        span,
    }));
}

/// Snapshots waiting for the UI, coalesced to the newest per Slot.
struct Outbox {
    pending: Mutex<Pending>,
    wake: Box<dyn Fn() + Send + Sync>,
}

#[derive(Default)]
struct Pending {
    refs: Option<Arc<RefsSnapshot>>,
    /// The newest refs generation ever accepted, drained or not.
    refs_newest: Option<Generation>,
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.refs.is_none()
    }
}

impl Outbox {
    fn publish_refs(&self, snapshot: Arc<RefsSnapshot>) {
        let _span = tracing::debug_span!("publish", generation = snapshot.generation.0).entered();
        let was_empty = {
            let mut pending = self.pending.lock().unwrap();
            let was_empty = pending.is_empty();
            // A newer generation may have been published, even drained, while this one
            // was checking whether it was still current. Never let it follow that one.
            if pending
                .refs_newest
                .is_some_and(|newest| newest >= snapshot.generation)
            {
                tracing::debug!("stale; dropped");
                return;
            }
            pending.refs_newest = Some(snapshot.generation);
            pending.refs = Some(snapshot);
            was_empty
        };
        if was_empty {
            (self.wake)();
        }
    }
}
