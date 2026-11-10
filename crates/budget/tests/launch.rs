//! `trace --only launch` explains where a warm Launch's time goes on macOS: each phase in ms from
//! the clock start, and how many frames painted before the first one Metal gave a time to.

use cairn_budget::launch::{Phase, breakdown};
use cairn_budget::presented::DrawableSource;
use cairn_budget::probe::{Frame, Line, Mark, Slot, Trace};

const START: u64 = 1_000_000_000;

/// `ms` after the clock start, in ns.
fn at(ms: u64) -> u64 {
    START + ms * 1_000_000
}

fn frame(ms: u64, answered: u64) -> Line {
    Line::Frame(Frame {
        paint_ns: at(ms),
        refs: answered,
        staged: answered,
        unstaged: answered,
        untracked: answered,
        ..Frame::default()
    })
}

fn drawable(id: u64, source: DrawableSource, ms: u64) -> Line {
    Line::Drawable {
        id,
        source,
        ns: at(ms),
    }
}

fn mark(mark: Mark, ms: u64) -> Line {
    Line::Mark { mark, ns: at(ms) }
}

fn trace(lines: impl IntoIterator<Item = Line>) -> Trace {
    let mut trace = Trace::default();
    for line in lines {
        trace.record(line);
    }
    trace
}

#[test]
fn a_launch_is_broken_into_phases_up_to_the_first_frame_metal_gave_a_time_to() {
    let launch = trace([
        mark(Mark::Main, 40),
        mark(Mark::Launched, 150),
        mark(Mark::Window, 200),
        mark(Mark::Service, 210),
        frame(215, 0),
        mark(Mark::Published(Slot::Refs), 220),
        frame(225, 0),
        drawable(1, DrawableSource::Layer, 226),
        Line::Dropped { id: 1 },
        mark(Mark::Drained(Slot::Refs), 240),
        mark(Mark::Published(Slot::Untracked), 250),
        mark(Mark::Drained(Slot::Untracked), 255),
        drawable(2, DrawableSource::DisplayLink, 258),
        frame(260, 1),
        drawable(3, DrawableSource::DisplayLink, 266),
        frame(268, 1),
        drawable(4, DrawableSource::DisplayLink, 274),
        frame(276, 1),
        Line::Dropped { id: 2 },
        Line::Presented { id: 3, ns: at(300) },
        Line::Presented { id: 4, ns: at(308) },
    ]);

    let breakdown = breakdown(START, &launch).unwrap();

    for (phase, ms) in [
        (Phase::Main, Some(40.0)),
        (Phase::Launched, Some(150.0)),
        (Phase::Window, Some(200.0)),
        (Phase::Service, Some(210.0)),
        (Phase::FirstPaint, Some(215.0)),
        (Phase::Published(Slot::Refs), Some(220.0)),
        (Phase::Drained(Slot::Refs), Some(240.0)),
        (Phase::Published(Slot::Untracked), Some(250.0)),
        (Phase::Drained(Slot::Untracked), Some(255.0)),
        // This launch never said so.
        (Phase::Published(Slot::Staged), None),
        (Phase::EverySlotPainted, Some(260.0)),
        // Drawable 3, from the display link, is the first with a time: the frame painted next
        // after it was taken holds it.
        (Phase::TimedFramePainted, Some(268.0)),
        (Phase::FirstPresented, Some(300.0)),
    ] {
        assert_eq!(breakdown.at(phase), ms, "{phase:?}");
    }
    assert_eq!(breakdown.frames_before_timed, 3);
    assert_eq!(breakdown.drawables_without_time, 2);
}

#[test]
fn when_the_first_timed_drawable_is_the_layers_the_frame_painted_just_before_holds_it() {
    let launch = trace([
        frame(215, 0),
        frame(230, 1),
        drawable(1, DrawableSource::Layer, 231),
        frame(240, 1),
        drawable(2, DrawableSource::DisplayLink, 239),
        Line::Presented { id: 1, ns: at(250) },
        Line::Presented { id: 2, ns: at(258) },
    ]);

    let breakdown = breakdown(START, &launch).unwrap();

    // Seen red: with a layer's drawable read as a display link's, the frame painted after it.
    assert_eq!(breakdown.at(Phase::TimedFramePainted), Some(230.0));
    assert_eq!(breakdown.at(Phase::FirstPresented), Some(250.0));
    assert_eq!(breakdown.frames_before_timed, 1);
    assert_eq!(breakdown.drawables_without_time, 0);
    // No marks: a build from before them, or a launch that quit first.
    assert_eq!(breakdown.at(Phase::Main), None);
}

#[test]
fn a_launch_metal_never_timed_has_no_breakdown() {
    let launch = trace([
        frame(215, 0),
        drawable(1, DrawableSource::Layer, 216),
        Line::Dropped { id: 1 },
    ]);
    assert!(breakdown(START, &launch).is_err());
}
