//! The history window, its commands, and recording finished jobs.
//!
//! Every finished job lands in `history.json` in the config folder, the same
//! one the CLI uses. The real work lives in plain functions that take the
//! folder ([`record_in`], [`list_in`], [`clear_in`]), so tests can point them
//! at a temp dir instead of my actual history.

use std::io;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use morphring_core::history::{History, HistoryEntry};
use morphring_core::preset::Preset;
use morphring_core::settings::Store;
use morphring_core::{FileResult, JobState};
use tauri::{AppHandle, Emitter, Runtime};

/// Label of the history window.
pub const HISTORY_WINDOW: &str = "history";

/// App-wide event sent (no payload) once the history file has changed.
pub const HISTORY_CHANGED: &str = "history-changed";

/// Everything known about a job once it has ended: what history keeps.
#[derive(Debug, Clone, PartialEq)]
pub struct JobRecord {
    /// The job's id, as in the `job-*` events.
    pub job_id: u64,
    /// The preset the job ran with.
    pub preset: Preset,
    /// How the job ended.
    pub state: JobState,
    /// One result per source file, in input order.
    pub results: Vec<FileResult>,
}

// Load, change, save is three steps, and the worker thread records while the
// history window may be clearing. One lock, so neither eats the other's write.
static HISTORY_LOCK: Mutex<()> = Mutex::new(());

/// Adds a finished job to the history, then tells the windows about it.
/// A failure to write is logged and otherwise shrugged off: losing one
/// history line is no reason to bother anyone mid-conversion.
pub fn record<R: Runtime>(app: &AppHandle<R>, record: &JobRecord) {
    match record_in(crate::settings::config_dir(app), record) {
        Ok(()) => {
            if let Err(e) = app.emit(HISTORY_CHANGED, ()) {
                tracing::warn!("could not announce the history change: {e}");
            }
        }
        Err(e) => tracing::warn!("could not record job {} in history: {e}", record.job_id),
    }
}

/// [`record`] without the app: writes one entry into the store at `dir`,
/// honouring `keepHistory` from the `settings.json` found there.
pub fn record_in(dir: Option<PathBuf>, record: &JobRecord) -> io::Result<()> {
    let store = Store::new(dir);
    // Read straight from disk rather than from app state, so this works no
    // matter who else is holding the settings at the time.
    let settings = store.load_settings();
    let entry = HistoryEntry::from_results(
        record.job_id.to_string(),
        &record.preset,
        record.state,
        &record.results,
    );
    let _guard = HISTORY_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    History::load(&store).add(&store, &settings, entry)
}

/// The entries in the store at `dir`, newest first.
pub fn list_in(dir: Option<PathBuf>) -> Vec<HistoryEntry> {
    let _guard = HISTORY_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    History::load(&Store::new(dir)).entries
}

/// Empties the history in the store at `dir`.
pub fn clear_in(dir: Option<PathBuf>) -> io::Result<()> {
    let store = Store::new(dir);
    let _guard = HISTORY_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    History::load(&store).clear(&store)
}

/// Shows the history window and brings it to the front.
pub fn open<R: Runtime>(app: &AppHandle<R>) {
    // Shared with the tray's own entry: show, focus, and rebuild the window
    // from the config if someone closed it for good.
    crate::tray::show_window(app, HISTORY_WINDOW);
}

/// The recorded jobs, newest first.
#[tauri::command]
pub fn history_list(app: AppHandle) -> Vec<HistoryEntry> {
    list_in(crate::settings::config_dir(&app))
}

/// Forgets every recorded job.
#[tauri::command]
pub fn history_clear(app: AppHandle) -> Result<(), String> {
    clear_in(crate::settings::config_dir(&app))
        .map_err(|e| format!("Could not clear the history: {e}"))?;
    if let Err(e) = app.emit(HISTORY_CHANGED, ()) {
        tracing::warn!("could not announce the history change: {e}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use morphring_core::FileState;
    use morphring_core::history::HISTORY_FILE;
    use morphring_core::settings::{AppSettings, SETTINGS_FILE};
    use tempfile::TempDir;

    fn job(id: u64) -> JobRecord {
        JobRecord {
            job_id: id,
            preset: Preset {
                id: "to-png".to_owned(),
                name: "PNG".to_owned(),
                ..Preset::default()
            },
            state: JobState::Completed,
            results: vec![FileResult {
                source: PathBuf::from("/in/a.jpg"),
                state: FileState::Done,
                outputs: vec![PathBuf::from("/in/a.png")],
            }],
        }
    }

    #[test]
    fn record_writes_one_entry() {
        let tmp = TempDir::new().unwrap();
        record_in(Some(tmp.path().to_path_buf()), &job(7)).unwrap();
        assert!(tmp.path().join(HISTORY_FILE).is_file());

        let entries = list_in(Some(tmp.path().to_path_buf()));
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.job_id, "7");
        assert_eq!(e.preset_id, "to-png");
        assert_eq!(e.state, JobState::Completed);
        assert_eq!(e.sources, [PathBuf::from("/in/a.jpg")]);
        assert_eq!(e.outputs, [PathBuf::from("/in/a.png")]);
    }

    #[test]
    fn newest_first_and_clear_forgets() {
        let tmp = TempDir::new().unwrap();
        let dir = Some(tmp.path().to_path_buf());
        record_in(dir.clone(), &job(1)).unwrap();
        record_in(dir.clone(), &job(2)).unwrap();
        let ids: Vec<_> = list_in(dir.clone()).into_iter().map(|e| e.job_id).collect();
        assert_eq!(ids, ["2", "1"]);

        clear_in(dir.clone()).unwrap();
        assert!(list_in(dir).is_empty());
    }

    #[test]
    fn keep_history_off_is_respected() {
        let tmp = TempDir::new().unwrap();
        let dir = Some(tmp.path().to_path_buf());
        let settings = AppSettings {
            keep_history: false,
            ..AppSettings::default()
        };
        Store::new(dir.clone()).save_settings(&settings).unwrap();
        assert!(tmp.path().join(SETTINGS_FILE).is_file());

        record_in(dir.clone(), &job(1)).unwrap();
        assert!(!tmp.path().join(HISTORY_FILE).exists());
        assert!(list_in(dir).is_empty());
    }

    #[test]
    fn no_config_dir_is_fine() {
        record_in(None, &job(1)).unwrap();
        assert!(list_in(None).is_empty());
        clear_in(None).unwrap();
    }
}
