//! Shared helpers for the `repo` integration tests: fixtures, real git, and a way to wait
//! for Snapshots through the public interface.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{OnceLock, mpsc};
use std::time::{Duration, Instant};

use cairn_repo::{RepoService, Snapshot};
use cairn_write::{Git, Invocation, JobClass, Output};

/// The small fixture: ripgrep at the #5 commit. `scripts/fixtures.sh small` creates it.
/// `CAIRN_FIXTURES` overrides the fixture root, which defaults to `~/cairn-fixtures`.
pub fn small_fixture() -> PathBuf {
    let root = std::env::var_os("CAIRN_FIXTURES")
        .map(PathBuf::from)
        .unwrap_or_else(|| real_home().join("cairn-fixtures"));
    let path = root.join("ripgrep");
    assert!(
        path.join(".git").exists(),
        "the small fixture is missing at {}. Create it with `scripts/fixtures.sh small`, \
         or point CAIRN_FIXTURES at a directory that holds it.",
        path.display()
    );
    path
}

/// The user's home as the test process found it, before [`hermetic`] replaced `HOME`.
fn real_home() -> &'static Path {
    hermetic().0.as_path()
}

/// Points git's user and system config at an empty home, once per test binary, so a
/// developer's config can't change what git or gix see. Returns the real home and the
/// temp one. Every git spawn and every fixture lookup goes through this first.
fn hermetic() -> &'static (PathBuf, tempfile::TempDir) {
    static HOMES: OnceLock<(PathBuf, tempfile::TempDir)> = OnceLock::new();
    HOMES.get_or_init(|| {
        let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        let real = PathBuf::from(std::env::var_os(var).expect("HOME or USERPROFILE is set"));
        let temp = tempfile::tempdir().expect("temp home");
        // SAFETY: this runs inside `OnceLock::get_or_init`, which blocks every other test
        // thread at the same call until it returns, and nothing spawns git before it.
        unsafe {
            std::env::set_var("HOME", temp.path());
            std::env::set_var("XDG_CONFIG_HOME", temp.path().join(".config"));
            std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        }
        (real, temp)
    })
}

/// Runs git in `repo` through Cairn's runner, with a test identity, and returns its output.
pub fn git_output(repo: &Path, args: &[&str]) -> Output {
    hermetic();
    let git = Git::shared().expect("a usable git is installed");
    let invocation = Invocation::new(JobClass::LocalWrite, repo)
        .args([
            "-c",
            "user.name=Cairn Test",
            "-c",
            "user.email=test@cairn.invalid",
        ])
        .args(args);
    git.run(&invocation).expect("git runs")
}

/// Runs git in `repo` and returns its stdout. Panics with git's stderr on failure.
pub fn git(repo: &Path, args: &[&str]) -> String {
    let out = git_output(repo, args);
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        out.stderr_text()
    );
    String::from_utf8(out.stdout).expect("git output is UTF-8")
}

/// A Repo service whose waker feeds a channel, so tests can block until Snapshots arrive.
pub struct Harness {
    pub service: RepoService,
    woken: mpsc::Receiver<()>,
}

impl Harness {
    pub fn start() -> Self {
        let (tx, woken) = mpsc::channel();
        let service = RepoService::start(move || {
            let _ = tx.send(());
        });
        Self { service, woken }
    }

    /// Drains Snapshots until `done` accepts one, returning everything drained on the way.
    pub fn drain_until(&self, mut done: impl FnMut(&Snapshot) -> bool) -> Vec<Snapshot> {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut seen = Vec::new();
        loop {
            let batch = self.service.drain();
            let finished = batch.iter().any(&mut done);
            seen.extend(batch);
            if finished {
                return seen;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(
                !left.is_zero(),
                "timed out waiting for a Snapshot; drained so far: {seen:?}"
            );
            let _ = self
                .woken
                .recv_timeout(left.min(Duration::from_millis(200)));
        }
    }
}
