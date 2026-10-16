//! Images to PDF through the built-in PDF engine (krilla), checked with lopdf.
//!
//! Nothing here needs an external program except the multi-page TIFF test,
//! which asks ImageMagick to build its fixture and skips without it.

#![cfg(feature = "pdf")]

mod common;

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use lopdf::{Document, Object};
use morphring_core::planner::Planner;
use morphring_core::{
    CancelToken, ConvertOptions, Converter, EngineError, FormatId, OutputAllocator, PageSize, Step,
};
use morphring_engines::pdf::PdfEngine;
use morphring_engines::registry;
use tempfile::TempDir;

use common::fixture;

/// Hands out `<dir>/<stem><suffix>.<ext>`, like the job's allocator would.
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

fn run_with(
    input: &Path,
    from: &'static str,
    options: ConvertOptions,
    out: Option<&mut dyn OutputAllocator>,
    cancel: &CancelToken,
) -> Run {
    let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e:?}"));
    let out_dir = tmp.path().join("out");
    let work_dir = tmp.path().join("work");
    std::fs::create_dir_all(&out_dir).unwrap_or_else(|e| panic!("out dir: {e:?}"));
    std::fs::create_dir_all(&work_dir).unwrap_or_else(|e| panic!("work dir: {e:?}"));
    let step = Step {
        input: input.to_path_buf(),
        from: FormatId::new(from),
        to: FormatId::new("pdf"),
        options,
        work_dir,
    };
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("out")
        .to_owned();
    let mut alloc = DirAllocator {
        dir: out_dir.clone(),
        stem,
    };
    let out: &mut dyn OutputAllocator = match out {
        Some(out) => out,
        None => &mut alloc,
    };
    let progress = RefCell::new(Vec::new());
    let result = PdfEngine::new().convert(&step, out, &|p| progress.borrow_mut().push(p), cancel);
    Run {
        _tmp: tmp,
        out_dir,
        result,
        progress: progress.into_inner(),
    }
}

fn run(input: &Path, from: &'static str, options: ConvertOptions) -> Run {
    run_with(input, from, options, None, &CancelToken::new())
}

/// The single PDF a run produced, loaded.
fn only_pdf(run: &Run) -> (PathBuf, Document) {
    let outputs = run
        .result
        .as_ref()
        .unwrap_or_else(|e| panic!("conversion succeeds: {e:?}"));
    assert_eq!(outputs.len(), 1, "one PDF per image: {outputs:?}");
    let path = outputs[0].clone();
    assert!(path.starts_with(&run.out_dir), "{path:?} is in the out dir");
    let doc = Document::load(&path).unwrap_or_else(|e| panic!("lopdf can read the PDF: {e:?}"));
    (path, doc)
}

fn number(o: &Object) -> f32 {
    match o {
        Object::Integer(i) => *i as f32,
        Object::Real(r) => *r,
        other => panic!("not a number: {other:?}"),
    }
}

/// MediaBox of page `n` (1-based) as [x0, y0, x1, y1].
fn media_box(doc: &Document, n: u32) -> Vec<f32> {
    let id = doc.get_pages()[&n];
    let page = doc
        .get_dictionary(id)
        .unwrap_or_else(|e| panic!("page dictionary: {e:?}"));
    page.get(b"MediaBox")
        .and_then(Object::as_array)
        .unwrap_or_else(|e| panic!("MediaBox: {e:?}"))
        .iter()
        .map(number)
        .collect()
}

#[test]
fn png_to_pdf_routes_to_the_builtin_pdf_engine() {
    let engines = registry::all();
    let refs: Vec<&dyn Converter> = engines.iter().map(|e| e.as_ref()).collect();
    let planner = Planner::new(&refs, true);
    for from in ["png", "jpg", "gif", "bmp", "tiff"] {
        let route = planner
            .plan(FormatId::new(from), FormatId::new("pdf"))
            .unwrap_or_else(|| panic!("{from} -> pdf has a route"));
        let ids: Vec<&str> = route.hops.iter().map(|h| engines[h.engine].id()).collect();
        assert_eq!(ids, ["pdf"], "{from} -> pdf");
    }
}

#[test]
fn sample_png_becomes_a_one_page_pdf_at_its_own_size() {
    let run = run(&fixture("sample.png"), "png", ConvertOptions::default());
    let (path, doc) = only_pdf(&run);
    assert_eq!(path.file_name().unwrap(), "sample.pdf");
    assert_eq!(doc.get_pages().len(), 1);
    // 64 x 48 px with no density: 96 DPI, so 48 x 36 pt.
    let mb = media_box(&doc, 1);
    assert!(
        (mb[2] - 48.0).abs() < 0.01 && (mb[3] - 36.0).abs() < 0.01,
        "{mb:?}"
    );
    assert_eq!(run.progress.last(), Some(&1.0));
    // Nothing left behind but the PDF.
    let names: Vec<_> = std::fs::read_dir(&run.out_dir).unwrap().collect();
    assert_eq!(names.len(), 1);
}

#[test]
fn sample_jpg_is_embedded_byte_for_byte() {
    let original = std::fs::read(fixture("sample.jpg")).unwrap();
    let run = run(&fixture("sample.jpg"), "jpg", ConvertOptions::default());
    let (_, doc) = only_pdf(&run);
    assert_eq!(doc.get_pages().len(), 1);
    let jpeg_streams: Vec<&lopdf::Stream> = doc
        .objects
        .values()
        .filter_map(|o| o.as_stream().ok())
        .filter(|s| {
            s.dict
                .get(b"Filter")
                .and_then(Object::as_name)
                .is_ok_and(|n| n == b"DCTDecode")
        })
        .collect();
    assert_eq!(jpeg_streams.len(), 1, "one DCTDecode image");
    assert!(
        jpeg_streams[0].content == original,
        "the JPEG went in untouched"
    );
}

#[test]
fn a4_gives_a_portrait_a4_media_box() {
    let mut options = ConvertOptions::default();
    options.pdf.page_size = PageSize::A4;
    options.pdf.margin_mm = 10.0;
    // sample.png is wider than tall, so take the tall one: the gif is square,
    // which counts as portrait too.
    let run = run(&fixture("sample.gif"), "gif", options);
    let (_, doc) = only_pdf(&run);
    let mb = media_box(&doc, 1);
    assert_eq!([mb[2].round(), mb[3].round()], [595.0, 842.0], "{mb:?}");
}

#[test]
fn a_wide_image_on_a4_turns_landscape() {
    let mut options = ConvertOptions::default();
    options.pdf.page_size = PageSize::A4;
    let run = run(&fixture("sample.png"), "png", options);
    let (_, doc) = only_pdf(&run);
    let mb = media_box(&doc, 1);
    assert_eq!([mb[2].round(), mb[3].round()], [842.0, 595.0], "{mb:?}");
}

#[test]
fn every_gif_frame_becomes_a_page_with_rising_progress() {
    let run = run(&fixture("sample.gif"), "gif", ConvertOptions::default());
    let (_, doc) = only_pdf(&run);
    assert_eq!(doc.get_pages().len(), 3);
    assert_eq!(run.progress.len(), 3);
    assert!((run.progress[0] - 1.0 / 3.0).abs() < 1e-6);
    assert!(run.progress.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(run.progress.last(), Some(&1.0));
}

#[test]
fn every_page_of_a_multi_page_tiff_becomes_a_page() {
    require_engine!("magick");
    let tmp = tempfile::tempdir().unwrap();
    let tiff = tmp.path().join("pages.tiff");
    let status = std::process::Command::new("magick")
        .args([
            "-units",
            "PixelsPerInch",
            "-density",
            "72",
            "-size",
            "40x20",
            "xc:red",
            "xc:lime",
            "xc:blue",
        ])
        .arg(&tiff)
        .status()
        .expect("magick runs");
    assert!(status.success());
    let run = run(&tiff, "tiff", ConvertOptions::default());
    let (_, doc) = only_pdf(&run);
    assert_eq!(doc.get_pages().len(), 3);
    // 72 DPI: one pixel per point.
    for n in 1..=3 {
        let mb = media_box(&doc, n);
        assert!(
            (mb[2] - 40.0).abs() < 0.01 && (mb[3] - 20.0).abs() < 0.01,
            "{mb:?}"
        );
    }
}

#[test]
fn transparent_png_keeps_its_alpha_as_a_soft_mask() {
    require_engine!("magick");
    let tmp = tempfile::tempdir().unwrap();
    let png = tmp.path().join("clear.png");
    let status = std::process::Command::new("magick")
        .args(["-size", "8x8", "xc:#ff000080"])
        .arg(format!("PNG32:{}", png.display()))
        .status()
        .expect("magick runs");
    assert!(status.success());
    let run = run(&png, "png", ConvertOptions::default());
    let (_, doc) = only_pdf(&run);
    let has_smask = doc
        .objects
        .values()
        .filter_map(|o| o.as_stream().ok())
        .any(|s| s.dict.get(b"SMask").is_ok());
    assert!(has_smask, "the image carries an SMask");
}

#[test]
fn a_skipped_output_writes_nothing() {
    let mut skip = SkipAll;
    let run = run_with(
        &fixture("sample.png"),
        "png",
        ConvertOptions::default(),
        Some(&mut skip),
        &CancelToken::new(),
    );
    assert_eq!(run.result.unwrap(), Vec::<PathBuf>::new());
    assert_eq!(std::fs::read_dir(&run.out_dir).unwrap().count(), 0);
}

#[test]
fn a_cancelled_job_writes_nothing() {
    let cancel = CancelToken::new();
    cancel.cancel();
    let run = run_with(
        &fixture("sample.png"),
        "png",
        ConvertOptions::default(),
        None,
        &cancel,
    );
    assert!(matches!(run.result, Err(EngineError::Cancelled)));
    assert_eq!(std::fs::read_dir(&run.out_dir).unwrap().count(), 0);
}

#[test]
fn a_broken_image_fails_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    let bad = tmp.path().join("bad.png");
    std::fs::write(&bad, b"definitely not a png").unwrap();
    let run = run(&bad, "png", ConvertOptions::default());
    match run.result {
        Err(EngineError::Failed(msg)) => assert!(msg.contains("could not read the image")),
        other => panic!("expected a failure, got {other:?}"),
    }
}
