//! PROTOTYPE, throwaway. Issue #9: can GPUI scroll 1.5M rows of painted graph
//! lanes at the Floors? See README.md for commands and how to read the output.

mod data;
mod sys;

use data::{Format, Store, Visible};
use gpui::{
    App, Bounds, Context, Entity, FocusHandle, MouseMoveEvent, PathBuilder, PlatformInput, Render,
    ScrollDelta, ScrollEvent, SharedString, TouchPhase, UniformListScrollHandle, Window,
    WindowBounds, WindowOptions, canvas, div, fill, point, prelude::*, px, rgb, size, uniform_list,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

const ROW: f32 = 24.0;
const LANE: f32 = 12.0;
const FIRST_CHUNK: usize = 5_000;
const LEAD_IN: f64 = 0.5;
const PALETTE: [u32; 8] =
    [0x5ec2ff, 0xff7a90, 0x8fe388, 0xffc857, 0xc792ea, 0x4fd6be, 0xff9e64, 0xa3b8ff];

#[derive(Clone)]
struct Cfg {
    mode: String,
    rows: usize,
    cap: usize,
    format: Format,
    per_edge: bool,
    runs: usize,
    seconds: f64,
    only: Option<String>,
    settle: f64,
    window: Option<(f32, f32)>,
}

#[derive(Clone, Copy, PartialEq)]
enum Motion {
    Wheel,
    Flick,
    Still,
}

#[derive(Clone, Copy)]
struct Scenario {
    name: &'static str,
    row: usize,
    motion: Motion,
    up: bool,
    held: bool,
    floor: bool,
}

fn scenarios(rows: usize) -> Vec<Scenario> {
    let s = |name, row, motion, up, held, floor| Scenario { name, row, motion, up, held, floor };
    vec![
        s("control", 0, Motion::Still, false, false, false),
        s("top-wheel", 0, Motion::Wheel, false, false, true),
        s("top-flick", 0, Motion::Flick, false, false, true),
        s("r100k-wheel", 100_000, Motion::Wheel, true, false, true),
        s("r100k-flick", 100_000, Motion::Flick, true, false, true),
        s("top-held", 0, Motion::Wheel, false, true, true),
        s("r750k-wheel", 750_000, Motion::Wheel, true, false, false),
        s("end-wheel", rows.saturating_sub(100), Motion::Wheel, true, false, false),
        s("end-flick", rows.saturating_sub(100), Motion::Flick, true, false, false),
    ]
}

struct Outcome {
    scenario: Scenario,
    run: usize,
    frames: usize,
    within: f64,
    p99: f64,
    median: f64,
    mean: f64,
    worst: f64,
    dropped: u64,
    keys: u64,
    notifies: u64,
    rows_moved: f64,
    paint_avg_us: f64,
    paint_max_us: u64,
    source: String,
    invalid: bool,
}

struct Bench {
    queue: Vec<(Scenario, usize)>,
    current: Option<(Scenario, usize)>,
    phase_started: Instant,
    recording: bool,
    last_tick: Instant,
    start_offset: f64,
    keys_at: u64,
    notifies_at: u64,
    key_thread: Option<Arc<std::sync::atomic::AtomicBool>>,
    results: Vec<Outcome>,
    settling_since: Option<Instant>,
    inactive_since: Option<Instant>,
    invalid: bool,
}

struct View {
    cfg: Cfg,
    store: Arc<Store>,
    full: bool,
    handle: UniformListScrollHandle,
    focus: FocusHandle,
    bench: Option<Bench>,
    control_phase: u32,
    keys: u64,
    notifies: u64,
    bg_notify: bool,
    paint_total: Rc<Cell<(u64, u64, u64)>>,
    visible: Rc<RefCell<Visible>>,
    hud: SharedString,
    hud_at: Instant,
    launch_probe: bool,
    drew: bool,
    control_spin: u64,
}

fn offset(handle: &UniformListScrollHandle) -> f32 {
    f32::from(handle.0.borrow().base_handle.offset().y)
}

fn row_el(ix: usize) -> impl IntoElement {
    const WORDS: [&str; 12] = [
        "mm:", "net:", "fix", "refactor", "drivers/gpu", "sched:", "xfs:", "remove stale",
        "handle overflow in", "bpf:", "locking", "arm64:",
    ];
    let h = (ix as u64).wrapping_mul(0x9E3779B97F4A7C15);
    let subject: String = (0..5).map(|i| WORDS[(h >> (i * 7)) as usize % 12]).collect::<Vec<_>>().join(" ");
    div()
        .h(px(ROW))
        .w_full()
        .flex()
        .items_center()
        .gap(px(14.))
        .child(div().w(px(70.)).text_color(rgb(0x7d8590)).child(format!("{:08x}", h >> 32)))
        .child(div().flex_1().overflow_hidden().whitespace_nowrap().child(format!("{subject} #{ix}")))
        .child(div().w(px(150.)).text_color(rgb(0x7d8590)).child(format!("Author {}", h % 4096)))
}

impl Render for View {
    fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let root = div()
            .id("root")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, _, _, cx| {
                this.keys += 1;
                cx.notify();
            }))
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(rgb(0x14161a))
            .text_color(rgb(0xe6edf3))
            .text_size(px(13.));
        if self.cfg.mode == "empty" {
            return root;
        }
        if self.control_phase > 0 {
            // Control: nothing to paint but a changing colour. If this can't
            // show clean 120 Hz intervals, nothing else here can be believed.
            let c = 0x202020 + (self.control_phase % 64) * 0x010101;
            let spin = self.control_spin;
            let body = canvas(
                move |_, _, _| {},
                move |bounds, _, window, _| {
                    let t = Instant::now();
                    while (t.elapsed().as_micros() as u64) < spin {}
                    window.paint_quad(fill(bounds, rgb(c)));
                },
            );
            return root.child(body.size_full());
        }
        let graph_w = self.cfg.cap as f32 * LANE + 12.;
        let store = self.store.clone();
        let list = uniform_list("rows", store.len(), move |range, _, _| {
            range.map(row_el).map(|r| r.into_any_element()).collect::<Vec<_>>()
        })
        .track_scroll(&self.handle)
        .size_full();
        let store = self.store.clone();
        let handle = self.handle.clone();
        let visible = self.visible.clone();
        let per_edge = self.cfg.per_edge;
        let total = self.paint_total.clone();
        let graph = canvas(
            |_, _, _| {},
            move |bounds: Bounds<gpui::Pixels>, _, window, _| {
                let t = Instant::now();
                paint_graph(bounds, &store, offset(&handle), &visible, per_edge, window);
                let us = t.elapsed().as_micros() as u64;
                let (n, sum, max) = total.get();
                total.set((n + 1, sum + us, max.max(us)));
            },
        )
        .absolute()
        .left_0()
        .top_0()
        .h_full()
        .w(px(graph_w));
        root.child(div().size_full().pl(px(graph_w)).child(list))
            .child(graph)
            .child(
                div()
                    .absolute()
                    .right_2()
                    .top_2()
                    .p_2()
                    .bg(rgb(0x000000))
                    .text_color(rgb(0x8fe388))
                    .child(self.hud.clone()),
            )
    }
}

fn paint_graph(
    bounds: Bounds<gpui::Pixels>,
    store: &Store,
    scroll: f32,
    visible: &RefCell<Visible>,
    per_edge: bool,
    window: &mut Window,
) {
    let top = -(scroll as f64);
    let first = (top / ROW as f64).floor() as usize;
    let frac = (top - first as f64 * ROW as f64) as f32;
    let count = (f32::from(bounds.size.height) / ROW).ceil() as usize + 2;
    let mut v = visible.borrow_mut();
    store.read(first..first + count, &mut v);
    let x0 = f32::from(bounds.origin.x) + 6.;
    let y0 = f32::from(bounds.origin.y) - frac;
    let at = |row: u32, col: u16| {
        let i = (row as usize - first) as f32;
        point(px(x0 + (col as f32 + 0.5) * LANE), px(y0 + (i + 0.5) * ROW))
    };
    let add = |b: &mut PathBuilder, row: u32, e: &data::Edge| {
        let (a, z) = (at(row, e.top), at(row + 1, e.bottom));
        b.move_to(a);
        if e.top == e.bottom {
            b.line_to(z);
        } else {
            let mid = (a.y + z.y) / 2.;
            b.cubic_bezier_to(z, point(a.x, mid), point(z.x, mid));
        }
    };
    // All edges go first and stay consecutive, so they share one MSAA batch.
    if per_edge {
        for (row, e) in &v.edges {
            let mut b = PathBuilder::stroke(px(1.5));
            add(&mut b, *row, e);
            if let Ok(p) = b.build() {
                window.paint_path(p, rgb(PALETTE[e.colour as usize % 8]));
            }
        }
    } else {
        let mut by_colour: [Option<PathBuilder>; 8] = Default::default();
        for (row, e) in &v.edges {
            let b = by_colour[e.colour as usize % 8].get_or_insert_with(|| PathBuilder::stroke(px(1.5)));
            add(b, *row, e);
        }
        for (i, b) in by_colour.into_iter().enumerate() {
            if let Some(Ok(p)) = b.map(|b| b.build()) {
                window.paint_path(p, rgb(PALETTE[i]));
            }
        }
    }
    for (row, col, colour) in &v.nodes {
        let c = at(*row, *col);
        let r = px(4.);
        let b = Bounds::new(point(c.x - r, c.y - r), size(r * 2., r * 2.));
        window.paint_quad(fill(b, rgb(PALETTE[*colour as usize % 8])).corner_radii(r));
    }
}

impl View {
    fn jump(&mut self, row: usize) {
        let y = -(row as f64 * ROW as f64) as f32;
        self.handle.0.borrow().base_handle.set_offset(point(px(0.), px(y)));
    }

    fn refresh_hud(&mut self, cx: &mut Context<Self>) {
        if self.hud_at.elapsed().as_secs_f64() < 1.0 {
            return;
        }
        self.hud_at = Instant::now();
        let off = offset(&self.handle);
        let ulp = f32::from_bits(off.abs().to_bits() + 1) - off.abs();
        let what = self.bench.as_ref().and_then(|b| b.current).map_or("idle".into(), |(s, r)| format!("{} #{r}", s.name));
        self.hud = format!(
            "{what}  row {}  ulp {ulp}px  {} MB  {:?}/{}",
            (-off / ROW) as i64,
            sys::memory().0 / 1_000_000,
            self.cfg.format,
            self.cfg.cap
        )
        .into();
        cx.notify();
    }

    /// One bench step per frame. Returns a scroll delta for this frame, if any.
    fn step(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<f32> {
        self.refresh_hud(cx);
        let cfg_seconds = self.cfg.seconds;
        let settle = self.cfg.settle;
        if !self.full {
            return None;
        }
        let mut bench = self.bench.take()?;
        let active = window.is_window_active();
        if !active {
            // GPUI throttles inactive windows, and macOS stops the display
            // link for a locked or covered one. Neither is a GPUI result.
            bench.invalid |= bench.recording;
            if bench.current.is_none() {
                let since = *bench.inactive_since.get_or_insert_with(Instant::now);
                if since.elapsed().as_secs() >= 30 {
                    eprintln!("ABORT: window never became active. Unlock the screen and keep this window in front.");
                    std::process::exit(2);
                }
                self.bench = Some(bench);
                return None;
            }
        }
        let delta = (|| {
            let now = Instant::now();
            let Some((sc, run)) = bench.current else {
                if let Some(next) = bench.queue.pop() {
                    self.start(next.0, next.1, &mut bench, window);
                    return None;
                }
                let since = *bench.settling_since.get_or_insert(now);
                if since.elapsed().as_secs_f64() >= settle {
                    finish(&self.cfg, &self.store, &bench.results);
                    cx.quit();
                }
                return None;
            };
            let dt = now.duration_since(bench.last_tick).as_secs_f64();
            bench.last_tick = now;
            let t = bench.phase_started.elapsed().as_secs_f64();
            if !bench.recording && t >= LEAD_IN {
                bench.recording = true;
                bench.phase_started = now;
                bench.start_offset = offset(&self.handle) as f64;
                bench.keys_at = self.keys;
                bench.notifies_at = self.notifies;
                self.paint_total.set((0, 0, 0));
                bench.invalid = false;
                sys::begin_recording(cfg_seconds);
                return self.velocity(&sc, 0.0).map(|v| (v * dt) as f32);
            }
            if bench.recording && t >= cfg_seconds {
                let rec = sys::end_recording();
                let out = self.outcome(sc, run, rec, &bench);
                println!("{}", line(&out));
                bench.results.push(out);
                if let Some(stop) = bench.key_thread.take() {
                    stop.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                self.bg_notify = false;
                self.control_phase = 0;
                bench.current = None;
                return None;
            }
            if sc.motion == Motion::Still {
                self.control_phase += 1;
                cx.notify();
            }
            let phase = if bench.recording { t + LEAD_IN } else { t };
            self.velocity(&sc, phase).map(|v| (v * dt) as f32)
        })();
        self.bench = Some(bench);
        delta
    }

    /// Pixels per second of content movement. Positive moves down the list.
    fn velocity(&self, sc: &Scenario, t: f64) -> Option<f64> {
        let dir = if sc.up { -1.0 } else { 1.0 };
        match sc.motion {
            Motion::Still => Some(1800.0), // same input path, nothing to scroll
            Motion::Wheel => Some(1800.0 * dir),
            Motion::Flick => {
                let cycle = t.floor();
                let alt = if cycle as i64 % 2 == 0 { 1.0 } else { -1.0 };
                Some(24_000.0 * (-(t - cycle) / 0.35).exp() * dir * alt)
            }
        }
    }

    fn start(&mut self, sc: Scenario, run: usize, b: &mut Bench, window: &mut Window) {
        eprintln!("-- {} run {run}", sc.name);
        self.jump(sc.row);
        b.current = Some((sc, run));
        b.phase_started = Instant::now();
        b.last_tick = Instant::now();
        b.recording = false;
        self.control_phase = (sc.motion == Motion::Still) as u32;
        self.bg_notify = sc.held;
        if sc.held {
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let s = stop.clone();
            std::thread::spawn(move || {
                let mut repeat = false;
                while !s.load(std::sync::atomic::Ordering::Relaxed) {
                    sys::inject_key_down(repeat);
                    repeat = true;
                    std::thread::sleep(Duration::from_millis(33));
                }
                sys::inject_key_up();
            });
            b.key_thread = Some(stop);
        }
        let _ = window;
    }

    fn outcome(&self, sc: Scenario, run: usize, rec: sys::Recording, b: &Bench) -> Outcome {
        let mut v = rec.intervals_ms;
        v.sort_by(f64::total_cmp);
        let n = v.len().max(1);
        let (paints, sum, max) = self.paint_total.get();
        Outcome {
            scenario: sc,
            run,
            frames: v.len(),
            within: v.iter().filter(|x| **x <= 12.5).count() as f64 / n as f64 * 100.0,
            p99: v.get(n * 99 / 100).copied().unwrap_or(0.0),
            median: v.get(n / 2).copied().unwrap_or(0.0),
            mean: v.iter().sum::<f64>() / n as f64,
            worst: v.last().copied().unwrap_or(0.0),
            dropped: rec.dropped,
            keys: self.keys - b.keys_at,
            notifies: self.notifies - b.notifies_at,
            rows_moved: (offset(&self.handle) as f64 - b.start_offset).abs() / ROW as f64,
            paint_avg_us: sum as f64 / paints.max(1) as f64,
            paint_max_us: max,
            source: rec.source,
            invalid: b.invalid,
        }
    }
}

fn line(o: &Outcome) -> String {
    format!(
        "{}{:<12} #{} frames {:>5}  median {:>5.2}  mean {:>5.2}  <=12.5ms {:>6.2}%  p99 {:>6.2}  worst {:>6.1}  dropped {:>3}  keys {:>3}  notifies {:>4}  rows {:>7.0}  paint avg {:>5.0}us max {:>6}us",
        if o.invalid { "[INACTIVE] " } else { "" }, o.scenario.name, o.run, o.frames, o.median, o.mean, o.within, o.p99, o.worst, o.dropped, o.keys, o.notifies, o.rows_moved, o.paint_avg_us, o.paint_max_us
    )
}

fn finish(cfg: &Cfg, store: &Store, results: &[Outcome]) {
    let mut report = String::new();
    let mut say = |s: String| {
        println!("{s}");
        report.push_str(&s);
        report.push('\n');
    };
    let src = results.first().map_or("", |r| r.source.as_str());
    say(format!(
        "\n== {:?} cap {} {} rows, paint {}, source: {src}",
        cfg.format, cfg.cap, cfg.rows, if cfg.per_edge { "per-edge" } else { "per-colour" }
    ));
    let control_ok = results.iter().filter(|r| r.scenario.name == "control").all(|r| r.mean < 9.0 && r.within >= 99.0);
    say(format!("control: {}", if control_ok { "PASS (source can be trusted at 120 Hz)" } else { "FAIL, every result below is suspect" }));
    for r in results {
        say(line(r));
    }
    say("-- worst of runs".into());
    for sc in scenarios(cfg.rows) {
        let rs: Vec<_> = results.iter().filter(|r| r.scenario.name == sc.name).collect();
        if rs.is_empty() {
            continue;
        }
        let within = rs.iter().map(|r| r.within).fold(f64::MAX, f64::min);
        let worst = rs.iter().map(|r| r.worst).fold(0.0, f64::max);
        let moved = sc.motion == Motion::Still || rs.iter().all(|r| r.rows_moved > 1.0);
        let keyed = !sc.held || rs.iter().all(|r| r.keys > 0);
        // Mean, not median: at 120 Hz, DWM's display intervals alternate about
        // 7.0 / 9.65 ms on Windows, which puts the median at 9.6 ms.
        let frames = rs.iter().all(|r| r.mean < 9.0);
        let verdict = if rs.iter().any(|r| r.invalid) {
            "INVALID (window lost focus during the run)"
        } else if !sc.floor {
            "recorded only"
        } else if !moved || !keyed {
            "INVALID (scroll or keys did not arrive)"
        } else if !frames {
            "INVALID (not presenting at 120 Hz)"
        } else if within >= 99.0 && worst <= 50.0 {
            "PASS"
        } else {
            "FAIL"
        };
        say(format!("{:<12} {:>6.2}%  worst {:>6.1} ms  {verdict}", sc.name, within, worst));
    }
    let (now, peak) = sys::memory();
    let mem_ok = now <= 300_000_000;
    say(format!(
        "memory settled {:.1} MB, peak {:.1} MB, store {:.1} MB  {}",
        now as f64 / 1e6,
        peak as f64 / 1e6,
        store.bytes() as f64 / 1e6,
        if mem_ok { "PASS" } else { "FAIL" }
    ));
    let name = format!("results/{:?}-{}-{}.txt", cfg.format, cfg.cap, std::process::id());
    let _ = std::fs::create_dir_all("results");
    let _ = std::fs::write(&name, report);
}

fn tick(view: Entity<View>, position: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut App) {
    let delta = view.update(cx, |this, cx| {
        if !this.drew {
            this.drew = true;
            this.focus.focus(window, cx);
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent { position, ..Default::default() }),
                cx,
            );
        }
        if this.launch_probe {
            if let Some(t) = sys::first_present_unix_ms() {
                let t0: f64 = std::env::var("CAIRN_T0").unwrap().parse().unwrap();
                println!("{:.1}", t - t0);
                std::process::exit(0);
            }
        }
        this.step(window, cx)
    });
    // CAIRN_IDLE_SETTLE: stop driving frames once the scenarios are done, so the
    // settled reading is taken with no GPU work in flight. Default is unchanged.
    if std::env::var_os("CAIRN_IDLE_SETTLE").is_some() {
        let v = view.read(cx);
        let done = match &v.bench {
            Some(b) => v.full && b.current.is_none() && b.queue.is_empty(),
            None => v.cfg.mode == "empty" && v.drew,
        };
        if done {
            let s = v.cfg.settle;
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs_f64(s));
                let (now, peak) = sys::memory();
                println!("idle settled {:.1} MB, peak {:.1} MB", now as f64 / 1e6, peak as f64 / 1e6);
                let pid = std::process::id().to_string();
                if let Ok(o) = std::process::Command::new("footprint").args(["-p", &pid]).output() {
                    print!("{}", String::from_utf8_lossy(&o.stdout));
                }
                std::process::exit(0);
            });
            return;
        }
    }
    if let Some(d) = delta {
        let e = ScrollEvent {
            position,
            delta: ScrollDelta::Pixels(point(px(0.), px(-d))),
            touch_phase: TouchPhase::Moved,
            ..Default::default()
        };
        window.dispatch_event(PlatformInput::Scroll(e), cx);
    }
    window.on_next_frame(move |w, cx| tick(view, position, w, cx));
}

fn parse_args() -> Cfg {
    let mut a: Vec<String> = std::env::args().skip(1).collect();
    let mode = if a.first().is_some_and(|s| !s.starts_with("--")) { a.remove(0) } else { "play".into() };
    let get = |k: &str| a.iter().position(|x| x == k).map(|i| a[i + 1].clone());
    Cfg {
        mode,
        rows: get("--rows").map_or(1_500_000, |v| v.parse().unwrap()),
        cap: get("--cap").map_or(32, |v| v.parse().unwrap()),
        format: if get("--format").as_deref() == Some("delta") { Format::Delta } else { Format::Rows },
        per_edge: get("--paint").as_deref() == Some("edge"),
        runs: get("--runs").map_or(3, |v| v.parse().unwrap()),
        seconds: get("--seconds").map_or(8.0, |v| v.parse().unwrap()),
        only: get("--only"),
        settle: get("--settle").map_or(10.0, |v| v.parse().unwrap()),
        window: get("--window").map(|v| {
            let (w, h) = v.split_once('x').unwrap();
            (w.parse().unwrap(), h.parse().unwrap())
        }),
    }
}

fn launch_bench(cfg: &Cfg) {
    let exe = std::env::current_exe().unwrap();
    let probe = |_i: usize| -> f64 {
        let out = std::process::Command::new(&exe)
            .args(["probe", "--cap", &cfg.cap.to_string(), "--rows", &cfg.rows.to_string()])
            .args(["--format", if cfg.format == Format::Delta { "delta" } else { "rows" }])
            .env("CAIRN_T0", format!("{:.3}", sys::unix_ms()))
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().parse().expect("probe output")
    };
    probe(0); // warm: the binary has run since boot
    let mut v: Vec<f64> = (0..20).map(probe).collect();
    v.sort_by(f64::total_cmp);
    let (median, worst) = (v[10], v[19]);
    let pass = median <= 400.0 && worst <= 800.0;
    println!("launch median {median:.0} ms, worst {worst:.0} ms, best {:.0} ms  {}", v[0], if pass { "PASS" } else { "FAIL" });
}

fn main() {
    let cfg = parse_args();
    if cfg.mode == "launch" {
        return launch_bench(&cfg);
    }
    sys::install_frame_source();
    gpui_platform::application().run(move |cx| {
        let display = cx.primary_display().expect("display");
        let bounds = match cfg.window {
            Some((w, h)) => Bounds::new(display.bounds().origin, size(px(w), px(h))),
            None => display.bounds(),
        };
        eprintln!("window {:?}", bounds.size);
        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            focus: true,
            ..Default::default()
        };
        let cfg2 = cfg.clone();
        let position = point(bounds.size.width / 2., bounds.size.height / 2.);
        cx.open_window(opts, move |window, cx| {
            let view = cx.new(|cx| {
                let cfg = cfg2.clone();
                let first_rows = if cfg.mode == "empty" { 1 } else { FIRST_CHUNK.min(cfg.rows) };
                let first = Store::build(cfg.format, first_rows, cfg.cap);
                let queue = {
                    let mut q = vec![];
                    for sc in scenarios(cfg.rows) {
                        if cfg.only.as_ref().is_none_or(|o| sc.name.contains(o.as_str())) {
                            q.extend((1..=cfg.runs).map(|r| (sc, r)));
                        }
                    }
                    q.reverse();
                    q
                };
                let bench = (cfg.mode == "bench").then(|| Bench {
                    queue,
                    current: None,
                    phase_started: Instant::now(),
                    recording: false,
                    last_tick: Instant::now(),
                    start_offset: 0.0,
                    keys_at: 0,
                    notifies_at: 0,
                    key_thread: None,
                    results: vec![],
                    settling_since: None,
                    inactive_since: None,
                    invalid: false,
                });
                let (format, rows, cap) = (cfg.format, cfg.rows, cfg.cap);
                let empty = cfg.mode == "empty";
                cx.spawn(async move |this: gpui::WeakEntity<View>, cx| {
                    if empty {
                        return;
                    }
                    let full = cx.background_spawn(async move { Store::build(format, rows, cap) }).await;
                    let _ = this.update(cx, |t, cx| {
                        t.store = Arc::new(full);
                        t.full = true;
                        cx.notify();
                    });
                })
                .detach();
                cx.spawn(async move |this: gpui::WeakEntity<View>, cx| {
                    loop {
                        cx.background_executor().timer(Duration::from_millis(8)).await;
                        let alive = this.update(cx, |t, cx| {
                            if t.bg_notify {
                                t.notifies += 1;
                                cx.notify();
                            }
                        });
                        if alive.is_err() {
                            break;
                        }
                    }
                })
                .detach();
                if cfg.mode == "empty" {
                    let s = cfg.settle;
                    cx.spawn(async move |_, cx| {
                        cx.background_executor().timer(Duration::from_secs_f64(s)).await;
                        let (now, peak) = sys::memory();
                        println!("empty window settled {:.1} MB, peak {:.1} MB", now as f64 / 1e6, peak as f64 / 1e6);
                        std::process::exit(0);
                    })
                    .detach();
                }
                View {
                    launch_probe: cfg.mode == "probe",
                    store: Arc::new(first),
                    full: false,
                    handle: UniformListScrollHandle::new(),
                    focus: cx.focus_handle(),
                    bench,
                    control_phase: 0,
                    keys: 0,
                    notifies: 0,
                    bg_notify: false,
                    paint_total: Rc::new(Cell::new((0, 0, 0))),
                    visible: Default::default(),
                    hud: "".into(),
                    hud_at: Instant::now(),
                    drew: false,
                    control_spin: std::env::var("CAIRN_CONTROL_SPIN_US").map_or(2000, |v| v.parse().unwrap()),
                    cfg,
                }
            });
            let v = view.clone();
            window.on_next_frame(move |w, cx| tick(v, position, w, cx));
            view
        })
        .unwrap();
        cx.activate(true);
    });
}
