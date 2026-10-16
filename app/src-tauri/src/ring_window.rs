//! Showing the ring window. Kept apart from the commands so the platform code
//! can change *where* the ring appears without touching the conversion queue.
//!
//! Closing the ring hides the window (see `commands::ring_close`); it is never
//! left on screen as a click-through overlay. That is on purpose: on Windows a
//! transparent window with ignored cursor events still eats clicks now and
//! then (Tauri issue #11461), and a hidden window cannot eat anything.

use tauri::{Runtime, WebviewWindow};

/// One thing [`show`] does to the ring window, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Put the ring in the middle of the screen.
    Center,
    /// Make the window visible. Nothing after this runs if it fails.
    Show,
    /// Drop the topmost flag and set it again, so the window really goes back
    /// to the top of the z-order.
    RaiseTopmost,
    /// Hand the keyboard focus to the ring.
    Focus,
}

/// Linux: centre, show, focus. Exactly what the ring has always done.
const STEPS_DEFAULT: &[Step] = &[Step::Center, Step::Show, Step::Focus];

/// Windows: centre, show, re-assert topmost, and keep the hands off the focus.
const STEPS_WINDOWS: &[Step] = &[Step::Center, Step::Show, Step::RaiseTopmost];

/// The steps for the platform this binary was built for.
fn steps() -> &'static [Step] {
    if cfg!(windows) {
        STEPS_WINDOWS
    } else {
        STEPS_DEFAULT
    }
}

/// Centres the ring window and shows it. On Linux it also takes the focus; on
/// Windows it is pushed back on top instead and leaves the focus alone.
///
/// Failures are logged, not returned: a ring that could not be centred is
/// still a perfectly good ring.
pub fn show<R: Runtime>(ring: &WebviewWindow<R>) {
    for step in steps() {
        match step {
            Step::Center => {
                // Filee opens the ring under the cursor. On Wayland nobody gets
                // to know where the cursor is, so the ring sits in the middle.
                if let Err(e) = ring.center() {
                    tracing::debug!("could not center the ring: {e}");
                }
            }
            Step::Show => {
                if let Err(e) = ring.show() {
                    tracing::warn!("could not show the ring: {e}");
                    return;
                }
            }
            Step::RaiseTopmost => raise_topmost(ring),
            Step::Focus => {
                if let Err(e) = ring.set_focus() {
                    tracing::debug!("could not focus the ring: {e}");
                }
            }
        }
    }
}

/// Toggles always-on-top off and on again.
///
/// Windows can quietly shove a topmost window under normal ones (a fullscreen
/// app or a UAC prompt is enough) while it keeps wearing the topmost badge, and
/// showing it again does not move it in the z-order. Explorer then sits on top
/// of the ring and the drop never lands. Filee does the same dance on every
/// show (`RadialController.cs:301-305`); the off-and-on is the only way to get
/// Windows to actually re-read the flag through Tauri.
///
/// No `set_focus` on Windows: the user is in the middle of a drag from
/// Explorer, and stealing the foreground there is how drags die. The window is
/// configured with `focus: false`.
///
/// TODO(v0.4): Tauri's `show()` may still activate the window on Windows. If
/// the Windows spike (docs/research/windows-support.md section 5) catches that,
/// switch to `ShowWindow(hwnd, SW_SHOWNOACTIVATE)` plus `WS_EX_NOACTIVATE`
/// through `window.hwnd()` with the `windows` crate. Tauri alone cannot ask
/// for a non-activating show.
fn raise_topmost<R: Runtime>(ring: &WebviewWindow<R>) {
    if let Err(e) = ring.set_always_on_top(false) {
        tracing::debug!("could not clear always-on-top on the ring: {e}");
    }
    if let Err(e) = ring.set_always_on_top(true) {
        tracing::warn!("could not keep the ring on top: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_and_friends_keep_the_old_sequence() {
        assert_eq!(STEPS_DEFAULT, &[Step::Center, Step::Show, Step::Focus]);
    }

    #[test]
    fn windows_raises_topmost_after_show_and_never_focuses() {
        let show = STEPS_WINDOWS.iter().position(|s| *s == Step::Show);
        let raise = STEPS_WINDOWS.iter().position(|s| *s == Step::RaiseTopmost);
        assert!(matches!((show, raise), (Some(s), Some(r)) if s < r));
        assert!(!STEPS_WINDOWS.contains(&Step::Focus));
    }

    #[test]
    fn steps_match_the_target() {
        if cfg!(windows) {
            assert_eq!(steps(), STEPS_WINDOWS);
        } else {
            assert_eq!(steps(), STEPS_DEFAULT);
        }
    }

    #[test]
    fn every_platform_centres_before_showing() {
        for steps in [STEPS_DEFAULT, STEPS_WINDOWS] {
            assert_eq!(&steps[..2], &[Step::Center, Step::Show]);
        }
    }
}
