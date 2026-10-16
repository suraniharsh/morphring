//! App lifecycle hooks: the setup step and the run-event loop.
//!
//! This is where macOS gets its manners (docs/research/macos-support.md,
//! sections 3 to 5):
//!
//! - Finder, the Dock and `open -a MorphRing file` hand files over as
//!   `RunEvent::Opened` with `file://` URLs, never through argv. They take the
//!   same road as a second `--convert` launch: the debouncer, then the ring.
//! - On a cold start those events can beat `setup`, so they wait in
//!   [`PendingOpen`] until the ring window exists.
//! - A `--background` launch starts as an Accessory app: no Dock icon, no
//!   Cmd-Tab entry, just the menu-bar icon.
//! - Clicking the Dock icon brings the main window back, and closing the last
//!   window does not quit while the tray is there to reopen it.

use std::error::Error;
use std::path::PathBuf;
use std::sync::Mutex;

use tauri::{App, AppHandle, Manager, RunEvent, Runtime, Url};

use crate::commands::RING_WINDOW;
use crate::dropzone::MAIN_WINDOW;

/// Files the OS asked us to open before the app was ready to show a ring.
/// Managed app state; filled and drained by the platform lifecycle code.
#[derive(Debug, Default)]
pub struct PendingOpen(pub Mutex<Vec<PathBuf>>);

impl PendingOpen {
    /// Queues `files` until [`PendingOpen::take`] is called.
    pub fn push(&self, files: impl IntoIterator<Item = PathBuf>) {
        match self.0.lock() {
            Ok(mut queue) => queue.extend(files),
            Err(e) => tracing::warn!("could not queue opened files: {e}"),
        }
    }

    /// Takes everything queued so far, in arrival order. `None` when the queue
    /// is empty, so callers don't open a ring for nothing.
    pub fn take(&self) -> Option<Vec<PathBuf>> {
        match self.0.lock() {
            Ok(mut queue) => drain(&mut queue),
            Err(e) => {
                tracing::warn!("could not read the opened files: {e}");
                None
            }
        }
    }
}

/// Empties `queue` and returns what was in it, or `None` when it was empty.
pub fn drain(queue: &mut Vec<PathBuf>) -> Option<Vec<PathBuf>> {
    (!queue.is_empty()).then(|| std::mem::take(queue))
}

/// The local path behind a `file://` URL, or `None` for any other scheme or a
/// URL that names no local path (a `file://server/share` on Unix, say).
pub fn file_url_to_path(url: &Url) -> Option<PathBuf> {
    if url.scheme() != "file" {
        return None;
    }
    // `to_file_path` undoes the percent-encoding too, so "My%20Cat.png" comes
    // back with its space where Finder found it.
    url.to_file_path().ok()
}

/// The files among `urls` that the ring can take: local, existing, and not a
/// folder. Order is kept; duplicates are the debouncer's job.
pub fn opened_files(urls: &[Url]) -> Vec<PathBuf> {
    // Folders are left out just like the drop zone and `--convert` leave them out.
    urls.iter()
        .filter_map(file_url_to_path)
        .filter(|path| path.is_file())
        .collect()
}

/// Runs at the end of the app's setup hook, once every other part is set up.
pub fn setup(app: &mut App) -> Result<(), Box<dyn Error>> {
    #[cfg(target_os = "macos")]
    if background_launch() {
        // Before the event loop runs, so the Dock icon never even bounces.
        app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    }
    // Whatever Finder sent while we were still getting dressed goes to the ring now.
    let pending = app.state::<PendingOpen>().take();
    if let Some(files) = pending {
        open_cold(app.handle(), files);
    }
    Ok(())
}

/// Whether this process was started with `--background` and nothing that
/// overrides it.
#[cfg(target_os = "macos")]
fn background_launch() -> bool {
    use crate::instance::{CommandLine, Launch};
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cwd = std::env::current_dir().unwrap_or_default();
    Launch::of(&CommandLine::parse(&args, &cwd), true) == Launch::Background
}

/// Called for every event of the app's run loop.
pub fn on_run_event(app: &AppHandle, event: &RunEvent) {
    match event {
        #[cfg(target_os = "macos")]
        RunEvent::Opened { urls } => on_opened(app, urls),
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => crate::tray::show_main(app),
        RunEvent::WindowEvent {
            label,
            event: tauri::WindowEvent::Destroyed,
            ..
        } if label == MAIN_WINDOW => crate::tray::main_window_closed(app),
        // `code: None` means the last window went away, not that someone picked
        // Quit (that one is `app.exit(0)`, with a code). macOS apps outlive their
        // windows, and with a tray icon to bring them back so do we.
        RunEvent::ExitRequested {
            code: None, api, ..
        } if app.tray_by_id(crate::tray::TRAY_ID).is_some() => api.prevent_exit(),
        _ => {}
    }
}

#[cfg(target_os = "macos")]
fn on_opened(app: &AppHandle, urls: &[Url]) {
    let files = opened_files(urls);
    tracing::debug!(
        urls = urls.len(),
        files = files.len(),
        "files opened by the OS"
    );
    if files.is_empty() {
        return;
    }
    if app.get_webview_window(RING_WINDOW).is_none() {
        // Cold start: LaunchServices is faster than our setup hook. Park the files
        // until `setup` drains them; there is no ring to show them on yet.
        app.state::<PendingOpen>().push(files);
    } else {
        open_files(app, files);
    }
}

/// Sends `files` down the road a second `--convert` launch takes: the
/// debouncer first, then the ring.
pub fn open_files<R: Runtime>(app: &AppHandle<R>, files: Vec<PathBuf>) {
    // Reusing the second-instance handler with a made-up argv keeps one code path
    // for "files arrived from outside". The paths are absolute, so the cwd is moot.
    crate::instance::on_second_instance(app, convert_argv(files), "/".into());
}

/// Like [`open_files`], for files that arrived before the app was set up: the
/// main window steps aside first, just as for a `--convert` cold start.
fn open_cold<R: Runtime>(app: &AppHandle<R>, files: Vec<PathBuf>) {
    if let Some(main) = app.get_webview_window(MAIN_WINDOW) {
        if let Err(e) = main.hide() {
            tracing::debug!("could not hide the main window: {e}");
        }
    }
    crate::tray::main_window_closed(app);
    if app.get_webview_window(RING_WINDOW).is_some() {
        open_files(app, files);
    } else {
        tracing::warn!("the ring window is missing; cannot open the ring");
    }
}

/// The argv of a pretend `morphring-app --convert <files>` launch.
fn convert_argv(files: Vec<PathBuf>) -> Vec<String> {
    ["morphring-app", "--convert"]
        .into_iter()
        .map(String::from)
        .chain(files.into_iter().map(|p| p.to_string_lossy().into_owned()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    use crate::instance::{CommandLine, Launch};

    fn touch(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, b"x").unwrap();
        path
    }

    #[test]
    fn file_urls_round_trip_to_paths() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("보고서 #1 100%.png");
        let url = Url::from_file_path(&path).unwrap();
        assert_eq!(file_url_to_path(&url), Some(path));
    }

    #[test]
    fn percent_encoding_is_undone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("My Cat.png");
        let url = Url::from_file_path(&path).unwrap();
        assert!(url.as_str().ends_with("My%20Cat.png"), "{url}");
        assert_eq!(file_url_to_path(&url), Some(path));
    }

    #[test]
    fn other_schemes_are_not_paths() {
        for url in ["https://example.com/a.png", "mailto:a@b.c", "data:,hi"] {
            assert_eq!(file_url_to_path(&Url::parse(url).unwrap()), None, "{url}");
        }
    }

    #[test]
    fn opened_files_keeps_existing_files_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let b = touch(dir.path(), "b.png");
        let a = touch(dir.path(), "a.wav");
        let folder = dir.path().join("album");
        fs::create_dir(&folder).unwrap();
        let urls = [
            Url::from_file_path(&b).unwrap(),
            Url::from_directory_path(&folder).unwrap(),
            Url::from_file_path(dir.path().join("gone.mp4")).unwrap(),
            Url::parse("https://example.com/c.png").unwrap(),
            Url::from_file_path(&a).unwrap(),
        ];
        assert_eq!(opened_files(&urls), vec![b, a]);
    }

    #[test]
    fn drain_empties_the_queue_once() {
        let mut queue = vec![PathBuf::from("/a"), PathBuf::from("/b")];
        assert_eq!(
            drain(&mut queue),
            Some(vec![PathBuf::from("/a"), PathBuf::from("/b")])
        );
        assert!(queue.is_empty());
        assert_eq!(drain(&mut queue), None);
    }

    #[test]
    fn pending_open_collects_until_taken() {
        let pending = PendingOpen::default();
        assert_eq!(pending.take(), None);
        pending.push([PathBuf::from("/a")]);
        pending.push([PathBuf::from("/b"), PathBuf::from("/a")]);
        // Duplicates stay: the debouncer downstream already folds them.
        assert_eq!(
            pending.take(),
            Some(vec![
                PathBuf::from("/a"),
                PathBuf::from("/b"),
                PathBuf::from("/a")
            ])
        );
        assert_eq!(pending.take(), None);
    }

    #[test]
    fn convert_argv_parses_back_to_the_same_files() {
        let dir = tempfile::tempdir().unwrap();
        let a = touch(dir.path(), "a b.png");
        let c = touch(dir.path(), "--c.png");
        let argv = convert_argv(vec![a.clone(), c.clone()]);
        let cmd = CommandLine::parse(&argv[1..], Path::new("/"));
        // A file named like an option still goes through: its path is absolute,
        // so it starts with a slash (or a drive letter), never with "--".
        assert_eq!(cmd.convert, vec![a.clone(), c.clone()]);
        assert_eq!(Launch::of(&cmd, false), Launch::Convert(vec![a, c]));
    }

    #[test]
    fn macos_config_turns_on_private_api_and_never_claims_defaults() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.macos.conf.json")).unwrap();
        assert_eq!(config["app"]["macOSPrivateApi"], true);
        let associations = config["bundle"]["fileAssociations"].as_array().unwrap();
        assert!(!associations.is_empty());
        for assoc in associations {
            // Viewer + Alternate: we show up under "Open With" and never steal the
            // double click from Preview or QuickTime.
            assert_eq!(assoc["role"], "Viewer", "{assoc}");
            assert_eq!(assoc["rank"], "Alternate", "{assoc}");
            assert!(!assoc["ext"].as_array().unwrap().is_empty(), "{assoc}");
        }
        // tauri-build compares the `tauri` features against the config of the OS
        // it builds on, so the base config and the dependency must both say yes.
        let base: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(base["app"]["macOSPrivateApi"], true);
        let manifest = include_str!("../Cargo.toml");
        assert!(manifest.contains(r#"features = ["tray-icon", "macos-private-api"]"#));
    }
}
