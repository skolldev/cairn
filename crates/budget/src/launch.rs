//! Where a warm Launch's time goes on macOS (#83): each phase from the clock start, which starts
//! just before the process does, to the first frame Metal gave a presented time to, which is
//! what `run` times.
//!
//! It explains a number and never gives one: the Launch Budget is `run`'s, by `stats::LAUNCH`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id};
use tracing::{Metadata, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

use crate::clock;
use crate::fixture::Sandbox;
use crate::presented::DrawableSource;
use crate::probe::{Line, Mark, Slot, Trace};
use crate::run::{self, Config};
use crate::stats;

/// Said before any breakdown, and in `docs/budget-harness.md`.
pub const NOT_A_NUMBER: &str = "This explains where a warm Launch's time goes, and gives no Budget \
     number: it writes no result file and gives no verdict. `scripts/budget.sh run --only launch` \
     measures the Budget.";

/// A point in a launch, in the order a breakdown lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    /// `main` is running.
    Main,
    /// GPUI's app has finished launching.
    Launched,
    /// The window exists.
    Window,
    /// The Repo service has started, and the Open has been sent.
    Service,
    /// The first frame painted, as the probe stamps it.
    FirstPaint,
    /// The Repo service published this Slot's first Snapshot.
    Published(Slot),
    /// The window drained this Slot's first Snapshot.
    Drained(Slot),
    /// The first frame painted with an answer from every Slot.
    EverySlotPainted,
    /// The frame that holds the first drawable Metal gave a presented time to.
    TimedFramePainted,
    /// When that drawable reached the screen: what `run` times.
    FirstPresented,
}

impl Phase {
    fn describe(self) -> String {
        match self {
            Phase::Main => "main running".to_owned(),
            Phase::Launched => "GPUI app launched".to_owned(),
            Phase::Window => "window created".to_owned(),
            Phase::Service => "Repo service started, Open sent".to_owned(),
            Phase::FirstPaint => "first painted frame".to_owned(),
            Phase::Published(slot) => format!("{} first published", slot.name()),
            Phase::Drained(slot) => format!("{} first drained", slot.name()),
            Phase::EverySlotPainted => "first frame painted with every Slot".to_owned(),
            Phase::TimedFramePainted => "timed frame painted".to_owned(),
            Phase::FirstPresented => "first presented time (what run times)".to_owned(),
        }
    }

    pub const ALL: [Phase; 16] = [
        Phase::Main,
        Phase::Launched,
        Phase::Window,
        Phase::Service,
        Phase::FirstPaint,
        Phase::Published(Slot::Refs),
        Phase::Published(Slot::Staged),
        Phase::Published(Slot::Unstaged),
        Phase::Published(Slot::Untracked),
        Phase::Drained(Slot::Refs),
        Phase::Drained(Slot::Staged),
        Phase::Drained(Slot::Unstaged),
        Phase::Drained(Slot::Untracked),
        Phase::EverySlotPainted,
        Phase::TimedFramePainted,
        Phase::FirstPresented,
    ];
}

/// One launch, in ms from its clock start.
#[derive(Clone, Debug, PartialEq)]
pub struct Breakdown {
    phases: BTreeMap<Phase, f64>,
    /// Frames painted before the timed frame, the first painted frame among them.
    pub frames_before_timed: usize,
    /// Drawables taken before the timed frame's, which Metal gave no presented time to.
    pub drawables_without_time: usize,
}

impl Breakdown {
    /// When the launch reached `phase`, or `None` if it never said.
    pub fn at(&self, phase: Phase) -> Option<f64> {
        self.phases.get(&phase).copied()
    }
}

/// Breaks one launch down, from its clock start and everything its probe said.
pub fn breakdown(start_ns: u64, trace: &Trace) -> Result<Breakdown, String> {
    let ms = |ns: u64| (ns as f64 - start_ns as f64) / 1e6;
    let mut phases = BTreeMap::new();
    for (phase, mark) in [
        (Phase::Main, Mark::Main),
        (Phase::Launched, Mark::Launched),
        (Phase::Window, Mark::Window),
        (Phase::Service, Mark::Service),
    ]
    .into_iter()
    .chain(Slot::ALL.into_iter().flat_map(|s| {
        [
            (Phase::Published(s), Mark::Published(s)),
            (Phase::Drained(s), Mark::Drained(s)),
        ]
    })) {
        if let Some(ns) = trace.mark(mark) {
            phases.insert(phase, ms(ns));
        }
    }

    let frames = trace.frames();
    if let Some(first) = frames.first() {
        phases.insert(Phase::FirstPaint, ms(first.paint_ns));
    }
    if let Some(every) = frames
        .iter()
        .find(|f| f.refs > 0 && f.staged > 0 && f.unstaged > 0 && f.untracked > 0)
    {
        phases.insert(Phase::EverySlotPainted, ms(every.paint_ns));
    }

    let mut drawables = trace.drawables();
    drawables.sort_by_key(|d| d.acquired_ns);
    let (timed, presented_ns) = drawables
        .iter()
        .filter_map(|d| Some((d, d.presented_ns?)))
        .min_by_key(|(_, presented)| *presented)
        .ok_or("Metal gave no drawable of this launch a presented time")?;
    // GPUI takes a display link's drawable and then paints into it; for a synchronous draw it
    // paints and then takes one from the layer.
    let frame = match timed.source {
        DrawableSource::DisplayLink => frames.iter().position(|f| f.paint_ns >= timed.acquired_ns),
        DrawableSource::Layer => frames.iter().rposition(|f| f.paint_ns <= timed.acquired_ns),
    }
    .ok_or("no painted frame holds the first drawable with a presented time")?;
    phases.insert(Phase::TimedFramePainted, ms(frames[frame].paint_ns));
    phases.insert(Phase::FirstPresented, ms(presented_ns));

    Ok(Breakdown {
        phases,
        frames_before_timed: frame,
        drawables_without_time: drawables
            .iter()
            .filter(|d| d.acquired_ns < timed.acquired_ns && d.presented_ns.is_none())
            .count(),
    })
}

/// Marks each Slot's first published Snapshot, from the Repo service's own spans: a `publish`
/// inside a `read_job` that names its Slot. Install it through [`FirstPublishes::layer`].
pub struct FirstPublishes {
    emit: Box<dyn Fn(Line) + Send + Sync>,
    marked: Mutex<BTreeSet<Slot>>,
}

impl FirstPublishes {
    /// The layer, filtered to its own two spans. The filter is the layer's own, so the other
    /// layers still see everything they asked for: a layer that answered no for every other
    /// callsite would silence them all.
    pub fn layer<S: Subscriber + for<'a> LookupSpan<'a>>(
        emit: impl Fn(Line) + Send + Sync + 'static,
    ) -> impl Layer<S> {
        Self {
            emit: Box::new(emit),
            marked: Mutex::default(),
        }
        .with_filter(filter_fn(watched))
    }
}

fn watched(metadata: &Metadata<'_>) -> bool {
    metadata.is_span() && matches!(metadata.name(), "read_job" | "publish")
}

/// The Slot a `read_job` span names.
struct SlotField(Option<Slot>);

impl Visit for SlotField {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "slot" {
            self.0 = Slot::from_name(format!("{value:?}").trim_matches('"'));
        }
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "slot" {
            self.0 = Slot::from_name(value);
        }
    }
}

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for FirstPublishes {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let ns = clock::now_ns();
        let Some(span) = ctx.span(id) else { return };
        match attrs.metadata().name() {
            "read_job" => {
                let mut slot = SlotField(None);
                attrs.record(&mut slot);
                if let Some(slot) = slot.0 {
                    span.extensions_mut().insert(slot);
                }
            }
            "publish" => {
                let Some(slot) = span
                    .parent()
                    .and_then(|read| read.extensions().get::<Slot>().copied())
                else {
                    return;
                };
                if self.marked.lock().unwrap().insert(slot) {
                    (self.emit)(Line::Mark {
                        mark: Mark::Published(slot),
                        ns,
                    });
                }
            }
            _ => {}
        }
    }
}

/// Every launch's breakdown, then each phase's median, min and max over the launches that
/// reached it, in ms from the clock start.
pub fn render(breakdowns: &[Breakdown]) -> String {
    let mut text = format!(
        "# Launch, traced\n\n{NOT_A_NUMBER}\n\n{} warm launches, in ms from just before the \
         process starts.\n\n| Phase | Median | Min | Max |\n| --- | --- | --- | --- |\n",
        breakdowns.len()
    );
    let row = |name: &str, values: Vec<f64>, digits: usize| {
        if values.is_empty() {
            return format!("| {name} | - | - | - |\n");
        }
        let min = values.iter().copied().fold(f64::INFINITY, f64::min);
        let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        format!(
            "| {name} | {:.digits$} | {min:.digits$} | {max:.digits$} |\n",
            stats::median(&values)
        )
    };
    for phase in Phase::ALL {
        let values = breakdowns.iter().filter_map(|b| b.at(phase)).collect();
        text.push_str(&row(&phase.describe(), values, 1));
    }
    let counts = |count: fn(&Breakdown) -> usize| {
        breakdowns
            .iter()
            .map(|b| count(b) as f64)
            .collect::<Vec<_>>()
    };
    text.push_str(&row(
        "frames painted before the timed frame",
        counts(|b| b.frames_before_timed),
        0,
    ));
    text.push_str(&row(
        "drawables with no presented time before it",
        counts(|b| b.drawables_without_time),
        0,
    ));

    text.push_str("\n## Each launch\n\n");
    let header: Vec<String> = Phase::ALL.iter().map(|p| p.describe()).collect();
    text.push_str(&format!(
        "| # | {} | frames before | untimed drawables |\n|{}\n",
        header.join(" | "),
        " --- |".repeat(header.len() + 3)
    ));
    for (number, breakdown) in breakdowns.iter().enumerate() {
        let cells: Vec<String> = Phase::ALL
            .iter()
            .map(|p| {
                breakdown
                    .at(*p)
                    .map_or("-".to_owned(), |ms| format!("{ms:.1}"))
            })
            .collect();
        text.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            number + 1,
            cells.join(" | "),
            breakdown.frames_before_timed,
            breakdown.drawables_without_time
        ));
    }
    text
}

/// `config.samples` warm launches on the small fixture, as `run` makes them, broken down.
pub fn trace_launch(config: &Config) -> Result<String, String> {
    let sandbox = Sandbox::clone_of(&run::launch_fixture(config)?)?;
    let breakdowns = run::metal_launches(config, &sandbox, config.samples)?
        .iter()
        .map(|(start, trace)| breakdown(*start, trace))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(render(&breakdowns))
}
