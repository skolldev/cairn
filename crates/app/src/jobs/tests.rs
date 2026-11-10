//! Double-clicking a local branch in the real window, with the real Repo service and git
//! behind it: the switch, the Job banner while it runs, and git's own error when it refuses.

use std::path::{Path, PathBuf};

use cairn_repo::{JobKind, JobState};
use gpui::TestAppContext;

use super::Banner;
use crate::testing::{App, git, hermetic, open};

/// `path` as git's own sh reads it: forward slashes, on Windows too.
fn slash(path: &Path) -> String {
    path.to_str().unwrap().replace('\\', "/")
}

/// A repository on `main` with branches `other` and `third`. `file.txt` holds `one` on
/// `main` and `third`, and `two` on `other`.
struct Fixture {
    repo: PathBuf,
    dir: tempfile::TempDir,
}

fn fixture() -> Fixture {
    hermetic();
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("copy");
    git(
        dir.path(),
        &[
            "init",
            "--quiet",
            "--initial-branch=main",
            repo.to_str().unwrap(),
        ],
    );
    std::fs::write(repo.join("file.txt"), "one\n").unwrap();
    git(&repo, &["add", "file.txt"]);
    git(&repo, &["commit", "--quiet", "-m", "base"]);
    git(&repo, &["branch", "third"]);
    git(&repo, &["switch", "--quiet", "-c", "other"]);
    std::fs::write(repo.join("file.txt"), "two\n").unwrap();
    git(&repo, &["commit", "--quiet", "-am", "two"]);
    git(&repo, &["switch", "--quiet", "main"]);
    Fixture { repo, dir }
}

impl Fixture {
    fn head(&self) -> String {
        git(&self.repo, &["symbolic-ref", "HEAD"]).trim().to_owned()
    }
}

/// Opens `repo` and waits until the sidebar shows `main` as current.
fn open_on_main<'a>(cx: &'a mut TestAppContext, repo: &Path) -> App<'a> {
    let mut app = open(cx, Some(repo));
    app.until("the sidebar shows main as current", |app| {
        app.draw();
        app.shown("sidebar:current:refs/heads/main") && app.shown("sidebar:refs/heads/other")
    });
    app
}

impl App<'_> {
    fn banner(&mut self) -> Option<Banner> {
        self.workspace
            .read_with(self.cx, |workspace, cx| workspace.jobs.read(cx).banner())
    }

    /// The kind and state of every Job the window has seen, oldest first.
    fn jobs_seen(&mut self) -> Vec<(JobKind, JobState)> {
        self.workspace.read_with(self.cx, |workspace, cx| {
            workspace
                .jobs
                .read(cx)
                .seen()
                .map(|job| (job.kind.clone(), job.state.clone()))
                .collect()
        })
    }
}

fn switch(branch: &str) -> JobKind {
    JobKind::Switch {
        branch: branch.to_owned(),
    }
}

#[gpui::test]
fn double_clicking_a_branch_switches_to_it(cx: &mut TestAppContext) {
    let f = fixture();
    let mut app = open_on_main(cx, &f.repo);

    app.double_click("sidebar:refs/heads/other");
    // Seen red: a row that switches only on a third click times out here.
    app.until("the sidebar marks other as current", |app| {
        app.draw();
        app.shown("sidebar:current:refs/heads/other")
    });

    assert_eq!(f.head(), "refs/heads/other");
    assert!(!app.shown("sidebar:current:refs/heads/main"));
}

#[gpui::test]
fn a_switch_git_refuses_shows_gits_own_stderr_until_dismissed(cx: &mut TestAppContext) {
    let f = fixture();
    std::fs::write(f.repo.join("file.txt"), "local\n").unwrap();
    let mut app = open_on_main(cx, &f.repo);

    app.double_click("sidebar:refs/heads/other");
    app.until("the switch fails", |app| {
        app.draw();
        matches!(app.banner(), Some(Banner::Failed { .. }))
    });

    let Some(Banner::Failed { error, .. }) = app.banner() else {
        unreachable!()
    };
    let reported = app
        .jobs_seen()
        .into_iter()
        .find_map(|(kind, state)| match state {
            JobState::Done(Err(error)) if kind == switch("other") => Some(error),
            _ => None,
        });
    // Seen red: a banner that trims the error, or adds a word of its own.
    assert_eq!(
        Some(&error),
        reported.as_ref(),
        "not as the Job reported it"
    );
    assert!(
        error.starts_with(
            "error: Your local changes to the following files would be overwritten by checkout:"
        ),
        "{error}"
    );
    assert!(error.contains("file.txt"), "{error}");
    assert!(app.shown("job:failed"));
    assert_eq!(f.head(), "refs/heads/main");

    app.click("job:dismiss");
    // Seen red: a Dismiss that does nothing.
    assert_eq!(app.banner(), None);
    assert!(!app.shown("job:failed"));
}

#[gpui::test]
fn starting_another_job_replaces_a_failed_ones_error(cx: &mut TestAppContext) {
    let f = fixture();
    std::fs::write(f.repo.join("file.txt"), "local\n").unwrap();
    let mut app = open_on_main(cx, &f.repo);
    app.double_click("sidebar:refs/heads/other");
    app.until("the switch fails", |app| {
        matches!(app.banner(), Some(Banner::Failed { .. }))
    });

    // `third` has the same `file.txt` as `main`, so the local change goes with it.
    app.double_click("sidebar:refs/heads/third");
    // Seen red: a banner that keeps showing the oldest Job.
    app.until("the new Job shows", |app| {
        matches!(
            app.banner(),
            Some(Banner::Done { title }) if title == "git switch third"
        )
    });
    assert_eq!(f.head(), "refs/heads/third");
}

#[gpui::test]
fn a_slow_post_checkout_hook_shows_the_job_running_with_its_latest_line_then_done(
    cx: &mut TestAppContext,
) {
    let f = fixture();
    let release = f.dir.path().join("release");
    let hook = f.repo.join(".git/hooks/post-checkout");
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\n\
             echo 'hook starting'\n\
             echo 'hook waiting'\n\
             while [ ! -f '{release}' ] && [ -d '{dir}' ]; do sleep 0.05; done\n",
            release = slash(&release),
            dir = slash(f.dir.path()),
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut app = open_on_main(cx, &f.repo);

    app.double_click("sidebar:refs/heads/other");
    app.until("the Job shows running with the hook's latest line", |app| {
        app.draw();
        app.banner()
            == Some(Banner::Running {
                title: "git switch other".to_owned(),
                output: Some("hook waiting".to_owned()),
            })
    });
    assert!(app.shown("job:running"));
    // Seen red: a banner that leaves out the output line.
    assert!(app.shown("job:output"));

    std::fs::write(&release, "").unwrap();
    app.until("the Job shows done", |app| {
        app.draw();
        app.shown("job:done")
    });
    // Seen red: a done Job shown for good.
    app.until("the done Job leaves the screen", |app| {
        app.draw();
        !app.shown("job")
    });
}

#[gpui::test]
fn double_clicking_the_current_branch_sends_no_intent(cx: &mut TestAppContext) {
    let f = fixture();
    let mut app = open_on_main(cx, &f.repo);

    app.double_click("sidebar:refs/heads/main");
    // The marker: Local write Jobs run in order, so a switch to `main` would have been
    // seen before this one ended.
    app.double_click("sidebar:refs/heads/other");
    app.until("the switch to other is done", |app| {
        app.jobs_seen()
            .iter()
            .any(|(kind, state)| *kind == switch("other") && *state == JobState::Done(Ok(())))
    });

    // Seen red: `switch_to` without its current-branch check.
    assert_eq!(
        app.jobs_seen(),
        [(switch("other"), JobState::Done(Ok(())))],
        "a Job besides the switch to other"
    );
}
