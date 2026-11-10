//! The Repo service: Intents in, Snapshots out.
//!
//! This is the only seam `app` uses. Jobs, the Job scheduler and gix stay private: `app`
//! sends [`Intent`]s, is woken when Snapshots are ready, and drains them with
//! [`RepoService::drain`]. Jobs run on threads this crate owns, with no async runtime and
//! no UI executor, so the whole service runs headless in tests.

mod fallback;
mod narrow;
mod network;
mod scheduler;
mod stash;
mod status;
mod switch;
mod watch;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};

pub use cairn_core::{ChangeKind, FileChange, Head, Oid, RefEntry, RefName, Refs, RepoPath, Stash};

use cairn_write::{StartGate, Stop};
use scheduler::{JobContext, JobQueue, Lane, Qos, QueueHandle, Running, Slot};

/// Interactive lane workers. Enough for the Read Jobs one click or one `git switch` starts to
/// run side by side: Refs, Staged, Unstaged and Untracked (#54).
const INTERACTIVE_THREADS: usize = 4;
/// Background lane workers. One history walk at a time is all it has to run so far.
const BACKGROUND_THREADS: usize = 1;

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
    /// Stash and switch: stash the local changes, then switch to this local branch as
    /// [`Intent::Switch`] does, as two Local write Jobs. The switch starts only if the stash
    /// succeeded, and not once a Quit has begun. `untracked` stashes untracked files too.
    /// [`JobSnapshot::stash_and_switch`] says when a failed switch offers this.
    StashAndSwitch { branch: String, untracked: bool },
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
    /// Quit would do, plus: [`Snapshot::Quit`] follows at once, no Local write Job's git
    /// starts after that, and dropping the service no longer waits. A Local write Job whose
    /// git is running isn't killed, but exiting the process closes its pipes and can kill it
    /// mid-step, so the caller confirms this first.
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
    pub fn get(self) -> u64 {
        self.0
    }
}

/// What a Job does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobKind {
    /// `git switch` to this local branch.
    Switch { branch: String },
    /// `git stash push`, with `-u` when `untracked`: the first Job of Stash and switch.
    Stash { untracked: bool },
    /// `git fetch --all --prune`.
    Fetch,
    /// `git push`. Stopping one can leave the user's work unpublished, so `app` asks first.
    Push,
}

impl JobKind {
    /// Whether this is a Local write Job: one a Quit waits for, and that nothing stops.
    pub fn is_local_write(&self) -> bool {
        match self {
            JobKind::Switch { .. } | JobKind::Stash { .. } => true,
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
    /// The Job whose success started this one, when this is the second Job of an Intent
    /// made of two, such as the switch of a Stash and switch.
    pub follows: Option<JobId>,
}

impl JobSnapshot {
    /// The Stash and switch this Job offers: only a switch git refused because local
    /// changes would be overwritten, and not the switch of a Stash and switch. It stashes
    /// untracked files when git's refusal named any.
    pub fn stash_and_switch(&self) -> Option<Intent> {
        let (JobKind::Switch { branch }, JobState::Done(Err(error)), None) =
            (&self.kind, &self.state, self.follows)
        else {
            return None;
        };
        let refusal = switch::refusal(error)?;
        Some(Intent::StashAndSwitch {
            branch: branch.clone(),
            untracked: refusal.untracked,
        })
    }
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
    /// Queues Local write Jobs, from an Intent or from the end of the Job before.
    write_jobs: Writes,
    network: JobQueue,
    /// Network Jobs that haven't ended, so a Stop Intent can reach them.
    network_jobs: Arc<Mutex<BTreeMap<JobId, Arc<NetworkJob>>>>,
    reads: Arc<Reads>,
    // Dropping a lane joins its threads. Kept as fields for that, not read.
    _interactive: Lane,
    _background: Lane,
    jobs: Jobs,
}

/// What every new Job needs, shared with Jobs that start a Job of their own.
#[derive(Clone)]
struct Jobs {
    next: Arc<AtomicU64>,
    reads: Arc<Reads>,
    /// Whether a Quit or a Force quit has begun. Held for the whole of each Intent, and while
    /// a Job starts the Job that follows it, so no Job can be queued behind the markers a
    /// Quit waits for.
    quitting: Arc<Mutex<bool>>,
}

/// Queues Local write Jobs: from an Intent, or from the end of the Job before.
#[derive(Clone)]
struct Writes {
    queue: QueueHandle,
    /// Every Local write Job's git passes through this. A Force quit closes it.
    gate: StartGate,
    jobs: Jobs,
}

/// The second Job of an Intent made of two: given the Job that succeeded, it queues the next
/// one in the same repository.
type Next = Box<dyn FnOnce(&Writes, JobId) + Send>;

/// The Read Job Slots and the outbox, shared with Local write Jobs so they can refresh.
struct Reads {
    refs_slot: Slot,
    staged_slot: Slot,
    unstaged_slot: Slot,
    untracked_slot: Slot,
    /// In the Background lane, unlike every other Slot.
    history_slot: Slot,
    /// How far an External change narrows the Unstaged and Untracked reads.
    unstaged_narrowed: Arc<narrow::Narrowed<FileChange>>,
    untracked_narrowed: Arc<narrow::Narrowed<RepoPath>>,
    outbox: Arc<Outbox>,
    /// The repository the last Open named.
    open: Mutex<Option<PathBuf>>,
    read_mode: Mutex<Arc<fallback::ReadMode>>,
}

impl RepoService {
    /// Starts the service. `wake` is called from a service thread when Snapshots become
    /// ready to drain, at most once until the next [`drain`](Self::drain).
    pub fn start(wake: impl Fn() + Send + Sync + 'static) -> Self {
        let interactive = Lane::new("interactive", INTERACTIVE_THREADS, Qos::UserInitiated);
        let background = Lane::new("background", BACKGROUND_THREADS, Qos::Utility);
        let reads = Arc::new(Reads {
            refs_slot: Slot::new("refs", &interactive),
            staged_slot: Slot::new("staged", &interactive),
            unstaged_slot: Slot::new("unstaged", &interactive),
            untracked_slot: Slot::new("untracked", &interactive),
            history_slot: Slot::new("history", &background),
            unstaged_narrowed: Arc::new(narrow::Narrowed::new()),
            untracked_narrowed: Arc::new(narrow::Narrowed::new()),
            outbox: Arc::new(Outbox {
                pending: Mutex::new(Pending::default()),
                wake: Box::new(wake),
            }),
            open: Mutex::new(None),
            read_mode: Mutex::new(Arc::new(fallback::ReadMode::default())),
        });
        let jobs = Jobs {
            next: Arc::new(AtomicU64::new(1)),
            reads: reads.clone(),
            quitting: Arc::new(Mutex::new(false)),
        };
        let writes = JobQueue::new("local-write");
        Self {
            watcher: Mutex::new(None),
            write_jobs: Writes {
                queue: writes.handle(),
                gate: StartGate::new(),
                jobs: jobs.clone(),
            },
            writes,
            network: JobQueue::new("network"),
            network_jobs: Arc::default(),
            reads,
            _interactive: interactive,
            _background: background,
            jobs,
        }
    }

    pub fn send(&self, intent: Intent) {
        let mut quitting = self.jobs.quitting.lock().unwrap();
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
                // After the queues, so that nothing is left to start, and before the Quit, so
                // that a Job taken off the queue just now can't start git as the app exits.
                self.write_jobs.gate.close();
                self.reads.outbox.publish_quit();
            }
            Intent::Open(_) if *quitting => {
                tracing::debug!("quitting; open ignored");
            }
            Intent::Open(path) => {
                let span = tracing::info_span!("intent", kind = "open", path = %path.display());
                let _entered = span.enter();
                *self.reads.open.lock().unwrap() = Some(path.clone());
                *self.reads.read_mode.lock().unwrap() = Arc::new(fallback::ReadMode::default());
                // Watching starts before the first read, so no change falls between them.
                // The old watcher stops here; its late events are for a repository that is
                // no longer open, which `refresh_if_open` ignores.
                let watcher = watch::start(&path, Arc::downgrade(&self.reads));
                *self.watcher.lock().unwrap() = watcher;
                // A walk of the repository that was open is no longer wanted. Nothing
                // starts a new one: nothing shows history yet.
                self.reads.history_slot.drop_jobs();
                self.reads.refresh(
                    &path,
                    Asked {
                        running: Running::Supersede,
                        cause: "open",
                        span: &span,
                    },
                );
            }
            Intent::Switch(branch) => {
                let span = tracing::info_span!("intent", kind = "switch", branch = %branch);
                self.write_jobs.queue_switch(branch, span, None);
            }
            Intent::StashAndSwitch { branch, untracked } => {
                let span = tracing::info_span!(
                    "intent",
                    kind = "stash_and_switch",
                    branch = %branch,
                    untracked
                );
                let switch_span = span.clone();
                let next: Next = Box::new(move |writes, stash| {
                    writes.queue_switch(branch, switch_span, Some(stash));
                });
                self.write_jobs.queue(
                    JobKind::Stash { untracked },
                    span,
                    None,
                    move |workdir, gate, output| stash::stash(workdir, untracked, gate, output),
                    Some(next),
                );
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

    /// Walks the open repository's history from all refs and HEAD, in the order of `git
    /// rev-list --topo-order --date-order --all`, as a Read Job in the Background lane: through
    /// gix when the repository has a commit-graph and `git rev-list` when it doesn't. gix may
    /// order commits of equal date differently from git.
    ///
    /// The answer comes back on the returned channel rather than as a Snapshot, until the
    /// graph Snapshot replaces this in Phase 2 (ADR 0006). Nothing starts a walk yet. The
    /// channel closes without an answer if the walk is Superseded by another walk or an
    /// Open, or dropped by a Quit.
    pub fn walk_history(&self) -> mpsc::Receiver<Result<Vec<Oid>, String>> {
        let (answer, answered) = mpsc::channel();
        // Held across the submit, as `Reads::close` requires.
        let open = self.reads.open.lock().unwrap();
        let Some(path) = open.clone() else {
            let _ = answer.send(Err("no repository is open".to_owned()));
            return answered;
        };
        let mode = self.reads.read_mode.lock().unwrap().clone();
        let span = tracing::info_span!("walk_history", path = %path.display());
        let _entered = span.enter();
        self.reads
            .history_slot
            .submit(Running::Supersede, "walk_history", move |job| {
                let walked = read_history(&path, job, &mode);
                if job.is_current() {
                    let _ = answer.send(walked);
                } else {
                    tracing::debug!("superseded while running; result dropped");
                }
            });
        answered
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
        let Some((publisher, workdir)) = self.jobs.new_job(kind, span, None) else {
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

impl Jobs {
    /// A publisher for a new Job in the open repository, or `None` after publishing that
    /// no repository is open. `follows` is the Job whose success started this one.
    fn new_job(
        &self,
        kind: JobKind,
        span: tracing::Span,
        follows: Option<JobId>,
    ) -> Option<(JobPublisher, PathBuf)> {
        let id = JobId(self.next.fetch_add(1, Ordering::Relaxed));
        let workdir = self.reads.open.lock().unwrap().clone();
        let job = JobPublisher {
            id,
            kind,
            workdir: workdir.clone().unwrap_or_default(),
            span,
            follows,
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

    /// Runs `start`, which queues the second Job of an Intent made of two after the first
    /// succeeded in `workdir`, unless that repository is no longer open: another Open has
    /// named a different one, or a Quit or a Force quit has closed it. Decided under the
    /// lock each Intent holds, so a Quit either comes first and nothing starts, or comes
    /// after and waits for the Job `start` queues. Any queue's Job can call it.
    fn start_next(&self, workdir: &Path, start: impl FnOnce()) {
        let _intents = self.quitting.lock().unwrap();
        if self.reads.open.lock().unwrap().as_deref() != Some(workdir) {
            tracing::debug!("the repository is no longer open; the next Job doesn't start");
            return;
        }
        start();
    }
}

impl Writes {
    /// Queues a switch to `branch`, as the Switch Intent does or as the second Job of a
    /// Stash and switch.
    fn queue_switch(&self, branch: String, span: tracing::Span, follows: Option<JobId>) {
        let kind = JobKind::Switch {
            branch: branch.clone(),
        };
        self.queue(
            kind,
            span,
            follows,
            move |workdir, gate, output| switch::switch(workdir, &branch, gate, output),
            None,
        );
    }

    /// Queues a Local write Job that runs `run` in the open repository, publishing its
    /// state and live output, and then refreshes refs and status whatever the outcome. If it
    /// succeeds, `next` queues the Job that follows it, before its outcome is published.
    fn queue(
        &self,
        kind: JobKind,
        span: tracing::Span,
        follows: Option<JobId>,
        run: impl FnOnce(&Path, &StartGate, &mut dyn FnMut(&[u8])) -> Result<(), String>
        + Send
        + 'static,
        next: Option<Next>,
    ) {
        let Some((job, workdir)) = self.jobs.new_job(kind, span, follows) else {
            return;
        };
        job.publish(JobState::Queued, "");
        let writes = self.clone();
        self.queue.push(move || {
            let _entered =
                tracing::info_span!(parent: &job.span, "local_write_job", id = job.id.0).entered();
            job.publish(JobState::Running, "");
            let (state, output) =
                job.run(|output| JobState::Done(run(&workdir, &writes.gate, output)));
            writes
                .jobs
                .reads
                .refresh_if_open(&workdir, Refresh::RefsAndStatus, &job.span);
            if let (Some(next), JobState::Done(Ok(()))) = (next, &state) {
                let id = job.id;
                writes.jobs.start_next(&workdir, || next(&writes, id));
            }
            job.publish(state, &output);
        });
    }
}

impl Drop for RepoService {
    /// Stops every Network Job, so that dropping the Network queue doesn't wait on the
    /// network. The fields then drop in order.
    fn drop(&mut self) {
        self.stop_network_jobs();
        // A walk can take seconds, and dropping the Background lane waits for it.
        self.reads.history_slot.drop_jobs();
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
    follows: Option<JobId>,
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
            follows: self.follows,
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
#[derive(Clone, Debug, PartialEq, Eq)]
enum Refresh {
    Refs,
    /// Staged, Unstaged and Untracked, in full.
    Status,
    RefsAndStatus,
    /// Files in the worktree changed, and nothing in git's own directories did. HEAD vs index
    /// can't change, so only Unstaged and Untracked are read, narrowed to these files.
    Worktree(Touched),
    /// The index or HEAD changed, and maybe these files. Staged is read in full, and Unstaged
    /// and Untracked only for these files and the entries the index changed (#54).
    Index(Touched),
    /// [`Refs`](Refresh::Refs) and [`Index`](Refresh::Index).
    RefsAndIndex(Touched),
}

/// How far a [`Refresh`] reads status, from least to most.
enum StatusRead {
    None,
    Worktree(Touched),
    Index(Touched),
    Full,
}

/// Worktree files an External change touched.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Touched {
    /// Relative to the worktree root, with `/` separators.
    paths: std::collections::BTreeSet<Vec<u8>>,
    /// A `.gitignore` changed, so any untracked file may now be ignored or not.
    excludes: bool,
}

impl Refresh {
    /// How a `read_job` span names it.
    fn name(&self) -> &'static str {
        match self {
            Refresh::Refs => "refs",
            Refresh::Status => "status",
            Refresh::RefsAndStatus => "refs_and_status",
            Refresh::Worktree(_) => "worktree",
            Refresh::Index(_) => "index",
            Refresh::RefsAndIndex(_) => "refs_and_index",
        }
    }

    /// Whether it reads refs, and how far it reads status.
    fn parts(self) -> (bool, StatusRead) {
        match self {
            Refresh::Refs => (true, StatusRead::None),
            Refresh::Status => (false, StatusRead::Full),
            Refresh::RefsAndStatus => (true, StatusRead::Full),
            Refresh::Worktree(t) => (false, StatusRead::Worktree(t)),
            Refresh::Index(t) => (false, StatusRead::Index(t)),
            Refresh::RefsAndIndex(t) => (true, StatusRead::Index(t)),
        }
    }

    /// Everything either refreshes.
    fn union(self, other: Self) -> Self {
        let ((refs_a, a), (refs_b, b)) = (self.parts(), other.parts());
        let both = |mut a: Touched, b: Touched| {
            a.paths.extend(b.paths);
            a.excludes |= b.excludes;
            a
        };
        let status = match (a, b) {
            (StatusRead::Full, _) | (_, StatusRead::Full) => StatusRead::Full,
            (StatusRead::None, x) | (x, StatusRead::None) => x,
            (StatusRead::Worktree(a), StatusRead::Worktree(b)) => StatusRead::Worktree(both(a, b)),
            (StatusRead::Index(a) | StatusRead::Worktree(a), StatusRead::Index(b))
            | (StatusRead::Index(a), StatusRead::Worktree(b)) => StatusRead::Index(both(a, b)),
        };
        match (refs_a || refs_b, status) {
            (true, StatusRead::None) => Refresh::Refs,
            (false, StatusRead::Full) => Refresh::Status,
            (true, StatusRead::Full) => Refresh::RefsAndStatus,
            (false, StatusRead::Worktree(t)) => Refresh::Worktree(t),
            (false, StatusRead::Index(t)) => Refresh::Index(t),
            // Nothing reads refs and only the worktree, so this reads Staged too.
            (true, StatusRead::Worktree(t) | StatusRead::Index(t)) => Refresh::RefsAndIndex(t),
            (false, StatusRead::None) => unreachable!("every Refresh reads something"),
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
            &self.history_slot,
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
            let asked = Asked {
                running: Running::Finish,
                cause: what.name(),
                span,
            };
            match what {
                Refresh::RefsAndStatus => self.refresh(path, asked),
                Refresh::Refs => self.refresh_refs(path, asked),
                Refresh::Status => self.refresh_status(path, asked),
                Refresh::Worktree(touched) => self.refresh_worktree(path, touched, asked),
                Refresh::Index(touched) => self.refresh_index(path, touched, asked),
                Refresh::RefsAndIndex(touched) => {
                    self.refresh_refs(path, asked);
                    self.refresh_index(path, touched, asked);
                }
            }
        } else {
            tracing::debug!(path = %path.display(), "repository no longer open; refresh skipped");
        }
    }

    /// Reads refs and status again: everything a switch can change, and everything an
    /// Open shows. Later Intents that change less refresh less.
    fn refresh(&self, path: &Path, asked: Asked) {
        self.refresh_refs(path, asked);
        self.refresh_status(path, asked);
    }

    fn refresh_refs(&self, path: &Path, asked: Asked) {
        let mode = self.read_mode.lock().unwrap().clone();
        self.submit(
            &self.refs_slot,
            path,
            asked,
            move |path, job| read_refs(path, job, &mode),
            |p| &mut p.refs,
        );
    }

    /// Reads Staged, Unstaged and Untracked again, in full.
    fn refresh_status(&self, path: &Path, asked: Asked) {
        self.submit(&self.staged_slot, path, asked, read_staged, |p| {
            &mut p.staged
        });
        self.unstaged_narrowed.everything();
        self.untracked_narrowed.everything();
        self.submit_unstaged_and_untracked(path, asked);
    }

    /// Reads Unstaged and Untracked again, narrowed to the files an External change touched.
    fn refresh_worktree(&self, path: &Path, touched: Touched, asked: Asked) {
        self.unstaged_narrowed.touch(touched.paths.iter().cloned());
        self.untracked_narrowed.touch(touched.paths);
        if touched.excludes {
            self.untracked_narrowed.everything();
        }
        self.submit_unstaged_and_untracked(path, asked);
    }

    /// Reads Staged again in full, and Unstaged and Untracked narrowed to the files an
    /// External change touched and the entries the index changed.
    fn refresh_index(&self, path: &Path, touched: Touched, asked: Asked) {
        self.submit(&self.staged_slot, path, asked, read_staged, |p| {
            &mut p.staged
        });
        self.unstaged_narrowed.index();
        self.untracked_narrowed.index();
        self.refresh_worktree(path, touched, asked);
    }

    fn submit_unstaged_and_untracked(&self, path: &Path, asked: Asked) {
        let narrowed = self.unstaged_narrowed.clone();
        self.submit(
            &self.unstaged_slot,
            path,
            asked,
            move |path, job| read_unstaged(path, job, &narrowed),
            |p| &mut p.unstaged,
        );
        let mode = self.read_mode.lock().unwrap().clone();
        let narrowed = self.untracked_narrowed.clone();
        self.submit(
            &self.untracked_slot,
            path,
            asked,
            move |path, job| read_untracked(path, job, &mode, &narrowed),
            |p| &mut p.untracked,
        );
    }

    /// Submits a Read Job to `slot` that runs `read` on the repository at `path` and
    /// publishes its answer into the outbox cell `cell` picks.
    fn submit<T: Send + Sync + 'static>(
        &self,
        slot: &Slot,
        path: &Path,
        asked: Asked,
        read: impl Fn(&Path, &JobContext) -> Result<T, String> + Send + 'static,
        cell: fn(&mut Pending) -> &mut Latest<T>,
    ) {
        let outbox = self.outbox.clone();
        let path = path.to_path_buf();
        let span = asked.span.clone();
        slot.submit(asked.running, asked.cause, move |job| {
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

/// Who asked for a refresh, and what it does to a read already running.
#[derive(Clone, Copy)]
struct Asked<'a> {
    running: Running,
    /// Names what asked, in each Read Job's span: `open`, or a [`Refresh`].
    cause: &'static str,
    /// The span of the Intent, Job or External change that asked.
    span: &'a tracing::Span,
}

/// Reads refs with gix or the CLI fallback.
fn read_refs(path: &Path, job: &JobContext, mode: &fallback::ReadMode) -> Result<Refs, String> {
    mode.read(path, job, fallback::refs, || {
        cairn_read::Repo::open(path)
            .and_then(|repo| repo.refs())
            .map_err(|e| e.message().to_owned())
    })
}

/// Walks history with the engine that passed for this repository (#13). No commit-graph
/// means `git rev-list`, which is not a gix failure. A gix error, a broken commit-graph's
/// included, retries through git like any other read.
fn read_history(
    path: &Path,
    job: &JobContext,
    mode: &fallback::ReadMode,
) -> Result<Vec<Oid>, String> {
    let message = |e: cairn_read::ReadError| e.message().to_owned();
    let opened = cairn_read::Repo::open(path)
        .and_then(|repo| repo.has_commit_graph().map(|graph| (repo, graph)))
        .map_err(message);
    if let Ok((_, false)) = opened {
        return fallback::walk(path, job.cli_stop());
    }
    mode.read(path, job, fallback::walk, || {
        opened?.0.walk(job.gix_interrupt()).map_err(message)
    })
}

/// Reads untracked files with gix or the CLI fallback, stopping once Superseded. gix reads
/// only what changed when it can; the fallback reads everything.
fn read_untracked(
    path: &Path,
    job: &JobContext,
    mode: &fallback::ReadMode,
    narrowed: &narrow::Narrowed<RepoPath>,
) -> Result<Vec<RepoPath>, String> {
    let (scope, reading) = narrowed.begin(path);
    let within = match scope {
        narrow::Scope::Unchanged { last, index } => {
            record_scope(Some(0));
            let result = Ok(last);
            reading.finish(&result, Some(index));
            return result;
        }
        narrow::Scope::Everything => None,
        narrow::Scope::Paths {
            last,
            before,
            paths,
        } => Some((last, before, paths)),
    };
    let mut changed_only = None;
    let mut read_against = None;
    let result = mode.read(path, job, fallback::untracked, || {
        let message = |e: cairn_read::ReadError| e.message().to_owned();
        let repo = cairn_read::Repo::open(path).map_err(message)?;
        // Loaded after the read took its changes: an index write after this lands in the next
        // read's changes. The walk reads against this same load.
        let index = repo.index().map_err(message)?;
        let narrowed_to = within.and_then(|(last, before, paths)| {
            let tracked = index.changes_since(&before).tracked;
            narrow::paths_with(paths, tracked).map(|paths| (last, before, paths))
        });
        let read = match &narrowed_to {
            None => repo.untracked_in(&index, job.gix_interrupt()),
            Some((last, before, paths)) => {
                let changed: Vec<&[u8]> = paths.iter().map(Vec::as_slice).collect();
                let read =
                    repo.untracked_within(job.gix_interrupt(), &index, before, last, &changed);
                // Only gix narrows: the fallback, after a failure here too, reads everything.
                changed_only = read.is_ok().then_some(paths.len());
                read
            }
        };
        if read.is_ok() {
            read_against = Some(index);
        }
        read.map_err(message)
    });
    record_scope(changed_only);
    reading.finish(&result, read_against);
    result
}

fn read_staged(path: &Path, job: &JobContext) -> Result<Vec<FileChange>, String> {
    status::staged(path, job.cli_stop())
}

fn read_unstaged(
    path: &Path,
    job: &JobContext,
    narrowed: &narrow::Narrowed<FileChange>,
) -> Result<Vec<FileChange>, String> {
    let (scope, reading) = narrowed.begin(path);
    if let narrow::Scope::Unchanged { last, index } = scope {
        record_scope(Some(0));
        let result = Ok(last);
        reading.finish(&result, Some(index));
        return result;
    }
    // Loaded after the read took its changes and before `diff-files` reads the index, so an
    // index write after this always lands in the next read's changes. Without it the next
    // read reads everything.
    let index = cairn_read::Repo::open(path)
        .and_then(|repo| repo.index())
        .inspect_err(|e| tracing::debug!(error = %e, "couldn't load the index"))
        .ok();
    let narrowed_to = match (scope, &index) {
        (
            narrow::Scope::Paths {
                last,
                before,
                paths,
            },
            Some(index),
        ) => narrow::paths_with(paths, index.changes_since(&before).entries)
            .map(|paths| (last, paths)),
        _ => None,
    };
    let result = match narrowed_to {
        None => {
            record_scope(None);
            status::unstaged(path, job.cli_stop())
        }
        // The index changed no entry, as when HEAD moved alone, and no worktree path changed.
        Some((last, paths)) if paths.is_empty() => {
            record_scope(Some(0));
            Ok(last)
        }
        Some((last, paths)) => {
            record_scope(Some(paths.len()));
            let paths: Vec<&[u8]> = paths.iter().map(Vec::as_slice).collect();
            status::unstaged_within(path, job.cli_stop(), &last, &paths)
        }
    };
    reading.finish(&result, index);
    result
}

/// Says in the running `read_job` span whether this read was narrowed to the paths an
/// External change touched, and to how many, or read everything.
fn record_scope(changed_only: Option<usize>) {
    let span = tracing::Span::current();
    match changed_only {
        Some(paths) => {
            span.record("scope", "paths");
            span.record("paths", paths);
        }
        None => {
            span.record("scope", "full");
        }
    }
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
