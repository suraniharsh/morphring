//! The "Convert with MorphRing" menu entry, on whatever desktop this build
//! targets.
//!
//! [`install`] and [`uninstall`] pick the backend at compile time:
//!
//! - Linux: [`context_menu`](crate::context_menu) (Dolphin, Nautilus, Nemo,
//!   Thunar),
//! - Windows: [`explorer`](crate::explorer),
//! - macOS: [`finder`](crate::finder),
//! - anything else: [`IntegrationError::Unsupported`].
//!
//! Both return the [`Change`]s they made (or, with `dry_run`, would make), so
//! the caller can report them without knowing which backend ran.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// The menu label shown by every file manager on every OS.
pub const LABEL: &str = "Convert with MorphRing";

/// One thing an install or uninstall wrote or removed (or would have).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// A file, such as a Linux service menu or a Windows shortcut.
    File(PathBuf),
    /// A registry key, written as its full path (Windows only).
    Registry(String),
    /// Something the user may still have to do by hand, such as turning the
    /// Finder Quick Action on in System Settings (macOS only).
    Note(String),
}

impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Change::File(path) => write!(f, "{}", path.display()),
            Change::Registry(key) | Change::Note(key) => f.write_str(key),
        }
    }
}

/// Errors from building, writing or removing a menu entry.
#[derive(Debug, thiserror::Error)]
pub enum IntegrationError {
    #[error("invalid executable path {path:?}: {reason}")]
    InvalidExecutable { path: PathBuf, reason: &'static str },
    #[error(
        "{path} exists and was not written by MorphRing; move it away and run the command again"
    )]
    Unmanaged { path: PathBuf },
    #[error("could not write {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("context menu integration is not supported on this operating system")]
    Unsupported,
}

/// Adds the menu entry starting `exe` for the user whose home folder is
/// `home`, and returns what changed. With `dry_run`, nothing is written and
/// the changes that would be made are returned.
pub fn install(exe: &Path, home: &Path, dry_run: bool) -> Result<Vec<Change>, IntegrationError> {
    backend::install(exe, home, dry_run)
}

/// Removes the menu entry for the user whose home folder is `home`, and
/// returns what changed. Only entries MorphRing wrote are removed. With
/// `dry_run`, nothing is removed and the changes that would be made are
/// returned.
pub fn uninstall(home: &Path, dry_run: bool) -> Result<Vec<Change>, IntegrationError> {
    backend::uninstall(home, dry_run)
}

// Exactly one `backend` survives cfg, so every OS sees plain function calls
// and no backend ever has to pretend to be another.
#[cfg(windows)]
use crate::explorer as backend;
#[cfg(target_os = "macos")]
use crate::finder as backend;

#[cfg(target_os = "linux")]
mod backend {
    use std::path::Path;

    use super::{Change, IntegrationError};
    use crate::context_menu;

    pub fn install(
        exe: &Path,
        home: &Path,
        dry_run: bool,
    ) -> Result<Vec<Change>, IntegrationError> {
        let files = context_menu::install(exe, home, dry_run)?;
        Ok(files
            .into_iter()
            .map(|file| Change::File(file.path))
            .collect())
    }

    pub fn uninstall(home: &Path, dry_run: bool) -> Result<Vec<Change>, IntegrationError> {
        let paths = context_menu::uninstall(home, dry_run)?;
        Ok(paths.into_iter().map(Change::File).collect())
    }
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
mod backend {
    use std::path::Path;

    use super::{Change, IntegrationError};

    pub fn install(
        _exe: &Path,
        _home: &Path,
        _dry_run: bool,
    ) -> Result<Vec<Change>, IntegrationError> {
        Err(IntegrationError::Unsupported)
    }

    pub fn uninstall(_home: &Path, _dry_run: bool) -> Result<Vec<Change>, IntegrationError> {
        Err(IntegrationError::Unsupported)
    }
}

/// The names of the places the menu entry shows up on this OS, for messages
/// such as "now opens ... in Dolphin, Nautilus, Nemo and Thunar".
pub fn surfaces() -> Vec<&'static str> {
    if cfg!(target_os = "linux") {
        crate::context_menu::FileManager::ALL
            .iter()
            .map(|manager| manager.name())
            .collect()
    } else if cfg!(windows) {
        vec!["Explorer"]
    } else if cfg!(target_os = "macos") {
        vec!["Finder"]
    } else {
        Vec::new()
    }
}

/// The name of the place a change belongs to, such as `Dolphin`, when there
/// is one worth showing.
pub fn surface_of(change: &Change) -> Option<&'static str> {
    match change {
        Change::File(path) if cfg!(target_os = "linux") => {
            crate::context_menu::FileManager::of_path(path).map(|manager| manager.name())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_display_as_their_path_or_key() {
        let file = Change::File(Path::new("menus").join("morphring.desktop"));
        assert_eq!(
            file.to_string(),
            Path::new("menus")
                .join("morphring.desktop")
                .display()
                .to_string()
        );
        let key = Change::Registry(r"HKCU\Software\Classes\*\shell\MorphRing".into());
        assert_eq!(key.to_string(), r"HKCU\Software\Classes\*\shell\MorphRing");
    }

    #[test]
    fn registry_changes_have_no_surface() {
        assert_eq!(surface_of(&Change::Registry("HKCU".into())), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_install_and_uninstall_wrap_the_context_menu_files() {
        use crate::context_menu::{self, FileManager};

        let home = tempfile::TempDir::new().unwrap();
        let exe = Path::new("/usr/bin/morphring-app");
        let expected: Vec<Change> = context_menu::menu_files(exe, home.path())
            .unwrap()
            .into_iter()
            .map(|file| Change::File(file.path))
            .collect();

        let planned = install(exe, home.path(), true).unwrap();
        assert_eq!(planned, expected);
        assert!(std::fs::read_dir(home.path()).unwrap().next().is_none());
        assert!(uninstall(home.path(), false).unwrap().is_empty());

        assert_eq!(install(exe, home.path(), false).unwrap(), expected);
        let names: Vec<_> = expected.iter().map(surface_of).collect();
        let all: Vec<_> = FileManager::ALL.iter().map(|m| Some(m.name())).collect();
        assert_eq!(names, all);
        assert_eq!(surfaces(), ["Dolphin", "Nautilus", "Nemo", "Thunar"]);

        assert_eq!(uninstall(home.path(), true).unwrap(), expected);
        assert_eq!(uninstall(home.path(), false).unwrap(), expected);
        assert!(uninstall(home.path(), false).unwrap().is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_file_outside_the_menus_has_no_surface() {
        let change = Change::File("/home/u/.local/share/notes.txt".into());
        assert_eq!(surface_of(&change), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_install_and_uninstall_write_the_quick_action() {
        let home = tempfile::TempDir::new().unwrap();
        let exe = home.path().join("morphring-app");
        let changes = install(&exe, home.path(), true).unwrap();
        assert_eq!(
            changes,
            crate::finder::install(&exe, home.path(), true).unwrap()
        );
        assert!(matches!(changes.last(), Some(Change::Note(_))));
        assert!(uninstall(home.path(), false).unwrap().is_empty());
        assert_eq!(surfaces(), ["Finder"]);
    }
}
