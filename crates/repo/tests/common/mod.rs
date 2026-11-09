//! Shared helpers for the `repo` integration tests: fixtures, real git, and a way to wait
//! for Snapshots through the public interface.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{OnceLock, mpsc};
use std::time::{Duration, Instant};

use cairn_repo::{RepoService, Snapshot};
use cairn_write::{Git, Invocation, JobClass, Output};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher as _};

/// Fail promptly when this host cannot deliver filesystem events to the watcher tests.
/// This probes the same temp filesystem as their repositories, without replacing their
/// end-to-end assertions about External changes and Snapshots.
pub fn assert_filesystem_events_available(repo: &Path) {
    let root = repo.canonicalize().expect("canonical repository directory");
    let marker = root.join("watcher-probe");
    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |event| {
        let _ = tx.send(event);
    })
    .unwrap_or_else(|error| panic!("filesystem watcher cannot start: {error}"));
    watcher
        .watch(&root, RecursiveMode::Recursive)
        .unwrap_or_else(|error| {
            panic!(
                "filesystem watcher cannot watch {}: {error}",
                root.display()
            )
        });
    std::fs::write(&marker, b"probe").expect("write watcher probe marker");
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining) {
            Ok(Ok(event)) if event.paths.iter().any(|path| path == &marker) => {
                std::fs::remove_file(&marker).expect("remove watcher probe marker");
                return;
            }
            Ok(Ok(_)) => {}
            Ok(Err(error)) => panic!("filesystem watcher reported an error: {error}"),
            Err(_) => panic!(
                "filesystem watcher delivered no event for {} within 2 seconds; \
                 this host cannot run event-driven Repo service tests",
                marker.display()
            ),
        }
    }
}

/// The small fixture: ripgrep at the #5 commit. `scripts/fixtures.sh small` creates it.
/// `CAIRN_FIXTURES` overrides the fixture root, which defaults to `~/cairn-fixtures`.
pub fn small_fixture() -> PathBuf {
    let path = fixture_root().join("ripgrep");
    assert!(
        path.join(".git").exists(),
        "the small fixture is missing at {}. Create it with `scripts/fixtures.sh small`, \
         or point CAIRN_FIXTURES at a directory that holds it.",
        path.display()
    );
    path
}

/// Where the fixtures live, one directory under `CAIRN_FIXTURES` or `~/cairn-fixtures` each.
pub fn fixture_root() -> PathBuf {
    std::env::var_os("CAIRN_FIXTURES")
        .map(PathBuf::from)
        .unwrap_or_else(|| real_home().join("cairn-fixtures"))
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
            std::env::set_var("GIT_TRACE2_EVENT", temp.path().join("trace2.jsonl"));
        }
        (real, temp)
    })
}

/// Whether any git run so far named this subcommand with `repo` as its `-C`, by git's own
/// trace2 log, which [`hermetic`] turns on. Repositories in temp dirs have unique paths, so
/// tests running side by side don't see each other's git.
pub fn git_ran(repo: &Path, subcommand: &str) -> bool {
    let log = hermetic().1.path().join("trace2.jsonl");
    // The directory's own name, which needs no JSON escaping on any OS. The copy of the
    // fixture is always named the same, so look one level up too.
    let parent = repo.parent().and_then(Path::file_name).unwrap();
    let parent = parent.to_str().unwrap();
    std::fs::read_to_string(log).is_ok_and(|log| {
        log.lines().any(|line| {
            line.contains(r#""event":"start""#)
                && line.contains(parent)
                && line.contains(&format!(r#""{subcommand}""#))
        })
    })
}

pub fn commit_graph_exists(repo: &Path) -> bool {
    let info = repo.join(".git/objects/info");
    info.join("commit-graph").exists() || info.join("commit-graphs/commit-graph-chain").exists()
}

/// Writes a commit-graph of everything reachable, and proves it's there.
pub fn write_commit_graph(repo: &Path) {
    git(repo, &["commit-graph", "write", "--reachable"]);
    assert!(commit_graph_exists(repo), "git wrote a commit-graph");
}

/// What `git rev-list` prints for the walk Cairn promises, run as a Read Job runs git.
pub fn rev_list_order(repo: &Path) -> Vec<Oid> {
    let out = git_read(repo, &["rev-list", "--topo-order", "--date-order", "--all"]);
    String::from_utf8(out)
        .unwrap()
        .lines()
        .map(Oid::from_hex)
        .collect()
}

/// Writes a commit object with exactly this commit date, and nothing that points to it. Object
/// first, because only the environment can set a commit date through `git commit`, and the
/// runner's environment is the same for every test. Its parents needn't exist. `scratch` is a
/// directory for the object's file.
pub fn write_commit(
    repo: &Path,
    scratch: &Path,
    tree: &str,
    parents: &[&str],
    date: u32,
    message: &str,
) -> String {
    let who = format!("Cairn Test <test@cairn.invalid> {date} +0000");
    let mut object = format!("tree {tree}\n");
    for parent in parents {
        object += &format!("parent {parent}\n");
    }
    object += &format!("author {who}\ncommitter {who}\n\n{message}\n");
    let file = scratch.join("commit-object");
    std::fs::write(&file, object).unwrap();
    git(
        repo,
        &["hash-object", "-t", "commit", "-w", file.to_str().unwrap()],
    )
    .trim()
    .to_owned()
}

/// A local clone copies the fixture's commit-graph, if it has one. This removes it.
pub fn remove_commit_graph(repo: &Path) {
    let info = repo.join(".git/objects/info");
    let _ = std::fs::remove_file(info.join("commit-graph"));
    let _ = std::fs::remove_dir_all(info.join("commit-graphs"));
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
    pub fn drain_until(&self, done: impl FnMut(&Snapshot) -> bool) -> Vec<Snapshot> {
        self.drain_until_with_timeout(Duration::from_secs(30), done)
    }

    pub fn drain_until_with_timeout(
        &self,
        timeout: Duration,
        mut done: impl FnMut(&Snapshot) -> bool,
    ) -> Vec<Snapshot> {
        let deadline = Instant::now() + timeout;
        let mut seen = Vec::new();
        loop {
            let batch = self.service.drain();
            let finished = batch.iter().any(&mut done);
            seen.extend(batch);
            if finished {
                return seen;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                // A timed-out test must not then hang while dropping a blocked Local write Job.
                self.service.send(Intent::ForceQuit);
                panic!("timed out waiting for a Snapshot; drained so far: {seen:?}");
            }
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
    let path = fixture_root().join("filters");
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

use cairn_repo::{Generation, Intent, JobId, JobKind, JobSnapshot, JobState, Oid, RefsSnapshot};

/// `path` as git's own sh reads it: forward slashes, on Windows too. For paths written
/// into hooks, aliases and `uploadpack` wrappers.
pub fn slash(path: &Path) -> String {
    path.to_str().unwrap().replace('\\', "/")
}

/// sh that waits until `release` exists, or until its directory is gone. A Force quit leaves
/// its Job's hook running after the test has ended, so without that second condition a release
/// file deleted with the temp dir would leave the hook looping forever.
pub fn wait_for_release(release: &Path) -> String {
    format!(
        "while [ ! -f '{release}' ] && [ -d '{dir}' ]; do sleep 0.05; done\n",
        release = slash(release),
        dir = slash(release.parent().expect("a release file has a directory")),
    )
}

/// sh that blocks only the first process to run it, until `release` exists. `started` appears
/// once that process is blocked, for [`wait_for_file`]. Decided by `mkdir`, because processes can
/// run this at once and only one `mkdir` can succeed, where testing for a file lets several
/// through. `scripts/check-test-gates.sh` keeps it that way.
pub fn block_first_run(started: &Path, release: &Path) -> String {
    format!(
        "if mkdir '{started}' 2>/dev/null; then\n{wait}fi\n",
        started = slash(started),
        wait = wait_for_release(release),
    )
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
         {wait}\
         echo \"end $branch\" >> '{log}'\n",
        log = slash(log),
        wait = wait_for_release(release),
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

/// A bare "remote" cloned from the small fixture, whose `upload-pack` and `receive-pack` are
/// slowed by a wrapper, a clone of it, and a commit on the remote that the clone hasn't
/// fetched yet.
pub struct SlowRemote {
    pub dir: tempfile::TempDir,
    pub remote: PathBuf,
    pub repo: PathBuf,
    pub branch: String,
    /// The remote's branch tip before and after its new commit.
    pub old: String,
    pub new: String,
    /// The wrapper waits for this file before it lets git on the remote run.
    pub release: PathBuf,
    /// One line per wrapper run.
    pub log: PathBuf,
}

pub fn slow_remote() -> SlowRemote {
    let dir = tempfile::tempdir().unwrap();
    let remote = dir.path().join("remote.git");
    let repo = dir.path().join("copy");
    let fixture = small_fixture();
    let path = |p: &Path| p.to_str().unwrap().to_owned();
    git(
        dir.path(),
        &[
            "clone",
            "--quiet",
            "--bare",
            &path(&fixture),
            &path(&remote),
        ],
    );
    git(
        dir.path(),
        &["clone", "--quiet", &path(&remote), &path(&repo)],
    );
    // A local clone copies the fixture's commit-graph. Remove it, so that one after the
    // fetch is the fetch's.
    remove_commit_graph(&repo);
    let branch = git(&repo, &["symbolic-ref", "--short", "HEAD"])
        .trim()
        .to_owned();
    let old = git(&remote, &["rev-parse", &branch]).trim().to_owned();
    let new = git(
        &remote,
        &[
            "commit-tree",
            &format!("{old}^{{tree}}"),
            "-p",
            &old,
            "-m",
            "on the remote",
        ],
    )
    .trim()
    .to_owned();
    git(
        &remote,
        &["update-ref", &format!("refs/heads/{branch}"), &new],
    );

    let release = dir.path().join("release");
    let log = dir.path().join("remote.log");
    let wrapper = dir.path().join("slow-remote.sh");
    std::fs::write(
        &wrapper,
        format!(
            "echo run >> '{log}'\n\
             echo 'slow remote waiting' >&2\n\
             {wait}\
             exec git \"$@\"\n",
            log = slash(&log),
            wait = wait_for_release(&release),
        ),
    )
    .unwrap();
    for (key, command) in [
        ("remote.origin.uploadpack", "upload-pack"),
        ("remote.origin.receivepack", "receive-pack"),
    ] {
        git(
            &repo,
            &[
                "config",
                key,
                &format!("sh '{}' {command}", slash(&wrapper)),
            ],
        );
    }
    SlowRemote {
        dir,
        remote,
        repo,
        branch,
        old,
        new,
        release,
        log,
    }
}

/// Waits until a Network Job of this kind is blocked in the slow remote, with its line
/// already published.
pub fn wait_until_network_job_is_waiting(seen: &mut Seen, harness: &Harness, kind: &JobKind) {
    seen.until(harness, |s| {
        s.job(kind).is_some_and(|j| {
            j.state == JobState::Running && j.progress.as_deref() == Some("slow remote waiting")
        })
    });
}

/// Lock files git left anywhere under `.git`.
pub fn lock_files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(lock_files(&path));
        } else if path.extension().is_some_and(|e| e == "lock") {
            found.push(path);
        }
    }
    found
}

/// Makes README.md stat-dirty and gives it a clean filter that runs `before` and then
/// passes the content through. Each Unstaged read then runs the filter, through
/// `hash-object`, inside the git it can stop.
pub fn install_slow_clean_filter(repo: &Path, scratch: &Path, before: &str) {
    let script = scratch.join("slow-clean.sh");
    std::fs::write(&script, format!("{before}cat\n")).unwrap();
    git(
        repo,
        &[
            "config",
            "filter.slow.clean",
            &format!("sh '{}'", slash(&script)),
        ],
    );
    std::fs::write(repo.join(".git/info/attributes"), "README.md filter=slow\n").unwrap();
    touch(&repo.join("README.md"));
}

/// Waits until `path` exists, such as a file a hook or filter creates once it runs.
pub fn wait_for_file(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "{} never appeared",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

impl SlowRemote {
    /// Commits to a new branch `pushed`, which a plain `git push` then sends to a branch
    /// the remote doesn't have. Returns git's line for the commit.
    pub fn commit_to_push(&self) -> String {
        git(&self.repo, &["config", "push.default", "current"]);
        git(&self.repo, &["switch", "--quiet", "-c", "pushed"]);
        git(
            &self.repo,
            &["commit", "--quiet", "--allow-empty", "-m", "to push"],
        );
        git(&self.repo, &["rev-parse", "HEAD"])
    }

    /// Whether the remote has the branch [`commit_to_push`](Self::commit_to_push) made.
    pub fn remote_has_pushed(&self) -> bool {
        !git_output(
            &self.remote,
            &["rev-parse", "--verify", "-q", "refs/heads/pushed"],
        )
        .stdout
        .is_empty()
    }
}
