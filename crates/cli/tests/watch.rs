//! `morphring watch`, end to end through `run_with()` on a fake engine.
//!
//! Ports Filee's `Watch_converts_files_that_arrive_until_cancelled` and
//! `Watch_needs_an_existing_folder` (`CliTests.cs`). The timeouts are roomy on
//! purpose: CI runners on macOS deliver FSEvents whenever they feel like it,
//! and a flaky watch test is worse than a slow one.

mod common;

use std::fs;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use morphring_core::testing::FakeConverter;
use morphring_core::{CancelToken, Converter};
use tempfile::TempDir;

use common::{Outcome, run_in, run_with_cancel};

/// How long a converted file may take to show up before we call it lost.
const ARRIVAL_TIMEOUT: Duration = Duration::from_secs(30);

fn png_to_jpg() -> Vec<Box<dyn Converter>> {
    vec![Box::new(FakeConverter::new("fake").edge("png", "jpg", 10))]
}

/// Runs `morphring <args>` on a thread, calls `during` once it's going, then
/// cancels and returns what the watch said.
fn watch_while(cwd: &Path, args: &[&str], during: impl FnOnce()) -> Outcome {
    let cancel = CancelToken::new();
    let worker = {
        let (cwd, cancel) = (cwd.to_path_buf(), cancel.clone());
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        thread::spawn(move || {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            run_with_cancel(&cwd, png_to_jpg(), &args, cancel)
        })
    };
    during();
    cancel.cancel();
    worker
        .join()
        .unwrap_or_else(|_| panic!("the watch thread panicked"))
}

/// Waits until `path` exists, rewriting `source` every few seconds in case
/// the first write happened before the watcher was listening (slow runners).
fn wait_for(path: &Path, source: &Path) -> bool {
    let start = Instant::now();
    let mut last_write = start;
    let mut generation = 1usize;
    while start.elapsed() < ARRIVAL_TIMEOUT {
        if path.is_file() {
            return true;
        }
        if last_write.elapsed() > Duration::from_secs(4) && !path.exists() {
            generation += 1;
            // A different size, so the settler sees a new file and not a
            // file it already handled.
            fs::write(source, vec![b'x'; generation])
                .unwrap_or_else(|e| panic!("rewrite the png: {e}"));
            last_write = Instant::now();
        }
        thread::sleep(Duration::from_millis(50));
    }
    path.is_file()
}

#[test]
fn watch_converts_files_that_arrive_until_cancelled() {
    let tmp = TempDir::new().unwrap();
    let folder = tmp.path().join("inbox");
    fs::create_dir(&folder).unwrap();
    let expected = folder.join("converted").join("arrived.jpg");

    let mut arrived = false;
    let r = watch_while(
        tmp.path(),
        &["watch", "inbox", "--to", "jpg", "--settle", "0.2"],
        || {
            thread::sleep(Duration::from_millis(500));
            let source = folder.join("arrived.png");
            fs::write(&source, b"x").unwrap();
            arrived = wait_for(&expected, &source);
        },
    );

    assert!(arrived, "no converted/arrived.jpg: {r:?}");
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(r.err, "", "{r:?}");
    assert!(r.out.starts_with("Watching "), "{r:?}");
    assert!(r.out.contains("Press Ctrl+C to stop."), "{r:?}");
    assert!(r.out.contains("arrived.png"), "{r:?}");
    // The original stays put without --move-originals.
    assert!(folder.join("arrived.png").is_file());
}

#[test]
fn watch_moves_originals_and_takes_files_already_there() {
    let tmp = TempDir::new().unwrap();
    let folder = tmp.path().join("inbox");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("early.png"), b"x").unwrap();
    let source = folder.join("early.png");
    let expected = folder.join("converted").join("early.jpg");
    let moved = folder.join("originals").join("early.png");

    let mut done = false;
    let r = watch_while(
        tmp.path(),
        &[
            "watch",
            "inbox",
            "--to",
            "jpg",
            "--settle",
            "0.2",
            "--move-originals",
            "--quiet",
        ],
        || {
            let start = Instant::now();
            while start.elapsed() < ARRIVAL_TIMEOUT && !(expected.is_file() && moved.is_file()) {
                thread::sleep(Duration::from_millis(50));
            }
            done = expected.is_file() && moved.is_file();
        },
    );

    assert!(done, "early.png was not converted and moved: {r:?}");
    assert!(!source.exists());
    assert_eq!(r.code, 0, "{r:?}");
    // --quiet: nothing on stdout when everything went fine.
    assert_eq!(r.out, "", "{r:?}");
}

#[test]
fn watch_needs_an_existing_folder() {
    let tmp = TempDir::new().unwrap();
    let r = run_in(tmp.path(), png_to_jpg(), &["watch", "nope", "--to", "jpg"]);
    assert_eq!(r.code, 2, "{r:?}");
    assert_eq!(r.out, "");
    assert!(r.err.to_lowercase().contains("not found"), "{r:?}");
    assert!(r.err.starts_with("morphring: Folder not found: "), "{r:?}");
}

#[test]
fn watch_rejects_a_bad_target_before_looking_at_the_folder() {
    let tmp = TempDir::new().unwrap();
    let r = run_in(tmp.path(), png_to_jpg(), &["watch", "nope", "--to", "zzz"]);
    assert_eq!(r.code, 2, "{r:?}");
    assert!(r.err.contains("unknown format 'zzz'"), "{r:?}");
}
