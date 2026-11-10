//! The coalesced bridge: the one place Snapshots cross from the Repo service into GPUI.
//!
//! The service wakes the bridge from its own threads. The bridge drains at most once per
//! frame, drops stale generations, swaps each Snapshot into the entity that shows it, and
//! notifies only entities whose Snapshot changed. That caps the notify rate, which matters
//! for zed#61469 on Windows. A window that draws no frames still drains, more slowly,
//! because the app can't exit until it has seen the Quit Snapshot.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cairn_repo::{Generation, RepoService, Snapshot};
use gpui::{Context, Entity, Task, Window};

use crate::jobs::JobsView;
use crate::quit::QuitView;
use crate::recent::RecentView;
use crate::refs_view::RefsView;
use crate::status_view::StatusView;

/// How long a drain waits for the next frame before going ahead without one. Several frames
/// long, so a window that is drawing always drains on its frames.
const FRAMELESS_DRAIN: Duration = Duration::from_millis(100);

/// The entities that show Snapshots.
pub struct Views {
    pub refs: Entity<RefsView>,
    pub status: Entity<StatusView>,
    pub quit: Entity<QuitView>,
    pub jobs: Entity<JobsView>,
    pub recent: Entity<RecentView>,
}

pub struct Bridge {
    service: Arc<RepoService>,
    refs: Entity<RefsView>,
    status: Entity<StatusView>,
    quit: Entity<QuitView>,
    jobs: Entity<JobsView>,
    recent: Entity<RecentView>,
    /// The workdir whose Open last succeeded, so a refresh of it isn't recorded again.
    recorded: Option<PathBuf>,
    /// The newest generation shown, per Slot. Generations of different Slots are unrelated.
    refs_shown: Option<Generation>,
    staged_shown: Option<Generation>,
    unstaged_shown: Option<Generation>,
    untracked_shown: Option<Generation>,
    /// A drain already ran this frame. The next-frame callback drains whatever came since.
    armed: bool,
    /// Ends the wait if no frame comes. Replacing it cancels the last one.
    frameless_drain: Option<Task<()>>,
}

impl Bridge {
    pub fn new(
        service: Arc<RepoService>,
        woken: async_channel::Receiver<()>,
        Views {
            refs,
            status,
            quit,
            jobs,
            recent,
        }: Views,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.spawn_in(window, async move |this, cx| {
            while woken.recv().await.is_ok() {
                if this
                    .update_in(cx, |bridge, window, cx| bridge.pump(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Self {
            service,
            refs,
            status,
            quit,
            jobs,
            recent,
            recorded: None,
            refs_shown: None,
            staged_shown: None,
            unstaged_shown: None,
            untracked_shown: None,
            armed: false,
            frameless_drain: None,
        }
    }

    /// An Open was sent: its first Ok Refs Snapshot is recorded, even for the workdir that
    /// is already open.
    pub fn forget_recorded(&mut self) {
        self.recorded = None;
    }

    fn pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.armed {
            return;
        }
        let snapshots = self.service.drain();
        if snapshots.is_empty() {
            return;
        }
        self.apply(snapshots, cx);
        self.armed = true;
        let this = cx.weak_entity();
        window.on_next_frame(move |window, cx| {
            let _ = this.update(cx, |bridge, cx| {
                bridge.armed = false;
                bridge.pump(window, cx);
            });
        });
        let timer = cx.background_executor().timer(FRAMELESS_DRAIN);
        self.frameless_drain = Some(cx.spawn_in(window, async move |this, cx| {
            timer.await;
            let _ = this.update_in(cx, |bridge, window, cx| {
                bridge.armed = false;
                bridge.pump(window, cx);
            });
        }));
    }

    fn apply(&mut self, snapshots: Vec<Snapshot>, cx: &mut Context<Self>) {
        let _span = tracing::debug_span!("bridge.drain", count = snapshots.len()).entered();
        for snapshot in snapshots {
            match snapshot {
                Snapshot::Refs(refs) => {
                    if newer(&mut self.refs_shown, refs.generation, "refs", &refs.span) {
                        // The first Ok Refs Snapshot for a workdir is its Open succeeding.
                        if refs.result.is_ok() && self.recorded.as_ref() != Some(&refs.workdir) {
                            self.recorded = Some(refs.workdir.clone());
                            self.recent
                                .update(cx, |view, cx| view.record(&refs.workdir, cx));
                        }
                        self.refs.update(cx, |view, cx| {
                            view.set_snapshot(refs, cx);
                            cx.notify();
                        });
                    }
                }
                Snapshot::Staged(staged) => {
                    if newer(
                        &mut self.staged_shown,
                        staged.generation,
                        "staged",
                        &staged.span,
                    ) {
                        self.status.update(cx, |view, cx| {
                            view.set_staged(staged);
                            cx.notify();
                        });
                    }
                }
                Snapshot::Unstaged(unstaged) => {
                    if newer(
                        &mut self.unstaged_shown,
                        unstaged.generation,
                        "unstaged",
                        &unstaged.span,
                    ) {
                        self.status.update(cx, |view, cx| {
                            view.set_unstaged(unstaged);
                            cx.notify();
                        });
                    }
                }
                // The banner shows the newest Job; a waiting Quit reads them on its own.
                Snapshot::Job(job) => {
                    self.quit
                        .update(cx, |view, cx| view.record_job(job.clone(), cx));
                    self.jobs.update(cx, |view, cx| view.record_job(job, cx));
                }
                Snapshot::Quit => self.quit.update(cx, |view, cx| view.exit(cx)),
                Snapshot::Untracked(untracked) => {
                    if newer(
                        &mut self.untracked_shown,
                        untracked.generation,
                        "untracked",
                        &untracked.span,
                    ) {
                        self.status.update(cx, |view, cx| {
                            view.set_untracked(untracked);
                            cx.notify();
                        });
                    }
                }
            }
        }
    }
}

/// Records `generation` as shown and returns true, unless it is no newer than `shown`.
fn newer(
    shown: &mut Option<Generation>,
    generation: Generation,
    slot: &str,
    span: &tracing::Span,
) -> bool {
    if shown.is_some_and(|shown| shown >= generation) {
        tracing::debug!(parent: span, slot, generation = generation.get(), "stale snapshot dropped");
        return false;
    }
    *shown = Some(generation);
    true
}
