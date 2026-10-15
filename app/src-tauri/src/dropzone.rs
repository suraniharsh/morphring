//! The main window's drop zone. Tauri reports every drag over the window as a
//! window event with real file paths; this module keeps the files worth
//! converting and tells the frontend about them.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{DragDropEvent, Emitter, Manager, Runtime, Window, WindowEvent};

/// Label of the window that owns the drop zone.
pub const MAIN_WINDOW: &str = "main";

/// Event sent to the main window after a drop, with a [`DroppedFiles`] payload.
pub const FILES_DROPPED: &str = "files-dropped";

/// What a drop left us with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accepted {
    /// Existing regular files, in drop order, each listed once.
    pub files: Vec<PathBuf>,
    /// Folders, missing paths and duplicates that were left out.
    pub ignored: usize,
}

/// Payload of [`FILES_DROPPED`].
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DroppedFiles {
    /// Dropped file paths, for display.
    pub files: Vec<String>,
    /// How many dropped items were left out.
    pub ignored: usize,
}

/// Keeps the dropped paths that are existing regular files.
///
/// Folders and paths that no longer exist are left out, as are repeats, so
/// every file appears once, in the order it was dropped.
pub fn accept_drop<P: AsRef<Path>>(paths: &[P]) -> Accepted {
    // Filee ignores folders in the drop zone too: folders only make sense for
    // archive presets, and those don't exist yet. One less way to accidentally
    // convert someone's entire Pictures directory.
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    for path in paths {
        let path = path.as_ref();
        if path.is_file() && seen.insert(path.to_path_buf()) {
            files.push(path.to_path_buf());
        }
    }
    Accepted {
        ignored: paths.len() - files.len(),
        files,
    }
}

impl From<&Accepted> for DroppedFiles {
    fn from(drop: &Accepted) -> Self {
        Self {
            // Display only, so a lossy conversion is fine; the real PathBufs stay
            // in Rust when T35 hands them to the job runner.
            files: drop
                .files
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
            ignored: drop.ignored,
        }
    }
}

/// Window event hook: turns drops on the main window into [`FILES_DROPPED`].
pub fn on_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    if window.label() != MAIN_WINDOW {
        return;
    }
    // Enter, Over and Leave are the frontend's business (it does the hover glow).
    // We only care about the moment the files actually land.
    let WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) = event else {
        return;
    };
    let drop = accept_drop(paths);
    tracing::debug!(
        files = drop.files.len(),
        ignored = drop.ignored,
        "drop received"
    );
    if let Err(e) = window.emit_to(MAIN_WINDOW, FILES_DROPPED, DroppedFiles::from(&drop)) {
        tracing::warn!("could not send the dropped files to the window: {e}");
    }
    // Filee's drop zone opens the ring for what landed, and so does ours.
    crate::commands::open_ring(window.app_handle(), drop.files);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, b"x").unwrap();
        path
    }

    #[test]
    fn keeps_files_in_drop_order() {
        let dir = tempfile::tempdir().unwrap();
        let b = touch(dir.path(), "b.png");
        let a = touch(dir.path(), "a.wav");
        let drop = accept_drop(&[&b, &a]);
        assert_eq!(drop.files, vec![b, a]);
        assert_eq!(drop.ignored, 0);
    }

    #[test]
    fn ignores_folders_and_missing_paths() {
        let dir = tempfile::tempdir().unwrap();
        let file = touch(dir.path(), "photo.jpg");
        let folder = dir.path().join("album");
        fs::create_dir(&folder).unwrap();
        let missing = dir.path().join("gone.mp4");
        let drop = accept_drop(&[folder, file.clone(), missing]);
        assert_eq!(drop.files, vec![file]);
        assert_eq!(drop.ignored, 2);
    }

    #[test]
    fn lists_a_file_dropped_twice_once() {
        let dir = tempfile::tempdir().unwrap();
        let file = touch(dir.path(), "song.flac");
        let drop = accept_drop(&[&file, &file]);
        assert_eq!(drop.files, vec![file]);
        assert_eq!(drop.ignored, 1);
    }

    #[test]
    fn keeps_unicode_and_spaces_in_names() {
        let dir = tempfile::tempdir().unwrap();
        let file = touch(dir.path(), "보고서 \"final\" v2.docx");
        let drop = accept_drop(&[&file]);
        assert_eq!(drop.files, vec![file.clone()]);
        let payload = DroppedFiles::from(&drop);
        assert_eq!(payload.files, vec![file.to_string_lossy().into_owned()]);
    }

    #[test]
    fn empty_drop_is_empty() {
        let drop = accept_drop::<PathBuf>(&[]);
        assert!(drop.files.is_empty());
        assert_eq!(drop.ignored, 0);
    }

    #[test]
    fn payload_is_camel_case_json() {
        let payload = DroppedFiles {
            files: vec!["/tmp/a.png".into()],
            ignored: 1,
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "files": ["/tmp/a.png"], "ignored": 1 })
        );
    }
}
