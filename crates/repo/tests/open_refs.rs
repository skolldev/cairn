//! Open Intent to refs Snapshot, checked against real git.

mod common;

use std::path::Path;
use std::sync::Arc;

use cairn_repo::{Head, Intent, RefEntry, Refs, RefsSnapshot, Snapshot};
use common::{Harness, git, git_output, small_fixture};

fn refs_of(snapshot: &Snapshot) -> Option<&Arc<RefsSnapshot>> {
    match snapshot {
        Snapshot::Refs(refs) => Some(refs),
    }
}

/// Opens `path` and returns the refs Snapshot that answers the Open.
fn open(path: &Path) -> Arc<RefsSnapshot> {
    let harness = Harness::start();
    harness.service.send(Intent::Open(path.to_path_buf()));
    let seen = harness.drain_until(|s| refs_of(s).is_some());
    refs_of(seen.last().unwrap()).unwrap().clone()
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
    let refs = snapshot.refs.as_ref().expect("the fixture opens");
    assert!(!refs.tags.is_empty(), "the fixture has tags");
    assert_matches_git(&fixture, refs);
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
    let refs = snapshot.refs.as_ref().expect("the copy opens");
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
    let first = refs_of(first.last().unwrap()).unwrap().clone();

    harness.service.send(Intent::Open(fixture));
    let second = harness.drain_until(|s| refs_of(s).is_some());
    let second = refs_of(second.last().unwrap()).unwrap().clone();

    assert!(second.generation > first.generation);
}

#[test]
fn opening_a_path_that_is_not_a_repo_publishes_the_error() {
    let dir = tempfile::tempdir().unwrap();
    let snapshot = open(dir.path());
    let error = snapshot.refs.as_ref().expect_err("not a repository");
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

    // Nothing older arrives after the latest generation.
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(harness.service.drain().is_empty());
}
