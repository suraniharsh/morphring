//! App lifecycle hooks: the setup step and the run-event loop.
//!
//! Both are empty on purpose. They are registered in `lib.rs` now so the
//! macOS task (XP-10: `RunEvent::Opened` for files handed over by Finder) only
//! ever edits this file. Until then they do a fine impression of a no-op.

use std::error::Error;
use std::path::PathBuf;
use std::sync::Mutex;

use tauri::{App, AppHandle, RunEvent};

/// Files the OS asked us to open before the app was ready to show a ring.
/// Managed app state; filled and drained by the platform lifecycle code.
#[derive(Debug, Default)]
pub struct PendingOpen(pub Mutex<Vec<PathBuf>>);

/// Runs at the end of the app's setup hook, once every other part is set up.
pub fn setup(app: &mut App) -> Result<(), Box<dyn Error>> {
    let _ = app;
    Ok(())
}

/// Called for every event of the app's run loop.
pub fn on_run_event(app: &AppHandle, event: &RunEvent) {
    let _ = app;
    // One lonely wildcard arm today; the platform arms move in with XP-10.
    #[allow(clippy::match_single_binding)]
    match event {
        _ => {}
    }
}
