//! Switch Intent to a Local write Job, its outcome, and the refresh that follows it.

mod common;

use std::path::Path;
use std::sync::Arc;

use cairn_repo::{Generation, Head, Intent, JobSnapshot, JobState, RefsSnapshot, Snapshot};
use common::{Harness, git, git_output, git_read, small_fixture_clone};

fn refs_of(snapshot: &Snapshot) -> Option<&Arc<RefsSnapshot>> {
    match snapshot {
        Snapshot::Refs(refs) => Some(refs),
        _ => None,
    }
}

fn job_of(snapshot: &Snapshot) -> Option<&Arc<JobSnapshot>> {
    match snapshot {
        Snapshot::Job(job) => Some(job),
        _ => None,
    }
}

fn is_done(job: &JobSnapshot) -> bool {
    matches!(job.state, JobState::Done(_))
}

/// A clone of the small fixture with a local branch `other` one commit ahead of the
/// default branch, which changes `README.md`. HEAD stays on the default branch.
fn repo_with_other_branch() -> (tempfile::TempDir, std::path::PathBuf, String) {
    let (dir, repo) = small_fixture_clone();
    let start = git(&repo, &["symbolic-ref", "--short", "HEAD"])
        .trim()
        .to_owned();
    git(&repo, &["switch", "--quiet", "-c", "other"]);
    std::fs::write(repo.join("README.md"), "on other\n").unwrap();
    git(
        &repo,
        &["commit", "--quiet", "-am", "change README on other"],
    );
    git(&repo, &["switch", "--quiet", &start]);
    (dir, repo, start)
}

/// Opens `repo` and returns the generation of the refs Snapshot that answers it.
fn open(harness: &Harness, repo: &Path) -> Generation {
    harness.service.send(Intent::Open(repo.to_path_buf()));
    let seen = harness.drain_until(|s| refs_of(s).is_some());
    refs_of(seen.last().unwrap()).unwrap().generation
}

/// Drains until the first Local write Job this test sent has finished, and returns it.
fn wait_for_done(harness: &Harness) -> Arc<JobSnapshot> {
    let seen = harness.drain_until(|s| job_of(s).is_some_and(|j| is_done(j)));
    seen.iter()
        .filter_map(job_of)
        .find(|j| is_done(j))
        .unwrap()
        .clone()
}

fn current_branch(repo: &Path) -> String {
    String::from_utf8(git_read(repo, &["symbolic-ref", "HEAD"]))
        .unwrap()
        .trim()
        .to_owned()
}

#[test]
fn a_switch_runs_git_switch_and_refreshes_refs_and_status() {
    let (_dir, repo, _start) = repo_with_other_branch();
    let harness = Harness::start();
    let opened = open(&harness, &repo);

    harness.service.send(Intent::Switch("other".to_owned()));
    // The refresh may publish before the Job's outcome, so drain for all of them at once.
    // The Open was each status Slot's first generation, so a second one answers the
    // refresh.
    let (mut job, mut refs, mut staged, mut unstaged, mut untracked) =
        (None, None, None, None, None);
    harness.drain_until(|s| {
        match s {
            Snapshot::Job(j) if is_done(j) => job = Some(j.clone()),
            Snapshot::Refs(r) if r.generation > opened => refs = Some(r.clone()),
            Snapshot::Staged(s) if s.generation.get() > 1 => staged = Some(s.clone()),
            Snapshot::Unstaged(s) if s.generation.get() > 1 => unstaged = Some(s.clone()),
            Snapshot::Untracked(s) if s.generation.get() > 1 => untracked = Some(s.clone()),
            _ => {}
        }
        job.is_some()
            && refs.is_some()
            && staged.is_some()
            && unstaged.is_some()
            && untracked.is_some()
    });
    let job = job.unwrap();
    assert_eq!(job.state, JobState::Done(Ok(())), "{job:?}");
    assert_eq!(current_branch(&repo), "refs/heads/other");

    // The refreshed refs show the new HEAD, and after a clean switch status is empty.
    let refs = refs.unwrap();
    let head = &refs.result.as_ref().expect("refs read").head;
    assert!(
        matches!(head, Head::Branch { name, .. } if name.as_str() == "refs/heads/other"),
        "{head:?}"
    );
    assert_eq!(staged.unwrap().result, Ok(Vec::new()));
    assert_eq!(unstaged.unwrap().result, Ok(Vec::new()));
    assert_eq!(untracked.unwrap().result, Ok(Vec::new()));
}

/// Installs a `post-checkout` hook that logs each run to `log`, prints a line, and then
/// blocks until `release` exists.
fn install_blocking_hook(repo: &Path, log: &Path, release: &Path) {
    // git runs hooks with its own sh, which takes forward slashes on Windows too.
    let slash = |p: &Path| p.to_str().unwrap().replace('\\', "/");
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

/// Creates the hook's release file when dropped. Declared after the [`Harness`], so a
/// failing assertion releases the hook before the service waits for its Local write Jobs.
struct Release<'a>(&'a Path);

impl Drop for Release<'_> {
    fn drop(&mut self) {
        let _ = std::fs::write(self.0, "");
    }
}

/// The newest Snapshot of every Job seen so far, kept across drains.
#[derive(Default)]
struct Jobs(std::collections::BTreeMap<cairn_repo::JobId, Arc<JobSnapshot>>);

impl Jobs {
    /// Drains until `done` accepts the Jobs seen so far.
    fn until(&mut self, harness: &Harness, mut done: impl FnMut(&Self) -> bool) {
        if done(self) {
            return;
        }
        harness.drain_until(|s| {
            if let Some(job) = job_of(s) {
                self.0.insert(job.id, job.clone());
            }
            done(self)
        });
    }

    fn nth(&self, n: usize) -> Option<&Arc<JobSnapshot>> {
        self.0.values().nth(n)
    }
}

#[test]
fn local_write_jobs_run_one_at_a_time_in_order_and_both_finish() {
    let (dir, repo, start) = repo_with_other_branch();
    git(&repo, &["branch", "third", &start]);
    let log = dir.path().join("hook.log");
    let release = dir.path().join("release");
    install_blocking_hook(&repo, &log, &release);
    let harness = Harness::start();
    let _release = Release(&release);
    open(&harness, &repo);

    harness.service.send(Intent::Switch("other".to_owned()));
    let mut jobs = Jobs::default();
    // The hook's output arrives while the hook is still blocked, so it is live.
    jobs.until(&harness, |j| {
        j.nth(0).is_some_and(|first| {
            first.state == JobState::Running && first.output.contains("hook waiting on other")
        })
    });

    harness.service.send(Intent::Switch("third".to_owned()));
    jobs.until(&harness, |j| j.nth(1).is_some());
    let second = jobs.nth(1).unwrap();
    assert_eq!(second.state, JobState::Queued, "{second:?}");
    assert_eq!(
        second.kind,
        cairn_repo::JobKind::Switch {
            branch: "third".to_owned()
        }
    );
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "start other\n");

    std::fs::write(&release, "").unwrap();
    jobs.until(&harness, |j| {
        j.0.len() == 2 && j.0.values().all(|job| is_done(job))
    });
    for job in jobs.0.values() {
        assert_eq!(job.state, JobState::Done(Ok(())), "{job:?}");
    }
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        "start other\nend other\nstart third\nend third\n",
        "the second switch ran only after the first finished"
    );
    assert_eq!(current_branch(&repo), "refs/heads/third");
    assert!(!repo.join(".git/index.lock").exists());
}

/// What git itself says when asked to switch `repo` to `branch` from a terminal.
fn terminal_switch_error(repo: &Path, branch: &str) -> String {
    let out = git_output(repo, &["switch", "--no-guess", "--", branch]);
    assert!(!out.status.success(), "the terminal switch must fail too");
    out.stderr_text().into_owned()
}

#[test]
fn a_switch_beside_a_held_index_lock_publishes_gits_error_and_leaves_the_lock_alone() {
    let (_dir, repo, start) = repo_with_other_branch();
    let harness = Harness::start();
    open(&harness, &repo);
    let lock = repo.join(".git/index.lock");
    std::fs::write(&lock, "held by the terminal\n").unwrap();
    let modified = std::fs::metadata(&lock).unwrap().modified().unwrap();

    harness.service.send(Intent::Switch("other".to_owned()));
    let job = wait_for_done(&harness);

    let JobState::Done(Err(error)) = &job.state else {
        panic!("the switch must fail while the index is locked: {job:?}");
    };
    assert!(error.contains("index.lock"), "{error}");
    assert_eq!(error, &terminal_switch_error(&repo, "other"));
    assert_eq!(
        std::fs::read_to_string(&lock).unwrap(),
        "held by the terminal\n"
    );
    assert_eq!(
        std::fs::metadata(&lock).unwrap().modified().unwrap(),
        modified
    );
    assert_eq!(current_branch(&repo), format!("refs/heads/{start}"));
}

#[test]
fn a_switch_refused_for_local_changes_publishes_gits_message() {
    let (_dir, repo, start) = repo_with_other_branch();
    let harness = Harness::start();
    open(&harness, &repo);
    // `other` changes README.md, so git refuses to overwrite this edit.
    std::fs::write(repo.join("README.md"), "edited in the worktree\n").unwrap();

    harness.service.send(Intent::Switch("other".to_owned()));
    let job = wait_for_done(&harness);

    let JobState::Done(Err(error)) = &job.state else {
        panic!("git must refuse the switch: {job:?}");
    };
    assert!(error.contains("would be overwritten"), "{error}");
    assert_eq!(error, &terminal_switch_error(&repo, "other"));
    assert_eq!(current_branch(&repo), format!("refs/heads/{start}"));
    assert_eq!(
        std::fs::read_to_string(repo.join("README.md")).unwrap(),
        "edited in the worktree\n"
    );
}
