//! Each Read Job is a `read_job` tracing span, which says where an External change's time
//! goes (#40): its Slot, the Refresh that started it, whether it waited behind a running read,
//! how long it waited for a lane worker and how long it ran. A subscriber is how a caller
//! sees spans, so these tests install one for the whole binary.

mod common;

use common::trace::{self, Fields};
use common::{Harness, Seen, git, open, small_fixture_clone, unstaged};

#[test]
fn a_file_saved_outside_the_service_traces_narrowed_reads_of_unstaged_and_untracked() {
    trace::install();
    let (_dir, repo) = small_fixture_clone();
    common::assert_filesystem_events_available(&repo);
    let harness = Harness::start();
    let opened = open(&harness, &repo);

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
            trace::about(f, &repo)
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
        // A save changes nothing in `.git`, so Unstaged compares against the index its last
        // read kept rather than loading it again, about 10 ms on the kernel (#78). Untracked
        // walks against a load of its own.
        let index = match slot {
            "unstaged" => "kept",
            _ => "loaded",
        };
        assert_eq!(
            span.get("index").map(String::as_str),
            Some(index),
            "{span:?}"
        );
    }
}

#[test]
fn an_index_write_traces_narrowed_reads_of_unstaged_and_untracked() {
    trace::install();
    let (_dir, repo) = small_fixture_clone();
    common::assert_filesystem_events_available(&repo);
    let harness = Harness::start();
    let opened = open(&harness, &repo);

    std::fs::write(repo.join("README.md"), "saved outside Cairn\n").unwrap();
    git(&repo, &["add", "--", "README.md"]);
    // Gone from Unstaged once the index write is read.
    let mut seen = Seen::default();
    seen.until(&harness, |seen| {
        unstaged(&seen.all).iter().any(|u| {
            u.generation > opened.unstaged
                && u.result
                    .as_ref()
                    .is_ok_and(|c| c.iter().all(|c| c.path.as_bytes() != b"README.md"))
        })
    });
    for (slot, scope) in [
        ("staged", None),
        ("unstaged", Some("paths")),
        ("untracked", Some("paths")),
    ] {
        let span = wait_for_span(|f| {
            trace::about(f, &repo)
                && f.get("slot").map(String::as_str) == Some(slot)
                && f.get("refresh").map(String::as_str) == Some("index")
        });
        // Only the entries the index write changed are read again. Staged has no scope: it
        // is read in full. Seen red: with an index write reading everything, Unstaged's is
        // `full`.
        assert_eq!(span.get("scope").map(String::as_str), scope, "{span:?}");
        // The control for a save's kept index: an index write loads it, to compare.
        if slot != "staged" {
            assert_eq!(
                span.get("index").map(String::as_str),
                Some("loaded"),
                "{span:?}"
            );
        }
    }
}

/// The first closed `read_job` span `matches` accepts. A span closes once its Read Job has
/// published, so it can trail the Snapshot.
fn wait_for_span(matches: impl Fn(&Fields) -> bool) -> Fields {
    trace::wait_for(trace::read_jobs, "read_job", matches).1
}

#[test]
fn an_open_traces_its_read_jobs_as_open_and_not_pending() {
    trace::install();
    let (_dir, repo) = small_fixture_clone();
    let harness = Harness::start();
    let opened = open(&harness, &repo);

    // A Read Job's span closes once it has published, so it can trail the Snapshot.
    let spans = |slot: &str| {
        wait_for_span(|f| {
            trace::about(f, &repo) && f.get("slot").map(String::as_str) == Some(slot)
        });
        trace::read_jobs()
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
                trace::about(f, &repo)
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

#[test]
fn an_opens_count_traces_inside_the_open_and_lets_the_refs_read_span_close() {
    trace::install();
    let (_dir, repo) = small_fixture_clone();
    let harness = Harness::start();
    let opened = open(&harness, &repo);

    // Every Refs read hands its branches to a count, even with none on screen. Its span sits
    // inside the Open's, so it names the repository. Seen red: `Counter::submit` taking the
    // Refs read's own `read_job` span, which then stayed open while an undrained Counts
    // Snapshot held it, and this span named no repository.
    let count = wait_for_span(|f| {
        trace::about(f, &repo) && f.get("slot").map(String::as_str) == Some("counts")
    });
    assert_eq!(
        count.get("refresh").map(String::as_str),
        Some("refs"),
        "{count:?}"
    );
    // The Counts Snapshot is still undrained here, and the Refs read's span closes anyway.
    let refs = wait_for_span(|f| {
        trace::about(f, &repo)
            && f.get("slot").map(String::as_str) == Some("refs")
            && f.get("generation") == Some(&opened.refs.get().to_string())
    });
    assert_eq!(
        refs.get("refresh").map(String::as_str),
        Some("open"),
        "{refs:?}"
    );
}
