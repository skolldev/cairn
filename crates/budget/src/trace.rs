//! Where an External change's time goes, Slot by Slot: the same samples as the Budget's rows,
//! against a headless Repo service, with every `read_job` span each one waited for (#40, #56).
//!
//! It explains a number and never gives one. With no window nothing is presented, so a sample
//! ends when the expected Snapshots are drained, not when a frame reaches the screen.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use cairn_repo::{Head, Intent, Refs, RepoService, Snapshot};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt as _};
use tracing_subscriber::registry::LookupSpan;

use crate::clock;
use crate::fixture::{BRANCH_A, Sandbox};
use crate::probe::Frame;
use crate::run::{self, Change, Config, QUIET, SAMPLE_TIMEOUT, WARM_UP_SAMPLES};

/// Said before any trace, and in `docs/budget-harness.md`.
pub const NOT_A_NUMBER: &str = "This explains where an External change's time goes, and gives no \
     Budget number: with no window nothing is presented, so it writes no result file and gives no \
     verdict. `scripts/budget.sh run` measures the Budget.";

/// One `read_job` span: when it ran, on the machine-wide clock, and what it said.
#[derive(Clone, Debug)]
pub struct Span {
    /// The repository of the Intent or External change that asked for the read.
    pub repository: String,
    /// When a lane worker started it.
    pub started_ns: u64,
    /// When it began to publish its Snapshot, if it did.
    pub published_ns: Option<u64>,
    /// When it ended, after publishing.
    pub closed_ns: u64,
    /// Its fields as text, by name: `slot`, `generation`, `refresh`, `pending`, `lane_wait_ms`,
    /// `ms`, and for a read that can be narrowed, `scope` and `paths`.
    pub fields: BTreeMap<String, String>,
}

impl Span {
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }
}

/// One kept sample.
#[derive(Clone, Debug)]
pub struct Sample {
    /// When the change began: just before the write, or before `git switch` started.
    pub changed_ns: u64,
    /// When the Budget's clock starts: just before the write, or once git has exited.
    pub start_ns: u64,
    /// When the Snapshots showing the expected state were drained.
    pub drained_ns: u64,
    /// The reads of the sandbox that closed after the change began, and had published or ended
    /// by the drain, in the order they started.
    pub spans: Vec<Span>,
}

/// Records `read_job` spans for the whole process, for [`trace_change`]. Call it before the
/// first Repo service starts. Fails if another subscriber was installed first.
pub fn install() -> Result<(), String> {
    static INSTALLED: OnceLock<Result<(), String>> = OnceLock::new();
    INSTALLED
        .get_or_init(|| {
            tracing::subscriber::set_global_default(tracing_subscriber::registry().with(ReadJobs))
                .map_err(|e| format!("couldn't record read_job spans: {e}"))
        })
        .clone()
}

/// Every `read_job` span so far: those still running, by span id, and those closed.
#[derive(Default)]
struct Store {
    running: BTreeMap<u64, Span>,
    closed: Vec<Span>,
}

fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(Mutex::default)
}

struct ReadJobs;

/// The repository an Intent's or External change's span names, for the reads inside it.
struct Repository(String);

struct Fields<'a>(&'a mut BTreeMap<String, String>);

impl Visit for Fields<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_owned(), format!("{value:?}"));
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_owned(), value.to_owned());
    }
}

impl<S: tracing::Subscriber + for<'a> LookupSpan<'a>> Layer<S> for ReadJobs {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let started_ns = clock::now_ns();
        let Some(span) = ctx.span(id) else { return };
        let mut fields = BTreeMap::new();
        attrs.record(&mut Fields(&mut fields));
        match attrs.metadata().name() {
            "read_job" => {}
            // A Read Job publishing its Snapshot.
            "publish" => {
                if let Some(read) = span.parent()
                    && let Some(read) = store()
                        .lock()
                        .unwrap()
                        .running
                        .get_mut(&read.id().into_u64())
                {
                    read.published_ns.get_or_insert(started_ns);
                }
                return;
            }
            _ => {
                if let Some(path) = fields.remove("path") {
                    span.extensions_mut().insert(Repository(path));
                }
                return;
            }
        }
        let repository = span
            .parent()
            .and_then(|parent| {
                let extensions = parent.extensions();
                extensions.get::<Repository>().map(|r| r.0.clone())
            })
            .unwrap_or_default();
        store().lock().unwrap().running.insert(
            id.into_u64(),
            Span {
                repository,
                started_ns,
                published_ns: None,
                closed_ns: 0,
                fields,
            },
        );
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, _ctx: Context<'_, S>) {
        if let Some(span) = store().lock().unwrap().running.get_mut(&id.into_u64()) {
            values.record(&mut Fields(&mut span.fields));
        }
    }

    fn on_close(&self, id: Id, _ctx: Context<'_, S>) {
        let closed_ns = clock::now_ns();
        let mut store = store().lock().unwrap();
        if let Some(mut span) = store.running.remove(&id.into_u64()) {
            span.closed_ns = closed_ns;
            store.closed.push(span);
        }
    }
}

/// A headless Repo service on one repository, and what a window would show of it.
struct Headless {
    service: RepoService,
    woken: mpsc::Receiver<()>,
    repository: String,
    /// What the views would hold: the probe's frame, with the drain's time for its paint.
    shown: Frame,
}

impl Headless {
    fn open(repo: &Path) -> Result<Self, String> {
        let (wake, woken) = mpsc::channel();
        let service = RepoService::start(move || {
            let _ = wake.send(());
        });
        service.send(Intent::Open(repo.to_path_buf()));
        let mut headless = Self {
            service,
            woken,
            repository: repo.display().to_string(),
            shown: Frame::default(),
        };
        headless.wait("the repository to open", |f| {
            f.refs > 0 && f.staged > 0 && f.unstaged > 0 && f.untracked > 0
        })?;
        Ok(headless)
    }

    /// Takes what has been published, as the Bridge does, and returns whether there was any.
    fn drain(&mut self) -> bool {
        let snapshots = self.service.drain();
        let drained = !snapshots.is_empty();
        if drained {
            self.shown.paint_ns = clock::now_ns();
        }
        for snapshot in snapshots {
            match snapshot {
                Snapshot::Refs(refs) => {
                    self.shown.refs = refs.generation.get();
                    self.shown.head = match &refs.result {
                        Ok(Refs {
                            head: Head::Branch { name, .. },
                            ..
                        }) => Some(name.short().to_owned()),
                        _ => None,
                    };
                }
                Snapshot::Staged(staged) => self.shown.staged = staged.generation.get(),
                Snapshot::Unstaged(unstaged) => {
                    self.shown.unstaged = unstaged.generation.get();
                    if let Ok(changes) = &unstaged.result {
                        self.shown.unstaged_paths =
                            changes.iter().map(|c| c.path.to_string()).collect();
                    }
                }
                Snapshot::Untracked(untracked) => {
                    self.shown.untracked = untracked.generation.get();
                    if let Ok(paths) = &untracked.result {
                        self.shown.untracked_paths =
                            paths.iter().map(ToString::to_string).collect();
                    }
                }
                Snapshot::Counts(_) | Snapshot::Job(_) | Snapshot::Quit => {}
            }
        }
        drained
    }

    /// Drains until `met` holds, and returns when it was drained.
    fn wait(&mut self, what: &str, met: impl Fn(&Frame) -> bool) -> Result<u64, String> {
        let deadline = Instant::now() + SAMPLE_TIMEOUT;
        loop {
            self.drain();
            if met(&self.shown) {
                return Ok(self.shown.paint_ns);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(format!(
                    "timed out waiting for {what}. Shown: {:?}",
                    self.shown
                ));
            }
            let _ = self.woken.recv_timeout(left);
        }
    }

    /// Waits until nothing has been published, and no read of this repository has started or
    /// ended, for [`QUIET`], so that one sample's late reads can't land in the next.
    fn settle(&mut self) -> Result<(), String> {
        let deadline = Instant::now() + SAMPLE_TIMEOUT;
        let mut since = Instant::now();
        let mut last = self.activity();
        loop {
            if self.drain() {
                since = Instant::now();
            }
            let now = self.activity();
            if now != last {
                last = now;
                since = Instant::now();
            }
            if since.elapsed() >= QUIET && now.0 == 0 {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("the Slots never stopped answering".to_owned());
            }
            let _ = self.woken.recv_timeout(Duration::from_millis(5));
        }
    }

    /// How many reads of this repository are running, and how many have closed.
    fn activity(&self) -> (usize, usize) {
        let store = store().lock().unwrap();
        let ours = |s: &&Span| s.repository == self.repository;
        (
            store.running.values().filter(ours).count(),
            store.closed.iter().filter(ours).count(),
        )
    }

    /// The reads that closed after `changed_ns`, and had published or ended by `drained_ns`, once
    /// those that had started by then have ended: the read that published the expected state
    /// ends just after the drain, and the read queued behind it starts then.
    fn spans(&self, changed_ns: u64, drained_ns: u64) -> Result<Vec<Span>, String> {
        let deadline = Instant::now() + SAMPLE_TIMEOUT;
        loop {
            {
                let store = store().lock().unwrap();
                let waited_for =
                    |s: &Span| s.repository == self.repository && s.started_ns <= drained_ns;
                if !store.running.values().any(waited_for) {
                    let mut spans: Vec<Span> = store
                        .closed
                        .iter()
                        .filter(|s| {
                            waited_for(s)
                                && s.closed_ns > changed_ns
                                && s.published_ns.unwrap_or(s.closed_ns) <= drained_ns
                        })
                        .cloned()
                        .collect();
                    spans.sort_by_key(|s| s.started_ns);
                    return Ok(spans);
                }
            }
            if Instant::now() >= deadline {
                return Err("a read never ended".to_owned());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// Runs `samples` samples of `change`, after the Budget's warm-up, against a headless Repo
/// service on `sandbox`, and returns the kept ones. Installs the recorder if it isn't yet.
pub fn trace_change(
    sandbox: &Sandbox,
    change: Change,
    samples: usize,
) -> Result<Vec<Sample>, String> {
    if !change.is_external() {
        return Err(format!("{change:?} isn't an External change"));
    }
    install()?;
    sandbox.save(false)?;
    sandbox.switch(BRANCH_A)?;
    let mut headless = Headless::open(sandbox.repo())?;
    let mut kept = Vec::with_capacity(samples);
    for index in 0..WARM_UP_SAMPLES + samples {
        headless.settle()?;
        let baseline = headless.shown.clone();
        let expect = change.expect(index);
        let changed_ns = clock::now_ns();
        let start_ns = change.perform_external(index, sandbox)?;
        let drained_ns = headless.wait(&format!("{expect:?}"), |shown| {
            expect.met_by(shown, &baseline)
        })?;
        let spans = headless.spans(changed_ns, drained_ns)?;
        if spans.is_empty() {
            // A change always reads again, so no span means none named the sandbox's path.
            return Err(format!(
                "no read_job span named {} for sample {index}",
                headless.repository
            ));
        }
        if index >= WARM_UP_SAMPLES {
            kept.push(Sample {
                changed_ns,
                start_ns,
                drained_ns,
                spans,
            });
        }
    }
    Ok(kept)
}

/// Milliseconds from `start_ns` to `ns`, negative for before it.
fn since(start_ns: u64, ns: u64) -> f64 {
    (ns as f64 - start_ns as f64) / 1e6
}

/// One row's samples as timelines, in ms from each sample's clock start.
pub fn render(name: &str, change: Change, samples: &[Sample]) -> String {
    let start = match change {
        Change::GitSwitch => "once git has exited",
        _ => "just before the write",
    };
    let mut text = format!("## {name}\n");
    for (number, sample) in samples.iter().enumerate() {
        text.push_str(&format!(
            "\nSample {}\n  {:>9.1} ms  clock starts, {start}\n",
            number + 1,
            0.0
        ));
        for span in &sample.spans {
            let field = |name: &str| span.field(name).unwrap_or("-");
            let ms = |name: &str| {
                span.field(name)
                    .and_then(|v| v.parse::<f64>().ok())
                    .map_or("-".to_owned(), |v| format!("{v:.1}"))
            };
            text.push_str(&format!(
                "  {:>9.1} ms  to {:>7.1} ms  {:<9} refresh={} pending={} lane_wait_ms={} ms={} \
                 scope={} paths={}\n",
                since(sample.start_ns, span.started_ns),
                since(sample.start_ns, span.closed_ns),
                field("slot"),
                field("refresh"),
                field("pending"),
                ms("lane_wait_ms"),
                ms("ms"),
                field("scope"),
                field("paths"),
            ));
        }
        text.push_str(&format!(
            "  {:>9.1} ms  expected Snapshots drained\n",
            since(sample.start_ns, sample.drained_ns)
        ));
    }
    text
}

/// Both External change rows on the Linux kernel, as `run` measures them, as timelines.
pub fn trace_external_change(config: &Config) -> Result<String, String> {
    eprintln!("Cloning the kernel fixture into a temp directory...");
    let sandbox = Sandbox::clone_of(&run::kernel_fixture(config)?)?;
    let mut text = format!("# External change, traced\n\n{NOT_A_NUMBER}\n");
    for (name, change) in [
        ("External change, file saved", Change::Save),
        ("External change, git switch", Change::GitSwitch),
    ] {
        let samples = trace_change(&sandbox, change, config.samples)?;
        text.push('\n');
        text.push_str(&render(name, change, &samples));
    }
    Ok(text)
}
