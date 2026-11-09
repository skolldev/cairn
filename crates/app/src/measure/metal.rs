//! Metal's presented times. GPUI doesn't expose them, so the two getters that hand it drawables
//! are hooked at runtime, as #9's spike did, and each drawable reports when it reached the
//! screen. Only in the `measure` build, and GPUI itself stays stock.

use std::ffi::{CString, c_char, c_void};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};

use block2::RcBlock;
use cairn_budget::clock;
use cairn_budget::presented::DrawableSource;
use cairn_budget::probe::Line;

use super::write_line;

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

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static ORIGINAL_LINK: AtomicUsize = AtomicUsize::new(0);
static ORIGINAL_LAYER: AtomicUsize = AtomicUsize::new(0);

fn sel(name: &str) -> Id {
    let name = CString::new(name).expect("a selector has no NUL");
    // SAFETY: a valid C string, which the runtime copies.
    unsafe { sel_registerName(name.as_ptr()) }
}

/// Reports `drawable`, and asks Metal to report when it reaches the screen.
fn watch(drawable: Id, source: DrawableSource) {
    if drawable.is_null() {
        return;
    }
    let id = NEXT_ID.fetch_add(1, Relaxed);
    write_line(Line::Drawable {
        id,
        source,
        ns: clock::now_ns(),
    });
    let handler = RcBlock::new(move |presented: *mut c_void| {
        // SAFETY: Metal calls this with the drawable, which has `presentedTime`, a double.
        let time: extern "C" fn(Id, Id) -> f64 =
            unsafe { std::mem::transmute(objc_msgSend as *const ()) };
        let seconds = time(presented, sel("presentedTime"));
        if seconds > 0.0 {
            write_line(Line::Presented {
                id,
                ns: (seconds * 1e9) as u64,
            });
        } else {
            write_line(Line::Dropped { id });
        }
    });
    // SAFETY: `addPresentedHandler:` takes one block, which it copies.
    let add: extern "C" fn(Id, Id, *const c_void) =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    add(
        drawable,
        sel("addPresentedHandler:"),
        &*handler as *const _ as *const c_void,
    );
}

extern "C" fn hooked_link(this: Id, selector: Id) -> Id {
    // SAFETY: `install` stored this method's own original implementation before hooking it.
    let original: extern "C" fn(Id, Id) -> Id =
        unsafe { std::mem::transmute(ORIGINAL_LINK.load(Relaxed)) };
    let drawable = original(this, selector);
    watch(drawable, DrawableSource::DisplayLink);
    drawable
}

extern "C" fn hooked_layer(this: Id, selector: Id) -> Id {
    // SAFETY: as in `hooked_link`.
    let original: extern "C" fn(Id, Id) -> Id =
        unsafe { std::mem::transmute(ORIGINAL_LAYER.load(Relaxed)) };
    let drawable = original(this, selector);
    watch(drawable, DrawableSource::Layer);
    drawable
}

pub fn install() {
    let hooks = [
        (
            c"CAMetalDisplayLinkUpdate",
            "drawable",
            hooked_link as *const () as usize,
            &ORIGINAL_LINK,
        ),
        (
            c"CAMetalLayer",
            "nextDrawable",
            hooked_layer as *const () as usize,
            &ORIGINAL_LAYER,
        ),
    ];
    for (class, method, hook, original) in hooks {
        // SAFETY: both hooks have the getter's own signature, and each stores the original before
        // it can be called. A class that this macOS lacks (the display link before 14) gets none.
        unsafe {
            let class = objc_getClass(class.as_ptr());
            if class.is_null() {
                continue;
            }
            let method = class_getInstanceMethod(class, sel(method));
            if !method.is_null() {
                original.store(method_getImplementation(method), Relaxed);
                method_setImplementation(method, hook);
            }
        }
    }
}
