//! The CLI Refs read when git fails, or answers "no" to both questions, as it works out HEAD
//! (#61, #77).
//!
//! No repository on disk makes just one of `symbolic-ref -q HEAD` and `rev-parse -q --verify
//! HEAD` fail: a config write racing between the two does. So this test binary's git is a
//! wrapper ahead of the real one on `PATH`. When an argument names a repository holding
//! `.git/cairn-fail-<subcommand>`, it has the real git include that file, a broken config,
//! for that subcommand only. Real git still fails, with its own exit code and error.
//!
//! Nor does any repository make both answer "no": a detached HEAD on a missing object makes
//! rev-parse succeed, and a malformed `ref:` makes symbolic-ref exit 128 or git reject the
//! repository. Only a race or a misbehaving git does. So a repository holding
//! `.git/cairn-no-<subcommand>` has the wrapper answer "no" without running git: exit 1,
//! printing the marker's contents to stderr (often nothing, as `-q` would).
//!
//! The wrapper finds the repository by the marker, not by where the runner puts `-C`. Unix
//! only: the wrapper is a shell script, and Windows resolves a `git.exe`.

#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt as _;
use std::sync::OnceLock;

use cairn_write::Git;
use common::{git, git_output, open_refs, small_fixture_clone};

const WRAPPER: &str = r#"#!/bin/sh
# Drop this directory from PATH, so that `git` below is the real one.
here=$(dirname "$0")
PATH=$(printf '%s' ":$PATH:" | sed "s|:$here:|:|g; s|^:||; s|:$||")
export PATH
# The subcommand is the first argument that names one, and the repository is whichever
# argument is a directory holding that subcommand's marker, wherever the runner puts it.
for arg; do
  case $arg in
  symbolic-ref | rev-parse)
    for dir; do
      broken="$dir/.git/cairn-fail-$arg"
      [ -f "$broken" ] && exec git -c include.path="$broken" "$@"
      no="$dir/.git/cairn-no-$arg"
      [ -f "$no" ] && { cat "$no" >&2; exit 1; }
    done
    break
    ;;
  esac
done
exec git "$@"
"#;

/// Puts the wrapper first on `PATH`, once per test binary and before anything resolves git.
fn wrapped_git() {
    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("git");
        std::fs::write(&script, WRAPPER).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::var_os("PATH").unwrap_or_default();
        let path = std::iter::once(dir.path().to_path_buf()).chain(std::env::split_paths(&path));
        let path = std::env::join_paths(path).unwrap();
        // SAFETY: this runs inside `OnceLock::get_or_init`, which every test calls first, so
        // every other test thread waits here, and nothing has spawned git yet.
        unsafe { std::env::set_var("PATH", path) };
        assert_eq!(
            Git::shared().unwrap().path(),
            script,
            "the wrapper is this binary's git"
        );
        dir
    });
}

/// A clone that reads refs through git, whose `subcommand` fails as git does when it can't
/// read the config. Returns git's own error for it.
fn failing(subcommand: &str) -> (tempfile::TempDir, std::path::PathBuf, String) {
    wrapped_git();
    let (dir, repo) = small_fixture_clone();
    git(&repo, &["config", "core.safecrlf", "true"]);
    std::fs::write(
        repo.join(".git").join(format!("cairn-fail-{subcommand}")),
        "[broken\n",
    )
    .unwrap();
    let args: &[&str] = match subcommand {
        "symbolic-ref" => &["symbolic-ref", "-q", "HEAD"],
        _ => &["rev-parse", "-q", "--verify", "HEAD"],
    };
    let failed = git_output(&repo, args);
    assert_eq!(failed.status.code(), Some(128), "git fails, not answers no");
    let error = failed.stderr_text().into_owned();
    (dir, repo, error)
}

#[test]
fn a_symbolic_ref_that_fails_publishes_gits_error_not_a_detached_head() {
    let (_dir, repo, error) = failing("symbolic-ref");
    let control = git_output(&repo, &["rev-parse", "-q", "--verify", "HEAD"]);
    assert!(control.status.success(), "rev-parse still answers");
    let refs = open_refs(&repo);
    assert_eq!(refs.result.as_ref().map(|r| &r.head), Err(&error));
}

#[test]
fn a_rev_parse_that_fails_publishes_gits_error_not_an_unborn_branch() {
    let (_dir, repo, error) = failing("rev-parse");
    let control = git_output(&repo, &["symbolic-ref", "-q", "HEAD"]);
    assert!(control.status.success(), "symbolic-ref still answers");
    let refs = open_refs(&repo);
    assert_eq!(refs.result.as_ref().map(|r| &r.head), Err(&error));
}

/// A clone that reads refs through git, whose `symbolic-ref` and `rev-parse` both answer "no",
/// printing `stderr` for each: HEAD names neither a branch nor a commit (#77).
fn answering_no(stderr: [&str; 2]) -> (tempfile::TempDir, std::path::PathBuf) {
    wrapped_git();
    let (dir, repo) = small_fixture_clone();
    git(&repo, &["config", "core.safecrlf", "true"]);
    for (subcommand, stderr) in ["symbolic-ref", "rev-parse"].into_iter().zip(stderr) {
        std::fs::write(
            repo.join(".git").join(format!("cairn-no-{subcommand}")),
            stderr,
        )
        .unwrap();
    }
    for args in [
        &["symbolic-ref", "-q", "HEAD"][..],
        &["rev-parse", "-q", "--verify", "HEAD"],
    ] {
        let no = git_output(&repo, args);
        assert_eq!(no.status.code(), Some(1), "git answers no: {args:?}");
    }
    (dir, repo)
}

#[test]
fn when_both_head_commands_answer_no_the_error_says_head_is_neither_a_branch_nor_a_commit() {
    let (_dir, repo) = answering_no(["", ""]);
    let refs = open_refs(&repo);
    let error = refs.result.as_ref().map(|r| &r.head).unwrap_err();
    assert!(
        error.contains("HEAD names neither a branch nor a commit"),
        "{error:?}"
    );
}

#[test]
fn when_both_head_commands_answer_no_the_error_includes_what_each_printed() {
    let (_dir, repo) = answering_no(["symbolic-ref said this\n", "rev-parse said that\n"]);
    let refs = open_refs(&repo);
    let error = refs.result.as_ref().map(|r| &r.head).unwrap_err();
    assert!(
        error.contains("HEAD names neither a branch nor a commit"),
        "{error:?}"
    );
    assert!(error.contains("symbolic-ref said this"), "{error:?}");
    assert!(error.contains("rev-parse said that"), "{error:?}");
}
