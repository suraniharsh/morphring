//! The history window, its commands, and recording finished jobs.
//!
//! Stubs for now, apart from [`open`]: P2-53 fills in [`record`] and the
//! commands. The shapes are fixed here so the conversion queue can already
//! hand over its records.

use morphring_core::history::HistoryEntry;
use morphring_core::preset::Preset;
use morphring_core::{FileResult, JobState};
use tauri::{AppHandle, Runtime};

/// Label of the history window.
pub const HISTORY_WINDOW: &str = "history";

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

/// Adds a finished job to the history.
pub fn record<R: Runtime>(app: &AppHandle<R>, record: &JobRecord) {
    let _ = (app, record);
}

/// Shows the history window and brings it to the front.
pub fn open<R: Runtime>(app: &AppHandle<R>) {
    // Shared with the tray's own entry: show, focus, and rebuild the window
    // from the config if someone closed it for good.
    crate::tray::show_window(app, HISTORY_WINDOW);
}

/// The recorded jobs, newest first.
#[tauri::command]
pub fn history_list() -> Vec<HistoryEntry> {
    Vec::new()
}

/// Forgets every recorded job.
#[tauri::command]
pub fn history_clear() -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stubs_answer_with_defaults() {
        assert!(history_list().is_empty());
        assert_eq!(history_clear(), Ok(()));
    }
}
