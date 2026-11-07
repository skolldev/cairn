//! The runner's rules for every call, observed through git itself.

mod common;

use common::{Repo, local_write, ok, read};

#[test]
fn every_call_turns_on_long_paths() {
    let repo = Repo::new();
    let out = ok(read(&repo.path, &["config", "--get", "core.longpaths"]));
    assert_eq!(out.stdout, b"true\n");
}

#[test]
fn maintenance_stays_at_gits_default() {
    let repo = Repo::new();
    let out = read(&repo.path, &["config", "--get", "maintenance.auto"]);
    assert_eq!(out.status.code(), Some(1), "unset, so git exits 1");
    assert!(out.stdout.is_empty());
}

#[test]
fn git_runs_in_the_c_locale_and_never_prompts() {
    let repo = Repo::new();
    // A shell alias prints the environment git hands to the programs it runs.
    let out = ok(read(&repo.path, &["-c", "alias.env=!env", "env"]));
    let env = String::from_utf8(out.stdout).unwrap();
    assert!(env.lines().any(|l| l == "LC_ALL=C"), "{env}");
    assert!(env.lines().any(|l| l == "GIT_TERMINAL_PROMPT=0"), "{env}");
}

/// A stat-dirty file makes `git status` want to refresh the index, which it does through
/// `index.lock`. A Read Job must leave the index alone; the Local write control shows the
/// setup really does provoke a refresh.
#[test]
fn reads_never_take_the_index_lock() {
    use std::time::{Duration, SystemTime};
    let repo = Repo::new();
    let file = repo.file("a.txt", "a\n");
    ok(local_write(&repo.path, &["add", "a.txt"], None));
    ok(local_write(&repo.path, &["commit", "-q", "-m", "a"], None));
    let index = repo.path.join(".git").join("index");
    let make_stat_dirty = |secs| {
        let when = SystemTime::now() - Duration::from_secs(secs);
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(when)
            .unwrap();
    };
    let snapshot = || {
        (
            std::fs::read(&index).unwrap(),
            std::fs::metadata(&index).unwrap().modified().unwrap(),
        )
    };

    make_stat_dirty(3600);
    let before = snapshot();
    ok(read(&repo.path, &["status", "--porcelain=v2", "-z"]));
    assert_eq!(snapshot(), before, "a Read Job rewrote the index");

    ok(local_write(
        &repo.path,
        &["status", "--porcelain=v2", "-z"],
        None,
    ));
    assert_ne!(
        snapshot(),
        before,
        "control: a Local write Job refreshes the index"
    );
}

#[test]
fn a_failure_carries_gits_stderr_unchanged() {
    let repo = Repo::new();
    let out = read(&repo.path, &["rev-parse", "--verify", "no-such-ref"]);
    assert!(!out.status.success());
    assert_eq!(out.stderr, b"fatal: Needed a single revision\n");

    let err = out.success().unwrap_err();
    assert_eq!(err.to_string(), "fatal: Needed a single revision\n");
}

/// The path needn't exist: the refusal comes before git is spawned.
#[cfg(windows)]
#[test]
fn a_windows_repo_path_over_260_characters_is_refused_before_any_spawn() {
    use cairn_write::{Invocation, JobClass, MAX_WINDOWS_REPO_PATH, RunError};
    let repo = std::env::temp_dir().join("x".repeat(MAX_WINDOWS_REPO_PATH));
    let err = common::git()
        .run(&Invocation::new(JobClass::Read, &repo).args(["status"]))
        .unwrap_err();
    let RunError::RepoPathTooLong { chars, .. } = &err else {
        panic!("expected RepoPathTooLong, got {err:?}");
    };
    assert!(*chars > MAX_WINDOWS_REPO_PATH);
    assert!(
        err.to_string()
            .starts_with("git can't use a repository whose path is over 260")
    );
}
