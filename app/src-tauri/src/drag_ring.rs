//! Ctrl+drag: hold Ctrl while dragging files and the ring pops up under the
//! cursor; drop the files on a slice and that slice's preset converts them.
//!
//! This module owns the ring's side of it: opening the ring empty while the
//! drag is still in flight, turning the window's drag-and-drop events into
//! hover and drop events for the frontend, and tidying up when the drop lands
//! somewhere else. Noticing the gesture itself is the platform watcher's job
//! (`gesture_linux.rs`).

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, DragDropEvent, Emitter, Manager, Runtime, Window, WindowEvent};

use crate::commands::RING_WINDOW;

/// The ring opened by the gesture, still empty: "drop something on me".
pub const RING_ARM: &str = "ring-arm";
/// The drag reached the ring; payload is the ring model for the dragged files.
pub const RING_DRAG_ENTER: &str = "ring-drag-enter";
/// The drag moved over the ring; payload is a [`DragPoint`].
pub const RING_DRAG_OVER: &str = "ring-drag-over";
/// The drag left the ring window.
pub const RING_DRAG_LEAVE: &str = "ring-drag-leave";
/// The files were dropped on the ring; payload is a [`DragPoint`].
pub const RING_DRAG_DROP: &str = "ring-drag-drop";
/// The drag ended somewhere else; the ring should go away.
pub const RING_DRAG_DISMISS: &str = "ring-drag-dismiss";

/// The ring window's title, which Hyprland matches when moving it.
#[cfg(target_os = "linux")]
const RING_TITLE: &str = "MorphRing ring";
/// The ring window is this many logical pixels square (see tauri.conf.json).
const RING_SIZE: i32 = 360;
/// After the mouse button comes up, how long a drop gets to arrive before we
/// decide the files landed elsewhere. Drops arrive within a frame or two; the
/// rest is slack for a busy machine.
const DROP_GRACE: Duration = Duration::from_millis(700);

/// A point inside the ring window, in CSS pixels.
#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct DragPoint {
    pub x: f64,
    pub y: f64,
}

/// Set while a gesture-opened ring waits for its drop.
static ARMED: AtomicBool = AtomicBool::new(false);
/// The files of the drag currently over the ring, from its enter event.
static DRAGGED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Whether the gesture watcher is running, so the app can stay out of sight.
#[derive(Debug, Default)]
pub struct GestureActive(pub AtomicBool);

/// True when the Ctrl+drag watcher started, i.e. the app may live in the tray.
pub fn gesture_active<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.try_state::<GestureActive>()
        .is_some_and(|g| g.0.load(Ordering::Relaxed))
}

/// Opens the empty ring centred on `cursor` (global layout coordinates), or
/// in the middle of the screen when we don't know where the cursor is.
pub fn arm<R: Runtime>(app: &AppHandle<R>, cursor: Option<(i32, i32)>) {
    let Some(ring) = app.get_webview_window(RING_WINDOW) else {
        return;
    };
    tracing::debug!(?cursor, "arming the ring");
    ARMED.store(true, Ordering::SeqCst);
    DRAGGED.lock().unwrap_or_else(|e| e.into_inner()).clear();
    if let Err(e) = ring.emit_to(RING_WINDOW, RING_ARM, ()) {
        tracing::warn!("could not arm the ring: {e}");
    }
    if cursor.is_none() {
        let _ = ring.center();
    }
    // No focus on purpose: the file manager is mid-drag, and yanking focus
    // away from it is a great way to find out how robust its DnD code is.
    if let Err(e) = ring.show() {
        tracing::warn!("could not show the ring: {e}");
        return;
    }
    if let Some((x, y)) = cursor {
        // Put the hole right under the cursor.
        place(&ring, x - RING_SIZE / 2, y - RING_SIZE / 2);
    }
}

/// Puts the ring's top-left corner at `x`, `y` (global layout coordinates).
///
/// On Linux, Wayland won't let a window place itself, so we ask Hyprland to do
/// it; a window can only be moved once it is mapped, so the compositor gets a
/// moment to map it first.
#[cfg(target_os = "linux")]
fn place<R: Runtime>(_ring: &tauri::WebviewWindow<R>, x: i32, y: i32) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        crate::hyprland::move_window(RING_TITLE, x, y);
    });
}

/// Puts the ring's top-left corner at `x`, `y` (logical screen coordinates).
///
/// Windows and macOS let a window move itself, no compositor diplomacy needed.
/// Nothing calls this today (the gesture watcher is Linux-only), but the day a
/// Windows hook or a macOS event tap arms the ring, it already lands right.
#[cfg(not(target_os = "linux"))]
fn place<R: Runtime>(ring: &tauri::WebviewWindow<R>, x: i32, y: i32) {
    if let Err(e) = ring.set_position(tauri::LogicalPosition::new(x, y)) {
        tracing::debug!("could not move the ring, centring it: {e}");
        let _ = ring.center();
    }
}

/// The mouse button came up. If no drop reaches the ring soon, the files
/// went somewhere else and the empty ring has no reason to stay.
pub fn released<R: Runtime>(app: &AppHandle<R>) {
    if !ARMED.load(Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(DROP_GRACE);
        if ARMED.swap(false, Ordering::SeqCst) {
            tracing::debug!("drag ended away from the ring");
            let _ = app.emit_to(RING_WINDOW, RING_DRAG_DISMISS, ());
        }
    });
}

fn point<R: Runtime>(window: &Window<R>, position: &tauri::PhysicalPosition<f64>) -> DragPoint {
    let scale = window.scale_factor().unwrap_or(1.0);
    DragPoint {
        x: position.x / scale,
        y: position.y / scale,
    }
}

/// Window event hook for the ring window's drag and drop.
pub fn on_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    if window.label() != RING_WINDOW || !ARMED.load(Ordering::SeqCst) {
        return;
    }
    let WindowEvent::DragDrop(drag) = event else {
        return;
    };
    let app = window.app_handle();
    match drag {
        DragDropEvent::Enter { paths, position } => {
            let files = crate::dropzone::accept_drop(paths).files;
            *DRAGGED.lock().unwrap_or_else(|e| e.into_inner()) = files.clone();
            let at = point(window, position);
            let app = app.clone();
            // The ring model may wait for engine probes on first use; keep that
            // off the event loop, which is busy delivering this very drag.
            tauri::async_runtime::spawn_blocking(move || {
                let model = crate::commands::ring_model_for(&app, &files);
                let _ = app.emit_to(RING_WINDOW, RING_DRAG_ENTER, model);
                let _ = app.emit_to(RING_WINDOW, RING_DRAG_OVER, at);
            });
        }
        DragDropEvent::Over { position } => {
            let _ = app.emit_to(RING_WINDOW, RING_DRAG_OVER, point(window, position));
        }
        DragDropEvent::Leave => {
            let _ = app.emit_to(RING_WINDOW, RING_DRAG_LEAVE, ());
        }
        DragDropEvent::Drop { paths, position } => {
            ARMED.store(false, Ordering::SeqCst);
            let entered = std::mem::take(&mut *DRAGGED.lock().unwrap_or_else(|e| e.into_inner()));
            if entered.is_empty() {
                // Some toolkits only hand over the paths at the drop. Then the
                // slices were never shown, so show the normal click ring now.
                let _ = app.emit_to(RING_WINDOW, RING_DRAG_DISMISS, ());
                crate::commands::open_ring(app, crate::dropzone::accept_drop(paths).files);
                return;
            }
            let _ = app.emit_to(RING_WINDOW, RING_DRAG_DROP, point(window, position));
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_point_is_camel_free_json() {
        let json = serde_json::to_value(DragPoint { x: 1.5, y: 2.0 }).unwrap();
        assert_eq!(json, serde_json::json!({ "x": 1.5, "y": 2.0 }));
    }
}
