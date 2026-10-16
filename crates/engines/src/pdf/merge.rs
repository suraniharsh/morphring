//! Merging several PDFs (or images) into one PDF, the PDF engine's merge mode.
//!
//! This is the skeleton: there is no merger yet, so the merge mode stays off.

use morphring_core::Merger;

/// The PDF engine's merger, or `None` while there is none.
pub(crate) fn merger() -> Option<&'static dyn Merger> {
    // None is the most honest merger there is: it never loses a page.
    None
}
