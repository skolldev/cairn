//! The lines Cairn's `measure` build writes, and what makes a frame the one a sample waits for.

use cairn_budget::presented::{Drawable, DrawableSource};
use cairn_budget::probe::{Expect, Frame, Line, Trace};

fn frame(paint_ns: u64) -> Frame {
    Frame {
        paint_ns,
        active: true,
        head: Some("budget-a".to_owned()),
        refs: 1,
        staged: 1,
        unstaged: 1,
        untracked: 1,
        unstaged_paths: vec!["Makefile".to_owned()],
        untracked_paths: vec!["budget.keep".to_owned(), "a file.txt".to_owned()],
    }
}

#[test]
fn every_line_survives_the_pipe() {
    let lines = [
        Line::Hello {
            release: true,
            pid: 41,
        },
        Line::Frame(frame(77)),
        Line::Frame(Frame {
            active: false,
            head: None,
            unstaged_paths: vec![],
            untracked_paths: vec![],
            ..frame(78)
        }),
        Line::Intent {
            ns: 5,
            branch: "budget-b".to_owned(),
        },
        Line::Drawable {
            id: 9,
            source: DrawableSource::Layer,
            ns: 6,
        },
        Line::Presented { id: 9, ns: 7 },
        Line::Dropped { id: 10 },
        Line::Key { ns: 8, held: true },
    ];
    for line in lines {
        assert_eq!(Line::parse(&line.to_string()), Some(line.clone()), "{line}");
    }
}

#[test]
fn lines_that_are_not_the_probes_are_ignored() {
    assert_eq!(Line::parse("2026-11-08 INFO intent{kind=\"open\"}"), None);
    assert_eq!(Line::parse(""), None);
}

#[test]
fn a_save_is_shown_once_a_newer_unstaged_lists_the_file() {
    let baseline = Frame {
        unstaged_paths: vec![],
        ..frame(10)
    };
    let saved = Expect::Unstaged {
        path: "Makefile".to_owned(),
        listed: true,
    };
    // Older than the baseline's answer, though the state matches: an answer from before the save.
    assert!(!saved.met_by(&frame(11), &baseline));
    let answered = Frame {
        unstaged: 2,
        ..frame(12)
    };
    assert!(saved.met_by(&answered, &baseline));
    // A newer answer that doesn't list the file is a read that came too early.
    let early = Frame {
        unstaged: 2,
        unstaged_paths: vec![],
        ..frame(13)
    };
    assert!(!saved.met_by(&early, &baseline));

    let restored = Expect::Unstaged {
        path: "Makefile".to_owned(),
        listed: false,
    };
    assert!(restored.met_by(&early, &frame(10)));
}

#[test]
fn a_switch_is_shown_once_head_status_and_the_untracked_flip_all_answer() {
    let baseline = frame(10);
    let switched = Expect::Switched {
        branch: "budget-b".to_owned(),
        untracked: "budget.keep".to_owned(),
        listed: false,
    };
    let done = Frame {
        head: Some("budget-b".to_owned()),
        refs: 2,
        staged: 2,
        unstaged: 2,
        untracked: 2,
        untracked_paths: vec![],
        ..frame(20)
    };
    assert!(switched.met_by(&done, &baseline));
    for (not_yet, why) in [
        (
            Frame {
                head: Some("budget-a".to_owned()),
                ..done.clone()
            },
            "HEAD is still the old branch",
        ),
        (
            Frame {
                staged: 1,
                ..done.clone()
            },
            "Staged hasn't answered",
        ),
        (
            Frame {
                unstaged: 1,
                ..done.clone()
            },
            "Unstaged hasn't answered",
        ),
        (
            Frame {
                untracked: 1,
                ..done.clone()
            },
            "Untracked hasn't answered",
        ),
        (
            Frame {
                untracked_paths: vec!["budget.keep".to_owned()],
                ..done.clone()
            },
            "Untracked hasn't flipped",
        ),
    ] {
        assert!(!switched.met_by(&not_yet, &baseline), "{why}");
    }
}

#[test]
fn a_trace_pairs_drawables_with_their_presented_times_in_any_arrival_order() {
    let mut trace = Trace::default();
    for line in [
        Line::Drawable {
            id: 1,
            source: DrawableSource::DisplayLink,
            ns: 100,
        },
        Line::Drawable {
            id: 2,
            source: DrawableSource::DisplayLink,
            ns: 200,
        },
        // Metal reports on its own thread, in no promised order.
        Line::Presented { id: 2, ns: 220 },
        Line::Dropped { id: 1 },
        Line::Frame(frame(150)),
    ] {
        trace.record(line);
    }
    assert_eq!(
        trace.drawables(),
        vec![
            Drawable {
                id: 1,
                source: DrawableSource::DisplayLink,
                acquired_ns: 100,
                presented_ns: None,
            },
            Drawable {
                id: 2,
                source: DrawableSource::DisplayLink,
                acquired_ns: 200,
                presented_ns: Some(220),
            },
        ]
    );
    assert_eq!(trace.frames().len(), 1);
}

/// GPUI throttles an inactive window to 30 Hz, and macOS stops presenting a covered one, so a
/// sample that overlaps one measures the window manager.
#[test]
fn a_trace_knows_whether_the_window_lost_focus_during_a_stretch() {
    let mut trace = Trace::default();
    for (paint_ns, active) in [(10, true), (20, false), (30, true)] {
        trace.record(Line::Frame(Frame {
            active,
            ..frame(paint_ns)
        }));
    }
    assert!(trace.inactive_during(10, 30));
    assert!(trace.inactive_during(20, 20), "both ends count");
    assert!(!trace.inactive_during(21, 30), "after it");
    assert!(!trace.inactive_during(0, 19), "before it");
}
