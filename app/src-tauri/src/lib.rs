//! The MorphRing desktop app: a main window with a drop zone, the conversion
//! ring window, the settings, history and toast windows, and the tray. This
//! crate is thin glue around `morphring-core` and `morphring-engines`;
//! conversion logic never lives here.

use anyhow::Context as _;

pub mod commands;
pub mod dropzone;
pub mod history;
pub mod instance;
pub mod lifecycle;
pub mod ring_window;
pub mod settings;
pub mod toast;
pub mod tray;

/// Builds and runs the app until the last window closes or the user quits.
pub fn run() -> anyhow::Result<()> {
    // Everything that will ever be registered is registered here, today, as a stub
    // if need be. The tray, single-instance and command tasks then fill in their own
    // modules and nobody fights over this file in a merge. Peace through foresight.
    let app = tauri::Builder::default()
        // Single-instance goes first: a second launch should hand over its argv and
        // leave before it builds a single window.
        .plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            instance::on_second_instance(app, argv, cwd);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(lifecycle::PendingOpen::default())
        .invoke_handler(tauri::generate_handler![
            commands::convert,
            commands::ring_close,
            commands::ping,
            commands::cancel_job,
            settings::settings_get,
            settings::settings_set,
            settings::presets_list,
            history::history_list,
            history::history_clear,
            toast::toast_dismiss
        ])
        .setup(|app| {
            tray::setup(app)?;
            instance::on_startup(app.handle(), std::env::args().collect());
            lifecycle::setup(app)?;
            Ok(())
        })
        .on_window_event(dropzone::on_window_event)
        .build(tauri::generate_context!())
        .context("while building the MorphRing app")?;
    // build + run instead of plain run, so the lifecycle module gets to see run
    // events (macOS hands over Finder's files through one of them).
    app.run(|app, event| lifecycle::on_run_event(app, &event));
    Ok(())
}
