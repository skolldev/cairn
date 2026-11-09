//! Matching what Cairn painted to when the screen showed it, and reading PresentMon's CSV.
//!
//! The CSV's columns here come from a real PresentMon 2.6.0 trace, not its README, whose table
//! lists columns that this version's default output doesn't have. The rule for DWM's flips was
//! scored against real traces too: see `docs/budget-harness.md`.

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

/// What PresentMon 2.6.0 wrote with `--qpc_time`: the BOM, the header and the CRLF are real.
const HEADER: &str = "\u{feff}Application,ProcessID,SwapChainAddress,PresentRuntime,SyncInterval,PresentFlags,AllowsTearing,PresentMode,TimeInQPC,MsBetweenSimulationStart,MsBetweenPresents,MsBetweenDisplayChange,MsInPresentAPI,MsRenderPresentLatency,MsUntilDisplayed,CPUStartQPC,MsBetweenAppStart,MsCPUBusy,MsCPUWait,MsGPULatency,MsGPUTime,MsGPUBusy,MsGPUWait,MsAnimationError,AnimationTime,MsFlipDelay,MsAllInputToPhotonLatency,MsClickToPhotonLatency\r\n";

/// A row with made-up times. `present_ms` is when Present() was called. At 10 MHz a tick is 100 ns.
fn row(app: &str, pid: u32, chain: &str, present_ms: u64, between: &str, until: &str) -> String {
    let ticks = present_ms * 10_000;
    format!(
        "{app},{pid},{chain},DXGI,0,0,0,Composed: Flip,{ticks},NA,{between},NA,0.04,0.9,{until},{ticks},8.3,2.0,6.3,1.0,0.3,0.3,0.0,NA,NA,NA,NA,NA\r\n"
    )
}

fn cairn(pid: u32, present_ms: u64, between: &str, until: &str) -> String {
    row("cairn.exe", pid, "0x1", present_ms, between, until)
}

/// A frame of DWM's. DWM has a chain per display.
fn dwm(chain: &str, present_ms: u64, until: &str) -> String {
    row("dwm.exe", 9, chain, present_ms, "8.3", until)
}

fn trace_of(rows: &[String]) -> PresentMonTrace {
    PresentMonTrace::parse(&format!("{HEADER}{}", rows.concat()), 10_000_000).unwrap()
}

/// Two processes that PresentMon times itself, as it does an HWND swapchain.
fn csv_trace() -> PresentMonTrace {
    trace_of(&[
        cairn(40, 1_002, "8.3", "18.0000"),
        cairn(40, 1_013, "11.0", "NA"),
        cairn(40, 1_021, "8.0", "15.0000"),
        cairn(41, 3_005, "8.3", "20.0000"),
    ])
}

#[test]
fn presentmon_times_come_from_the_present_call_and_how_long_until_it_was_displayed() {
    let trace = csv_trace();
    // Present() at 1.002 s, and the screen 18 ms after that.
    assert_eq!(trace.of_process(40).first_presented(), Some(1_020 * MS));
    assert_eq!(trace.of_process(41).first_presented(), Some(3_025 * MS));
    assert_eq!(trace.of_process(99).first_presented(), None);
}

#[test]
fn a_frame_presentmon_cant_time_is_shown_by_dwms_next_flip_on_the_display() {
    // DirectComposition: every Cairn frame reads NA. DWM's flip presented at 1.000 s went before
    // this Present() at 1.005 s, so the frame is in the next one: 1.009 s, on screen 16 ms later.
    let trace = trace_of(&[
        dwm("0x0", 1_000, "16.0000"),
        cairn(40, 1_005, "NA", "NA"),
        dwm("0x0", 1_009, "16.0000"),
    ])
    .of_process(40);
    assert_eq!(trace.shown_at(1_004 * MS), Some(1_025 * MS));
    assert_eq!(trace.displayed(), [1_025 * MS]);
}

#[test]
fn a_frame_after_dwms_last_flip_has_no_time() {
    let trace = trace_of(&[dwm("0x0", 1_000, "16.0000"), cairn(40, 1_005, "NA", "NA")]);
    // Seen red: falling back to DWM's last flip read 1.016 s, a time before this Present().
    assert_eq!(trace.of_process(40).shown_at(1_004 * MS), None);
}

#[test]
fn presentmons_own_time_for_a_frame_beats_dwms_flip() {
    let trace = trace_of(&[
        cairn(40, 1_005, "NA", "30.0000"),
        dwm("0x0", 1_009, "16.0000"),
    ]);
    // Seen red: dropping PresentMon's own time read DWM's 1.025 s.
    assert_eq!(trace.of_process(40).shown_at(1_004 * MS), Some(1_035 * MS));
}

#[test]
fn dwms_flips_come_from_the_display_that_flips_soonest_after_cairns_presents() {
    // A display Cairn isn't on flips now and then. Which chain is Cairn's follows from the whole
    // session, whatever its address and wherever it comes in the file, so a short launch can't
    // pick another display by chance: process 41 alone would pick `other`.
    let rows = |window: &str, other: &str| {
        trace_of(&[
            dwm(other, 1_001, "1.0000"),
            cairn(40, 1_005, "NA", "NA"),
            dwm(window, 1_006, "16.0000"),
            cairn(40, 1_015, "NA", "NA"),
            dwm(window, 1_016, "16.0000"),
            dwm(other, 1_017, "1.0000"),
            cairn(40, 1_025, "NA", "NA"),
            dwm(window, 1_026, "16.0000"),
            cairn(41, 1_030, "NA", "NA"),
            dwm(other, 1_031, "1.0000"),
            dwm(window, 1_036, "16.0000"),
        ])
    };
    for (window, other) in [("0x0", "0x100000000"), ("0x100000000", "0x0")] {
        let trace = rows(window, other);
        assert_eq!(
            trace.of_process(40).shown_at(1_014 * MS),
            Some(1_032 * MS),
            "Cairn on {window}"
        );
        assert_eq!(
            trace.of_process(41).first_presented(),
            Some(1_052 * MS),
            "Cairn on {window}"
        );
    }
}

#[test]
fn a_process_first_present_has_no_row_and_comes_from_the_next_rows_gap() {
    // PresentMon writes no row for a swapchain's first Present(). The first row's Present() at
    // 1.055 s came 51 ms after it: at 1.004 s, so DWM's flip at 1.008 s holds the first frame.
    let trace = trace_of(&[
        dwm("0x0", 1_000, "16.0000"),
        dwm("0x0", 1_008, "16.0000"),
        cairn(40, 1_055, "51.0000", "21.0000"),
        dwm("0x0", 1_060, "16.0000"),
    ])
    .of_process(40);
    assert_eq!(trace.first_presented(), Some(1_024 * MS));
    assert_eq!(trace.shown_at(1_003 * MS), Some(1_024 * MS));
    assert_eq!(trace.shown_at(1_005 * MS), Some(1_076 * MS));
}

#[test]
fn a_presentmon_frame_is_the_first_present_after_the_paint_that_reached_the_screen() {
    let trace = csv_trace().of_process(40);
    // A paint at 1.001 s: Present() follows it at 1.002 s.
    assert_eq!(trace.shown_at(1_001 * MS), Some(1_020 * MS));
    // After that Present(): the next one, at 1.013 s, has "NA", so it never reached the screen.
    // The next that did is at 1.021 s, shown 15 ms later.
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
    let wrong = "Application,ProcessID,SwapChainAddress,MsBetweenPresents\ncairn.exe,40,0x1,8.3\n";
    let error = PresentMonTrace::parse(wrong, 10_000_000).unwrap_err();
    assert!(error.contains("TimeInQPC"), "{error}");
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
