//! Showing the ring window. Kept apart from the commands so the platform code
//! can change *where* the ring appears without touching the conversion queue.

use tauri::{Runtime, WebviewWindow};

/// Centres the ring window, shows it and gives it the focus.
///
/// Failures are logged, not returned: a ring that could not be centred is
/// still a perfectly good ring.
pub fn show<R: Runtime>(ring: &WebviewWindow<R>) {
    // Filee opens the ring under the cursor. On Wayland nobody gets to know
    // where the cursor is, so the ring sits in the middle of the screen.
    if let Err(e) = ring.center() {
        tracing::debug!("could not center the ring: {e}");
    }
    if let Err(e) = ring.show() {
        tracing::warn!("could not show the ring: {e}");
        return;
    }
    if let Err(e) = ring.set_focus() {
        tracing::debug!("could not focus the ring: {e}");
    }
}
