//! Raster images to PDF, one image per page.
//!
//! This is the skeleton: no edges yet, and [`convert`] refuses everything.

use std::path::PathBuf;

use morphring_core::{CancelToken, Edge, EngineError, OutputAllocator, Step};

/// The edges this part handles.
pub(crate) fn edges() -> Vec<Edge> {
    // Empty on purpose: an edge here would put a route in front of the planner
    // before there is any code behind it.
    Vec::new()
}

/// Converts `step`, which is one of [`edges`].
pub(crate) fn convert(
    _step: &Step,
    _out: &mut dyn OutputAllocator,
    _progress: &dyn Fn(f32),
    _cancel: &CancelToken,
) -> Result<Vec<PathBuf>, EngineError> {
    Err(crate::parts::not_implemented())
}
