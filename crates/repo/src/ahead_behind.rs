//! Ahead/behind counts of the local branches on screen against their upstreams (#68).
//!
//! The UI names the branches whose rows it shows with [`Intent::ShowCounts`](crate::Intent),
//! and each Refs read hands over every local branch's (local oid, upstream oid). A Read Job in
//! the Background lane then counts the visible pairs it hasn't counted before, with `git
//! rev-list --left-right --count`, so a refresh that moves neither oid counts nothing. A
//! count can take seconds on a large repository, so the Interactive lane never waits on one.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cairn_write::Stop;

use crate::scheduler::{JobContext, Lane, Running, Slot};
use crate::{Count, Counts, Generation, Oid, Outbox, RefName, Refs, SlotSnapshot};

/// The counts Slot, and what its Read Jobs count from.
pub(crate) struct Counter {
    slot: Slot,
    outbox: Arc<Outbox>,
    /// Every submission happens under this lock, so a [`close`](Self::close) leaves none.
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// The repository the last Open named, until a Quit closes it.
    open: Option<PathBuf>,
    /// The local branches whose rows are on screen, in the order the UI named them.
    visible: Vec<RefName>,
    /// From the newest Refs read of the open repository.
    refs: Option<RefsRead>,
    /// Ahead and behind for each (local oid, upstream oid) counted in the open repository.
    cache: HashMap<(Oid, Oid), (usize, usize)>,
}

struct RefsRead {
    generation: Generation,
    /// Each local branch with an upstream that exists: (local oid, upstream oid).
    pairs: HashMap<RefName, (Oid, Oid)>,
}

impl Counter {
    pub(crate) fn new(lane: &Lane, outbox: Arc<Outbox>) -> Arc<Self> {
        Arc::new(Self {
            slot: Slot::new("counts", lane),
            outbox,
            state: Mutex::default(),
        })
    }

    /// Forgets the repository that was open, its counts included, and stops counting it.
    pub(crate) fn opened(&self, path: &Path) {
        let mut state = self.state.lock().unwrap();
        state.open = Some(path.to_path_buf());
        state.refs = None;
        state.cache.clear();
        self.slot.drop_jobs();
    }

    /// Stops counting, and starts nothing more until the next Open.
    pub(crate) fn close(&self) {
        let mut state = self.state.lock().unwrap();
        state.open = None;
        self.slot.drop_jobs();
    }

    /// Takes the pairs a Refs read of `path` found, answering Refs `generation`, and counts
    /// the visible ones not counted before. A count already running finishes: its pair may
    /// still be wanted.
    pub(crate) fn refs_read(self: &Arc<Self>, path: &Path, generation: Generation, refs: &Refs) {
        let mut state = self.state.lock().unwrap();
        if state.open.as_deref() != Some(path) {
            return;
        }
        let pairs = refs
            .local
            .iter()
            .filter_map(|entry| {
                let upstream = entry.upstream.as_ref()?.target.clone()?;
                Some((entry.name.clone(), (entry.target.clone(), upstream)))
            })
            .collect();
        state.refs = Some(RefsRead { generation, pairs });
        self.submit(&state, Running::Finish, "refs");
    }

    /// Names the local branches on screen. Counting what scrolled away is no longer wanted,
    /// so a running count is Superseded. The same set again changes nothing.
    pub(crate) fn show(self: &Arc<Self>, visible: Vec<RefName>) {
        let mut state = self.state.lock().unwrap();
        if state.visible == visible {
            return;
        }
        state.visible = visible;
        if state.refs.is_some() {
            self.submit(&state, Running::Supersede, "show_counts");
        }
    }

    /// Stops the running count, if any, for a drop that shouldn't wait on it.
    pub(crate) fn drop_jobs(&self) {
        self.slot.drop_jobs();
    }

    fn submit(self: &Arc<Self>, state: &State, running: Running, cause: &'static str) {
        let Some(path) = state.open.clone() else {
            return;
        };
        let counter = self.clone();
        let span = tracing::Span::current();
        self.slot.submit(running, cause, move |job| {
            counter.count(&path, job, span);
        });
    }

    /// The Read Job: counts each visible pair not counted yet, publishing what it has before
    /// each count and once at the end, so a slow count holds back only its own row.
    fn count(&self, path: &Path, job: &JobContext, span: tracing::Span) {
        let (refs, mut branches, mut pending) = {
            let state = self.state.lock().unwrap();
            let Some(refs) = &state.refs else {
                return;
            };
            let mut branches = BTreeMap::new();
            let mut pending = Vec::new();
            for name in &state.visible {
                let Some((local, upstream)) = refs.pairs.get(name) else {
                    continue;
                };
                match state.cache.get(&(local.clone(), upstream.clone())) {
                    Some(&(ahead, behind)) => {
                        branches.insert(name.clone(), count(local, upstream, ahead, behind));
                    }
                    None => pending.push((name.clone(), local.clone(), upstream.clone())),
                }
            }
            (refs.generation, branches, pending)
        };
        pending.reverse();
        while let Some((name, local, upstream)) = pending.pop() {
            let still: Vec<RefName> = std::iter::once(name.clone())
                .chain(pending.iter().rev().map(|(name, ..)| name.clone()))
                .collect();
            self.publish(path, job, &span, refs, &branches, still);
            let counted = rev_list(path, job.cli_stop(), &local, &upstream);
            if !job.is_current() {
                tracing::debug!("superseded while running; result dropped");
                return;
            }
            match counted {
                Ok((ahead, behind)) => {
                    self.state
                        .lock()
                        .unwrap()
                        .cache
                        .insert((local.clone(), upstream.clone()), (ahead, behind));
                    branches.insert(name, count(&local, &upstream, ahead, behind));
                }
                // Shown as no count, and tried again by the next Read Job.
                Err(error) => tracing::warn!(%error, branch = %name, "couldn't count ahead/behind"),
            }
        }
        self.publish(path, job, &span, refs, &branches, Vec::new());
    }

    fn publish(
        &self,
        path: &Path,
        job: &JobContext,
        span: &tracing::Span,
        refs: Generation,
        branches: &BTreeMap<RefName, Count>,
        pending: Vec<RefName>,
    ) {
        if !job.is_current() {
            return;
        }
        self.outbox.publish_progress(
            |p| &mut p.counts,
            Arc::new(SlotSnapshot {
                generation: job.generation(),
                workdir: path.to_path_buf(),
                result: Ok(Counts {
                    refs,
                    branches: branches.clone(),
                    pending,
                }),
                span: span.clone(),
            }),
        );
    }
}

fn count(local: &Oid, upstream: &Oid, ahead: usize, behind: usize) -> Count {
    Count {
        local: local.clone(),
        upstream: upstream.clone(),
        ahead,
        behind,
    }
}

/// `git rev-list --left-right --count <local>...<upstream>^{commit}`: commits only on the local
/// side, then only on the upstream's. An upstream can be a tag, through `branch.<name>.remote =
/// .`, and in a partial clone newer git fetches a tag's missing target for `^{commit}` but not
/// for a bare symmetric difference (`docs/git-versions.md`).
fn rev_list(
    path: &Path,
    stop: &Stop,
    local: &Oid,
    upstream: &Oid,
) -> Result<(usize, usize), String> {
    let range = format!("{local}...{upstream}^{{commit}}");
    let output =
        crate::fallback::run(path, stop, &["rev-list", "--left-right", "--count", &range])?;
    let text = String::from_utf8_lossy(&output.stdout);
    let parsed = text
        .trim()
        .split_once('\t')
        .and_then(|(ahead, behind)| Some((ahead.parse().ok()?, behind.parse().ok()?)));
    parsed.ok_or_else(|| format!("git rev-list --count returned {:?}", text.trim()))
}
