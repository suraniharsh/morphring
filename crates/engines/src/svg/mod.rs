//! The built-in SVG engine: SVG and SVGZ rendered to raster images with resvg.
//!
//! This is the skeleton. It has no edges yet, so the planner never picks it;
//! the SVG task fills in the formats.

use std::path::PathBuf;

use morphring_core::{
    CancelToken, Converter, Edge, EngineError, EngineStatus, OutputAllocator, Step,
};

use crate::discovery;

/// The Built-in SVG engine.
#[derive(Clone, Copy, Debug, Default)]
pub struct SvgEngine;

impl SvgEngine {
    /// A new engine.
    pub fn new() -> Self {
        Self
    }
}

impl Converter for SvgEngine {
    fn id(&self) -> &'static str {
        "resvg"
    }

    fn display_name(&self) -> &'static str {
        "Built-in SVG"
    }

    fn edges(&self) -> &[Edge] {
        // Nothing to rasterize yet. resvg is waiting in Cargo.toml, tapping its foot.
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
        let engine = SvgEngine::new();
        assert_eq!(engine.id(), "resvg");
        assert_eq!(engine.display_name(), "Built-in SVG");
        assert!(engine.edges().is_empty());
    }

    #[test]
    fn the_skeleton_refuses_to_convert() {
        let err = SvgEngine::new()
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
            SvgEngine::new().status(),
            EngineStatus::Available {
                path: PathBuf::new(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            }
        );
    }
}
