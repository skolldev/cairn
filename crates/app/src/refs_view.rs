//! The refs Snapshot as plain lists: HEAD, local and remote branches, tags and stashes.

use std::sync::Arc;

use cairn_repo::{Generation, Head, RefEntry, RefsSnapshot};
use gpui::{
    AnyElement, Context, IntoElement, ParentElement, Render, SharedString, Styled, Window, div,
    prelude::*, px, rgb,
};

const DIM: u32 = 0x7d8590;

#[derive(Default)]
pub struct RefsView {
    snapshot: Option<Arc<RefsSnapshot>>,
    shown: Option<Generation>,
}

impl RefsView {
    pub fn set_snapshot(&mut self, snapshot: Arc<RefsSnapshot>) {
        self.snapshot = Some(snapshot);
    }
}

impl Render for RefsView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let body = div()
            .id("refs")
            .size_full()
            .overflow_y_scroll()
            .p_3()
            .flex()
            .flex_col()
            .gap_3();
        let Some(snapshot) = self.snapshot.clone() else {
            return body.child(
                div()
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
                return body.child(div().text_color(rgb(0xff7a90)).child(format!(
                    "Couldn't open {}: {error}",
                    snapshot.workdir.display()
                )));
            }
        };
        let head = match &refs.head {
            Head::Branch {
                name,
                target: Some(target),
            } => {
                format!("{} at {}", name.short(), target.short(10))
            }
            Head::Branch { name, target: None } => format!("{} (unborn)", name.short()),
            Head::Detached { target } => format!("detached at {}", target.short(10)),
        };
        let stashes = refs
            .stashes
            .iter()
            .enumerate()
            .map(|(i, s)| row(format!("stash@{{{i}}}"), s.message.clone()))
            .collect();
        body.child(section("HEAD", vec![row(head, String::new())]))
            .child(section("Local branches", entries(&refs.local)))
            .child(section("Remote branches", entries(&refs.remote)))
            .child(section("Tags", entries(&refs.tags)))
            .child(section("Stashes", stashes))
    }
}

fn entries(list: &[RefEntry]) -> Vec<AnyElement> {
    list.iter()
        .map(|entry| {
            let detail = match &entry.symref {
                Some(symref) => format!("-> {}", symref.short()),
                None => entry.target.short(10).to_owned(),
            };
            row(entry.name.short().to_owned(), detail)
        })
        .collect()
}

fn section(title: &'static str, rows: Vec<AnyElement>) -> impl IntoElement {
    let count = rows.len();
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .text_color(rgb(DIM))
                .child(SharedString::from(format!("{title} ({count})"))),
        )
        .children(rows)
}

fn row(name: String, detail: String) -> AnyElement {
    div()
        .flex()
        .gap(px(12.))
        .child(div().child(name))
        .child(div().text_color(rgb(DIM)).child(detail))
        .into_any_element()
}
