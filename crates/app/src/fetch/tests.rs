//! The Fetch control in the real window, with the real Repo service and git behind it: a
//! fetch from a real local remote, its progress and Stop, and git's own error.

use std::path::{Path, PathBuf};

use cairn_repo::{JobKind, JobState};
use gpui::TestAppContext;

use crate::jobs::Banner;
use crate::testing::{
    App, Release, block_until, git, hermetic, open, repo_with_commit, slash, slow_remote,
};

/// A clone `copy` of the repository `origin`, which has `main` and `gone`.
struct Fixture {
    repo: PathBuf,
    origin: PathBuf,
    dir: tempfile::TempDir,
}

fn fixture() -> Fixture {
    hermetic();
    let dir = tempfile::tempdir().unwrap();
    let origin = repo_with_commit(dir.path(), "origin");
    git(&origin, &["branch", "gone"]);
    let repo = dir.path().join("copy");
    git(
        dir.path(),
        &[
            "clone",
            "--quiet",
            origin.to_str().unwrap(),
            repo.to_str().unwrap(),
        ],
    );
    Fixture { repo, origin, dir }
}

/// Opens `repo` and waits until the sidebar shows `origin/main`.
fn open_with_remote<'a>(cx: &'a mut TestAppContext, repo: &Path) -> App<'a> {
    let mut app = open(cx, Some(repo));
    app.until("the sidebar shows origin/main", |app| {
        app.draw();
        app.shown("sidebar:refs/remotes/origin/main")
    });
    app
}

#[gpui::test]
fn pressing_fetch_shows_a_new_remote_branch_and_drops_a_deleted_one_once_the_job_is_done(
    cx: &mut TestAppContext,
) {
    let f = fixture();
    git(&f.origin, &["branch", "new"]);
    git(&f.origin, &["branch", "--quiet", "-D", "gone"]);
    let mut app = open_with_remote(cx, &f.repo);
    assert!(!app.shown("sidebar:refs/remotes/origin/new"));
    assert!(app.shown("sidebar:refs/remotes/origin/gone"));

    app.click("toolbar:fetch");
    // The refs refresh comes before the Job's outcome, and the watcher can show refs read
    // mid-fetch, so wait for both.
    app.until("the fetch is done and origin/new is shown", |app| {
        app.draw();
        app.shown("job:done") && app.shown("sidebar:refs/remotes/origin/new")
    });
    assert_eq!(app.jobs_seen(), [(JobKind::Fetch, JobState::Done(Ok(())))]);
    // Seen red: the service's fetch without `--prune` leaves it shown.
    assert!(!app.shown("sidebar:refs/remotes/origin/gone"));
}

#[gpui::test]
fn the_keyboard_shortcut_fetches(cx: &mut TestAppContext) {
    let f = fixture();
    git(&f.origin, &["branch", "new"]);
    let mut app = open_with_remote(cx, &f.repo);

    // Seen red: without `track_focus` on the window's root, nothing handles the key.
    press_shortcut(&mut app);
    app.until("the fetch is done and origin/new is shown", |app| {
        app.draw();
        app.shown("job:done") && app.shown("sidebar:refs/remotes/origin/new")
    });

    // With focus in the filter box, the shortcut still fetches.
    app.click("sidebar:filter");
    press_shortcut(&mut app);
    app.until("a second fetch is done", |app| {
        app.jobs_seen()
            .iter()
            .filter(|(_, state)| *state == JobState::Done(Ok(())))
            .count()
            == 2
    });
}

fn press_shortcut(app: &mut App) {
    #[cfg(target_os = "macos")]
    app.cx.simulate_keystrokes("cmd-shift-f");
    #[cfg(not(target_os = "macos"))]
    app.cx.simulate_keystrokes("ctrl-shift-f");
}

#[gpui::test]
fn fetch_is_disabled_while_no_repository_is_open(cx: &mut TestAppContext) {
    let f = fixture();
    let mut app = open(cx, None);
    app.draw();

    app.click("toolbar:fetch");
    press_shortcut(&mut app);
    // The marker: a fetch asked for before the Open would have published its Snapshot
    // before this fetch's.
    app.open_repo(&f.repo);
    app.until("the sidebar shows origin/main", |app| {
        app.draw();
        app.shown("sidebar:refs/remotes/origin/main")
    });
    app.click("toolbar:fetch");
    app.until("the fetch is done", |app| {
        app.jobs_seen()
            .iter()
            .any(|(_, state)| *state == JobState::Done(Ok(())))
    });

    // Seen red: a Workspace::fetch without its `opened` check.
    assert_eq!(app.jobs_seen(), [(JobKind::Fetch, JobState::Done(Ok(())))]);
}

#[gpui::test]
fn a_slow_fetch_shows_the_remotes_latest_line_and_stop_ends_it_as_stopped(cx: &mut TestAppContext) {
    let f = fixture();
    let release = Release(f.dir.path().join("release"));
    slow_remote(
        &f.repo,
        f.dir.path(),
        &format!(
            "echo 'slow remote waiting' >&2\n{}",
            block_until(&release.0, f.dir.path())
        ),
    );
    let mut app = open_with_remote(cx, &f.repo);

    app.click("toolbar:fetch");
    app.until("the fetch shows running with the remote's line", |app| {
        app.draw();
        app.banner()
            == Some(Banner::Running {
                title: "git fetch".to_owned(),
                output: Some("slow remote waiting".to_owned()),
            })
    });
    assert!(app.shown("job:output"));

    app.click("job:stop");
    app.until("the fetch shows stopped", |app| {
        app.draw();
        app.shown("job:stopped")
    });
    assert!(!app.shown("job:failed"));
    assert_eq!(app.jobs_seen(), [(JobKind::Fetch, JobState::Stopped)]);
    assert!(!release.0.exists(), "the remote was released, not stopped");
}

#[gpui::test]
fn a_remote_that_refuses_shows_gits_own_stderr(cx: &mut TestAppContext) {
    let f = fixture();
    let mut app = open_with_remote(cx, &f.repo);
    let missing = f.dir.path().join("missing");
    git(
        &f.repo,
        &["remote", "set-url", "origin", missing.to_str().unwrap()],
    );

    app.click("toolbar:fetch");
    app.until("the fetch fails", |app| {
        app.draw();
        matches!(app.banner(), Some(Banner::Failed { .. }))
    });
    let Some(Banner::Failed { title, error }) = app.banner() else {
        unreachable!()
    };
    assert_eq!(title, "git fetch");
    let reported = app
        .jobs_seen()
        .into_iter()
        .find_map(|(kind, state)| match state {
            JobState::Done(Err(error)) if kind == JobKind::Fetch => Some(error),
            _ => None,
        });
    // Seen red: a banner that shows only git's first line.
    assert_eq!(
        Some(&error),
        reported.as_ref(),
        "not as the Job reported it"
    );
    assert!(
        error.contains("does not appear to be a git repository"),
        "{error}"
    );
    assert!(app.shown("job:failed"));
    assert!(!app.shown("job:stop"), "an ended fetch offers Stop");
}

#[gpui::test]
fn pressing_fetch_again_while_one_is_running_starts_no_other(cx: &mut TestAppContext) {
    let f = fixture();
    let release = Release(f.dir.path().join("release"));
    let log = f.dir.path().join("remote.log");
    slow_remote(
        &f.repo,
        f.dir.path(),
        &format!(
            "echo run >> '{}'\necho 'slow remote waiting' >&2\n{}",
            slash(&log),
            block_until(&release.0, f.dir.path())
        ),
    );
    let mut app = open_with_remote(cx, &f.repo);

    // The second press comes before the first fetch's Snapshot has reached the window.
    app.click("toolbar:fetch");
    app.click("toolbar:fetch");
    app.until("the fetch is running", |app| {
        app.draw();
        app.shown("job:running")
    });
    app.click("toolbar:fetch");
    release.release();
    // The marker: a second fetch's Queued Snapshot is published as it is asked for, so it
    // would reach the window before the first fetch's outcome.
    app.until("the fetch is done", |app| {
        app.jobs_seen()
            .iter()
            .any(|(_, state)| *state == JobState::Done(Ok(())))
    });

    assert_eq!(app.jobs_seen(), [(JobKind::Fetch, JobState::Done(Ok(())))]);
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "run\n");
}
