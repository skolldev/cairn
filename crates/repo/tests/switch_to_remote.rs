//! Switch to remote Intent: switching to the local branch that tracks a remote branch, or
//! creating one with `git switch --track` when none does.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cairn_repo::{Intent, JobSnapshot, JobState, RefName};
use common::{Harness, git, git_output, git_read, is_ended, job_of, open, small_fixture_clone};

/// A clone of the small fixture with a remote-tracking branch `origin/feature/x` at the
/// commit before HEAD, and no local branch for it. HEAD stays on the default branch.
fn repo_with_remote_branch() -> (tempfile::TempDir, PathBuf, String) {
    let (dir, repo) = small_fixture_clone();
    let start = git(&repo, &["symbolic-ref", "--short", "HEAD"])
        .trim()
        .to_owned();
    git(
        &repo,
        &["update-ref", "refs/remotes/origin/feature/x", "HEAD~1"],
    );
    (dir, repo, start)
}

fn remote() -> RefName {
    RefName::new("refs/remotes/origin/feature/x")
}

/// Drains until the first Job this test sent has finished, and returns it.
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

fn local_branches(repo: &Path) -> Vec<String> {
    String::from_utf8(git_read(
        repo,
        &["for-each-ref", "--format=%(refname)", "refs/heads/"],
    ))
    .unwrap()
    .lines()
    .map(str::to_owned)
    .collect()
}

fn rev(repo: &Path, rev: &str) -> String {
    git(repo, &["rev-parse", rev]).trim().to_owned()
}

#[test]
fn a_remote_branch_with_no_local_branch_creates_one_tracking_it_and_switches_to_it() {
    let (_dir, repo, _start) = repo_with_remote_branch();
    let harness = Harness::start();
    open(&harness, &repo);

    harness.service.send(Intent::SwitchToRemote(remote()));
    let job = wait_for_done(&harness);

    assert_eq!(job.state, JobState::Done(Ok(())), "{job:?}");
    assert_eq!(current_branch(&repo), "refs/heads/feature/x");
    assert_eq!(rev(&repo, "HEAD"), rev(&repo, "origin/feature/x"));
    assert_eq!(
        git(&repo, &["config", "branch.feature/x.remote"]).trim(),
        "origin"
    );
    assert_eq!(
        git(&repo, &["config", "branch.feature/x.merge"]).trim(),
        "refs/heads/feature/x"
    );
}

#[test]
fn a_remote_branch_a_local_branch_tracks_switches_to_it_without_creating_anything() {
    let (_dir, repo, start) = repo_with_remote_branch();
    git(
        &repo,
        &[
            "branch",
            "--quiet",
            "--track",
            "feature/x",
            "origin/feature/x",
        ],
    );
    let before = local_branches(&repo);
    let harness = Harness::start();
    open(&harness, &repo);

    harness.service.send(Intent::SwitchToRemote(remote()));
    let job = wait_for_done(&harness);

    assert_eq!(job.state, JobState::Done(Ok(())), "{job:?}");
    assert_eq!(current_branch(&repo), "refs/heads/feature/x");
    assert_eq!(local_branches(&repo), before);
    assert!(before.contains(&format!("refs/heads/{start}")));
}

#[test]
fn of_several_local_branches_tracking_it_the_one_named_after_it_wins_and_else_the_first() {
    let (_dir, repo, _start) = repo_with_remote_branch();
    // `aaa` sorts before `feature/x`, and `zzz` after.
    for branch in ["aaa", "feature/x", "zzz"] {
        git(
            &repo,
            &["branch", "--quiet", "--track", branch, "origin/feature/x"],
        );
    }
    let harness = Harness::start();
    open(&harness, &repo);

    harness.service.send(Intent::SwitchToRemote(remote()));
    let job = wait_for_done(&harness);
    assert_eq!(job.state, JobState::Done(Ok(())), "{job:?}");
    // Seen red: taking the first tracking branch whatever its name switches to `aaa`.
    assert_eq!(current_branch(&repo), "refs/heads/feature/x");

    // With none named after it, the first in git's order.
    let start = git(&repo, &["rev-parse", "--abbrev-ref", "@{-1}"])
        .trim()
        .to_owned();
    git(&repo, &["switch", "--quiet", &start]);
    git(&repo, &["branch", "--quiet", "-D", "feature/x"]);
    let before = local_branches(&repo);
    harness.service.send(Intent::SwitchToRemote(remote()));
    let job = wait_for_done(&harness);
    assert_eq!(job.state, JobState::Done(Ok(())), "{job:?}");
    // Seen red: taking the tracking branches in reverse order switches to `zzz`.
    assert_eq!(current_branch(&repo), "refs/heads/aaa");
    assert_eq!(local_branches(&repo), before);
}

#[test]
fn a_local_branch_of_that_name_tracking_nothing_fails_with_gits_error_and_head_stays() {
    let (_dir, repo, start) = repo_with_remote_branch();
    // At HEAD, not at `origin/feature/x`, and with no upstream.
    git(&repo, &["branch", "--quiet", "--no-track", "feature/x"]);
    let before = local_branches(&repo);
    let harness = Harness::start();
    open(&harness, &repo);

    harness.service.send(Intent::SwitchToRemote(remote()));
    let job = wait_for_done(&harness);

    // Seen red: choosing a local branch by its name alone, whatever it tracks, switches.
    let JobState::Done(Err(error)) = &job.state else {
        panic!("git must refuse to create feature/x again: {job:?}");
    };
    assert_eq!(current_branch(&repo), format!("refs/heads/{start}"));
    assert_eq!(local_branches(&repo), before);
    assert!(
        git_output(&repo, &["config", "branch.feature/x.merge"])
            .stdout
            .is_empty()
    );
    let terminal = git_output(
        &repo,
        &["switch", "--track", "--", "refs/remotes/origin/feature/x"],
    );
    assert!(
        !terminal.status.success(),
        "the terminal switch must fail too"
    );
    assert_eq!(error, &terminal.stderr_text().into_owned());
    assert!(
        error.contains("a branch named 'feature/x' already exists"),
        "{error}"
    );
}
