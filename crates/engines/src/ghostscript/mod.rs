//! The Ghostscript engine: PDF, EPS and PostScript through `gs`.
//!
//! This is the skeleton. It finds `gs` and reports its status, but has no
//! edges yet, so the planner never picks it; the Ghostscript task adds them.

use std::path::PathBuf;

use morphring_core::{
    CancelToken, Converter, Edge, EngineError, EngineStatus, OutputAllocator, Step,
};

use crate::discovery::{self, Tool};

/// The Ghostscript engine.
#[derive(Clone, Copy, Debug, Default)]
pub struct GhostscriptEngine;

impl GhostscriptEngine {
    /// A new engine.
    pub fn new() -> Self {
        Self
    }
}

impl Converter for GhostscriptEngine {
    fn id(&self) -> &'static str {
        Tool::Ghostscript.id()
    }

    fn display_name(&self) -> &'static str {
        Tool::Ghostscript.display_name()
    }

    fn edges(&self) -> &[Edge] {
        // gs can do roughly everything to a PDF, which is exactly why it gets no
        // edges until someone decides which everything we want.
        &[]
    }

    fn status(&self) -> EngineStatus {
        discovery::status(Tool::Ghostscript)
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
        let engine = GhostscriptEngine::new();
        assert_eq!(engine.id(), "ghostscript");
        assert_eq!(engine.display_name(), "Ghostscript");
        assert!(engine.edges().is_empty());
    }

    #[test]
    fn the_skeleton_refuses_to_convert() {
        let err = GhostscriptEngine::new()
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
            GhostscriptEngine::new().status(),
            discovery::status(Tool::Ghostscript)
        );
    }
}
