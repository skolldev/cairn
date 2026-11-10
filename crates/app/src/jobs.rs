//! The Jobs `app` has seen, by [`JobId`], from their Snapshots, and the banner showing the
//! newest: queued or running with its latest output line, done for a moment, or failed with
//! git's own words until the user dismisses them or starts another Job.
//!
//! A Local write Job has no stop button (ADR 0002). Quit and Force quit handle a running one,
//! and [`QuitView`](crate::quit::QuitView) reads the same Snapshots on its own.

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use cairn_repo::{JobId, JobKind, JobSnapshot, JobState};
use gpui::{
    Context, IntoElement, ParentElement, Render, SharedString, Styled, Task, Window, div,
    prelude::*, rgb,
};

/// How long a Job that ended well stays on screen.
const DONE_SHOWN: Duration = Duration::from_secs(3);

/// What the banner shows of one Job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Banner {
    Queued {
        title: String,
    },
    Running {
        title: String,
        /// The latest complete line git or a hook wrote.
        output: Option<String>,
    },
    Done {
        title: String,
    },
    Stopped {
        title: String,
    },
    /// Git's stderr, exactly as the Job reported it.
    Failed {
        title: String,
        error: String,
    },
}

#[derive(Default)]
pub struct JobsView {
    jobs: BTreeMap<JobId, Arc<JobSnapshot>>,
    /// Jobs up to this one are off screen: dismissed, or done and shown for long enough.
    hidden_through: Option<JobId>,
    /// Hides the newest Job once it has been shown done for [`DONE_SHOWN`].
    hide_done: Option<Task<()>>,
}

impl JobsView {
    pub fn record_job(&mut self, job: Arc<JobSnapshot>, cx: &mut Context<Self>) {
        let id = job.id;
        let ended_well = matches!(job.state, JobState::Done(Ok(())) | JobState::Stopped);
        self.jobs.insert(id, job);
        if self.newest().is_some_and(|newest| newest.id == id) {
            self.hide_done = ended_well.then(|| {
                let timer = cx.background_executor().timer(DONE_SHOWN);
                cx.spawn(async move |this, cx| {
                    timer.await;
                    let _ = this.update(cx, |this, cx| this.hide_through(id, cx));
                })
            });
        }
        cx.notify();
    }

    /// Every Job seen, oldest first, as its newest Snapshot.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn seen(&self) -> impl Iterator<Item = &Arc<JobSnapshot>> {
        self.jobs.values()
    }

    fn newest(&self) -> Option<&Arc<JobSnapshot>> {
        self.jobs.values().next_back()
    }

    fn hide_through(&mut self, id: JobId, cx: &mut Context<Self>) {
        self.hidden_through = Some(id);
        cx.notify();
    }

    /// What the banner shows, if anything: the newest Job, unless it is hidden.
    pub fn banner(&self) -> Option<Banner> {
        let job = self.newest()?;
        if self.hidden_through.is_some_and(|hidden| job.id <= hidden) {
            return None;
        }
        let title = title(&job.kind);
        Some(match &job.state {
            JobState::Queued => Banner::Queued { title },
            JobState::Running => Banner::Running {
                title,
                output: job.progress.clone(),
            },
            JobState::Done(Ok(())) => Banner::Done { title },
            JobState::Stopped => Banner::Stopped { title },
            JobState::Done(Err(error)) => Banner::Failed {
                title,
                error: error.clone(),
            },
        })
    }
}

/// The git command a Job runs, as the user would type it.
fn title(kind: &JobKind) -> String {
    match kind {
        JobKind::Switch { branch } => format!("git switch {branch}"),
        JobKind::Fetch => "git fetch".to_owned(),
        JobKind::Push => "git push".to_owned(),
    }
}

const DIM: u32 = 0x7d8590;
const FAILED: u32 = 0xff7a90;

impl Render for JobsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(banner) = self.banner() else {
            return div();
        };
        let row = div()
            .debug_selector(|| "job".to_owned())
            .flex()
            .items_start()
            .gap_3()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(rgb(0x2a2f37));
        let line = |selector: &'static str, text: SharedString| {
            div()
                .debug_selector(move || selector.to_owned())
                .child(text)
        };
        match banner {
            Banner::Queued { title } => row
                .child(div().child(title))
                .child(line("job:queued", "Waiting for git…".into()).text_color(rgb(DIM))),
            Banner::Running { title, output } => row
                .child(div().child(title))
                .child(line("job:running", "Running…".into()).text_color(rgb(DIM)))
                .children(output.map(|output| {
                    line("job:output", output.into())
                        .flex_1()
                        .min_w_0()
                        .text_color(rgb(DIM))
                        .whitespace_nowrap()
                        .overflow_hidden()
                })),
            Banner::Done { title } => row
                .child(div().child(title))
                .child(line("job:done", "Done".into()).text_color(rgb(DIM))),
            Banner::Stopped { title } => row
                .child(div().child(title))
                .child(line("job:stopped", "Stopped".into()).text_color(rgb(DIM))),
            Banner::Failed { title, error } => {
                let id = self.newest().map(|job| job.id);
                row.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().child(title))
                        .child(
                            line("job:failed", error.into())
                                .font_family("monospace")
                                .text_color(rgb(FAILED)),
                        ),
                )
                .child(
                    div()
                        .id("job-dismiss")
                        .debug_selector(|| "job:dismiss".to_owned())
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .bg(rgb(0x2a2f37))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(id) = id {
                                this.hide_through(id, cx);
                            }
                        }))
                        .child("Dismiss"),
                )
            }
        }
    }
}
