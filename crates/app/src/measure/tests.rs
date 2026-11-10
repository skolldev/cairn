//! The real window, Repo service and git, with the probe's lines read from a sink: what a frame
//! says it painted has to be what the views hold, because the harness times what it says.

use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cairn_budget::fixture::{BRANCH_A, BRANCH_B, KEEP, KEEP_DIR, SAVED, Sandbox};
use cairn_budget::probe::Line;
use gpui::{Entity, TestAppContext, VisualTestContext, px, size};

use super::{Command, Probe};
use crate::Workspace;
use crate::testing::hermetic;

/// A repository with one commit, standing in as the fixture a sandbox is cloned from.
fn sandbox() -> (tempfile::TempDir, Sandbox) {
    hermetic();
    let dir = tempfile::tempdir().unwrap();
    let git = cairn_write::Git::shared().unwrap();
    let run = |args: &[&str]| {
        let invocation =
            cairn_write::Invocation::new(cairn_write::JobClass::LocalWrite, dir.path())
                .args([
                    "-c",
                    "user.name=Cairn Test",
                    "-c",
                    "user.email=test@cairn.invalid",
                ])
                .args(args.iter().copied());
        let out = git.run(&invocation).unwrap();
        assert!(out.status.success(), "{args:?}: {}", out.stderr_text());
    };
    run(&["init", "--quiet"]);
    run(&["commit", "--quiet", "--allow-empty", "-m", "base"]);
    let sandbox = Sandbox::clone_of(dir.path()).unwrap();
    (dir, sandbox)
}

struct App<'a> {
    cx: &'a mut VisualTestContext,
    workspace: Entity<Workspace>,
    lines: Arc<Mutex<Vec<Line>>>,
}

fn open<'a>(cx: &'a mut TestAppContext, repo: &Path) -> App<'a> {
    cx.executor().allow_parking();
    cx.update(gpui_component::init);
    let lines: Arc<Mutex<Vec<Line>>> = Arc::default();
    let sink = lines.clone();
    let path = repo.to_path_buf();
    let (workspace, cx) = cx.add_window_view(move |window, cx| {
        let mut workspace = Workspace::new(
            Some(path),
            crate::recent::RecentRepos::load(None),
            Rc::new(|_| {}),
            window,
            cx,
        );
        workspace.set_probe(Probe::new(
            Rc::new(move |line| sink.lock().unwrap().push(line)),
            false,
            window,
            cx,
        ));
        workspace
    });
    App {
        cx,
        workspace,
        lines,
    }
}

impl App<'_> {
    /// Drains the Bridge and draws until a frame line passes `done`. The test window draws only
    /// when resized, and the Bridge drains on its timer, so this advances both.
    fn wait_for_frame(
        &mut self,
        mut done: impl FnMut(&cairn_budget::probe::Frame) -> bool,
    ) -> cairn_budget::probe::Frame {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut width = 900.;
        loop {
            self.cx.executor().advance_clock(Duration::from_millis(150));
            width = if width == 900. { 901. } else { 900. };
            self.cx.simulate_resize(size(px(width), px(700.)));
            self.cx.run_until_parked();
            let found = self
                .lines
                .lock()
                .unwrap()
                .iter()
                .rev()
                .find_map(|line| match line {
                    Line::Frame(frame) if done(frame) => Some(frame.clone()),
                    _ => None,
                });
            if let Some(frame) = found {
                return frame;
            }
            assert!(
                Instant::now() < deadline,
                "no such frame: {:?}",
                self.lines.lock().unwrap().last()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn command(&mut self, command: Command) {
        self.workspace
            .update(self.cx, |workspace, cx| workspace.measure(command, cx));
    }
}

#[gpui::test]
async fn a_frame_reports_what_the_views_hold_after_an_external_change(cx: &mut TestAppContext) {
    let (_dir, sandbox) = sandbox();
    let mut app = open(cx, sandbox.repo());

    // Every Slot, because each answers the Open on its own schedule.
    let opened = app.wait_for_frame(|f| {
        f.head.as_deref() == Some(BRANCH_A)
            && f.untracked_paths.iter().any(|p| p == KEEP_DIR)
            && f.refs > 0
            && f.staged > 0
            && f.unstaged > 0
            && f.untracked > 0
    });
    assert!(opened.unstaged_paths.is_empty());

    sandbox.save(true).unwrap();
    let saved = app.wait_for_frame(|f| f.unstaged_paths.iter().any(|p| p == SAVED));
    assert!(saved.unstaged > opened.unstaged, "a newer answer");
    assert!(saved.paint_ns > opened.paint_ns, "stamped when it painted");
}

#[gpui::test]
async fn a_frame_reports_whether_its_window_is_active(cx: &mut TestAppContext) {
    let (_dir, sandbox) = sandbox();
    let mut app = open(cx, sandbox.repo());
    assert!(!app.wait_for_frame(|f| f.head.is_some()).active);

    app.cx.update(|window, _| window.activate_window());
    app.wait_for_frame(|f| f.active);
}

#[gpui::test]
async fn a_switch_intent_is_stamped_and_ends_with_the_flip_on_screen(cx: &mut TestAppContext) {
    let (_dir, sandbox) = sandbox();
    let mut app = open(cx, sandbox.repo());
    let opened = app.wait_for_frame(|f| f.head.as_deref() == Some(BRANCH_A));

    app.command(Command::Switch(BRANCH_B.to_owned()));
    let intent = app
        .lines
        .lock()
        .unwrap()
        .iter()
        .find_map(|line| match line {
            Line::Intent { ns, branch } => Some((*ns, branch.clone())),
            _ => None,
        });
    let (intent_ns, branch) = intent.expect("the Intent is stamped as it is sent");
    assert_eq!(branch, BRANCH_B);
    assert!(intent_ns > opened.paint_ns);

    let switched = app.wait_for_frame(|f| {
        f.head.as_deref() == Some(BRANCH_B) && f.untracked_paths.iter().any(|p| p == KEEP)
    });
    assert!(switched.paint_ns > intent_ns);
}

#[gpui::test]
async fn a_key_held_down_is_reported_with_the_os_repeat_flag(cx: &mut TestAppContext) {
    let (_dir, sandbox) = sandbox();
    let mut app = open(cx, sandbox.repo());
    app.wait_for_frame(|f| f.head.is_some());

    app.cx.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse("down").unwrap(),
        is_held: false,
        prefer_character_input: false,
    });
    app.cx.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse("down").unwrap(),
        is_held: true,
        prefer_character_input: false,
    });
    app.cx.run_until_parked();
    let keys: Vec<bool> = app
        .lines
        .lock()
        .unwrap()
        .iter()
        .filter_map(|line| match line {
            Line::Key { held, .. } => Some(*held),
            _ => None,
        })
        .collect();
    assert_eq!(keys, [false, true]);
}
