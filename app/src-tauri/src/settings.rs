//! The settings window, its commands, and the managed [`Prefs`] state the
//! rest of the app reads settings and presets from.
//!
//! The state is loaded lazily on first use (lib.rs is not ours to edit, so
//! nobody calls `manage` for us at startup). Everything that matters lives in
//! plain functions on [`Prefs`] so tests can poke it without a running app.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use morphring_core::profiles::Library;
use morphring_core::settings::{AppSettings, DefaultLocation, SETTINGS_SCHEMA_VERSION, Store};
use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime};

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

/// The app's settings and preset library, as managed state.
pub struct Prefs {
    store: Store,
    settings: Mutex<AppSettings>,
    library: Mutex<Library>,
}

/// Locks a mutex, poisoned or not. A panic elsewhere while holding a clone of
/// some settings is no reason to lock the user out of their own preferences.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Prefs {
    /// Reads `settings.json`, `presets.json` and `profiles.json` from `store`.
    /// Missing or broken files give defaults (the store handles the drama).
    pub fn load(store: Store) -> Self {
        let settings = store.load_settings();
        let library = Library::load(&store);
        Self {
            store,
            settings: Mutex::new(settings),
            library: Mutex::new(library),
        }
    }

    /// The folder the files live in, if there is one.
    pub fn dir(&self) -> Option<&Path> {
        self.store.dir.as_deref()
    }

    /// A snapshot of the settings and the library.
    pub fn current(&self) -> (AppSettings, Library) {
        // Settings first, library second, always: two locks taken in a fixed
        // order never meet in a dark alley.
        let settings = lock(&self.settings).clone();
        let library = lock(&self.library).clone();
        (settings, library)
    }

    /// Parses, validates and saves new settings, then makes them current.
    /// Nothing changes, on disk or in memory, unless all of that worked.
    pub fn set(&self, raw: serde_json::Value) -> Result<AppSettings, String> {
        let mut settings: AppSettings =
            serde_json::from_value(raw).map_err(|e| format!("The settings are not valid: {e}"))?;
        settings.schema_version = SETTINGS_SCHEMA_VERSION;
        validate(&settings)?;
        let mut current = lock(&self.settings);
        self.store
            .save_settings(&settings)
            .map_err(|e| format!("Could not save the settings: {e}"))?;
        *current = settings.clone();
        Ok(settings)
    }

    /// The presets as the settings page lists them.
    pub fn presets(&self) -> Vec<PresetSummary> {
        lock(&self.library)
            .presets
            .iter()
            .map(|p| PresetSummary {
                id: p.id.clone(),
                name: p.display_name(),
                target: p.target.clone(),
            })
            .collect()
    }
}

/// Checks settings before they are saved. Only the active default output
/// location is checked: a stale custom folder sitting next to "Same folder"
/// hurts nobody.
pub fn validate(settings: &AppSettings) -> Result<(), String> {
    let out = &settings.default_output;
    match out.location {
        DefaultLocation::SameFolder => Ok(()),
        DefaultLocation::Subfolder => validate_subfolder(&out.subfolder_name),
        DefaultLocation::CustomFolder => validate_custom_folder(&out.custom_folder),
    }
}

/// A subfolder name must have something in it besides whitespace.
pub fn validate_subfolder(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("Enter a name for the subfolder.".to_owned());
    }
    Ok(())
}

/// A custom folder must be an absolute path: relative to what, otherwise?
pub fn validate_custom_folder(folder: &str) -> Result<(), String> {
    let folder = folder.trim();
    if folder.is_empty() {
        return Err("Choose a folder for the converted files.".to_owned());
    }
    if !Path::new(folder).is_absolute() {
        return Err("The output folder must be an absolute path.".to_owned());
    }
    Ok(())
}

/// The [`Prefs`] state, loaded and managed on first use.
pub fn prefs<R: Runtime>(app: &AppHandle<R>) -> tauri::State<'_, Prefs> {
    if let Some(state) = app.try_state::<Prefs>() {
        return state;
    }
    // Two threads may both get here and both load the files. The second
    // `manage` loses and returns false, which is fine: same files, same
    // answer, slightly warmer CPU.
    app.manage(Prefs::load(Store::new(config_dir(app))));
    app.state::<Prefs>()
}

/// The current settings and preset library, for the other modules.
pub fn current<R: Runtime>(app: &AppHandle<R>) -> (AppSettings, Library) {
    prefs(app).current()
}

/// One preset as the settings page lists it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PresetSummary {
    pub id: String,
    pub name: String,
    pub target: String,
}

/// What `settings_get` answers: the settings JSON, plus the config folder so
/// the page can show it. `configDir` is never saved; `settings_set` ignores it.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    #[serde(flatten)]
    pub settings: AppSettings,
    pub config_dir: Option<String>,
}

impl Prefs {
    /// The settings as `settings_get` sends them.
    pub fn view(&self) -> SettingsView {
        SettingsView {
            settings: lock(&self.settings).clone(),
            config_dir: self.dir().map(|d| d.display().to_string()),
        }
    }
}

/// The app settings, plus the folder they live in.
#[tauri::command]
pub fn settings_get<R: Runtime>(app: AppHandle<R>) -> SettingsView {
    prefs(&app).view()
}

/// Validates and saves the app settings.
#[tauri::command]
pub fn settings_set<R: Runtime>(
    app: AppHandle<R>,
    settings: serde_json::Value,
) -> Result<(), String> {
    prefs(&app).set(settings).map(|_| ())
}

/// The presets, for the read-only list on the settings page.
#[tauri::command]
pub fn presets_list<R: Runtime>(app: AppHandle<R>) -> Vec<PresetSummary> {
    prefs(&app).presets()
}

#[cfg(test)]
mod tests {
    use super::*;
    use morphring_core::settings::SETTINGS_FILE;
    use serde_json::json;
    use tempfile::TempDir;

    fn prefs_in(tmp: &TempDir) -> Prefs {
        Prefs::load(Store::new(Some(tmp.path().to_path_buf())))
    }

    fn with_output(location: &str, subfolder: &str, custom: &str) -> serde_json::Value {
        json!({
            "keepHistory": true,
            "keepFileDates": false,
            "defaultOutput": {
                "location": location,
                "subfolderName": subfolder,
                "customFolder": custom,
            }
        })
    }

    #[test]
    fn blank_subfolder_is_rejected() {
        assert!(validate_subfolder("").is_err());
        assert!(validate_subfolder("   ").is_err());
        assert!(validate_subfolder("out").is_ok());
    }

    #[test]
    fn custom_folder_must_be_absolute() {
        assert!(validate_custom_folder("").is_err());
        assert!(validate_custom_folder("relative/out").is_err());
        assert!(validate_custom_folder("./out").is_err());
        let absolute = std::env::temp_dir();
        assert!(validate_custom_folder(&absolute.display().to_string()).is_ok());
    }

    #[test]
    fn only_the_active_location_is_validated() {
        let tmp = TempDir::new().unwrap();
        let prefs = prefs_in(&tmp);
        // A blank subfolder name is fine while nobody is using it.
        assert!(prefs.set(with_output("SameFolder", "", "nope")).is_ok());
        assert!(prefs.set(with_output("Subfolder", " ", "")).is_err());
        assert!(
            prefs
                .set(with_output("CustomFolder", "out", "rel"))
                .is_err()
        );
    }

    #[test]
    fn set_saves_subfolder_to_settings_json() {
        let tmp = TempDir::new().unwrap();
        let prefs = prefs_in(&tmp);
        let saved = prefs.set(with_output("Subfolder", "out", "")).unwrap();
        assert_eq!(saved.default_output.location, DefaultLocation::Subfolder);

        let text = std::fs::read_to_string(tmp.path().join(SETTINGS_FILE)).unwrap();
        let file: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(file["defaultOutput"]["location"], "Subfolder");
        assert_eq!(file["defaultOutput"]["subfolderName"], "out");
        assert_eq!(file["schemaVersion"], 1);

        // And the state moved along with the file.
        let (settings, _) = prefs.current();
        assert_eq!(settings, saved);
        assert_eq!(prefs_in(&tmp).current().0, saved);
    }

    #[test]
    fn rejected_settings_change_nothing() {
        let tmp = TempDir::new().unwrap();
        let prefs = prefs_in(&tmp);
        let err = prefs.set(with_output("Subfolder", "", "")).unwrap_err();
        assert!(err.contains("subfolder"), "{err}");
        assert!(!tmp.path().join(SETTINGS_FILE).exists());
        assert_eq!(prefs.current().0, AppSettings::default());
    }

    #[test]
    fn malformed_json_is_an_error() {
        let tmp = TempDir::new().unwrap();
        let prefs = prefs_in(&tmp);
        assert!(prefs.set(json!({ "keepHistory": "yes please" })).is_err());
        assert!(!tmp.path().join(SETTINGS_FILE).exists());
    }

    #[test]
    fn config_dir_is_ignored_on_set_and_shown_on_get() {
        let tmp = TempDir::new().unwrap();
        let prefs = prefs_in(&tmp);
        let mut raw = with_output("SameFolder", "converted", "");
        raw["configDir"] = json!("/somewhere/else");
        prefs.set(raw).unwrap();
        let text = std::fs::read_to_string(tmp.path().join(SETTINGS_FILE)).unwrap();
        assert!(!text.contains("configDir"), "{text}");

        let view = serde_json::to_value(prefs.view()).unwrap();
        assert_eq!(view["configDir"], tmp.path().display().to_string());
        assert_eq!(view["defaultOutput"]["location"], "SameFolder");
        assert_eq!(view["keepHistory"], true);
    }

    #[test]
    fn presets_list_the_builtins() {
        let tmp = TempDir::new().unwrap();
        let prefs = prefs_in(&tmp);
        let presets = prefs.presets();
        assert!(!presets.is_empty());
        assert!(
            presets
                .iter()
                .all(|p| !p.id.is_empty() && !p.name.is_empty())
        );
        let (_, library) = prefs.current();
        assert_eq!(presets.len(), library.presets.len());
    }

    #[test]
    fn no_config_dir_still_works() {
        let prefs = Prefs::load(Store::new(None));
        assert!(prefs.dir().is_none());
        assert!(prefs.set(with_output("Subfolder", "out", "")).is_ok());
        assert_eq!(prefs.view().config_dir, None);
    }
}
