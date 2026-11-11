//! Stash and switch: offered on git's refusal to overwrite local changes, then a stash and the
//! switch, as two Local write Jobs, the second only after the first succeeded.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use cairn_repo::{Head, Intent, JobKind, JobSnapshot, JobState, RefName, Snapshot, SwitchTarget};
use common::{
    Harness, Release, Seen, block_first_run, git, git_output, git_read, is_ended, make_executable,
    open, refs_of, switch_to, wait_for_file,
};

/// A repository on `main` with a branch `other`, one commit ahead, that changes `file.txt`
/// and adds `added.txt`.
struct Fixture {
    dir: tempfile::TempDir,
    repo: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "--quiet", "-b", "main"]);
    std::fs::write(repo.join("file.txt"), "one\n").unwrap();
    git(&repo, &["add", "file.txt"]);
    git(&repo, &["commit", "--quiet", "-m", "one"]);
    git(&repo, &["switch", "--quiet", "-c", "other"]);
    std::fs::write(repo.join("file.txt"), "two\n").unwrap();
    std::fs::write(repo.join("added.txt"), "on other\n").unwrap();
    git(&repo, &["add", "file.txt", "added.txt"]);
    git(&repo, &["commit", "--quiet", "-m", "two"]);
    git(&repo, &["switch", "--quiet", "main"]);
    Fixture { dir, repo }
}

impl Fixture {
    /// A change to `file.txt`, which `other` changes too, so git refuses to switch.
    fn edit_tracked(&self) {
        std::fs::write(self.repo.join("file.txt"), "local\n").unwrap();
    }

    /// An untracked `added.txt`, which `other` tracks, so git refuses to switch.
    fn add_untracked(&self) {
        std::fs::write(self.repo.join("added.txt"), "local\n").unwrap();
    }

    fn head(&self) -> String {
        String::from_utf8(git_read(
            &self.repo,
            &["rev-parse", "--symbolic-full-name", "HEAD"],
        ))
        .unwrap()
        .trim()
        .to_owned()
    }

    /// `git stash list`'s subjects, newest first.
    fn stashes(&self) -> Vec<String> {
        String::from_utf8(git_read(&self.repo, &["stash", "list", "--format=%gs"]))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Makes `post-index-change` run `script`. Read Jobs never write the index, so only Local
    /// write Jobs run it: the stash first, several times.
    fn post_index_change(&self, script: &str) {
        let hook = self.repo.join(".git/hooks/post-index-change");
        std::fs::write(&hook, format!("#!/bin/sh\n{script}")).unwrap();
        make_executable(&hook);
    }
}

fn stash(untracked: bool) -> JobKind {
    JobKind::Stash { untracked }
}

fn stash_and_switch(branch: &str, untracked: bool) -> Intent {
    Intent::StashAndSwitch {
        to: SwitchTarget::Branch(branch.to_owned()),
        untracked,
    }
}

/// Sends a switch to `other`, waits for git to refuse it, and returns the failed Job.
fn refused_switch(harness: &Harness, seen: &mut Seen) -> Arc<JobSnapshot> {
    harness.service.send(Intent::Switch("other".to_owned()));
    seen.until(harness, |s| {
        s.job(&switch_to("other")).is_some_and(|j| is_ended(j))
    });
    let job = seen.job(&switch_to("other")).unwrap().clone();
    assert!(matches!(job.state, JobState::Done(Err(_))), "{job:?}");
    job
}

/// Waits until every Job seen so far has ended and there are `count` of them.
fn until_jobs_end(harness: &Harness, seen: &mut Seen, count: usize) {
    seen.until(harness, |s| {
        s.jobs.len() == count && s.jobs.values().all(|j| is_ended(j))
    });
}

/// The Jobs seen, oldest first, as their kind and state.
fn jobs(seen: &Seen) -> Vec<(JobKind, JobState)> {
    seen.jobs
        .values()
        .map(|j| (j.kind.clone(), j.state.clone()))
        .collect()
}

#[test]
fn a_switch_refused_for_tracked_changes_offers_a_stash_without_untracked_files_then_switches() {
    let f = fixture();
    f.edit_tracked();
    let harness = Harness::start();
    open(&harness, &f.repo);
    let mut seen = Seen::default();

    let refused = refused_switch(&harness, &mut seen);
    // Seen red: with the tracked refusal left out of `switch::refusal`, nothing is offered.
    // This pins git's words: reworded, they offer nothing.
    let offer = refused
        .stash_and_switch()
        .expect("a refusal for local changes offers it");
    assert!(
        matches!(&offer, Intent::StashAndSwitch { to: SwitchTarget::Branch(branch), untracked: false } if branch == "other"),
        "{offer:?}"
    );

    harness.service.send(offer);
    until_jobs_end(&harness, &mut seen, 3);
    assert_eq!(
        jobs(&seen)[1..],
        [
            (stash(false), JobState::Done(Ok(()))),
            (switch_to("other"), JobState::Done(Ok(()))),
        ]
    );
    assert_eq!(f.head(), "refs/heads/other");
    assert_eq!(f.stashes(), ["On main: WIP on main"]);
    // The stash appears in the refs Snapshot, as the sidebar's Stashes show it.
    seen.until(&harness, |s| {
        let refs = s.all.iter().rev().find_map(refs_of);
        refs.and_then(|r| r.result.as_ref().ok()).is_some_and(|refs| {
            refs.stashes.first().is_some_and(|stash| stash.message == "On main: WIP on main")
                && matches!(&refs.head, Head::Branch { name, .. } if name.as_str() == "refs/heads/other")
        })
    });
}

#[test]
fn a_switch_refused_for_untracked_files_the_branch_tracks_offers_a_stash_with_them() {
    let f = fixture();
    f.add_untracked();
    let harness = Harness::start();
    open(&harness, &f.repo);
    let mut seen = Seen::default();

    let refused = refused_switch(&harness, &mut seen);
    // Seen red: with the untracked refusal left unread, this is `untracked: false`. This pins
    // git's words: reworded, they offer nothing.
    let offer = refused
        .stash_and_switch()
        .expect("a refusal for untracked files offers it");
    assert!(
        matches!(&offer, Intent::StashAndSwitch { to: SwitchTarget::Branch(branch), untracked: true } if branch == "other"),
        "{offer:?}"
    );

    harness.service.send(offer);
    until_jobs_end(&harness, &mut seen, 3);
    assert_eq!(
        jobs(&seen)[1..],
        [
            (stash(true), JobState::Done(Ok(()))),
            (switch_to("other"), JobState::Done(Ok(()))),
        ]
    );
    assert_eq!(f.head(), "refs/heads/other");
    assert_eq!(f.stashes(), ["On main: WIP on main"]);
    // The untracked file went into the stash's third parent, where `-u` puts it. Seen red:
    // without `-u`, the switch is refused again.
    assert_eq!(
        git_read(&f.repo, &["show", "stash@{0}^3:added.txt"]),
        b"local\n"
    );
}

#[test]
fn a_switch_refused_for_both_kinds_of_change_offers_a_stash_with_untracked_files() {
    let f = fixture();
    f.edit_tracked();
    f.add_untracked();
    let harness = Harness::start();
    open(&harness, &f.repo);
    let mut seen = Seen::default();

    let refused = refused_switch(&harness, &mut seen);
    let JobState::Done(Err(error)) = &refused.state else {
        unreachable!()
    };
    // Both refusals, so the offer must look past the first. Seen red: reading only the first
    // line of git's error offers nothing.
    assert!(error.contains("Your local changes"), "{error}");
    assert!(error.contains("untracked working tree files"), "{error}");
    assert!(
        matches!(
            refused.stash_and_switch(),
            Some(Intent::StashAndSwitch {
                untracked: true,
                ..
            })
        ),
        "{refused:?}"
    );
}

#[test]
fn a_switch_that_fails_for_another_reason_offers_no_stash_and_switch() {
    let f = fixture();
    let harness = Harness::start();
    open(&harness, &f.repo);
    let mut seen = Seen::default();

    // Not a local branch. Control: the refusals above offer it.
    harness
        .service
        .send(Intent::Switch("no-such-branch".to_owned()));
    seen.until(&harness, |s| {
        s.job(&switch_to("no-such-branch"))
            .is_some_and(|j| is_ended(j))
    });
    let failed = seen.job(&switch_to("no-such-branch")).unwrap();
    assert!(matches!(failed.state, JobState::Done(Err(_))), "{failed:?}");
    assert!(failed.stash_and_switch().is_none(), "{failed:?}");
}

/// Sends a Stash and switch to `other`, waits for the stash to end, then sends a switch to
/// `marker` and waits for that to end too. Local write Jobs run in order, so a switch the
/// stash started would come between them. Returns the stash's error.
fn failed_stash_then_marker(harness: &Harness, seen: &mut Seen, marker: &str) -> String {
    harness.service.send(stash_and_switch("other", false));
    seen.until(harness, |s| {
        s.job(&stash(false)).is_some_and(|j| is_ended(j))
    });
    let stashed = seen.job(&stash(false)).unwrap().clone();
    let JobState::Done(Err(error)) = &stashed.state else {
        panic!("the stash must fail: {stashed:?}");
    };
    harness.service.send(Intent::Switch(marker.to_owned()));
    seen.until(harness, |s| {
        s.job(&switch_to(marker)).is_some_and(|j| is_ended(j))
    });
    let kinds: Vec<_> = jobs(seen).into_iter().map(|(kind, _)| kind).collect();
    assert_eq!(kinds, [stash(false), switch_to(marker)], "{:?}", jobs(seen));
    error.clone()
}

#[test]
fn a_stash_that_fails_shows_gits_error_and_never_starts_the_switch() {
    let f = fixture();
    // Unmerged paths, from a merge that conflicts in `file.txt`.
    git(&f.repo, &["switch", "--quiet", "-c", "clash"]);
    std::fs::write(f.repo.join("file.txt"), "three\n").unwrap();
    git(&f.repo, &["commit", "--quiet", "-am", "three"]);
    let merge = git_output(&f.repo, &["merge", "--quiet", "other"]);
    assert!(!merge.status.success(), "the merge must conflict");
    let harness = Harness::start();
    open(&harness, &f.repo);
    let mut seen = Seen::default();

    // Seen red: with `start_next` called whatever the stash's outcome, a switch to other
    // comes before the marker.
    let error = failed_stash_then_marker(&harness, &mut seen, "clash");
    // git writes this to stdout. Newer git adds `error: could not write index` on stderr
    // (`docs/git-versions.md`). Seen red: an error from stderr alone lacks it.
    assert!(error.starts_with("file.txt: needs merge\n"), "{error}");
    assert_eq!(f.head(), "refs/heads/clash");
    assert!(f.stashes().is_empty());
}

#[test]
fn a_stash_that_fails_on_a_held_index_lock_says_so_even_when_git_says_nothing() {
    let f = fixture();
    f.edit_tracked();
    let harness = Harness::start();
    open(&harness, &f.repo);
    let lock = f.repo.join(".git/index.lock");
    std::fs::write(&lock, "held by the terminal\n").unwrap();
    let mut seen = Seen::default();

    let error = failed_stash_then_marker(&harness, &mut seen, "main");
    // git 2.37 says nothing on either stream for a held index lock, and newer git names the
    // lock on stderr (`docs/git-versions.md`). The terminal's own stash tells which this is.
    // Seen red, on 2.37: without the fallback, the error is empty.
    let terminal = git_output(&f.repo, &["stash", "push", "-m", "from the terminal"]);
    assert!(!terminal.status.success());
    if terminal.stdout.is_empty() && terminal.stderr.is_empty() {
        assert!(error.starts_with("git stash push failed with "), "{error}");
    } else {
        assert!(error.contains("index.lock"), "{error}");
    }
    assert_eq!(f.head(), "refs/heads/main");
    assert_eq!(
        std::fs::read_to_string(&lock).unwrap(),
        "held by the terminal\n"
    );
}

#[test]
fn a_switch_that_fails_after_the_stash_keeps_the_stash_and_offers_nothing_more() {
    let f = fixture();
    f.edit_tracked();
    f.add_untracked();
    let harness = Harness::start();
    open(&harness, &f.repo);
    let mut seen = Seen::default();

    // Without untracked files, so `added.txt` stays in the switch's way.
    harness.service.send(stash_and_switch("other", false));
    until_jobs_end(&harness, &mut seen, 2);
    let stashed = seen.nth(0).unwrap();
    assert_eq!(stashed.state, JobState::Done(Ok(())), "{stashed:?}");
    let switched = seen.nth(1).unwrap();
    let JobState::Done(Err(error)) = &switched.state else {
        panic!("git must refuse the switch for the untracked file: {switched:?}");
    };
    assert!(error.contains("untracked working tree files"), "{error}");
    assert_eq!(switched.follows, Some(stashed.id));
    // Seen red: with the offer's `follows` check removed, this offers a second stash.
    // Control: the same refusal on a plain switch offers it, as above.
    assert!(switched.stash_and_switch().is_none(), "{switched:?}");
    assert_eq!(f.head(), "refs/heads/main");
    assert_eq!(f.stashes(), ["On main: WIP on main"]);
    assert_eq!(
        std::fs::read_to_string(f.repo.join("file.txt")).unwrap(),
        "one\n",
        "the stash isn't popped"
    );
}

#[test]
fn with_nothing_left_to_stash_the_switch_still_runs() {
    let f = fixture();
    let harness = Harness::start();
    open(&harness, &f.repo);
    let mut seen = Seen::default();

    harness.service.send(stash_and_switch("other", false));
    until_jobs_end(&harness, &mut seen, 2);
    assert_eq!(
        jobs(&seen),
        [
            (stash(false), JobState::Done(Ok(()))),
            (switch_to("other"), JobState::Done(Ok(()))),
        ]
    );
    assert_eq!(f.head(), "refs/heads/other");
    assert!(f.stashes().is_empty());
}

#[test]
fn from_a_detached_head_the_stash_is_named_for_its_short_sha() {
    let f = fixture();
    git(&f.repo, &["switch", "--quiet", "--detach", "main"]);
    let sha = String::from_utf8(git_read(&f.repo, &["rev-parse", "--short", "HEAD"]))
        .unwrap()
        .trim()
        .to_owned();
    f.edit_tracked();
    let harness = Harness::start();
    open(&harness, &f.repo);
    let mut seen = Seen::default();

    harness.service.send(stash_and_switch("other", false));
    until_jobs_end(&harness, &mut seen, 2);
    assert_eq!(seen.nth(1).unwrap().state, JobState::Done(Ok(())));
    assert_eq!(f.head(), "refs/heads/other");
    // Seen red: naming it from `symbolic-ref` alone gives `WIP on `.
    assert_eq!(f.stashes(), [format!("On (no branch): WIP on {sha}")]);
}

#[test]
fn a_quit_while_the_stash_runs_lets_it_finish_and_never_starts_the_switch() {
    let f = fixture();
    f.edit_tracked();
    let started = f.dir.path().join("started");
    let release = f.dir.path().join("release");
    f.post_index_change(&block_first_run(&started, &release));
    let harness = Harness::start();
    let _release = Release(&release);
    open(&harness, &f.repo);
    let mut seen = Seen::default();

    harness.service.send(stash_and_switch("other", false));
    wait_for_file(&started);
    harness.service.send(Intent::Quit);
    std::fs::write(&release, "").unwrap();

    // Marker: the Quit follows the final Snapshot of every Job it waited for, so a switch
    // the stash started would be seen before it. Control: the same Stash and switch without
    // a Quit switches, above. Seen red: with `start_next`'s open-repository check removed,
    // the switch runs here.
    seen.until(&harness, |s| {
        s.all.iter().any(|s| matches!(s, Snapshot::Quit))
    });
    assert_eq!(jobs(&seen), [(stash(false), JobState::Done(Ok(())))]);
    assert_eq!(f.head(), "refs/heads/main");
    assert_eq!(f.stashes(), ["On main: WIP on main"]);
}

/// Gives the fixture a remote `origin` whose `feature/x` is at `other`, with no local branch
/// for it. The remote's URL is never fetched from.
fn remote_feature_x(f: &Fixture) -> RefName {
    git(&f.repo, &["remote", "add", "origin", "unused"]);
    git(
        &f.repo,
        &["update-ref", "refs/remotes/origin/feature/x", "other"],
    );
    RefName::new("refs/remotes/origin/feature/x")
}

#[test]
fn a_switch_to_a_remote_branch_refused_for_local_changes_offers_a_stash_then_that_switch() {
    let f = fixture();
    let remote = remote_feature_x(&f);
    f.edit_tracked();
    let harness = Harness::start();
    open(&harness, &f.repo);
    let mut seen = Seen::default();
    let to_remote = JobKind::SwitchToRemote {
        remote: remote.clone(),
    };

    harness.service.send(Intent::SwitchToRemote(remote.clone()));
    seen.until(&harness, |s| s.job(&to_remote).is_some_and(|j| is_ended(j)));
    let refused = seen.job(&to_remote).unwrap().clone();
    let JobState::Done(Err(error)) = &refused.state else {
        panic!("git must refuse to overwrite file.txt: {refused:?}");
    };
    assert!(error.contains("Your local changes"), "{error}");
    // git checks the worktree before it makes the branch.
    assert!(
        git_output(
            &f.repo,
            &["rev-parse", "--verify", "-q", "refs/heads/feature/x"]
        )
        .stdout
        .is_empty()
    );
    let offer = refused
        .stash_and_switch()
        .expect("a refusal for local changes offers it");
    assert!(
        matches!(
            &offer,
            Intent::StashAndSwitch { to: SwitchTarget::Remote(r), untracked: false } if *r == remote
        ),
        "{offer:?}"
    );

    harness.service.send(offer);
    until_jobs_end(&harness, &mut seen, 3);
    assert_eq!(
        jobs(&seen)[1..],
        [
            (stash(false), JobState::Done(Ok(()))),
            (to_remote, JobState::Done(Ok(()))),
        ]
    );
    assert_eq!(f.head(), "refs/heads/feature/x");
    assert_eq!(
        git(&f.repo, &["config", "branch.feature/x.merge"]).trim(),
        "refs/heads/feature/x"
    );
    assert_eq!(f.stashes(), ["On main: WIP on main"]);
}
