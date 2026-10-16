//! Windows Explorer integration.
//!
//! The stage 1 plan from `docs/research/windows-support.md` section 3
//! (options 1 and 2), both per user and without admin rights:
//!
//! - a classic verb under `HKCU\Software\Classes\*\shell\MorphRing`, plus the
//!   same verb under `Directory\shell` so folders get it too;
//! - a Send To shortcut, `%APPDATA%\Microsoft\Windows\SendTo\MorphRing.lnk`.
//!
//! The verb starts one process per selected file (`MultiSelectModel=Player`);
//! the app gathers those back up with single-instance and its 350 ms
//! debouncer. Send To hands over every file in one command line.
//!
//! What gets written is computed by pure functions ([`verb_entries`],
//! [`sendto_path`]) that are tested on every OS. Only the part that actually
//! pokes the registry is `#[cfg(windows)]`.

use std::path::{Path, PathBuf};

use crate::integration::{Change, IntegrationError, LABEL};

/// Where per-user classes live, relative to `HKEY_CURRENT_USER`.
pub const CLASSES_ROOT: &str = r"Software\Classes";

/// The verb for files, relative to `HKEY_CURRENT_USER`.
pub const FILE_VERB_KEY: &str = r"Software\Classes\*\shell\MorphRing";

/// The verb for folders, relative to `HKEY_CURRENT_USER`.
pub const DIRECTORY_VERB_KEY: &str = r"Software\Classes\Directory\shell\MorphRing";

/// The verb keys below a classes root, in the order they are written.
pub const VERB_SUBKEYS: [&str; 2] = [r"*\shell\MorphRing", r"Directory\shell\MorphRing"];

/// The value set on every verb key MorphRing writes. Uninstall removes a key
/// only when this value is `1`.
pub const MARKER: &str = "MorphRingManaged";

/// The Send To shortcut, relative to `%APPDATA%`.
pub const SENDTO_RELATIVE: &str = r"Microsoft\Windows\SendTo\MorphRing.lnk";

/// The argument that tells the app to convert the paths after it.
pub const CONVERT_ARG: &str = "--convert";

/// One registry value: `name` under `key` (relative to `HKEY_CURRENT_USER`)
/// set to the string `value`. An empty `name` is the key's default value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegEntry {
    pub key: String,
    pub name: String,
    pub value: String,
}

/// The verb keys below `root`, relative to `HKEY_CURRENT_USER`.
pub fn verb_keys(root: &str) -> [String; 2] {
    VERB_SUBKEYS.map(|sub| format!(r"{root}\{sub}"))
}

/// The command line the verb runs for `exe`: `"<exe>" --convert "%1"`.
pub fn verb_command(exe: &Path) -> String {
    format!("\"{}\" {CONVERT_ARG} \"%1\"", exe.to_string_lossy())
}

/// Every registry value the Explorer verb needs for `exe`, below
/// [`CLASSES_ROOT`].
pub fn verb_entries(exe: &Path) -> Vec<RegEntry> {
    verb_entries_under(CLASSES_ROOT, exe)
}

/// Every registry value the Explorer verb needs for `exe`, below `root`
/// instead of [`CLASSES_ROOT`], so tests can use a throwaway root.
pub fn verb_entries_under(root: &str, exe: &Path) -> Vec<RegEntry> {
    let exe_str = exe.to_string_lossy().into_owned();
    let entry = |key: &str, name: &str, value: &str| RegEntry {
        key: key.to_owned(),
        name: name.to_owned(),
        value: value.to_owned(),
    };
    let mut entries = Vec::new();
    for key in verb_keys(root) {
        // The marker goes first: if writing dies halfway, the half-built key
        // is still ours, so the next install or uninstall may clean it up
        // instead of tiptoeing around it forever.
        entries.push(entry(&key, MARKER, "1"));
        entries.push(entry(&key, "", LABEL));
        entries.push(entry(&key, "Icon", &exe_str));
        entries.push(entry(&key, "MultiSelectModel", "Player"));
        entries.push(entry(&format!(r"{key}\command"), "", &verb_command(exe)));
    }
    entries
}

/// The Send To shortcut below the roaming application data folder
/// (`%APPDATA%`).
pub fn sendto_path(appdata: &Path) -> PathBuf {
    SENDTO_RELATIVE
        .split('\\')
        .fold(appdata.to_path_buf(), |path, part| path.join(part))
}

/// Checks that `exe` can be written into a quoted command line, and returns
/// it as a string.
pub fn validate_exe(exe: &Path) -> Result<&str, IntegrationError> {
    let invalid = |reason| IntegrationError::InvalidExecutable {
        path: exe.to_path_buf(),
        reason,
    };
    let text = exe.to_str().ok_or_else(|| invalid("not valid Unicode"))?;
    if text.is_empty() {
        return Err(invalid("empty path"));
    }
    // Windows forbids quotes in file names anyway, and one would end the
    // quoted exe in the command line early. Two good reasons, one check.
    if text.contains('"') {
        return Err(invalid("contains a double quote"));
    }
    Ok(text)
}

/// Adds the Explorer verb and the Send To shortcut starting `exe` for the
/// user whose profile folder is `home`, and returns what changed (or, with
/// `dry_run`, what would change).
#[cfg(windows)]
pub fn install(exe: &Path, home: &Path, dry_run: bool) -> Result<Vec<Change>, IntegrationError> {
    if !exe.is_absolute() {
        return Err(IntegrationError::InvalidExecutable {
            path: exe.to_path_buf(),
            reason: "not an absolute path",
        });
    }
    registry::install_at(CLASSES_ROOT, exe, &appdata(home), dry_run)
}

/// Removes the Explorer verb and the Send To shortcut for the user whose
/// profile folder is `home`. Only keys carrying [`MARKER`] are removed.
#[cfg(windows)]
pub fn uninstall(home: &Path, dry_run: bool) -> Result<Vec<Change>, IntegrationError> {
    registry::uninstall_at(CLASSES_ROOT, &appdata(home), dry_run)
}

/// The Explorer menu only exists on Windows.
#[cfg(not(windows))]
pub fn install(exe: &Path, home: &Path, dry_run: bool) -> Result<Vec<Change>, IntegrationError> {
    let _ = (exe, home, dry_run);
    Err(IntegrationError::Unsupported)
}

/// The Explorer menu only exists on Windows.
#[cfg(not(windows))]
pub fn uninstall(home: &Path, dry_run: bool) -> Result<Vec<Change>, IntegrationError> {
    let _ = (home, dry_run);
    Err(IntegrationError::Unsupported)
}

/// `%APPDATA%` for the profile at `home`. Derived from `home` rather than the
/// environment so a test's temporary home stays a temporary home.
#[cfg(windows)]
fn appdata(home: &Path) -> PathBuf {
    home.join("AppData").join("Roaming")
}

#[cfg(windows)]
mod registry {
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};

    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    use super::{
        CONVERT_ARG, LABEL, MARKER, sendto_path, validate_exe, verb_entries_under, verb_keys,
    };
    use crate::integration::{Change, IntegrationError};

    /// What a verb key currently is.
    enum KeyState {
        Missing,
        Ours,
        Foreign,
    }

    fn full_name(key: &str) -> String {
        format!(r"HKCU\{key}")
    }

    fn io_error(key: &str, source: io::Error) -> IntegrationError {
        IntegrationError::Io {
            path: PathBuf::from(full_name(key)),
            source,
        }
    }

    fn state(hkcu: &RegKey, key: &str) -> Result<KeyState, IntegrationError> {
        match hkcu.open_subkey(key) {
            Ok(opened) => match opened.get_value::<String, _>(MARKER) {
                Ok(marker) if marker == "1" => Ok(KeyState::Ours),
                _ => Ok(KeyState::Foreign),
            },
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(KeyState::Missing),
            Err(err) => Err(io_error(key, err)),
        }
    }

    /// Installs the verbs below `root` (relative to `HKEY_CURRENT_USER`) and
    /// the Send To shortcut below `appdata`.
    pub(super) fn install_at(
        root: &str,
        exe: &Path,
        appdata: &Path,
        dry_run: bool,
    ) -> Result<Vec<Change>, IntegrationError> {
        let exe_str = validate_exe(exe)?;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let keys = verb_keys(root);
        for key in &keys {
            if let KeyState::Foreign = state(&hkcu, key)? {
                return Err(IntegrationError::Unmanaged {
                    path: PathBuf::from(full_name(key)),
                });
            }
        }
        let link = sendto_path(appdata);
        let mut changes: Vec<Change> = keys
            .iter()
            .map(|k| Change::Registry(full_name(k)))
            .collect();
        changes.push(Change::File(link.clone()));
        if dry_run {
            return Ok(changes);
        }

        for entry in verb_entries_under(root, exe) {
            let (key, _) = hkcu
                .create_subkey(&entry.key)
                .map_err(|err| io_error(&entry.key, err))?;
            key.set_value(&entry.name, &entry.value)
                .map_err(|err| io_error(&entry.key, err))?;
        }

        let link_error = |source| IntegrationError::Io {
            path: link.clone(),
            source,
        };
        if let Some(dir) = link.parent() {
            fs::create_dir_all(dir).map_err(link_error)?;
        }
        let mut shortcut = mslnk::ShellLink::new(exe)
            .map_err(|err| link_error(io::Error::other(err.to_string())))?;
        shortcut.set_arguments(Some(CONVERT_ARG.to_owned()));
        shortcut.set_icon_location(Some(exe_str.to_owned()));
        shortcut.set_name(Some(LABEL.to_owned()));
        shortcut
            .create_lnk(&link)
            .map_err(|err| link_error(io::Error::other(err.to_string())))?;
        Ok(changes)
    }

    /// Removes the marked verbs below `root` and the Send To shortcut below
    /// `appdata`. A verb key without the marker belongs to someone else and
    /// is left exactly where it is.
    pub(super) fn uninstall_at(
        root: &str,
        appdata: &Path,
        dry_run: bool,
    ) -> Result<Vec<Change>, IntegrationError> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let mut changes = Vec::new();
        for key in verb_keys(root) {
            if let KeyState::Ours = state(&hkcu, &key)? {
                if !dry_run {
                    hkcu.delete_subkey_all(&key)
                        .map_err(|err| io_error(&key, err))?;
                }
                changes.push(Change::Registry(full_name(&key)));
            }
        }
        let link = sendto_path(appdata);
        if link.is_file() {
            if !dry_run {
                fs::remove_file(&link).map_err(|source| IntegrationError::Io {
                    path: link.clone(),
                    source,
                })?;
            }
            changes.push(Change::File(link));
        }
        Ok(changes)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// A throwaway classes root. Not a real ProgID, so Explorer never
        /// shows anything written here, even if a test dies midway.
        const TEST_ROOT: &str = r"Software\Classes\MorphRingTest";

        fn exists(key: &str) -> bool {
            RegKey::predef(HKEY_CURRENT_USER).open_subkey(key).is_ok()
        }

        #[test]
        fn registry_round_trip_under_the_test_root_leaves_nothing() {
            let hkcu = RegKey::predef(HKEY_CURRENT_USER);
            let _ = hkcu.delete_subkey_all(TEST_ROOT);
            let dir = tempfile::TempDir::new().unwrap();
            let exe = dir.path().join("Morph Ring é").join("morphring-app.exe");
            fs::create_dir_all(exe.parent().unwrap()).unwrap();
            fs::write(&exe, b"MZ").unwrap();
            let appdata = dir.path().join("AppData").join("Roaming");
            let keys = verb_keys(TEST_ROOT);
            let link = sendto_path(&appdata);

            let planned = install_at(TEST_ROOT, &exe, &appdata, true).unwrap();
            assert_eq!(planned.len(), 3);
            assert!(!exists(&keys[0]) && !exists(&keys[1]) && !link.exists());

            assert_eq!(
                install_at(TEST_ROOT, &exe, &appdata, false).unwrap(),
                planned
            );
            for entry in verb_entries_under(TEST_ROOT, &exe) {
                let key = hkcu.open_subkey(&entry.key).unwrap();
                assert_eq!(
                    key.get_value::<String, _>(&entry.name).unwrap(),
                    entry.value
                );
            }
            assert!(link.is_file());
            // Installing over our own keys is fine.
            assert_eq!(
                install_at(TEST_ROOT, &exe, &appdata, false).unwrap(),
                planned
            );

            assert_eq!(uninstall_at(TEST_ROOT, &appdata, true).unwrap(), planned);
            assert!(exists(&keys[0]) && link.is_file());
            assert_eq!(uninstall_at(TEST_ROOT, &appdata, false).unwrap(), planned);
            assert!(!exists(&keys[0]) && !exists(&keys[1]) && !link.exists());
            assert!(uninstall_at(TEST_ROOT, &appdata, false).unwrap().is_empty());

            // A verb someone else wrote is neither replaced nor removed.
            hkcu.create_subkey(&keys[1]).unwrap();
            assert!(matches!(
                install_at(TEST_ROOT, &exe, &appdata, false),
                Err(IntegrationError::Unmanaged { .. })
            ));
            assert!(uninstall_at(TEST_ROOT, &appdata, false).unwrap().is_empty());
            assert!(exists(&keys[1]));

            hkcu.delete_subkey_all(TEST_ROOT).unwrap();
            assert!(!exists(TEST_ROOT));
        }

        #[test]
        fn integration_dry_run_lists_both_verbs_and_the_shortcut() {
            let home = tempfile::TempDir::new().unwrap();
            let exe = home.path().join("Programs").join("morphring-app.exe");
            let changes = crate::integration::install(&exe, home.path(), true).unwrap();
            assert_eq!(
                changes,
                [
                    Change::Registry(full_name(super::super::FILE_VERB_KEY)),
                    Change::Registry(full_name(super::super::DIRECTORY_VERB_KEY)),
                    Change::File(sendto_path(&home.path().join("AppData").join("Roaming"))),
                ]
            );
            assert!(!home.path().join("AppData").exists());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command_of(entries: &[RegEntry], key: &str) -> String {
        entries
            .iter()
            .find(|e| e.key == format!(r"{key}\command") && e.name.is_empty())
            .map(|e| e.value.clone())
            .unwrap()
    }

    #[test]
    fn the_exported_keys_match_the_generated_ones() {
        assert_eq!(
            verb_keys(CLASSES_ROOT),
            [FILE_VERB_KEY.to_owned(), DIRECTORY_VERB_KEY.to_owned()]
        );
    }

    #[test]
    fn verb_entries_cover_files_and_folders() {
        let exe = Path::new(r"C:\Program Files\MorphRing\morphring-app.exe");
        let entries = verb_entries(exe);
        assert_eq!(entries.len(), 10);
        for key in [FILE_VERB_KEY, DIRECTORY_VERB_KEY] {
            let value = |name: &str| {
                entries
                    .iter()
                    .find(|e| e.key == key && e.name == name)
                    .map(|e| e.value.as_str())
            };
            assert_eq!(value(""), Some(crate::integration::LABEL));
            assert_eq!(value("Icon"), Some(exe.to_str().unwrap()));
            assert_eq!(value("MultiSelectModel"), Some("Player"));
            assert_eq!(value(MARKER), Some("1"));
            assert_eq!(
                command_of(&entries, key),
                r#""C:\Program Files\MorphRing\morphring-app.exe" --convert "%1""#
            );
        }
        // The marker is written before anything else on each key.
        assert_eq!(entries[0].name, MARKER);
        assert_eq!(entries[5].name, MARKER);
    }

    #[test]
    fn verb_command_quotes_non_ascii_paths_verbatim() {
        let exe = Path::new(r"C:\Users\Zoë\アプリ\morphring-app.exe");
        let entries = verb_entries_under(r"Software\Classes\MorphRingTest", exe);
        assert_eq!(
            command_of(
                &entries,
                r"Software\Classes\MorphRingTest\Directory\shell\MorphRing"
            ),
            r#""C:\Users\Zoë\アプリ\morphring-app.exe" --convert "%1""#
        );
        assert!(
            entries
                .iter()
                .all(|e| e.key.starts_with(r"Software\Classes\MorphRingTest\"))
        );
    }

    #[test]
    fn sendto_path_sits_under_appdata() {
        let appdata = Path::new("roaming");
        assert_eq!(
            sendto_path(appdata),
            appdata
                .join("Microsoft")
                .join("Windows")
                .join("SendTo")
                .join("MorphRing.lnk")
        );
    }

    #[test]
    fn exe_paths_that_would_break_the_command_are_refused() {
        assert_eq!(
            validate_exe(Path::new(r"C:\A B\app.exe")).unwrap(),
            r"C:\A B\app.exe"
        );
        for bad in ["", r#"C:\a"b\app.exe"#] {
            assert!(matches!(
                validate_exe(Path::new(bad)),
                Err(IntegrationError::InvalidExecutable { .. })
            ));
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn other_systems_have_no_explorer() {
        let (exe, home) = (Path::new("/opt/morphring-app"), Path::new("/home/u"));
        assert!(matches!(
            install(exe, home, true),
            Err(IntegrationError::Unsupported)
        ));
        assert!(matches!(
            uninstall(home, true),
            Err(IntegrationError::Unsupported)
        ));
    }
}
