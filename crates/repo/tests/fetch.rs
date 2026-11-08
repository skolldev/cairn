//! Fetch Intent to a Network Job: progress, its own queue beside Local write Jobs, the
//! user's stop, and the refs refresh that follows.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use cairn_repo::{Intent, JobKind, JobState, RefsSnapshot, Snapshot};
use common::{Harness, Release, Seen, git, git_read, install_blocking_hook, is_ended, open, slash};

/// A bare "remote" cloned from the small fixture, a clone of it whose origin is slowed by
/// a wrapper around `upload-pack`, and a commit on the remote that the clone hasn't
/// fetched yet.
struct Fixture {
    dir: tempfile::TempDir,
    repo: PathBuf,
    branch: String,
    /// The remote's branch tip before and after its new commit.
    old: String,
    new: String,
    /// The wrapper waits for this file before it lets `upload-pack` run.
    release: PathBuf,
    /// One line per wrapper run.
    log: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let remote = dir.path().join("remote.git");
    let repo = dir.path().join("copy");
    let fixture = common::small_fixture();
    let path = |p: &Path| p.to_str().unwrap().to_owned();
    git(
        dir.path(),
        &[
            "clone",
            "--quiet",
            "--bare",
            &path(&fixture),
            &path(&remote),
        ],
    );
    git(
        dir.path(),
        &["clone", "--quiet", &path(&remote), &path(&repo)],
    );
    // A local clone copies the fixture's commit-graph. Remove it, so that one after the
    // fetch is the fetch's.
    let info = repo.join(".git/objects/info");
    let _ = std::fs::remove_file(info.join("commit-graph"));
    let _ = std::fs::remove_dir_all(info.join("commit-graphs"));
    let branch = git(&repo, &["symbolic-ref", "--short", "HEAD"])
        .trim()
        .to_owned();
    let old = git(&remote, &["rev-parse", &branch]).trim().to_owned();
    let new = git(
        &remote,
        &[
            "commit-tree",
            &format!("{old}^{{tree}}"),
            "-p",
            &old,
            "-m",
            "on the remote",
        ],
    )
    .trim()
    .to_owned();
    git(
        &remote,
        &["update-ref", &format!("refs/heads/{branch}"), &new],
    );

    let release = dir.path().join("release");
    let log = dir.path().join("upload-pack.log");
    let wrapper = dir.path().join("slow-upload-pack.sh");
    std::fs::write(
        &wrapper,
        format!(
            "echo run >> '{log}'\n\
             echo 'slow remote waiting' >&2\n\
             while [ ! -f '{release}' ]; do sleep 0.05; done\n\
             exec git upload-pack \"$@\"\n",
            log = slash(&log),
            release = slash(&release),
        ),
    )
    .unwrap();
    git(
        &repo,
        &[
            "config",
            "remote.origin.uploadpack",
            &format!("sh '{}'", slash(&wrapper)),
        ],
    );
    Fixture {
        dir,
        repo,
        branch,
        old,
        new,
        release,
        log,
    }
}

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

/// Waits until the fetch is blocked in the slow remote, with its line already published.
fn wait_until_fetch_is_waiting(seen: &mut Seen, harness: &Harness) {
    seen.until(harness, |s| {
        s.job(&JobKind::Fetch).is_some_and(|j| {
            j.state == JobState::Running && j.progress.as_deref() == Some("slow remote waiting")
        })
    });
}

#[test]
fn a_fetch_streams_progress_and_ends_with_a_refs_only_refresh() {
    let f = fixture();
    let harness = Harness::start();
    let _release = Release(&f.release);
    let opened = open(&harness, &f.repo);
    let graph_before = commit_graph_exists(&f.repo);

    harness.service.send(Intent::Fetch);
    let mut seen = Seen::default();
    // What the remote side writes is published while git is still running.
    wait_until_fetch_is_waiting(&mut seen, &harness);

    std::fs::write(&f.release, "").unwrap();
    seen.until(&harness, |s| {
        s.job(&JobKind::Fetch).is_some_and(|j| is_ended(j)) && s.refs_after(opened.refs).is_some()
    });
    let job = seen.job(&JobKind::Fetch).unwrap().clone();
    assert_eq!(job.state, JobState::Done(Ok(())), "{job:?}");
    // Progress reports come only with --progress, because stderr isn't a terminal. Before
    // 2.37, `--all` fetches even a lone remote in a sub-fetch that doesn't get --progress.
    let installed = cairn_write::Git::shared().unwrap().version();
    if installed >= version(2, 37) {
        assert!(job.output.contains("Counting objects"), "{}", job.output);
    }
    assert_eq!(
        tracking_tip(seen.refs_after(opened.refs).unwrap(), &f.branch),
        f.new
    );
    assert_eq!(
        git(&f.repo, &["rev-parse", &format!("origin/{}", f.branch)]).trim(),
        f.new
    );

    // git 2.24 introduced fetch.writeCommitGraph; older git ignores it.
    if installed >= version(2, 24) {
        assert!(!graph_before, "the clone already had a commit-graph");
        assert!(
            commit_graph_exists(&f.repo),
            "the fetch wrote no commit-graph"
        );
    }

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

fn version(major: u32, minor: u32) -> cairn_write::GitVersion {
    cairn_write::GitVersion {
        major,
        minor,
        patch: 0,
    }
}

fn commit_graph_exists(repo: &Path) -> bool {
    let info = repo.join(".git/objects/info");
    info.join("commit-graph").exists() || info.join("commit-graphs/commit-graph-chain").exists()
}

#[test]
fn a_local_write_job_completes_while_a_fetch_is_still_running() {
    let f = fixture();
    git(&f.repo, &["branch", "other"]);
    let harness = Harness::start();
    let _release = Release(&f.release);
    open(&harness, &f.repo);

    harness.service.send(Intent::Fetch);
    let mut seen = Seen::default();
    wait_until_fetch_is_waiting(&mut seen, &harness);

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

/// Lock files git left anywhere under `.git`.
fn lock_files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(lock_files(&path));
        } else if path.extension().is_some_and(|e| e == "lock") {
            found.push(path);
        }
    }
    found
}

#[test]
fn stopping_a_running_fetch_kills_git_refreshes_refs_and_leaves_the_repo_intact() {
    let f = fixture();
    let harness = Harness::start();
    let _release = Release(&f.release);
    let opened = open(&harness, &f.repo);

    harness.service.send(Intent::Fetch);
    let mut seen = Seen::default();
    wait_until_fetch_is_waiting(&mut seen, &harness);
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
    let f = fixture();
    let harness = Harness::start();
    let _release = Release(&f.release);
    open(&harness, &f.repo);

    harness.service.send(Intent::Fetch);
    let mut seen = Seen::default();
    wait_until_fetch_is_waiting(&mut seen, &harness);
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
    let f = fixture();
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
