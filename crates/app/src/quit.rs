//! Quitting from the window: `app` asks before stopping a push and before a Force quit,
//! and shows what a Quit is waiting for.
//!
//! The Repo service does the quitting. [`QuitFlow`] decides from Job Snapshots what a close
//! request means. [`QuitView`] puts the questions to the user.

use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use cairn_repo::{Intent, JobId, JobKind, JobSnapshot, JobState, RepoService};
use gpui::{
    App, Context, IntoElement, ParentElement, PromptButton, PromptLevel, Render, SharedString,
    Styled, Window, div, prelude::*, rgb,
};

/// What a request to close the window leads to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CloseStep {
    /// Nothing to ask: send the Quit Intent.
    Quit,
    /// A push hasn't ended, and a Quit would stop it.
    ConfirmStopPush,
    /// A Quit is already waiting, so the only way further is a Force quit.
    ConfirmForceQuit,
}

/// The Jobs that haven't ended, from their Snapshots, and whether a Quit has been sent.
#[derive(Default)]
struct QuitFlow {
    jobs: BTreeMap<JobId, Arc<JobSnapshot>>,
    quit_sent: bool,
}

impl QuitFlow {
    fn record(&mut self, job: Arc<JobSnapshot>) {
        match job.state {
            JobState::Queued | JobState::Running => {
                self.jobs.insert(job.id, job);
            }
            JobState::Done(_) | JobState::Stopped => {
                self.jobs.remove(&job.id);
            }
        }
    }

    fn on_close(&self) -> CloseStep {
        if self.quit_sent {
            CloseStep::ConfirmForceQuit
        } else if self.jobs.values().any(|job| job.kind == JobKind::Push) {
            CloseStep::ConfirmStopPush
        } else {
            CloseStep::Quit
        }
    }

    /// The oldest Local write Job a sent Quit is waiting for: the one running, since they
    /// run in order.
    fn waiting_on(&self) -> Option<&Arc<JobSnapshot>> {
        if !self.quit_sent {
            return None;
        }
        self.jobs.values().find(|job| job.kind.is_local_write())
    }
}

/// A yes-or-no question for the user.
struct Question {
    level: PromptLevel,
    message: &'static str,
    detail: &'static str,
    accept: &'static str,
    decline: &'static str,
}

const STOP_PUSH: Question = Question {
    level: PromptLevel::Warning,
    message: "A push is running",
    detail: "Quitting stops it, and your commits may not reach the remote.",
    accept: "Stop push and quit",
    decline: "Keep pushing",
};

const FORCE_QUIT: Question = Question {
    level: PromptLevel::Critical,
    message: "Force quit while git is running?",
    detail: "Exiting can kill git mid-step, which can leave a stale index.lock or a half-applied \
             operation in the repository.",
    accept: "Force quit",
    decline: "Keep waiting",
};

/// Answers the window's close request, and shows a banner while a Quit waits on git. A hung
/// hook has no stop button (ADR 0002): its output is here, and Force quit is the way out.
pub struct QuitView {
    service: Arc<RepoService>,
    /// What ends the app once the Repo service has quit.
    exit: Rc<dyn Fn(&mut App)>,
    flow: QuitFlow,
    /// A question is on screen. A second close request mustn't stack another.
    asking: bool,
}

impl QuitView {
    pub fn new(service: Arc<RepoService>, exit: Rc<dyn Fn(&mut App)>) -> Self {
        Self {
            service,
            exit,
            flow: QuitFlow::default(),
            asking: false,
        }
    }

    pub fn record_job(&mut self, job: Arc<JobSnapshot>, cx: &mut Context<Self>) {
        self.flow.record(job);
        if self.flow.quit_sent {
            cx.notify();
        }
    }

    /// The Repo service has quit: nothing is left to wait for.
    pub fn exit(&mut self, cx: &mut Context<Self>) {
        (self.exit)(cx);
    }

    pub fn close_requested(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.flow.on_close() {
            CloseStep::Quit => self.send_quit(cx),
            CloseStep::ConfirmStopPush => {
                self.ask(STOP_PUSH, |this, cx| this.send_quit(cx), window, cx);
            }
            CloseStep::ConfirmForceQuit => self.confirm_force_quit(window, cx),
        }
    }

    fn send_quit(&mut self, cx: &mut Context<Self>) {
        self.flow.quit_sent = true;
        self.service.send(Intent::Quit);
        cx.notify();
    }

    fn confirm_force_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ask(
            FORCE_QUIT,
            |this, _| this.service.send(Intent::ForceQuit),
            window,
            cx,
        );
    }

    /// Puts one question to the user, and runs `confirmed` if they accept. Any other answer
    /// changes nothing.
    fn ask(
        &mut self,
        question: Question,
        confirmed: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if std::mem::replace(&mut self.asking, true) {
            return;
        }
        let buttons = [
            PromptButton::new(question.accept),
            PromptButton::cancel(question.decline),
        ];
        let answer = window.prompt(
            question.level,
            question.message,
            Some(question.detail),
            &buttons,
            cx,
        );
        cx.spawn(async move |this, cx| {
            let accepted = answer.await == Ok(0);
            let _ = this.update(cx, |this, cx| {
                this.asking = false;
                if accepted {
                    confirmed(this, cx);
                }
            });
        })
        .detach();
    }
}

impl Render for QuitView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(job) = self.flow.waiting_on() else {
            return div();
        };
        let output: Option<SharedString> = job.progress.clone().map(Into::into);
        div()
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(rgb(0x2a2f37))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child("Waiting for git to finish before quitting…")
                    .children(output.map(|line| div().text_color(rgb(0x7d8590)).child(line))),
            )
            .child(
                div()
                    .id("force-quit")
                    .debug_selector(|| "force-quit".to_owned())
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(rgb(0x2a2f37))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.confirm_force_quit(window, cx);
                    }))
                    .child("Force quit…"),
            )
    }
}

#[cfg(test)]
mod tests;
