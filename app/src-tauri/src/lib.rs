//! The MorphRing desktop app: a main window with a drop zone, the conversion
//! ring window, the settings, history and toast windows, and the tray. This
//! crate is thin glue around `morphring-core` and `morphring-engines`;
//! conversion logic never lives here.

use anyhow::Context as _;

pub mod commands;
pub mod drag_ring;
pub mod dropzone;
#[cfg(target_os = "linux")]
mod gesture_linux;
pub mod history;
#[cfg(unix)]
pub mod hyprland;
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
        .manage(drag_ring::GestureActive::default())
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
            toast::toast_dismiss,
            toast::toast_check,
            toast::toast_dismiss_delay,
            toast::toast_layout
        ])
        .setup(|app| {
            tray::setup(app)?;
            // Before on_startup: whether the gesture runs decides whether the main
            // window shows at all.
            start_gesture(app.handle());
            instance::on_startup(app.handle(), std::env::args().collect());
            lifecycle::setup(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            dropzone::on_window_event(window, event);
            drag_ring::on_window_event(window, event);
        })
        .build(tauri::generate_context!())
        .context("while building the MorphRing app")?;
    // build + run instead of plain run, so the lifecycle module gets to see run
    // events (macOS hands over Finder's files through one of them).
    app.run(|app, event| lifecycle::on_run_event(app, &event));
    Ok(())
}

/// Starts the Ctrl+drag watcher where we have one, and remembers whether it runs.
fn start_gesture<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    use tauri::Manager as _;
    #[cfg(target_os = "linux")]
    let running = gesture_linux::start(app);
    // Windows hooks and the macOS event tap come later; until then the drop
    // zone and the file manager menus are the way in there.
    #[cfg(not(target_os = "linux"))]
    let running = false;
    app.state::<drag_ring::GestureActive>()
        .0
        .store(running, std::sync::atomic::Ordering::Relaxed);
}
