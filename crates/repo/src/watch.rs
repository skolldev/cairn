//! The watcher: how External changes reach the Repo service (ADR 0004).
//!
//! notify reports every change under the open repository's worktree, `.git/` included.
//! Each event maps to the Slots it can change, and refreshes them at once. There is no
//! debounce timer: the Slot coalesces, because it holds at most one running and one
//! pending Read Job, so a burst costs at most two refreshes back to back.
//!
//! A change to a file git ignores and doesn't track, such as a build writing to `target/`,
//! refreshes nothing: see [`Filter`].

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use notify::event::ModifyKind;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};

use crate::{Reads, Refresh};

/// Watches one open repository until dropped.
pub(crate) struct Watcher {
    // Kept for its Drop, which stops the watch.
    _inner: RecommendedWatcher,
}

/// Starts watching the repository at `workdir`, refreshing `reads` while that repository
/// is still the open one. `None`, after logging why, if notify can't watch it.
pub(crate) fn start(workdir: &Path, reads: Weak<Reads>) -> Option<Watcher> {
    // notify reports resolved paths (on macOS a temp dir under /var arrives as
    // /private/var), so events are matched against the resolved root.
    let root = match workdir.canonicalize() {
        Ok(root) => root,
        Err(e) => {
            tracing::debug!(path = %workdir.display(), error = %e, "not watched");
            return None;
        }
    };
    let workdir = workdir.to_path_buf();
    let git_dir = root.join(".git");
    let filter = if git_dir.is_dir() {
        Some(Filter::start(&workdir))
    } else {
        // A linked worktree or a submodule keeps its git dir elsewhere, so a switch or a
        // commit there isn't seen. Worktree files still are, all of them: with the index
        // unwatched, which files are tracked would go stale unnoticed.
        tracing::warn!(path = %root.display(), "`.git` is not a directory; git's own changes aren't watched");
        None
    };
    let handler_root = root.clone();
    let handler = move |event: notify::Result<Event>| {
        let Some(reads) = reads.upgrade() else {
            return;
        };
        let event = match event {
            Ok(event) => event,
            Err(e) => {
                // A lost event could hide any change, so read everything again.
                tracing::warn!(error = %e, "watcher error; refreshing everything");
                external_change(&reads, &workdir, Refresh::RefsAndStatus);
                return;
            }
        };
        // Not for reads: on Linux every read of the index or a `.gitignore`, the rebuild's
        // own included, reports an open, and each would start the next rebuild.
        if let Some(filter) = &filter
            && !matches!(event.kind, EventKind::Access(_))
            && event
                .paths
                .iter()
                .any(|path| changes_filter(path, &handler_root, &git_dir))
        {
            // Before classifying, so that this event is never skipped by the old matcher,
            // not even for a `.gitignore` that is itself ignored.
            filter.rebuild();
        }
        let skips = |relative: &Path| filter.as_ref().is_some_and(|f| f.skips(relative));
        if let Some(what) = classify(&event, &handler_root, &git_dir, skips) {
            external_change(&reads, &workdir, what);
        }
    };
    let mut inner = match notify::recommended_watcher(handler) {
        Ok(inner) => inner,
        Err(e) => {
            tracing::warn!(error = %e, "couldn't start the watcher");
            return None;
        }
    };
    if let Err(e) = inner.watch(&root, RecursiveMode::Recursive) {
        tracing::warn!(path = %root.display(), error = %e, "couldn't watch the repository");
        return None;
    }
    Some(Watcher { _inner: inner })
}

fn external_change(reads: &Reads, workdir: &Path, what: Refresh) {
    let span = tracing::info_span!("external_change", path = %workdir.display());
    let _entered = span.enter();
    reads.refresh_if_open(workdir, what, &span);
}

/// The Slots one event can change, or `None` for an event that changes nothing Cairn shows.
/// `skips` says whether a change at a worktree path, relative to `root`, can't change status.
fn classify(
    event: &Event,
    root: &Path,
    git_dir: &Path,
    mut skips: impl FnMut(&Path) -> bool,
) -> Option<Refresh> {
    // Reads open files too. Seeing them would make every refresh trigger the next.
    if matches!(event.kind, EventKind::Access(_)) {
        return None;
    }
    if event.need_rescan() {
        return Some(Refresh::RefsAndStatus);
    }
    // A directory's own changes, such as the timestamps Windows reports when a read walks
    // it, mean nothing to git: a file added, removed or changed inside reports its own
    // event. A removed directory no longer exists, so its event still counts.
    let directory_only = matches!(
        event.kind,
        EventKind::Modify(ModifyKind::Any | ModifyKind::Metadata(_))
    );
    event
        .paths
        .iter()
        .filter(|path| !(directory_only && path.is_dir()))
        .filter_map(|path| classify_path(path, root, git_dir, &mut skips))
        .reduce(Refresh::union)
}

fn classify_path(
    path: &Path,
    root: &Path,
    git_dir: &Path,
    skips: &mut impl FnMut(&Path) -> bool,
) -> Option<Refresh> {
    let Ok(inside) = path.strip_prefix(git_dir) else {
        // Anything else under the worktree is a file git may track or list as untracked,
        // unless it is ignored and untracked.
        let relative = path.strip_prefix(root).ok()?;
        return (!skips(relative)).then_some(Refresh::Status);
    };
    // git writes `<file>.lock` and renames it over `<file>`; the rename reports `<file>`.
    if inside.extension().is_some_and(|e| e == "lock") {
        return None;
    }
    let mut parts = inside.components().map(|c| c.as_os_str());
    match (parts.next()?.to_str()?, parts.next()) {
        // HEAD moving changes what the index is compared with.
        ("HEAD", None) => Some(Refresh::RefsAndStatus),
        ("index", None) => Some(Refresh::Status),
        ("packed-refs" | "FETCH_HEAD", None) => Some(Refresh::Refs),
        // Stashes are listed from the stash reflog, which `git stash drop` rewrites alone.
        ("logs", Some(refs)) if refs == "refs" && parts.next().is_some_and(|p| p == "stash") => {
            Some(Refresh::Refs)
        }
        // Excludes and `status.showUntrackedFiles` change Untracked; config can also rename
        // a remote.
        ("info", Some(exclude)) if exclude == "exclude" => Some(Refresh::Status),
        ("config", None) => Some(Refresh::RefsAndStatus),
        // A local branch may be the one HEAD names, as after `git reset --soft`.
        ("refs", Some(kind)) if kind == "heads" => Some(Refresh::RefsAndStatus),
        ("refs", Some(_)) => Some(Refresh::Refs),
        // objects/, the other logs, hooks/ and the rest change nothing shown.
        _ => None,
    }
}

/// Whether a change at `path` can change what the [`Filter`] answers: the index, the
/// excludes and config git reads, or any `.gitignore`.
fn changes_filter(path: &Path, root: &Path, git_dir: &Path) -> bool {
    match path.strip_prefix(git_dir) {
        Ok(inside) => {
            inside == Path::new("index")
                || inside == Path::new("config")
                || inside == Path::new("info/exclude")
        }
        Err(_) => path.starts_with(root) && path.file_name().is_some_and(|n| n == ".gitignore"),
    }
}

/// Skips worktree paths git ignores and doesn't track (ADR 0004: a refresh storm is fixed in
/// how events map to Slots). Shared by notify's thread, which asks, and the thread that
/// rebuilds the matcher, which never holds the lock while it reads the index.
struct Filter {
    workdir: PathBuf,
    state: Mutex<FilterState>,
}

#[derive(Default)]
struct FilterState {
    /// `None` while a rebuild runs, or after one failed: then nothing is skipped.
    matcher: Option<cairn_read::Excludes>,
    rebuilding: bool,
    /// A change the running rebuild may have missed arrived while it ran.
    rerun: bool,
}

impl Filter {
    /// Builds the matcher here, once per Open, before any event is seen.
    fn start(workdir: &Path) -> Arc<Self> {
        Arc::new(Self {
            workdir: workdir.to_path_buf(),
            state: Mutex::new(FilterState {
                matcher: Self::build(workdir),
                ..FilterState::default()
            }),
        })
    }

    fn build(workdir: &Path) -> Option<cairn_read::Excludes> {
        cairn_read::Excludes::build(workdir)
            .inspect_err(|e| {
                tracing::warn!(error = %e, "couldn't read excludes; every change refreshes");
            })
            .ok()
    }

    fn skips(&self, relative: &Path) -> bool {
        let mut state = self.state.lock().unwrap();
        state.matcher.as_mut().is_some_and(|m| m.skips(relative))
    }

    /// Drops the matcher, which may now be wrong, and builds a new one on its own thread.
    fn rebuild(self: &Arc<Self>) {
        let mut state = self.state.lock().unwrap();
        state.matcher = None;
        if state.rebuilding {
            state.rerun = true;
            return;
        }
        state.rebuilding = true;
        drop(state);
        let filter = self.clone();
        let spawned = std::thread::Builder::new()
            .name("excludes".into())
            .spawn(move || {
                loop {
                    let matcher = Self::build(&filter.workdir);
                    let mut state = filter.state.lock().unwrap();
                    if std::mem::take(&mut state.rerun) {
                        continue;
                    }
                    state.matcher = matcher;
                    state.rebuilding = false;
                    return;
                }
            });
        if let Err(e) = spawned {
            // With no matcher, nothing is skipped: correct, only slower.
            tracing::warn!(error = %e, "couldn't start a rebuild of the excludes");
            self.state.lock().unwrap().rebuilding = false;
        }
    }
}
