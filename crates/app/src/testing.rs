//! What the window tests share: hermetic git, and the real window with the real Repo service
//! behind it. The test platform's `quit` does nothing, so the exit hook sets a flag instead.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use cairn_repo::Intent;
use cairn_write::{Git, Invocation, JobClass};
use gpui::{
    Bounds, Entity, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, Pixels, TestAppContext,
    VisualTestContext,
};

use crate::Workspace;
use crate::recent::RecentRepos;

/// Points git at an empty home, once per test binary, so a developer's config can't change
/// the result. Runs before anything spawns git.
pub fn hermetic() {
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

pub fn git(repo: &Path, args: &[&str]) -> String {
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
pub fn slash(path: &Path) -> String {
    path.to_str().unwrap().replace('\\', "/")
}

/// Creates the release file when dropped, so a failing test leaves nothing blocked on it.
pub struct Release(pub PathBuf);

impl Release {
    pub fn release(&self) {
        std::fs::write(&self.0, "").unwrap();
    }
}

impl Drop for Release {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.0, "");
    }
}

/// A line of sh that blocks until `release` exists or `dir` is gone. A Force quit can leave
/// a hook running after its test has ended, and the temp dir going away ends it.
pub fn block_until(release: &Path, dir: &Path) -> String {
    format!(
        "while [ ! -f '{release}' ] && [ -d '{dir}' ]; do sleep 0.05; done\n",
        release = slash(release),
        dir = slash(dir),
    )
}

/// Installs `body` as the repository's `post-checkout` hook, a real one that git runs.
pub fn post_checkout_hook(repo: &Path, body: &str) {
    let hook = repo.join(".git/hooks/post-checkout");
    std::fs::write(&hook, format!("#!/bin/sh\n{body}")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// The real window, whose exit hook sets a flag instead of ending the test process.
pub struct App<'a> {
    pub cx: &'a mut VisualTestContext,
    pub workspace: Entity<Workspace>,
    pub exited: Rc<Cell<bool>>,
}

/// The window, opening `repo` if given, with a recent list that is never saved.
pub fn open<'a>(cx: &'a mut TestAppContext, repo: Option<&Path>) -> App<'a> {
    open_with_recent(cx, repo, None)
}

/// The window, keeping its recent list in `recent`, as a launch reads it from app data.
pub fn open_with_recent<'a>(
    cx: &'a mut TestAppContext,
    repo: Option<&Path>,
    recent: Option<PathBuf>,
) -> App<'a> {
    // Real git runs on the service's threads, so the executor must wait for them.
    cx.executor().allow_parking();
    cx.update(gpui_component::init);
    let exited = Rc::new(Cell::new(false));
    let exit = exited.clone();
    let path = repo.map(Path::to_path_buf);
    let (workspace, cx) = cx.add_window_view(move |window, cx| {
        Workspace::new(
            path,
            RecentRepos::load(recent),
            Rc::new(move |_| exit.set(true)),
            window,
            cx,
        )
    });
    App {
        cx,
        workspace,
        exited,
    }
}

impl App<'_> {
    /// Runs the app until `done` holds. Snapshots arrive from the service's threads.
    pub fn until(&mut self, what: &str, mut done: impl FnMut(&mut Self) -> bool) {
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

    pub fn send(&mut self, intent: Intent) {
        self.workspace
            .read_with(self.cx, |workspace, _| workspace.service.send(intent));
    }

    /// The recent repositories, as the window lists them.
    pub fn recent(&mut self) -> Vec<PathBuf> {
        self.workspace.read_with(self.cx, |workspace, cx| {
            workspace.recent.read(cx).entries().to_vec()
        })
    }

    /// Draws a frame. The test window draws only when asked, and only a drawn element has
    /// bounds or takes input.
    pub fn draw(&mut self) {
        self.cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        self.cx.run_until_parked();
    }

    /// The bounds of the element drawn with `selector` in the last frame.
    pub fn bounds(&mut self, selector: &str) -> Option<Bounds<Pixels>> {
        // `debug_bounds` takes only a `&'static str`. A test leaks a few short strings.
        self.cx
            .debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
    }

    pub fn shown(&mut self, selector: &str) -> bool {
        self.bounds(selector).is_some()
    }

    pub fn click(&mut self, selector: &str) {
        let bounds = self
            .bounds(selector)
            .unwrap_or_else(|| panic!("{selector} isn't drawn"));
        self.cx
            .simulate_click(bounds.center(), Modifiers::default());
        self.cx.run_until_parked();
        self.draw();
    }

    /// Two clicks, as the platform reports a double-click: the second carries a count of 2.
    pub fn double_click(&mut self, selector: &str) {
        let position = self
            .bounds(selector)
            .unwrap_or_else(|| panic!("{selector} isn't drawn"))
            .center();
        for click_count in [1, 2] {
            self.cx.simulate_event(MouseDownEvent {
                position,
                modifiers: Modifiers::default(),
                button: MouseButton::Left,
                click_count,
                first_mouse: false,
            });
            self.cx.simulate_event(MouseUpEvent {
                position,
                modifiers: Modifiers::default(),
                button: MouseButton::Left,
                click_count,
            });
        }
        self.cx.run_until_parked();
        self.draw();
    }
}
