// Black-box latency probe for Windows: post a real input event, record the screen with
// desktop duplication, time the first and last changed frames in a region.
// Throwaway spike for issue 7, the Windows counterpart of ../probe.swift.
use serde_json::{Value, json};
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows::Win32::Security::*;
use windows::Win32::System::Console::*;
use windows::Win32::System::Diagnostics::ToolHelp::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Performance::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

type R<T> = windows::core::Result<T>;

fn qpc_freq() -> f64 {
    let mut f = 0i64;
    unsafe { QueryPerformanceFrequency(&mut f).unwrap() };
    f as f64
}
fn host_ms() -> f64 {
    let mut c = 0i64;
    unsafe { QueryPerformanceCounter(&mut c).unwrap() };
    c as f64 * 1000.0 / qpc_freq()
}
fn sleep_ms(ms: f64) {
    let end = host_ms() + ms;
    if ms > 3.0 {
        std::thread::sleep(Duration::from_secs_f64((ms - 2.0) / 1000.0));
    }
    while host_ms() < end {
        std::hint::spin_loop();
    }
}

#[derive(Clone, Copy)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}
fn rect_arg(s: &str) -> Rect {
    let v: Vec<i32> = s.split(',').map(|p| p.parse().unwrap()).collect();
    Rect { x: v[0], y: v[1], w: v[2], h: v[3] }
}
fn pt_arg(s: &str) -> (i32, i32) {
    let v: Vec<i32> = s.split(',').map(|p| p.parse::<f64>().unwrap() as i32).collect();
    (v[0], v[1])
}

#[derive(Clone, Copy)]
struct Frame {
    t_ms: f64,
    diff_prev: f64,
    diff_base: f64,
    accumulated: u32,
}

#[derive(Default)]
struct Store {
    prev: Option<Vec<u8>>,
    base: Option<Vec<u8>>,
    frames: Vec<Frame>,
}

struct Recorder {
    store: Mutex<Store>,
    stop: AtomicBool,
    error: Mutex<Option<String>>,
}
impl Recorder {
    fn rebase(&self) {
        let mut s = self.store.lock().unwrap();
        s.base = s.prev.clone();
        s.frames.clear();
    }
    fn snapshot(&self) -> Vec<Frame> {
        self.store.lock().unwrap().frames.clone()
    }
}

fn diff(a: &[u8], b: &[u8]) -> f64 {
    let n = a.iter().zip(b).filter(|(x, y)| (**x as i32 - **y as i32).abs() > 24).count();
    n as f64 / a.len() as f64
}

struct Duplicator {
    _dev: ID3D11Device,
    ctx: ID3D11DeviceContext,
    dup: IDXGIOutputDuplication,
    staging: ID3D11Texture2D,
    region: D3D11_BOX,
    w: usize,
    h: usize,
}

fn open_duplicator(rect: Rect) -> R<Duplicator> {
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut i = 0;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            i += 1;
            let mut j = 0;
            while let Ok(output) = adapter.EnumOutputs(j) {
                j += 1;
                let d = output.GetDesc()?;
                let c = d.DesktopCoordinates;
                if rect.x < c.left || rect.y < c.top || rect.x >= c.right || rect.y >= c.bottom {
                    continue;
                }
                let mut dev = None;
                let mut ctx = None;
                D3D11CreateDevice(
                    &adapter,
                    D3D_DRIVER_TYPE_UNKNOWN,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut dev),
                    None,
                    Some(&mut ctx),
                )?;
                let (dev, ctx): (ID3D11Device, ID3D11DeviceContext) = (dev.unwrap(), ctx.unwrap());
                let out5: IDXGIOutput5 = output.cast()?;
                let dup = out5.DuplicateOutput1(&dev, 0, &[DXGI_FORMAT_B8G8R8A8_UNORM])?;
                let desc = D3D11_TEXTURE2D_DESC {
                    Width: rect.w as u32,
                    Height: rect.h as u32,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_STAGING,
                    BindFlags: 0,
                    CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                    MiscFlags: 0,
                };
                let mut tex = None;
                dev.CreateTexture2D(&desc, None, Some(&mut tex))?;
                let (lx, ly) = (rect.x - c.left, rect.y - c.top);
                let region = D3D11_BOX {
                    left: lx as u32,
                    top: ly as u32,
                    front: 0,
                    right: (lx + rect.w) as u32,
                    bottom: (ly + rect.h) as u32,
                    back: 1,
                };
                return Ok(Duplicator {
                    _dev: dev,
                    ctx,
                    dup,
                    staging: tex.unwrap(),
                    region,
                    w: rect.w as usize,
                    h: rect.h as usize,
                });
            }
        }
        Err(Error::new(E_FAIL, "no output at rect"))
    }
}

impl Duplicator {
    /// Next desktop image in BGRA; Ok(None) on timeout or a pointer-only update.
    fn next(&self, timeout_ms: u32) -> R<Option<(i64, u32, Vec<u8>)>> {
        unsafe {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut res = None;
            match self.dup.AcquireNextFrame(timeout_ms, &mut info, &mut res) {
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
                Err(e) => return Err(e),
                Ok(()) => {}
            }
            let mut out = None;
            if info.LastPresentTime != 0 {
                let tex: ID3D11Texture2D = res.unwrap().cast()?;
                self.ctx.CopySubresourceRegion(&self.staging, 0, 0, 0, 0, &tex, 0, Some(&self.region));
                let mut m = D3D11_MAPPED_SUBRESOURCE::default();
                self.ctx.Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
                let mut bgra = vec![0u8; self.w * self.h * 4];
                for y in 0..self.h {
                    let row = (m.pData as *const u8).add(y * m.RowPitch as usize);
                    std::ptr::copy_nonoverlapping(row, bgra.as_mut_ptr().add(y * self.w * 4), self.w * 4);
                }
                self.ctx.Unmap(&self.staging, 0);
                out = Some((info.LastPresentTime, info.AccumulatedFrames, bgra));
            }
            self.dup.ReleaseFrame()?;
            Ok(out)
        }
    }
}

fn luma(bgra: &[u8]) -> Vec<u8> {
    bgra.chunks_exact(4).map(|p| ((p[0] as u32 + 2 * p[1] as u32 + p[2] as u32) / 4) as u8).collect()
}

fn start_recorder(rect: Rect) -> (Arc<Recorder>, std::thread::JoinHandle<()>) {
    let rec = Arc::new(Recorder {
        store: Mutex::new(Store::default()),
        stop: AtomicBool::new(false),
        error: Mutex::new(None),
    });
    let r = rec.clone();
    let (tx, rx) = mpsc::channel();
    let h = std::thread::spawn(move || {
        unsafe {
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
        }
        let freq = qpc_freq();
        let mut d = match open_duplicator(rect) {
            Ok(d) => d,
            Err(e) => {
                *r.error.lock().unwrap() = Some(e.to_string());
                let _ = tx.send(());
                return;
            }
        };
        let _ = tx.send(());
        while !r.stop.load(Ordering::Relaxed) {
            match d.next(20) {
                Ok(Some((t, acc, bgra))) => {
                    let cur = luma(&bgra);
                    let mut s = r.store.lock().unwrap();
                    if s.base.is_none() {
                        s.base = Some(cur.clone());
                    }
                    let f = Frame {
                        t_ms: t as f64 * 1000.0 / freq,
                        diff_prev: s.prev.as_ref().map_or(0.0, |p| diff(&cur, p)),
                        diff_base: diff(&cur, s.base.as_ref().unwrap()),
                        accumulated: acc,
                    };
                    s.frames.push(f);
                    s.prev = Some(cur);
                }
                Ok(None) => {}
                Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => match open_duplicator(rect) {
                    Ok(n) => d = n,
                    Err(_) => std::thread::sleep(Duration::from_millis(50)),
                },
                Err(e) => {
                    *r.error.lock().unwrap() = Some(e.to_string());
                    return;
                }
            }
        }
    });
    rx.recv().unwrap();
    (rec, h)
}

fn input_mouse(flags: MOUSE_EVENT_FLAGS, data: i32) {
    let i = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT { dwFlags: flags, mouseData: data as u32, ..Default::default() },
        },
    };
    unsafe { SendInput(&[i], size_of::<INPUT>() as i32) };
}
fn input_key(vk: u16, up: bool) {
    let i = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                ..Default::default()
            },
        },
    };
    unsafe { SendInput(&[i], size_of::<INPUT>() as i32) };
}
fn move_to(p: (i32, i32)) {
    unsafe { SetCursorPos(p.0, p.1).unwrap() };
}
fn click() -> f64 {
    let t0 = host_ms();
    input_mouse(MOUSEEVENTF_LEFTDOWN, 0);
    sleep_ms(10.0);
    input_mouse(MOUSEEVENTF_LEFTUP, 0);
    t0
}

fn report(fs: &[Frame], t0: f64, threshold: f64) -> serde_json::Map<String, Value> {
    let after: Vec<&Frame> = fs.iter().filter(|f| f.t_ms >= t0).collect();
    let changed: Vec<&&Frame> = after.iter().filter(|f| f.diff_prev >= threshold).collect();
    let mut m = serde_json::Map::new();
    m.insert("first_ms".into(), json!(changed.first().map(|f| f.t_ms - t0)));
    m.insert("settle_ms".into(), json!(changed.last().map(|f| f.t_ms - t0)));
    m.insert("changed_frames".into(), json!(changed.len()));
    m.insert("frames".into(), json!(after.len()));
    m.insert("final_diff_base".into(), json!(after.last().map_or(0.0, |f| f.diff_base)));
    m.insert("accumulated_max".into(), json!(after.iter().map(|f| f.accumulated).max().unwrap_or(0)));
    m
}

fn run_action(a: &str) -> f64 {
    if let Some(p) = a.strip_prefix("click:") {
        move_to(pt_arg(p));
        sleep_ms(120.0);
        return f64::NAN; // the caller rebases, then clicks
    }
    if let Some(c) = a.strip_prefix("exec:") {
        use std::os::windows::process::CommandExt;
        let t0 = host_ms();
        // Null handles: a launched GitKraken must not inherit the pipes the driver reads.
        std::process::Command::new("cmd.exe")
            .raw_arg("/c")
            .raw_arg(c)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        return t0;
    }
    if let Some(p) = a.strip_prefix("append:") {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(p).unwrap();
        let t0 = host_ms();
        f.write_all(b"\n").unwrap();
        return t0;
    }
    if let Some(p) = a.strip_prefix("truncate:") {
        let (size, path) = p.split_once(':').unwrap();
        let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        let t0 = host_ms();
        f.set_len(size.parse().unwrap()).unwrap();
        return t0;
    }
    panic!("unknown action {a}");
}

fn watch(rect: Rect, action: &str, threshold: f64, quiet: f64, timeout: f64, min: f64) {
    let (rec, h) = start_recorder(rect);
    if let Some(e) = rec.error.lock().unwrap().clone() {
        println!("{}", json!({"error": e}));
        return;
    }
    sleep_ms(400.0);
    rec.rebase();
    let mut t0 = run_action(action);
    if t0.is_nan() {
        rec.rebase();
        t0 = click();
    }
    loop {
        sleep_ms(50.0);
        let now = host_ms();
        let fs = rec.snapshot();
        if now - t0 >= timeout {
            break;
        }
        let last = fs.iter().filter(|f| f.t_ms >= t0 && f.diff_prev >= threshold).last();
        if let Some(l) = last {
            if now - l.t_ms >= quiet && now - t0 >= min {
                break;
            }
        }
    }
    rec.stop.store(true, Ordering::Relaxed);
    h.join().unwrap();
    let fs = rec.snapshot();
    let mut r = report(&fs, t0, threshold);
    r.insert("kind".into(), json!("watch"));
    r.insert("action".into(), json!(action));
    let series: Vec<Value> = fs
        .iter()
        .filter(|f| f.t_ms >= t0 && f.diff_prev >= threshold)
        .take(400)
        .map(|f| json!([(f.t_ms - t0).round(), (f.diff_prev * 1000.0).round() / 1000.0]))
        .collect();
    r.insert("series".into(), json!(series));
    println!("{}", Value::Object(r));
}

fn gaps(fs: &[Frame]) -> Vec<f64> {
    fs.windows(2).map(|w| ((w[1].t_ms - w[0].t_ms) * 10.0).round() / 10.0).collect()
}

fn scroll(rect: Rect, at: (i32, i32), delta: i32, ticks: u32, every: f64) {
    let (rec, h) = start_recorder(rect);
    move_to(at);
    sleep_ms(400.0);
    rec.rebase();
    let t0 = host_ms();
    for i in 0..ticks {
        input_mouse(MOUSEEVENTF_WHEEL, delta);
        let wait = t0 + (i + 1) as f64 * every - host_ms();
        if wait > 0.0 {
            sleep_ms(wait);
        }
    }
    let t1 = host_ms();
    sleep_ms(300.0);
    rec.stop.store(true, Ordering::Relaxed);
    h.join().unwrap();
    let f: Vec<Frame> = rec.snapshot().into_iter().filter(|f| f.t_ms >= t0 && f.t_ms <= t1 && f.diff_prev > 0.0).collect();
    println!(
        "{}",
        json!({"kind": "scroll", "seconds": (t1 - t0) / 1000.0, "frames": f.len(), "gaps_ms": gaps(&f), "t0_ms": t0, "t1_ms": t1})
    );
}

fn shot(rect: Rect, path: &str) {
    let d = open_duplicator(rect).unwrap();
    let start = host_ms();
    let (_, _, bgra) = loop {
        if let Some(f) = d.next(100).unwrap() {
            break f;
        }
        if host_ms() - start > 3000.0 {
            panic!("no frame");
        }
    };
    let file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut enc = png::Encoder::new(file, rect.w as u32, rect.h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let rgb: Vec<u8> = bgra.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0]]).collect();
    enc.write_header().unwrap().write_image_data(&rgb).unwrap();
}

// --- windows owned by a process name
fn pids_of(name: &str) -> Vec<u32> {
    let mut out = vec![];
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).unwrap();
        let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let n = String::from_utf16_lossy(&e.szExeFile);
            if n.split('\0').next().unwrap_or("").eq_ignore_ascii_case(name) {
                out.push(e.th32ProcessID);
            }
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

struct Win {
    hwnd: HWND,
    title: String,
    rect: RECT,
    pid: u32,
    zoomed: bool,
}
unsafe extern "system" fn enum_cb(h: HWND, l: LPARAM) -> BOOL {
    unsafe {
        let v = &mut *(l.0 as *mut Vec<Win>);
        if IsWindowVisible(h).as_bool() {
            let mut pid = 0;
            GetWindowThreadProcessId(h, Some(&mut pid));
            let mut b = [0u16; 256];
            let n = GetWindowTextW(h, &mut b);
            let mut rect = RECT::default();
            let _ = GetWindowRect(h, &mut rect);
            v.push(Win { hwnd: h, title: String::from_utf16_lossy(&b[..n as usize]), rect, pid, zoomed: IsZoomed(h).as_bool() });
        }
    }
    TRUE
}
fn all_windows() -> Vec<Win> {
    let mut v: Vec<Win> = vec![];
    unsafe { EnumWindows(Some(enum_cb), LPARAM(&mut v as *mut _ as isize)).unwrap() };
    v
}
fn gk_windows() -> Vec<Win> {
    let pids = pids_of("gitkraken.exe");
    all_windows().into_iter().filter(|w| pids.contains(&w.pid) && !w.title.is_empty()).collect()
}
fn main_window() -> Option<Win> {
    gk_windows().into_iter().max_by_key(|w| (w.rect.right - w.rect.left) * (w.rect.bottom - w.rect.top))
}
fn activate() {
    if let Some(w) = main_window() {
        unsafe {
            if IsIconic(w.hwnd).as_bool() {
                let _ = ShowWindow(w.hwnd, SW_RESTORE);
            }
            // A background process may only take the foreground right after input.
            input_key(VK_MENU.0, false);
            input_key(VK_MENU.0, true);
            let _ = SetForegroundWindow(w.hwnd);
        }
    }
}
fn foreground() -> Value {
    unsafe {
        let h = GetForegroundWindow();
        let mut pid = 0;
        GetWindowThreadProcessId(h, Some(&mut pid));
        json!({"gitkraken": pids_of("gitkraken.exe").contains(&pid), "pid": pid})
    }
}

// --- display
fn primary_device() -> [u16; 32] {
    unsafe {
        let mut i = 0;
        loop {
            let mut d = DISPLAY_DEVICEW { cb: size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
            if !EnumDisplayDevicesW(PCWSTR::null(), i, &mut d, 0).as_bool() {
                panic!("no primary");
            }
            if (d.StateFlags & DISPLAY_DEVICE_PRIMARY_DEVICE).0 != 0 {
                return d.DeviceName;
            }
            i += 1;
        }
    }
}
fn current_mode(dev: &[u16; 32]) -> DEVMODEW {
    let mut m = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() };
    unsafe { EnumDisplaySettingsW(PCWSTR(dev.as_ptr()), ENUM_CURRENT_SETTINGS, &mut m).ok().unwrap() };
    m
}
fn modes() {
    let dev = primary_device();
    let c = current_mode(&dev);
    let mut hz = vec![];
    let mut i = 0;
    loop {
        let mut m = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() };
        if unsafe { !EnumDisplaySettingsW(PCWSTR(dev.as_ptr()), ENUM_DISPLAY_SETTINGS_MODE(i), &mut m).as_bool() } {
            break;
        }
        if m.dmPelsWidth == c.dmPelsWidth && m.dmPelsHeight == c.dmPelsHeight && !hz.contains(&m.dmDisplayFrequency) {
            hz.push(m.dmDisplayFrequency);
        }
        i += 1;
    }
    println!("{}", json!({"current_hz": c.dmDisplayFrequency, "w": c.dmPelsWidth, "h": c.dmPelsHeight, "hz": hz}));
}
fn set_hz(hz: u32) {
    let dev = primary_device();
    let mut m = current_mode(&dev);
    m.dmDisplayFrequency = hz;
    m.dmFields = DM_DISPLAYFREQUENCY;
    let r = unsafe { ChangeDisplaySettingsExW(PCWSTR(dev.as_ptr()), Some(&m), None, CDS_TYPE(0), None) };
    println!("{}", json!({"result": r.0, "now": current_mode(&dev).dmDisplayFrequency}));
}

// --- our own window: calibrate flips it inside WM_LBUTTONDOWN, control repaints every refresh.
struct Gfx {
    ctx: ID3D11DeviceContext,
    sc: IDXGISwapChain,
    rtv: ID3D11RenderTargetView,
}
impl Gfx {
    fn paint(&self, v: f32) {
        unsafe {
            self.ctx.ClearRenderTargetView(&self.rtv, &[v, v, v, 1.0]);
            let _ = self.sc.Present(1, DXGI_PRESENT(0));
        }
    }
}
static mut GFX: Option<Gfx> = None;
static mut WHITE: bool = false;
#[allow(static_mut_refs)]
unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        if m == WM_LBUTTONDOWN {
            WHITE = !WHITE;
            if let Some(g) = &GFX {
                g.paint(if WHITE { 1.0 } else { 0.0 });
            }
            return LRESULT(0);
        }
        DefWindowProcW(h, m, w, l)
    }
}
#[allow(static_mut_refs)]
fn own_window(r: Rect) -> HWND {
    unsafe {
        let inst = GetModuleHandleW(None).unwrap();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: inst.into(),
            lpszClassName: w!("cairnprobe"),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let h = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            w!("cairnprobe"),
            w!("cairnprobe"),
            WS_POPUP | WS_VISIBLE,
            r.x,
            r.y,
            r.w,
            r.h,
            None,
            None,
            Some(inst.into()),
            None,
        )
        .unwrap();
        let sd = DXGI_SWAP_CHAIN_DESC {
            BufferDesc: DXGI_MODE_DESC {
                Width: r.w as u32,
                Height: r.h as u32,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                ..Default::default()
            },
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            OutputWindow: h,
            Windowed: TRUE,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            Flags: 0,
        };
        let (mut sc, mut dev, mut ctx) = (None, None, None);
        D3D11CreateDeviceAndSwapChain(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&sd),
            Some(&mut sc),
            Some(&mut dev),
            None,
            Some(&mut ctx),
        )
        .unwrap();
        let (sc, dev, ctx): (IDXGISwapChain, ID3D11Device, ID3D11DeviceContext) = (sc.unwrap(), dev.unwrap(), ctx.unwrap());
        let bb: ID3D11Texture2D = sc.GetBuffer(0).unwrap();
        let mut rtv = None;
        dev.CreateRenderTargetView(&bb, None, Some(&mut rtv)).unwrap();
        let g = Gfx { ctx, sc, rtv: rtv.unwrap() };
        g.paint(0.0);
        GFX = Some(g);
        // Foreground like the app under test, in case Windows treats background windows differently.
        input_key(VK_MENU.0, false);
        input_key(VK_MENU.0, true);
        let _ = SetForegroundWindow(h);
        h
    }
}
fn pump(ms: f64) {
    let end = host_ms() + ms;
    unsafe {
        let mut m = MSG::default();
        while host_ms() < end {
            while PeekMessageW(&mut m, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&m);
                DispatchMessageW(&m);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
fn calibrate(n: usize) {
    let r = Rect { x: 1400, y: 800, w: 600, h: 450 };
    let _h = own_window(r);
    pump(500.0);
    let inner = Rect { x: r.x + 75, y: r.y + 75, w: 450, h: 300 };
    let (rec, h) = start_recorder(inner);
    pump(500.0);
    move_to((r.x + 300, r.y + 225));
    pump(200.0);
    let mut out = vec![];
    for i in 0..n {
        pump(180.0 + (i as f64 * 37.0) % 100.0);
        rec.rebase();
        let t0 = host_ms();
        input_mouse(MOUSEEVENTF_LEFTDOWN, 0);
        pump(10.0);
        input_mouse(MOUSEEVENTF_LEFTUP, 0);
        pump(180.0);
        if let Some(f) = report(&rec.snapshot(), t0, 0.5)["first_ms"].as_f64() {
            out.push((f * 100.0).round() / 100.0);
        }
    }
    rec.stop.store(true, Ordering::Relaxed);
    h.join().unwrap();
    println!("{}", json!({"kind": "calibrate", "n": out.len(), "ms": out}));
}
#[allow(static_mut_refs)]
fn control(seconds: f64, capture: Rect) {
    let r = Rect { x: 630, y: 240, w: 1425, h: 1350 };
    let _h = own_window(r);
    pump(500.0);
    let (rec, h) = start_recorder(capture);
    pump(500.0);
    rec.rebase();
    let t0 = host_ms();
    let mut v = 0.0f32;
    let mut m = MSG::default();
    while host_ms() - t0 < seconds * 1000.0 {
        unsafe {
            while PeekMessageW(&mut m, None, 0, 0, PM_REMOVE).as_bool() {
                DispatchMessageW(&m);
            }
            v = if v > 0.9 { 0.0 } else { v + 0.11 };
            if let Some(g) = &GFX {
                g.paint(v);
            }
        }
    }
    let t1 = host_ms();
    rec.stop.store(true, Ordering::Relaxed);
    h.join().unwrap();
    let f: Vec<Frame> = rec.snapshot().into_iter().filter(|f| f.diff_prev > 0.0).collect();
    println!(
        "{}",
        json!({"kind": "control", "seconds": seconds, "frames": f.len(), "gaps_ms": gaps(&f), "t0_ms": t0, "t1_ms": t1})
    );
}

// --- standby-list purge, the counterpart of `sudo purge`. Needs an elevated token.
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtSetSystemInformation(class: i32, info: *mut c_void, len: u32) -> i32;
}
fn purge() {
    unsafe {
        let mut tok = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut tok).unwrap();
        let mut luid = LUID::default();
        LookupPrivilegeValueW(None, w!("SeProfileSingleProcessPrivilege"), &mut luid).unwrap();
        let tp = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
        };
        AdjustTokenPrivileges(tok, false, Some(&tp), 0, None, None).unwrap();
        // SystemMemoryListInformation: 3 flushes the modified list, 4 purges standby.
        let mut flush: i32 = 3;
        let a = NtSetSystemInformation(80, &mut flush as *mut _ as *mut c_void, 4);
        let mut standby: i32 = 4;
        let b = NtSetSystemInformation(80, &mut standby as *mut _ as *mut c_void, 4);
        println!("{}", json!({"flush_status": a, "purge_status": b}));
    }
}

fn main() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        timeBeginPeriod(1);
        // Spawned children inherit every inheritable handle, and a launched GitKraken would
        // then hold the driver's pipes open for as long as it runs.
        for h in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            if let Ok(h) = GetStdHandle(h) {
                let _ = SetHandleInformation(h, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0));
            }
        }
    }
    let a: Vec<String> = std::env::args().collect();
    let mut o: HashMap<String, String> = HashMap::new();
    for w in a.windows(2) {
        if let Some(k) = w[0].strip_prefix("--") {
            o.insert(k.into(), w[1].clone());
        }
    }
    let g = |k: &str, d: &str| o.get(k).cloned().unwrap_or_else(|| d.into());
    let f = |k: &str, d: &str| g(k, d).parse::<f64>().unwrap();
    match a.get(1).map_or("", |s| s.as_str()) {
        "watch" => watch(
            rect_arg(&g("rect", "")),
            &g("action", ""),
            f("threshold", "0.002"),
            f("quiet", "1000"),
            f("timeout", "15000"),
            f("min", "0"),
        ),
        "scroll" => scroll(rect_arg(&g("rect", "")), pt_arg(&g("at", "")), f("delta", "-14") as i32, f("ticks", "500") as u32, f("every", "4")),
        "shot" => shot(rect_arg(&g("rect", "0,0,3840,2160")), &g("out", "shot.png")),
        "calibrate" => calibrate(f("n", "40") as usize),
        "control" => control(f("seconds", "5"), rect_arg(&g("rect", "630,240,1425,1350"))),
        "windows" => {
            let v: Vec<Value> = gk_windows()
                .iter()
                .map(|w| json!({"title": w.title, "pid": w.pid, "zoomed": w.zoomed, "rect": [w.rect.left, w.rect.top, w.rect.right, w.rect.bottom]}))
                .collect();
            println!("{}", json!(v));
        }
        "all-windows" => {
            for w in all_windows().iter().filter(|w| !w.title.is_empty()) {
                println!("{} {:?} {},{},{},{} zoomed={}", w.pid, w.title, w.rect.left, w.rect.top, w.rect.right, w.rect.bottom, w.zoomed);
            }
        }
        "activate" => {
            activate();
            sleep_ms(250.0);
            println!("{}", foreground());
        }
        "foreground" => println!("{}", foreground()),
        "place" => {
            // Move GitKraken to the primary display and maximize it there.
            if let Some(w) = main_window() {
                unsafe {
                    let _ = ShowWindow(w.hwnd, SW_RESTORE);
                    let _ = SetWindowPos(w.hwnd, None, 200, 200, 2000, 1400, SWP_NOZORDER);
                    let _ = ShowWindow(w.hwnd, SW_MAXIMIZE);
                }
            }
        }
        "close" => {
            if let Some(w) = main_window() {
                unsafe { PostMessageW(Some(w.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)).unwrap() };
            }
        }
        "pids" => println!("{}", json!(pids_of(&g("name", "gitkraken.exe")))),
        "key" => {
            // e.g. key --vk 0x1B, or key --mod 0x11 --vk 0x57 for ctrl+w
            let vk = u16::from_str_radix(g("vk", "").trim_start_matches("0x"), 16).unwrap();
            let md = o.get("mod").map(|m| u16::from_str_radix(m.trim_start_matches("0x"), 16).unwrap());
            if let Some(m) = md {
                input_key(m, false);
            }
            input_key(vk, false);
            sleep_ms(15.0);
            input_key(vk, true);
            if let Some(m) = md {
                input_key(m, true);
            }
        }
        "modes" => modes(),
        "hz" => set_hz(f("to", "120") as u32),
        "purge" => purge(),
        _ => {
            println!("usage: probe watch|scroll|shot|calibrate|control|windows|activate|place|close|key|modes|hz|purge");
            std::process::exit(2);
        }
    }
    unsafe {
        let _ = timeEndPeriod(1);
    }
}
