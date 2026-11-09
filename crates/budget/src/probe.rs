//! What Cairn's `measure` build tells the harness, one line each on its stdout: what each frame
//! painted, and when. The harness decides from these which frame ended a sample.

use std::collections::BTreeMap;
use std::fmt;

use crate::presented::{Drawable, DrawableSource};

/// Tells `cairn` to turn its probe on, in a build with the `measure` feature.
pub const ENV: &str = "CAIRN_MEASURE";
/// The value of [`ENV`] for a window that draws without stopping. Any other value draws only
/// when something changes.
pub const ANIMATE: &str = "animate";

/// Starts every probe line, so that nothing else on stdout can read as one.
const MARK: &str = "cairn-budget";
/// Separates paths in a list. Fields are separated by tabs. No path the harness creates or
/// expects holds either, and [`Line`]'s `Display` replaces them in any other.
const LIST: char = '\u{1f}';

/// What one painted frame showed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    pub paint_ns: u64,
    /// Whether the window was the active one when this painted.
    pub active: bool,
    /// The branch HEAD is on, as shown. `None` before refs have answered, or when detached.
    pub head: Option<String>,
    /// The generation shown per Slot, 0 before it has answered.
    pub refs: u64,
    pub staged: u64,
    pub unstaged: u64,
    pub untracked: u64,
    pub unstaged_paths: Vec<String>,
    pub untracked_paths: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    /// The first line a probe writes.
    Hello {
        release: bool,
        pid: u32,
    },
    Frame(Frame),
    /// Just before Cairn sends this Intent to the Repo service.
    Intent {
        ns: u64,
        branch: String,
    },
    /// GPUI was handed a Metal drawable (macOS).
    Drawable {
        id: u64,
        source: DrawableSource,
        ns: u64,
    },
    /// Metal reports that the drawable reached the screen at `ns`.
    Presented {
        id: u64,
        ns: u64,
    },
    /// Metal reports that the drawable never reached the screen.
    Dropped {
        id: u64,
    },
    /// A key-down reached the window. `held` is the OS's own repeat flag.
    Key {
        ns: u64,
        held: bool,
    },
}

fn clean(text: &str) -> String {
    text.replace(['\t', '\n', '\r', LIST], "?")
}

fn list(paths: &[String]) -> String {
    let cleaned: Vec<String> = paths.iter().map(|p| clean(p)).collect();
    cleaned.join(&LIST.to_string())
}

fn unlist(field: &str) -> Vec<String> {
    match field {
        "" => Vec::new(),
        _ => field.split(LIST).map(str::to_owned).collect(),
    }
}

impl fmt::Display for Line {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{MARK}\t")?;
        match self {
            Line::Hello { release, pid } => write!(f, "hello\t{release}\t{pid}"),
            Line::Frame(frame) => write!(
                f,
                "frame\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                frame.paint_ns,
                frame.active,
                frame.head.as_deref().map(clean).unwrap_or_default(),
                frame.refs,
                frame.staged,
                frame.unstaged,
                frame.untracked,
                list(&frame.unstaged_paths),
                list(&frame.untracked_paths),
            ),
            Line::Intent { ns, branch } => write!(f, "intent\t{ns}\t{}", clean(branch)),
            Line::Drawable { id, source, ns } => {
                let source = match source {
                    DrawableSource::DisplayLink => "link",
                    DrawableSource::Layer => "layer",
                };
                write!(f, "drawable\t{id}\t{source}\t{ns}")
            }
            Line::Presented { id, ns } => write!(f, "presented\t{id}\t{ns}"),
            Line::Dropped { id } => write!(f, "dropped\t{id}"),
            Line::Key { ns, held } => write!(f, "key\t{ns}\t{held}"),
        }
    }
}

impl Line {
    /// The probe line this text holds, or `None` for anything else on stdout.
    pub fn parse(text: &str) -> Option<Line> {
        let mut fields = text.split('\t');
        if fields.next()? != MARK {
            return None;
        }
        let kind = fields.next()?;
        let rest: Vec<&str> = fields.collect();
        let number = |index: usize| rest.get(index)?.parse::<u64>().ok();
        let flag = |index: usize| rest.get(index)?.parse::<bool>().ok();
        match kind {
            "hello" => Some(Line::Hello {
                release: flag(0)?,
                pid: rest.get(1)?.parse().ok()?,
            }),
            "frame" => Some(Line::Frame(Frame {
                paint_ns: number(0)?,
                active: flag(1)?,
                head: Some(*rest.get(2)?)
                    .filter(|head| !head.is_empty())
                    .map(str::to_owned),
                refs: number(3)?,
                staged: number(4)?,
                unstaged: number(5)?,
                untracked: number(6)?,
                unstaged_paths: unlist(rest.get(7)?),
                untracked_paths: unlist(rest.get(8)?),
            })),
            "intent" => Some(Line::Intent {
                ns: number(0)?,
                branch: (*rest.get(1)?).to_owned(),
            }),
            "drawable" => Some(Line::Drawable {
                id: number(0)?,
                source: match *rest.get(1)? {
                    "link" => DrawableSource::DisplayLink,
                    "layer" => DrawableSource::Layer,
                    _ => return None,
                },
                ns: number(2)?,
            }),
            "presented" => Some(Line::Presented {
                id: number(0)?,
                ns: number(1)?,
            }),
            "dropped" => Some(Line::Dropped { id: number(0)? }),
            "key" => Some(Line::Key {
                ns: number(0)?,
                held: flag(1)?,
            }),
            _ => None,
        }
    }
}

/// The state a sample waits to see on screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expect {
    /// A file save: a newer Unstaged lists, or no longer lists, this path.
    Unstaged { path: String, listed: bool },
    /// A branch switch: HEAD is on the branch, every status Slot has answered since the
    /// baseline, and Untracked shows the flip that switching to this branch causes.
    Switched {
        branch: String,
        untracked: String,
        listed: bool,
    },
}

impl Expect {
    /// Whether `frame` shows this state, by what the screen held at `baseline`, the last frame
    /// before the change.
    pub fn met_by(&self, frame: &Frame, baseline: &Frame) -> bool {
        let has = |paths: &[String], path: &str| paths.iter().any(|p| p == path);
        match self {
            Expect::Unstaged { path, listed } => {
                frame.unstaged > baseline.unstaged && has(&frame.unstaged_paths, path) == *listed
            }
            Expect::Switched {
                branch,
                untracked,
                listed,
            } => {
                frame.head.as_deref() == Some(branch)
                    && frame.staged > baseline.staged
                    && frame.unstaged > baseline.unstaged
                    && frame.untracked > baseline.untracked
                    && has(&frame.untracked_paths, untracked) == *listed
            }
        }
    }
}

/// Everything one Cairn process has said so far.
#[derive(Clone, Debug, Default)]
pub struct Trace {
    pub pid: Option<u32>,
    pub release: Option<bool>,
    frames: Vec<Frame>,
    drawables: BTreeMap<u64, Drawable>,
    intents: Vec<(u64, String)>,
    keys: Vec<(u64, bool)>,
}

impl Trace {
    pub fn record(&mut self, line: Line) {
        match line {
            Line::Hello { release, pid } => {
                self.release = Some(release);
                self.pid = Some(pid);
            }
            Line::Frame(frame) => self.frames.push(frame),
            Line::Intent { ns, branch } => self.intents.push((ns, branch)),
            Line::Drawable { id, source, ns } => {
                self.drawables.insert(
                    id,
                    Drawable {
                        id,
                        source,
                        acquired_ns: ns,
                        presented_ns: None,
                    },
                );
            }
            Line::Presented { id, ns } => {
                if let Some(drawable) = self.drawables.get_mut(&id) {
                    drawable.presented_ns = Some(ns);
                }
            }
            Line::Dropped { .. } => {}
            Line::Key { ns, held } => self.keys.push((ns, held)),
        }
    }

    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }

    pub fn drawables(&self) -> Vec<Drawable> {
        self.drawables.values().copied().collect()
    }

    /// Whether any frame that painted in this stretch was in an inactive window.
    pub fn inactive_during(&self, start_ns: u64, end_ns: u64) -> bool {
        self.frames
            .iter()
            .any(|f| !f.active && (start_ns..=end_ns).contains(&f.paint_ns))
    }

    /// When Cairn sent each Intent, in order.
    pub fn intents(&self) -> &[(u64, String)] {
        &self.intents
    }

    /// Each key-down: when, and whether the OS flagged it as a repeat.
    pub fn keys(&self) -> &[(u64, bool)] {
        &self.keys
    }
}
