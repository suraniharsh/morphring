//! Single instance and `--convert <files>` handling. Filled in by T28.

use tauri::{AppHandle, Manager, Runtime};

/// Called with the argv of this process once the app is set up.
pub fn on_startup<R: Runtime>(_app: &AppHandle<R>, _argv: Vec<String>) {
    // Intentionally empty: T28 parses `--convert` here (with its 350 ms debounce).
}

/// Called in the running instance when someone launches the app a second time.
pub fn on_second_instance<R: Runtime>(app: &AppHandle<R>, argv: Vec<String>, cwd: String) {
    tracing::debug!(?argv, %cwd, "second instance started");
    // Until T28 forwards `--convert`, the least surprising thing a second launch can
    // do is bring the window you already have to the front instead of a twin.
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}
