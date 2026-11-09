//! When the screen showed what Cairn painted: presented frames, never paint timers.
//!
//! All times are nanoseconds on [`crate::clock`], which every process on a machine shares.

use std::collections::{BTreeMap, HashSet};

/// A record of a process's presented frames.
pub trait PresentSource {
    /// When the screen first showed a frame whose paint ended at `paint_ns`.
    fn shown_at(&self, paint_ns: u64) -> Option<u64>;
    /// When the screen showed the process's first frame.
    fn first_presented(&self) -> Option<u64>;
    /// When the screen showed each frame that reached it, in order.
    fn displayed(&self) -> Vec<u64>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawableSource {
    /// `CAMetalDisplayLinkUpdate.drawable`: GPUI takes it, then paints the frame that uses it.
    DisplayLink,
    /// `CAMetalLayer.nextDrawable`: for a synchronous draw, GPUI paints and then takes it.
    Layer,
}

/// One Metal drawable. `presented_ns` is `None` until Metal reports it, and stays `None` if it
/// was dropped or never used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Drawable {
    pub id: u64,
    pub source: DrawableSource,
    pub acquired_ns: u64,
    pub presented_ns: Option<u64>,
}

/// Metal's presented times, on macOS.
#[derive(Clone, Debug, Default)]
pub struct MetalTrace {
    /// In the order they were acquired.
    drawables: Vec<Drawable>,
}

impl MetalTrace {
    pub fn new(mut drawables: Vec<Drawable>) -> Self {
        drawables.sort_by_key(|d| d.acquired_ns);
        Self { drawables }
    }
}

impl PresentSource for MetalTrace {
    /// A frame can't be matched by time alone: the previous frame's drawable is often still on
    /// its way to the screen when this one paints, so it would be read a frame early. Which
    /// drawable holds the frame follows from the order GPUI does things in.
    fn shown_at(&self, paint_ns: u64) -> Option<u64> {
        let next = self
            .drawables
            .partition_point(|d| d.acquired_ns <= paint_ns);
        let takes_next = self
            .drawables
            .get(next)
            .is_some_and(|d| d.source == DrawableSource::Layer);
        let own = match next
            .checked_sub(1)
            .map(|last| (last, &self.drawables[last]))
        {
            Some((last, d)) if !takes_next && d.source == DrawableSource::DisplayLink => last,
            _ => next,
        };
        // A dropped drawable's frame reaches the screen in the next one that does.
        self.drawables[own..].iter().find_map(|d| d.presented_ns)
    }

    fn first_presented(&self) -> Option<u64> {
        self.drawables.iter().filter_map(|d| d.presented_ns).min()
    }

    fn displayed(&self) -> Vec<u64> {
        let mut displayed: Vec<u64> = self
            .drawables
            .iter()
            .filter_map(|d| d.presented_ns)
            .collect();
        displayed.sort_unstable();
        displayed
    }
}

/// One Present() of a process's in a PresentMon CSV.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Row {
    pid: u32,
    /// When the process called Present().
    present_ns: u64,
    /// When the frame reached the screen. `None` if nothing in the trace shows it did.
    displayed_ns: Option<u64>,
}

/// One frame of DWM's that reached the screen.
#[derive(Clone, Copy, Debug)]
struct Flip {
    present_ns: u64,
    screen_ns: u64,
}

/// The process that composes the desktop.
pub const DWM: &str = "dwm.exe";

/// The first of `flips`, sorted by time, that holds a frame presented at `present_ns`.
fn flip_after(flips: &[Flip], present_ns: u64) -> Option<&Flip> {
    flips.get(flips.partition_point(|flip| flip.present_ns < present_ns))
}

/// DWM's flips on the display `rows` are on: the chain that flips soonest after them. DWM has one
/// chain per display, and the others flip seconds late or not at all when they're idle.
fn flips_of_display<'a>(chains: &'a BTreeMap<&str, Vec<Flip>>, rows: &[Row]) -> Option<&'a [Flip]> {
    let median_wait = |flips: &[Flip]| {
        let mut waits: Vec<u64> = rows
            .iter()
            .map(|row| {
                flip_after(flips, row.present_ns)
                    .map_or(u64::MAX, |flip| flip.present_ns - row.present_ns)
            })
            .collect();
        waits.sort_unstable();
        waits.get(waits.len() / 2).copied().unwrap_or(u64::MAX)
    };
    chains
        .values()
        .min_by_key(|flips| median_wait(flips))
        .map(Vec::as_slice)
}

/// PresentMon's trace of Cairn and DWM, on Windows. Written with `--qpc_time`, so frames are on
/// the same clock as everything else.
///
/// PresentMon reports no frame of a DirectComposition swapchain, which is what GPUI draws through,
/// as displayed. Those frames reach the screen in DWM's next flip on their display, so that's the
/// time they get. PresentMon's own time wins where it has one.
#[derive(Clone, Debug, Default)]
pub struct PresentMonTrace {
    rows: Vec<Row>,
}

impl PresentMonTrace {
    /// Parses a CSV whose QPC ticks run at `ticks_per_second`. A CSV without the columns is an
    /// error, so that a format change can't read as a window that never presented.
    pub fn parse(csv: &str, ticks_per_second: u64) -> Result<Self, String> {
        let csv = csv.strip_prefix('\u{feff}').unwrap_or(csv);
        let mut lines = csv.lines();
        let header: Vec<&str> = lines
            .next()
            .unwrap_or("")
            .split(',')
            .map(str::trim)
            .collect();
        let column = |name: &str| {
            header
                .iter()
                .position(|h| *h == name)
                .ok_or_else(|| format!("the PresentMon CSV has no {name} column: {header:?}"))
        };
        let application = column("Application")?;
        let pid = column("ProcessID")?;
        let chain = column("SwapChainAddress")?;
        let present = column("TimeInQPC")?;
        let between_presents = column("MsBetweenPresents")?;
        let until_displayed = column("MsUntilDisplayed")?;

        let mut chains: BTreeMap<&str, Vec<Flip>> = BTreeMap::new();
        let mut timed = Vec::new();
        for line in lines.filter(|line| !line.trim().is_empty()) {
            let fields: Vec<&str> = line.split(',').map(str::trim).collect();
            let field = |index: usize| {
                fields
                    .get(index)
                    .copied()
                    .ok_or_else(|| format!("a PresentMon row is too short: {line}"))
            };
            // Milliseconds as nanoseconds, or `None` where PresentMon has "NA".
            let ms_as_ns = |index: usize| match field(index)? {
                "NA" => Ok(None),
                text => text
                    .parse::<f64>()
                    .map(|ms| Some((ms * 1e6) as u64))
                    .map_err(|_| format!("not a number in a PresentMon row, {text:?}: {line}")),
            };
            let present_ticks: u64 = field(present)?
                .parse()
                .map_err(|_| format!("not a QPC value in a PresentMon row: {line}"))?;
            let present_ns = crate::clock::ticks_to_ns(present_ticks, ticks_per_second);
            let displayed_ns = ms_as_ns(until_displayed)?.map(|until| present_ns + until);

            if field(application)?.eq_ignore_ascii_case(DWM) {
                if let Some(screen_ns) = displayed_ns {
                    chains.entry(field(chain)?).or_default().push(Flip {
                        present_ns,
                        screen_ns,
                    });
                }
                continue;
            }
            let row = Row {
                pid: field(pid)?
                    .parse()
                    .map_err(|_| format!("not a process id in a PresentMon row: {line}"))?,
                present_ns,
                displayed_ns,
            };
            timed.push((row, ms_as_ns(between_presents)?));
        }
        timed.sort_by_key(|(row, _)| row.present_ns);

        // PresentMon writes no row for a swapchain's first Present(), but the next row says how
        // long before it that was.
        let mut seen = HashSet::new();
        let mut rows = Vec::with_capacity(timed.len());
        for (row, between_ns) in timed {
            if seen.insert(row.pid)
                && let Some(between_ns) = between_ns
            {
                rows.push(Row {
                    present_ns: row.present_ns.saturating_sub(between_ns),
                    displayed_ns: None,
                    ..row
                });
            }
            rows.push(row);
        }
        rows.sort_by_key(|row| row.present_ns);

        for chain in chains.values_mut() {
            chain.sort_by_key(|flip| flip.present_ns);
        }
        if let Some(flips) = flips_of_display(&chains, &rows) {
            for row in &mut rows {
                row.displayed_ns = row
                    .displayed_ns
                    .or_else(|| flip_after(flips, row.present_ns).map(|flip| flip.screen_ns));
            }
        }
        Ok(Self { rows })
    }

    /// The same trace, narrowed to one process.
    pub fn of_process(&self, pid: u32) -> Self {
        Self {
            rows: self.rows.iter().filter(|r| r.pid == pid).copied().collect(),
        }
    }
}

impl PresentSource for PresentMonTrace {
    /// Present() follows the paint on the same thread, so the frame is in the first Present()
    /// after the paint. If that one never reached the screen, the next that does shows it.
    fn shown_at(&self, paint_ns: u64) -> Option<u64> {
        let first = self.rows.partition_point(|r| r.present_ns < paint_ns);
        self.rows[first..].iter().find_map(|r| r.displayed_ns)
    }

    fn first_presented(&self) -> Option<u64> {
        self.rows.iter().filter_map(|r| r.displayed_ns).min()
    }

    fn displayed(&self) -> Vec<u64> {
        let mut displayed: Vec<u64> = self.rows.iter().filter_map(|r| r.displayed_ns).collect();
        displayed.sort_unstable();
        displayed
    }
}

/// The longest the screen went without a new frame inside the window, in milliseconds. Both
/// ends of the window count: frames that stop partway are a freeze until the window ends.
pub fn largest_gap_ms(displayed_ns: &[u64], window_start_ns: u64, window_end_ns: u64) -> f64 {
    let mut inside: Vec<u64> = displayed_ns
        .iter()
        .copied()
        .filter(|t| (window_start_ns..=window_end_ns).contains(t))
        .collect();
    inside.sort_unstable();
    let mut last = window_start_ns;
    let mut largest = 0;
    for time in inside.into_iter().chain([window_end_ns]) {
        largest = largest.max(time - last);
        last = time;
    }
    largest as f64 / 1e6
}
