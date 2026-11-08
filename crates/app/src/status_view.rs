//! The status Snapshots as plain lists: Staged, Unstaged and untracked files.

use std::sync::Arc;

use cairn_repo::{ChangeKind, ChangesSnapshot, FileChange, UntrackedSnapshot};
use gpui::{
    AnyElement, Context, IntoElement, ParentElement, Render, SharedString, Styled, Window, div,
    prelude::*, px, rgb,
};

const DIM: u32 = 0x7d8590;
const ERROR: u32 = 0xff7a90;

#[derive(Default)]
pub struct StatusView {
    staged: Option<Arc<ChangesSnapshot>>,
    unstaged: Option<Arc<ChangesSnapshot>>,
    untracked: Option<Arc<UntrackedSnapshot>>,
}

impl StatusView {
    pub fn set_staged(&mut self, snapshot: Arc<ChangesSnapshot>) {
        self.staged = Some(snapshot);
    }

    pub fn set_unstaged(&mut self, snapshot: Arc<ChangesSnapshot>) {
        self.unstaged = Some(snapshot);
    }

    pub fn set_untracked(&mut self, snapshot: Arc<UntrackedSnapshot>) {
        self.untracked = Some(snapshot);
    }

    /// Tells the Budget harness what this view shows.
    #[cfg(feature = "measure")]
    pub fn report_to(&self, frame: &mut cairn_budget::probe::Frame) {
        if let Some(staged) = &self.staged {
            frame.staged = staged.generation.get();
        }
        if let Some(unstaged) = &self.unstaged {
            frame.unstaged = unstaged.generation.get();
            if let Ok(changes) = &unstaged.result {
                frame.unstaged_paths = changes.iter().map(|c| c.path.to_string()).collect();
            }
        }
        if let Some(untracked) = &self.untracked {
            frame.untracked = untracked.generation.get();
            if let Ok(paths) = &untracked.result {
                frame.untracked_paths = paths.iter().map(ToString::to_string).collect();
            }
        }
    }
}

impl Render for StatusView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("status")
            .size_full()
            .overflow_y_scroll()
            .p_3()
            .flex()
            .flex_col()
            .gap_3()
            .child(section("Staged", self.staged.as_ref().map(|s| changes(s))))
            .child(section(
                "Unstaged",
                self.unstaged.as_ref().map(|s| changes(s)),
            ))
            .child(section(
                "Untracked",
                self.untracked.as_ref().map(|s| untracked(s)),
            ))
    }
}

fn untracked(snapshot: &UntrackedSnapshot) -> Result<Vec<AnyElement>, String> {
    snapshot
        .result
        .as_ref()
        .map(|paths| paths.iter().map(|p| row('?', p.to_string())).collect())
        .map_err(Clone::clone)
}

fn changes(snapshot: &ChangesSnapshot) -> Result<Vec<AnyElement>, String> {
    snapshot
        .result
        .as_ref()
        .map(|list| list.iter().map(change_row).collect())
        .map_err(Clone::clone)
}

fn change_row(change: &FileChange) -> AnyElement {
    let path = match &change.kind {
        ChangeKind::Renamed { from, .. } => format!("{from} -> {}", change.path),
        _ => change.path.to_string(),
    };
    row(change.kind.letter(), path)
}

/// A titled list. `None` while the Slot hasn't answered yet.
fn section(title: &'static str, rows: Option<Result<Vec<AnyElement>, String>>) -> impl IntoElement {
    let (heading, body): (String, Vec<AnyElement>) = match rows {
        None => (format!("{title} (reading…)"), Vec::new()),
        Some(Ok(rows)) => (format!("{title} ({})", rows.len()), rows),
        Some(Err(error)) => (
            title.to_owned(),
            vec![div().text_color(rgb(ERROR)).child(error).into_any_element()],
        ),
    };
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .text_color(rgb(DIM))
                .child(SharedString::from(heading)),
        )
        .children(body)
}

fn row(letter: char, path: String) -> AnyElement {
    div()
        .debug_selector(|| format!("status:{letter}:{path}"))
        .flex()
        .gap(px(8.))
        .child(
            div()
                .text_color(rgb(DIM))
                .child(SharedString::from(letter.to_string())),
        )
        .child(div().child(path))
        .into_any_element()
}
