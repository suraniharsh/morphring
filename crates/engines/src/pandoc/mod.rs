//! The Pandoc engine: Markdown, HTML, DOCX, EPUB and the other text formats
//! through `pandoc`.
//!
//! This is the skeleton. It finds `pandoc` and reports its status, but has no
//! edges yet, so the planner never picks it; the Pandoc task adds them.

use std::path::PathBuf;

use morphring_core::{
    CancelToken, Converter, Edge, EngineError, EngineStatus, OutputAllocator, Step,
};

use crate::discovery::{self, Tool};

/// The Pandoc engine.
#[derive(Clone, Copy, Debug, Default)]
pub struct PandocEngine;

impl PandocEngine {
    /// A new engine.
    pub fn new() -> Self {
        Self
    }
}

impl Converter for PandocEngine {
    fn id(&self) -> &'static str {
        Tool::Pandoc.id()
    }

    fn display_name(&self) -> &'static str {
        Tool::Pandoc.display_name()
    }

    fn edges(&self) -> &[Edge] {
        // Pandoc converts between more formats than I can name, so for now it
        // converts between none. Baby steps.
        &[]
    }

    fn status(&self) -> EngineStatus {
        discovery::status(Tool::Pandoc)
    }

    fn convert(
        &self,
        _step: &Step,
        _out: &mut dyn OutputAllocator,
        _progress: &dyn Fn(f32),
        _cancel: &CancelToken,
    ) -> Result<Vec<PathBuf>, EngineError> {
        Err(crate::parts::not_implemented())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parts::test_support::{NoOutputs, step};

    #[test]
    fn the_skeleton_has_an_id_and_no_edges() {
        let engine = PandocEngine::new();
        assert_eq!(engine.id(), "pandoc");
        assert_eq!(engine.display_name(), "Pandoc");
        assert!(engine.edges().is_empty());
    }

    #[test]
    fn the_skeleton_refuses_to_convert() {
        let err = PandocEngine::new()
            .convert(
                &step("pdf", "png"),
                &mut NoOutputs,
                &|_| {},
                &CancelToken::new(),
            )
            .unwrap_err();
        assert_eq!(err.to_string(), "not implemented");
    }

    #[test]
    fn status_comes_from_discovery() {
        assert_eq!(
            PandocEngine::new().status(),
            discovery::status(Tool::Pandoc)
        );
    }
}
