//! The settings store: app settings and the atomic JSON files they live in.
//! Ported from Filee's `src/Filee.Core/Settings/AppSettings.cs` and
//! `UserDataStore.cs`.
//!
//! The rules (ENGINEERING.md section 7): camelCase JSON, `#[serde(default)]`
//! everywhere, atomic writes, and a corrupt file never crashes anything. It
//! gets copied to `<name>.broken` and we carry on with defaults, like adults.

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tempfile::NamedTempFile;

use crate::output::{DEFAULT_SUBFOLDER, OutputLocation, OutputRule};

/// File name of the app settings inside the store dir.
pub const SETTINGS_FILE: &str = "settings.json";

/// Current `schemaVersion` of `settings.json`.
pub const SETTINGS_SCHEMA_VERSION: u32 = 1;

/// A folder of JSON files. `dir: None` means "no config dir at all": reads
/// give defaults and writes are quietly skipped.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Store {
    pub dir: Option<PathBuf>,
}

impl Store {
    /// A store over `dir`, or a store that never touches the disk.
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self { dir }
    }

    /// Reads `name` from the store dir. A missing file (or no dir) gives the
    /// default. A file that can't be read or parsed is copied to
    /// `<name>.broken`, logged, and also gives the default.
    pub fn read_json<T: DeserializeOwned + Default>(&self, name: &str) -> T {
        let Some(dir) = &self.dir else {
            return T::default();
        };
        let path = dir.join(name);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return T::default(),
            Err(e) => {
                quarantine(&path, &e);
                return T::default();
            }
        };
        match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(e) => {
                quarantine(&path, &e);
                T::default()
            }
        }
    }

    /// Writes `value` to `name` atomically: a temp file in the same dir,
    /// pretty JSON, `sync_all`, then a rename over the old file. With no dir
    /// it writes nothing and returns `Ok`.
    pub fn write_json<T: Serialize>(&self, name: &str, value: &T) -> io::Result<()> {
        let Some(dir) = &self.dir else {
            return Ok(());
        };
        fs::create_dir_all(dir)?;
        let mut tmp = NamedTempFile::new_in(dir)?;
        serde_json::to_writer_pretty(&mut tmp, value)?;
        tmp.write_all(b"\n")?;
        tmp.as_file().sync_all()?;
        tmp.persist(dir.join(name)).map_err(|e| e.error)?;
        Ok(())
    }

    /// Loads `settings.json`.
    pub fn load_settings(&self) -> AppSettings {
        self.read_json(SETTINGS_FILE)
    }

    /// Saves `settings.json`.
    pub fn save_settings(&self, settings: &AppSettings) -> io::Result<()> {
        self.write_json(SETTINGS_FILE, settings)
    }
}

/// Copies a file we could not read to `<name>.broken` so the next save does
/// not destroy the evidence. Best effort: if even the copy fails, we shrug
/// in the log and move on.
fn quarantine(path: &Path, err: &dyn fmt::Display) {
    let mut broken = path.as_os_str().to_owned();
    broken.push(".broken");
    let broken = PathBuf::from(broken);
    match fs::copy(path, &broken) {
        Ok(_) => tracing::warn!(
            path = %path.display(),
            copy = %broken.display(),
            error = %err,
            "could not read the file; using defaults"
        ),
        Err(copy_err) => tracing::warn!(
            path = %path.display(),
            error = %err,
            copy_error = %copy_err,
            "could not read the file and could not back it up; using defaults"
        ),
    }
}

/// App settings. Only the fields v0.2 uses; anything else in the file is
/// ignored, and anything missing takes its default.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    pub schema_version: u32,
    /// Record finished conversions in the history.
    pub keep_history: bool,
    /// Give every output the dates of its source, whatever the preset says.
    pub keep_file_dates: bool,
    /// Where presets set to [`OutputLocation::Default`] write.
    pub default_output: DefaultOutput,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            keep_history: true,
            keep_file_dates: false,
            default_output: DefaultOutput::default(),
        }
    }
}

impl AppSettings {
    /// Filee's `WithDefaultOutput`: swaps [`OutputLocation::Default`] for the
    /// default output location and turns on `keep_dates` when
    /// `keep_file_dates` is set. Any other location is left alone.
    pub fn with_default_output(&self, rule: &OutputRule) -> OutputRule {
        let mut out = rule.clone();
        if out.location == OutputLocation::Default {
            out.location = self.default_output.to_location();
        }
        out.keep_dates |= self.keep_file_dates;
        out
    }
}

/// The save location presets set to "Default" use.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DefaultOutput {
    pub location: DefaultLocation,
    /// Used when `location` is [`DefaultLocation::Subfolder`].
    pub subfolder_name: String,
    /// Used when `location` is [`DefaultLocation::CustomFolder`].
    pub custom_folder: String,
}

impl Default for DefaultOutput {
    fn default() -> Self {
        Self {
            location: DefaultLocation::SameFolder,
            subfolder_name: DEFAULT_SUBFOLDER.to_owned(),
            custom_folder: String::new(),
        }
    }
}

impl DefaultOutput {
    /// The [`OutputLocation`] this default stands for. Blank names are fine
    /// here: `output::resolve` already falls back to the source folder for an
    /// empty custom folder and to "converted" for an empty subfolder.
    pub fn to_location(&self) -> OutputLocation {
        match self.location {
            DefaultLocation::SameFolder => OutputLocation::SameFolder,
            DefaultLocation::Subfolder => OutputLocation::Subfolder(self.subfolder_name.clone()),
            DefaultLocation::CustomFolder => OutputLocation::CustomFolder {
                path: PathBuf::from(&self.custom_folder),
                keep_folders_from: None,
            },
        }
    }
}

/// The kind of default output location. Read case-insensitively; a value we
/// don't know becomes `SameFolder` instead of declaring the whole file broken
/// over one typo.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DefaultLocation {
    #[default]
    SameFolder,
    Subfolder,
    CustomFolder,
}

impl DefaultLocation {
    fn as_str(self) -> &'static str {
        match self {
            Self::SameFolder => "SameFolder",
            Self::Subfolder => "Subfolder",
            Self::CustomFolder => "CustomFolder",
        }
    }
}

impl Serialize for DefaultLocation {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for DefaultLocation {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        let found = [Self::SameFolder, Self::Subfolder, Self::CustomFolder]
            .into_iter()
            .find(|l| l.as_str().eq_ignore_ascii_case(raw.trim()));
        Ok(found.unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store(tmp: &TempDir) -> Store {
        Store::new(Some(tmp.path().to_path_buf()))
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn missing_file_gives_defaults() {
        let tmp = TempDir::new().unwrap();
        let s = store(&tmp).load_settings();
        assert_eq!(s, AppSettings::default());
        assert_eq!(s.schema_version, 1);
        assert!(s.keep_history);
        assert!(!s.keep_file_dates);
        assert_eq!(s.default_output.location, DefaultLocation::SameFolder);
        assert_eq!(s.default_output.subfolder_name, "converted");
        assert_eq!(s.default_output.custom_folder, "");
    }

    #[test]
    fn missing_dir_gives_defaults() {
        let tmp = TempDir::new().unwrap();
        let store = Store::new(Some(tmp.path().join("nope")));
        assert_eq!(store.load_settings(), AppSettings::default());
    }

    #[test]
    fn round_trip() {
        let tmp = TempDir::new().unwrap();
        let store = store(&tmp);
        let settings = AppSettings {
            keep_history: false,
            keep_file_dates: true,
            default_output: DefaultOutput {
                location: DefaultLocation::CustomFolder,
                subfolder_name: "out".into(),
                custom_folder: "/srv/out".into(),
            },
            ..AppSettings::default()
        };
        store.save_settings(&settings).unwrap();
        assert_eq!(store.load_settings(), settings);

        let text = fs::read_to_string(tmp.path().join(SETTINGS_FILE)).unwrap();
        assert!(text.contains("\"schemaVersion\": 1"), "{text}");
        assert!(text.contains("\"defaultOutput\""), "{text}");
        assert!(text.contains("\"CustomFolder\""), "{text}");
    }

    #[test]
    fn write_creates_the_dir() {
        let tmp = TempDir::new().unwrap();
        let store = Store::new(Some(tmp.path().join("a/b")));
        store.save_settings(&AppSettings::default()).unwrap();
        assert!(tmp.path().join("a/b").join(SETTINGS_FILE).is_file());
    }

    #[test]
    fn unknown_fields_ignored_and_missing_fields_defaulted() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join(SETTINGS_FILE),
            r##"{"keepFileDates": true, "theme": {"accent": "#7F77DD"},
                "defaultOutput": {"location": "subfolder"}}"##,
        )
        .unwrap();
        let s = store(&tmp).load_settings();
        assert!(s.keep_file_dates);
        assert!(s.keep_history);
        assert_eq!(s.default_output.location, DefaultLocation::Subfolder);
        assert_eq!(s.default_output.subfolder_name, "converted");
        assert!(!tmp.path().join("settings.json.broken").exists());
    }

    #[test]
    fn unknown_location_falls_back_to_same_folder() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join(SETTINGS_FILE),
            r#"{"defaultOutput": {"location": "Moon"}}"#,
        )
        .unwrap();
        let s = store(&tmp).load_settings();
        assert_eq!(s.default_output.location, DefaultLocation::SameFolder);
    }

    #[test]
    fn corrupt_file_gives_defaults_and_a_broken_copy() {
        let tmp = TempDir::new().unwrap();
        let bad = "{ \"keepHistory\": fals";
        fs::write(tmp.path().join(SETTINGS_FILE), bad).unwrap();
        let s = store(&tmp).load_settings();
        assert_eq!(s, AppSettings::default());
        let copy = fs::read_to_string(tmp.path().join("settings.json.broken")).unwrap();
        assert_eq!(copy, bad);
        // The original stays until the next save overwrites it.
        assert!(tmp.path().join(SETTINGS_FILE).exists());
    }

    #[test]
    fn no_dir_never_writes() {
        let store = Store::new(None);
        store.save_settings(&AppSettings::default()).unwrap();
        store.write_json("anything.json", &42).unwrap();
        assert_eq!(store.load_settings(), AppSettings::default());
        assert_eq!(store.read_json::<u32>("anything.json"), 0);
    }

    #[test]
    fn atomic_write_leaves_no_temp_files() {
        let tmp = TempDir::new().unwrap();
        let store = store(&tmp);
        store.save_settings(&AppSettings::default()).unwrap();
        let changed = AppSettings {
            keep_history: false,
            ..AppSettings::default()
        };
        store.save_settings(&changed).unwrap();
        assert_eq!(names(tmp.path()), vec![SETTINGS_FILE.to_owned()]);
        assert_eq!(store.load_settings(), changed);
    }

    fn settings_with(location: DefaultLocation) -> AppSettings {
        AppSettings {
            default_output: DefaultOutput {
                location,
                subfolder_name: "done".into(),
                custom_folder: "/srv/out".into(),
            },
            ..AppSettings::default()
        }
    }

    fn default_rule() -> OutputRule {
        OutputRule {
            location: OutputLocation::Default,
            ..OutputRule::default()
        }
    }

    #[test]
    fn with_default_output_same_folder() {
        let out = settings_with(DefaultLocation::SameFolder).with_default_output(&default_rule());
        assert_eq!(out.location, OutputLocation::SameFolder);
        assert!(!out.keep_dates);
    }

    #[test]
    fn with_default_output_subfolder() {
        let out = settings_with(DefaultLocation::Subfolder).with_default_output(&default_rule());
        assert_eq!(out.location, OutputLocation::Subfolder("done".into()));
    }

    #[test]
    fn with_default_output_custom_folder() {
        let out = settings_with(DefaultLocation::CustomFolder).with_default_output(&default_rule());
        assert_eq!(
            out.location,
            OutputLocation::CustomFolder {
                path: PathBuf::from("/srv/out"),
                keep_folders_from: None,
            }
        );
    }

    #[test]
    fn with_default_output_leaves_explicit_locations_alone() {
        let rule = OutputRule {
            location: OutputLocation::Subfolder("mine".into()),
            pattern: "{name}-x".into(),
            ..OutputRule::default()
        };
        let out = settings_with(DefaultLocation::CustomFolder).with_default_output(&rule);
        assert_eq!(out, rule);
    }

    #[test]
    fn with_default_output_ors_keep_dates() {
        let mut settings = settings_with(DefaultLocation::SameFolder);
        settings.keep_file_dates = true;
        let out = settings.with_default_output(&OutputRule::default());
        assert!(out.keep_dates);

        settings.keep_file_dates = false;
        let rule = OutputRule {
            keep_dates: true,
            ..OutputRule::default()
        };
        assert!(settings.with_default_output(&rule).keep_dates);
    }
}
