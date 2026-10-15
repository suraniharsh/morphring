//! The MorphRing desktop app: a main window with a drop zone, the conversion
//! ring window and the tray. This crate is thin glue around `morphring-core`
//! and `morphring-engines`; conversion logic never lives here.

use anyhow::Context as _;

pub mod commands;
pub mod dropzone;
pub mod instance;
pub mod tray;

/// Builds and runs the app until the last window closes or the user quits.
pub fn run() -> anyhow::Result<()> {
    // Everything that will ever be registered is registered here, today, as a stub
    // if need be. The tray, single-instance and command tasks then fill in their own
    // modules and nobody fights over this file in a merge. Peace through foresight.
    tauri::Builder::default()
        // Single-instance goes first: a second launch should hand over its argv and
        // leave before it builds a single window.
        .plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            instance::on_second_instance(app, argv, cwd);
        }))
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::convert,
            commands::ring_close,
            commands::ping
        ])
        .setup(|app| {
            tray::setup(app)?;
            instance::on_startup(app.handle(), std::env::args().collect());
            Ok(())
        })
        .on_window_event(dropzone::on_window_event)
        .run(tauri::generate_context!())
        .context("while running the MorphRing app")
}
