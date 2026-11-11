//! Double-clicking a local branch in the real window, with the real Repo service and git
//! behind it: the switch, the Job banner while it runs, and git's own error when it refuses.

use std::path::{Path, PathBuf};

use cairn_repo::{JobKind, JobState, RefName};
use gpui::TestAppContext;

use super::Banner;
use crate::testing::{
    App, Release, block_until, git, hermetic, open, post_checkout_hook, repo_with_commit,
};

/// A repository on `main` with branches `other` and `third`. `file.txt` holds `one` on
/// `main` and `third`, and `two` on `other`.
struct Fixture {
    repo: PathBuf,
    dir: tempfile::TempDir,
}

fn fixture() -> Fixture {
    hermetic();
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_with_commit(dir.path(), "copy");
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
    let release = Release(f.dir.path().join("release"));
    post_checkout_hook(
        &f.repo,
        &format!(
            "echo 'hook starting'\necho 'hook waiting'\n{}",
            block_until(&release.0, f.dir.path())
        ),
    );
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

    release.release();
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

fn stash(untracked: bool) -> JobKind {
    JobKind::Stash { untracked }
}

impl App<'_> {
    /// Double-clicks `other` and waits for git's refusal to show.
    fn refused_switch_to_other(&mut self) -> Banner {
        self.double_click("sidebar:refs/heads/other");
        self.until("the switch fails", |app| {
            app.draw();
            matches!(app.banner(), Some(Banner::Failed { .. }))
        });
        self.banner().unwrap()
    }
}

#[gpui::test]
fn stash_and_switch_on_a_refusal_for_local_changes_stashes_them_and_switches(
    cx: &mut TestAppContext,
) {
    let f = fixture();
    std::fs::write(f.repo.join("file.txt"), "local\n").unwrap();
    let mut app = open_on_main(cx, &f.repo);

    let refused = app.refused_switch_to_other();
    assert!(
        matches!(
            refused,
            Banner::Failed {
                stash_and_switch: true,
                ..
            }
        ),
        "{refused:?}"
    );
    assert!(app.shown("job:dismiss"));
    app.click("job:stash-and-switch");
    // Seen red: a button that sends nothing times out here.
    app.until(
        "the sidebar marks other as current and shows the stash",
        |app| {
            app.draw();
            app.shown("sidebar:current:refs/heads/other") && app.shown("sidebar:stash@{0}")
        },
    );

    assert_eq!(f.head(), "refs/heads/other");
    assert_eq!(
        git(&f.repo, &["stash", "list", "--format=%gs"]),
        "On main: WIP on main\n"
    );
    app.until("both Jobs are done", |app| {
        app.jobs_seen()
            .iter()
            .filter(|(_, s)| *s == JobState::Done(Ok(())))
            .count()
            == 2
    });
    assert_eq!(
        app.jobs_seen()[1..],
        [
            (stash(false), JobState::Done(Ok(()))),
            (switch("other"), JobState::Done(Ok(()))),
        ]
    );
}

#[gpui::test]
fn stash_and_switch_on_a_refusal_for_untracked_files_stashes_them_too(cx: &mut TestAppContext) {
    let f = fixture();
    // Tracked on `other`, so the switch would overwrite it.
    git(&f.repo, &["switch", "--quiet", "other"]);
    std::fs::write(f.repo.join("only-on-other.txt"), "tracked\n").unwrap();
    git(&f.repo, &["add", "only-on-other.txt"]);
    git(&f.repo, &["commit", "--quiet", "-m", "only on other"]);
    git(&f.repo, &["switch", "--quiet", "main"]);
    std::fs::write(f.repo.join("only-on-other.txt"), "untracked\n").unwrap();
    let mut app = open_on_main(cx, &f.repo);

    app.refused_switch_to_other();
    app.click("job:stash-and-switch");
    app.until("the sidebar marks other as current", |app| {
        app.draw();
        app.shown("sidebar:current:refs/heads/other")
    });

    assert_eq!(f.head(), "refs/heads/other");
    // Seen red: an offer that always says `untracked: false` leaves the switch refused.
    assert!(
        app.jobs_seen()
            .contains(&(stash(true), JobState::Done(Ok(()))))
    );
}

#[gpui::test]
fn a_switch_that_fails_for_another_reason_offers_only_dismiss(cx: &mut TestAppContext) {
    let f = fixture();
    let mut app = open_on_main(cx, &f.repo);
    std::fs::write(f.repo.join(".git/index.lock"), "held by the terminal\n").unwrap();

    let failed = app.refused_switch_to_other();
    // Control: `stash_and_switch_on_a_refusal_for_local_changes_stashes_them_and_switches`.
    // Seen red: a banner that offers it on every failure.
    assert!(
        matches!(
            failed,
            Banner::Failed {
                stash_and_switch: false,
                ..
            }
        ),
        "{failed:?}"
    );
    assert!(app.shown("job:dismiss"));
    assert!(!app.shown("job:stash-and-switch"));
}

#[gpui::test]
fn dismissing_a_refusal_hides_it_and_sends_nothing(cx: &mut TestAppContext) {
    let f = fixture();
    std::fs::write(f.repo.join("file.txt"), "local\n").unwrap();
    let mut app = open_on_main(cx, &f.repo);

    app.refused_switch_to_other();
    app.click("job:dismiss");
    assert_eq!(app.banner(), None);
    assert!(!app.shown("job:stash-and-switch"));

    // Marker: Local write Jobs run in order, so a stash the Dismiss sent would be seen before
    // this switch ended. `third` has `main`'s `file.txt`, so the change goes with it.
    app.double_click("sidebar:refs/heads/third");
    app.until("the switch to third is done", |app| {
        app.jobs_seen()
            .iter()
            .any(|(kind, state)| *kind == switch("third") && *state == JobState::Done(Ok(())))
    });
    // Seen red: a Dismiss that also sends the offer puts a stash between them.
    let kinds: Vec<_> = app.jobs_seen().into_iter().map(|(kind, _)| kind).collect();
    assert_eq!(kinds, [switch("other"), switch("third")]);
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

impl Fixture {
    /// Adds a remote `origin` whose `feature/x` is at `other`, with no local branch for it.
    fn add_remote_feature_x(&self) {
        let bare = self.dir.path().join("origin.git");
        let bare = bare.to_str().unwrap();
        git(self.dir.path(), &["init", "--quiet", "--bare", bare]);
        git(&self.repo, &["remote", "add", "origin", bare]);
        git(
            &self.repo,
            &["push", "--quiet", "origin", "other:feature/x"],
        );
        git(&self.repo, &["fetch", "--quiet", "origin"]);
    }
}

/// Gives `f` the remote branch `origin/feature/x`, then opens it and waits until the sidebar
/// shows that branch and `main` as current.
fn open_with_remote_feature_x<'a>(cx: &'a mut TestAppContext, f: &Fixture) -> App<'a> {
    f.add_remote_feature_x();
    let mut app = open_on_main(cx, &f.repo);
    app.until("the sidebar shows origin/feature/x", |app| {
        app.draw();
        app.shown("sidebar:refs/remotes/origin/feature/x")
    });
    app
}

fn feature_x() -> JobKind {
    JobKind::SwitchToRemote {
        remote: RefName::new("refs/remotes/origin/feature/x"),
    }
}

#[gpui::test]
fn double_clicking_a_remote_branch_switches_to_it_through_a_new_tracking_branch(
    cx: &mut TestAppContext,
) {
    let f = fixture();
    let mut app = open_with_remote_feature_x(cx, &f);

    app.double_click("sidebar:refs/remotes/origin/feature/x");
    // The refresh can show the switch before the Job's end arrives, so wait for both.
    app.until(
        "the sidebar marks feature/x as current and the Job ends",
        |app| {
            app.draw();
            app.shown("sidebar:current:refs/heads/feature/x")
                && app
                    .jobs_seen()
                    .iter()
                    .any(|(_, state)| matches!(state, JobState::Done(_)))
        },
    );

    assert_eq!(app.jobs_seen(), [(feature_x(), JobState::Done(Ok(())))]);
    assert_eq!(f.head(), "refs/heads/feature/x");
    assert_eq!(
        git(&f.repo, &["config", "branch.feature/x.merge"]).trim(),
        "refs/heads/feature/x"
    );
}

#[gpui::test]
fn stash_and_switch_on_a_refused_switch_to_a_remote_branch_stashes_then_makes_that_switch(
    cx: &mut TestAppContext,
) {
    let f = fixture();
    // `feature/x` is at `other`, which changes `file.txt`.
    std::fs::write(f.repo.join("file.txt"), "local\n").unwrap();
    let mut app = open_with_remote_feature_x(cx, &f);

    app.double_click("sidebar:refs/remotes/origin/feature/x");
    app.until("the switch fails", |app| {
        app.draw();
        matches!(app.banner(), Some(Banner::Failed { .. }))
    });
    // Seen red: offering Stash and switch only for a switch to a local branch.
    assert!(
        matches!(
            app.banner(),
            Some(Banner::Failed {
                stash_and_switch: true,
                ..
            })
        ),
        "{:?}",
        app.banner()
    );
    app.click("job:stash-and-switch");
    app.until(
        "the sidebar marks feature/x as current and shows the stash",
        |app| {
            app.draw();
            app.shown("sidebar:current:refs/heads/feature/x") && app.shown("sidebar:stash@{0}")
        },
    );

    assert_eq!(f.head(), "refs/heads/feature/x");
    app.until("all three Jobs are done", |app| {
        app.jobs_seen()
            .iter()
            .filter(|(_, s)| matches!(s, JobState::Done(_)))
            .count()
            == 3
    });
    assert_eq!(
        app.jobs_seen()[1..],
        [
            (stash(false), JobState::Done(Ok(()))),
            (feature_x(), JobState::Done(Ok(()))),
        ]
    );
}
