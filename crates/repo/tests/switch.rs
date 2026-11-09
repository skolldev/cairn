//! Switch Intent to a Local write Job, its outcome, and the refresh that follows it.

mod common;

use std::path::Path;
use std::sync::Arc;

use cairn_repo::{ChangesSnapshot, Head, Intent, JobSnapshot, JobState, RefsSnapshot, Snapshot};
use common::{
    Harness, Release, Seen, git, git_output, git_read, install_blocking_hook, is_ended, job_of,
    open, small_fixture_clone,
};

/// A clone of the small fixture with a local branch `other` one commit ahead of the
/// default branch, which changes `README.md`. HEAD stays on the default branch.
fn repo_with_other_branch() -> (tempfile::TempDir, std::path::PathBuf, String) {
    let (dir, repo) = small_fixture_clone();
    let start = git(&repo, &["symbolic-ref", "--short", "HEAD"])
        .trim()
        .to_owned();
    git(&repo, &["switch", "--quiet", "-c", "other"]);
    std::fs::write(repo.join("README.md"), "on other\n").unwrap();
    git(
        &repo,
        &["commit", "--quiet", "-am", "change README on other"],
    );
    git(&repo, &["switch", "--quiet", &start]);
    (dir, repo, start)
}

/// The workdir a Read Slot's Snapshot answers, or `None` for any other.
fn read_workdir(snapshot: &Snapshot) -> Option<&Path> {
    match snapshot {
        Snapshot::Refs(s) => Some(&s.workdir),
        Snapshot::Staged(s) | Snapshot::Unstaged(s) => Some(&s.workdir),
        Snapshot::Untracked(s) => Some(&s.workdir),
        Snapshot::Job(_) | Snapshot::Quit => None,
    }
}

/// Drains until the first Local write Job this test sent has finished, and returns it.
fn wait_for_done(harness: &Harness) -> Arc<JobSnapshot> {
    let seen = harness.drain_until(|s| job_of(s).is_some_and(|j| is_ended(j)));
    seen.iter()
        .filter_map(job_of)
        .find(|j| is_ended(j))
        .unwrap()
        .clone()
}

fn current_branch(repo: &Path) -> String {
    String::from_utf8(git_read(repo, &["symbolic-ref", "HEAD"]))
        .unwrap()
        .trim()
        .to_owned()
}

#[test]
fn a_switch_runs_git_switch_and_refreshes_refs_and_status() {
    let (_dir, repo, _start) = repo_with_other_branch();
    let harness = Harness::start();
    let opened = open(&harness, &repo);

    harness.service.send(Intent::Switch("other".to_owned()));
    // The refresh may publish before the Job's outcome, and the watcher may publish status
    // read midway through the checkout. So drain until each Slot's newest Snapshot shows
    // the switch: refs at the new HEAD, and status empty after a clean switch.
    let on_other = |r: &RefsSnapshot| {
        r.result.as_ref().is_ok_and(|refs| {
            matches!(&refs.head, Head::Branch { name, .. } if name.as_str() == "refs/heads/other")
        })
    };
    let clean = |s: &ChangesSnapshot| s.result == Ok(Vec::new());
    let (mut job, mut refs, mut staged, mut unstaged, mut untracked) =
        (None, None, None, None, None);
    harness.drain_until(|s| {
        match s {
            Snapshot::Job(j) if is_ended(j) => job = Some(j.clone()),
            Snapshot::Refs(r) if r.generation > opened.refs => refs = Some(r.clone()),
            Snapshot::Staged(s) if s.generation > opened.staged => staged = Some(s.clone()),
            Snapshot::Unstaged(s) if s.generation > opened.unstaged => unstaged = Some(s.clone()),
            Snapshot::Untracked(s) if s.generation > opened.untracked => {
                untracked = Some(s.clone())
            }
            _ => {}
        }
        job.is_some()
            && refs.as_deref().is_some_and(on_other)
            && staged.as_deref().is_some_and(clean)
            && unstaged.as_deref().is_some_and(clean)
            && untracked
                .as_deref()
                .is_some_and(|u| u.result == Ok(Vec::new()))
    });
    let job = job.unwrap();
    assert_eq!(job.state, JobState::Done(Ok(())), "{job:?}");
    assert_eq!(current_branch(&repo), "refs/heads/other");
}

#[test]
fn local_write_jobs_run_one_at_a_time_in_order_and_both_finish() {
    let (dir, repo, start) = repo_with_other_branch();
    git(&repo, &["branch", "third", &start]);
    let log = dir.path().join("hook.log");
    let release = dir.path().join("release");
    install_blocking_hook(&repo, &log, &release);
    let harness = Harness::start();
    let _release = Release(&release);
    open(&harness, &repo);

    harness.service.send(Intent::Switch("other".to_owned()));
    let mut jobs = Seen::default();
    // The hook's output arrives while the hook is still blocked, so it is live.
    jobs.until(&harness, |j| {
        j.nth(0).is_some_and(|first| {
            first.state == JobState::Running && first.output.contains("hook waiting on other")
        })
    });

    harness.service.send(Intent::Switch("third".to_owned()));
    jobs.until(&harness, |j| j.nth(1).is_some());
    let second = jobs.nth(1).unwrap();
    assert_eq!(second.state, JobState::Queued, "{second:?}");
    assert_eq!(
        second.kind,
        cairn_repo::JobKind::Switch {
            branch: "third".to_owned()
        }
    );
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "start other\n");

    std::fs::write(&release, "").unwrap();
    jobs.until(&harness, |j| {
        j.jobs.len() == 2 && j.jobs.values().all(|job| is_ended(job))
    });
    for job in jobs.jobs.values() {
        assert_eq!(job.state, JobState::Done(Ok(())), "{job:?}");
    }
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        "start other\nend other\nstart third\nend third\n",
        "the second switch ran only after the first finished"
    );
    assert_eq!(current_branch(&repo), "refs/heads/third");
    assert!(!repo.join(".git/index.lock").exists());
}

/// What git itself says when asked to switch `repo` to `branch` from a terminal.
fn terminal_switch_error(repo: &Path, branch: &str) -> String {
    let out = git_output(repo, &["switch", "--no-guess", "--", branch]);
    assert!(!out.status.success(), "the terminal switch must fail too");
    out.stderr_text().into_owned()
}

#[test]
fn a_switch_beside_a_held_index_lock_publishes_gits_error_and_leaves_the_lock_alone() {
    let (_dir, repo, start) = repo_with_other_branch();
    let harness = Harness::start();
    open(&harness, &repo);
    let lock = repo.join(".git/index.lock");
    std::fs::write(&lock, "held by the terminal\n").unwrap();
    let modified = std::fs::metadata(&lock).unwrap().modified().unwrap();

    harness.service.send(Intent::Switch("other".to_owned()));
    let job = wait_for_done(&harness);

    let JobState::Done(Err(error)) = &job.state else {
        panic!("the switch must fail while the index is locked: {job:?}");
    };
    assert!(error.contains("index.lock"), "{error}");
    assert_eq!(error, &terminal_switch_error(&repo, "other"));
    assert_eq!(
        std::fs::read_to_string(&lock).unwrap(),
        "held by the terminal\n"
    );
    assert_eq!(
        std::fs::metadata(&lock).unwrap().modified().unwrap(),
        modified
    );
    assert_eq!(current_branch(&repo), format!("refs/heads/{start}"));
}

#[test]
fn a_timed_out_snapshot_wait_does_not_wait_for_a_blocked_local_write() {
    let (dir, repo, _) = repo_with_other_branch();
    let log = dir.path().join("hook.log");
    let release = dir.path().join("release");
    install_blocking_hook(&repo, &log, &release);
    let harness = Harness::start();
    let _release_on_panic = Release(&release);
    open(&harness, &repo);

    harness.service.send(Intent::Switch("other".to_owned()));
    let mut jobs = Seen::default();
    jobs.until(&harness, |j| {
        j.nth(0)
            .is_some_and(|job| job.output.contains("hook waiting on other"))
    });

    // Only a backstop, so a regression fails instead of hanging: nothing else releases the hook
    // until the service is gone. The marker below is what proves the drop didn't wait.
    let (cancel, cancelled) = std::sync::mpsc::channel::<()>();
    let release_later = release.clone();
    let watchdog = std::thread::spawn(move || {
        if cancelled
            .recv_timeout(std::time::Duration::from_secs(30))
            .is_err()
        {
            std::fs::write(release_later, "").unwrap();
        }
    });
    let timed_out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        harness.drain_until_with_timeout(std::time::Duration::from_millis(100), |_| false)
    }));
    assert!(timed_out.is_err());
    drop(harness);

    // Seen red: with `drain_until_with_timeout`'s Force quit removed, the drop waits until the
    // backstop releases the hook, and this fails.
    // Marker: the hook is still blocked once the drop has returned. Its `end` line can only
    // follow `release`, which nothing has written yet unless the backstop fired.
    let log_after_drop = std::fs::read_to_string(&log).unwrap();
    let backstop_fired = release.exists();
    let _ = cancel.send(());
    watchdog.join().unwrap();
    assert!(
        !backstop_fired && !log_after_drop.contains("end other"),
        "the drop waited for the blocked write: {log_after_drop:?}"
    );

    // Control: releasing the hook lets it end, so it really was blocked until then.
    std::fs::write(&release, "").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !std::fs::read_to_string(&log).unwrap().contains("end other") {
        assert!(
            std::time::Instant::now() < deadline,
            "the released hook never ended"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn a_switch_refused_for_local_changes_publishes_gits_message() {
    let (_dir, repo, start) = repo_with_other_branch();
    let harness = Harness::start();
    open(&harness, &repo);
    // `other` changes README.md, so git refuses to overwrite this edit.
    std::fs::write(repo.join("README.md"), "edited in the worktree\n").unwrap();

    harness.service.send(Intent::Switch("other".to_owned()));
    let job = wait_for_done(&harness);

    let JobState::Done(Err(error)) = &job.state else {
        panic!("git must refuse the switch: {job:?}");
    };
    assert!(error.contains("would be overwritten"), "{error}");
    assert_eq!(error, &terminal_switch_error(&repo, "other"));
    assert_eq!(current_branch(&repo), format!("refs/heads/{start}"));
    assert_eq!(
        std::fs::read_to_string(repo.join("README.md")).unwrap(),
        "edited in the worktree\n"
    );
}

#[test]
fn a_switch_never_creates_a_branch_from_a_remote_tracking_one() {
    let (_dir, repo, start) = repo_with_other_branch();
    git(
        &repo,
        &["update-ref", "refs/remotes/origin/only-remote", "HEAD"],
    );
    let harness = Harness::start();
    open(&harness, &repo);

    harness
        .service
        .send(Intent::Switch("only-remote".to_owned()));
    let job = wait_for_done(&harness);

    let JobState::Done(Err(error)) = &job.state else {
        panic!("only-remote is not a local branch: {job:?}");
    };
    assert_eq!(error, &terminal_switch_error(&repo, "only-remote"));
    assert_eq!(current_branch(&repo), format!("refs/heads/{start}"));
    assert!(
        git_output(
            &repo,
            &["rev-parse", "--verify", "-q", "refs/heads/only-remote"]
        )
        .stdout
        .is_empty()
    );
    // Control: a plain `git switch` would have created it from the remote-tracking branch.
    git(&repo, &["switch", "--quiet", "only-remote"]);
    assert_eq!(current_branch(&repo), "refs/heads/only-remote");
}

#[test]
fn a_switch_that_finishes_after_another_open_doesnt_refresh_the_new_repo_with_the_old() {
    let (dir, repo, _start) = repo_with_other_branch();
    let (_other_dir, newer) = small_fixture_clone();
    let log = dir.path().join("hook.log");
    let release = dir.path().join("release");
    install_blocking_hook(&repo, &log, &release);
    let harness = Harness::start();
    let _release = Release(&release);
    open(&harness, &repo);

    harness.service.send(Intent::Switch("other".to_owned()));
    let mut jobs = Seen::default();
    jobs.until(&harness, |j| {
        j.nth(0)
            .is_some_and(|job| job.output.contains("hook waiting on other"))
    });
    open(&harness, &newer);

    std::fs::write(&release, "").unwrap();
    // The Job still finishes, in the repo it was asked for.
    let mut late = Vec::new();
    late.extend(harness.drain_until(|s| job_of(s).is_some_and(|j| is_ended(j))));
    let done = late
        .iter()
        .filter_map(job_of)
        .find(|j| is_ended(j))
        .unwrap();
    assert_eq!(done.state, JobState::Done(Ok(())), "{done:?}");
    assert_eq!(done.workdir, repo);
    assert_eq!(current_branch(&repo), "refs/heads/other");

    // Nothing marks a refresh that was skipped, so this waits. A refresh is queued before
    // the outcome is published, so one that ran would publish within this window.
    // Control: `a_switch_runs_git_switch_and_refreshes_refs_and_status`.
    std::thread::sleep(std::time::Duration::from_millis(300));
    late.extend(harness.service.drain());
    let stale: Vec<_> = late
        .iter()
        .filter(|s| read_workdir(s).is_some_and(|w| w == repo))
        .collect();
    assert!(
        stale.is_empty(),
        "the old repo's refresh replaced the new one's: {stale:?}"
    );
}
