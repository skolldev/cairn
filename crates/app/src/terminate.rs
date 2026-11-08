//! macOS only: an app-level quit takes the same path as closing the window.
//!
//! The Dock's Quit and logging out ask `NSApplication` to terminate, and GPUI lets that
//! through: its delegate has no `applicationShouldTerminate:`, and its quit handlers get only
//! a moment. So this adds that method to GPUI's delegate class at run time. It refuses each
//! terminate and starts the close flow instead, until the Repo service has quit.
//!
//! `GPUIApplicationDelegate` is a private name, kept stable by the exact `gpui-pre` pin. Check
//! it after an upgrade: if it is gone, launching logs an error. GPUI's test platform has no
//! such class, so this is checked by hand (docs/pins.md).

use std::ffi::CStr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{App, Entity, Window};
use objc2::ffi::class_addMethod;
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::sel;

use crate::quit::QuitView;

const DELEGATE_CLASS: &CStr = c"GPUIApplicationDelegate";
/// `applicationShouldTerminate:`'s type: returns an `NSUInteger`, takes `self`, `_cmd`, sender.
const METHOD_TYPES: &CStr = c"Q@:@";
const NS_TERMINATE_CANCEL: usize = 0;
const NS_TERMINATE_NOW: usize = 1;

static EXIT_ALLOWED: AtomicBool = AtomicBool::new(false);
static CLOSE_REQUESTS: OnceLock<async_channel::Sender<()>> = OnceLock::new();

/// From here on, terminating goes ahead: the app is exiting on purpose.
pub fn allow_exit() {
    EXIT_ALLOWED.store(true, Ordering::Release);
}

extern "C-unwind" fn should_terminate(_: &AnyObject, _: Sel, _: *mut AnyObject) -> usize {
    if EXIT_ALLOWED.load(Ordering::Acquire) {
        return NS_TERMINATE_NOW;
    }
    if let Some(requests) = CLOSE_REQUESTS.get() {
        let _ = requests.try_send(());
    }
    NS_TERMINATE_CANCEL
}

/// Sends each app-level quit to `quit` as a close request.
pub fn route_to_close_flow(quit: Entity<QuitView>, window: &mut Window, cx: &mut App) {
    let Some(delegate) = AnyClass::get(DELEGATE_CLASS) else {
        tracing::error!("GPUI's app delegate is missing, so an app-level quit won't wait for git");
        return;
    };
    // A bounded channel of one: quits that arrive before the first is handled coalesce.
    let (requests, requested) = async_channel::bounded(1);
    if CLOSE_REQUESTS.set(requests).is_err() {
        return;
    }
    // SAFETY: an `Imp` is called with the arguments `METHOD_TYPES` declares, which are those
    // `should_terminate` takes, and the class and strings outlive the call.
    let added = unsafe {
        let imp = std::mem::transmute::<
            extern "C-unwind" fn(&AnyObject, Sel, *mut AnyObject) -> usize,
            Imp,
        >(should_terminate);
        class_addMethod(
            std::ptr::from_ref(delegate).cast_mut(),
            sel!(applicationShouldTerminate:),
            imp,
            METHOD_TYPES.as_ptr(),
        )
    };
    if !added.as_bool() {
        tracing::error!("GPUI's app delegate already decides on termination; not replacing it");
        return;
    }
    window
        .spawn(cx, async move |cx| {
            while requested.recv().await.is_ok() {
                let _ = cx.update(|window, cx| {
                    quit.update(cx, |view, cx| view.close_requested(window, cx));
                });
            }
        })
        .detach();
}
