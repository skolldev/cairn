//! The sidebar: the Refs Snapshot as Local, Remotes, Tags and Stashes, with remote groups,
//! folders for `/` in names, and a filter box on top. Only the visible rows are laid out, so
//! a repository with thousands of tags stays responsive with every section open.

mod rows;
#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(feature = "measure")]
use cairn_repo::Refs;
use cairn_repo::{CountsSnapshot, Generation, Head, RefName, RefsSnapshot};
use gpui::{
    AnyElement, ClickEvent, Context, ElementId, Entity, EventEmitter, FontWeight, IntoElement,
    ParentElement, Render, SharedString, Styled, Subscription, Window, div, prelude::*, px, rgb,
    uniform_list,
};
use gpui_component::input::{Input, InputEvent, InputState};

use rows::{Row, Tree};

const DIM: u32 = 0x7d8590;
const CURRENT: u32 = 0x58a6ff;
const ROW_HEIGHT: f32 = 22.;
const INDENT: f32 = 14.;

/// The user asked to switch to this local branch, by its short name.
pub struct SwitchTo(pub String);

/// The user asked to switch to this remote-tracking branch, by its full name: to the local
/// branch that tracks it, or to a new one.
pub struct SwitchToRemote(pub RefName);

/// The local branches whose rows are on screen changed: these want ahead/behind counts.
pub struct ShowCounts(pub Vec<RefName>);

pub struct RefsView {
    snapshot: Option<Arc<RefsSnapshot>>,
    shown: Option<Generation>,
    /// The repository open since its Refs last read, until the next Open: an error then is a
    /// failed refresh of a repository already open, not a failed Open.
    opened: Option<PathBuf>,
    /// The Snapshot's refs as a tree, or `None` while there are none to show.
    tree: Option<Tree>,
    /// The ids of the sections and folders the user closed. The filter leaves it alone, so
    /// clearing the filter brings back the state from before.
    closed: HashSet<String>,
    filter: Entity<InputState>,
    rows: Vec<Row>,
    /// The newest counts, shown only for a branch whose oids they still match.
    counts: Option<Arc<CountsSnapshot>>,
    /// The local branches on screen, as last reported with [`ShowCounts`].
    visible: Vec<RefName>,
    /// The rows the list last asked for. To measure a row it first asks for row 0 alone, on
    /// every layout, and only then for the rows it draws.
    drawn: std::ops::Range<usize>,
    _filter_changed: Subscription,
}

impl RefsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
        let filter_changed = cx.subscribe(&filter, |view, _, event, cx| {
            if let InputEvent::Change = event {
                view.update_rows(cx);
                cx.notify();
            }
        });
        Self {
            snapshot: None,
            shown: None,
            opened: None,
            tree: None,
            closed: HashSet::new(),
            filter,
            rows: Vec::new(),
            counts: None,
            visible: Vec::new(),
            drawn: 0..0,
            _filter_changed: filter_changed,
        }
    }

    pub fn set_snapshot(&mut self, snapshot: Arc<RefsSnapshot>, cx: &mut Context<Self>) {
        self.tree = snapshot.result.as_ref().ok().map(Tree::new);
        if snapshot.result.is_ok() {
            self.opened = Some(snapshot.workdir.clone());
        }
        self.snapshot = Some(snapshot);
        self.update_rows(cx);
    }

    /// An Open was sent: until its Refs read, an error is a failed Open, even of the same
    /// repository again.
    pub fn opening(&mut self) {
        self.opened = None;
    }

    pub fn set_counts(&mut self, counts: Arc<CountsSnapshot>) {
        self.counts = Some(counts);
    }

    /// The counts Snapshot shown last.
    #[cfg(test)]
    pub fn counts(&self) -> Option<&Arc<CountsSnapshot>> {
        self.counts.as_ref()
    }

    /// Reports the local branches among the rows the list drew, when they differ from those
    /// reported last. It runs once the frame is drawn, so the row the list asked for only to
    /// measure it is never reported: the count of a branch on screen would be Superseded, and
    /// start over, on every frame.
    fn report_visible(&mut self, cx: &mut Context<Self>) {
        let range = &self.drawn;
        let visible: Vec<RefName> = self.rows
            [range.start.min(self.rows.len())..range.end.min(self.rows.len())]
            .iter()
            .filter_map(|row| match row {
                Row::Ref { name, .. } if name.is_local_branch() => Some(name.clone()),
                _ => None,
            })
            .collect();
        if visible != self.visible {
            self.visible = visible.clone();
            cx.emit(ShowCounts(visible));
        }
    }

    /// What to draw beside a local branch: `↑2 ↓5` against its upstream, nothing when it is
    /// level or its count hasn't come, and `gone` when its upstream no longer exists. A count
    /// for other oids than the row's is stale, and shows nothing until the new one comes.
    fn divergence(&self, row: &Row) -> Option<(String, String)> {
        let Row::Ref {
            name,
            target,
            upstream: Some(upstream),
            ..
        } = row
        else {
            return None;
        };
        let Some(upstream_target) = &upstream.target else {
            return Some((format!("sidebar:gone:{name}"), "gone".to_owned()));
        };
        let counts = self.counts.as_ref()?.result.as_ref().ok()?;
        let count = counts.branches.get(name)?;
        if count.local != *target || count.upstream != *upstream_target {
            return None;
        }
        let text = [("↑", count.ahead), ("↓", count.behind)]
            .iter()
            .filter(|(_, n)| *n > 0)
            .map(|(arrow, n)| format!("{arrow}{n}"))
            .collect::<Vec<_>>()
            .join(" ");
        (!text.is_empty()).then(|| (format!("sidebar:counts:{name}:{text}"), text))
    }

    /// The Refs Snapshot shown last.
    #[cfg(test)]
    pub fn snapshot(&self) -> Option<&Arc<RefsSnapshot>> {
        self.snapshot.as_ref()
    }

    fn update_rows(&mut self, cx: &mut Context<Self>) {
        let filter = self.filter.read(cx).value();
        self.rows = match &self.tree {
            Some(tree) => tree.rows(&self.closed, filter.trim()),
            None => Vec::new(),
        };
    }

    /// Opens or closes the section or folder `id`. While the filter is set, everything
    /// leading to a match is open, and a click changes nothing.
    fn toggle(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.filter.read(cx).value().trim().is_empty() {
            return;
        }
        if !self.closed.remove(id) {
            self.closed.insert(id.to_owned());
        }
        self.update_rows(cx);
        cx.notify();
    }

    /// Asks to switch to the local branch `branch`, unless it is already the current one. A
    /// double-click on its row lands here, and so does the Budget harness's switch.
    pub fn switch_to(&mut self, branch: String, cx: &mut Context<Self>) {
        let current = self
            .tree
            .as_ref()
            .and_then(Tree::current)
            .is_some_and(|current| current.short() == branch);
        if !current {
            cx.emit(SwitchTo(branch));
        }
    }

    /// Tells the Budget harness what this view shows.
    #[cfg(feature = "measure")]
    pub fn report_to(&self, frame: &mut cairn_budget::probe::Frame) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        frame.refs = snapshot.generation.get();
        if let Ok(Refs {
            head: Head::Branch { name, .. },
            ..
        }) = &snapshot.result
        {
            frame.head = Some(name.short().to_owned());
        }
    }
}

impl EventEmitter<SwitchTo> for RefsView {}
impl EventEmitter<SwitchToRemote> for RefsView {}
impl EventEmitter<ShowCounts> for RefsView {}

impl Render for RefsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar = div().size_full().flex().flex_col().child(
            div()
                .debug_selector(|| "sidebar:filter".to_owned())
                .p_2()
                .child(Input::new(&self.filter).cleanable(true)),
        );
        let Some(snapshot) = self.snapshot.clone() else {
            return sidebar.child(
                div()
                    .px_3()
                    .text_color(rgb(DIM))
                    .child("No repository open. Pass a path on the command line, or use Open."),
            );
        };
        if self.shown != Some(snapshot.generation) {
            self.shown = Some(snapshot.generation);
            tracing::info!(parent: &snapshot.span, generation = snapshot.generation.get(), "snapshot shown");
        }
        let refs = match &snapshot.result {
            Ok(refs) => refs,
            Err(error) => {
                let (selector, failed) = if self.opened.as_ref() == Some(&snapshot.workdir) {
                    (
                        "sidebar:error:Couldn't read refs",
                        "Couldn't read refs".to_owned(),
                    )
                } else {
                    (
                        "sidebar:error:Couldn't open",
                        format!("Couldn't open {}", snapshot.workdir.display()),
                    )
                };
                return sidebar.child(
                    div()
                        .debug_selector(|| selector.to_owned())
                        .px_3()
                        .text_color(rgb(0xff7a90))
                        .child(format!("{failed}: {error}")),
                );
            }
        };
        let head = match &refs.head {
            Head::Detached { target } => {
                let short = target.short(10).to_owned();
                Some(
                    div()
                        .debug_selector(|| "sidebar:detached".to_owned())
                        .child(
                            div()
                                .debug_selector(|| format!("sidebar:detached:{short}"))
                                .child(format!("HEAD detached at {short}")),
                        ),
                )
            }
            Head::Branch { name, target: None } => {
                Some(div().child(format!("{} has no commits yet", name.short())))
            }
            Head::Branch { .. } => None,
        };
        let list = uniform_list(
            "sidebar-rows",
            self.rows.len(),
            cx.processor(|view, range: std::ops::Range<usize>, window, cx| {
                view.drawn = range.clone();
                cx.defer_in(window, |view, _, cx| view.report_visible(cx));
                range.map(|ix| view.render_row(ix, cx)).collect::<Vec<_>>()
            }),
        )
        .flex_1()
        .min_h_0();
        sidebar
            .children(head.map(|head| head.px_3().pb_1().text_color(rgb(CURRENT))))
            .child(list)
    }
}

impl RefsView {
    fn render_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let row = &self.rows[ix];
        let divergence = self.divergence(row).map(|(selector, text)| {
            div()
                .debug_selector(|| selector)
                .text_color(rgb(DIM))
                .child(text)
        });
        let id = row.id();
        let selector = format!("sidebar:{id}");
        let mut element = div()
            .id(ElementId::Name(SharedString::from(id.clone())))
            .debug_selector(|| selector)
            .h(px(ROW_HEIGHT))
            .px_3()
            .flex()
            .items_center()
            .gap(px(6.))
            .whitespace_nowrap();
        element = match row {
            Row::Section {
                section,
                count,
                open,
            } => element
                .text_color(rgb(DIM))
                .child(chevron(*open))
                .child(format!("{} ({count})", section.title())),
            Row::Folder {
                label, depth, open, ..
            } => element
                .pl(indent(*depth))
                .child(chevron(*open))
                .child(label.clone()),
            Row::Ref {
                name,
                label,
                depth,
                current,
                ..
            } => {
                let element = element.pl(indent(*depth));
                if *current {
                    let marker = format!("sidebar:current:{}", name.as_str());
                    element
                        .text_color(rgb(CURRENT))
                        .font_weight(FontWeight::BOLD)
                        .child(div().debug_selector(|| marker).child("●"))
                        .child(label.clone())
                        .children(divergence)
                } else {
                    element.child(label.clone()).children(divergence)
                }
            }
            Row::Stash { index, message } => element
                .pl(indent(0))
                .child(format!("stash@{{{index}}}"))
                .child(div().text_color(rgb(DIM)).child(message.clone())),
        };
        if row.toggles() {
            element = element
                .cursor_pointer()
                .on_click(cx.listener(move |view, _, _, cx| view.toggle(&id, cx)));
        } else if let Some(branch) = row.local_branch() {
            element = element.on_click(cx.listener(move |view, event: &ClickEvent, _, cx| {
                if event.click_count() == 2 {
                    view.switch_to(branch.clone(), cx);
                }
            }));
        } else if let Some(remote) = row.remote_branch() {
            element = element.on_click(cx.listener(move |_, event: &ClickEvent, _, cx| {
                if event.click_count() == 2 {
                    cx.emit(SwitchToRemote(remote.clone()));
                }
            }));
        }
        element.into_any_element()
    }
}

fn indent(depth: usize) -> gpui::Pixels {
    px(12. + INDENT * (depth + 1) as f32)
}

fn chevron(open: bool) -> &'static str {
    if open { "▾" } else { "▸" }
}
