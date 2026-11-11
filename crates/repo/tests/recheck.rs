//! A Recheck reads again every Slot whose last Snapshot was an error (#43, ADR 0009). Its own
//! binary, because each test breaks the user config that every git run in the binary reads.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use cairn_repo::{Intent, JobState, Snapshot};
use common::{Harness, Seen, git, open, switch_to, trace};

/// The tests share one user config, so they take turns.
static USER_CONFIG: Mutex<()> = Mutex::new(());

/// Whether each Slot's last Snapshot was an error, or `None` if it published none.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Shown {
    refs: Option<bool>,
    staged: Option<bool>,
    unstaged: Option<bool>,
    untracked: Option<bool>,
}

impl Shown {
    fn of(seen: &Seen, repo: &Path) -> Self {
        let mut shown = Self::default();
        for s in &seen.all {
            match s {
                Snapshot::Refs(s) if s.workdir == repo => shown.refs = Some(s.result.is_err()),
                Snapshot::Staged(s) if s.workdir == repo => shown.staged = Some(s.result.is_err()),
                Snapshot::Unstaged(s) if s.workdir == repo => {
                    shown.unstaged = Some(s.result.is_err())
                }
                Snapshot::Untracked(s) if s.workdir == repo => {
                    shown.untracked = Some(s.result.is_err())
                }
                _ => {}
            }
        }
        shown
    }

    fn all(failed: bool) -> Self {
        Self {
            refs: Some(failed),
            staged: Some(failed),
            unstaged: Some(failed),
            untracked: Some(failed),
        }
    }
}

/// A committed repository on `main`, opened.
fn opened(harness: &Harness) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("r");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "--quiet", "-b", "main"]);
    std::fs::write(repo.join("tracked.txt"), "x\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "x"]);
    common::assert_filesystem_events_available(&repo);
    open(harness, &repo);
    (dir, repo)
}

/// The user config every git run reads. The watcher doesn't watch it, so a fix there reports
/// no External change.
fn user_config() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("the hermetic HOME")).join(".gitconfig")
}

/// Breaks the user config, so that git dies reading it, until dropped.
struct Broken;

impl Broken {
    fn config() -> Self {
        std::fs::write(user_config(), "[broken\n").unwrap();
        Broken
    }
}

impl Drop for Broken {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(user_config());
    }
}

/// Waits until the watcher has gone quiet: one write can report several events, and a read
/// a late one starts after the fix would clear the error without a Recheck. Nothing marks
/// the last event, so this waits a fixed time; the control is
/// `a_recheck_reads_again_every_slot_that_shows_an_error` going red with Recheck doing nothing.
fn settle(seen: &mut Seen, harness: &Harness, repo: &Path) {
    let changes = || {
        trace::external_changes()
            .iter()
            .filter(|span| trace::about(span, repo))
            .count()
    };
    loop {
        let before = (changes(), seen.all.len());
        std::thread::sleep(Duration::from_secs(1));
        seen.drain_now(harness);
        if (changes(), seen.all.len()) == before {
            return;
        }
    }
}

#[test]
fn a_recheck_reads_again_every_slot_that_shows_an_error() {
    let _turn = USER_CONFIG.lock().unwrap_or_else(|e| e.into_inner());
    trace::install();
    let harness = Harness::start();
    let (_dir, repo) = opened(&harness);
    let mut seen = Seen::default();

    let broken = Broken::config();
    // HEAD written again as it was: an External change that reads Refs and all of status.
    let head = std::fs::read(repo.join(".git/HEAD")).unwrap();
    std::fs::write(repo.join(".git/HEAD"), &head).unwrap();
    seen.until(&harness, |seen| Shown::of(seen, &repo) == Shown::all(true));
    settle(&mut seen, &harness, &repo);
    assert_eq!(Shown::of(&seen, &repo), Shown::all(true));

    drop(broken);
    // Seen red: with Recheck doing nothing, every Slot still shows the error here.
    harness.service.send(Intent::Recheck);
    seen.until(&harness, |seen| Shown::of(seen, &repo) == Shown::all(false));
}

#[test]
fn a_recheck_leaves_alone_the_slots_that_answered() {
    let _turn = USER_CONFIG.lock().unwrap_or_else(|e| e.into_inner());
    trace::install();
    let harness = Harness::start();
    let (_dir, repo) = opened(&harness);
    let mut seen = Seen::default();

    let broken = Broken::config();
    // A saved worktree file reads only Unstaged and Untracked.
    std::fs::write(repo.join("tracked.txt"), "saved\n").unwrap();
    let only_status_failed = Shown {
        refs: None,
        staged: None,
        unstaged: Some(true),
        untracked: Some(true),
    };
    seen.until(&harness, |seen| {
        Shown::of(seen, &repo) == only_status_failed
    });
    settle(&mut seen, &harness, &repo);
    assert_eq!(Shown::of(&seen, &repo), only_status_failed);

    drop(broken);
    // Spans of the other test in this binary closed before this one opened, settling waited
    // out the reads the save started, and every read this Recheck queued closes after this
    // point.
    let from = trace::read_jobs().len();
    harness.service.send(Intent::Recheck);
    // The marker: a switch to the current branch refreshes refs and status once it ends, and
    // its writes start refreshes through the watcher. Each of those reads queues behind any
    // read the Recheck started in its Slot, and a later one takes a queued one's place
    // (ADR 0004), so in each Slot the last of them reads after the Recheck's (#102).
    harness.service.send(Intent::Switch("main".to_owned()));
    seen.until(&harness, |seen| {
        let switched = seen
            .job(&switch_to("main"))
            .is_some_and(|job| matches!(job.state, JobState::Done(Ok(()))));
        switched && Shown::of(seen, &repo).untracked == Some(false)
    });
    // The switch's own Intent names no path, so its reads can't be matched by repository:
    // any read that closed after `from` and that the Recheck didn't ask for marks its Slot.
    let spans = trace::wait_until(
        trace::read_jobs,
        from,
        "no read after the Recheck's in every Slot",
        |spans| {
            ["refs", "staged", "unstaged", "untracked"]
                .iter()
                .all(|slot| {
                    spans.iter().any(|span| {
                        span.get("slot").is_some_and(|s| s == slot)
                            && span.get("refresh").is_some_and(|r| r != "recheck")
                    })
                })
        },
    );
    let mut rechecked: Vec<&str> = spans
        .iter()
        .filter(|span| {
            trace::about(span, &repo) && span.get("refresh").is_some_and(|r| r == "recheck")
        })
        .map(|span| span["slot"].as_str())
        .collect();
    rechecked.sort();
    // Seen red: Recheck reading every Slot lists all four.
    assert_eq!(rechecked, ["unstaged", "untracked"]);
}
