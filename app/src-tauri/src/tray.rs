//! Tray icon and menu. Filled in by T28; until then the app has no tray.

use tauri::{App, Runtime};

/// Creates the tray icon. Called once from the app's setup hook.
pub fn setup<R: Runtime>(_app: &mut App<R>) -> tauri::Result<()> {
    // Intentionally empty. T28 moves in here with an Open / Quit menu.
    Ok(())
}
