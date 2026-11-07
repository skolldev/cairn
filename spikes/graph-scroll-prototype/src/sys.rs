//! Platform measurement: presented frames, memory, OS-level key injection.

pub struct Recording {
    pub intervals_ms: Vec<f64>,
    pub dropped: u64,
    pub source: String,
}

#[cfg(target_os = "macos")]
pub use mac::*;
#[cfg(target_os = "windows")]
pub use win::*;

#[cfg(target_os = "macos")]
mod mac {
    use super::Recording;
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
    unsafe extern "C" {
        fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut u64) -> i32;
    }

    static PRESENTED: Mutex<Vec<f64>> = Mutex::new(Vec::new());
    static DROPPED: AtomicU64 = AtomicU64::new(0);
    static SEEN: AtomicU64 = AtomicU64::new(0);
    static ORIG_UPDATE: AtomicUsize = AtomicUsize::new(0);
    static ORIG_LAYER: AtomicUsize = AtomicUsize::new(0);

    fn sel(name: &str) -> Id {
        let c = CString::new(name).unwrap();
        unsafe { sel_registerName(c.as_ptr()) }
    }

    /// Attach Metal's presented-time handler to a drawable. GPUI doesn't expose
    /// it, so the two drawable getters are hooked. GPUI itself stays stock.
    fn watch(drawable: Id) {
        if drawable.is_null() {
            return;
        }
        SEEN.fetch_add(1, Relaxed);
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

    /// Unix ms of the first presented frame, once one has landed.
    pub fn first_present_unix_ms() -> Option<f64> {
        let first = *PRESENTED.lock().unwrap().first()?;
        Some(super::unix_ms() - (unsafe { CACurrentMediaTime() } - first) * 1000.0)
    }

    /// (current phys_footprint, lifetime peak) in bytes.
    pub fn memory() -> (u64, u64) {
        let mut b = [0u64; 64];
        // RUSAGE_INFO_V4: after a 16-byte uuid, phys_footprint is field 9 and
        // ri_lifetime_max_phys_footprint is field 30, counted in u64s.
        let rc = unsafe { proc_pid_rusage(std::process::id() as i32, 4, b.as_mut_ptr()) };
        assert_eq!(rc, 0);
        (b[9], b[30])
    }

    /// One Down-arrow event through the real AppKit queue. CGEventPostToPid
    /// never arrived in testing, so this posts an NSEvent to NSApp instead.
    fn post_key(kind: u64, repeat: bool) {
        unsafe {
            type Send0 = extern "C" fn(Id, Id) -> Id;
            let send0: Send0 = std::mem::transmute(objc_msgSend as *const ());
            let app = send0(objc_getClass(c"NSApplication".as_ptr()), sel("sharedApplication"));
            let window = send0(app, sel("keyWindow"));
            if window.is_null() {
                return;
            }
            let number: extern "C" fn(Id, Id) -> i64 = std::mem::transmute(objc_msgSend as *const ());
            let number = number(window, sel("windowNumber"));
            let string: extern "C" fn(Id, Id, *const u16, u64) -> Id =
                std::mem::transmute(objc_msgSend as *const ());
            let down_arrow = [0xF701u16];
            let chars = string(
                objc_getClass(c"NSString".as_ptr()),
                sel("stringWithCharacters:length:"),
                down_arrow.as_ptr(),
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
                125,
            );
            let post: extern "C" fn(Id, Id, Id, bool) = std::mem::transmute(objc_msgSend as *const ());
            post(app, sel("postEvent:atStart:"), event, false);
        }
    }
    pub fn inject_key_down(repeat: bool) {
        post_key(10, repeat);
    }
    pub fn inject_key_up() {
        post_key(11, false);
    }
}

#[cfg(target_os = "windows")]
mod win {
    //! UNTESTED: written on a Mac. Expect to fix this on the first Windows run.
    use super::Recording;
    use std::process::{Child, Command};
    use std::sync::Mutex;

    #[repr(C)]
    #[derive(Default)]
    struct MemCounters {
        cb: u32,
        faults: u32,
        peak_ws: usize,
        ws: usize,
        q: [usize; 4],
        pagefile: usize,
        peak_pagefile: usize,
        private: usize,
        private_ws: usize,
        shared_commit: u64,
    }
    /// INPUT with KEYBDINPUT. The union is 8-aligned on x64, so `_align` keeps
    /// `vk` at offset 8 and the total at 40 bytes, or SendInput rejects it.
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
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(p: isize, c: *mut MemCounters, cb: u32) -> i32;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn SendInput(n: u32, inputs: *const KeyInput, size: i32) -> u32;
    }

    static PEAK: Mutex<u64> = Mutex::new(0);
    static SESSION: Mutex<Option<(Child, String)>> = Mutex::new(None);

    pub fn install_frame_source() {}

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
        let (col, name) = ["msbetweendisplaychange", "msbetweenpresents"]
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

    /// In-process stand-in: PresentMon can't see this early from here. Read
    /// launch results on Windows as "second frame requested", not as presented.
    pub fn first_present_unix_ms() -> Option<f64> {
        Some(super::unix_ms())
    }

    pub fn memory() -> (u64, u64) {
        let mut c = MemCounters { cb: std::mem::size_of::<MemCounters>() as u32, ..Default::default() };
        unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
        let now = c.private_ws as u64;
        let mut peak = PEAK.lock().unwrap();
        *peak = (*peak).max(now);
        (now, *peak)
    }

    fn send(flags: u32) {
        let k = KeyInput { kind: 1, _align: 0, vk: 0x28, scan: 0, flags, time: 0, extra: 0, pad: 0 };
        unsafe { SendInput(1, &k, std::mem::size_of::<KeyInput>() as i32) };
    }
    pub fn inject_key_down(_repeat: bool) {
        send(0);
    }
    pub fn inject_key_up() {
        send(2);
    }
}

pub fn unix_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64() * 1000.0
}
