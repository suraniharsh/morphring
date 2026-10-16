//! Single instance and `--convert <files>` handling.
//!
//! File managers launch `morphring-app --convert %F`. The first launch keeps
//! running; every later launch hands its argv to it through
//! `tauri-plugin-single-instance` and exits. Files from launches that arrive
//! within [`DEBOUNCE`] of each other are collected, then the ring opens once
//! for all of them.
//!
//! Ported from Filee's `src/Filee.App/Services/CommandLine.cs` and
//! `HandleCommandLine` / `OpenPendingFiles` in `src/Filee.App/App.axaml.cs`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime};

use crate::dropzone::MAIN_WINDOW;

/// How long to wait for more `--convert` launches before opening the ring.
pub const DEBOUNCE: Duration = Duration::from_millis(350);

/// Event sent to the ring window, with a [`RingFiles`] payload, right before
/// it is shown for files that came from the command line.
pub const RING_FILES: &str = "ring-files";

/// Payload of [`RING_FILES`].
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RingFiles {
    /// Absolute paths of the files to convert, each listed once.
    pub files: Vec<String>,
}

/// The options of one launch that the app acts on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandLine {
    /// `--background`: start in the tray without showing the main window.
    pub background: bool,
    /// `--settings`: show the main window.
    pub settings: bool,
    /// `--quit`: ask the running app to exit.
    pub quit: bool,
    /// Files after `--convert`, made absolute, in the order given.
    pub convert: Vec<PathBuf>,
}

impl CommandLine {
    /// Parses `args` (without the program name). Relative paths are resolved
    /// against `cwd`, the working directory of the launch they came from.
    ///
    /// Options are case-insensitive and may be written `--name=value`.
    /// Unknown options are ignored. After `--convert`, every argument that is
    /// not an option and names an existing file is kept; folders and missing
    /// paths are left out.
    pub fn parse<S: AsRef<str>>(args: &[S], cwd: &Path) -> CommandLine {
        let mut cmd = CommandLine::default();
        let mut collecting = false;
        for arg in args {
            let arg = arg.as_ref();
            // `--Name=value` is matched by its name alone. We take no option values
            // yet, but an installer from the future may well pass some.
            let name = match arg.split_once('=') {
                Some((name, _)) if arg.starts_with("--") => name,
                _ => arg,
            }
            .to_ascii_lowercase();
            match name.as_str() {
                "--background" => cmd.background = true,
                "--settings" => cmd.settings = true,
                "--quit" => cmd.quit = true,
                "--convert" => collecting = true,
                // Filee also has --convert-list, but only its Windows Explorer menu writes
                // those list files. TODO(v0.4): port it with the Windows menu.
                _ if collecting && !arg.starts_with("--") => {
                    if let Some(path) = existing_file(arg, cwd) {
                        cmd.convert.push(path);
                    }
                }
                // Anything else is an option from a newer build or a file before
                // --convert. Filee shrugs at both, so older builds survive newer menus.
                _ => {}
            }
        }
        cmd
    }
}

fn existing_file(arg: &str, cwd: &Path) -> Option<PathBuf> {
    // `join` keeps an absolute arg as it is. `absolute` tidies `./` without touching
    // symlinks, so the ring shows the path the file manager actually used.
    let path = std::path::absolute(cwd.join(arg)).ok()?;
    // Filee would also take folders here, but the drop zone skips them and so do
    // we: no folder preset exists yet, and a ring with nothing to offer is just sad.
    path.is_file().then_some(path)
}

/// What a launch should make the app do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// Exit the app.
    Quit,
    /// Collect these files and open the ring for them.
    Convert(Vec<PathBuf>),
    /// Bring the main window to the front.
    ShowMain,
    /// Keep the main window hidden; the tray is all there is.
    Background,
}

impl Launch {
    /// Decides what `cmd` means. `first_launch` is true for the process that
    /// started the app, false for a launch forwarded from a second process.
    pub fn of(cmd: &CommandLine, first_launch: bool) -> Launch {
        if cmd.quit {
            Launch::Quit
        } else if !cmd.convert.is_empty() {
            Launch::Convert(cmd.convert.clone())
        } else if first_launch && cmd.background && !cmd.settings {
            Launch::Background
        } else {
            // A second launch without files always shows the window: someone
            // clicked the app icon and expects to see something.
            Launch::ShowMain
        }
    }
}

/// Collects files from `--convert` launches until none has arrived for
/// [`DEBOUNCE`].
///
/// The clock is passed in as `now` on every call, so tests never sleep.
#[derive(Debug, Default)]
pub struct Debouncer {
    pending: Vec<PathBuf>,
    deadline: Option<Instant>,
}

impl Debouncer {
    /// Adds `files` and restarts the wait. Returns the new deadline.
    pub fn push(&mut self, files: impl IntoIterator<Item = PathBuf>, now: Instant) -> Instant {
        // Every arrival restarts the timer, like Filee disposing its DispatcherTimer.
        // A file manager that starts one process per selected file keeps us
        // waiting until the last one has spoken.
        self.pending.extend(files);
        let deadline = now + DEBOUNCE;
        self.deadline = Some(deadline);
        deadline
    }

    /// When the collected files are due, or `None` when nothing is waiting.
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// Takes the collected files once the deadline has passed: each listed
    /// once, in arrival order. Returns `None` while still waiting, or when
    /// nothing was collected.
    pub fn take_due(&mut self, now: Instant) -> Option<Vec<PathBuf>> {
        let deadline = self.deadline?;
        if now < deadline {
            return None;
        }
        self.deadline = None;
        let mut seen = HashSet::new();
        let files: Vec<PathBuf> = self
            .pending
            .drain(..)
            .filter(|p| seen.insert(p.clone()))
            .collect();
        (!files.is_empty()).then_some(files)
    }
}

/// The app-wide debouncer, kept in Tauri's state.
#[derive(Default)]
struct Pending(Mutex<Debouncer>);

/// Called with the argv of this process once the app is set up.
pub fn on_startup<R: Runtime>(app: &AppHandle<R>, argv: Vec<String>) {
    let cwd = std::env::current_dir().unwrap_or_default();
    let cmd = CommandLine::parse(argv.get(1..).unwrap_or_default(), &cwd);
    match Launch::of(&cmd, true) {
        Launch::Quit => app.exit(0),
        Launch::Convert(files) => {
            // The ring is the whole point of this launch; the main window would
            // only get in its way.
            hide_main(app);
            enqueue(app, files);
        }
        Launch::Background => hide_main(app),
        // With Ctrl+drag running the app lives in the tray, out of the way; the
        // tray's Open brings the window back. Without it, the drop zone is the
        // only way in, so show it.
        Launch::ShowMain => {
            if !crate::drag_ring::gesture_active(app) {
                crate::tray::show_main(app);
            }
        }
    }
}

/// Called in the running instance when someone launches the app a second time.
pub fn on_second_instance<R: Runtime>(app: &AppHandle<R>, argv: Vec<String>, cwd: String) {
    tracing::debug!(?argv, %cwd, "second instance started");
    // The relative paths in argv are relative to where *that* process was
    // started, not to us. A file manager usually passes absolute ones anyway,
    // but a terminal user typing `morphring-app --convert cat.png` does not.
    let cmd = CommandLine::parse(argv.get(1..).unwrap_or_default(), Path::new(&cwd));
    match Launch::of(&cmd, false) {
        Launch::Quit => app.exit(0),
        Launch::Convert(files) => enqueue(app, files),
        Launch::ShowMain | Launch::Background => crate::tray::show_main(app),
    }
}

fn enqueue<R: Runtime>(app: &AppHandle<R>, files: Vec<PathBuf>) {
    // The forwarded launch can race the setup hook, so the state is created on
    // first use; `manage` ignores the second attempt if both get here.
    if app.try_state::<Pending>().is_none() {
        app.manage(Pending::default());
    }
    let deadline = match app.state::<Pending>().0.lock() {
        Ok(mut pending) => pending.push(files, Instant::now()),
        Err(e) => {
            tracing::warn!("could not queue files for the ring: {e}");
            return;
        }
    };
    // One sleeper per launch. All but the last wake up early, find the deadline
    // moved and go back to whatever threads do when they are not needed.
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
        let due = match app.state::<Pending>().0.lock() {
            Ok(mut pending) => pending.take_due(Instant::now()),
            Err(_) => None,
        };
        if let Some(files) = due {
            let handle = app.clone();
            if let Err(e) = app.run_on_main_thread(move || open_ring(&handle, files)) {
                tracing::warn!("could not open the ring: {e}");
            }
        }
    });
}

/// Sends `files` to the ring window and shows it.
pub fn open_ring<R: Runtime>(app: &AppHandle<R>, files: Vec<PathBuf>) {
    // The ring model (slices, reasons) is built next to the conversion queue,
    // which knows the engines; this module only decides *when* the ring opens.
    crate::commands::open_ring(app, files);
}

fn hide_main<R: Runtime>(app: &AppHandle<R>) {
    // Let chains would read nicer, but they need Rust 1.88 and we promise 1.85.
    if let Some(main) = app.get_webview_window(MAIN_WINDOW) {
        if let Err(e) = main.hide() {
            tracing::debug!("could not hide the main window: {e}");
        }
    }
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

    fn parse(args: &[&str], cwd: &Path) -> CommandLine {
        CommandLine::parse(args, cwd)
    }

    #[test]
    fn no_args_means_nothing_special() {
        let cmd = parse(&[], Path::new("/"));
        assert_eq!(cmd, CommandLine::default());
    }

    #[test]
    fn convert_keeps_existing_files_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let b = touch(dir.path(), "b.png");
        let a = touch(dir.path(), "a.wav");
        let cmd = parse(
            &["--convert", b.to_str().unwrap(), a.to_str().unwrap()],
            Path::new("/"),
        );
        assert_eq!(cmd.convert, vec![b, a]);
    }

    #[test]
    fn convert_skips_folders_missing_paths_and_options() {
        let dir = tempfile::tempdir().unwrap();
        let file = touch(dir.path(), "photo.jpg");
        let folder = dir.path().join("album");
        fs::create_dir(&folder).unwrap();
        let missing = dir.path().join("gone.mp4");
        let cmd = parse(
            &[
                "--convert",
                folder.to_str().unwrap(),
                "--from-the-future",
                missing.to_str().unwrap(),
                file.to_str().unwrap(),
            ],
            Path::new("/"),
        );
        assert_eq!(cmd.convert, vec![file]);
    }

    #[test]
    fn files_before_convert_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let early = touch(dir.path(), "early.png");
        let late = touch(dir.path(), "late.png");
        let cmd = parse(
            &[early.to_str().unwrap(), "--convert", late.to_str().unwrap()],
            Path::new("/"),
        );
        assert_eq!(cmd.convert, vec![late]);
    }

    #[test]
    fn relative_paths_resolve_against_the_launch_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let file = touch(dir.path(), "cat.png");
        let cmd = parse(&["--convert", "./cat.png"], dir.path());
        assert_eq!(cmd.convert, vec![file]);
        assert!(cmd.convert[0].is_absolute());
    }

    #[test]
    fn unicode_spaces_and_quotes_survive() {
        let dir = tempfile::tempdir().unwrap();
        // Windows forbids " in file names, so it gets curly quotes, which it allows.
        let name = if cfg!(windows) {
            "보고서 “final” v2.docx"
        } else {
            "보고서 \"final\" v2.docx"
        };
        let file = touch(dir.path(), name);
        let cmd = parse(&["--convert", file.to_str().unwrap()], Path::new("/"));
        assert_eq!(cmd.convert, vec![file]);
    }

    #[test]
    fn options_are_case_insensitive_and_take_equals_values() {
        let cmd = parse(
            &["--BACKGROUND", "--Settings=yes", "--quit"],
            Path::new("/"),
        );
        assert!(cmd.background);
        assert!(cmd.settings);
        assert!(cmd.quit);
    }

    #[test]
    fn convert_with_equals_still_starts_collecting() {
        let dir = tempfile::tempdir().unwrap();
        let file = touch(dir.path(), "a.png");
        let cmd = parse(&["--Convert=", file.to_str().unwrap()], Path::new("/"));
        assert_eq!(cmd.convert, vec![file]);
    }

    #[test]
    fn launch_quit_wins_over_everything() {
        let cmd = CommandLine {
            quit: true,
            convert: vec![PathBuf::from("/a.png")],
            ..Default::default()
        };
        assert_eq!(Launch::of(&cmd, true), Launch::Quit);
        assert_eq!(Launch::of(&cmd, false), Launch::Quit);
    }

    #[test]
    fn launch_with_files_converts() {
        let cmd = CommandLine {
            background: true,
            convert: vec![PathBuf::from("/a.png")],
            ..Default::default()
        };
        assert_eq!(
            Launch::of(&cmd, true),
            Launch::Convert(vec![PathBuf::from("/a.png")])
        );
        assert_eq!(
            Launch::of(&cmd, false),
            Launch::Convert(vec![PathBuf::from("/a.png")])
        );
    }

    #[test]
    fn background_only_counts_on_the_first_launch() {
        let cmd = CommandLine {
            background: true,
            ..Default::default()
        };
        assert_eq!(Launch::of(&cmd, true), Launch::Background);
        assert_eq!(Launch::of(&cmd, false), Launch::ShowMain);
    }

    #[test]
    fn settings_overrides_background() {
        let cmd = CommandLine {
            background: true,
            settings: true,
            ..Default::default()
        };
        assert_eq!(Launch::of(&cmd, true), Launch::ShowMain);
    }

    #[test]
    fn plain_launches_show_the_main_window() {
        let cmd = CommandLine::default();
        assert_eq!(Launch::of(&cmd, true), Launch::ShowMain);
        assert_eq!(Launch::of(&cmd, false), Launch::ShowMain);
    }

    #[test]
    fn empty_debouncer_has_nothing_due() {
        let mut d = Debouncer::default();
        assert_eq!(d.deadline(), None);
        assert_eq!(d.take_due(Instant::now()), None);
    }

    #[test]
    fn files_are_due_exactly_350_ms_after_the_push() {
        let t0 = Instant::now();
        let mut d = Debouncer::default();
        assert_eq!(d.push([PathBuf::from("/a")], t0), t0 + DEBOUNCE);
        assert_eq!(d.take_due(t0 + Duration::from_millis(349)), None);
        assert_eq!(
            d.take_due(t0 + Duration::from_millis(350)),
            Some(vec![PathBuf::from("/a")])
        );
        // Taken means gone: the ring opens once per batch.
        assert_eq!(d.take_due(t0 + Duration::from_secs(5)), None);
        assert_eq!(d.deadline(), None);
    }

    #[test]
    fn every_push_restarts_the_wait() {
        let t0 = Instant::now();
        let mut d = Debouncer::default();
        d.push([PathBuf::from("/a")], t0);
        d.push([PathBuf::from("/b")], t0 + Duration::from_millis(300));
        // The first deadline has passed, but the second launch moved it.
        assert_eq!(d.take_due(t0 + Duration::from_millis(400)), None);
        assert_eq!(
            d.take_due(t0 + Duration::from_millis(650)),
            Some(vec![PathBuf::from("/a"), PathBuf::from("/b")])
        );
    }

    #[test]
    fn duplicates_collapse_in_arrival_order() {
        let t0 = Instant::now();
        let mut d = Debouncer::default();
        d.push([PathBuf::from("/b"), PathBuf::from("/a")], t0);
        d.push([PathBuf::from("/a"), PathBuf::from("/c")], t0);
        assert_eq!(
            d.take_due(t0 + DEBOUNCE),
            Some(vec![
                PathBuf::from("/b"),
                PathBuf::from("/a"),
                PathBuf::from("/c")
            ])
        );
    }

    #[test]
    fn a_new_batch_after_a_take_starts_fresh() {
        let t0 = Instant::now();
        let mut d = Debouncer::default();
        d.push([PathBuf::from("/a")], t0);
        assert!(d.take_due(t0 + DEBOUNCE).is_some());
        let t1 = t0 + Duration::from_secs(1);
        d.push([PathBuf::from("/b")], t1);
        assert_eq!(d.take_due(t1), None);
        assert_eq!(d.take_due(t1 + DEBOUNCE), Some(vec![PathBuf::from("/b")]));
    }

    #[test]
    fn ring_payload_is_camel_case_json() {
        let payload = RingFiles {
            files: vec!["/tmp/a.png".into()],
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json, serde_json::json!({ "files": ["/tmp/a.png"] }));
    }
}
