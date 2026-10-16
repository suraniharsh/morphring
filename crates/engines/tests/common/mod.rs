//! Shared helpers for the engines integration tests.
//!
//! Each test binary pulls this in with `mod common;` and uses only a slice of
//! it, hence the dead-code allowance below. Helpers call `Command` directly on
//! purpose (PLAN.md D6): checking a golden output needs the real tools.

// Every tests/*.rs file is its own crate, so whatever one of them doesn't use
// would otherwise light up as dead code.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// Skips the current test (by returning early) when `tool` is not on `PATH`.
///
/// Prints `SKIP: <tool> not found` so a skipped golden test is visible in the
/// `--nocapture` output instead of passing silently.
#[macro_export]
macro_rules! require_engine {
    ($tool:expr) => {
        if !$crate::common::engine_available($tool) {
            println!("SKIP: {} not found", $tool);
            return;
        }
    };
}

/// True when `tool` can be found on `PATH`.
///
/// Takes plain program names (`pdftoppm`, `pdftotext`, `pdfinfo`, `gs`,
/// `pandoc`, ...) and asks `which`, so it works for helpers that the engine
/// discovery code knows nothing about.
pub fn engine_available(tool: &str) -> bool {
    which::which(tool).is_ok()
}

/// Absolute path of a committed fixture in `crates/engines/tests/fixtures`.
pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// Runs a tool and returns its stdout, panicking with stderr on failure.
fn run(program: &str, args: &[&str], path: &Path) -> String {
    let output = Command::new(program)
        .args(args)
        .arg(path)
        .output()
        .unwrap_or_else(|e| panic!("failed to run {program}: {e}"));
    assert!(
        output.status.success(),
        "{program} failed on {}: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Width and height of the first frame, via `magick identify -format '%w %h'`.
pub fn image_size(path: &Path) -> (u32, u32) {
    // `[0]` keeps multi-frame files (gif, tiff) from printing one size per frame.
    let first = PathBuf::from(format!("{}[0]", path.display()));
    let text = run("magick", &["identify", "-format", "%w %h"], &first);
    let mut parts = text.split_whitespace().map(|n| {
        n.parse::<u32>()
            .unwrap_or_else(|e| panic!("bad size {text:?} for {}: {e}", path.display()))
    });
    match (parts.next(), parts.next()) {
        (Some(w), Some(h)) => (w, h),
        _ => panic!("magick identify gave no size for {}", path.display()),
    }
}

/// Number of frames (or pages/layers) in an image, via `magick identify`.
pub fn frame_count(path: &Path) -> usize {
    // One line per frame; counting lines beats trusting %n on every format.
    run("magick", &["identify", "-format", "%p\n"], path)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count()
}

/// Container duration in seconds, via ffprobe.
pub fn duration_secs(path: &Path) -> f64 {
    let text = run(
        "ffprobe",
        &[
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
        ],
        path,
    );
    text.trim()
        .parse::<f64>()
        .unwrap_or_else(|e| panic!("bad duration {text:?} for {}: {e}", path.display()))
}

/// Page count of a PDF, via `pdfinfo`.
///
/// Returns `None` when pdfinfo is not installed (or chokes on the file), so a
/// caller can pair it with `require_engine!("pdfinfo")` or skip on its own.
pub fn pdf_page_count(path: &Path) -> Option<usize> {
    let output = Command::new("pdfinfo").arg(path).output().ok()?;
    if !output.status.success() {
        return None;
    }
    // pdfinfo pads the value with spaces, e.g. "Pages:          3".
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("Pages:"))
        .and_then(|n| n.trim().parse().ok())
}

/// Reads a text output as UTF-8, minus a leading BOM and with CRLF turned into LF.
///
/// Our pdftotext output carries a BOM on purpose (Filee does it too) and
/// Windows tools love CRLF; neither should make a golden comparison fail.
pub fn read_text(path: &Path) -> String {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {} as UTF-8: {e}", path.display()));
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    text.replace("\r\n", "\n")
}

/// True when `path` is a directory holding every entry in `names`.
///
/// Names may be nested (`"dir/inner.txt"`); extra entries are fine, since
/// this checks "at least these", not "exactly these".
pub fn is_dir_with(path: &Path, names: &[&str]) -> bool {
    path.is_dir() && names.iter().all(|name| path.join(name).exists())
}
