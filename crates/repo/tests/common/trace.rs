//! One tracing subscriber for the whole test binary, which records the spans a caller can see
//! (#40): each `read_job` span once it closes, and each `external_change` span as it starts.
//! Every span carries the repository it is about, since other tests' services run in the
//! same binary.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt as _};
use tracing_subscriber::registry::LookupSpan;

/// One span's fields as text, by name, with its repository as `repository`.
pub type Fields = BTreeMap<String, String>;

#[derive(Default)]
struct Spans {
    /// Every `read_job` span closed so far, in the order they closed.
    read_jobs: Mutex<Vec<Fields>>,
    /// Every `external_change` span so far, in the order the watcher started them.
    external_changes: Mutex<Vec<Fields>>,
}

fn spans() -> &'static Spans {
    static SPANS: OnceLock<Spans> = OnceLock::new();
    SPANS.get_or_init(|| {
        let subscriber = tracing_subscriber::registry().with(Recorded);
        tracing::subscriber::set_global_default(subscriber).expect("one subscriber per binary");
        Spans::default()
    })
}

/// Installs the subscriber, before the test starts anything it should see.
pub fn install() {
    spans();
}

/// Every `read_job` span closed so far, in the order they closed.
pub fn read_jobs() -> Vec<Fields> {
    spans().read_jobs.lock().unwrap().clone()
}

/// Every `external_change` span so far, in the order the watcher started them. The watcher
/// handles one event at a time, in the order the OS reported them.
pub fn external_changes() -> Vec<Fields> {
    spans().external_changes.lock().unwrap().clone()
}

/// Whether a span is about the repository at `repo`, as the test opened it.
pub fn about(span: &Fields, repo: &Path) -> bool {
    span.get("repository")
        .is_some_and(|r| r.as_str() == repo.display().to_string())
}

/// Waits until `spans` lists one that `matches` accepts, and returns where the first such
/// one lies, and it. A span can trail the Snapshot or the event that caused it. `kind`
/// names the spans for the failure message.
pub fn wait_for(
    spans: fn() -> Vec<Fields>,
    kind: &str,
    matches: impl Fn(&Fields) -> bool,
) -> (usize, Fields) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let listed = spans();
        if let Some(at) = listed.iter().position(&matches) {
            return (at, listed[at].clone());
        }
        assert!(Instant::now() < deadline, "no such {kind} span: {listed:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct Recorded;

/// The repository a span is about, kept for the `read_job` spans inside it.
struct Repository(String);

struct Recorder<'a>(&'a mut Fields);

impl Visit for Recorder<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_owned(), format!("{value:?}"));
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_owned(), value.to_owned());
    }
}

impl<S: tracing::Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Recorded {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).unwrap();
        let mut fields = Fields::new();
        attrs.record(&mut Recorder(&mut fields));
        let name = attrs.metadata().name();
        if name != "read_job" {
            // An Intent's or an External change's span names the repository it is about.
            if let Some(path) = fields.remove("path") {
                span.extensions_mut().insert(Repository(path.clone()));
                if name == "external_change" {
                    fields.insert("repository".to_owned(), path);
                    spans().external_changes.lock().unwrap().push(fields);
                }
            }
            return;
        }
        // Which repository it read.
        if let Some(parent) = span.parent()
            && let Some(Repository(path)) = parent.extensions().get::<Repository>()
        {
            fields.insert("repository".to_owned(), path.clone());
        }
        span.extensions_mut().insert(fields);
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let span = ctx.span(id).unwrap();
        if let Some(fields) = span.extensions_mut().get_mut::<Fields>() {
            values.record(&mut Recorder(fields));
        }
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        let span = ctx.span(&id).unwrap();
        if let Some(fields) = span.extensions_mut().remove::<Fields>() {
            spans().read_jobs.lock().unwrap().push(fields);
        }
    }
}
