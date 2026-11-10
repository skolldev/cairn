//! What the window tests share: hermetic git, and the real window with the real Repo service
//! behind it. The test platform's `quit` does nothing, so the exit hook sets a flag instead.

use std::cell::Cell;
use std::path::Path;
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

/// The real window, whose exit hook sets a flag instead of ending the test process.
pub struct App<'a> {
    pub cx: &'a mut VisualTestContext,
    pub workspace: Entity<Workspace>,
    pub exited: Rc<Cell<bool>>,
}

pub fn open<'a>(cx: &'a mut TestAppContext, repo: Option<&Path>) -> App<'a> {
    // Real git runs on the service's threads, so the executor must wait for them.
    cx.executor().allow_parking();
    cx.update(gpui_component::init);
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
