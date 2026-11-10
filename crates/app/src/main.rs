//! The Cairn GPUI binary.
//!
//! `cairn [repo-path]` opens a window on the repository and shows its refs and status. Without
//! a path, it lists the recent repositories to choose from. `app` speaks to
//! the Repo service only in Intents, and hears back only through Snapshots drained by the
//! bridge. `RUST_LOG` filters the `tracing` output (default `info`).

mod bridge;
mod fetch;
#[cfg(debug_assertions)]
mod frame_overlay;
mod jobs;
#[cfg(feature = "measure")]
mod measure;
mod quit;
mod recent;
mod refs_view;
mod status_view;
#[cfg(target_os = "macos")]
mod terminate;
#[cfg(test)]
mod testing;

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use cairn_repo::{Intent, RepoService};
use gpui::{
    App, Context, Entity, IntoElement, ParentElement, PathPromptOptions, Render, Styled,
    TitlebarOptions, Window, WindowBounds, WindowOptions, div, prelude::*, px, rgb, size,
};

use bridge::Bridge;
use jobs::{JobsView, StopJob};
use quit::QuitView;
use recent::{OpenRecent, Place, RecentRepos, RecentView};
use refs_view::{RefsView, SwitchTo};
use status_view::StatusView;

struct Workspace {
    service: Arc<RepoService>,
    refs: Entity<RefsView>,
    status: Entity<StatusView>,
    quit: Entity<QuitView>,
    jobs: Entity<JobsView>,
    recent: Entity<RecentView>,
    /// An Open has been sent. Until then, the window shows the empty state.
    opened: bool,
    /// Holds focus when nothing inside the window does, so that the window's shortcuts
    /// still reach it: with nothing focused, GPUI dispatches keys to no element.
    focus: gpui::FocusHandle,
    _bridge: Entity<Bridge>,
    _switch_to: gpui::Subscription,
    _stop_job: gpui::Subscription,
    _open_recent: gpui::Subscription,
    #[cfg(debug_assertions)]
    frame_clock: Option<std::rc::Rc<frame_overlay::FrameClock>>,
    #[cfg(feature = "measure")]
    probe: Option<measure::Probe>,
}

impl Workspace {
    fn new(
        path: Option<PathBuf>,
        recent: RecentRepos,
        exit: Rc<dyn Fn(&mut App)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // A bounded channel of one: wakes that arrive before the bridge runs coalesce.
        let (wake, woken) = async_channel::bounded(1);
        let service = Arc::new(RepoService::start(move || {
            let _ = wake.try_send(());
        }));
        let refs = cx.new(|cx| RefsView::new(window, cx));
        let status = cx.new(|_| StatusView::default());
        let quit = cx.new(|_| QuitView::new(service.clone(), exit));
        let jobs = cx.new(|_| JobsView::default());
        let recent = cx.new(|_| RecentView::new(recent, path.clone()));
        let open_recent = cx.subscribe(&recent, |workspace, _, OpenRecent(path), cx| {
            workspace.open(path.clone(), cx);
        });
        let switch_to = cx.subscribe(&refs, |workspace, _, SwitchTo(branch), _| {
            workspace.service.send(Intent::Switch(branch.clone()));
        });
        let stop_job = cx.subscribe(&jobs, |workspace, _, StopJob(id), _| {
            workspace.service.send(Intent::Stop(*id));
        });
        // The window stays open until the Repo service says it has quit, and the bridge then
        // exits the app. Deferred, so that a question isn't asked inside the platform's callback.
        window.on_window_should_close(cx, {
            let quit = quit.clone();
            move |window, cx| {
                let quit = quit.clone();
                window.defer(cx, move |window, cx| {
                    quit.update(cx, |view, cx| view.close_requested(window, cx));
                });
                false
            }
        });
        let bridge = cx.new(|cx| {
            Bridge::new(
                service.clone(),
                woken,
                bridge::Views {
                    refs: refs.clone(),
                    status: status.clone(),
                    quit: quit.clone(),
                    jobs: jobs.clone(),
                    recent: recent.clone(),
                },
                window,
                cx,
            )
        });
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let opened = path.is_some();
        if let Some(path) = path {
            service.send(Intent::Open(path));
        }
        #[cfg(feature = "measure")]
        measure::mark(cairn_budget::probe::Mark::Service);
        Self {
            service,
            refs,
            status,
            quit,
            jobs,
            recent,
            opened,
            focus,
            _bridge: bridge,
            _switch_to: switch_to,
            _stop_job: stop_job,
            _open_recent: open_recent,
            #[cfg(debug_assertions)]
            frame_clock: frame_overlay::FrameClock::enabled().then(Default::default),
            #[cfg(feature = "measure")]
            probe: None,
        }
    }

    fn choose_repo(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = chosen.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update(cx, |workspace, cx| workspace.open(path, cx));
            }
        })
        .detach();
    }

    /// Fetches every remote, while a repository is open and no fetch is queued or running.
    fn fetch(&mut self, cx: &mut Context<Self>) {
        if !self.opened || self.jobs.read(cx).fetching() {
            return;
        }
        self.jobs.update(cx, |jobs, _| jobs.fetch_asked());
        self.service.send(Intent::Fetch);
    }

    /// Opens the repository at `path`. It joins the recent list only once it has opened.
    fn open(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.opened = true;
        self.recent
            .update(cx, |recent, _| recent.opening(path.clone()));
        self.service.send(Intent::Open(path));
        cx.notify();
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(debug_assertions)]
        if let Some(clock) = &self.frame_clock {
            clock.begin();
        }
        let header = div()
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(rgb(0x2a2f37))
            .child(
                div()
                    .id("open")
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(rgb(0x2a2f37))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.choose_repo(cx)))
                    .child("Open repository…"),
            )
            .child(
                div()
                    .id("open-recent")
                    .debug_selector(|| "open-recent".to_owned())
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(rgb(0x2a2f37))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.recent.update(cx, |recent, cx| recent.toggle_menu(cx))
                    }))
                    .child(if self.recent.read(cx).menu_open() {
                        "Open Recent ▴"
                    } else {
                        "Open Recent ▾"
                    }),
            )
            .child(fetch::button(
                self.opened,
                cx.listener(|this, _, _, cx| this.fetch(cx)),
            ));
        let content = if self.opened {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(div().flex_1().min_w_0().child(self.refs.clone()))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .border_l_1()
                        .border_color(rgb(0x2a2f37))
                        .child(self.status.clone()),
                )
        } else {
            // The empty state: nothing open yet, so the recent repositories are the way in.
            let list = self.recent.update(cx, |recent, cx| {
                recent.list(Place::Empty, cx).into_any_element()
            });
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .gap_2()
                .p_3()
                .child(
                    div()
                        .text_color(rgb(0x7d8590))
                        .child("No repository open. Pass a path on the command line, use Open, or choose a recent one."),
                )
                .child(list)
        };
        #[allow(unused_mut)]
        let mut root = div()
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &fetch::Fetch, _, cx| this.fetch(cx)))
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(rgb(0x14161a))
            .text_color(rgb(0xe6edf3))
            .text_size(px(13.))
            .child(header)
            .child(self.recent.clone())
            .child(self.quit.clone())
            .child(self.jobs.clone())
            .child(content);
        #[cfg(debug_assertions)]
        if let Some(clock) = &self.frame_clock {
            root = root.child(clock.overlay()).child(clock.end_marker());
        }
        #[cfg(feature = "measure")]
        if let Some(probe) = &self.probe {
            // Last, so that it paints after everything the frame shows.
            root = probe.attach(root, self.refs.clone(), self.status.clone(), _window, cx);
        }
        root
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::fmt::format::FmtSpan;
    #[cfg(feature = "measure")]
    if let Some(first_publishes) = measure::first_publishes() {
        use tracing_subscriber::prelude::*;
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_span_events(FmtSpan::CLOSE)
                    .with_filter(
                        EnvFilter::try_from_default_env()
                            .unwrap_or_else(|_| EnvFilter::new("info")),
                    ),
            )
            .with(first_publishes)
            .init();
        return;
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_span_events(FmtSpan::CLOSE)
        .init();
}

/// Ends the app. By now nothing is left to ask about: the Repo service has quit, or the window
/// is gone and there is nowhere to ask.
fn exit(cx: &mut App) {
    #[cfg(target_os = "macos")]
    terminate::allow_exit();
    cx.quit();
}

fn main() {
    #[cfg(feature = "measure")]
    measure::mark(cairn_budget::probe::Mark::Main);
    init_tracing();
    #[cfg(feature = "measure")]
    measure::install_frame_source();
    let path = std::env::args_os().nth(1).map(PathBuf::from);
    gpui_platform::application().run(move |cx: &mut App| {
        #[cfg(feature = "measure")]
        measure::mark(cairn_budget::probe::Mark::Launched);
        gpui_component::init(cx);
        fetch::bind_keys(cx);
        gpui_component::Theme::change(gpui_component::ThemeMode::Dark, None, cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(900.), px(700.)), cx)),
            titlebar: Some(TitlebarOptions {
                title: Some("Cairn".into()),
                ..Default::default()
            }),
            focus: true,
            ..Default::default()
        };
        cx.open_window(options, move |window, cx| {
            #[cfg(feature = "measure")]
            measure::mark(cairn_budget::probe::Mark::Window);
            let workspace = cx.new(|cx| {
                #[allow(unused_mut)]
                let mut workspace = Workspace::new(
                    path,
                    RecentRepos::load(RecentRepos::default_file()),
                    Rc::new(exit),
                    window,
                    cx,
                );
                #[cfg(feature = "measure")]
                workspace.probe_from_env(window, cx);
                workspace
            });
            #[cfg(target_os = "macos")]
            terminate::route_to_close_flow(workspace.read(cx).quit.clone(), window, cx);
            workspace
        })
        .expect("open the Cairn window");
        cx.on_window_closed(|cx, _| exit(cx)).detach();
        cx.activate(true);
    });
}
