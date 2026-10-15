//! Commands the webviews can invoke. T35 fills in `convert` and the ring model;
//! the names are already registered in `lib.rs`, so only this file changes.

use tauri::{AppHandle, Manager, Runtime};

/// Label of the hidden window that shows the conversion ring.
pub const RING_WINDOW: &str = "ring";

/// Health check for the frontend: answers `"pong"`.
#[tauri::command]
pub fn ping() -> &'static str {
    "pong"
}

/// Converts `files` to the format `to`.
#[tauri::command]
pub fn convert(files: Vec<String>, to: String) -> Result<(), String> {
    // Placeholder until T35 wires the job runner in. Saying no politely beats
    // pretending we converted something.
    tracing::info!(count = files.len(), %to, "convert requested");
    Err(
        "Converting from the app is not available yet. Use the morphring command line for now."
            .into(),
    )
}

/// Hides the ring window. Does nothing when the ring is not open.
#[tauri::command]
pub fn ring_close<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    // The ring is created once and reused, so "close" means hide; tearing the
    // webview down every time would make the next open visibly slower.
    match app.get_webview_window(RING_WINDOW) {
        Some(ring) => ring
            .hide()
            .map_err(|e| format!("Could not hide the ring: {e}")),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_answers_pong() {
        assert_eq!(ping(), "pong");
    }

    #[test]
    fn convert_is_a_clear_error_until_wired() {
        let err = convert(vec!["a.png".into()], "jpg".into()).unwrap_err();
        assert!(err.contains("morphring command line"), "{err}");
    }
}
