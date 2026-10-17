//! Golden tests for PDF to text through `pdftotext`. They skip when poppler is
//! missing (Windows and macOS CI, for now), so run them on a Linux box too.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::fixture;
use morphring_core::{
    CancelToken, ConvertOptions, Converter, EngineError, FormatId, OutputAllocator, Step,
};
use morphring_engines::poppler::PopplerEngine;

/// Hands out `<dir>/<stem>.<ext>`, or nothing at all when `skip` is set.
struct DirAllocator {
    dir: PathBuf,
    stem: &'static str,
    skip: bool,
}

impl OutputAllocator for DirAllocator {
    fn allocate(&mut self, ext: &str, suffix: &str) -> Option<PathBuf> {
        (!self.skip).then(|| self.dir.join(format!("{}{suffix}.{ext}", self.stem)))
    }
}

fn step(input: PathBuf, work: &Path) -> Step {
    Step {
        input,
        from: FormatId::new("pdf"),
        to: FormatId::new("txt"),
        options: ConvertOptions::default(),
        work_dir: work.to_path_buf(),
    }
}

fn convert(input: PathBuf, dir: &Path, skip: bool) -> Result<Vec<PathBuf>, EngineError> {
    let mut out = DirAllocator {
        dir: dir.to_path_buf(),
        stem: "result",
        skip,
    };
    PopplerEngine::new().convert(
        &step(input, &dir.join("work")),
        &mut out,
        &|_| {},
        &CancelToken::new(),
    )
}

/// A one-page PDF with nothing on the page: the honest stand-in for a scan.
/// It has no xref table at all; poppler rebuilds one and moves on.
fn blank_pdf(dir: &Path) -> PathBuf {
    let path = dir.join("blank.pdf");
    let body = "%PDF-1.4\n\
        1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
        2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
        3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\n\
        trailer << /Root 1 0 R /Size 4 >>\n\
        %%EOF\n";
    fs::write(&path, body).unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
    path
}

#[test]
fn the_engine_offers_pdf_to_txt_at_cost_ten() {
    let engine = PopplerEngine::new();
    let edge = engine
        .edges()
        .iter()
        .find(|e| e.from.as_str() == "pdf" && e.to.as_str() == "txt")
        .expect("pdf -> txt edge");
    assert_eq!(edge.cost, 10);
}

#[test]
fn sample_pdf_becomes_utf8_text_with_a_bom() {
    require_engine!("pdftotext");
    // Git for Windows ships a lone pdftotext; the engine wants all of Poppler.
    require_engine!("pdftoppm");
    let dir = tempfile::tempdir().unwrap();
    let written = convert(fixture("sample.pdf"), dir.path(), false).unwrap();
    assert_eq!(written, [dir.path().join("result.txt")]);

    let bytes = fs::read(&written[0]).unwrap();
    assert_eq!(&bytes[..3], b"\xEF\xBB\xBF", "missing BOM");
    let text = std::str::from_utf8(&bytes[3..]).expect("valid UTF-8");
    assert!(text.contains("Page 1"), "{text:?}");
    assert!(text.contains("Café déjà vu"), "{text:?}");
    // Pages are separated by exactly one blank line, and no form feeds survive.
    assert!(text.contains("\n\nPage 2"), "{text:?}");
    assert!(!text.contains('\u{c}'), "{text:?}");
    assert!(!text.contains("\n\n\n"), "{text:?}");

    // Partial work never outlives the run.
    let leftovers: Vec<_> = fs::read_dir(dir.path().join("work")).unwrap().collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn a_pdf_without_text_fails_with_a_clear_message() {
    require_engine!("pdftotext");
    // Git for Windows ships a lone pdftotext; the engine wants all of Poppler.
    require_engine!("pdftoppm");
    let dir = tempfile::tempdir().unwrap();
    let err = convert(blank_pdf(dir.path()), dir.path(), false).unwrap_err();
    assert_eq!(
        err.to_string(),
        "No text was found in this file (it may be scanned or contain only pictures)."
    );
    assert!(!dir.path().join("result.txt").exists());
}

#[test]
fn a_missing_input_fails_without_output() {
    require_engine!("pdftotext");
    // Git for Windows ships a lone pdftotext; the engine wants all of Poppler.
    require_engine!("pdftoppm");
    let dir = tempfile::tempdir().unwrap();
    let err = convert(dir.path().join("nope.pdf"), dir.path(), false).unwrap_err();
    assert!(err.to_string().starts_with("pdftotext "), "{err}");
    assert!(!dir.path().join("result.txt").exists());
}

#[test]
fn a_skipped_output_writes_nothing() {
    require_engine!("pdftotext");
    // Git for Windows ships a lone pdftotext; the engine wants all of Poppler.
    require_engine!("pdftoppm");
    let dir = tempfile::tempdir().unwrap();
    let written = convert(fixture("sample.pdf"), dir.path(), true).unwrap();
    assert!(written.is_empty());
    assert!(!dir.path().join("result.txt").exists());
}

#[test]
fn a_cancelled_job_does_not_start() {
    require_engine!("pdftotext");
    // Git for Windows ships a lone pdftotext; the engine wants all of Poppler.
    require_engine!("pdftoppm");
    let dir = tempfile::tempdir().unwrap();
    let cancel = CancelToken::new();
    cancel.cancel();
    let mut out = DirAllocator {
        dir: dir.path().to_path_buf(),
        stem: "result",
        skip: false,
    };
    let err = PopplerEngine::new()
        .convert(
            &step(fixture("sample.pdf"), dir.path()),
            &mut out,
            &|_| {},
            &cancel,
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::Cancelled), "{err}");
}
