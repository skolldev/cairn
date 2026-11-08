//! Matching what Cairn painted to when the screen showed it, and reading PresentMon's CSV.
//!
//! The CSV here follows the column table in PresentMon's console README (v2.x). It has not
//! come from a real trace: this was written on a Mac, so the first Windows run proves it.

use cairn_budget::presented::{
    Drawable, DrawableSource, MetalTrace, PresentMonTrace, PresentSource, largest_gap_ms,
};

fn drawable(id: u64, source: DrawableSource, acquired_ns: u64, presented: Option<u64>) -> Drawable {
    Drawable {
        id,
        source,
        acquired_ns,
        presented_ns: presented,
    }
}

const MS: u64 = 1_000_000;

#[test]
fn a_display_link_frame_is_shown_by_the_drawable_it_was_handed_before_painting() {
    // Drawable 1 is still on its way to the screen when drawable 2's frame paints, and is shown
    // later than that paint. Taking the first time after the paint would pick it: a frame early.
    let trace = MetalTrace::new(vec![
        drawable(1, DrawableSource::DisplayLink, 100 * MS, Some(120 * MS)),
        drawable(2, DrawableSource::DisplayLink, 108 * MS, Some(128 * MS)),
        drawable(3, DrawableSource::DisplayLink, 116 * MS, Some(136 * MS)),
    ]);
    assert_eq!(trace.shown_at(110 * MS), Some(128 * MS));
}

#[test]
fn a_synchronous_draw_is_shown_by_the_layer_drawable_taken_after_painting() {
    // With the display link detached, GPUI paints and only then takes `nextDrawable`.
    let trace = MetalTrace::new(vec![
        drawable(1, DrawableSource::DisplayLink, 100 * MS, Some(120 * MS)),
        drawable(2, DrawableSource::Layer, 201 * MS, Some(220 * MS)),
    ]);
    assert_eq!(trace.shown_at(200 * MS), Some(220 * MS));
}

#[test]
fn a_dropped_drawable_leaves_the_frame_to_the_next_one_presented() {
    let trace = MetalTrace::new(vec![
        drawable(1, DrawableSource::DisplayLink, 100 * MS, None),
        drawable(2, DrawableSource::DisplayLink, 108 * MS, Some(128 * MS)),
    ]);
    assert_eq!(trace.shown_at(101 * MS), Some(128 * MS));
}

#[test]
fn a_frame_with_nothing_presented_after_it_has_no_time() {
    let trace = MetalTrace::new(vec![drawable(
        1,
        DrawableSource::DisplayLink,
        100 * MS,
        None,
    )]);
    assert_eq!(trace.shown_at(101 * MS), None);
}

#[test]
fn metal_first_presented_is_the_earliest_screen_time() {
    let trace = MetalTrace::new(vec![
        drawable(1, DrawableSource::DisplayLink, 100 * MS, None),
        drawable(2, DrawableSource::DisplayLink, 108 * MS, Some(128 * MS)),
    ]);
    assert_eq!(trace.first_presented(), Some(128 * MS));
    assert_eq!(trace.displayed(), [128 * MS]);
}

/// Two processes, at 10 MHz: a tick is 100 ns. Row order is PresentMon's.
const CSV: &str = "\
Application,ProcessID,SwapChainAddress,CPUStartQPC,MsCPUBusy,MsCPUWait,DisplayLatency,DisplayedTime
cairn.exe,40,0x1,10000000,2.0,1.0,20.0,8.3
cairn.exe,40,0x1,10100000,3.0,1.0,NA,NA
cairn.exe,40,0x1,10200000,1.0,1.0,16.0,8.3
cairn.exe,41,0x2,30000000,5.0,1.0,25.0,8.3
";

fn csv_trace() -> PresentMonTrace {
    PresentMonTrace::parse(CSV, 10_000_000).unwrap()
}

#[test]
fn presentmon_times_come_from_cpu_start_busy_and_display_latency() {
    let trace = csv_trace();
    // 10000000 ticks is 1 s. Present() is at 1 s + 2 ms, the screen at 1 s + 20 ms.
    assert_eq!(trace.first_presented_of(40), Some(1_020 * MS));
    assert_eq!(trace.first_presented_of(41), Some(3_025 * MS));
    assert_eq!(trace.first_presented_of(99), None);
}

#[test]
fn a_presentmon_frame_is_the_first_present_after_the_paint_that_reached_the_screen() {
    let trace = csv_trace().of_process(40);
    // A paint at 1.001 s: Present() follows it at 1.002 s.
    assert_eq!(trace.shown_at(1_001 * MS), Some(1_020 * MS));
    // After that Present(): the next one, at 1.013 s, has "NA", so it never reached the screen.
    // The next that did starts at 1.02 s, shown 16 ms later.
    assert_eq!(trace.shown_at(1_003 * MS), Some(1_036 * MS));
    assert_eq!(trace.shown_at(5_000 * MS), None);
    assert_eq!(
        trace.displayed(),
        [1_020 * MS, 1_036 * MS],
        "the NA frame never showed"
    );
}

#[test]
fn a_presentmon_csv_without_the_columns_is_an_error_rather_than_no_frames() {
    let wrong = "Application,ProcessID,MsBetweenPresents\ncairn.exe,40,8.3\n";
    let error = PresentMonTrace::parse(wrong, 10_000_000).unwrap_err();
    assert!(error.contains("CPUStartQPC"), "{error}");
}

#[test]
fn the_largest_gap_counts_the_stretch_to_each_end_of_the_window() {
    // Frames stop 10 ms in: a freeze lasting to the end of the window is the gap that matters,
    // and looking only between frames would read 5 ms.
    assert_eq!(largest_gap_ms(&[5 * MS, 10 * MS], 0, 1_000 * MS), 990.0,);
    // Sorted first, and only frames inside the window count.
    assert_eq!(
        largest_gap_ms(&[900 * MS, 50 * MS, 2_000 * MS], 0, 1_000 * MS),
        850.0,
    );
    assert_eq!(largest_gap_ms(&[], 0, 1_000 * MS), 1_000.0);
}
