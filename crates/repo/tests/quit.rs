//! Quit and Force quit: quitting can't damage the repository.

mod common;

use std::path::Path;
use std::time::Duration;

use cairn_repo::{Intent, JobKind, JobState, Snapshot};
use common::{
    Harness, Release, Seen, git, git_read, install_blocking_hook, install_slow_clean_filter,
    is_ended, lock_files, open, slash, slow_remote, small_fixture_clone, wait_for_file,
    wait_until_network_job_is_waiting,
};

/// How long a test waits when nothing marks what must not happen. Long enough for a Quit
/// that isn't waiting: `a_quit_with_nothing_running_completes` is the control.
const SETTLE: Duration = Duration::from_millis(300);

fn is_quit(s: &Snapshot) -> bool {
    matches!(s, Snapshot::Quit)
}

fn quit_seen(seen: &Seen) -> bool {
    seen.all.iter().any(is_quit)
}

fn switch_to(branch: &str) -> JobKind {
    JobKind::Switch {
        branch: branch.to_owned(),
    }
}

fn current_branch(repo: &Path) -> String {
    String::from_utf8(git_read(repo, &["symbolic-ref", "--short", "HEAD"]))
        .unwrap()
        .trim()
        .to_owned()
}

/// A repository with branches `other` and `third`, and a hook that blocks each switch until
/// `release` exists.
struct BlockingRepo {
    _dir: tempfile::TempDir,
    repo: std::path::PathBuf,
    release: std::path::PathBuf,
    log: std::path::PathBuf,
}

fn blocking_repo() -> BlockingRepo {
    let (dir, repo) = small_fixture_clone();
    git(&repo, &["branch", "other"]);
    git(&repo, &["branch", "third"]);
    let release = dir.path().join("release");
    let log = dir.path().join("hook.log");
    install_blocking_hook(&repo, &log, &release);
    BlockingRepo {
        _dir: dir,
        repo,
        release,
        log,
    }
}

/// Sends a switch to `other` and waits until its hook is blocked.
fn block_a_switch(harness: &Harness, seen: &mut Seen) {
    harness.service.send(Intent::Switch("other".to_owned()));
    seen.until(harness, |s| {
        s.job(&switch_to("other"))
            .is_some_and(|j| j.progress.as_deref() == Some("hook waiting on other"))
    });
}

#[test]
fn a_quit_with_nothing_running_completes() {
    let (_dir, repo) = small_fixture_clone();
    let harness = Harness::start();
    open(&harness, &repo);

    harness.service.send(Intent::Quit);
    let mut seen = Seen::default();
    seen.until(&harness, quit_seen);
}

#[test]
fn a_quit_waits_for_a_blocked_local_write_job_and_the_one_queued_behind_it() {
    let b = blocking_repo();
    let harness = Harness::start();
    let _release = Release(&b.release);
    open(&harness, &b.repo);
    let mut seen = Seen::default();
    block_a_switch(&harness, &mut seen);
    harness.service.send(Intent::Switch("third".to_owned()));
    seen.until(&harness, |s| s.job(&switch_to("third")).is_some());

    harness.service.send(Intent::Quit);
    // Seen red: a Quit that doesn't wait for the Local write queue publishes here.
    std::thread::sleep(SETTLE);
    seen.drain_now(&harness);
    assert!(!quit_seen(&seen), "quit completed under a blocked hook");
    let settled = seen.all.len();

    std::fs::write(&b.release, "").unwrap();
    seen.until(&harness, quit_seen);
    for kind in [switch_to("other"), switch_to("third")] {
        let job = seen.job(&kind).unwrap();
        assert_eq!(job.state, JobState::Done(Ok(())), "{job:?}");
    }
    assert_eq!(current_branch(&b.repo), "third");
    assert!(!b.repo.join(".git/index.lock").exists());

    // The Jobs' refreshes are dropped too: nobody is left to show them. A refresh is queued
    // before a Job's outcome, so one that ran would publish within this window. Control:
    // `a_switch_runs_git_switch_and_refreshes_refs_and_status`. Seen red: this fails if the
    // Quit leaves the repository open.
    std::thread::sleep(SETTLE);
    seen.drain_now(&harness);
    let reads: Vec<_> = seen.all[settled..]
        .iter()
        .filter(|s| !matches!(s, Snapshot::Job(_) | Snapshot::Quit))
        .collect();
    assert!(reads.is_empty(), "a read ran after the quit: {reads:?}");
}

#[test]
fn nothing_new_starts_after_a_quit() {
    let b = blocking_repo();
    let harness = Harness::start();
    let _release = Release(&b.release);
    open(&harness, &b.repo);
    let mut seen = Seen::default();
    block_a_switch(&harness, &mut seen);
    harness.service.send(Intent::Quit);

    harness.service.send(Intent::Switch("third".to_owned()));
    seen.until(&harness, |s| {
        s.job(&switch_to("third")).is_some_and(|j| is_ended(j))
    });
    let refused = seen.job(&switch_to("third")).unwrap();
    assert!(
        matches!(refused.state, JobState::Done(Err(_))),
        "{refused:?}"
    );

    std::fs::write(&b.release, "").unwrap();
    seen.until(&harness, quit_seen);
    // Seen red: a Switch that was only queued would run behind the Quit's marker.
    assert_eq!(
        std::fs::read_to_string(&b.log).unwrap(),
        "start other\nend other\n"
    );
    assert_eq!(current_branch(&b.repo), "other");

    let settled = seen.all.len();
    harness.service.send(Intent::Open(b.repo.clone()));
    // Seen red: an Open that isn't ignored publishes refs within this window, as the `open`
    // helper relies on.
    std::thread::sleep(SETTLE);
    seen.drain_now(&harness);
    assert!(seen.all[settled..].is_empty(), "{:?}", &seen.all[settled..]);
}

#[test]
fn a_quit_drops_the_running_and_the_pending_read_job() {
    let (dir, repo) = small_fixture_clone();
    let started = dir.path().join("started");
    let release = dir.path().join("release");
    // Blocks the first Unstaged read in its filter. Later reads pass straight through.
    install_slow_clean_filter(
        &repo,
        dir.path(),
        &format!(
            "if [ ! -f '{started}' ]; then\n\
             touch '{started}'\n\
             while [ ! -f '{release}' ]; do sleep 0.05; done\n\
             fi\n",
            started = slash(&started),
            release = slash(&release),
        ),
    );
    let harness = Harness::start();
    let _release = Release(&release);
    harness.service.send(Intent::Open(repo.clone()));
    wait_for_file(&started);
    // An External change leaves a pending read behind the blocked one. Nothing marks that
    // it arrived, so this waits. Control: the "pending kept" break named below goes red.
    std::fs::write(repo.join("changed.txt"), "changed\n").unwrap();
    std::thread::sleep(SETTLE);

    harness.service.send(Intent::Quit);
    let mut seen = Seen::default();
    // The Quit doesn't wait for the blocked read.
    seen.until(&harness, quit_seen);
    assert!(!release.exists());

    // Seen red: without the stop, the running read publishes once released. With the
    // pending read kept, it runs once the stopped one ends. Control for the wait:
    // `a_newer_status_read_stops_the_running_one_instead_of_waiting_for_it`.
    std::fs::write(&release, "").unwrap();
    std::thread::sleep(SETTLE);
    seen.drain_now(&harness);
    let unstaged: Vec<_> = seen
        .all
        .iter()
        .filter(|s| matches!(s, Snapshot::Unstaged(_)))
        .collect();
    assert!(
        unstaged.is_empty(),
        "a dropped read published: {unstaged:?}"
    );
}

#[test]
fn a_quit_stops_a_running_fetch_and_a_queued_one() {
    let f = slow_remote();
    let harness = Harness::start();
    let _release = Release(&f.release);
    open(&harness, &f.repo);
    harness.service.send(Intent::Fetch);
    let mut seen = Seen::default();
    wait_until_network_job_is_waiting(&mut seen, &harness, &JobKind::Fetch);
    harness.service.send(Intent::Fetch);
    seen.until(&harness, |s| s.nth(1).is_some());

    // Seen red: without the stop, the wrapper holds the fetch and this times out.
    harness.service.send(Intent::Quit);
    seen.until(&harness, quit_seen);
    // The Quit follows the final Snapshot of each.
    assert_eq!(seen.nth(0).unwrap().state, JobState::Stopped);
    assert_eq!(seen.nth(1).unwrap().state, JobState::Stopped);
    assert!(!f.release.exists(), "the wrapper was released, not stopped");
    assert_eq!(std::fs::read_to_string(&f.log).unwrap(), "run\n");
    assert_eq!(
        git(&f.repo, &["rev-parse", &format!("origin/{}", f.branch)]).trim(),
        f.old
    );
    assert_eq!(
        lock_files(&f.repo.join(".git")),
        Vec::<std::path::PathBuf>::new()
    );
}

#[test]
fn a_quit_stops_a_running_push() {
    let f = slow_remote();
    f.commit_to_push();
    let harness = Harness::start();
    let _release = Release(&f.release);
    open(&harness, &f.repo);
    harness.service.send(Intent::Push);
    let mut seen = Seen::default();
    wait_until_network_job_is_waiting(&mut seen, &harness, &JobKind::Push);

    // Seen red: without the stop, the wrapper holds the push and this times out.
    harness.service.send(Intent::Quit);
    seen.until(&harness, quit_seen);
    let job = seen.job(&JobKind::Push).unwrap();
    assert_eq!(job.state, JobState::Stopped, "{job:?}");
    // Control: `a_push_is_reported_as_running_and_updates_the_remote` reaches the remote.
    assert!(!f.remote_has_pushed());
}

#[test]
fn a_force_quit_completes_under_a_blocked_hook_and_leaves_the_queued_job_unstarted() {
    let b = blocking_repo();
    let harness = Harness::start();
    let _release = Release(&b.release);
    open(&harness, &b.repo);
    let mut seen = Seen::default();
    block_a_switch(&harness, &mut seen);
    harness.service.send(Intent::Switch("third".to_owned()));
    seen.until(&harness, |s| s.job(&switch_to("third")).is_some());

    harness.service.send(Intent::ForceQuit);
    seen.until(&harness, quit_seen);
    assert!(!b.release.exists());
    let running = seen.job(&switch_to("other")).unwrap();
    assert_eq!(running.state, JobState::Running, "{running:?}");

    // Force quit doesn't kill git: the running Job still finishes if the process lives on.
    std::fs::write(&b.release, "").unwrap();
    seen.until(&harness, |s| {
        s.job(&switch_to("other")).is_some_and(|j| is_ended(j))
    });
    let finished = seen.job(&switch_to("other")).unwrap();
    assert_eq!(finished.state, JobState::Done(Ok(())), "{finished:?}");
    // A queued Job starts as soon as the one before it ends, as
    // `local_write_jobs_run_one_at_a_time_in_order_and_both_finish` shows. So it would have
    // started within this window. Seen red: this fails if the queue isn't cleared.
    std::thread::sleep(SETTLE);
    assert_eq!(
        std::fs::read_to_string(&b.log).unwrap(),
        "start other\nend other\n"
    );
}

#[test]
fn a_force_quit_ends_a_waiting_quit() {
    let b = blocking_repo();
    let harness = Harness::start();
    let _release = Release(&b.release);
    open(&harness, &b.repo);
    let mut seen = Seen::default();
    block_a_switch(&harness, &mut seen);

    harness.service.send(Intent::Quit);
    harness.service.send(Intent::ForceQuit);
    seen.until(&harness, quit_seen);
    assert!(!b.release.exists());
}

#[test]
fn a_force_quit_after_a_finished_quit_publishes_nothing_more() {
    let (_dir, repo) = small_fixture_clone();
    let harness = Harness::start();
    open(&harness, &repo);
    harness.service.send(Intent::Quit);
    let mut seen = Seen::default();
    seen.until(&harness, quit_seen);

    // Such as the user confirming a Force quit just as the Job ends. `send` publishes a
    // Force quit's Snapshot before it returns, so there is nothing to wait for. Seen red:
    // without the `quit_published` check, this drains a second Quit.
    harness.service.send(Intent::ForceQuit);
    seen.drain_now(&harness);
    assert_eq!(seen.all.iter().filter(|s| is_quit(s)).count(), 1);
}

#[test]
fn dropping_the_service_after_a_force_quit_doesnt_wait_for_a_blocked_hook() {
    let b = blocking_repo();
    let harness = Harness::start();
    let _release = Release(&b.release);
    open(&harness, &b.repo);
    let mut seen = Seen::default();
    block_a_switch(&harness, &mut seen);
    harness.service.send(Intent::ForceQuit);
    seen.until(&harness, quit_seen);

    // On another thread, since a service that waits would wait on `_release`, which drops
    // only when this test ends. The app drops the service as it exits. Seen red: this times
    // out if the queue still joins its worker.
    let (dropped, wait) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        drop(harness);
        let _ = dropped.send(());
    });
    wait.recv_timeout(Duration::from_secs(10))
        .expect("dropping the service waited for the blocked hook");
    assert!(!b.release.exists());
}
