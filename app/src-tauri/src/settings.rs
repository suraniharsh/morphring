//! The settings window and its commands.
//!
//! Only [`config_dir`] and [`open`] are real so far; the commands answer with
//! defaults until the settings page lands (P2-52).

use std::path::PathBuf;

use morphring_core::settings::AppSettings;
use serde::Serialize;
use tauri::{AppHandle, Runtime};

/// Label of the settings window.
pub const SETTINGS_WINDOW: &str = "settings";

/// The folder holding `settings.json`, `presets.json` and friends: the same
/// folder the CLI uses, so both agree on one set of files. `None` when the
/// system has no home folder to put it in.
pub fn config_dir<R: Runtime>(_app: &AppHandle<R>) -> Option<PathBuf> {
    // Deliberately not Tauri's app_config_dir: that one is named after the
    // bundle identifier, and the CLI would never find a single setting there.
    directories::ProjectDirs::from("", "", "morphring").map(|dirs| dirs.config_dir().to_path_buf())
}

/// Shows the settings window and brings it to the front.
pub fn open<R: Runtime>(app: &AppHandle<R>) {
    // Shared with the tray's own entry: show, focus, and rebuild the window
    // from the config if someone closed it for good.
    crate::tray::show_window(app, SETTINGS_WINDOW);
}

/// One preset as the settings page lists it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PresetSummary {
    pub id: String,
    pub name: String,
    pub target: String,
}

/// The app settings.
#[tauri::command]
pub fn settings_get() -> AppSettings {
    AppSettings::default()
}

/// Validates and saves the app settings.
#[tauri::command]
pub fn settings_set(settings: serde_json::Value) -> Result<(), String> {
    let _ = settings;
    Ok(())
}

/// The presets, for the read-only list on the settings page.
#[tauri::command]
pub fn presets_list() -> Vec<PresetSummary> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stubs_answer_with_defaults() {
        assert_eq!(settings_get(), AppSettings::default());
        assert_eq!(settings_set(serde_json::json!({})), Ok(()));
        assert!(presets_list().is_empty());
    }
}
