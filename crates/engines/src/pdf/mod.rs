//! The built-in PDF engine: images and SVG to PDF, page operations and merging,
//! in plain Rust (krilla for writing, lopdf for reading).
//!
//! Each job lives in its own part: `image_to_pdf`, `pages` and `svg_pdf` for
//! conversions, and `merge` for the merge mode. This is the skeleton: every
//! part is a stub, so the engine has no edges and no merger yet.

mod image_to_pdf;
mod merge;
mod pages;
mod svg_pdf;

use std::path::PathBuf;
use std::sync::OnceLock;

use morphring_core::{
    CancelToken, Converter, Edge, EngineError, EngineStatus, Merger, OutputAllocator, Step,
};

use crate::discovery;
use crate::parts::{Part, Parts};

/// The Built-in PDF engine.
#[derive(Clone, Copy, Debug, Default)]
pub struct PdfEngine;

impl PdfEngine {
    /// A new engine.
    pub fn new() -> Self {
        Self
    }
}

/// Every part, with their edges collected on first use.
fn parts() -> &'static Parts {
    static PARTS: OnceLock<Parts> = OnceLock::new();
    PARTS.get_or_init(|| {
        Parts::new(&[
            Part {
                edges: image_to_pdf::edges,
                convert: image_to_pdf::convert,
            },
            Part {
                edges: pages::edges,
                convert: pages::convert,
            },
            Part {
                edges: svg_pdf::edges,
                convert: svg_pdf::convert,
            },
        ])
    })
}

impl Converter for PdfEngine {
    fn id(&self) -> &'static str {
        "pdf"
    }

    fn display_name(&self) -> &'static str {
        "Built-in PDF"
    }

    fn edges(&self) -> &[Edge] {
        parts().edges()
    }

    fn status(&self) -> EngineStatus {
        discovery::builtin_status()
    }

    fn merger(&self) -> Option<&dyn Merger> {
        merge::merger()
    }

    fn convert(
        &self,
        step: &Step,
        out: &mut dyn OutputAllocator,
        progress: &dyn Fn(f32),
        cancel: &CancelToken,
    ) -> Result<Vec<PathBuf>, EngineError> {
        parts().convert(self.display_name(), step, out, progress, cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parts::test_support::{NoOutputs, step};

    #[test]
    fn the_engine_has_an_id_and_image_edges() {
        let engine = PdfEngine::new();
        assert_eq!(engine.id(), "pdf");
        assert_eq!(engine.display_name(), "Built-in PDF");
        assert!(engine.edges().iter().any(|e| e.from.as_str() == "png"));
        assert!(engine.edges().iter().any(|e| e.from.as_str() == "svg"));
    }

    #[test]
    fn a_pair_without_an_edge_is_refused() {
        let err = PdfEngine::new()
            .convert(
                &step("pdf", "png"),
                &mut NoOutputs,
                &|_| {},
                &CancelToken::new(),
            )
            .unwrap_err();
        assert_eq!(err.to_string(), "Built-in PDF cannot convert pdf to png");
    }

    #[test]
    fn every_part_is_a_stub_for_now() {
        let parts: [crate::parts::ConvertFn; 1] = [pages::convert];
        for convert in parts {
            let err = convert(
                &step("pdf", "png"),
                &mut NoOutputs,
                &|_| {},
                &CancelToken::new(),
            )
            .unwrap_err();
            assert_eq!(err.to_string(), "not implemented");
        }
    }

    #[test]
    fn a_builtin_engine_is_always_available() {
        assert_eq!(
            PdfEngine::new().status(),
            EngineStatus::Available {
                path: PathBuf::new(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            }
        );
    }

    #[test]
    fn there_is_no_merger_yet() {
        assert!(PdfEngine::new().merger().is_none());
    }
}
