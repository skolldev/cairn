//! Each Read Job is a `read_job` tracing span, which says where an External change's time
//! goes (#40): its Slot, the Refresh that started it, whether it waited behind a running read,
//! how long it waited for a lane worker and how long it ran. A subscriber is how a caller
//! sees spans, so these tests install one for the whole binary.

mod common;

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use common::{Harness, Seen, open, small_fixture_clone, unstaged};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt as _};
use tracing_subscriber::registry::LookupSpan;

/// One closed `read_job` span: its fields as text, by name.
type Fields = BTreeMap<String, String>;

/// Every `read_job` span closed so far, in the order they closed.
fn closed() -> &'static Mutex<Vec<Fields>> {
    static CLOSED: OnceLock<Mutex<Vec<Fields>>> = OnceLock::new();
    CLOSED.get_or_init(|| {
        let subscriber = tracing_subscriber::registry().with(ReadJobs);
        tracing::subscriber::set_global_default(subscriber).expect("one subscriber per binary");
        Mutex::default()
    })
}

struct ReadJobs;

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

impl<S: tracing::Subscriber + for<'a> LookupSpan<'a>> Layer<S> for ReadJobs {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).unwrap();
        let mut fields = Fields::new();
        attrs.record(&mut Recorder(&mut fields));
        if attrs.metadata().name() != "read_job" {
            // An Intent's or an External change's span names the repository it is about.
            if let Some(path) = fields.remove("path") {
                span.extensions_mut().insert(Repository(path));
            }
            return;
        }
        // Which repository it read: other tests' services run in this binary too.
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
            closed().lock().unwrap().push(fields);
        }
    }
}

#[test]
fn a_file_saved_outside_the_service_traces_narrowed_reads_of_unstaged_and_untracked() {
    closed();
    let (_dir, repo) = small_fixture_clone();
    common::assert_filesystem_events_available(&repo);
    let harness = Harness::start();
    let opened = open(&harness, &repo);
    let ours = repo.display().to_string();

    std::fs::write(repo.join("README.md"), "saved outside Cairn\n").unwrap();
    // Shown in Unstaged: by then every status Slot has been asked again.
    let mut seen = Seen::default();
    seen.until(&harness, |seen| {
        unstaged(&seen.all).iter().any(|u| {
            u.generation > opened.unstaged
                && u.result
                    .as_ref()
                    .is_ok_and(|c| c.iter().any(|c| c.path.as_bytes() == b"README.md"))
        })
    });
    // A saved file can't change HEAD vs index, so Staged isn't asked again.
    for slot in ["unstaged", "untracked"] {
        // The saved file wakes both Slots. Each Slot's newest Snapshot so far may
        // still be the Open's, so wait for its span by name and Refresh, not generation.
        let after = match slot {
            "unstaged" => opened.unstaged,
            _ => opened.untracked,
        }
        .get();
        let span = wait_for_span(|f| {
            f.get("repository") == Some(&ours)
                && f.get("slot").map(String::as_str) == Some(slot)
                && f.get("refresh").map(String::as_str) == Some("worktree")
                && f.get("generation").and_then(|g| g.parse::<u64>().ok()) > Some(after)
        });
        assert!(
            span.contains_key("pending"),
            "whether it waited behind a running read: {span:?}"
        );
        // Only what the watcher saw is read again.
        assert_eq!(
            span.get("scope").map(String::as_str),
            Some("paths"),
            "{span:?}"
        );
        for duration in ["lane_wait_ms", "ms"] {
            let value: f64 = span
                .get(duration)
                .unwrap_or_else(|| panic!("no {duration}: {span:?}"))
                .parse()
                .unwrap();
            assert!(value >= 0.0, "{duration}: {span:?}");
        }
    }
}

/// The first closed `read_job` span `matches` accepts. A span closes once its Read Job has
/// published, so it can trail the Snapshot.
fn wait_for_span(matches: impl Fn(&Fields) -> bool) -> Fields {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Some(found) = closed().lock().unwrap().iter().find(|f| matches(f)) {
            return found.clone();
        }
        let spans = closed().lock().unwrap().clone();
        assert!(
            std::time::Instant::now() < deadline,
            "no such read_job span: {spans:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn an_open_traces_its_read_jobs_as_open_and_not_pending() {
    closed();
    let (_dir, repo) = small_fixture_clone();
    let harness = Harness::start();
    let opened = open(&harness, &repo);
    let repository = repo.display().to_string();

    // A Read Job's span closes once it has published, so it can trail the Snapshot.
    let spans = |slot: &str| {
        wait_for_span(|f| {
            f.get("repository") == Some(&repository)
                && f.get("slot").map(String::as_str) == Some(slot)
        });
        closed().lock().unwrap().clone()
    };
    for (slot, generation) in [
        ("refs", opened.refs),
        ("staged", opened.staged),
        ("unstaged", opened.unstaged),
        ("untracked", opened.untracked),
    ] {
        let spans = spans(slot);
        let span = spans
            .iter()
            .find(|f| {
                f.get("repository") == Some(&repository)
                    && f.get("slot").map(String::as_str) == Some(slot)
                    && f.get("generation") == Some(&generation.get().to_string())
            })
            .unwrap_or_else(|| panic!("{slot}: {spans:?}"));
        assert_eq!(
            span.get("refresh").map(String::as_str),
            Some("open"),
            "{span:?}"
        );
        assert_eq!(
            span.get("pending").map(String::as_str),
            Some("false"),
            "{span:?}"
        );
        if matches!(slot, "unstaged" | "untracked") {
            assert_eq!(
                span.get("scope").map(String::as_str),
                Some("full"),
                "an Open reads {slot} in full"
            );
        }
    }
}
