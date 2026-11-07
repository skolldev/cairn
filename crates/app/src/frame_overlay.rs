//! A debug-build frame-time overlay, for seeing frame cost while dogfooding.
//!
//! It times each frame the window actually draws, from the root view's render to the end of
//! its paint, and the interval since the previous drawn frame. It never forces frames, so an
//! idle window shows the last frame it drew. These are CPU timings: presented-frame Budgets
//! are measured with PresentMon and Instruments, never with this.
//!
//! Set `CAIRN_FRAME_OVERLAY=0` to hide it.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{IntoElement, ParentElement, Styled, canvas, div, px, rgb};

/// How long the worst frame stays on the overlay.
const WORST_WINDOW: Duration = Duration::from_secs(1);

#[derive(Default)]
pub struct FrameClock {
    render_start: Cell<Option<Instant>>,
    last_end: Cell<Option<Instant>>,
    cost: Cell<Duration>,
    interval: Cell<Duration>,
    worst: Cell<(Duration, Option<Instant>)>,
    frames: Cell<u64>,
}

impl FrameClock {
    pub fn enabled() -> bool {
        std::env::var_os("CAIRN_FRAME_OVERLAY").is_none_or(|v| v != "0")
    }

    /// Call first thing in the root view's render.
    pub fn begin(&self) {
        self.render_start.set(Some(Instant::now()));
    }

    /// The last child of the root view: it paints last and closes the frame's timing.
    pub fn end_marker(self: &Rc<Self>) -> impl IntoElement {
        let clock = self.clone();
        canvas(|_, _, _| {}, move |_, _, _, _| clock.end())
            .absolute()
            .size_0()
    }

    fn end(&self) {
        let now = Instant::now();
        if let Some(start) = self.render_start.take() {
            let cost = now - start;
            self.cost.set(cost);
            let (worst, at) = self.worst.get();
            if cost >= worst || at.is_none_or(|at| now - at > WORST_WINDOW) {
                self.worst.set((cost, Some(now)));
            }
        }
        if let Some(last) = self.last_end.replace(Some(now)) {
            self.interval.set(now - last);
        }
        self.frames.set(self.frames.get() + 1);
    }

    /// The overlay, showing the previous drawn frame.
    pub fn overlay(&self) -> impl IntoElement {
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        div()
            .absolute()
            .top_2()
            .right_2()
            .px_2()
            .py_1()
            .bg(rgb(0x000000))
            .text_color(rgb(0x8fe388))
            .text_size(px(11.))
            .child(format!(
                "frame {:.1} ms · worst 1s {:.1} ms · interval {:.1} ms · #{}",
                ms(self.cost.get()),
                ms(self.worst.get().0),
                ms(self.interval.get()),
                self.frames.get(),
            ))
    }
}
