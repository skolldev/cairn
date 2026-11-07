//! The coalesced bridge: the one place Snapshots cross from the Repo service into GPUI.
//!
//! The service wakes the bridge from its own threads. The bridge drains at most once per
//! frame, drops stale generations, swaps each Snapshot into the entity that shows it, and
//! notifies only entities whose Snapshot changed. That caps the notify rate, which matters
//! for zed#61469 on Windows.

use std::sync::Arc;

use cairn_repo::{Generation, RepoService, Snapshot};
use gpui::{Context, Entity, Window};

use crate::refs_view::RefsView;

pub struct Bridge {
    service: Arc<RepoService>,
    refs: Entity<RefsView>,
    refs_shown: Option<Generation>,
    /// A drain already ran this frame. The next-frame callback drains whatever came since.
    armed: bool,
}

impl Bridge {
    pub fn new(
        service: Arc<RepoService>,
        woken: async_channel::Receiver<()>,
        refs: Entity<RefsView>,
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
            refs_shown: None,
            armed: false,
        }
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
    }

    fn apply(&mut self, snapshots: Vec<Snapshot>, cx: &mut Context<Self>) {
        let _span = tracing::debug_span!("bridge.drain", count = snapshots.len()).entered();
        for snapshot in snapshots {
            match snapshot {
                Snapshot::Refs(refs) => {
                    if self
                        .refs_shown
                        .is_some_and(|shown| shown >= refs.generation)
                    {
                        tracing::debug!(parent: &refs.span, generation = refs.generation.get(), "stale refs snapshot dropped");
                        continue;
                    }
                    self.refs_shown = Some(refs.generation);
                    self.refs.update(cx, |view, cx| {
                        view.set_snapshot(refs);
                        cx.notify();
                    });
                }
            }
        }
    }
}
