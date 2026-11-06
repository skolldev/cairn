//! Platform measurement: presented frames, OS-level key injection, watchdog.
//! The frame sources are lifted from the #9 spike.

pub struct Recording {
    pub intervals_ms: Vec<f64>,
    pub dropped: u64,
    pub source: String,
}

#[derive(Clone, Copy, Debug)]
pub enum Key {
    Backspace,
    Left,
    Char(char),
}

#[cfg(target_os = "macos")]
pub use mac::*;
#[cfg(target_os = "windows")]
pub use win::*;

#[cfg(target_os = "macos")]
mod mac {
    use super::{Key, Recording};
    use block2::RcBlock;
    use std::ffi::{CString, c_char, c_void};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::*};

    type Id = *mut c_void;
    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Id;
        fn class_getInstanceMethod(class: Id, sel: Id) -> Id;
        fn method_getImplementation(method: Id) -> usize;
        fn method_setImplementation(method: Id, imp: usize) -> usize;
        fn objc_msgSend();
    }
    #[link(name = "QuartzCore", kind = "framework")]
    unsafe extern "C" {
        fn CACurrentMediaTime() -> f64;
    }

    static PRESENTED: Mutex<Vec<f64>> = Mutex::new(Vec::new());
    static DROPPED: AtomicU64 = AtomicU64::new(0);
    static ORIG_UPDATE: AtomicUsize = AtomicUsize::new(0);
    static ORIG_LAYER: AtomicUsize = AtomicUsize::new(0);

    fn sel(name: &str) -> Id {
        let c = CString::new(name).unwrap();
        unsafe { sel_registerName(c.as_ptr()) }
    }

    fn watch(drawable: Id) {
        if drawable.is_null() {
            return;
        }
        let block = RcBlock::new(|d: *mut c_void| {
            let get: extern "C" fn(Id, Id) -> f64 =
                unsafe { std::mem::transmute(objc_msgSend as *const ()) };
            let t = get(d, sel("presentedTime"));
            if t > 0.0 {
                PRESENTED.lock().unwrap().push(t);
            } else {
                DROPPED.fetch_add(1, Relaxed);
            }
        });
        let add: extern "C" fn(Id, Id, *const c_void) =
            unsafe { std::mem::transmute(objc_msgSend as *const ()) };
        add(drawable, sel("addPresentedHandler:"), &*block as *const _ as *const c_void);
    }

    extern "C" fn hooked_update(this: Id, s: Id) -> Id {
        let orig: extern "C" fn(Id, Id) -> Id = unsafe { std::mem::transmute(ORIG_UPDATE.load(Relaxed)) };
        let d = orig(this, s);
        watch(d);
        d
    }
    extern "C" fn hooked_layer(this: Id, s: Id) -> Id {
        let orig: extern "C" fn(Id, Id) -> Id = unsafe { std::mem::transmute(ORIG_LAYER.load(Relaxed)) };
        let d = orig(this, s);
        watch(d);
        d
    }

    pub fn install_frame_source() {
        unsafe {
            for (class, name, hook, slot) in [
                (c"CAMetalDisplayLinkUpdate", "drawable", hooked_update as *const () as usize, &ORIG_UPDATE),
                (c"CAMetalLayer", "nextDrawable", hooked_layer as *const () as usize, &ORIG_LAYER),
            ] {
                let m = class_getInstanceMethod(objc_getClass(class.as_ptr()), sel(name));
                assert!(!m.is_null(), "no method {name}");
                slot.store(method_getImplementation(m), Relaxed);
                method_setImplementation(m, hook);
            }
        }
    }

    pub fn begin_recording(_seconds: f64) {
        PRESENTED.lock().unwrap().clear();
        DROPPED.store(0, Relaxed);
    }

    pub fn end_recording() -> Recording {
        let mut t = std::mem::take(&mut *PRESENTED.lock().unwrap());
        t.sort_by(f64::total_cmp);
        Recording {
            intervals_ms: t.windows(2).map(|w| (w[1] - w[0]) * 1000.0).collect(),
            dropped: DROPPED.load(Relaxed),
            source: "Metal presentedTime".into(),
        }
    }

    fn keycode(c: char) -> u16 {
        const MAP: &str = "asdfhgzxcv\0bqweryt";
        if c == ' ' {
            return 49;
        }
        if let Some(i) = MAP.find(c) {
            return i as u16;
        }
        match c {
            'o' => 31,
            'u' => 32,
            'i' => 34,
            'p' => 35,
            'l' => 37,
            'j' => 38,
            'k' => 40,
            'n' => 45,
            'm' => 46,
            _ => 0,
        }
    }

    /// One key event through the real AppKit queue. False if there's no key
    /// window to send it to, which makes the run invalid.
    fn post(key: Key, kind: u64, repeat: bool) -> bool {
        let (unit, code) = match key {
            Key::Backspace => (0x7Fu16, 51),
            Key::Left => (0xF702, 123),
            Key::Char(c) => (c as u16, keycode(c)),
        };
        unsafe {
            type Send0 = extern "C" fn(Id, Id) -> Id;
            let send0: Send0 = std::mem::transmute(objc_msgSend as *const ());
            let app = send0(objc_getClass(c"NSApplication".as_ptr()), sel("sharedApplication"));
            let window = send0(app, sel("keyWindow"));
            if window.is_null() {
                return false;
            }
            let number: extern "C" fn(Id, Id) -> i64 = std::mem::transmute(objc_msgSend as *const ());
            let number = number(window, sel("windowNumber"));
            let string: extern "C" fn(Id, Id, *const u16, u64) -> Id =
                std::mem::transmute(objc_msgSend as *const ());
            let units = [unit];
            let chars = string(
                objc_getClass(c"NSString".as_ptr()),
                sel("stringWithCharacters:length:"),
                units.as_ptr(),
                1,
            );
            let make: extern "C" fn(Id, Id, u64, f64, f64, u64, f64, i64, Id, Id, Id, bool, u16) -> Id =
                std::mem::transmute(objc_msgSend as *const ());
            let event = make(
                objc_getClass(c"NSEvent".as_ptr()),
                sel("keyEventWithType:location:modifierFlags:timestamp:windowNumber:context:characters:charactersIgnoringModifiers:isARepeat:keyCode:"),
                kind,
                0.0,
                0.0,
                0,
                CACurrentMediaTime(),
                number,
                std::ptr::null_mut(),
                chars,
                chars,
                repeat,
                code,
            );
            let post: extern "C" fn(Id, Id, Id, bool) = std::mem::transmute(objc_msgSend as *const ());
            post(app, sel("postEvent:atStart:"), event, false);
        }
        true
    }
    pub fn inject_key_down(key: Key, repeat: bool) -> bool {
        post(key, 10, repeat)
    }
    pub fn inject_key_up(key: Key) {
        post(key, 11, false);
    }

    pub fn claim_focus() -> bool {
        true
    }

    pub fn start_watchdog() {
        eprintln!("watchdog is Windows only, ignored");
    }
}

#[cfg(target_os = "windows")]
mod win {
    use super::{Key, Recording};
    use std::ffi::c_void;
    use std::process::{Child, Command};
    use std::sync::Mutex;

    /// INPUT on x64: the union starts at offset 8, and the whole is 40 bytes.
    #[repr(C)]
    struct KeyInput {
        kind: u32,
        _align: u32,
        vk: u16,
        scan: u16,
        flags: u32,
        time: u32,
        extra: usize,
        pad: u64,
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn SendInput(n: u32, inputs: *const KeyInput, size: i32) -> u32;
        fn GetForegroundWindow() -> *mut c_void;
        fn GetWindowThreadProcessId(hwnd: *mut c_void, pid: *mut u32) -> u32;
        fn RedrawWindow(hwnd: *mut c_void, rect: *const c_void, rgn: *const c_void, flags: u32) -> i32;
        fn GetWindowTextW(hwnd: *mut c_void, text: *mut u16, len: i32) -> i32;
        fn EnumWindows(f: extern "system" fn(*mut c_void, isize) -> i32, arg: isize) -> i32;
        fn IsWindowVisible(hwnd: *mut c_void) -> i32;
        fn SetForegroundWindow(hwnd: *mut c_void) -> i32;
    }
    #[link(name = "winmm")]
    unsafe extern "system" {
        fn timeBeginPeriod(ms: u32) -> u32;
    }

    static SESSION: Mutex<Option<(Child, String)>> = Mutex::new(None);

    /// The default timer tick is too coarse for the 8.33 ms `Frame` drain.
    pub fn install_frame_source() {
        unsafe { timeBeginPeriod(1) };
    }

    /// Set CAIRN_PRESENTMON to the PresentMon console exe. Needs an elevated
    /// shell, or membership of the Performance Log Users group.
    pub fn begin_recording(seconds: f64) {
        let exe = std::env::var("CAIRN_PRESENTMON").expect("set CAIRN_PRESENTMON to PresentMon.exe");
        let csv = format!("{}\\cairn-pm-{}.csv", std::env::temp_dir().display(), std::process::id());
        let _ = std::fs::remove_file(&csv);
        let child = Command::new(exe)
            .args(["--process_id", &std::process::id().to_string()])
            .args(["--output_file", &csv, "--no_console_stats", "--stop_existing_session"])
            .args(["--terminate_after_timed", "--timed", &format!("{}", seconds.ceil() as u64)])
            .spawn()
            .expect("start PresentMon");
        *SESSION.lock().unwrap() = Some((child, csv));
    }

    pub fn end_recording() -> Recording {
        let (mut child, csv) = SESSION.lock().unwrap().take().expect("no session");
        let _ = child.wait();
        let text = std::fs::read_to_string(&csv).unwrap_or_default();
        let mut lines = text.lines();
        let header: Vec<String> =
            lines.next().unwrap_or("").split(',').map(|h| h.trim().to_lowercase()).collect();
        let (col, name) = ["msbetweendisplaychange", "displayedtime", "msbetweenpresents"]
            .iter()
            .find_map(|n| header.iter().position(|h| h == n).map(|c| (c, *n)))
            .expect("no interval column in the PresentMon CSV");
        let mut intervals_ms = vec![];
        let mut dropped = 0;
        for line in lines {
            match line.split(',').nth(col).and_then(|v| v.parse::<f64>().ok()) {
                Some(v) if v > 0.0 => intervals_ms.push(v),
                _ => dropped += 1,
            }
        }
        Recording { intervals_ms, dropped, source: format!("PresentMon {name}") }
    }

    fn vk(key: Key) -> u16 {
        match key {
            Key::Backspace => 0x08,
            Key::Left => 0x25,
            Key::Char(' ') => 0x20,
            // VK codes follow the layout, so z stays z on QWERTZ.
            Key::Char(c) => c.to_ascii_uppercase() as u16,
        }
    }
    /// SendInput goes to whatever has focus, so never fire it at another app. False
    /// makes the run invalid, like the macOS key window check.
    fn ours_in_front() -> bool {
        let mut pid = 0;
        let hwnd = unsafe { GetForegroundWindow() };
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        let ours = pid == std::process::id();
        if !ours {
            let mut title = [0u16; 128];
            let n = unsafe { GetWindowTextW(hwnd, title.as_mut_ptr(), 128) };
            eprintln!("focus lost to pid {pid}: {}", String::from_utf16_lossy(&title[..n.max(0) as usize]));
        }
        ours
    }
    fn send(key: Key, flags: u32) -> bool {
        let k = KeyInput {
            kind: 1,
            _align: 0,
            vk: vk(key),
            scan: 0,
            flags,
            time: 0,
            extra: 0,
            pad: 0,
        };
        unsafe { SendInput(1, &k, std::mem::size_of::<KeyInput>() as i32) == 1 }
    }
    pub fn inject_key_down(key: Key, _repeat: bool) -> bool {
        ours_in_front() && send(key, 0)
    }

    extern "system" fn find_ours(hwnd: *mut c_void, out: isize) -> i32 {
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid == std::process::id() && unsafe { IsWindowVisible(hwnd) } != 0 {
            unsafe { *(out as *mut *mut c_void) = hwnd };
            return 0;
        }
        1
    }

    /// A display mode change leaves the taskbar in front, and Windows won't let a
    /// process launched in the background take over unless it has just had input.
    /// A lone Alt tap is the usual way to satisfy that.
    pub fn claim_focus() -> bool {
        if ours_in_front() {
            return true;
        }
        let mut hwnd: *mut c_void = std::ptr::null_mut();
        unsafe { EnumWindows(find_ours, &mut hwnd as *mut _ as isize) };
        if hwnd.is_null() {
            return false;
        }
        let alt = |flags| {
            let k = KeyInput { kind: 1, _align: 0, vk: 0x12, scan: 0, flags, time: 0, extra: 0, pad: 0 };
            unsafe { SendInput(1, &k, std::mem::size_of::<KeyInput>() as i32) };
        };
        alt(0);
        alt(2);
        unsafe { SetForegroundWindow(hwnd) };
        ours_in_front()
    }
    pub fn inject_key_up(key: Key) {
        send(key, 2);
    }

    /// The workaround reported on zed#61469: RDW_INVALIDATE | RDW_UPDATENOW
    /// every 8 ms from a thread of its own. Call while our window is in front.
    pub fn start_watchdog() {
        let hwnd = unsafe { GetForegroundWindow() } as usize;
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd as *mut c_void, &mut pid) };
        assert_eq!(pid, std::process::id(), "watchdog: our window isn't in front");
        std::thread::spawn(move || {
            loop {
                unsafe { RedrawWindow(hwnd as *mut c_void, std::ptr::null(), std::ptr::null(), 0x1 | 0x100) };
                std::thread::sleep(std::time::Duration::from_millis(8));
            }
        });
    }
}
