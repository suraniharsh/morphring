//! SVG to PDF as vector graphics, through krilla-svg.
//!
//! usvg parses the file (gzipped SVGZ included), krilla-svg draws the tree onto
//! one PDF page sized like the SVG. Text stays real, selectable text with
//! subsetted fonts, so we skip the glyph-outline gymnastics Filee needed to
//! keep Skia from embedding a 13 MB font for one Korean word.

use std::path::{Path, PathBuf};

use krilla::Document;
use krilla::geom::Size;
use krilla::metadata::Metadata;
use krilla::page::PageSettings;
use krilla_svg::{SurfaceExt, SvgSettings};
use morphring_core::{CancelToken, Edge, EngineError, OutputAllocator, Step};

use crate::process;

/// Filee's message for an SVG that parses but has nothing to put on a page.
const NO_CONTENT: &str = "The SVG file has no drawable content or no size.";

/// CSS pixels are 1/96 in, PDF points are 1/72 in.
const PX_TO_PT: f32 = 72.0 / 96.0;

/// The edges this part handles.
pub(crate) fn edges() -> Vec<Edge> {
    vec![
        Edge::with_cost("svg", "pdf", 10),
        Edge::with_cost("svgz", "pdf", 10),
    ]
}

/// Converts `step`, which is one of [`edges`].
pub(crate) fn convert(
    step: &Step,
    out: &mut dyn OutputAllocator,
    progress: &dyn Fn(f32),
    cancel: &CancelToken,
) -> Result<Vec<PathBuf>, EngineError> {
    if cancel.is_cancelled() {
        return Err(EngineError::Cancelled);
    }
    let data = std::fs::read(&step.input)?;
    let pdf = svg_to_pdf(&data, &step.input)?;
    progress(0.8);

    if cancel.is_cancelled() {
        return Err(EngineError::Cancelled);
    }
    let Some(target) = out.allocate("pdf", "") else {
        return Ok(Vec::new());
    };

    // Same dance as every other engine: write in a private temp dir, then move
    // it into place, so a half-written PDF never sits next to the user's files.
    std::fs::create_dir_all(&step.work_dir)?;
    let temp = tempfile::Builder::new()
        .prefix("svg-pdf-")
        .tempdir_in(&step.work_dir)?;
    let scratch = temp.path().join("out.pdf");
    std::fs::write(&scratch, pdf)?;
    process::move_into_place(&scratch, &target)?;
    progress(1.0);
    Ok(vec![target])
}

/// Renders the SVG (or gzipped SVGZ) in `data` to the bytes of a one-page PDF.
///
/// `input` is only used to find the folder that relative image references
/// resolve against.
fn svg_to_pdf(data: &[u8], input: &Path) -> Result<Vec<u8>, EngineError> {
    let options = usvg::Options {
        // Local files next to the SVG only. usvg has no network code at all,
        // which is the kind of security feature I can get behind.
        resources_dir: input.parent().map(Path::to_path_buf),
        fontdb: crate::svg::fonts(),
        ..usvg::Options::default()
    };
    // from_data sniffs the 1F 8B gzip header itself, so svgz needs no extra work.
    let tree = usvg::Tree::from_data(data, &options)
        .map_err(|e| EngineError::Failed(format!("Cannot read the SVG: {e}")))?;

    let size = tree.size();
    let (width, height) = (size.width(), size.height());
    let has_size = width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0;
    if !has_size || !tree.root().has_children() {
        return Err(EngineError::Failed(NO_CONTENT.to_owned()));
    }

    let page_size = Size::from_wh(width * PX_TO_PT, height * PX_TO_PT)
        .ok_or_else(|| EngineError::Failed(NO_CONTENT.to_owned()))?;

    let mut document = Document::new();
    document.set_metadata(
        Metadata::new()
            .creator("MorphRing".to_owned())
            .producer(format!("MorphRing {}", env!("CARGO_PKG_VERSION"))),
    );
    let mut page = document.start_page_with(PageSettings::new(page_size));
    let mut surface = page.surface();
    surface
        .draw_svg(&tree, page_size, SvgSettings::default())
        .ok_or_else(|| EngineError::Failed("Cannot draw the SVG onto the PDF page.".to_owned()))?;
    surface.finish();
    page.finish();

    document
        .finish()
        .map_err(|e| EngineError::Failed(format!("Cannot write the PDF: {e:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQUARE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="96" height="48">
        <rect width="96" height="48" fill="red"/></svg>"#;

    #[test]
    fn both_edges_cost_ten() {
        let pairs: Vec<(String, String, u32)> = edges()
            .iter()
            .map(|e| (e.from.to_string(), e.to.to_string(), e.cost))
            .collect();
        assert_eq!(
            pairs,
            [
                ("svg".to_owned(), "pdf".to_owned(), 10),
                ("svgz".to_owned(), "pdf".to_owned(), 10)
            ]
        );
    }

    #[test]
    fn a_page_is_the_svg_size_in_points() {
        let pdf = svg_to_pdf(SQUARE.as_bytes(), Path::new("x.svg")).unwrap();
        assert!(pdf.starts_with(b"%PDF-"));
        // 96 x 48 px is 72 x 36 pt.
        let doc = lopdf::Document::load_mem(&pdf).unwrap();
        let pages = doc.get_pages();
        assert_eq!(pages.len(), 1);
        let page = doc.get_dictionary(pages[&1]).unwrap();
        let media_box: Vec<f32> = page
            .get(b"MediaBox")
            .and_then(lopdf::Object::as_array)
            .unwrap()
            .iter()
            .map(|n| n.as_float().unwrap())
            .collect();
        assert_eq!(media_box, [0.0, 0.0, 72.0, 36.0]);
    }

    #[test]
    fn an_empty_svg_fails_with_the_filee_message() {
        let empty = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"/>"#;
        let err = svg_to_pdf(empty.as_bytes(), Path::new("x.svg")).unwrap_err();
        assert_eq!(err.to_string(), NO_CONTENT);
    }

    #[test]
    fn garbage_is_not_an_svg() {
        let err = svg_to_pdf(b"definitely not xml", Path::new("x.svg")).unwrap_err();
        assert!(err.to_string().starts_with("Cannot read the SVG"), "{err}");
    }
}
