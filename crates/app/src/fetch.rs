//! The Fetch control: a toolbar button and a keyboard shortcut (Cmd+Shift+F on macOS,
//! Ctrl+Shift+F elsewhere) that send `Intent::Fetch` while a repository is open, and do
//! nothing while none is.

#[cfg(test)]
mod tests;

use gpui::{ClickEvent, Div, KeyBinding, Stateful, Styled, Window, actions, div, prelude::*, rgb};

actions!(cairn, [Fetch]);

#[cfg(target_os = "macos")]
const SHORTCUT: &str = "cmd-shift-f";
#[cfg(not(target_os = "macos"))]
const SHORTCUT: &str = "ctrl-shift-f";

/// Binds the shortcut, app-wide: the window handles [`Fetch`] wherever focus is.
pub fn bind_keys(cx: &mut gpui::App) {
    cx.bind_keys([KeyBinding::new(SHORTCUT, Fetch, None)]);
}

/// The toolbar's Fetch button. `on_fetch` runs on a click, and only while `enabled`.
pub fn button(
    enabled: bool,
    on_fetch: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> Stateful<Div> {
    let button = div()
        .id("fetch")
        .debug_selector(|| "toolbar:fetch".to_owned())
        .px_2()
        .py_1()
        .rounded_sm()
        .bg(rgb(0x2a2f37))
        .child("Fetch");
    if enabled {
        button.cursor_pointer().on_click(on_fetch)
    } else {
        button.text_color(rgb(0x7d8590))
    }
}
