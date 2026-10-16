//! Golden tests for the built-in PDF engine's SVG to PDF part (krilla-svg).
//!
//! Needs no external tool except pdftotext for the text check, which skips
//! politely when Poppler is not around.

#![cfg(feature = "pdf")]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use morphring_core::{
    CancelToken, ConvertOptions, Converter, EngineError, FormatId, OutputAllocator, Step,
};
use morphring_engines::pdf::PdfEngine;

/// Hands out `<dir>/out.<ext>`, like a job writing next to the source.
struct DirAllocator(PathBuf);

impl OutputAllocator for DirAllocator {
    fn allocate(&mut self, ext: &str, suffix: &str) -> Option<PathBuf> {
        Some(self.0.join(format!("out{suffix}.{ext}")))
    }
}

/// The skip policy with every output already taken.
struct SkipAll;

impl OutputAllocator for SkipAll {
    fn allocate(&mut self, _ext: &str, _suffix: &str) -> Option<PathBuf> {
        None
    }
}

fn step(input: &Path, from: &'static str, work: &Path) -> Step {
    Step {
        input: input.to_path_buf(),
        from: FormatId::new(from),
        to: FormatId::new("pdf"),
        options: ConvertOptions::default(),
        work_dir: work.join("work"),
    }
}

fn convert(input: &Path, from: &'static str, dir: &Path) -> Result<Vec<PathBuf>, EngineError> {
    PdfEngine::new().convert(
        &step(input, from, dir),
        &mut DirAllocator(dir.to_path_buf()),
        &|_| {},
        &CancelToken::new(),
    )
}

/// Pages and the first page's media box, read back with lopdf.
fn pages_and_media_box(path: &Path) -> (usize, Vec<f32>) {
    let doc = lopdf::Document::load(path)
        .unwrap_or_else(|e| panic!("{} is not a readable PDF: {e}", path.display()));
    let pages = doc.get_pages();
    let Some(&first) = pages.values().next() else {
        panic!("{} has no pages", path.display());
    };
    let media_box = doc
        .get_dictionary(first)
        .and_then(|page| page.get(b"MediaBox"))
        .and_then(lopdf::Object::as_array)
        .unwrap_or_else(|e| panic!("no media box in {}: {e}", path.display()))
        .iter()
        .map(|n| {
            n.as_float()
                .unwrap_or_else(|e| panic!("bad media box number: {e}"))
        })
        .collect();
    (pages.len(), media_box)
}

#[test]
fn the_engine_lists_svg_and_svgz_to_pdf() {
    let engine = PdfEngine::new();
    for from in ["svg", "svgz"] {
        let edge = engine
            .edges()
            .iter()
            .find(|e| e.from == FormatId::new(from) && e.to == FormatId::new("pdf"))
            .unwrap_or_else(|| panic!("no {from} -> pdf edge"));
        assert_eq!(edge.cost, 10);
    }
}

#[test]
fn sample_svg_becomes_one_page_with_its_text() {
    let dir = tempfile::tempdir().unwrap();
    let outputs = convert(&common::fixture("sample.svg"), "svg", dir.path()).unwrap();
    assert_eq!(outputs, [dir.path().join("out.pdf")]);

    // sample.svg is 120 x 80 px, which is 90 x 60 pt.
    let (pages, media_box) = pages_and_media_box(&outputs[0]);
    assert_eq!(pages, 1);
    assert_eq!(media_box, [0.0, 0.0, 90.0, 60.0]);

    // The temp dir inside work_dir is gone, so nothing half-written lingers.
    let leftovers = std::fs::read_dir(dir.path().join("work")).unwrap().count();
    assert_eq!(leftovers, 0);

    require_engine!("pdftotext");
    let text = Command::new("pdftotext")
        .arg(&outputs[0])
        .arg("-")
        .output()
        .expect("pdftotext runs");
    assert!(text.status.success());
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(text.contains("Ring"), "pdftotext gave {text:?}");
}

#[test]
fn sample_svgz_becomes_one_page() {
    let dir = tempfile::tempdir().unwrap();
    let outputs = convert(&common::fixture("sample.svgz"), "svgz", dir.path()).unwrap();
    let (pages, _) = pages_and_media_box(&outputs[0]);
    assert_eq!(pages, 1);
}

#[test]
fn an_svg_with_nothing_to_draw_fails_with_filees_message() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("empty.svg");
    std::fs::write(
        &input,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"></svg>"#,
    )
    .unwrap();
    let err = convert(&input, "svg", dir.path()).unwrap_err();
    assert_eq!(
        err.to_string(),
        "The SVG file has no drawable content or no size."
    );
    assert!(!dir.path().join("out.pdf").exists());
}

#[test]
fn a_skipped_output_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let outputs = PdfEngine::new()
        .convert(
            &step(&common::fixture("sample.svg"), "svg", dir.path()),
            &mut SkipAll,
            &|_| {},
            &CancelToken::new(),
        )
        .unwrap();
    assert!(outputs.is_empty());
}

#[test]
fn a_cancelled_token_stops_before_writing() {
    let dir = tempfile::tempdir().unwrap();
    let cancel = CancelToken::new();
    cancel.cancel();
    let err = PdfEngine::new()
        .convert(
            &step(&common::fixture("sample.svg"), "svg", dir.path()),
            &mut DirAllocator(dir.path().to_path_buf()),
            &|_| {},
            &cancel,
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::Cancelled));
    assert!(!dir.path().join("out.pdf").exists());
}

#[test]
fn a_local_image_next_to_the_svg_is_embedded() {
    // resources_dir is the SVG's folder, so a relative href finds its neighbour.
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(common::fixture("sample.png"), dir.path().join("pic.png")).unwrap();
    let input = dir.path().join("pic.svg");
    std::fs::write(
        &input,
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="40" height="40">
             <image href="pic.png" width="40" height="40"/></svg>"#,
    )
    .unwrap();
    let outputs = convert(&input, "svg", dir.path()).unwrap();
    let doc = lopdf::Document::load(&outputs[0]).unwrap();
    let has_image = doc.objects.values().any(|object| {
        object
            .as_stream()
            .ok()
            .and_then(|s| s.dict.get(b"Subtype").ok())
            .and_then(|s| s.as_name().ok())
            == Some(b"Image".as_slice())
    });
    assert!(has_image, "the PNG next to the SVG should be in the PDF");
}
