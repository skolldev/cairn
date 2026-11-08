//! The real window, Repo service and git in a temp repository: opening a repo, closing the
//! window, and answering its prompts.
//! The test platform's `quit` does nothing, so each test watches the exit hook instead.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use cairn_repo::Intent;
use cairn_write::{Git, Invocation, JobClass};
use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, px, size};

use crate::Workspace;

/// How long a test waits when nothing marks what must not happen. A close that exits
/// does so well within this: `closing_with_nothing_running_exits_without_asking` is the
/// control.
const SETTLE: Duration = Duration::from_millis(300);

/// Points git at an empty home, once per test binary, so a developer's config can't change
/// the result. Runs before anything spawns git.
fn hermetic() {
    static HOME: OnceLock<tempfile::TempDir> = OnceLock::new();
    HOME.get_or_init(|| {
        let home = tempfile::tempdir().expect("temp home");
        // SAFETY: this runs inside `OnceLock::get_or_init`, which blocks every other test
        // thread at the same call until it returns, and nothing spawns git before it.
        unsafe {
            std::env::set_var("HOME", home.path());
            std::env::set_var("XDG_CONFIG_HOME", home.path().join(".config"));
            std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        }
        home
    });
}

fn git(repo: &Path, args: &[&str]) -> String {
    hermetic();
    let invocation = Invocation::new(JobClass::LocalWrite, repo)
        .args([
            "-c",
            "user.name=Cairn Test",
            "-c",
            "user.email=test@cairn.invalid",
        ])
        .args(args.iter().copied());
    let out = Git::shared()
        .expect("a usable git is installed")
        .run(&invocation)
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        out.stderr_text()
    );
    String::from_utf8(out.stdout).expect("git output is UTF-8")
}

/// `path` as git's own sh reads it: forward slashes, on Windows too.
fn slash(path: &Path) -> String {
    path.to_str().unwrap().replace('\\', "/")
}

/// Creates the release file when dropped, so a failing test leaves nothing blocked on it.
struct Release(PathBuf);

impl Drop for Release {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.0, "");
    }
}

/// A repository with a branch `other` and a commit to push. A `post-checkout` hook and a
/// wrapper around git on the remote each block until the release file exists or the temp dir
/// is gone: a Force quit leaves its hook running after the test has ended.
struct Fixture {
    repo: PathBuf,
    remote: PathBuf,
    release: Release,
    /// Created by the hook and the wrapper once they are blocked.
    blocked: PathBuf,
    // Last, so that it drops after `release`.
    _dir: tempfile::TempDir,
}

fn fixture() -> Fixture {
    hermetic();
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("copy");
    let remote = dir.path().join("remote.git");
    let release = dir.path().join("release");
    let blocked = dir.path().join("blocked");
    let path = |p: &Path| p.to_str().unwrap().to_owned();
    git(dir.path(), &["init", "--quiet", &path(&repo)]);
    git(dir.path(), &["init", "--quiet", "--bare", &path(&remote)]);
    git(&repo, &["commit", "--quiet", "--allow-empty", "-m", "base"]);
    git(&repo, &["branch", "other"]);
    git(&repo, &["remote", "add", "origin", &path(&remote)]);
    git(&repo, &["config", "push.default", "current"]);

    let block = format!(
        "touch '{blocked}'\n\
         while [ ! -f '{release}' ] && [ -d '{dir}' ]; do sleep 0.05; done\n",
        blocked = slash(&blocked),
        release = slash(&release),
        dir = slash(dir.path()),
    );
    let hook = repo.join(".git/hooks/post-checkout");
    std::fs::write(&hook, format!("#!/bin/sh\n{block}")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let wrapper = dir.path().join("slow-remote.sh");
    std::fs::write(&wrapper, format!("{block}exec git \"$@\"\n")).unwrap();
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
    Fixture {
        repo,
        remote,
        release: Release(release),
        blocked,
        _dir: dir,
    }
}

impl Fixture {
    fn release(&self) {
        std::fs::write(&self.release.0, "").unwrap();
    }

    /// Whether the push reached the remote: the bare remote started with no commits.
    fn remote_has_the_push(&self) -> bool {
        !git(&self.remote, &["for-each-ref"]).is_empty()
    }
}

/// The real window, whose exit hook sets a flag instead of ending the test process.
struct App<'a> {
    cx: &'a mut VisualTestContext,
    workspace: Entity<Workspace>,
    exited: Rc<Cell<bool>>,
}

fn open<'a>(cx: &'a mut TestAppContext, repo: Option<&Path>) -> App<'a> {
    // Real git runs on the service's threads, so the executor must wait for them.
    cx.executor().allow_parking();
    let exited = Rc::new(Cell::new(false));
    let exit = exited.clone();
    let path = repo.map(Path::to_path_buf);
    let (workspace, cx) = cx.add_window_view(move |window, cx| {
        Workspace::new(path, Rc::new(move |_| exit.set(true)), window, cx)
    });
    App {
        cx,
        workspace,
        exited,
    }
}

impl App<'_> {
    /// Runs the app until `done` holds. Snapshots arrive from the service's threads.
    fn until(&mut self, what: &str, mut done: impl FnMut(&mut Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            // The test window draws no frames, so the Bridge drains on its timer, and that
            // runs on the executor's clock.
            self.cx.executor().advance_clock(Duration::from_millis(100));
            self.cx.run_until_parked();
            if done(self) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting until {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Runs the app while two reads of the repository finish, and for at least [`SETTLE`].
    /// A wrong Quit still needs the Network queue and Bridge to publish and show its exit.
    fn settle(&mut self, repo: &Path) {
        let end = Instant::now() + SETTLE;
        for _ in 0..2 {
            git(repo, &["diff-files", "-z"]);
            git(repo, &["hash-object", ".git/HEAD"]);
            self.cx.executor().advance_clock(Duration::from_millis(100));
            self.cx.run_until_parked();
        }
        self.until("the wait ends", |_| Instant::now() >= end);
    }

    fn send(&mut self, intent: Intent) {
        self.workspace
            .read_with(self.cx, |workspace, _| workspace.service.send(intent));
    }

    /// A user's click on the window's close button. It never closes the window itself: the
    /// app exits once the Repo service has quit.
    fn close(&mut self) {
        assert!(!self.cx.simulate_close(), "the window closed by itself");
        self.cx.run_until_parked();
    }

    fn prompt(&self) -> Option<String> {
        self.cx.pending_prompt().map(|(message, _)| message)
    }

    fn answer(&mut self, button: &str) {
        self.cx.simulate_prompt_answer(button);
        self.cx.run_until_parked();
    }

    /// Whether the banner is up, saying a Quit waits on a Local write Job.
    fn banner_shown(&mut self) -> bool {
        self.workspace.read_with(self.cx, |workspace, cx| {
            workspace.quit.read(cx).flow.waiting_on().is_some()
        })
    }

    /// Sends `intent` and waits until git is blocked and the Job's Snapshot reached the view.
    fn start_blocked(&mut self, intent: Intent, f: &Fixture) {
        self.send(intent);
        self.until("the Job is blocked and shown", |app| {
            f.blocked.exists()
                && app.workspace.read_with(app.cx, |workspace, cx| {
                    !workspace.quit.read(cx).flow.jobs.is_empty()
                })
        });
    }

    fn start_a_blocked_switch(&mut self, f: &Fixture) {
        self.start_blocked(Intent::Switch("other".to_owned()), f);
    }

    /// A close under a blocked hook, leaving the Quit waiting on it.
    fn quit_under_a_blocked_hook(&mut self, f: &Fixture) {
        self.start_a_blocked_switch(f);
        self.close();
        self.until("the banner is up", |app| app.banner_shown());
    }

    /// Fails if the app exits during the work-sized window.
    fn assert_stays_open(&mut self, repo: &Path, why: &str) {
        self.settle(repo);
        assert!(!self.exited.get(), "{why}");
    }
}

#[gpui::test]
fn closing_with_nothing_running_exits_without_asking(cx: &mut TestAppContext) {
    let mut app = open(cx, None);
    // Seen red: a window that closes by itself fails `close`.
    app.close();
    assert_eq!(app.prompt(), None);
    app.until("the app exits", |app| app.exited.get());
}

#[gpui::test]
fn opening_a_repo_shows_its_refs_and_status_in_the_window(cx: &mut TestAppContext) {
    let f = fixture();
    std::fs::write(f.repo.join("untracked-marker.txt"), "new\n").unwrap();
    let mut app = open(cx, Some(&f.repo));

    app.until("the branch and untracked file are rendered", |app| {
        // The test window draws only on resize. These selectors belong to rendered rows,
        // so seeing both proves that the Bridge delivered both kinds of Snapshot. Seen
        // red: dropping either Refs or Untracked in Bridge::apply times out here.
        app.cx.simulate_resize(size(px(901.), px(701.)));
        app.cx.run_until_parked();
        app.cx.debug_bounds("refs:other").is_some()
            && app
                .cx
                .debug_bounds("status:?:untracked-marker.txt")
                .is_some()
    });
}

#[gpui::test]
fn closing_under_a_blocked_hook_waits_with_the_banner_and_asks_nothing(cx: &mut TestAppContext) {
    let f = fixture();
    let mut app = open(cx, Some(&f.repo));

    app.quit_under_a_blocked_hook(&f);
    assert_eq!(app.prompt(), None);
    // Seen red: a window that closes without the Quit waiting sets the flag in this window.
    app.assert_stays_open(&f.repo, "exited under a blocked hook");

    f.release();
    app.until("the app exits", |app| app.exited.get());
}

#[gpui::test]
fn a_second_close_under_a_blocked_hook_asks_before_force_quit(cx: &mut TestAppContext) {
    let f = fixture();
    let mut app = open(cx, Some(&f.repo));
    app.quit_under_a_blocked_hook(&f);

    app.close();
    assert_eq!(
        app.prompt().as_deref(),
        Some("Force quit while git is running?")
    );
    // Seen red: a Force quit sent before the answer exits within this window.
    app.assert_stays_open(&f.repo, "force quit without an answer");

    app.answer("Force quit");
    app.until("the app exits", |app| app.exited.get());
    assert!(!f.release.0.exists(), "the hook is still blocked");
}

#[gpui::test]
fn the_banners_force_quit_button_asks_first(cx: &mut TestAppContext) {
    let f = fixture();
    let mut app = open(cx, Some(&f.repo));
    app.quit_under_a_blocked_hook(&f);

    // The test window draws only when resized. This one also proves the banner renders.
    app.cx.simulate_resize(size(px(901.), px(701.)));
    // Seen red: without the button's click handler, no question comes up.
    let button = app
        .cx
        .debug_bounds("force-quit")
        .expect("the banner shows its button");
    app.cx.simulate_click(button.center(), Modifiers::default());
    app.cx.run_until_parked();
    assert_eq!(
        app.prompt().as_deref(),
        Some("Force quit while git is running?")
    );

    app.answer("Force quit");
    app.until("the app exits", |app| app.exited.get());
}

#[gpui::test]
fn keep_waiting_leaves_the_quit_waiting_for_the_hook(cx: &mut TestAppContext) {
    let f = fixture();
    let mut app = open(cx, Some(&f.repo));
    app.quit_under_a_blocked_hook(&f);

    app.close();
    app.answer("Keep waiting");
    assert_eq!(app.prompt(), None);
    // Seen red: taking any answer as a yes exits within this window.
    app.assert_stays_open(&f.repo, "force quit after Keep waiting");

    // The question can be asked again, and the Quit still finishes on its own.
    app.close();
    assert!(app.prompt().is_some(), "the second question wasn't asked");
    app.answer("Keep waiting");
    f.release();
    app.until("the app exits", |app| app.exited.get());
}

#[gpui::test]
fn another_close_while_a_question_is_up_asks_nothing_more(cx: &mut TestAppContext) {
    let f = fixture();
    let mut app = open(cx, Some(&f.repo));
    app.quit_under_a_blocked_hook(&f);

    app.close();
    app.close();
    app.answer("Keep waiting");
    // Seen red: without the `asking` check, a second question is left pending here.
    assert_eq!(app.prompt(), None);
    f.release();
}

#[gpui::test]
fn closing_during_a_push_asks_first_and_keep_pushing_leaves_it_running(cx: &mut TestAppContext) {
    let f = fixture();
    let mut app = open(cx, Some(&f.repo));
    app.start_blocked(Intent::Push, &f);

    app.close();
    assert_eq!(app.prompt().as_deref(), Some("A push is running"));
    app.answer("Keep pushing");
    // Seen red: a Quit sent regardless stops the push and exits within this window.
    app.assert_stays_open(&f.repo, "exited after Keep pushing");

    f.release();
    app.until("the push reaches the remote", |_| f.remote_has_the_push());
}

#[gpui::test]
fn stopping_the_push_when_asked_exits_and_nothing_reaches_the_remote(cx: &mut TestAppContext) {
    let f = fixture();
    let mut app = open(cx, Some(&f.repo));
    app.start_blocked(Intent::Push, &f);

    // Seen red: a Quit sent without asking leaves no prompt to answer.
    app.close();
    app.answer("Stop push and quit");
    app.until("the app exits", |app| app.exited.get());
    // Control: `closing_during_a_push_asks_first_and_keep_pushing_leaves_it_running`
    // reaches the remote.
    assert!(!f.remote_has_the_push());
}

#[gpui::test]
fn closing_during_a_fetch_doesnt_ask(cx: &mut TestAppContext) {
    let f = fixture();
    let mut app = open(cx, Some(&f.repo));
    app.start_blocked(Intent::Fetch, &f);

    app.close();
    // Seen red: treating every Network Job as a push asks here.
    assert_eq!(app.prompt(), None);
    app.until("the app exits", |app| app.exited.get());
}

#[gpui::test]
fn closing_after_a_push_ended_doesnt_ask(cx: &mut TestAppContext) {
    let f = fixture();
    let mut app = open(cx, Some(&f.repo));
    app.start_blocked(Intent::Push, &f);
    f.release();
    app.until("the view knows the push ended", |app| {
        app.workspace.read_with(app.cx, |workspace, cx| {
            workspace.quit.read(cx).flow.jobs.is_empty()
        })
    });

    // Seen red: a view that keeps ended Jobs never gets here.
    app.close();
    assert_eq!(app.prompt(), None);
    app.until("the app exits", |app| app.exited.get());
}
