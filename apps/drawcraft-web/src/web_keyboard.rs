//! Magic Keyboard and trackpad hygiene for Safari.
//!
//! The app handles shortcuts itself (egui). This module only stops the browser from stealing
//! the same key combinations (find-on-page, page zoom, print, save, ...) and from zooming the
//! whole page on a trackpad pinch, so the canvas receives them instead.

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

/// Letters that, combined with Cmd (iPadOS/macOS) or Ctrl (Windows/Linux), belong to the app:
/// Undo/Redo (`Z`, `Y`), Command palette (`K`), Find (`F`), New (`N`), Open (`O`), Save/Export (`S`),
/// Print is intentionally *not* captured so printing still works.
const APP_LETTERS: &[&str] = &["z", "y", "k", "f", "n", "o", "s", "d", "c", "x", "v"];
/// Zoom digits: Cmd+0 fits the page width, Cmd+9 fits the whole page.
const APP_DIGITS: &[&str] = &["0", "9"];

/// Whether a key combination should be kept away from the browser.
///
/// `key` is `KeyboardEvent.key`. Browser-level shortcuts that must keep working for accessibility
/// and escape routes (`Cmd+R` reload, `Cmd+L` address bar, `Cmd+T/W` tabs, `Cmd+Q`) are never listed.
#[must_use]
pub fn browser_should_not_see(key: &str, command: bool, alt: bool) -> bool {
    if !command || alt {
        return false;
    }
    let lower = key.to_ascii_lowercase();
    APP_LETTERS.contains(&lower.as_str()) || APP_DIGITS.contains(&lower.as_str())
}

/// Installs `keydown`, `wheel` and Safari `gesture*` listeners on the window.
pub fn install() {
    let Some(window) = web_sys::window() else {
        return;
    };

    let keydown = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::wrap(Box::new(
        move |ev: web_sys::KeyboardEvent| {
            let command = ev.meta_key() || ev.ctrl_key();
            if browser_should_not_see(&ev.key(), command, ev.alt_key()) {
                ev.prevent_default();
            }
        },
    ));
    let options = web_sys::AddEventListenerOptions::new();
    options.set_capture(true);
    options.set_passive(false);
    let _ = window.add_event_listener_with_callback_and_add_event_listener_options(
        "keydown",
        keydown.as_ref().unchecked_ref(),
        &options,
    );
    keydown.forget();

    // Trackpad pinch arrives as a wheel event with ctrlKey set: keep the browser from zooming
    // the page; the canvas zooms around the pointer instead.
    let wheel = Closure::<dyn FnMut(web_sys::WheelEvent)>::wrap(Box::new(
        move |ev: web_sys::WheelEvent| {
            if ev.ctrl_key() {
                ev.prevent_default();
            }
        },
    ));
    let _ = window.add_event_listener_with_callback_and_add_event_listener_options(
        "wheel",
        wheel.as_ref().unchecked_ref(),
        &options,
    );
    wheel.forget();

    // Safari (macOS and iPadOS with a trackpad) also fires non-standard gesture events for a
    // pinch; cancelling them stops the page-wide zoom.
    for name in ["gesturestart", "gesturechange", "gestureend"] {
        let cb = Closure::<dyn FnMut(web_sys::Event)>::wrap(Box::new(move |ev: web_sys::Event| {
            ev.prevent_default();
        }));
        let _ = window.add_event_listener_with_callback_and_add_event_listener_options(
            name,
            cb.as_ref().unchecked_ref(),
            &options,
        );
        cb.forget();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_shortcuts_with_the_command_key_are_kept_from_the_browser() {
        for key in ["z", "Z", "k", "f", "0", "9", "n"] {
            assert!(browser_should_not_see(key, true, false), "{key}");
        }
    }

    #[test]
    fn plain_keys_and_alt_combinations_are_left_alone() {
        assert!(!browser_should_not_see("k", false, false));
        assert!(!browser_should_not_see("z", true, true));
        assert!(!browser_should_not_see("Escape", true, false));
    }

    #[test]
    fn escape_routes_stay_available() {
        for key in ["r", "l", "t", "w", "q", "Tab", "p"] {
            assert!(
                !browser_should_not_see(key, true, false),
                "{key} must keep working"
            );
        }
    }
}
