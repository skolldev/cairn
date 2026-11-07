//! Shared helpers for the `repo` integration tests: fixtures, real git, and a way to wait
//! for Snapshots through the public interface.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cairn_repo::{RepoService, Snapshot};

/// The small fixture: ripgrep at the #5 commit. `scripts/fixtures.sh small` creates it.
/// `CAIRN_FIXTURES` overrides the fixture root, which defaults to `~/cairn-fixtures`.
pub fn small_fixture() -> PathBuf {
    let root = std::env::var_os("CAIRN_FIXTURES")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .expect("HOME or USERPROFILE is set");
            PathBuf::from(home).join("cairn-fixtures")
        });
    let path = root.join("ripgrep");
    assert!(
        path.join(".git").exists(),
        "the small fixture is missing at {}. Create it with `scripts/fixtures.sh small`, \
         or point CAIRN_FIXTURES at a directory that holds it.",
        path.display()
    );
    path
}

/// Runs real git in `repo` and returns its stdout. Panics with git's stderr on failure.
pub fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "-c",
            "user.name=Cairn Test",
            "-c",
            "user.email=test@cairn.invalid",
        ])
        .args(args)
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
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
