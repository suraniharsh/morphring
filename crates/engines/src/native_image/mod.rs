//! The built-in image engine: raster formats decoded and encoded in plain Rust
//! with the `image` crate, no ImageMagick required.
//!
//! This is the skeleton. It has no edges yet, so the planner never picks it;
//! the native image task fills in the formats.

use std::path::PathBuf;

use morphring_core::{
    CancelToken, Converter, Edge, EngineError, EngineStatus, OutputAllocator, Step,
};

use crate::discovery;

/// The Built-in images engine.
#[derive(Clone, Copy, Debug, Default)]
pub struct ImageEngine;

impl ImageEngine {
    /// A new engine.
    pub fn new() -> Self {
        Self
    }
}

impl Converter for ImageEngine {
    fn id(&self) -> &'static str {
        "image"
    }

    fn display_name(&self) -> &'static str {
        "Built-in images"
    }

    fn edges(&self) -> &[Edge] {
        // An engine with zero edges is the purest kind: it cannot do anything
        // wrong because it cannot do anything. The image task fixes that.
        &[]
    }

    fn status(&self) -> EngineStatus {
        discovery::builtin_status()
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
        let engine = ImageEngine::new();
        assert_eq!(engine.id(), "image");
        assert_eq!(engine.display_name(), "Built-in images");
        assert!(engine.edges().is_empty());
    }

    #[test]
    fn the_skeleton_refuses_to_convert() {
        let err = ImageEngine::new()
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
    fn a_builtin_engine_is_always_available() {
        assert_eq!(
            ImageEngine::new().status(),
            EngineStatus::Available {
                path: PathBuf::new(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            }
        );
    }
}
