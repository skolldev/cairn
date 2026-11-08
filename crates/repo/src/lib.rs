//! The Repo service: Intents in, Snapshots out.
//!
//! This is the only seam `app` uses. Jobs, the Job scheduler and gix stay private: `app`
//! sends [`Intent`]s, is woken when Snapshots are ready, and drains them with
//! [`RepoService::drain`]. Jobs run on threads this crate owns, with no async runtime and
//! no UI executor, so the whole service runs headless in tests.

mod network;
mod scheduler;
mod status;
mod switch;
mod watch;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub use cairn_core::{ChangeKind, FileChange, Head, Oid, RefEntry, RefName, Refs, RepoPath, Stash};

use cairn_write::Stop;
use scheduler::{JobContext, JobQueue, Lane, Running, Slot};

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
    /// Switch the open repository to this local branch: `git switch`, run as a Local
    /// write Job. It never creates a branch, not even from a remote-tracking one.
    Switch(String),
    /// Fetch every remote, pruning what they deleted: `git fetch --all --prune`, run as a
    /// Network Job beside any Local write Job.
    Fetch,
    /// Push as the user's own `git push` would: run as a Network Job beside any Local
    /// write Job.
    Push,
    /// Stop this Network Job. A queued one ends at once and never starts. A running one's
    /// git is killed, and refs are refreshed before it ends. Anything else, including a Local write Job or a Job that already ended, is
    /// left alone.
    Stop(JobId),
    /// Quit: drop Read Jobs, stop Network Jobs and wait for every Local write Job, queued
    /// ones included (ADR 0002). [`Snapshot::Quit`] follows once they have ended. A push
    /// is stopped like any Network Job, so the caller confirms that first. A repository
    /// can't be opened and no Job can start from here on.
    Quit,
    /// Quit without waiting: the only way out while a Local write Job runs. Whatever a
    /// Quit would do, plus: [`Snapshot::Quit`] follows at once, Local write Jobs still
    /// queued never start, and dropping the service no longer waits. A running Local write
    /// Job isn't killed, but exiting the process closes its pipes and can kill it mid-step,
    /// so the caller confirms this first.
    ForceQuit,
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
    /// Where one Job stands: queued, running with its output so far, or done.
    Job(Arc<JobSnapshot>),
    /// A Quit or a Force quit has finished: exit the process. Published once, and after the
    /// final Snapshot of every Job the Quit waited for.
    Quit,
}

/// Which Job a [`JobSnapshot`] is about. Jobs are numbered in the order they were asked
/// for, so a larger id is a later Job.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobId(u64);

impl JobId {
    /// An id as a [`JobSnapshot`] would carry it, for tests of what `app` does with one.
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

/// What a Job does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobKind {
    /// `git switch` to this local branch.
    Switch { branch: String },
    /// `git fetch --all --prune`.
    Fetch,
    /// `git push`. Stopping one can leave the user's work unpublished, so `app` asks first.
    Push,
}

impl JobKind {
    /// Whether this is a Local write Job: one a Quit waits for, and that nothing stops.
    pub fn is_local_write(&self) -> bool {
        match self {
            JobKind::Switch { .. } => true,
            JobKind::Fetch | JobKind::Push => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobState {
    /// Waiting behind the Jobs queued before it.
    Queued,
    Running,
    /// Finished: success, or git's error exactly as git wrote it.
    Done(Result<(), String>),
    /// Ended by a Stop Intent before it finished. Only a Network Job can be stopped.
    Stopped,
}

/// One Job's state. A Job publishes a new one each time its state or output changes, and
/// each holds everything before it, so only the newest per Job matters. Jobs carry a
/// [`JobId`] rather than a [`Generation`]: nothing Supersedes a Job, and one Job's
/// Snapshots are published in order.
#[derive(Debug)]
pub struct JobSnapshot {
    pub id: JobId,
    pub kind: JobKind,
    /// The repository the Job runs in.
    pub workdir: PathBuf,
    pub state: JobState,
    /// What git and its hooks have written so far, live while the Job runs.
    pub output: String,
    /// The last complete line of [`output`](Self::output), such as git's newest
    /// `--progress` report, which git ends with a carriage return so the next replaces it.
    pub progress: Option<String>,
    /// The span of the Intent this Job answers.
    pub span: tracing::Span,
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
///
/// Dropping it stops every Network Job and waits for every queued Local write Job to
/// finish, because those are never stopped, unless a Force quit has given up on them.
pub struct RepoService {
    /// Watches the open repository. Declared first so it stops before anything it
    /// refreshes is dropped.
    watcher: Mutex<Option<watch::Watcher>>,
    // Declared before the lane so they drop first: a finishing Job still submits its
    // refresh to the lane.
    writes: JobQueue,
    network: JobQueue,
    /// Network Jobs that haven't ended, so a Stop Intent can reach them.
    network_jobs: Arc<Mutex<BTreeMap<JobId, Arc<NetworkJob>>>>,
    reads: Arc<Reads>,
    // Dropping the lane joins its threads. Kept as a field for that, not read.
    _interactive: Lane,
    next_job: AtomicU64,
    /// Whether a Quit or a Force quit has begun. Held for the whole of each Intent, so no
    /// Job can be queued behind the markers a Quit waits for.
    quitting: Mutex<bool>,
}

/// The Read Job Slots and the outbox, shared with Local write Jobs so they can refresh.
struct Reads {
    refs_slot: Slot,
    staged_slot: Slot,
    unstaged_slot: Slot,
    untracked_slot: Slot,
    outbox: Arc<Outbox>,
    /// The repository the last Open named.
    open: Mutex<Option<PathBuf>>,
}

impl RepoService {
    /// Starts the service. `wake` is called from a service thread when Snapshots become
    /// ready to drain, at most once until the next [`drain`](Self::drain).
    pub fn start(wake: impl Fn() + Send + Sync + 'static) -> Self {
        let interactive = Lane::new("interactive", INTERACTIVE_THREADS);
        Self {
            watcher: Mutex::new(None),
            writes: JobQueue::new("local-write"),
            network: JobQueue::new("network"),
            network_jobs: Arc::default(),
            reads: Arc::new(Reads {
                refs_slot: Slot::new("refs", &interactive),
                staged_slot: Slot::new("staged", &interactive),
                unstaged_slot: Slot::new("unstaged", &interactive),
                untracked_slot: Slot::new("untracked", &interactive),
                outbox: Arc::new(Outbox {
                    pending: Mutex::new(Pending::default()),
                    wake: Box::new(wake),
                }),
                open: Mutex::new(None),
            }),
            _interactive: interactive,
            next_job: AtomicU64::new(1),
            quitting: Mutex::new(false),
        }
    }

    pub fn send(&self, intent: Intent) {
        let mut quitting = self.quitting.lock().unwrap();
        match intent {
            Intent::Quit => {
                let _span = tracing::info_span!("intent", kind = "quit").entered();
                if self.wind_down_once(&mut quitting) {
                    self.publish_quit_once_queues_drain();
                }
            }
            Intent::ForceQuit => {
                let _span = tracing::info_span!("intent", kind = "force_quit").entered();
                self.wind_down_once(&mut quitting);
                self.writes.abandon();
                self.network.abandon();
                self.reads.outbox.publish_quit();
            }
            Intent::Open(_) if *quitting => {
                tracing::debug!("quitting; open ignored");
            }
            Intent::Open(path) => {
                let span = tracing::info_span!("intent", kind = "open", path = %path.display());
                let _entered = span.enter();
                *self.reads.open.lock().unwrap() = Some(path.clone());
                // Watching starts before the first read, so no change falls between them.
                // The old watcher stops here; its late events are for a repository that is
                // no longer open, which `refresh_if_open` ignores.
                let watcher = watch::start(&path, Arc::downgrade(&self.reads));
                *self.watcher.lock().unwrap() = watcher;
                self.reads.refresh(&path, Running::Supersede, &span);
            }
            Intent::Switch(branch) => {
                let span = tracing::info_span!("intent", kind = "switch", branch = %branch);
                let kind = JobKind::Switch {
                    branch: branch.clone(),
                };
                self.local_write(kind, span, move |workdir, output| {
                    switch::switch(workdir, &branch, output)
                });
            }
            Intent::Fetch => {
                let span = tracing::info_span!("intent", kind = "fetch");
                self.network_job(JobKind::Fetch, span, network::fetch);
            }
            Intent::Push => {
                let span = tracing::info_span!("intent", kind = "push");
                self.network_job(JobKind::Push, span, network::push);
            }
            Intent::Stop(id) => {
                let _span = tracing::info_span!("intent", kind = "stop", job = id.0).entered();
                let job = self.network_jobs.lock().unwrap().get(&id).cloned();
                match job {
                    Some(job) => job.stop(&self.network_jobs),
                    None => tracing::debug!("not a running or queued Network Job; nothing to stop"),
                }
            }
        }
    }

    /// What the first Quit or Force quit does: nothing new reads, refreshes or starts from
    /// here on, and only Local write Jobs keep running. Returns whether this was the first.
    fn wind_down_once(&self, quitting: &mut bool) -> bool {
        if std::mem::replace(quitting, true) {
            return false;
        }
        *self.watcher.lock().unwrap() = None;
        self.reads.close();
        self.stop_network_jobs();
        true
    }

    /// Queues a marker behind the last Job of each queue. New Jobs can't follow them, so
    /// the second marker to run means both queues have drained. Waiting for the Network
    /// queue too gives a stopped git its time to remove its lock files.
    fn publish_quit_once_queues_drain(&self) {
        let remaining = Arc::new(AtomicUsize::new(2));
        for queue in [&self.writes, &self.network] {
            let remaining = remaining.clone();
            let outbox = self.reads.outbox.clone();
            queue.push(move || {
                if remaining.fetch_sub(1, Ordering::AcqRel) == 1 {
                    outbox.publish_quit();
                }
            });
        }
    }

    fn stop_network_jobs(&self) {
        let jobs: Vec<_> = self
            .network_jobs
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect();
        for job in jobs {
            job.stop(&self.network_jobs);
        }
    }

    /// Takes the Snapshots published since the last drain: the newest one per Slot, then
    /// the newest one per Job, in Job order, then the Quit, which follows every Job.
    pub fn drain(&self) -> Vec<Snapshot> {
        let mut pending = self.reads.outbox.pending.lock().unwrap();
        let reads = [
            pending.refs.take().map(Snapshot::Refs),
            pending.staged.take().map(Snapshot::Staged),
            pending.unstaged.take().map(Snapshot::Unstaged),
            pending.untracked.take().map(Snapshot::Untracked),
        ];
        let jobs = std::mem::take(&mut pending.jobs);
        let quit = std::mem::take(&mut pending.quit).then_some(Snapshot::Quit);
        reads
            .into_iter()
            .flatten()
            .chain(jobs.into_values().map(Snapshot::Job))
            .chain(quit)
            .collect()
    }

    /// A publisher for a new Job in the open repository, or `None` after publishing that
    /// no repository is open.
    fn new_job(&self, kind: JobKind, span: tracing::Span) -> Option<(JobPublisher, PathBuf)> {
        let id = JobId(self.next_job.fetch_add(1, Ordering::Relaxed));
        let workdir = self.reads.open.lock().unwrap().clone();
        let job = JobPublisher {
            id,
            kind,
            workdir: workdir.clone().unwrap_or_default(),
            span,
            outbox: self.reads.outbox.clone(),
        };
        match workdir {
            Some(workdir) => Some((job, workdir)),
            None => {
                job.publish(JobState::Done(Err("no repository is open".to_owned())), "");
                None
            }
        }
    }

    /// Queues a Local write Job that runs `run` in the open repository, publishing its
    /// state and live output, and then refreshes refs and status whatever the outcome.
    fn local_write(
        &self,
        kind: JobKind,
        span: tracing::Span,
        run: impl FnOnce(&Path, &mut dyn FnMut(&[u8])) -> Result<(), String> + Send + 'static,
    ) {
        let Some((job, workdir)) = self.new_job(kind, span) else {
            return;
        };
        job.publish(JobState::Queued, "");
        let reads = self.reads.clone();
        self.writes.push(move || {
            let _entered =
                tracing::info_span!(parent: &job.span, "local_write_job", id = job.id.0).entered();
            job.publish(JobState::Running, "");
            let (state, output) = job.run(|output| JobState::Done(run(&workdir, output)));
            reads.refresh_if_open(&workdir, Refresh::RefsAndStatus, &job.span);
            job.publish(state, &output);
        });
    }

    /// Queues a Network Job that runs `run` in the open repository, publishing its state
    /// and live output, and then refreshes refs whatever the outcome, a stop included.
    /// `run` gets the Stop that a Stop Intent uses. A Job stopped while queued never runs,
    /// so it changes nothing and isn't refreshed.
    fn network_job(
        &self,
        kind: JobKind,
        span: tracing::Span,
        run: impl FnOnce(&Path, &Stop, &mut dyn FnMut(&[u8])) -> JobState + Send + 'static,
    ) {
        let Some((publisher, workdir)) = self.new_job(kind, span) else {
            return;
        };
        // Queued is published before a Stop Intent can find the Job, so that a stop's
        // Snapshot always comes after it.
        publisher.publish(JobState::Queued, "");
        let id = publisher.id;
        let job = Arc::new(NetworkJob {
            publisher,
            stop: Stop::new(),
            phase: Mutex::new(Phase::Queued),
        });
        self.network_jobs.lock().unwrap().insert(id, job.clone());
        let reads = self.reads.clone();
        let network_jobs = self.network_jobs.clone();
        self.network.push(move || {
            let publisher = &job.publisher;
            let _entered =
                tracing::info_span!(parent: &publisher.span, "network_job", id = id.0).entered();
            {
                let mut phase = job.phase.lock().unwrap();
                if *phase == Phase::Ended {
                    tracing::debug!("stopped while queued; never started");
                    return;
                }
                *phase = Phase::Running;
                publisher.publish(JobState::Running, "");
            }
            let (state, output) = publisher.run(|output| run(&workdir, &job.stop, output));
            reads.refresh_if_open(&workdir, Refresh::Refs, &publisher.span);
            *job.phase.lock().unwrap() = Phase::Ended;
            network_jobs.lock().unwrap().remove(&id);
            publisher.publish(state, &output);
        });
    }
}

impl Drop for RepoService {
    /// Stops every Network Job, so that dropping the Network queue doesn't wait on the
    /// network. The fields then drop in order.
    fn drop(&mut self) {
        self.stop_network_jobs();
    }
}

/// A Network Job that hasn't ended, as a Stop Intent finds it.
struct NetworkJob {
    publisher: JobPublisher,
    stop: Stop,
    /// Where the Job stands. Changed and published under this lock, so a stop and the
    /// Job's own start never publish out of order.
    phase: Mutex<Phase>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Queued,
    Running,
    Ended,
}

impl NetworkJob {
    /// A queued Job ends Stopped at once and never starts. A running one's git is killed,
    /// and the Job then ends as it does after any outcome: refresh, then Stopped.
    fn stop(&self, network_jobs: &Mutex<BTreeMap<JobId, Arc<NetworkJob>>>) {
        let mut phase = self.phase.lock().unwrap();
        match *phase {
            Phase::Queued => {
                *phase = Phase::Ended;
                self.publisher.publish(JobState::Stopped, "");
                network_jobs.lock().unwrap().remove(&self.publisher.id);
            }
            Phase::Running => self.stop.stop(),
            Phase::Ended => {}
        }
    }
}

/// What a Job needs to publish its own Snapshots.
struct JobPublisher {
    id: JobId,
    kind: JobKind,
    workdir: PathBuf,
    span: tracing::Span,
    outbox: Arc<Outbox>,
}

impl JobPublisher {
    fn publish(&self, state: JobState, output: &str) {
        self.outbox.publish_job(Arc::new(JobSnapshot {
            id: self.id,
            kind: self.kind.clone(),
            workdir: self.workdir.clone(),
            state,
            output: output.to_owned(),
            progress: last_line(output),
            span: self.span.clone(),
        }));
    }

    /// Runs a started Job's work, publishing its output live, and returns how it ended
    /// with all of its output.
    fn run(&self, work: impl FnOnce(&mut dyn FnMut(&[u8])) -> JobState) -> (JobState, String) {
        let mut output = Vec::new();
        // A panic must still end the Job and refresh, or its Snapshot would stay Running.
        let state = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            work(&mut |chunk| {
                output.extend_from_slice(chunk);
                self.publish(JobState::Running, &String::from_utf8_lossy(&output));
            })
        }))
        .unwrap_or_else(|_| {
            tracing::error!("job panicked");
            JobState::Done(Err(
                "Cairn hit an internal error while running this Job".to_owned()
            ))
        });
        (state, String::from_utf8_lossy(&output).into_owned())
    }
}

/// The last non-blank line of `output` that git has finished, by newline or by the
/// carriage return that ends a `--progress` report.
fn last_line(output: &str) -> Option<String> {
    let finished = &output[..output.rfind(['\r', '\n'])?];
    finished
        .rsplit(['\r', '\n'])
        .find(|line| !line.trim().is_empty())
        .map(str::to_owned)
}

/// Which Slots a finished Job or an External change refreshes: only those it can change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Refresh {
    Refs,
    /// Staged, Unstaged and Untracked.
    Status,
    RefsAndStatus,
}

impl Refresh {
    /// Everything either refreshes.
    fn union(self, other: Self) -> Self {
        if self == other {
            self
        } else {
            Refresh::RefsAndStatus
        }
    }
}

impl Reads {
    /// Drops every Read Job and keeps new ones from starting: with no repository open,
    /// a finishing Job's refresh and a late watcher event are both skipped. New submissions
    /// check `open` under its lock, so none can slip in after the Slots are emptied.
    fn close(&self) {
        *self.open.lock().unwrap() = None;
        for slot in [
            &self.refs_slot,
            &self.staged_slot,
            &self.unstaged_slot,
            &self.untracked_slot,
        ] {
            slot.drop_jobs();
        }
    }

    /// Refreshes what a finished Job can change, unless an Open has named another
    /// repository since: that refresh would replace the newer repo's Snapshots in the
    /// shared Slots. Called before the Job publishes its outcome, so that by the time the
    /// UI sees the outcome, the Read Jobs that show its effect are already queued.
    ///
    /// It asks the same questions again, so a read already running finishes and publishes.
    fn refresh_if_open(&self, path: &Path, what: Refresh, span: &tracing::Span) {
        let open = self.open.lock().unwrap();
        if open.as_deref() == Some(path) {
            let running = Running::Finish;
            match what {
                Refresh::RefsAndStatus => self.refresh(path, running, span),
                Refresh::Refs => self.refresh_refs(path, running, span),
                Refresh::Status => self.refresh_status(path, running, span),
            }
        } else {
            tracing::debug!(path = %path.display(), "repository no longer open; refresh skipped");
        }
    }

    /// Reads refs and status again: everything a switch can change, and everything an
    /// Open shows. Later Intents that change less refresh less.
    fn refresh(&self, path: &Path, running: Running, span: &tracing::Span) {
        self.refresh_refs(path, running, span);
        self.refresh_status(path, running, span);
    }

    fn refresh_refs(&self, path: &Path, running: Running, span: &tracing::Span) {
        self.submit(&self.refs_slot, path, running, span, read_refs, |p| {
            &mut p.refs
        });
    }

    fn refresh_status(&self, path: &Path, running: Running, span: &tracing::Span) {
        self.submit(&self.staged_slot, path, running, span, read_staged, |p| {
            &mut p.staged
        });
        self.submit(
            &self.unstaged_slot,
            path,
            running,
            span,
            read_unstaged,
            |p| &mut p.unstaged,
        );
        self.submit(
            &self.untracked_slot,
            path,
            running,
            span,
            read_untracked,
            |p| &mut p.untracked,
        );
    }

    /// Submits a Read Job to `slot` that runs `read` on the repository at `path` and
    /// publishes its answer into the outbox cell `cell` picks.
    fn submit<T: Send + Sync + 'static>(
        &self,
        slot: &Slot,
        path: &Path,
        running: Running,
        span: &tracing::Span,
        read: fn(&Path, &JobContext) -> Result<T, String>,
        cell: fn(&mut Pending) -> &mut Latest<T>,
    ) {
        let outbox = self.outbox.clone();
        let path = path.to_path_buf();
        let span = span.clone();
        slot.submit(running, move |job| {
            let result = read(&path, job);
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

/// Opens the repository with gix and reads its refs. Too quick to be worth stopping.
fn read_refs(path: &Path, _job: &JobContext) -> Result<Refs, String> {
    cairn_read::Repo::open(path)
        .and_then(|repo| repo.refs())
        .map_err(|e| e.message().to_owned())
}

/// Opens the repository with gix and walks the worktree for untracked files, stopping
/// early once Superseded.
fn read_untracked(path: &Path, job: &JobContext) -> Result<Vec<RepoPath>, String> {
    cairn_read::Repo::open(path)
        .and_then(|repo| repo.untracked(job.gix_interrupt()))
        .map_err(|e| e.message().to_owned())
}

fn read_staged(path: &Path, job: &JobContext) -> Result<Vec<FileChange>, String> {
    status::staged(path, job.cli_stop())
}

fn read_unstaged(path: &Path, job: &JobContext) -> Result<Vec<FileChange>, String> {
    status::unstaged(path, job.cli_stop())
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
    /// The newest Snapshot of each Job, by id.
    jobs: BTreeMap<JobId, Arc<JobSnapshot>>,
    /// A Quit has finished and the UI hasn't drained that yet.
    quit: bool,
    /// A Quit has finished: a Force quit and the Quit it overtook publish it only once.
    quit_published: bool,
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.refs.pending.is_none()
            && self.staged.pending.is_none()
            && self.unstaged.pending.is_none()
            && self.untracked.pending.is_none()
            && self.jobs.is_empty()
            && !self.quit
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
    /// Publishes a Job's Snapshot, replacing any undrained one of the same Job. A Job
    /// publishes from one thread at a time, so its Snapshots arrive here in order.
    fn publish_job(&self, snapshot: Arc<JobSnapshot>) {
        let was_empty = {
            let mut pending = self.pending.lock().unwrap();
            let was_empty = pending.is_empty();
            pending.jobs.insert(snapshot.id, snapshot);
            was_empty
        };
        if was_empty {
            (self.wake)();
        }
    }

    /// Publishes that a Quit has finished, unless that has been published already.
    fn publish_quit(&self) {
        let was_empty = {
            let mut pending = self.pending.lock().unwrap();
            if std::mem::replace(&mut pending.quit_published, true) {
                return;
            }
            let was_empty = pending.is_empty();
            pending.quit = true;
            was_empty
        };
        if was_empty {
            (self.wake)();
        }
    }

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
