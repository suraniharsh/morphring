//! Golden tests for the FFmpeg engine. They run real conversions into a
//! TempDir and check what came out with ffprobe (never magick, so they also
//! run in CI where only ffmpeg is installed, PLAN.md D7).

mod common;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use common::fixture;
use morphring_core::{
    CancelToken, ConvertOptions, Converter, EngineError, FormatId, OutputAllocator, Step,
};
use morphring_engines::ffmpeg::FfmpegEngine;
use tempfile::TempDir;

/// Puts outputs in `dir` as `<input stem><suffix>.<ext>`, or refuses every one.
struct Folder {
    dir: PathBuf,
    stem: String,
    skip: bool,
}

impl OutputAllocator for Folder {
    fn allocate(&mut self, ext: &str, suffix: &str) -> Option<PathBuf> {
        (!self.skip).then(|| self.dir.join(format!("{}{suffix}.{ext}", self.stem)))
    }
}

/// One test's folders: where outputs go and the job's scratch directory.
struct Setup {
    _root: TempDir,
    out: PathBuf,
    work: PathBuf,
}

fn setup() -> Setup {
    let root = TempDir::new().unwrap_or_else(|e| panic!("no temp dir: {e}"));
    let out = root.path().join("out");
    let work = root.path().join("work");
    for dir in [&out, &work] {
        std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("mkdir {}: {e}", dir.display()));
    }
    Setup {
        _root: root,
        out,
        work,
    }
}

fn step(input: &Path, from: &'static str, to: &'static str, work: &Path) -> Step {
    Step {
        input: input.to_path_buf(),
        from: FormatId::new(from),
        to: FormatId::new(to),
        options: ConvertOptions::default(),
        work_dir: work.to_path_buf(),
    }
}

fn folder(dir: &Path, input: &Path) -> Folder {
    Folder {
        dir: dir.to_path_buf(),
        stem: input
            .file_stem()
            .unwrap_or_else(|| panic!("no file name in {}", input.display()))
            .to_string_lossy()
            .into_owned(),
        skip: false,
    }
}

/// Converts `input` and returns the written files plus every progress value.
fn convert(
    s: &Setup,
    input: &Path,
    from: &'static str,
    to: &'static str,
) -> (Result<Vec<PathBuf>, EngineError>, Vec<f32>) {
    let seen = RefCell::new(Vec::new());
    let result = FfmpegEngine::new().convert(
        &step(input, from, to, &s.work),
        &mut folder(&s.out, input),
        &|p| seen.borrow_mut().push(p),
        &CancelToken::new(),
    );
    (result, seen.into_inner())
}

fn entries(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
        .map(|e| {
            e.unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
                .path()
        })
        .collect()
}

/// A field of the first video stream, via ffprobe.
fn video_field(path: &Path, field: &str) -> String {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-count_frames"])
        .args(["-show_entries", &format!("stream={field}")])
        .args(["-of", "csv=p=0"])
        .arg(path)
        .output()
        .unwrap_or_else(|e| panic!("failed to run ffprobe: {e}"));
    assert!(
        output.status.success(),
        "ffprobe failed on {}",
        path.display()
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn assert_single_output(files: &[PathBuf], expected: &Path) {
    assert_eq!(files, [expected.to_path_buf()]);
    let len = std::fs::metadata(expected)
        .unwrap_or_else(|e| panic!("{}: {e}", expected.display()))
        .len();
    assert!(len > 0, "{} is empty", expected.display());
}

#[test]
fn status_is_available_when_installed() {
    require_engine!("ffmpeg");
    assert!(FfmpegEngine::new().status().is_available());
}

#[test]
fn wav_to_mp3_keeps_the_duration() {
    require_engine!("ffmpeg");
    let s = setup();
    let input = fixture("sample.wav");
    let (result, _) = convert(&s, &input, "wav", "mp3");
    let expected = s.out.join("sample.mp3");
    assert_single_output(&result.unwrap(), &expected);
    let seconds = common::duration_secs(&expected);
    assert!((seconds - 1.0).abs() <= 0.1, "duration was {seconds}");
}

#[test]
fn mp4_to_webm_reencodes_to_vp9() {
    require_engine!("ffmpeg");
    let s = setup();
    let input = fixture("sample.mp4");
    let (result, _) = convert(&s, &input, "mp4", "webm");
    let expected = s.out.join("sample.webm");
    assert_single_output(&result.unwrap(), &expected);
    assert_eq!(video_field(&expected, "codec_name"), "vp9");
}

#[test]
fn mp4_to_mkv_is_a_stream_copy() {
    require_engine!("ffmpeg");
    let s = setup();
    let input = fixture("sample.mp4");
    let (result, _) = convert(&s, &input, "mp4", "mkv");
    let expected = s.out.join("sample.mkv");
    assert_single_output(&result.unwrap(), &expected);
    assert_eq!(video_field(&expected, "codec_name"), "h264");
}

#[test]
fn mp4_to_gif_is_animated() {
    require_engine!("ffmpeg");
    let s = setup();
    let input = fixture("sample.mp4");
    let (result, progress) = convert(&s, &input, "mp4", "gif");
    let expected = s.out.join("sample.gif");
    assert_single_output(&result.unwrap(), &expected);
    let frames: u32 = video_field(&expected, "nb_read_frames").parse().unwrap();
    assert!(frames > 1, "only {frames} frame(s)");
    assert_eq!(progress.last(), Some(&1.0));
    // The palette is scratch, never an output.
    assert_eq!(entries(&s.out).len(), 1);
}

#[test]
fn mp4_to_png_is_one_full_size_frame() {
    require_engine!("ffmpeg");
    let s = setup();
    let input = fixture("sample.mp4");
    let (result, _) = convert(&s, &input, "mp4", "png");
    let expected = s.out.join("sample.png");
    assert_single_output(&result.unwrap(), &expected);
    assert_eq!(video_field(&expected, "codec_name"), "png");
    assert_eq!(video_field(&expected, "width,height"), "64,64");
}

#[test]
fn progress_is_monotonic_and_ends_at_one() {
    require_engine!("ffmpeg");
    let s = setup();
    let input = fixture("sample.mp4");
    let (result, progress) = convert(&s, &input, "mp4", "webm");
    result.unwrap();
    assert!(!progress.is_empty());
    assert!(
        progress.windows(2).all(|w| w[0] < w[1]),
        "not monotonic: {progress:?}"
    );
    assert!(progress.iter().all(|p| (0.0..=1.0).contains(p)));
    assert_eq!(progress.last(), Some(&1.0));
}

#[test]
fn scratch_files_are_cleaned_up() {
    require_engine!("ffmpeg");
    let s = setup();
    let (result, _) = convert(&s, &fixture("sample.mp4"), "mp4", "gif");
    result.unwrap();
    assert!(entries(&s.work).is_empty(), "left: {:?}", entries(&s.work));
}

#[test]
fn a_skipped_output_writes_nothing() {
    require_engine!("ffmpeg");
    let s = setup();
    let input = fixture("sample.wav");
    let mut out = folder(&s.out, &input);
    out.skip = true;
    let written = FfmpegEngine::new()
        .convert(
            &step(&input, "wav", "mp3", &s.work),
            &mut out,
            &|_| {},
            &CancelToken::new(),
        )
        .unwrap();
    assert!(written.is_empty());
    assert!(entries(&s.out).is_empty());
}

#[test]
fn audio_to_a_video_target_fails_before_writing() {
    require_engine!("ffmpeg");
    let s = setup();
    let (result, _) = convert(&s, &fixture("sample.wav"), "wav", "gif");
    let err = result.unwrap_err();
    assert!(err.to_string().contains("no video"), "{err}");
    assert!(entries(&s.out).is_empty());
    assert!(entries(&s.work).is_empty());
}

#[test]
fn an_unreadable_file_fails_with_ffprobes_reason() {
    require_engine!("ffmpeg");
    let s = setup();
    let (result, _) = convert(&s, &fixture("notes.unknownext"), "mp4", "webm");
    let err = result.unwrap_err();
    assert!(
        err.to_string().starts_with("FFmpeg cannot read this file"),
        "{err}"
    );
    assert!(entries(&s.out).is_empty());
}

#[test]
fn cancelling_mid_run_leaves_no_output() {
    require_engine!("ffmpeg");
    let s = setup();
    // The 1 s fixture finishes before anyone can blink, so make a clip that
    // keeps VP9 busy for a while. Test helpers may run ffmpeg directly (D6).
    let input = s.work.join("long.mp4");
    let made = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi"])
        .args(["-i", "testsrc=duration=60:size=640x360:rate=30"])
        .args([
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&input)
        .status()
        .unwrap();
    assert!(made.success());

    let cancel = CancelToken::new();
    let started = Instant::now();
    let cancelled_at = RefCell::new(None);
    let result = FfmpegEngine::new().convert(
        &step(&input, "mp4", "webm", &s.work),
        &mut folder(&s.out, &input),
        &|p| {
            // Pull the plug the moment ffmpeg reports a real position, which
            // is as mid-run as it gets.
            if p > 0.02 && cancelled_at.borrow().is_none() {
                *cancelled_at.borrow_mut() = Some(Instant::now());
                cancel.cancel();
            }
        },
        &cancel,
    );

    assert!(
        matches!(result, Err(EngineError::Cancelled)),
        "expected Cancelled, got {result:?}"
    );
    let at = cancelled_at
        .into_inner()
        .expect("ffmpeg never reported progress");
    assert!(at.elapsed() < Duration::from_secs(2), "cancel was slow");
    assert!(started.elapsed() < Duration::from_secs(60));
    assert!(entries(&s.out).is_empty(), "left: {:?}", entries(&s.out));
    // Only the generated input is left in the work dir, no scratch folder.
    assert_eq!(entries(&s.work), [input]);
}
