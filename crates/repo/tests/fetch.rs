//! Fetch Intent to a Network Job: progress, its own queue beside Local write Jobs, the
//! user's stop, and the refs refresh that follows.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use cairn_repo::{Intent, JobKind, JobState, RefsSnapshot, Snapshot};
use common::{
    Harness, Release, Seen, commit_graph_exists, git, git_read, install_blocking_hook, is_ended,
    lock_files, open, slow_remote, wait_until_network_job_is_waiting,
};

/// Where the remote-tracking branch `origin/<branch>` points in a refs Snapshot.
fn tracking_tip(refs: &RefsSnapshot, branch: &str) -> String {
    let name = format!("refs/remotes/origin/{branch}");
    refs.result
        .as_ref()
        .expect("refs read")
        .remote
        .iter()
        .find(|r| r.name.as_str() == name)
        .unwrap_or_else(|| panic!("{name} is missing"))
        .target
        .as_str()
        .to_owned()
}

#[test]
fn a_fetch_streams_progress_and_ends_with_a_refs_only_refresh() {
    let f = slow_remote();
    let harness = Harness::start();
    let _release = Release(&f.release);
    let opened = open(&harness, &f.repo);
    let graph_before = commit_graph_exists(&f.repo);

    harness.service.send(Intent::Fetch);
    let mut seen = Seen::default();
    // What the remote side writes is published while git is still running.
    wait_until_network_job_is_waiting(&mut seen, &harness, &JobKind::Fetch);

    std::fs::write(&f.release, "").unwrap();
    // The watcher may publish refs read mid-fetch, such as after FETCH_HEAD is written, so
    // wait for the Snapshot that shows the fetched tip.
    seen.until(&harness, |s| {
        s.job(&JobKind::Fetch).is_some_and(|j| is_ended(j))
            && s.refs_after(opened.refs)
                .is_some_and(|r| tracking_tip(r, &f.branch) == f.new)
    });
    let job = seen.job(&JobKind::Fetch).unwrap().clone();
    assert_eq!(job.state, JobState::Done(Ok(())), "{job:?}");
    // Progress reports come only with --progress, because stderr isn't a terminal.
    assert!(job.output.contains("Counting objects"), "{}", job.output);
    assert_eq!(
        git(&f.repo, &["rev-parse", &format!("origin/{}", f.branch)]).trim(),
        f.new
    );

    assert!(!graph_before, "the clone already had a commit-graph");
    assert!(
        commit_graph_exists(&f.repo),
        "the fetch wrote no commit-graph"
    );

    // Nothing marks a refresh that was never queued, so this waits. A refresh is queued
    // before the outcome is published, so a status refresh would publish within this
    // window. Control: `a_switch_runs_git_switch_and_refreshes_refs_and_status`.
    std::thread::sleep(Duration::from_millis(300));
    seen.drain_now(&harness);
    let status: Vec<_> = seen
        .all
        .iter()
        .filter(|s| match s {
            Snapshot::Staged(x) => x.generation > opened.staged,
            Snapshot::Unstaged(x) => x.generation > opened.unstaged,
            Snapshot::Untracked(x) => x.generation > opened.untracked,
            _ => false,
        })
        .collect();
    assert!(status.is_empty(), "a fetch refreshed status: {status:?}");
}

fn current_branch(repo: &Path) -> String {
    String::from_utf8(git_read(repo, &["symbolic-ref", "HEAD"]))
        .unwrap()
        .trim()
        .to_owned()
}

#[test]
fn a_local_write_job_completes_while_a_fetch_is_still_running() {
    let f = slow_remote();
    git(&f.repo, &["branch", "other"]);
    let harness = Harness::start();
    let _release = Release(&f.release);
    open(&harness, &f.repo);

    harness.service.send(Intent::Fetch);
    let mut seen = Seen::default();
    wait_until_network_job_is_waiting(&mut seen, &harness, &JobKind::Fetch);

    harness.service.send(Intent::Switch("other".to_owned()));
    let switch = JobKind::Switch {
        branch: "other".to_owned(),
    };
    seen.until(&harness, |s| s.job(&switch).is_some_and(|j| is_ended(j)));
    let switched = seen.job(&switch).unwrap();
    assert_eq!(switched.state, JobState::Done(Ok(())), "{switched:?}");
    // The wrapper hasn't been released, so the fetch can't have got past it.
    let fetch = seen.job(&JobKind::Fetch).unwrap();
    assert_eq!(fetch.state, JobState::Running, "{fetch:?}");
    assert!(!f.release.exists());

    std::fs::write(&f.release, "").unwrap();
    seen.until(&harness, |s| {
        s.job(&JobKind::Fetch).is_some_and(|j| is_ended(j))
    });
    let fetch = seen.job(&JobKind::Fetch).unwrap();
    assert_eq!(fetch.state, JobState::Done(Ok(())), "{fetch:?}");
    assert_eq!(
        git(&f.repo, &["rev-parse", &format!("origin/{}", f.branch)]).trim(),
        f.new
    );
}

#[test]
fn stopping_a_running_fetch_kills_git_refreshes_refs_and_leaves_the_repo_intact() {
    let f = slow_remote();
    let harness = Harness::start();
    let _release = Release(&f.release);
    let opened = open(&harness, &f.repo);

    harness.service.send(Intent::Fetch);
    let mut seen = Seen::default();
    wait_until_network_job_is_waiting(&mut seen, &harness, &JobKind::Fetch);
    let id = seen.job(&JobKind::Fetch).unwrap().id;

    harness.service.send(Intent::Stop(id));
    seen.until(&harness, |s| {
        s.job(&JobKind::Fetch).is_some_and(|j| is_ended(j)) && s.refs_after(opened.refs).is_some()
    });
    let job = seen.job(&JobKind::Fetch).unwrap();
    assert_eq!(job.state, JobState::Stopped, "{job:?}");
    assert!(job.output.contains("slow remote waiting"), "{}", job.output);
    // The refresh shows the true state: nothing was fetched.
    assert_eq!(
        tracking_tip(seen.refs_after(opened.refs).unwrap(), &f.branch),
        f.old
    );
    assert!(!f.release.exists(), "the wrapper was released, not stopped");

    assert_eq!(std::fs::read_to_string(&f.log).unwrap(), "run\n");
    assert_eq!(
        git(&f.repo, &["rev-parse", &format!("origin/{}", f.branch)]).trim(),
        f.old
    );
    git_read(&f.repo, &["fsck", "--no-progress", "--no-dangling"]);
    assert_eq!(lock_files(&f.repo.join(".git")), Vec::<PathBuf>::new());

    // Intact means usable: the next fetch gets the remote's commit.
    std::fs::write(&f.release, "").unwrap();
    harness.service.send(Intent::Fetch);
    seen.until(&harness, |s| {
        s.nth(1)
            .is_some_and(|j| j.kind == JobKind::Fetch && is_ended(j))
    });
    let again = seen.nth(1).unwrap();
    assert_eq!(again.state, JobState::Done(Ok(())), "{again:?}");
    assert_eq!(
        git(&f.repo, &["rev-parse", &format!("origin/{}", f.branch)]).trim(),
        f.new
    );
}

#[test]
fn network_jobs_run_one_at_a_time_and_a_queued_one_stops_without_starting() {
    let f = slow_remote();
    let harness = Harness::start();
    let _release = Release(&f.release);
    open(&harness, &f.repo);

    harness.service.send(Intent::Fetch);
    let mut seen = Seen::default();
    wait_until_network_job_is_waiting(&mut seen, &harness, &JobKind::Fetch);
    harness.service.send(Intent::Fetch);
    seen.until(&harness, |s| s.nth(1).is_some());
    let second = seen.nth(1).unwrap().clone();
    assert_eq!(second.state, JobState::Queued, "{second:?}");

    harness.service.send(Intent::Stop(second.id));
    seen.until(&harness, |s| s.nth(1).is_some_and(|j| is_ended(j)));
    assert_eq!(seen.nth(1).unwrap().state, JobState::Stopped);
    // It ended while the first was still running.
    assert_eq!(seen.nth(0).unwrap().state, JobState::Running);

    std::fs::write(&f.release, "").unwrap();
    seen.until(&harness, |s| s.nth(0).is_some_and(|j| is_ended(j)));
    assert_eq!(seen.nth(0).unwrap().state, JobState::Done(Ok(())));
    // The queue runs in order, so the third fetch runs only after the queue has dealt
    // with the stopped second one. Two runs means the second never reached the remote.
    harness.service.send(Intent::Fetch);
    seen.until(&harness, |s| s.nth(2).is_some_and(|j| is_ended(j)));
    assert_eq!(seen.nth(2).unwrap().state, JobState::Done(Ok(())));
    assert_eq!(std::fs::read_to_string(&f.log).unwrap(), "run\nrun\n");
    assert_eq!(seen.nth(1).unwrap().state, JobState::Stopped);
}

#[test]
fn a_stop_for_a_local_write_job_leaves_it_running_to_completion() {
    let f = slow_remote();
    git(&f.repo, &["branch", "other"]);
    let release = f.dir.path().join("hook-release");
    install_blocking_hook(&f.repo, &f.dir.path().join("hook.log"), &release);
    let harness = Harness::start();
    let _release = Release(&release);
    open(&harness, &f.repo);

    harness.service.send(Intent::Switch("other".to_owned()));
    let mut seen = Seen::default();
    seen.until(&harness, |s| {
        s.nth(0)
            .is_some_and(|j| j.progress.as_deref() == Some("hook waiting on other"))
    });
    // The Stop is handled before `send` returns, and the hook is still blocked then. A
    // stop that reached git would end the Job Stopped or with git's error, not Done(Ok).
    harness.service.send(Intent::Stop(seen.nth(0).unwrap().id));

    std::fs::write(&release, "").unwrap();
    seen.until(&harness, |s| s.nth(0).is_some_and(|j| is_ended(j)));
    assert_eq!(seen.nth(0).unwrap().state, JobState::Done(Ok(())));
    assert_eq!(current_branch(&f.repo), "refs/heads/other");
}
