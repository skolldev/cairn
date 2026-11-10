//! Shared helpers for the runner tests: a git config the tests control, and scratch
//! repositories driven only through the runner.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use cairn_write::{Git, Invocation, JobClass, Output};

/// Points git's user and system config at an empty home, once per test binary, so a
/// developer's `commit.gpgsign` or `core.longpaths` can't change what the tests see. Every
/// test calls this (through [`git`]) before anything spawns git.
fn hermetic() {
    static HOME: OnceLock<tempfile::TempDir> = OnceLock::new();
    HOME.get_or_init(|| {
        let home = tempfile::tempdir().expect("temp home");
        // SAFETY: this runs inside `OnceLock::get_or_init`, which blocks every other test
        // thread at the same call until it returns, and no test spawns git before it.
        unsafe {
            std::env::set_var("HOME", home.path());
            std::env::set_var("XDG_CONFIG_HOME", home.path().join(".config"));
            std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
            // The runner keeps a user's own askpass (ADR 0008), so a developer's, such as the
            // `GIT_ASKPASS` VS Code's terminal sets, would change what the tests see.
            std::env::remove_var("SSH_ASKPASS");
            std::env::remove_var("SSH_ASKPASS_REQUIRE");
            std::env::remove_var("GIT_ASKPASS");
        }
        home
    });
}

pub fn git() -> &'static Git {
    hermetic();
    Git::shared().expect("a usable git is installed")
}

/// Runs a Read Job in `repo`.
pub fn read(repo: &Path, args: &[&str]) -> Output {
    git()
        .run(&Invocation::new(JobClass::Read, repo).args(args))
        .expect("git runs")
}

/// Runs a Local write Job in `repo`, with optional stdin.
pub fn local_write(repo: &Path, args: &[&str], stdin: Option<&[u8]>) -> Output {
    let mut invocation = Invocation::new(JobClass::LocalWrite, repo).args(args);
    if let Some(bytes) = stdin {
        invocation = invocation.stdin(bytes.to_vec());
    }
    git().run(&invocation).expect("git runs")
}

/// Panics with git's stderr unless it succeeded.
pub fn ok(out: Output) -> Output {
    assert!(out.status.success(), "git failed: {}", out.stderr_text());
    out
}

/// A fresh repository with an identity, in a temp dir that lives as long as this value.
pub struct Repo {
    _dir: tempfile::TempDir,
    pub path: PathBuf,
}

impl Repo {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("repo");
        std::fs::create_dir(&path).unwrap();
        ok(local_write(&path, &["init", "-q"], None));
        ok(local_write(
            &path,
            &["config", "user.name", "Cairn Test"],
            None,
        ));
        ok(local_write(
            &path,
            &["config", "user.email", "test@cairn.invalid"],
            None,
        ));
        Self { _dir: dir, path }
    }

    /// Writes `contents` to `rel` inside the work tree, creating parent directories.
    pub fn file(&self, rel: impl AsRef<Path>, contents: &str) -> PathBuf {
        let path = self.path.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
        path
    }
}

/// An alias that runs a shell which announces itself on stderr and then waits for
/// `release` to exist, or its directory to go. The shell is a child of git, as `upload-pack` or a remote helper
/// is during a fetch.
pub fn blocking_alias(release: &Path) -> String {
    let dir = forward_slashes(release.parent().unwrap());
    let release = forward_slashes(release);
    format!(
        "alias.block=!echo waiting >&2; while [ ! -f '{release}' ] && [ -d '{dir}' ]; do sleep 0.05; done; \
         echo released >&2"
    )
}

/// `path` as git's own sh takes it, which is forward slashes on Windows too.
pub fn forward_slashes(path: &Path) -> String {
    path.to_str().unwrap().replace('\\', "/")
}

/// The `SSH_ASKPASS` named in `env`, output with one `NAME=value` per line.
pub fn ssh_askpass_in(env: &str) -> Option<&str> {
    env.lines().find_map(|l| l.strip_prefix("SSH_ASKPASS="))
}

/// Whether `path` is the `cairn-askpass` this test binary was built with.
pub fn is_cairns_askpass(path: &str) -> bool {
    let built = std::fs::canonicalize(env!("CARGO_BIN_EXE_cairn-askpass")).unwrap();
    std::fs::canonicalize(path).ok() == Some(built)
}

/// Creates the release file when dropped, so a failing test never leaves a shell waiting.
pub struct Release<'a>(pub &'a Path);

impl Drop for Release<'_> {
    fn drop(&mut self) {
        let _ = std::fs::write(self.0, "");
    }
}
