//! The Job scheduler, private to the Repo service.
//!
//! One Interactive lane of worker threads that `repo` owns, Slots and generation tokens
//! for Read Jobs, and the Local write queue. The Background lane and Network Jobs arrive
//! with their own tickets.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

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
    slot: Arc<SlotShared>,
}

impl JobContext {
    pub(crate) fn generation(&self) -> Generation {
        self.generation
    }

    /// False once a newer Read Job for the same Slot has Superseded this one.
    pub(crate) fn is_current(&self) -> bool {
        self.slot.state.lock().unwrap().latest == self.generation
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

struct SlotState {
    latest: Generation,
    running: bool,
    pending: Option<(Generation, tracing::Span, ReadJob)>,
}

impl Slot {
    pub(crate) fn new(name: &'static str, lane: &Lane) -> Self {
        Self(Arc::new(SlotShared {
            name,
            lane: lane.handle(),
            state: Mutex::new(SlotState {
                latest: Generation(0),
                running: false,
                pending: None,
            }),
        }))
    }

    /// Submits a Read Job and returns the generation it answers. It Supersedes the pending
    /// Read Job, if any, and makes the running one's result unwanted.
    pub(crate) fn submit(&self, job: impl FnOnce(&JobContext) + Send + 'static) -> Generation {
        let span = tracing::Span::current();
        let mut state = self.0.state.lock().unwrap();
        state.latest = Generation(state.latest.0 + 1);
        let generation = state.latest;
        if state.running {
            if let Some((old, ..)) = state.pending.replace((generation, span, Box::new(job))) {
                tracing::debug!(
                    slot = self.0.name,
                    generation = old.0,
                    "superseded pending read job"
                );
            }
        } else {
            state.running = true;
            drop(state);
            self.start(generation, span, Box::new(job));
        }
        generation
    }

    fn start(&self, generation: Generation, parent: tracing::Span, job: ReadJob) {
        let slot = self.clone();
        self.0.lane.push(Box::new(move || {
            let context = JobContext {
                generation,
                slot: slot.0.clone(),
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
                drop(state);
                self.start(generation, span, job);
            }
            None => state.running = false,
        }
    }
}

/// The Local write queue: one thread that runs Local write Jobs one at a time, in the
/// order they were queued. A Local write Job is never stopped or Superseded (ADR 0002),
/// so dropping the queue waits for every queued Job to finish.
pub(crate) struct WriteQueue {
    shared: Arc<LaneShared>,
    worker: Option<JoinHandle<()>>,
}

impl WriteQueue {
    pub(crate) fn new() -> Self {
        let shared = Arc::new(LaneShared {
            queue: Mutex::new(VecDeque::new()),
            ready: Condvar::new(),
            closing: AtomicBool::new(false),
        });
        let worker = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("cairn-local-write".to_owned())
                .spawn(move || shared.work_to_completion())
                .expect("spawn the local write worker")
        };
        Self {
            shared,
            worker: Some(worker),
        }
    }

    /// Queues a Local write Job behind every one queued before it.
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
                tracing::error!("local write job panicked");
            }
        }
    }
}

impl Drop for WriteQueue {
    fn drop(&mut self) {
        self.shared.closing.store(true, Ordering::Release);
        self.shared.ready.notify_all();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
