//! The Job scheduler, private to the Repo service.
//!
//! One Interactive lane of worker threads that `repo` owns, Slots and generation tokens
//! for Read Jobs, and the Job queues that Local write Jobs and Network Jobs each run in.
//! The Background lane arrives with its own ticket.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use cairn_write::Stop;

use crate::Generation;

type Work = Box<dyn FnOnce() + Send>;

/// Worker threads that run Read Jobs in arrival order. Capped, named, and joined on drop.
pub(crate) struct Lane {
    shared: Arc<LaneShared>,
    workers: Vec<JoinHandle<()>>,
}

struct LaneShared {
    queue: Mutex<VecDeque<Work>>,
    ready: Condvar,
    closing: AtomicBool,
}

impl Lane {
    pub(crate) fn new(name: &str, threads: usize) -> Self {
        let shared = Arc::new(LaneShared {
            queue: Mutex::new(VecDeque::new()),
            ready: Condvar::new(),
            closing: AtomicBool::new(false),
        });
        let workers = (0..threads.max(1))
            .map(|i| {
                let shared = shared.clone();
                std::thread::Builder::new()
                    .name(format!("cairn-{name}-{i}"))
                    .spawn(move || shared.work())
                    .expect("spawn a lane worker")
            })
            .collect();
        Self { shared, workers }
    }

    fn handle(&self) -> LaneHandle {
        LaneHandle(self.shared.clone())
    }
}

impl LaneShared {
    fn work(&self) {
        loop {
            let job = {
                let mut queue = self.queue.lock().unwrap();
                loop {
                    if self.closing.load(Ordering::Acquire) {
                        return;
                    }
                    if let Some(job) = queue.pop_front() {
                        break job;
                    }
                    queue = self.ready.wait(queue).unwrap();
                }
            };
            job();
        }
    }
}

impl Drop for Lane {
    fn drop(&mut self) {
        self.shared.closing.store(true, Ordering::Release);
        // Queued Read Jobs are dropped: nobody needs their results any more.
        self.shared.queue.lock().unwrap().clear();
        self.shared.ready.notify_all();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

/// What a Slot keeps of its lane: enough to queue work, without keeping the threads alive.
#[derive(Clone)]
struct LaneHandle(Arc<LaneShared>);

impl LaneHandle {
    fn push(&self, work: Work) {
        if self.0.closing.load(Ordering::Acquire) {
            return;
        }
        self.0.queue.lock().unwrap().push_back(work);
        self.0.ready.notify_one();
    }
}

/// What a running Read Job knows about itself.
pub(crate) struct JobContext {
    generation: Generation,
    stops: Stops,
}

impl JobContext {
    pub(crate) fn generation(&self) -> Generation {
        self.generation
    }

    /// False once a newer Read Job for the same Slot has Superseded this one. A Read Job
    /// that a refresh followed is still wanted: its answer is newer than what was shown.
    pub(crate) fn is_current(&self) -> bool {
        !self.stops.gix.load(Ordering::Relaxed)
    }

    /// The handle that kills this Read Job's git once it is Superseded. Every CLI read
    /// attaches it.
    pub(crate) fn cli_stop(&self) -> &Stop {
        &self.stops.cli
    }

    /// Set once this Read Job is Superseded, for gix to stop on. It is also what
    /// [`is_current`](Self::is_current) reads.
    pub(crate) fn gix_interrupt(&self) -> &AtomicBool {
        &self.stops.gix
    }
}

/// The ways a running Read Job can be stopped cooperatively: a process kill for a CLI read
/// and a flag for a gix one. A Superseded read that checks neither runs to its end, and its
/// result is dropped.
#[derive(Clone, Default)]
struct Stops {
    cli: Stop,
    gix: Arc<AtomicBool>,
}

impl Stops {
    fn stop(&self) {
        self.gix.store(true, Ordering::Relaxed);
        self.cli.stop();
    }
}

type ReadJob = Box<dyn FnOnce(&JobContext) + Send>;

/// One purpose a Read Job serves. Holds at most one running and one pending Read Job.
#[derive(Clone)]
pub(crate) struct Slot(Arc<SlotShared>);

struct SlotShared {
    name: &'static str,
    lane: LaneHandle,
    state: Mutex<SlotState>,
}

/// What a newly submitted Read Job does to the one already running in its Slot. Either way
/// it replaces the pending one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Running {
    /// Supersede it: the new Read Job answers a new question, such as another repository,
    /// so the running one is stopped where it can be and its result dropped.
    Supersede,
    /// Let it finish and publish: the new Read Job asks the same question again after an
    /// External change or a Job, and the running one's answer is newer than what is shown.
    /// A stream of changes then still publishes one answer per read (ADR 0004).
    Finish,
}

struct SlotState {
    latest: Generation,
    /// How to stop the running Read Job, if one is running.
    running: Option<Stops>,
    pending: Option<(Generation, tracing::Span, ReadJob)>,
}

impl Slot {
    pub(crate) fn new(name: &'static str, lane: &Lane) -> Self {
        Self(Arc::new(SlotShared {
            name,
            lane: lane.handle(),
            state: Mutex::new(SlotState {
                latest: Generation(0),
                running: None,
                pending: None,
            }),
        }))
    }

    /// Submits a Read Job and returns the generation it answers. It Supersedes the pending
    /// Read Job, if any, and treats the running one as `running` says.
    pub(crate) fn submit(
        &self,
        running: Running,
        job: impl FnOnce(&JobContext) + Send + 'static,
    ) -> Generation {
        let span = tracing::Span::current();
        let mut state = self.0.state.lock().unwrap();
        state.latest = Generation(state.latest.0 + 1);
        let generation = state.latest;
        match &state.running {
            Some(stops) => {
                if running == Running::Supersede {
                    stops.stop();
                }
                if let Some((old, ..)) = state.pending.replace((generation, span, Box::new(job))) {
                    tracing::debug!(
                        slot = self.0.name,
                        generation = old.0,
                        "superseded pending read job"
                    );
                }
            }
            None => {
                let stops = Stops::default();
                state.running = Some(stops.clone());
                drop(state);
                self.start(generation, stops, span, Box::new(job));
            }
        }
        generation
    }

    fn start(&self, generation: Generation, stops: Stops, parent: tracing::Span, job: ReadJob) {
        let slot = self.clone();
        self.0.lane.push(Box::new(move || {
            let context = JobContext {
                generation,
                stops,
            };
            if context.is_current() {
                let span = tracing::info_span!(
                    parent: &parent,
                    "read_job",
                    slot = slot.0.name,
                    generation = generation.0
                );
                // A panicking Read Job must not wedge its Slot or kill a lane worker.
                let outcome = span.in_scope(|| {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&context)))
                });
                if outcome.is_err() {
                    tracing::error!(parent: &parent, slot = slot.0.name, generation = generation.0, "read job panicked");
                }
            } else {
                tracing::debug!(parent: &parent, slot = slot.0.name, generation = generation.0, "superseded before it ran");
            }
            slot.finish();
        }));
    }

    fn finish(&self) {
        let mut state = self.0.state.lock().unwrap();
        match state.pending.take() {
            Some((generation, span, job)) => {
                let stops = Stops::default();
                state.running = Some(stops.clone());
                drop(state);
                self.start(generation, stops, span, job);
            }
            None => state.running = None,
        }
    }
}

/// A Job queue: one thread that runs Jobs one at a time, in the order they were queued.
/// The Local write queue and the Network queue are one each, so they overlap each other
/// but never themselves. Nothing here stops or Supersedes a Job (ADR 0002): dropping the
/// queue waits for every queued Job to finish, so a Network Job must be stopped first.
pub(crate) struct JobQueue {
    shared: Arc<LaneShared>,
    worker: Option<JoinHandle<()>>,
}

impl JobQueue {
    pub(crate) fn new(name: &str) -> Self {
        let shared = Arc::new(LaneShared {
            queue: Mutex::new(VecDeque::new()),
            ready: Condvar::new(),
            closing: AtomicBool::new(false),
        });
        let worker = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name(format!("cairn-{name}"))
                .spawn(move || shared.work_to_completion())
                .expect("spawn a job queue worker")
        };
        Self {
            shared,
            worker: Some(worker),
        }
    }

    /// Queues a Job behind every one queued before it.
    pub(crate) fn push(&self, job: impl FnOnce() + Send + 'static) {
        self.shared.queue.lock().unwrap().push_back(Box::new(job));
        self.shared.ready.notify_one();
    }
}

impl LaneShared {
    /// Like [`work`](Self::work), but closing only stops it once the queue is empty.
    fn work_to_completion(&self) {
        loop {
            let job = {
                let mut queue = self.queue.lock().unwrap();
                loop {
                    if let Some(job) = queue.pop_front() {
                        break job;
                    }
                    if self.closing.load(Ordering::Acquire) {
                        return;
                    }
                    queue = self.ready.wait(queue).unwrap();
                }
            };
            // A panicking Job must not take the queue down with it.
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).is_err() {
                tracing::error!("queued job panicked");
            }
        }
    }
}

impl Drop for JobQueue {
    fn drop(&mut self) {
        self.shared.closing.store(true, Ordering::Release);
        self.shared.ready.notify_all();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
