//! Golden tests for the Poppler render part: real `pdftoppm` on the committed
//! three-page sample. They skip (printing `SKIP: pdftoppm not found`) when
//! Poppler is not installed, which on Windows and macOS CI is the usual case.

mod common;

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};

use morphring_core::{
    CancelToken, ConvertOptions, Converter, EngineError, FormatId, OutputAllocator, Step,
};
use morphring_engines::poppler::PopplerEngine;
use tempfile::TempDir;

use common::{fixture, image_size};

/// Hands out `<dir>/sample<suffix>.<ext>`, the way the job runner names outputs.
struct DirAllocator {
    dir: PathBuf,
}

impl OutputAllocator for DirAllocator {
    fn allocate(&mut self, ext: &str, suffix: &str) -> Option<PathBuf> {
        Some(self.dir.join(format!("sample{suffix}.{ext}")))
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

fn convert(to: &'static str, options: ConvertOptions) -> Run {
    let tmp = TempDir::new().unwrap_or_else(|e| panic!("temp dir: {e}"));
    let out_dir = tmp.path().join("out");
    fs::create_dir(&out_dir).unwrap_or_else(|e| panic!("out dir: {e}"));
    let step = Step {
        input: fixture("sample.pdf"),
        from: FormatId::new("pdf"),
        to: FormatId::new(to),
        options,
        work_dir: tmp.path().join("work"),
    };
    let progress = RefCell::new(Vec::new());
    let result = PopplerEngine::new().convert(
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

fn with_range(range: &str) -> ConvertOptions {
    let mut options = ConvertOptions::default();
    options.pdf.page_range = range.to_string();
    options
}

fn assert_non_empty(path: &Path) {
    let len = fs::metadata(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .len();
    assert!(len > 0, "{} is empty", path.display());
}

#[test]
fn every_page_becomes_its_own_png() {
    require_engine!("pdftoppm");
    let run = convert("png", ConvertOptions::default());
    assert_eq!(
        run.names(),
        ["sample_p1.png", "sample_p2.png", "sample_p3.png"]
    );
    for name in run.names() {
        assert_non_empty(&run.output(&name));
    }
    // Progress only goes up and ends at done.
    assert!(run.progress.windows(2).all(|w| w[0] <= w[1]));
    assert_eq!(run.progress.last().copied(), Some(1.0));
}

#[test]
fn a_single_page_range_has_no_suffix() {
    require_engine!("pdftoppm");
    let run = convert("png", with_range("2"));
    assert_eq!(run.names(), ["sample.png"]);
    assert_non_empty(&run.output("sample.png"));
}

#[test]
fn split_ranges_keep_their_page_numbers() {
    require_engine!("pdftoppm");
    let run = convert("jpg", with_range("1,3"));
    assert_eq!(run.names(), ["sample_p1.jpg", "sample_p3.jpg"]);
}

#[test]
fn tiff_pages_get_the_tiff_extension() {
    require_engine!("pdftoppm");
    let run = convert("tiff", with_range("2-3"));
    assert_eq!(run.names(), ["sample_p2.tiff", "sample_p3.tiff"]);
}

#[test]
fn letter_pages_at_150_dpi_are_1275_by_1650() {
    require_engine!("pdftoppm");
    require_engine!("magick");
    // The sample is US Letter, 612 x 792 pt: 8.5 x 11 in at 150 dpi.
    let run = convert("png", with_range("1"));
    assert_eq!(image_size(&run.output("sample.png")), (1275, 1650));
}

#[test]
fn the_skip_policy_renders_nothing() {
    require_engine!("pdftoppm");
    let tmp = TempDir::new().unwrap_or_else(|e| panic!("temp dir: {e}"));
    let step = Step {
        input: fixture("sample.pdf"),
        from: FormatId::new("pdf"),
        to: FormatId::new("png"),
        options: ConvertOptions::default(),
        work_dir: tmp.path().join("work"),
    };
    let written = PopplerEngine::new()
        .convert(&step, &mut SkipAll, &|_| {}, &CancelToken::new())
        .unwrap_or_else(|e| panic!("convert: {e}"));
    assert!(written.is_empty());
}

#[test]
fn a_cancelled_token_stops_before_rendering() {
    require_engine!("pdftoppm");
    let tmp = TempDir::new().unwrap_or_else(|e| panic!("temp dir: {e}"));
    let step = Step {
        input: fixture("sample.pdf"),
        from: FormatId::new("pdf"),
        to: FormatId::new("png"),
        options: ConvertOptions::default(),
        work_dir: tmp.path().join("work"),
    };
    let cancel = CancelToken::new();
    cancel.cancel();
    let err = PopplerEngine::new()
        .convert(
            &step,
            &mut DirAllocator {
                dir: tmp.path().to_path_buf(),
            },
            &|_| {},
            &cancel,
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::Cancelled), "{err}");
}

/// Builds a PDF locked with a user password in `dir`, using Ghostscript.
/// The page itself is beside the point; nobody without "u" gets to see it.
fn locked_pdf(dir: &Path) -> PathBuf {
    let ps = dir.join("page.ps");
    fs::write(
        &ps,
        "/Helvetica findfont 24 scalefont setfont 72 720 moveto (Locked) show showpage\n",
    )
    .unwrap_or_else(|e| panic!("write ps: {e}"));
    let pdf = dir.join("locked.pdf");
    let status = std::process::Command::new("gs")
        .args([
            "-q",
            "-dNOPAUSE",
            "-dBATCH",
            "-dSAFER",
            "-sDEVICE=pdfwrite",
            "-sOwnerPassword=o",
            "-sUserPassword=u",
        ])
        .arg(format!("-sOutputFile={}", pdf.display()))
        .arg(&ps)
        .status()
        .unwrap_or_else(|e| panic!("run gs: {e}"));
    assert!(status.success(), "gs could not build the locked PDF");
    pdf
}

/// Converts `input` to `to` and returns the error it must fail with.
fn convert_err(input: &Path, to: &'static str, dir: &Path) -> EngineError {
    let step = Step {
        input: input.to_path_buf(),
        from: FormatId::new("pdf"),
        to: FormatId::new(to),
        options: ConvertOptions::default(),
        work_dir: dir.join(format!("work-{to}")),
    };
    let result = PopplerEngine::new().convert(
        &step,
        &mut DirAllocator {
            dir: dir.to_path_buf(),
        },
        &|_| {},
        &CancelToken::new(),
    );
    match result {
        Ok(files) => panic!("{to}: a locked PDF must not convert, got {files:?}"),
        Err(err) => err,
    }
}

#[test]
fn a_password_protected_pdf_is_explained_not_echoed() {
    require_engine!("pdftoppm");
    require_engine!("pdftotext");
    require_engine!("gs");
    let tmp = TempDir::new().unwrap_or_else(|e| panic!("temp dir: {e}"));
    let pdf = locked_pdf(tmp.path());
    for to in ["png", "txt"] {
        let err = convert_err(&pdf, to, tmp.path());
        assert!(matches!(err, EngineError::Failed(_)), "{to}: {err:?}");
        assert_eq!(
            err.to_string(),
            "The PDF is password-protected. Remove the password and try again.",
            "{to}"
        );
    }
}
