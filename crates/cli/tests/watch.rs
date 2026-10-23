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

use morphring_core::testing::{Behaviour, FakeConverter};
use morphring_core::{CancelToken, Converter};
use tempfile::TempDir;

use common::{Outcome, run_in, run_with_cancel, run_with_quit};

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

/// What watch says on stderr when the first Ctrl+C lands mid-batch.
const STOPPING: &str = "Stopping after the current batch... press Ctrl+C again to quit now.";

/// Polls `done` every 20 ms for up to `limit`.
fn eventually(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if done() {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    done()
}

/// FX-07, in process: the first Ctrl+C lets the batch run (and says so), the
/// second one cancels it instead of leaving it to exit().
#[test]
fn second_ctrl_c_cancels_the_batch_in_flight() {
    let tmp = TempDir::new().unwrap();
    let folder = tmp.path().join("inbox");
    fs::create_dir(&folder).unwrap();
    // A minute-long conversion: long enough that only a cancel ends it in time.
    let fake = FakeConverter::new("fake")
        .edge("png", "jpg", 10)
        .behaviour(Behaviour::Sleep(Duration::from_secs(60)));
    let counters = fake.counters();
    let engines: Vec<Box<dyn Converter>> = vec![Box::new(fake)];

    let (cancel, quit) = (CancelToken::new(), CancelToken::new());
    let worker = {
        let (cwd, cancel, quit) = (tmp.path().to_path_buf(), cancel.clone(), quit.clone());
        thread::spawn(move || {
            let args = ["watch", "inbox", "--to", "jpg", "--settle", "0.2"];
            run_with_quit(&cwd, engines, &args, cancel, quit)
        })
    };
    thread::sleep(Duration::from_millis(500));
    let source = folder.join("slow.png");
    fs::write(&source, b"x").unwrap();
    let mut polls = 0usize;
    let started = eventually(ARRIVAL_TIMEOUT, || {
        if counters.calls() > 0 {
            return true;
        }
        // Same nudge as wait_for, every 4 s: the first write may have beaten
        // the watcher, and a new size makes it a new file to the settler.
        polls += 1;
        if polls.is_multiple_of(200) {
            fs::write(&source, vec![b'x'; polls / 200 + 1]).unwrap();
        }
        false
    });
    assert!(started, "the slow conversion never started");

    cancel.cancel();
    // The first press stops the watching, not the batch.
    thread::sleep(Duration::from_millis(500));
    assert!(!worker.is_finished(), "the first Ctrl+C ended the batch");

    quit.cancel();
    assert!(
        eventually(Duration::from_secs(10), || worker.is_finished()),
        "the second Ctrl+C did not cancel the batch"
    );
    let r = worker.join().unwrap();
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(r.err.matches(STOPPING).count(), 1, "{r:?}");
    assert!(r.out.contains("slow.png"), "{r:?}");
    assert!(!folder.join("converted").join("slow.jpg").exists(), "{r:?}");
}

/// FX-07 on the real binary with a real ffmpeg: two SIGINTs end `morphring
/// watch` with 130, take the ffmpeg down with it and leave no job folder.
/// Linux only: it reads /proc to find the child.
#[cfg(target_os = "linux")]
mod ctrl_c {
    use std::fs::File;
    use std::process::{Child, Command, Stdio};

    use super::*;

    /// Kills whatever it holds on drop, so a failing assertion does not
    /// leave a 720p encode running on the developer's machine.
    struct Reaper(Vec<u32>);

    impl Drop for Reaper {
        fn drop(&mut self) {
            for pid in &self.0 {
                let _ = Command::new("kill")
                    .args(["-KILL", &pid.to_string()])
                    .stderr(Stdio::null())
                    .status();
            }
        }
    }

    fn signal(pid: u32, sig: &str) {
        let ok = Command::new("kill")
            .args([sig, &pid.to_string()])
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok, "kill {sig} {pid} failed");
    }

    /// The fields of /proc/<pid>/stat after the command name: state first,
    /// then ppid. `None` once the process is gone.
    fn stat(pid: u32) -> Option<(String, String, Vec<String>)> {
        let raw = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let (head, rest) = raw.rsplit_once(')')?;
        let comm = head.split_once('(')?.1.to_owned();
        let mut fields = rest.split_whitespace().map(str::to_owned);
        let state = fields.next()?;
        Some((comm, state, fields.collect()))
    }

    /// A running (not zombie) process.
    fn alive(pid: u32) -> bool {
        stat(pid).is_some_and(|(_, state, _)| state != "Z")
    }

    /// The pid of a running `ffmpeg` whose parent is `parent`.
    fn ffmpeg_child_of(parent: u32) -> Option<u32> {
        fs::read_dir("/proc")
            .ok()?
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
            .find(|pid| {
                stat(*pid).is_some_and(|(comm, state, rest)| {
                    comm == "ffmpeg" && state != "Z" && rest.first() == Some(&parent.to_string())
                })
            })
    }

    fn exits_within(child: &mut Child, limit: Duration) -> Option<std::process::ExitStatus> {
        let start = Instant::now();
        while start.elapsed() < limit {
            if let Ok(Some(status)) = child.try_wait() {
                return Some(status);
            }
            thread::sleep(Duration::from_millis(20));
        }
        None
    }

    #[test]
    fn second_sigint_kills_the_engine_and_cleans_up() {
        crate::require_engine!("ffmpeg");
        let tmp = TempDir::new().unwrap();
        let (inbox, stage, private_tmp, home) = (
            tmp.path().join("inbox"),
            tmp.path().join("stage"),
            tmp.path().join("tmp"),
            tmp.path().join("home"),
        );
        for dir in [&inbox, &stage, &private_tmp, &home] {
            fs::create_dir(dir).unwrap();
        }
        // 20 s of 720p: seconds to make as MPEG-4, far longer to turn into VP9.
        let input = stage.join("long.mp4");
        let made = Command::new("ffmpeg")
            .args(["-loglevel", "error", "-y", "-f", "lavfi"])
            .args(["-i", "testsrc=d=20:s=1280x720", "-pix_fmt", "yuv420p"])
            .args(["-c:v", "mpeg4"])
            .arg(&input)
            .status()
            .is_ok_and(|s| s.success());
        assert!(made, "could not make the test video");

        let out_log = tmp.path().join("out.txt");
        let err_log = tmp.path().join("err.txt");
        let mut child = Command::new(env!("CARGO_BIN_EXE_morphring"))
            .args(["watch", "inbox", "--to", "webm", "--settle", "0.5"])
            .current_dir(tmp.path())
            .env("TMPDIR", &private_tmp)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .stdin(Stdio::null())
            .stdout(File::create(&out_log).unwrap())
            .stderr(File::create(&err_log).unwrap())
            .spawn()
            .unwrap_or_else(|e| panic!("cannot start morphring: {e}"));
        let pid = child.id();
        let mut reaper = Reaper(vec![pid]);
        let read = |path: &Path| fs::read_to_string(path).unwrap_or_default();

        assert!(
            eventually(Duration::from_secs(10), || read(&out_log)
                .contains("Watching ")),
            "watch never started: {}",
            read(&err_log)
        );
        fs::rename(&input, inbox.join("long.mp4")).unwrap();

        // The encode, not some quick probe: the same ffmpeg a second later.
        let mut ffmpeg = None;
        let found = eventually(ARRIVAL_TIMEOUT, || {
            let Some(candidate) = ffmpeg_child_of(pid) else {
                return false;
            };
            thread::sleep(Duration::from_secs(1));
            let still = ffmpeg_child_of(pid) == Some(candidate);
            if still {
                ffmpeg = Some(candidate);
            }
            still
        });
        let ffmpeg = ffmpeg
            .filter(|_| found)
            .unwrap_or_else(|| panic!("no ffmpeg encode under morphring: {}", read(&err_log)));
        reaper.0.push(ffmpeg);

        signal(pid, "-INT");
        assert!(
            eventually(Duration::from_secs(5), || read(&err_log).contains(STOPPING)),
            "no word after the first Ctrl+C: {:?}",
            read(&err_log)
        );
        assert!(
            alive(ffmpeg),
            "the first Ctrl+C should let the batch finish"
        );
        assert!(
            child.try_wait().unwrap().is_none(),
            "watch quit on the first Ctrl+C"
        );

        signal(pid, "-INT");
        let status = exits_within(&mut child, Duration::from_secs(15))
            .unwrap_or_else(|| panic!("morphring is still running after the second Ctrl+C"));
        assert_eq!(status.code(), Some(130), "{}", read(&err_log));
        assert!(
            eventually(Duration::from_secs(5), || !alive(ffmpeg)),
            "ffmpeg {ffmpeg} outlived morphring"
        );

        let leftovers: Vec<_> = fs::read_dir(private_tmp.join("morphring"))
            .map(|entries| entries.filter_map(Result::ok).map(|e| e.path()).collect())
            .unwrap_or_default();
        assert!(
            leftovers.is_empty(),
            "job folders left behind: {leftovers:?}"
        );
        assert!(!inbox.join("converted").join("long.webm").exists());
        assert_eq!(
            read(&err_log).matches(STOPPING).count(),
            1,
            "{}",
            read(&err_log)
        );
    }
}
