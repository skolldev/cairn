//! Open Intent to refs Snapshot, checked against real git.

mod common;

use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;

use cairn_repo::{Head, Intent, JobState, RefEntry, Refs, RefsSnapshot, Snapshot};
use common::{
    Harness, assert_filesystem_events_available, git, git_output, git_ran, git_runs, small_fixture,
    small_fixture_clone,
};

fn refs_of(snapshot: &Snapshot) -> Option<&Arc<RefsSnapshot>> {
    match snapshot {
        Snapshot::Refs(refs) => Some(refs),
        _ => None,
    }
}

/// The newest refs Snapshot in `seen`. A drain returns the status Slots' Snapshots after
/// the refs one, so the refs Snapshot isn't always the last thing drained.
fn last_refs(seen: &[Snapshot]) -> Arc<RefsSnapshot> {
    seen.iter()
        .rev()
        .find_map(refs_of)
        .expect("a refs Snapshot")
        .clone()
}

/// Opens `path` and returns the refs Snapshot that answers the Open.
fn open(path: &Path) -> Arc<RefsSnapshot> {
    let harness = Harness::start();
    harness.service.send(Intent::Open(path.to_path_buf()));
    let seen = harness.drain_until(|s| refs_of(s).is_some());
    last_refs(&seen)
}

/// One line per ref, in the shape `git for-each-ref` prints with the same format.
fn render(entries: &[RefEntry]) -> Vec<String> {
    entries
        .iter()
        .map(|e| {
            format!(
                "{}\0{}\0{}\0{}",
                e.name.as_str(),
                e.target.as_str(),
                e.peeled.as_ref().map_or("", |p| p.as_str()),
                e.symref.as_ref().map_or("", |s| s.as_str()),
            )
        })
        .collect()
}

fn for_each_ref(repo: &Path, prefix: &str) -> Vec<String> {
    git(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(*objectname)%00%(symref)",
            prefix,
        ],
    )
    .lines()
    .map(str::to_owned)
    .collect()
}

/// Asserts that every field of `refs` matches what git itself reports for `repo`.
fn assert_matches_git(repo: &Path, refs: &Refs) {
    assert_eq!(
        render(&refs.local),
        for_each_ref(repo, "refs/heads/"),
        "local branches"
    );
    assert_eq!(
        render(&refs.remote),
        for_each_ref(repo, "refs/remotes/"),
        "remote branches"
    );
    assert_eq!(render(&refs.tags), for_each_ref(repo, "refs/tags/"), "tags");

    let stash_list = git(repo, &["stash", "list", "--format=%H%x00%gs"]);
    let stashes: Vec<String> = refs
        .stashes
        .iter()
        .map(|s| format!("{}\0{}", s.target.as_str(), s.message))
        .collect();
    assert_eq!(stashes, stash_list.lines().collect::<Vec<_>>(), "stashes");

    let head_oid = git(repo, &["rev-parse", "HEAD"]).trim().to_owned();
    match &refs.head {
        Head::Branch { name, target } => {
            assert_eq!(name.as_str(), git(repo, &["symbolic-ref", "HEAD"]).trim());
            assert_eq!(target.as_ref().map(|t| t.as_str()), Some(head_oid.as_str()));
        }
        Head::Detached { target } => {
            let symbolic = git_output(repo, &["symbolic-ref", "-q", "HEAD"]);
            assert!(!symbolic.status.success(), "git says HEAD is on a branch");
            assert_eq!(target.as_str(), head_oid);
        }
    }
}

#[test]
fn refs_snapshot_of_the_small_fixture_matches_git_for_each_ref() {
    let fixture = small_fixture();
    let snapshot = open(&fixture);
    let refs = snapshot.result.as_ref().expect("the fixture opens");
    assert!(!refs.tags.is_empty(), "the fixture has tags");
    assert_matches_git(&fixture, refs);
}

#[test]
fn cli_refs_path_matches_git_for_each_ref_head_and_stash() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("copy");
    git(
        dir.path(),
        &[
            "clone",
            "--quiet",
            small_fixture().to_str().unwrap(),
            repo.to_str().unwrap(),
        ],
    );
    git(&repo, &["config", "core.safecrlf", "true"]);
    git(&repo, &["tag", "-a", "cli-tag", "-m", "tag"]);
    std::fs::write(repo.join("README.md"), b"changed\n").unwrap();
    git(&repo, &["stash", "push", "--quiet", "-m", "cli stash"]);
    let snapshot = open(&repo);
    assert_matches_git(&repo, snapshot.result.as_ref().unwrap());
}

#[test]
fn malformed_stash_reflog_retries_refs_with_git_and_latches_cli() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("copy");
    git(
        dir.path(),
        &[
            "clone",
            "--quiet",
            small_fixture().to_str().unwrap(),
            repo.to_str().unwrap(),
        ],
    );
    let original = git(&repo, &["symbolic-ref", "--short", "HEAD"])
        .trim()
        .to_owned();
    git(&repo, &["branch", "fallback-other"]);
    std::fs::write(repo.join("README.md"), b"changed\n").unwrap();
    git(&repo, &["stash", "push", "--quiet", "-m", "fallback probe"]);
    let log = repo.join(".git/logs/refs/stash");
    let valid_log = std::fs::read(&log).unwrap();
    // gix rejects this reflog line; `git stash list` ignores it. It is a real
    // repository difference that exercises same-call CLI retry at the public seam.
    std::fs::OpenOptions::new()
        .append(true)
        .open(&log)
        .unwrap()
        .write_all(b"malformed\n")
        .unwrap();

    let harness = Harness::start();
    harness.service.send(Intent::Open(repo.clone()));
    let first = harness.drain_until(|s| refs_of(s).is_some());
    let first = last_refs(&first);
    // Seen red: returning the gix error instead of retrying through git fails here.
    assert_matches_git(
        &repo,
        first.result.as_ref().expect("git retries a gix refs error"),
    );

    let mut previous = first.generation;
    harness
        .service
        .send(Intent::Switch("fallback-other".to_owned()));
    let seen = harness.drain_until(|snapshot| match snapshot {
        Snapshot::Refs(refs) if refs.generation > previous => refs.result.as_ref().is_ok_and(
            |refs| matches!(&refs.head, Head::Branch { name, .. } if name.as_str() == "refs/heads/fallback-other"),
        ),
        _ => false,
    });
    let second = last_refs(&seen);
    previous = second.generation;
    assert_matches_git(
        &repo,
        second
            .result
            .as_ref()
            .expect("second gix error retries through git"),
    );

    // Now gix can read Refs, while Git's stash command fails on this date format.
    // A newly opened service proves that control through the same Snapshot seam.
    std::fs::write(&log, valid_log).unwrap();
    git(&repo, &["config", "log.date", "bogus"]);
    let control = open(&repo);
    assert!(control.result.is_ok(), "gix reads Refs with this config");

    harness.service.send(Intent::Switch(original));
    let mut switched = false;
    let mut git_read_it = false;
    // Wait for this error, not the next Snapshot: the watcher's refresh after the reflog write
    // can read through git before `log.date` is set, and succeed. Such a Snapshot says nothing
    // about the latch. Nor does another error: on Windows, a refresh racing `git config` above
    // can fail with `Permission denied` on `.git/config`. Only git reports this error, so its
    // arrival shows git did this read.
    // Seen red: removing the failure latch means gix reads Refs from here on, and this times out.
    harness.drain_until(|snapshot| {
        match snapshot {
            Snapshot::Job(job) if job.state == JobState::Done(Ok(())) => switched = true,
            Snapshot::Refs(refs)
                if refs.generation > previous
                    && refs
                        .result
                        .as_ref()
                        .is_err_and(|e| e.contains("unknown date format bogus")) =>
            {
                git_read_it = true;
            }
            _ => {}
        }
        switched && git_read_it
    });
}

#[test]
fn refs_snapshot_covers_branches_stashes_annotated_tags_and_symrefs() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("copy");
    git(
        dir.path(),
        &[
            "clone",
            "--quiet",
            small_fixture().to_str().unwrap(),
            repo.to_str().unwrap(),
        ],
    );
    git(&repo, &["switch", "--quiet", "-c", "feature/one"]);
    git(&repo, &["branch", "second"]);
    git(
        &repo,
        &["tag", "-a", "annotated-test", "-m", "an annotated tag"],
    );
    git(&repo, &["tag", "lightweight-test"]);
    for (i, message) in ["first stash", "second stash"].iter().enumerate() {
        std::fs::write(repo.join("README.md"), format!("changed {i}\n")).unwrap();
        git(&repo, &["stash", "push", "--quiet", "-m", message]);
    }

    let snapshot = open(&repo);
    let refs = snapshot.result.as_ref().expect("the copy opens");
    assert_eq!(refs.stashes.len(), 2);
    assert!(
        refs.remote.iter().any(|r| r.symref.is_some()),
        "origin/HEAD is symbolic"
    );
    assert!(
        refs.tags.iter().any(|t| t.peeled.is_some()),
        "an annotated tag peels"
    );
    assert_matches_git(&repo, refs);
}

#[test]
fn each_refs_snapshot_carries_a_newer_generation() {
    let harness = Harness::start();
    let fixture = small_fixture();
    harness.service.send(Intent::Open(fixture.clone()));
    let first = harness.drain_until(|s| refs_of(s).is_some());
    let first = last_refs(&first);

    harness.service.send(Intent::Open(fixture));
    let second = harness.drain_until(|s| refs_of(s).is_some());
    let second = last_refs(&second);

    assert!(second.generation > first.generation);
}

#[test]
fn opening_a_path_that_is_not_a_repo_publishes_the_error() {
    let dir = tempfile::tempdir().unwrap();
    let snapshot = open(dir.path());
    let error = snapshot.result.as_ref().expect_err("not a repository");
    assert!(!error.is_empty());
    assert_eq!(snapshot.workdir, dir.path());
}

#[test]
fn a_newer_open_supersedes_older_ones_and_no_stale_generation_follows_it() {
    let harness = Harness::start();
    let fixture = small_fixture();
    let missing = tempfile::tempdir().unwrap();
    for _ in 0..20 {
        harness
            .service
            .send(Intent::Open(missing.path().to_path_buf()));
    }
    harness.service.send(Intent::Open(fixture.clone()));

    let seen = harness.drain_until(|s| refs_of(s).is_some_and(|r| r.workdir == fixture));
    let generations: Vec<_> = seen
        .iter()
        .filter_map(refs_of)
        .map(|r| r.generation)
        .collect();
    assert!(
        generations.windows(2).all(|w| w[0] < w[1]),
        "generations must only increase: {generations:?}"
    );
    assert!(
        generations.len() < 21,
        "superseded Opens must not all publish: {generations:?}"
    );

    // No refs Snapshot, older or otherwise, arrives after the latest generation. The
    // status Slots answer the same Opens on their own schedule, so they may still arrive.
    std::thread::sleep(std::time::Duration::from_millis(200));
    let late: Vec<_> = harness.service.drain();
    assert!(
        !late.iter().any(|s| refs_of(s).is_some()),
        "a refs Snapshot followed the latest: {late:?}"
    );
}

/// Opens `repo` on `harness`, then makes a branch outside Cairn and returns the refs
/// Snapshot that shows it: a second read, after the Open's.
fn read_again_after_a_new_branch(
    harness: &Harness,
    repo: &Path,
    branch: &str,
) -> Arc<RefsSnapshot> {
    let name = format!("refs/heads/{branch}");
    git(repo, &["branch", branch]);
    let seen = harness.drain_until(|s| {
        refs_of(s).is_some_and(|r| {
            r.result
                .as_ref()
                .is_ok_and(|refs| refs.local.iter().any(|b| b.name.as_str() == name))
        })
    });
    last_refs(&seen)
}

/// How many times git ran the read mode's detection for `repo`.
fn detections(repo: &Path) -> usize {
    git_runs(repo)
        .iter()
        .filter(|run| run.contains("--get-regexp"))
        .count()
}

#[test]
fn in_cli_mode_a_detached_head_is_published_as_detached() {
    let (_dir, repo) = small_fixture_clone();
    git(&repo, &["config", "core.safecrlf", "true"]);
    git(&repo, &["switch", "--quiet", "--detach"]);
    let snapshot = open(&repo);
    let refs = snapshot.result.as_ref().expect("the copy opens");
    // Seen red: an exit 1 from `symbolic-ref -q HEAD` taken as git failing.
    assert!(
        matches!(refs.head, Head::Detached { .. }),
        "{:?}",
        refs.head
    );
    assert_matches_git(&repo, refs);
}

#[test]
fn in_cli_mode_an_unborn_branch_is_published_with_no_target() {
    let (_dir, repo) = small_fixture_clone();
    git(&repo, &["config", "core.safecrlf", "true"]);
    git(&repo, &["symbolic-ref", "HEAD", "refs/heads/unborn"]);
    let snapshot = open(&repo);
    let refs = snapshot.result.as_ref().expect("the copy opens");
    // Seen red: an exit 1 from `rev-parse -q --verify HEAD` taken as git failing.
    assert!(
        matches!(&refs.head, Head::Branch { name, target: None } if name.as_str() == "refs/heads/unborn"),
        "{:?}",
        refs.head
    );
}

/// git fails to read the config (exit 128) while the read mode is detected: that read fails
/// with git's error, and the next read detects again rather than reusing a guess.
#[test]
fn a_failed_detection_publishes_gits_error_and_the_next_read_detects_again() {
    let (_dir, repo) = small_fixture_clone();
    assert_filesystem_events_available(&repo);
    git(&repo, &["config", "core.safecrlf", "true"]);
    let include = repo.join(".git").join("broken-include");
    std::fs::write(&include, "").unwrap();
    git(
        &repo,
        &["config", "include.path", include.to_str().unwrap()],
    );
    // Every git that reads this repository's config now exits 128, until the file is fixed.
    std::fs::write(&include, "[broken\n").unwrap();
    // `--get` rather than detection's `--get-regexp`, so that [`detections`] counts only Cairn's.
    let failing = git_output(&repo, &["config", "--get", "core.safecrlf"]);
    assert_eq!(
        failing.status.code(),
        Some(128),
        "git can't read the config"
    );

    let harness = Harness::start();
    harness.service.send(Intent::Open(repo.clone()));
    let failed = last_refs(&harness.drain_until(|s| refs_of(s).is_some()));
    assert_eq!(
        failed.result.as_ref().map(|_| ()),
        Err(&failing.stderr_text().into_owned()),
        "the read fails with git's own error"
    );

    std::fs::write(&include, "").unwrap();
    let after = read_again_after_a_new_branch(&harness, &repo, "after-fix");
    assert!(after.result.is_ok());
    assert!(
        detections(&repo) >= 2,
        "detection ran again: {:?}",
        git_runs(&repo)
    );
    // A Refs read through git that succeeded, which only the second read can be: the first
    // one's git all failed.
    assert!(
        git_runs(&repo)
            .iter()
            .any(|run| run.contains("for-each-ref") && run.ends_with("exit 0")),
        "core.safecrlf picks the CLI Refs read: {:?}",
        git_runs(&repo)
    );
}

/// No matching key is an answer (exit 1), kept until the next Open: the repository reads
/// through gix, and git doesn't detect again.
#[test]
fn a_detection_that_finds_no_key_is_kept_and_the_repository_reads_through_gix() {
    let (_dir, repo) = small_fixture_clone();
    assert_filesystem_events_available(&repo);
    let harness = Harness::start();
    harness.service.send(Intent::Open(repo.clone()));
    let opened = last_refs(&harness.drain_until(|s| refs_of(s).is_some()));
    opened.result.as_ref().expect("the copy opens");

    read_again_after_a_new_branch(&harness, &repo, "after-open");
    // Seen red: a `false` detection not kept (`*detected = value.then_some(true)`).
    assert_eq!(detections(&repo), 1, "{:?}", git_runs(&repo));
    assert!(!git_ran(&repo, "for-each-ref"), "{:?}", git_runs(&repo));
}
