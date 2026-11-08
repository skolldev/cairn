//! When the screen showed what Cairn painted: presented frames, never paint timers.
//!
//! All times are nanoseconds on [`crate::clock`], which every process on a machine shares.

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

/// One row of a PresentMon CSV.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Row {
    pid: u32,
    /// When the process called Present().
    present_ns: u64,
    /// When the frame reached the screen. `None` for a frame PresentMon reports as "NA".
    displayed_ns: Option<u64>,
}

/// PresentMon's trace, on Windows. Written with `--qpc_time`, so frames are on the same clock
/// as everything else.
#[derive(Clone, Debug, Default)]
pub struct PresentMonTrace {
    rows: Vec<Row>,
}

impl PresentMonTrace {
    /// Parses a CSV whose QPC ticks run at `ticks_per_second`. A CSV without the columns is an
    /// error, so that a format change can't read as a window that never presented.
    pub fn parse(csv: &str, ticks_per_second: u64) -> Result<Self, String> {
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
        let pid = column("ProcessID")?;
        let start = column("CPUStartQPC")?;
        let busy = column("MsCPUBusy")?;
        let latency = column("DisplayLatency")?;

        let ms_to_ns = |ms: f64| (ms * 1e6) as u64;
        let mut rows = Vec::new();
        for line in lines.filter(|line| !line.trim().is_empty()) {
            let fields: Vec<&str> = line.split(',').map(str::trim).collect();
            let field = |index: usize| {
                fields
                    .get(index)
                    .copied()
                    .ok_or_else(|| format!("a PresentMon row is too short: {line}"))
            };
            let number = |index: usize| {
                let text = field(index)?;
                text.parse::<f64>()
                    .map_err(|_| format!("not a number in a PresentMon row, {text:?}: {line}"))
            };
            let start_ticks: u64 = field(start)?
                .parse()
                .map_err(|_| format!("not a QPC value in a PresentMon row: {line}"))?;
            let start_ns = crate::clock::ticks_to_ns(start_ticks, ticks_per_second);
            rows.push(Row {
                pid: field(pid)?
                    .parse()
                    .map_err(|_| format!("not a process id in a PresentMon row: {line}"))?,
                present_ns: start_ns + ms_to_ns(number(busy)?),
                displayed_ns: match field(latency)? {
                    "NA" => None,
                    _ => Some(start_ns + ms_to_ns(number(latency)?)),
                },
            });
        }
        rows.sort_by_key(|row| row.present_ns);
        Ok(Self { rows })
    }

    /// When the screen showed this process's first frame.
    pub fn first_presented_of(&self, pid: u32) -> Option<u64> {
        self.rows
            .iter()
            .filter(|row| row.pid == pid)
            .filter_map(|row| row.displayed_ns)
            .min()
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
