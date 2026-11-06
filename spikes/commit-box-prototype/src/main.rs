//! PROTOTYPE, issue #10. Does gpui-component's Textarea hold up as a commit box?
//!
//! Two faces: `cargo run --release` is the window the human runs CHECKLIST.md in,
//! and `-- bench` scripts the key-input Floor from #8. See README.md.
mod sys;

use gpui::*;
use gpui_component::button::Button;
use gpui_component::input::{InputEvent, Paste, Textarea, TextareaState};
use gpui_component::Root;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::*};
use std::time::Duration;
use sys::Key;

const GAP_LIMIT_MS: f64 = 50.0;
const FRAME: Duration = Duration::from_micros(8333);
const WORST: Duration = Duration::from_millis(1);
const CRLF: &str = "fix: straighten the walk\r\n\r\nFirst body line.\r\nSecond body line.\r\n";
/// Excel's text/plain: tabs, CRLF rows, a quoted cell with a bare LF, a trailing CRLF.
const EXCEL: &str = "sha\tauthor\tnote\r\nabc123\tJürgen\t\"two\nlines\"\r\ndef456\tAnna\tok\r\n";
const BURST: &str = "the quick brown fox jumps over a lazy dog while we wait for the graph walk ";

/// How background results reach the view. `Frame` decides the Floor: results are
/// drained once per 120 Hz frame by a timer, which posts to the UI thread as the
/// scheduler's bridge would. `Vsync` drains from on_next_frame, which can't outrun
/// presents. `Worst` is one notify per millisecond, recorded only.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Rate {
    Off,
    Vsync,
    Frame,
    Worst,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    HoldBackspace,
    HoldLeft,
    Burst,
}

#[derive(Clone)]
struct Cfg {
    bench: bool,
    runs: usize,
    only: Option<String>,
    rates: Vec<Rate>,
    hold: f64,
    burst: f64,
    watchdog: bool,
    stall_ms: u64,
    paste_fix: bool,
}

struct View {
    state: Entity<TextareaState>,
    other: Entity<TextareaState>,
    cfg: Cfg,
    rate: Rate,
    generation: u64,
    pending: Arc<AtomicU64>,
    drains: u64,
    renders: u64,
    log: Vec<String>,
    _subs: Vec<Subscription>,
}

fn esc(s: &str) -> String {
    s.replace('\r', "␍").replace('\n', "␊\n").replace('\t', "␉")
}

fn tail(s: &str, n: usize) -> String {
    let c = s.chars().count();
    s.chars().skip(c.saturating_sub(n)).collect()
}

impl View {
    fn say(&mut self, line: String) {
        eprintln!("{line}");
        self.log.push(line);
        let n = self.log.len();
        if n > 16 {
            self.log.drain(..n - 16);
        }
    }

    fn drain(&mut self, cx: &mut Context<Self>) {
        if self.pending.swap(0, Relaxed) > 0 {
            self.drains += 1;
            cx.notify();
        }
    }

    fn set_rate(&mut self, rate: Rate, window: &mut Window, cx: &mut Context<Self>) {
        self.rate = rate;
        self.generation += 1;
        let generation = self.generation;
        match rate {
            Rate::Off => {}
            Rate::Vsync => {
                let view = cx.entity();
                window.on_next_frame(move |w, cx| vsync(view, generation, w, cx));
            }
            Rate::Frame | Rate::Worst => {
                let period = if rate == Rate::Frame { FRAME } else { WORST };
                cx.spawn(async move |this: WeakEntity<View>, cx| {
                    loop {
                        cx.background_executor().timer(period).await;
                        let live = this.update(cx, |t, cx| {
                            if t.generation == generation {
                                t.drain(cx);
                            }
                            t.generation == generation
                        });
                        if !matches!(live, Ok(true)) {
                            break;
                        }
                    }
                })
                .detach();
            }
        }
    }

    fn clip(&mut self, text: &'static str, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
        self.say(format!("clipboard: {name} fixture, now paste"));
        self.state.update(cx, |s, cx| s.focus(window, cx));
    }
}

fn vsync(view: Entity<View>, generation: u64, window: &mut Window, cx: &mut App) {
    let live = view.update(cx, |t, cx| {
        if t.generation == generation {
            t.drain(cx);
        }
        t.generation == generation
    });
    if live {
        window.on_next_frame(move |w, cx| vsync(view, generation, w, cx));
    }
}

impl Render for View {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let marked = self.state.update(cx, |s, cx| s.marked_text_range(window, cx));
        let s = self.state.read(cx);
        let value = s.value().to_string();
        let selected = s.selected_range();
        let cursor = s.cursor();
        let selection = s.selected_value().to_string();
        let focused = s.focus_handle(cx).is_focused(window);
        let cr = value.matches('\r').count();
        let tabs = value.matches('\t').count();
        let combining = value.chars().filter(|c| ('\u{300}'..='\u{36f}').contains(c)).count();
        let hud = format!(
            "rate {:?}  drains {}  renders {}\nfocused {focused}  active {}\nbytes {}  chars {}  cursor {cursor}  selected {selected:?}\nmarked (utf16) {marked:?}\nCR {cr}  TAB {tabs}  combining marks {combining}\nselection: {}",
            self.rate,
            self.drains,
            self.renders,
            window.is_window_active(),
            value.len(),
            value.chars().count(),
            esc(&tail(&selection, 60)),
        );
        let fix = self.cfg.paste_fix;
        let mut area = Textarea::new(&self.state).h(px(240.));
        if fix {
            let state = self.state.clone();
            area = area.on_paste(move |item, window, cx| {
                let Some(text) = item.text() else { return false };
                let clean = text.replace("\r\n", "\n").replace('\r', "\n");
                state.update(cx, |s, cx| s.insert(clean, window, cx));
                true
            });
        }
        div()
            .id("root")
            .size_full()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111111))
            .p_3()
            .gap_3()
            .flex()
            .child(
                div()
                    .w(px(620.))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child("Commit box. Cmd/Ctrl-Return logs a COMMIT.")
                    .child(area)
                    .child("Second field, for focus changes:")
                    .child(Textarea::new(&self.other).h(px(60.)))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(Button::new("crlf").label("Clipboard: CRLF").on_click(
                                cx.listener(|t, _, w, cx| t.clip(CRLF, "CRLF", w, cx)),
                            ))
                            .child(Button::new("excel").label("Clipboard: Excel").on_click(
                                cx.listener(|t, _, w, cx| t.clip(EXCEL, "Excel", w, cx)),
                            ))
                            .child(Button::new("rate").label("Next rate").on_click(cx.listener(
                                |t, _, w, cx| {
                                    let next = match t.rate {
                                        Rate::Off => Rate::Vsync,
                                        Rate::Vsync => Rate::Frame,
                                        Rate::Frame => Rate::Worst,
                                        Rate::Worst => Rate::Off,
                                    };
                                    t.set_rate(next, w, cx);
                                },
                            )))
                            .child(Button::new("clear").label("Clear").on_click(cx.listener(
                                |t, _, w, cx| {
                                    t.state.update(cx, |s, cx| {
                                        s.set_value("", w, cx);
                                        s.focus(w, cx);
                                    });
                                },
                            ))),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .text_size(px(12.))
                    .child(div().font_family("Menlo").whitespace_normal().child(hud))
                    .child(div().bg(rgb(0xf2f2f2)).p_1().child(esc(&tail(&value, 400))))
                    .child(div().bg(rgb(0xf2f2f2)).p_1().child(self.log.join("\n"))),
            )
    }
}

fn key_thread(kind: Kind, seconds: f64) -> (Arc<AtomicU32>, Arc<AtomicBool>, Arc<AtomicBool>) {
    let sent = Arc::new(AtomicU32::new(0));
    let done = Arc::new(AtomicBool::new(false));
    let missed = Arc::new(AtomicBool::new(false));
    let (s, d, m) = (sent.clone(), done.clone(), missed.clone());
    std::thread::spawn(move || {
        let t0 = std::time::Instant::now();
        let wait_until = |at: f64| {
            let left = at - t0.elapsed().as_secs_f64();
            if left > 0.0 {
                std::thread::sleep(Duration::from_secs_f64(left));
            }
        };
        let mut i = 0usize;
        match kind {
            Kind::HoldBackspace | Kind::HoldLeft => {
                let key = if kind == Kind::HoldLeft { Key::Left } else { Key::Backspace };
                while t0.elapsed().as_secs_f64() < seconds {
                    if sys::inject_key_down(key, i > 0) {
                        s.fetch_add(1, Relaxed);
                    } else {
                        m.store(true, Relaxed);
                    }
                    i += 1;
                    wait_until(i as f64 / 30.0);
                }
                sys::inject_key_up(key);
            }
            Kind::Burst => {
                let total = (seconds * 10.0) as usize;
                for c in BURST.chars().cycle().take(total) {
                    let key = Key::Char(c);
                    if sys::inject_key_down(key, false) {
                        s.fetch_add(1, Relaxed);
                    } else {
                        m.store(true, Relaxed);
                    }
                    sys::inject_key_up(key);
                    i += 1;
                    wait_until(i as f64 / 10.0);
                }
            }
        }
        d.store(true, Relaxed);
    });
    (sent, done, missed)
}

struct Outcome {
    name: String,
    rate: Rate,
    run: usize,
    gap: f64,
    presents: usize,
    dropped: u64,
    renders: u64,
    drains: u64,
    errors: String,
    invalid: Option<String>,
}

impl Outcome {
    fn pass(&self) -> bool {
        self.gap <= GAP_LIMIT_MS && self.errors.is_empty()
    }
}

async fn wait(cx: &mut AsyncWindowContext, seconds: f64) {
    cx.background_executor().timer(Duration::from_secs_f64(seconds)).await;
}

async fn paste_probe(
    this: &WeakEntity<View>,
    cx: &mut AsyncWindowContext,
    name: &str,
    text: &'static str,
) {
    let _ = this.update_in(cx, |t, window, cx| {
        t.state.update(cx, |s, cx| s.set_value("", window, cx));
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
        window.dispatch_action(Box::new(Paste), cx);
    });
    wait(cx, 0.3).await;
    let _ = this.update_in(cx, |t, _, cx| {
        let v = t.state.read(cx).value().to_string();
        println!(
            "paste {name}: CR {} TAB {} value {:?}",
            v.matches('\r').count(),
            v.matches('\t').count(),
            v
        );
    });
}

async fn bench(this: WeakEntity<View>, cfg: Cfg, cx: &mut AsyncWindowContext) {
    wait(cx, 1.5).await;
    let _ = this.update_in(cx, |t, window, cx| {
        t.state.update(cx, |s, cx| s.focus(window, cx));
        if !window.is_window_active() {
            println!("window isn't active, results will be INVALID");
        }
    });
    wait(cx, 0.3).await;
    if cfg.watchdog {
        sys::start_watchdog();
    }

    let saved = cx.update(|_, cx| cx.read_from_clipboard()).ok().flatten();
    paste_probe(&this, cx, "crlf", CRLF).await;
    paste_probe(&this, cx, "excel", EXCEL).await;
    if let Some(item) = saved {
        let _ = cx.update(|_, cx| cx.write_to_clipboard(item));
    }

    let seed: String = "0123456789abcde\n".repeat(250);
    let burst_text = |n: usize| -> String { BURST.chars().cycle().take(n).collect() };
    let mut results: Vec<Outcome> = vec![];
    for rate in cfg.rates.clone() {
        for kind in [Kind::HoldBackspace, Kind::HoldLeft, Kind::Burst] {
            let name = format!("{kind:?}").to_lowercase();
            if cfg.only.as_ref().is_some_and(|o| !name.contains(o.as_str())) {
                continue;
            }
            for run in 1..=cfg.runs {
                let seconds = if kind == Kind::Burst { cfg.burst } else { cfg.hold };
                let _ = this.update_in(cx, |t, window, cx| {
                    let (text, at) = if kind == Kind::Burst { ("", 0) } else { (seed.as_str(), seed.len()) };
                    t.state.update(cx, |s, cx| {
                        s.set_value(text, window, cx);
                        s.set_selected_range(at..at, cx);
                        s.focus(window, cx);
                    });
                    t.set_rate(rate, window, cx);
                    t.renders = 0;
                    t.drains = 0;
                });
                wait(cx, 0.7).await;
                sys::begin_recording(1.0 + seconds + 0.5);
                wait(cx, 1.0).await;
                let (sent, done, missed) = key_thread(kind, seconds);
                if cfg.stall_ms > 0 {
                    wait(cx, seconds / 2.0).await;
                    std::thread::sleep(Duration::from_millis(cfg.stall_ms));
                }
                while !done.load(Relaxed) {
                    wait(cx, 0.05).await;
                }
                wait(cx, 0.5).await;
                let burst_expected = burst_text(sent.load(Relaxed) as usize);
                let outcome = this.update_in(cx, |t, window, cx| {
                    let rec = sys::end_recording();
                    let sent = sent.load(Relaxed) as usize;
                    let s = t.state.read(cx);
                    let errors = match kind {
                        Kind::HoldBackspace => {
                            let gone = seed.len() - s.value().len();
                            if gone == sent { String::new() } else { format!("deleted {gone} of {sent}") }
                        }
                        Kind::HoldLeft => {
                            let moved = seed.len() - s.cursor();
                            if moved == sent { String::new() } else { format!("moved {moved} of {sent}") }
                        }
                        Kind::Burst => {
                            let v = s.value().to_string();
                            if v == burst_expected {
                                String::new()
                            } else {
                                let at = v.chars().zip(burst_expected.chars()).take_while(|(a, b)| a == b).count();
                                format!("got {} of {} chars, first diff at {at}", v.chars().count(), sent)
                            }
                        }
                    };
                    let gap = rec.intervals_ms.iter().copied().fold(0.0, f64::max);
                    let minimum = ((1.0 + seconds) * 30.0) as usize;
                    let invalid = if missed.load(Relaxed) || !window.is_window_active() {
                        Some("window lost key status".to_string())
                    } else if sent == 0 {
                        Some("no keys sent".to_string())
                    } else if rec.intervals_ms.len() < minimum {
                        Some(format!("only {} presents, want {minimum}+", rec.intervals_ms.len()))
                    } else {
                        None
                    };
                    Outcome {
                        name: name.clone(),
                        rate,
                        run,
                        gap,
                        presents: rec.intervals_ms.len(),
                        dropped: rec.dropped,
                        renders: t.renders,
                        drains: t.drains,
                        errors,
                        invalid,
                    }
                });
                if let Ok(o) = outcome {
                    println!(
                        "{:<14} {:<6} run {} gap {:>7.1} ms  presents {:>5} dropped {:>3} renders {:>5} drains {:>5} {} {}",
                        o.name,
                        format!("{:?}", o.rate),
                        o.run,
                        o.gap,
                        o.presents,
                        o.dropped,
                        o.renders,
                        o.drains,
                        if o.pass() { "pass" } else { "FAIL" },
                        match (&o.invalid, o.errors.is_empty()) {
                            (Some(why), _) => format!("INVALID: {why} {}", o.errors),
                            (None, false) => o.errors.clone(),
                            _ => String::new(),
                        }
                    );
                    results.push(o);
                }
            }
        }
    }

    println!("\nworst of {} runs, limit {GAP_LIMIT_MS} ms, watchdog {}", cfg.runs, cfg.watchdog);
    for rate in &cfg.rates {
        for kind in ["holdbackspace", "holdleft", "burst"] {
            let rs: Vec<_> = results.iter().filter(|r| r.rate == *rate && r.name == kind).collect();
            if rs.is_empty() {
                continue;
            }
            let worst = rs.iter().map(|r| r.gap).fold(0.0, f64::max);
            let verdict = if rs.iter().any(|r| r.invalid.is_some()) {
                "INVALID"
            } else if rs.iter().all(|r| r.pass()) {
                "pass"
            } else {
                "FAIL"
            };
            let role = if *rate == Rate::Frame { "Floor" } else { "recorded" };
            println!("{kind:<14} {:<6} {role:<9} worst gap {worst:>7.1} ms  {verdict}", format!("{rate:?}"));
        }
    }
    std::process::exit(0);
}

fn parse_args() -> Cfg {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |n: &str| args.iter().position(|a| a == n).and_then(|i| args.get(i + 1)).cloned();
    let has = |n: &str| args.iter().any(|a| a == n);
    let num = |n: &str, d: f64| flag(n).map_or(d, |v| v.parse().unwrap());
    let rates = flag("--rates").unwrap_or("frame,vsync,worst".into());
    Cfg {
        bench: args.first().is_some_and(|a| a == "bench"),
        runs: num("--runs", 3.0) as usize,
        only: flag("--only"),
        rates: rates
            .split(',')
            .map(|r| match r {
                "off" => Rate::Off,
                "vsync" => Rate::Vsync,
                "frame" => Rate::Frame,
                "worst" => Rate::Worst,
                _ => panic!("rate {r}"),
            })
            .collect(),
        hold: num("--hold", 10.0),
        burst: num("--burst", 30.0),
        watchdog: has("--watchdog"),
        stall_ms: num("--stall-ms", 0.0) as u64,
        paste_fix: has("--paste-fix"),
    }
}

fn main() {
    let cfg = parse_args();
    sys::install_frame_source();
    gpui_platform::application().run(move |cx| {
        gpui_component::init(cx);
        let bounds = Bounds::centered(None, size(px(1180.), px(760.)), cx);
        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            focus: true,
            ..Default::default()
        };
        let pending = Arc::new(AtomicU64::new(0));
        let p = pending.clone();
        std::thread::spawn(move || {
            // The results a scheduler would be producing, at 1 kHz.
            loop {
                p.fetch_add(1, Relaxed);
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        cx.open_window(opts, move |window, cx| {
            let view = cx.new(|cx| {
                let state = cx.new(|cx| TextareaState::new(window, cx));
                let other = cx.new(|cx| TextareaState::new(window, cx));
                let mut subs = vec![cx.subscribe_in(&state, window, |t: &mut View, st, ev: &InputEvent, window, cx| {
                    match ev {
                        InputEvent::PressEnter { secondary: true, .. } => {
                            let marked = st.update(cx, |s, cx| s.marked_text_range(window, cx));
                            let v = st.read(cx).value().to_string();
                            t.say(format!("COMMIT marked {marked:?} value {v:?}"));
                        }
                        InputEvent::Focus => t.say("focus".into()),
                        InputEvent::Blur => t.say("blur".into()),
                        _ => {}
                    }
                })];
                if !cfg.bench {
                    subs.push(cx.observe_keystrokes(|t: &mut View, e, _, cx| {
                        let k = &e.keystroke;
                        t.say(format!("key {:?} mods {:?} char {:?}", k.key, k.modifiers, k.key_char));
                        cx.notify();
                    }));
                    subs.push(cx.observe_window_activation(window, |t: &mut View, w, cx| {
                        t.say(format!("window active {}", w.is_window_active()));
                        cx.notify();
                    }));
                }
                if cfg.bench {
                    let c = cfg.clone();
                    cx.spawn_in(window, async move |this, cx| bench(this, c, cx).await).detach();
                }
                let mut v = View {
                    state,
                    other,
                    cfg: cfg.clone(),
                    rate: Rate::Off,
                    generation: 0,
                    pending,
                    drains: 0,
                    renders: 0,
                    log: vec![],
                    _subs: subs,
                };
                if !cfg.bench {
                    v.set_rate(Rate::Frame, window, cx);
                }
                v
            });
            view.update(cx, |t, cx| t.state.update(cx, |s, cx| s.focus(window, cx)));
            cx.new(|cx| Root::new(view, window, cx))
        })
        .unwrap();
        cx.activate(true);
    });
}
