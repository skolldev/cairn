//! The Jobs `app` has seen, by [`JobId`], from their Snapshots, and the banner showing the
//! newest: queued or running with its latest output line, done for a moment, or failed with
//! git's own words until the user dismisses them or starts another Job. A switch git refused
//! because local changes would be overwritten also offers Stash and switch.
//!
//! A queued or running fetch has a Stop button. A Local write Job has none (ADR 0002):
//! Quit and Force quit handle a running one, and [`QuitView`](crate::quit::QuitView) reads
//! the same Snapshots on its own.

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use cairn_repo::{Intent, JobId, JobKind, JobSnapshot, JobState};
use gpui::{
    Context, EventEmitter, IntoElement, ParentElement, Render, SharedString, Styled, Task, Window,
    div, prelude::*, rgb,
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
        /// The failure offers Stash and switch beside Dismiss.
        stash_and_switch: bool,
    },
}

/// The user clicked Stash and switch: send this Intent.
pub struct StashAndSwitch(pub Intent);

impl EventEmitter<StashAndSwitch> for JobsView {}

/// The user asked to stop this Network Job.
pub struct StopJob(pub JobId);

#[derive(Default)]
pub struct JobsView {
    jobs: BTreeMap<JobId, Arc<JobSnapshot>>,
    /// Jobs up to this one are off screen: dismissed, or done and shown for long enough.
    hidden_through: Option<JobId>,
    /// Hides the newest Job once it has been shown done for [`DONE_SHOWN`].
    hide_done: Option<Task<()>>,
    /// A Fetch Intent has been sent, and no Snapshot of its Job has arrived yet.
    fetch_asked: bool,
}

impl JobsView {
    pub fn record_job(&mut self, job: Arc<JobSnapshot>, cx: &mut Context<Self>) {
        let id = job.id;
        if job.kind == JobKind::Fetch {
            self.fetch_asked = false;
        }
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

    /// Records that a Fetch Intent has been sent, until its Job's first Snapshot arrives.
    pub fn record_fetch_sent(&mut self) {
        self.fetch_asked = true;
    }

    /// Whether a fetch has been asked for and hasn't ended, so another would only queue.
    pub fn fetching(&self) -> bool {
        self.fetch_asked || self.jobs.values().any(|job| unended_fetch(job))
    }

    fn newest(&self) -> Option<&Arc<JobSnapshot>> {
        self.jobs.values().next_back()
    }

    /// The newest Job, if the user can stop it: a fetch that hasn't ended. A push isn't
    /// offered, because stopping one can leave the user's work unpublished.
    fn stoppable(&self) -> Option<JobId> {
        self.newest()
            .filter(|job| unended_fetch(job))
            .map(|job| job.id)
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
                stash_and_switch: job.stash_and_switch().is_some(),
            },
        })
    }
}

/// Whether `job` is a fetch that is queued or running.
fn unended_fetch(job: &JobSnapshot) -> bool {
    job.kind == JobKind::Fetch && matches!(job.state, JobState::Queued | JobState::Running)
}

/// The git command a Job runs, as the user would type it.
fn title(kind: &JobKind) -> String {
    match kind {
        JobKind::Switch { branch } => format!("git switch {branch}"),
        JobKind::Stash { .. } => "git stash push".to_owned(),
        JobKind::SwitchToRemote { remote } => format!("git switch --track {}", remote.short()),
        JobKind::Fetch => "git fetch".to_owned(),
        JobKind::Push => "git push".to_owned(),
    }
}

impl EventEmitter<StopJob> for JobsView {}

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
        let stop = self.stoppable().map(|id| {
            div()
                .id("job-stop")
                .debug_selector(|| "job:stop".to_owned())
                .px_2()
                .py_1()
                .rounded_sm()
                .bg(rgb(0x2a2f37))
                .cursor_pointer()
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(StopJob(id))))
                .child("Stop")
        });
        match banner {
            Banner::Queued { title } => row
                .child(div().child(title))
                .child(
                    line("job:queued", "Waiting for git…".into())
                        .flex_1()
                        .text_color(rgb(DIM)),
                )
                .children(stop),
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
                }))
                .children(stop),
            Banner::Done { title } => row
                .child(div().child(title))
                .child(line("job:done", "Done".into()).text_color(rgb(DIM))),
            Banner::Stopped { title } => row
                .child(div().child(title))
                .child(line("job:stopped", "Stopped".into()).text_color(rgb(DIM))),
            Banner::Failed { title, error, .. } => {
                let newest = self.newest();
                let id = newest.map(|job| job.id);
                let offer = newest.and_then(|job| job.stash_and_switch());
                let button = |id: &'static str| {
                    div()
                        .id(id)
                        .debug_selector(move || format!("job:{id}"))
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .bg(rgb(0x2a2f37))
                        .cursor_pointer()
                };
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
                .children(offer.map(|intent| {
                    button("stash-and-switch")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(id) = id {
                                this.hide_through(id, cx);
                            }
                            cx.emit(StashAndSwitch(intent.clone()));
                        }))
                        .child("Stash and switch")
                }))
                .child(
                    button("dismiss")
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
