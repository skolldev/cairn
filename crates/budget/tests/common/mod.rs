//! Shared by the harness's tests: fixtures through real git, under an empty config.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Points git at an empty home, once per test binary, and returns the real one, for finding
/// the fixtures. Runs before anything spawns git.
fn hermetic() -> &'static (PathBuf, tempfile::TempDir) {
    static HOMES: OnceLock<(PathBuf, tempfile::TempDir)> = OnceLock::new();
    HOMES.get_or_init(|| {
        let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        let real = PathBuf::from(std::env::var_os(var).expect("HOME or USERPROFILE is set"));
        let temp = tempfile::tempdir().expect("temp home");
        // SAFETY: inside `OnceLock::get_or_init`, which blocks every other test thread at the
        // same call until it returns, and nothing spawns git before it.
        unsafe {
            std::env::set_var("HOME", temp.path());
            std::env::set_var("XDG_CONFIG_HOME", temp.path().join(".config"));
            std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        }
        (real, temp)
    })
}

/// The small fixture. `scripts/fixtures.sh small` creates it.
pub fn small_fixture() -> PathBuf {
    let root = std::env::var_os("CAIRN_FIXTURES")
        .map(PathBuf::from)
        .unwrap_or_else(|| hermetic().0.join("cairn-fixtures"));
    hermetic();
    let path = root.join("ripgrep");
    assert!(
        path.join(".git").exists(),
        "the small fixture is missing at {}. Create it with `scripts/fixtures.sh small`.",
        path.display()
    );
    path
}

/// `git` in `repo`, as a Read Job runs it, so an oracle doesn't refresh the index.
pub fn git_read(repo: &Path, args: &[&str]) -> String {
    hermetic();
    let out = cairn_write::Git::shared()
        .expect("a usable git is installed")
        .run(
            &cairn_write::Invocation::new(cairn_write::JobClass::Read, repo)
                .args(args.iter().copied()),
        )
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {}", out.stderr_text());
    String::from_utf8(out.stdout).expect("git output is UTF-8")
}
