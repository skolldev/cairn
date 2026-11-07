//! The Cairn GPUI binary.
//!
//! `cairn [repo-path]` opens a window on the repository and shows its refs. `app` speaks to
//! the Repo service only in Intents, and hears back only through Snapshots drained by the
//! bridge. `RUST_LOG` filters the `tracing` output (default `info`).

mod bridge;
#[cfg(debug_assertions)]
mod frame_overlay;
mod refs_view;

use std::path::PathBuf;
use std::sync::Arc;

use cairn_repo::{Intent, RepoService};
use gpui::{
    App, Context, Entity, IntoElement, ParentElement, PathPromptOptions, Render, Styled,
    TitlebarOptions, Window, WindowBounds, WindowOptions, div, prelude::*, px, rgb, size,
};

use bridge::Bridge;
use refs_view::RefsView;

struct Workspace {
    service: Arc<RepoService>,
    refs: Entity<RefsView>,
    _bridge: Entity<Bridge>,
    #[cfg(debug_assertions)]
    frame_clock: Option<std::rc::Rc<frame_overlay::FrameClock>>,
}

impl Workspace {
    fn new(path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // A bounded channel of one: wakes that arrive before the bridge runs coalesce.
        let (wake, woken) = async_channel::bounded(1);
        let service = Arc::new(RepoService::start(move || {
            let _ = wake.try_send(());
        }));
        let refs = cx.new(|_| RefsView::default());
        let bridge = cx.new(|cx| Bridge::new(service.clone(), woken, refs.clone(), window, cx));
        if let Some(path) = path {
            service.send(Intent::Open(path));
        }
        Self {
            service,
            refs,
            _bridge: bridge,
            #[cfg(debug_assertions)]
            frame_clock: frame_overlay::FrameClock::enabled().then(Default::default),
        }
    }

    fn choose_repo(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        let service = self.service.clone();
        cx.spawn(async move |_, _| {
            if let Ok(Ok(Some(paths))) = chosen.await
                && let Some(path) = paths.into_iter().next()
            {
                service.send(Intent::Open(path));
            }
        })
        .detach();
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
            );
        #[allow(unused_mut)]
        let mut root = div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(rgb(0x14161a))
            .text_color(rgb(0xe6edf3))
            .text_size(px(13.))
            .child(header)
            .child(div().flex_1().min_h_0().child(self.refs.clone()));
        #[cfg(debug_assertions)]
        if let Some(clock) = &self.frame_clock {
            root = root.child(clock.overlay()).child(clock.end_marker());
        }
        root
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::fmt::format::FmtSpan;
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_span_events(FmtSpan::CLOSE)
        .init();
}

fn main() {
    init_tracing();
    let path = std::env::args_os().nth(1).map(PathBuf::from);
    gpui_platform::application().run(move |cx: &mut App| {
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
            cx.new(|cx| Workspace::new(path, window, cx))
        })
        .expect("open the Cairn window");
        cx.on_window_closed(|cx, _| cx.quit()).detach();
        cx.activate(true);
    });
}
