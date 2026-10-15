//! Golden tests for the ImageMagick engine. They run the real `magick` and
//! skip (printing `SKIP: magick not found`) when it is not installed.

mod common;

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use morphring_core::{
    CancelToken, ConvertOptions, Converter, EngineError, FormatId, OutputAllocator, Step,
};
use morphring_engines::magick::MagickEngine;
use tempfile::TempDir;

use common::{fixture, frame_count, image_size};

/// Hands out `<out_dir>/<stem><suffix>.<ext>`, the way the job runner names outputs.
struct DirAllocator {
    dir: PathBuf,
    stem: String,
}

impl OutputAllocator for DirAllocator {
    fn allocate(&mut self, ext: &str, suffix: &str) -> Option<PathBuf> {
        Some(self.dir.join(format!("{}{suffix}.{ext}", self.stem)))
    }
}

/// The skip policy with every output already taken.
struct SkipAll;

impl OutputAllocator for SkipAll {
    fn allocate(&mut self, _ext: &str, _suffix: &str) -> Option<PathBuf> {
        None
    }
}

struct Run {
    // Held so the folders outlive the assertions.
    _tmp: TempDir,
    out_dir: PathBuf,
    result: Result<Vec<PathBuf>, EngineError>,
    progress: Vec<f32>,
}

fn step(tmp: &Path, input: &str, from: &'static str, to: &'static str) -> Step {
    Step {
        input: fixture(input),
        from: FormatId::new(from),
        to: FormatId::new(to),
        options: ConvertOptions::default(),
        work_dir: tmp.join("work"),
    }
}

fn convert(input: &str, from: &'static str, to: &'static str) -> Run {
    let tmp = TempDir::new().unwrap_or_else(|e| panic!("temp dir: {e}"));
    let out_dir = tmp.path().join("out");
    fs::create_dir(&out_dir).unwrap_or_else(|e| panic!("out dir: {e}"));
    let stem = Path::new(input)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| panic!("fixture {input} has no stem"));
    let mut alloc = DirAllocator {
        dir: out_dir.clone(),
        stem,
    };
    let progress = RefCell::new(Vec::new());
    let result = MagickEngine::new().convert(
        &step(tmp.path(), input, from, to),
        &mut alloc,
        &|p| progress.borrow_mut().push(p),
        &CancelToken::new(),
    );
    Run {
        _tmp: tmp,
        out_dir,
        result,
        progress: progress.into_inner(),
    }
}

fn magick_format(path: &Path) -> String {
    let first = format!("{}[0]", path.display());
    let out = Command::new("magick")
        .args(["identify", "-format", "%m", &first])
        .output()
        .unwrap_or_else(|e| panic!("failed to run magick identify: {e}"));
    assert!(
        out.status.success(),
        "identify failed on {}",
        path.display()
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn status_is_available_when_installed() {
    require_engine!("magick");
    assert!(MagickEngine::new().status().is_available());
}

#[test]
fn png_to_jpg_keeps_size() {
    require_engine!("magick");
    let run = convert("sample.png", "png", "jpg");
    let written = run.result.expect("png -> jpg");
    assert_eq!(written, [run.out_dir.join("sample.jpg")]);
    assert_eq!(image_size(&written[0]), (64, 48));
    assert_eq!(magick_format(&written[0]), "JPEG");
    assert_eq!(run.progress, [0.0, 1.0]);
}

#[test]
fn png_to_webp_and_ico() {
    require_engine!("magick");
    for (to, format) in [("webp", "WEBP"), ("ico", "ICO")] {
        let run = convert("sample.png", "png", to);
        let written = run.result.unwrap_or_else(|e| panic!("png -> {to}: {e}"));
        assert_eq!(written.len(), 1);
        assert!(fs::metadata(&written[0]).expect("output").len() > 0);
        assert_eq!(magick_format(&written[0]), format);
    }
}

#[test]
fn gif_to_png_writes_one_file_per_frame() {
    require_engine!("magick");
    let run = convert("sample.gif", "gif", "png");
    let written = run.result.expect("gif -> png");
    let expected: Vec<PathBuf> = (1..=3)
        .map(|n| run.out_dir.join(format!("sample_p{n}.png")))
        .collect();
    assert_eq!(written, expected);
    assert_eq!(
        listing(&run.out_dir),
        ["sample_p1.png", "sample_p2.png", "sample_p3.png"]
    );
    for path in &written {
        assert_eq!(magick_format(path), "PNG");
        assert_eq!(image_size(path), (32, 32));
    }
}

#[test]
fn gif_to_gif_stays_one_animated_file() {
    require_engine!("magick");
    let run = convert("sample.gif", "gif", "gif");
    let written = run.result.expect("gif -> gif");
    assert_eq!(written, [run.out_dir.join("sample.gif")]);
    assert_eq!(frame_count(&written[0]), 3);
}

#[test]
fn png_to_pdf_writes_a_pdf() {
    require_engine!("magick");
    let run = convert("sample.png", "png", "pdf");
    let written = run.result.expect("png -> pdf");
    assert_eq!(written.len(), 1);
    let bytes = fs::read(&written[0]).expect("read pdf");
    assert!(bytes.starts_with(b"%PDF"), "not a PDF");
}

#[test]
fn skipped_output_writes_nothing() {
    require_engine!("magick");
    let tmp = TempDir::new().expect("temp dir");
    for (input, from) in [("sample.png", "png"), ("sample.gif", "gif")] {
        let written = MagickEngine::new()
            .convert(
                &step(tmp.path(), input, from, "png"),
                &mut SkipAll,
                &|_| {},
                &CancelToken::new(),
            )
            .expect("skip is not an error");
        assert!(written.is_empty());
    }
    // Nothing anywhere, not even a stray temp dir.
    let work = tmp.path().join("work");
    assert!(!work.exists() || listing(&work).is_empty());
}

#[test]
fn cancelled_before_start_writes_nothing() {
    require_engine!("magick");
    let tmp = TempDir::new().expect("temp dir");
    let out_dir = tmp.path().join("out");
    fs::create_dir(&out_dir).expect("out dir");
    let cancel = CancelToken::new();
    cancel.cancel();
    let result = MagickEngine::new().convert(
        &step(tmp.path(), "sample.png", "png", "jpg"),
        &mut DirAllocator {
            dir: out_dir.clone(),
            stem: "sample".into(),
        },
        &|_| {},
        &cancel,
    );
    assert!(matches!(result, Err(EngineError::Cancelled)));
    assert!(listing(&out_dir).is_empty());
}

#[test]
fn unreadable_input_fails_with_detail() {
    require_engine!("magick");
    let tmp = TempDir::new().expect("temp dir");
    let out_dir = tmp.path().join("out");
    fs::create_dir(&out_dir).expect("out dir");
    let bogus = tmp.path().join("broken.png");
    fs::write(&bogus, b"definitely not a png").expect("write");
    let mut s = step(tmp.path(), "sample.png", "png", "jpg");
    s.input = bogus;
    let result = MagickEngine::new().convert(
        &s,
        &mut DirAllocator {
            dir: out_dir.clone(),
            stem: "broken".into(),
        },
        &|_| {},
        &CancelToken::new(),
    );
    match result {
        Err(EngineError::Failed(detail)) => assert!(detail.starts_with("ImageMagick"), "{detail}"),
        other => panic!("expected a failure, got {other:?}"),
    }
    assert!(listing(&out_dir).is_empty());
    // The temp subdir went with the failure.
    assert!(listing(&tmp.path().join("work")).is_empty());
}
