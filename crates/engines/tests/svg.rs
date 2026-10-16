//! Golden tests for the built-in SVG engine. No external tools: resvg renders
//! in-process, and PNG sizes are read straight from the IHDR chunk.

#![cfg(feature = "svg")]

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use morphring_core::planner::Planner;
use morphring_core::{
    CancelToken, ConvertOptions, Converter, EngineError, FormatId, OutputAllocator, Step,
};
use morphring_engines::registry;
use morphring_engines::svg::SvgEngine;
use tempfile::TempDir;

use common::fixture;

/// Hands out `<dir>/<stem>.<ext>`, the way the job runner names outputs.
struct DirAllocator {
    dir: PathBuf,
    stem: String,
}

impl OutputAllocator for DirAllocator {
    fn allocate(&mut self, ext: &str, suffix: &str) -> Option<PathBuf> {
        Some(self.dir.join(format!("{}{suffix}.{ext}", self.stem)))
    }
}

/// Runs the engine on `input` and returns the outputs; `tmp` holds them.
fn convert(
    tmp: &TempDir,
    input: &Path,
    from: &'static str,
    to: &'static str,
) -> Result<Vec<PathBuf>, EngineError> {
    let step = Step {
        input: input.to_path_buf(),
        from: FormatId::new(from),
        to: FormatId::new(to),
        options: ConvertOptions::default(),
        work_dir: tmp.path().to_path_buf(),
    };
    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut out = DirAllocator {
        dir: tmp.path().to_path_buf(),
        stem: format!("{stem}-out"),
    };
    SvgEngine::new().convert(&step, &mut out, &|_| {}, &CancelToken::new())
}

fn temp() -> TempDir {
    TempDir::new().unwrap_or_else(|e| panic!("temp dir: {e}"))
}

/// Width and height from a PNG's IHDR chunk.
fn png_size(path: &Path) -> (u32, u32) {
    let bytes = fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    assert!(
        bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "{} is not a PNG",
        path.display()
    );
    let be =
        |at: usize| u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    (be(16), be(20))
}

fn single(result: Result<Vec<PathBuf>, EngineError>) -> PathBuf {
    let outputs = result.unwrap_or_else(|e| panic!("conversion failed: {e}"));
    assert_eq!(outputs.len(), 1, "{outputs:?}");
    let path = outputs.into_iter().next().unwrap_or_default();
    assert!(path.is_file(), "{} was not written", path.display());
    path
}

#[test]
fn sample_svg_renders_at_least_1024_on_the_long_edge() {
    let tmp = temp();
    let png = single(convert(&tmp, &fixture("sample.svg"), "svg", "png"));
    let (w, h) = png_size(&png);
    // 120x80 user units: the 1024 px floor wins over 150 dpi.
    assert_eq!((w, h), (1024, 683));
}

#[test]
fn sample_svgz_renders_the_same_size() {
    let tmp = temp();
    let png = single(convert(&tmp, &fixture("sample.svgz"), "svgz", "png"));
    assert!(png_size(&png).0.max(png_size(&png).1) >= 1024);
}

#[test]
fn svgz_round_trip_gives_back_the_same_text() {
    let tmp = temp();
    let original = fs::read(fixture("sample.svg")).unwrap_or_default();
    let packed = single(convert(&tmp, &fixture("sample.svg"), "svg", "svgz"));
    let packed_bytes = fs::read(&packed).unwrap_or_default();
    assert!(packed_bytes.starts_with(&[0x1f, 0x8b]), "svgz is not gzip");
    assert!(packed_bytes.len() < original.len());

    let back = single(convert(&tmp, &packed, "svgz", "svg"));
    assert_eq!(fs::read(back).unwrap_or_default(), original);
}

#[test]
fn the_committed_svgz_unpacks_to_an_svg() {
    let tmp = temp();
    let svg = single(convert(&tmp, &fixture("sample.svgz"), "svgz", "svg"));
    let text = fs::read_to_string(svg).unwrap_or_default();
    assert!(text.contains("<svg"), "{text}");
}

#[test]
fn a_text_file_is_not_an_svg() {
    let tmp = temp();
    let err = convert(&tmp, &fixture("sample.txt"), "svg", "svgz").unwrap_err();
    assert_eq!(err.to_string(), "the file is not an SVG image");
    let err = convert(&tmp, &fixture("sample.txt"), "svg", "png").unwrap_err();
    assert!(
        err.to_string().starts_with("cannot read the SVG file"),
        "{err}"
    );
}

#[test]
fn an_empty_svg_fails_with_no_drawable_content() {
    let tmp = temp();
    let input = tmp.path().join("empty.svg");
    fs::write(
        &input,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"/>"#,
    )
    .unwrap_or_else(|e| panic!("write: {e}"));
    let err = convert(&tmp, &input, "svg", "png").unwrap_err();
    assert_eq!(
        err.to_string(),
        "the SVG has no drawable content or no size"
    );
}

#[test]
fn a_cancelled_job_writes_nothing() {
    let tmp = temp();
    let cancel = CancelToken::new();
    cancel.cancel();
    let step = Step {
        input: fixture("sample.svg"),
        from: FormatId::new("svg"),
        to: FormatId::new("png"),
        options: ConvertOptions::default(),
        work_dir: tmp.path().to_path_buf(),
    };
    let mut out = DirAllocator {
        dir: tmp.path().to_path_buf(),
        stem: "cancelled".to_owned(),
    };
    let err = SvgEngine::new()
        .convert(&step, &mut out, &|_| {}, &cancel)
        .unwrap_err();
    assert!(matches!(err, EngineError::Cancelled), "{err}");
    assert!(!tmp.path().join("cancelled.png").exists());
}

#[test]
fn svg_to_jpg_routes_through_png() {
    let engines = registry::all();
    let refs: Vec<&dyn Converter> = engines.iter().map(|e| e.as_ref()).collect();
    let planner = Planner::new(&refs, true);
    let route = planner
        .plan(FormatId::new("svg"), FormatId::new("jpg"))
        .unwrap_or_else(|| panic!("svg -> jpg has no route"));
    let hops: Vec<(String, String, String)> = route
        .hops
        .iter()
        .map(|h| {
            (
                engines[h.engine].id().to_owned(),
                h.from.to_string(),
                h.to.to_string(),
            )
        })
        .collect();
    assert_eq!(hops.len(), 2, "{hops:?}");
    assert_eq!(
        hops[0],
        ("resvg".to_owned(), "svg".to_owned(), "png".to_owned())
    );
    assert_eq!((hops[1].1.as_str(), hops[1].2.as_str()), ("png", "jpg"));
}

#[test]
fn svg_and_svgz_repack_directly() {
    let engines = registry::all();
    let refs: Vec<&dyn Converter> = engines.iter().map(|e| e.as_ref()).collect();
    let planner = Planner::new(&refs, true);
    for (from, to) in [("svg", "svgz"), ("svgz", "svg")] {
        let route = planner
            .plan(FormatId::new(from), FormatId::new(to))
            .unwrap_or_else(|| panic!("{from} -> {to} has no route"));
        assert_eq!(route.hops.len(), 1);
        assert_eq!(engines[route.hops[0].engine].id(), "resvg");
    }
}
