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

/// Runs git in `repo` as a Read Job would, with `--no-optional-locks`, so that commands
/// like `git status` don't refresh the index on disk. Panics with git's stderr on failure.
pub fn git_read(repo: &Path, args: &[&str]) -> Vec<u8> {
    hermetic();
    let git = Git::shared().expect("a usable git is installed");
    let out = git
        .run(&Invocation::new(JobClass::Read, repo).args(args))
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        out.stderr_text()
    );
    out.stdout
}

/// Changes the file's mtime without changing its content, so its index entry is stat-dirty.
pub fn touch(path: &Path) {
    let file = std::fs::File::options().write(true).open(path).unwrap();
    let when = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
    file.set_modified(when).unwrap();
}

/// A fresh clone of the small fixture in a temp dir, safe to change.
pub fn small_fixture_clone() -> (tempfile::TempDir, PathBuf) {
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
    (dir, repo)
}

/// The autocrlf and filters fixture. `scripts/fixtures.sh filters` creates it.
pub fn filters_fixture() -> PathBuf {
    let root = std::env::var_os("CAIRN_FIXTURES")
        .map(PathBuf::from)
        .unwrap_or_else(|| real_home().join("cairn-fixtures"));
    let path = root.join("filters");
    assert!(
        path.join(".git").exists(),
        "the filters fixture is missing at {}. Create it with `scripts/fixtures.sh filters`, \
         or point CAIRN_FIXTURES at a directory that holds it.",
        path.display()
    );
    path
}

/// A file-by-file copy of the filters fixture, config and all. Copying gives every file a
/// new mtime, so every index entry starts stat-dirty.
pub fn filters_fixture_copy() -> (tempfile::TempDir, PathBuf) {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("filters");
    copy(&filters_fixture(), &repo);
    (dir, repo)
}

// Driving the service: shared by every test that opens a repository and runs Jobs.

use std::collections::BTreeMap;
use std::sync::Arc;

use cairn_repo::{Generation, Intent, JobId, JobKind, JobSnapshot, JobState, RefsSnapshot};

/// `path` as git's own sh reads it: forward slashes, on Windows too. For paths written
/// into hooks, aliases and `uploadpack` wrappers.
pub fn slash(path: &Path) -> String {
    path.to_str().unwrap().replace('\\', "/")
}

/// Creates a release file when dropped, so a failing test never leaves a hook or wrapper
/// waiting on it. Declare it after the [`Harness`]: it then drops first, releasing the
/// wait before the service waits for its Local write Jobs.
pub struct Release<'a>(pub &'a Path);

impl Drop for Release<'_> {
    fn drop(&mut self) {
        let _ = std::fs::write(self.0, "");
    }
}

/// Installs a `post-checkout` hook that logs each run to `log` as `start <branch>` and
/// `end <branch>`, prints `hook waiting on <branch>`, and blocks until `release` exists.
pub fn install_blocking_hook(repo: &Path, log: &Path, release: &Path) {
    let script = format!(
        "#!/bin/sh\n\
         branch=$(git symbolic-ref --short HEAD)\n\
         echo \"start $branch\" >> '{log}'\n\
         echo \"hook waiting on $branch\"\n\
         while [ ! -f '{release}' ]; do sleep 0.05; done\n\
         echo \"end $branch\" >> '{log}'\n",
        log = slash(log),
        release = slash(release),
    );
    let hook = repo.join(".git/hooks/post-checkout");
    std::fs::write(&hook, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// The generation each Slot answered an Open with.
pub struct Opened {
    pub refs: Generation,
    pub staged: Generation,
    pub unstaged: Generation,
    pub untracked: Generation,
}

/// Opens `repo` and waits until every Slot has answered it.
pub fn open(harness: &Harness, repo: &Path) -> Opened {
    harness.service.send(Intent::Open(repo.to_path_buf()));
    let (mut refs, mut staged, mut unstaged, mut untracked) = (None, None, None, None);
    harness.drain_until(|s| {
        match s {
            Snapshot::Refs(r) if r.workdir == repo => refs = Some(r.generation),
            Snapshot::Staged(s) if s.workdir == repo => staged = Some(s.generation),
            Snapshot::Unstaged(s) if s.workdir == repo => unstaged = Some(s.generation),
            Snapshot::Untracked(s) if s.workdir == repo => untracked = Some(s.generation),
            _ => {}
        }
        refs.is_some() && staged.is_some() && unstaged.is_some() && untracked.is_some()
    });
    Opened {
        refs: refs.unwrap(),
        staged: staged.unwrap(),
        unstaged: unstaged.unwrap(),
        untracked: untracked.unwrap(),
    }
}

pub fn job_of(snapshot: &Snapshot) -> Option<&Arc<JobSnapshot>> {
    match snapshot {
        Snapshot::Job(job) => Some(job),
        _ => None,
    }
}

/// Done or Stopped: the Job will publish nothing more.
pub fn is_ended(job: &JobSnapshot) -> bool {
    matches!(job.state, JobState::Done(_) | JobState::Stopped)
}

/// Every Snapshot drained so far, and the newest Snapshot of each Job, kept across drains.
#[derive(Default)]
pub struct Seen {
    pub jobs: BTreeMap<JobId, Arc<JobSnapshot>>,
    pub all: Vec<Snapshot>,
}

impl Seen {
    fn record(&mut self, snapshot: &Snapshot) {
        if let Some(job) = job_of(snapshot) {
            self.jobs.insert(job.id, job.clone());
        }
        self.all.push(snapshot.clone());
    }

    /// Drains until `done` accepts what has been seen so far.
    pub fn until(&mut self, harness: &Harness, mut done: impl FnMut(&Self) -> bool) {
        if done(self) {
            return;
        }
        harness.drain_until(|s| {
            self.record(s);
            done(self)
        });
    }

    /// Records whatever has been published by now, without waiting.
    pub fn drain_now(&mut self, harness: &Harness) {
        for s in harness.service.drain() {
            self.record(&s);
        }
    }

    /// The `n`th Job, in the order they were asked for.
    pub fn nth(&self, n: usize) -> Option<&Arc<JobSnapshot>> {
        self.jobs.values().nth(n)
    }

    /// The first Job of this kind.
    pub fn job(&self, kind: &JobKind) -> Option<&Arc<JobSnapshot>> {
        self.jobs.values().find(|j| &j.kind == kind)
    }

    /// The newest refs Snapshot newer than `opened`, if any.
    pub fn refs_after(&self, opened: Generation) -> Option<&Arc<RefsSnapshot>> {
        self.all.iter().rev().find_map(|s| match s {
            Snapshot::Refs(r) if r.generation > opened => Some(r),
            _ => None,
        })
    }
}
