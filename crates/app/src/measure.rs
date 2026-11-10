//! The Budget harness's probe, in a build with the `measure` feature and only when
//! `CAIRN_MEASURE` is set. It tells the harness what each frame painted and when, on stdout, and
//! takes commands on stdin. The harness decides what a sample's end looks like, and takes the
//! frame's time from the OS (`cairn_budget::presented`). See `docs/budget-harness.md`.

use std::rc::Rc;

use cairn_budget::clock;
use cairn_budget::probe::{ANIMATE, ENV, Frame, Line};
use cairn_repo::Intent;
use gpui::{
    App, Context, Div, Entity, FocusHandle, KeyDownEvent, ParentElement, Styled, Window, canvas,
    prelude::*,
};

use crate::Workspace;
use crate::refs_view::RefsView;
use crate::status_view::StatusView;

#[cfg(target_os = "macos")]
mod metal;
#[cfg(test)]
mod tests;

/// Writes one probe line. Metal reports from its own thread, so stdout is what they share.
fn write_line(line: Line) {
    use std::io::Write as _;
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// Hooks Metal so that it reports presented frames. Before anything draws, so that the first
/// frame counts. Does nothing where PresentMon sees frames from outside.
pub fn install_frame_source() {
    #[cfg(target_os = "macos")]
    if std::env::var_os(ENV).is_some() {
        metal::install();
    }
}

/// What the harness can ask of the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// A Local write Job, started where a double-click on the branch's row starts it.
    Switch(String),
    Quit,
}

impl Command {
    fn parse(line: &str) -> Option<Self> {
        match line.split_once(' ') {
            Some(("switch", branch)) => Some(Command::Switch(branch.to_owned())),
            None if line == "quit" => Some(Command::Quit),
            _ => None,
        }
    }
}

/// The probe in one window.
pub struct Probe {
    emit: Rc<dyn Fn(Line)>,
    /// Key-downs reach a window only through a focused element.
    focus: FocusHandle,
    /// Keep drawing: Metal reports no presented time in an idle window, and a freeze shows only
    /// as a gap in frames that should have kept coming.
    animating: bool,
}

impl Probe {
    pub fn new(
        emit: Rc<dyn Fn(Line)>,
        animating: bool,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Self {
        emit(Line::Hello {
            release: !cfg!(debug_assertions),
            pid: std::process::id(),
        });
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self {
            emit,
            focus,
            animating,
        }
    }

    /// Adds what the probe needs to a frame's root element.
    pub fn attach(
        &self,
        root: Div,
        refs: Entity<RefsView>,
        status: Entity<StatusView>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Div {
        if self.animating {
            window.request_animation_frame();
        }
        let emit = self.emit.clone();
        root.track_focus(&self.focus)
            .on_key_down(cx.listener(Workspace::measure_key))
            .child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window: &mut Window, cx: &mut App| {
                        let mut frame = Frame {
                            paint_ns: clock::now_ns(),
                            active: window.is_window_active(),
                            ..Frame::default()
                        };
                        refs.read(cx).report_to(&mut frame);
                        status.read(cx).report_to(&mut frame);
                        emit(Line::Frame(frame));
                    },
                )
                .absolute()
                .size_0(),
            )
    }
}

impl Workspace {
    pub fn set_probe(&mut self, probe: Probe) {
        self.probe = Some(probe);
    }

    /// Turns the probe on if the harness asked for it, and listens for its commands. When the
    /// harness goes away, stdin ends, and Cairn quits rather than outliving it.
    pub fn probe_from_env(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mode) = std::env::var_os(ENV) else {
            return;
        };
        self.set_probe(Probe::new(Rc::new(write_line), mode == ANIMATE, window, cx));
        let (send, commands) = async_channel::unbounded();
        std::thread::spawn(move || {
            for line in std::io::stdin().lines().map_while(Result::ok) {
                if let Some(command) = Command::parse(&line) {
                    let _ = send.send_blocking(command);
                }
            }
            let _ = send.send_blocking(Command::Quit);
        });
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(command) = commands.recv().await {
                if this
                    .update(cx, |workspace, cx| workspace.measure(command, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    pub fn measure(&mut self, command: Command, cx: &mut Context<Self>) {
        let Some(probe) = &self.probe else { return };
        match command {
            Command::Switch(branch) => {
                (probe.emit)(Line::Intent {
                    ns: clock::now_ns(),
                    branch: branch.clone(),
                });
                self.refs.update(cx, |refs, cx| refs.switch_to(branch, cx));
            }
            Command::Quit => self.service.send(Intent::Quit),
        }
    }

    fn measure_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(probe) = &self.probe {
            (probe.emit)(Line::Key {
                ns: clock::now_ns(),
                held: event.is_held,
            });
            cx.notify();
        }
    }
}
