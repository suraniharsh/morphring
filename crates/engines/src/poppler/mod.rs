//! The Poppler engine: PDF pages rendered to images with `pdftoppm`, and text
//! pulled out with `pdftotext`.
//!
//! Each job lives in its own part: `render` and `text`.

mod render;
mod text;

use std::path::PathBuf;
use std::sync::OnceLock;

use morphring_core::{
    CancelToken, Converter, Edge, EngineError, EngineStatus, OutputAllocator, Step,
};

use crate::discovery::{self, Tool};
use crate::parts::{Part, Parts};

/// The Poppler engine.
#[derive(Clone, Copy, Debug, Default)]
pub struct PopplerEngine;

impl PopplerEngine {
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
                edges: render::edges,
                convert: render::convert,
            },
            Part {
                edges: text::edges,
                convert: text::convert,
            },
        ])
    })
}

impl Converter for PopplerEngine {
    fn id(&self) -> &'static str {
        Tool::Pdftoppm.id()
    }

    fn display_name(&self) -> &'static str {
        Tool::Pdftoppm.display_name()
    }

    fn edges(&self) -> &[Edge] {
        parts().edges()
    }

    fn status(&self) -> EngineStatus {
        discovery::status(Tool::Pdftoppm)
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
    fn the_engine_has_an_id_and_both_edges() {
        let engine = PopplerEngine::new();
        assert_eq!(engine.id(), "poppler");
        assert_eq!(engine.display_name(), "Poppler");
        let edges = engine.edges();
        assert!(
            edges
                .iter()
                .any(|e| e.from.as_str() == "pdf" && e.to.as_str() == "png")
        );
        assert!(
            edges
                .iter()
                .any(|e| e.from.as_str() == "pdf" && e.to.as_str() == "txt")
        );
    }

    #[test]
    fn a_pair_without_an_edge_is_refused() {
        let err = PopplerEngine::new()
            .convert(
                &step("pdf", "mp3"),
                &mut NoOutputs,
                &|_| {},
                &CancelToken::new(),
            )
            .unwrap_err();
        assert_eq!(err.to_string(), "Poppler cannot convert pdf to mp3");
    }

    #[test]
    fn status_comes_from_discovery() {
        assert_eq!(
            PopplerEngine::new().status(),
            discovery::status(Tool::Pdftoppm)
        );
    }
}
