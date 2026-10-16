//! PDF to PDF with lopdf: keep a page range, split into single pages, or just
//! re-save (Filee's `PdfSharpConverter.SplitOrExtract`).
//!
//! lopdf copies objects, not pixels, so links, annotations and form fields on
//! the pages we keep come along for the ride.

use std::path::{Path, PathBuf};

use lopdf::{Document, LoadOptions};
use morphring_core::page_range;
use morphring_core::{CancelToken, Edge, EngineError, OutputAllocator, Step};

use crate::process::move_into_place;

/// The most any single stream may inflate to while loading. A real PDF's
/// object streams are a few MB at worst; a zip bomb wearing a PDF costume is
/// not, and it gets turned away at the door.
const MAX_DECOMPRESSED_BYTES: usize = 256 * 1024 * 1024;

/// The edges this part handles.
pub(crate) fn edges() -> Vec<Edge> {
    // A self edge: pdf in, pdf out, with the page range or split in between.
    vec![Edge::new("pdf", "pdf")]
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
    let doc = load(&step.input)?;
    let page_count = doc.get_pages().len();
    if page_count == 0 {
        return Err(EngineError::Failed("The PDF has no pages.".to_owned()));
    }

    std::fs::create_dir_all(&step.work_dir)?;
    let scratch = tempfile::Builder::new()
        .prefix("pdf-pages-")
        .tempdir_in(&step.work_dir)?;

    let options = &step.options.pdf;
    if options.split_pages {
        let pages = page_range::parse(&options.page_range, page_count);
        return split(&doc, &pages, scratch.path(), out, progress, cancel);
    }

    let Some(path) = out.allocate("pdf", "") else {
        return Ok(Vec::new());
    };
    let mut doc = doc;
    // A blank range means "every page", and keeping every page is a re-save,
    // which is exactly what Filee does for a same-format drop.
    if !options.page_range.trim().is_empty() {
        let pages = page_range::parse(&options.page_range, page_count);
        keep_only(&mut doc, &pages);
    }
    if cancel.is_cancelled() {
        return Err(EngineError::Cancelled);
    }
    save_into_place(&mut doc, scratch.path(), &path)?;
    progress(1.0);
    Ok(vec![path])
}

/// One file per page in `pages` (0-based), named with Filee's `_p{page}`
/// suffix (1-based). Removes whatever it wrote if it fails or is cancelled.
fn split(
    doc: &Document,
    pages: &[usize],
    scratch: &Path,
    out: &mut dyn OutputAllocator,
    progress: &dyn Fn(f32),
    cancel: &CancelToken,
) -> Result<Vec<PathBuf>, EngineError> {
    let mut written: Vec<PathBuf> = Vec::new();
    let total = pages.len().max(1) as f32;
    for (done, &index) in pages.iter().enumerate() {
        if cancel.is_cancelled() {
            remove_all(&written);
            return Err(EngineError::Cancelled);
        }
        // A skipped page (the file exists and the user said skip) still
        // counts as progress, or the bar would stall short of the finish.
        if let Some(path) = out.allocate("pdf", &format!("_p{}", index + 1)) {
            // Clone, then throw away everything but one page. Wasteful on
            // paper, but it keeps every shared font and image intact without
            // us playing object surgeon.
            let mut single = doc.clone();
            keep_only(&mut single, &[index]);
            if let Err(err) = save_into_place(&mut single, scratch, &path) {
                remove_all(&written);
                return Err(err);
            }
            written.push(path);
        }
        progress((done + 1) as f32 / total);
    }
    Ok(written)
}

/// Loads `path` with a decompression cap and refuses encrypted files.
fn load(path: &Path) -> Result<Document, EngineError> {
    let doc = Document::load_with_options(
        path,
        LoadOptions::with_max_decompressed_size(MAX_DECOMPRESSED_BYTES),
    )
    .map_err(|err| EngineError::Failed(format!("The PDF could not be read: {err}")))?;
    // lopdf quietly opens files whose user password is empty, and still has
    // the /Encrypt entry when it could not. Saving either would hand back a
    // file with its protection silently stripped, so both are a no.
    if doc.is_encrypted() {
        return Err(EngineError::Failed(
            "The PDF is password-protected. Remove the password and try again.".to_owned(),
        ));
    }
    if doc.was_encrypted() {
        return Err(EngineError::Failed(
            "The PDF is encrypted. Remove its protection and try again.".to_owned(),
        ));
    }
    Ok(doc)
}

/// Deletes every page whose 0-based index is not in `keep`, then drops the
/// objects nobody points at any more and compresses what is left.
fn keep_only(doc: &mut Document, keep: &[usize]) {
    let doomed: Vec<u32> = doc
        .get_pages()
        .into_keys()
        .filter(|&number| !keep.contains(&(number as usize - 1)))
        .collect();
    if doomed.is_empty() {
        return;
    }
    doc.delete_pages(&doomed);
    doc.prune_objects();
    doc.compress();
}

/// Saves `doc` to a temp file in `scratch`, then moves it to `to`, so a
/// failed save never leaves half a PDF under the user's chosen name.
fn save_into_place(doc: &mut Document, scratch: &Path, to: &Path) -> Result<(), EngineError> {
    let tmp = scratch.join("out.pdf");
    doc.save(&tmp)
        .map_err(|err| EngineError::Failed(format!("The PDF could not be written: {err}")))?;
    move_into_place(&tmp, to)?;
    Ok(())
}

/// Best-effort removal of outputs written before a failure or a cancel.
fn remove_all(paths: &[PathBuf]) {
    for path in paths {
        // Already gone is fine: we only care that it is not left behind.
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_only_edge_is_pdf_to_pdf_at_the_default_cost() {
        assert_eq!(edges(), vec![Edge::with_cost("pdf", "pdf", 10)]);
    }

    #[test]
    fn a_missing_file_is_a_read_error() {
        let err = load(Path::new("does-not-exist.pdf")).unwrap_err();
        assert!(
            err.to_string().starts_with("The PDF could not be read"),
            "{err}"
        );
    }
}
