//! Golden tests for the Ghostscript engine: real `gs` on the committed EPS,
//! PS and PDF samples. They skip (printing `SKIP: gs not found`) when
//! Ghostscript is not installed.

mod common;

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};

use morphring_core::{
    CancelToken, ConvertOptions, Converter, EngineError, FormatId, OutputAllocator, Step,
};
use morphring_engines::ghostscript::GhostscriptEngine;
use tempfile::TempDir;

use common::fixture;

/// Hands out `<dir>/sample<suffix>.<ext>`, the way the job runner names outputs.
struct DirAllocator {
    dir: PathBuf,
}

impl OutputAllocator for DirAllocator {
    fn allocate(&mut self, ext: &str, suffix: &str) -> Option<PathBuf> {
        Some(self.dir.join(format!("sample{suffix}.{ext}")))
    }
}

struct Run {
    // Held so the folder outlives the assertions.
    _tmp: TempDir,
    out_dir: PathBuf,
    result: Result<Vec<PathBuf>, EngineError>,
    progress: Vec<f32>,
}

impl Run {
    /// The written files' names, sorted.
    fn names(&self) -> Vec<String> {
        let files = self
            .result
            .as_ref()
            .unwrap_or_else(|e| panic!("convert: {e}"));
        let mut names: Vec<String> = files
            .iter()
            .map(|p| {
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    fn output(&self, name: &str) -> PathBuf {
        self.out_dir.join(name)
    }
}

/// Converts `input` (an absolute path) from `from` to `to`.
fn convert_path(
    input: PathBuf,
    from: &'static str,
    to: &'static str,
    options: ConvertOptions,
) -> Run {
    let tmp = TempDir::new().unwrap_or_else(|e| panic!("temp dir: {e}"));
    let out_dir = tmp.path().join("out");
    fs::create_dir(&out_dir).unwrap_or_else(|e| panic!("out dir: {e}"));
    let step = Step {
        input,
        from: FormatId::new(from),
        to: FormatId::new(to),
        options,
        work_dir: tmp.path().join("work"),
    };
    let progress = RefCell::new(Vec::new());
    let result = GhostscriptEngine::new().convert(
        &step,
        &mut DirAllocator {
            dir: out_dir.clone(),
        },
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

fn convert(
    fixture_name: &str,
    from: &'static str,
    to: &'static str,
    options: ConvertOptions,
) -> Run {
    convert_path(fixture(fixture_name), from, to, options)
}

/// The colour type byte of a PNG's IHDR chunk (6 means RGBA).
fn png_colour_type(path: &Path) -> u8 {
    let bytes = fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert_eq!(
        &bytes[..8],
        b"\x89PNG\r\n\x1a\n",
        "{} is not a PNG",
        path.display()
    );
    assert_eq!(&bytes[12..16], b"IHDR");
    bytes[25]
}

fn assert_jpeg(path: &Path) {
    let bytes = fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert!(
        bytes.starts_with(&[0xFF, 0xD8, 0xFF]),
        "{} is not a JPEG",
        path.display()
    );
}

fn assert_progress_rises_to_one(progress: &[f32]) {
    assert_eq!(progress.last().copied(), Some(1.0), "{progress:?}");
    assert!(
        progress.windows(2).all(|w| w[0] <= w[1]),
        "progress went down: {progress:?}"
    );
}

#[test]
fn eps_to_png_keeps_the_alpha_channel() {
    require_engine!("gs");
    let run = convert("sample.eps", "eps", "png", ConvertOptions::default());
    assert_eq!(run.names(), ["sample.png"]);
    assert_eq!(png_colour_type(&run.output("sample.png")), 6);
    assert_progress_rises_to_one(&run.progress);
}

#[test]
fn eps_is_cropped_to_its_bounding_box() {
    require_engine!("gs");
    // The box is 100 x 80 pt; at 72 dpi that is 100 x 80 pixels, not a letter page.
    let mut options = ConvertOptions::default();
    options.pdf.render_dpi = 72;
    let run = convert("sample.eps", "eps", "png", options);
    let bytes = fs::read(run.output("sample.png")).expect("read png");
    let width = u32::from_be_bytes(bytes[16..20].try_into().expect("width"));
    let height = u32::from_be_bytes(bytes[20..24].try_into().expect("height"));
    assert_eq!((width, height), (100, 80));
}

#[test]
fn ps_to_png_renders_the_page() {
    require_engine!("gs");
    let run = convert("sample.ps", "ps", "png", ConvertOptions::default());
    assert_eq!(run.names(), ["sample.png"]);
    assert_eq!(png_colour_type(&run.output("sample.png")), 6);
}

#[test]
fn pdf_page_range_to_jpg_gives_one_file_per_page() {
    require_engine!("gs");
    let mut options = ConvertOptions::default();
    options.pdf.page_range = "1-2".to_string();
    let run = convert("sample.pdf", "pdf", "jpg", options);
    assert_eq!(run.names(), ["sample_p1.jpg", "sample_p2.jpg"]);
    for name in run.names() {
        assert_jpeg(&run.output(&name));
    }
    assert_progress_rises_to_one(&run.progress);
}

#[test]
fn pdf_gaps_in_the_range_keep_real_page_numbers() {
    require_engine!("gs");
    let mut options = ConvertOptions::default();
    options.pdf.page_range = "1,3".to_string();
    let run = convert("sample.pdf", "pdf", "png", options);
    assert_eq!(run.names(), ["sample_p1.png", "sample_p3.png"]);
    // PDFs render on white paper: RGB, no alpha.
    assert_eq!(png_colour_type(&run.output("sample_p3.png")), 2);
}

#[test]
fn pdf_to_tiff_writes_every_page() {
    require_engine!("gs");
    let run = convert("sample.pdf", "pdf", "tiff", ConvertOptions::default());
    assert_eq!(
        run.names(),
        ["sample_p1.tiff", "sample_p2.tiff", "sample_p3.tiff"]
    );
    let bytes = fs::read(run.output("sample_p2.tiff")).expect("read tiff");
    assert!(bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*"));
}

#[test]
fn a_pdf_based_ai_file_renders_like_a_pdf() {
    require_engine!("gs");
    let tmp = TempDir::new().expect("temp dir");
    let ai = tmp.path().join("drawing.ai");
    fs::copy(fixture("sample.pdf"), &ai).expect("copy");
    let mut options = ConvertOptions::default();
    options.pdf.page_range = "2".to_string();
    let run = convert_path(ai, "ai", "png", options);
    assert_eq!(run.names(), ["sample.png"]);
    assert_eq!(png_colour_type(&run.output("sample.png")), 6);
}

#[test]
fn a_postscript_ai_file_renders_like_an_eps() {
    require_engine!("gs");
    let tmp = TempDir::new().expect("temp dir");
    let ai = tmp.path().join("drawing.ai");
    fs::copy(fixture("sample.eps"), &ai).expect("copy");
    let run = convert_path(ai, "ai", "jpg", ConvertOptions::default());
    assert_eq!(run.names(), ["sample.jpg"]);
    assert_jpeg(&run.output("sample.jpg"));
}

#[test]
fn broken_postscript_fails_with_the_ghostscript_error() {
    require_engine!("gs");
    let tmp = TempDir::new().expect("temp dir");
    let ps = tmp.path().join("broken.ps");
    fs::write(&ps, "%!PS\nthisisnotanoperator\nshowpage\n").expect("write");
    let run = convert_path(ps, "ps", "png", ConvertOptions::default());
    let err = run.result.expect_err("broken PostScript must fail");
    let message = err.to_string();
    assert!(message.starts_with("Ghostscript failed"), "{message}");
    assert!(message.contains("undefined"), "{message}");
    assert!(fs::read_dir(&run.out_dir).expect("list").next().is_none());
}

#[test]
fn a_cancelled_conversion_writes_nothing() {
    require_engine!("gs");
    let tmp = TempDir::new().expect("temp dir");
    let out_dir = tmp.path().join("out");
    fs::create_dir(&out_dir).expect("out dir");
    let step = Step {
        input: fixture("sample.eps"),
        from: FormatId::new("eps"),
        to: FormatId::new("png"),
        options: ConvertOptions::default(),
        work_dir: tmp.path().join("work"),
    };
    let cancel = CancelToken::new();
    cancel.cancel();
    let err = GhostscriptEngine::new()
        .convert(
            &step,
            &mut DirAllocator {
                dir: out_dir.clone(),
            },
            &|_| {},
            &cancel,
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::Cancelled), "{err}");
    assert!(fs::read_dir(&out_dir).expect("list").next().is_none());
}
