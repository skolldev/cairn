//! The sidebar: the Refs Snapshot as Local, Remotes, Tags and Stashes, with remote groups,
//! folders for `/` in names, and a filter box on top. Only the visible rows are laid out, so
//! a repository with thousands of tags stays responsive with every section open.

mod rows;
#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::sync::Arc;

#[cfg(feature = "measure")]
use cairn_repo::Refs;
use cairn_repo::{Generation, Head, RefsSnapshot};
use gpui::{
    AnyElement, Context, ElementId, Entity, FontWeight, IntoElement, ParentElement, Render,
    SharedString, Styled, Subscription, Window, div, prelude::*, px, rgb, uniform_list,
};
use gpui_component::input::{Input, InputEvent, InputState};

use rows::{Row, Tree};

const DIM: u32 = 0x7d8590;
const CURRENT: u32 = 0x58a6ff;
const ROW_HEIGHT: f32 = 22.;
const INDENT: f32 = 14.;

pub struct RefsView {
    snapshot: Option<Arc<RefsSnapshot>>,
    shown: Option<Generation>,
    /// The Snapshot's refs as a tree, or `None` while there are none to show.
    tree: Option<Tree>,
    /// The ids of the sections and folders the user closed. The filter leaves it alone, so
    /// clearing the filter brings back the state from before.
    closed: HashSet<String>,
    filter: Entity<InputState>,
    rows: Vec<Row>,
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
            tree: None,
            closed: HashSet::new(),
            filter,
            rows: Vec::new(),
            _filter_changed: filter_changed,
        }
    }

    pub fn set_snapshot(&mut self, snapshot: Arc<RefsSnapshot>, cx: &mut Context<Self>) {
        self.tree = snapshot.result.as_ref().ok().map(Tree::new);
        self.snapshot = Some(snapshot);
        self.update_rows(cx);
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
                return sidebar.child(div().px_3().text_color(rgb(0xff7a90)).child(format!(
                    "Couldn't open {}: {error}",
                    snapshot.workdir.display()
                )));
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
            cx.processor(|view, range: std::ops::Range<usize>, _window, cx| {
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
            } => {
                let element = element.pl(indent(*depth));
                if *current {
                    let marker = format!("sidebar:current:{}", name.as_str());
                    element
                        .text_color(rgb(CURRENT))
                        .font_weight(FontWeight::BOLD)
                        .child(div().debug_selector(|| marker).child("●"))
                        .child(label.clone())
                } else {
                    element.child(label.clone())
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
