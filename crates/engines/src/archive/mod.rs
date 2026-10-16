//! The built-in archive engine: ZIP and tar archives unpacked and packed in plain
//! Rust.
//!
//! Each job lives in its own part: `extract` and `pack`, with the path checks
//! that keep an archive from writing outside its folder in `safety`.

mod extract;
mod pack;
mod safety;

use std::path::PathBuf;
use std::sync::OnceLock;

use morphring_core::{
    CancelToken, Converter, Edge, EngineError, EngineStatus, OutputAllocator, Step,
};

use crate::discovery;
use crate::parts::{Part, Parts};

/// The Built-in archives engine.
#[derive(Clone, Copy, Debug, Default)]
pub struct ArchiveEngine;

impl ArchiveEngine {
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
                edges: extract::edges,
                convert: extract::convert,
            },
            Part {
                edges: pack::edges,
                convert: pack::convert,
            },
        ])
    })
}

impl Converter for ArchiveEngine {
    fn id(&self) -> &'static str {
        "archive"
    }

    fn display_name(&self) -> &'static str {
        "Built-in archives"
    }

    fn edges(&self) -> &[Edge] {
        parts().edges()
    }

    fn status(&self) -> EngineStatus {
        discovery::builtin_status()
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
    fn the_engine_has_an_id_and_the_extract_edges() {
        let engine = ArchiveEngine::new();
        assert_eq!(engine.id(), "archive");
        assert_eq!(engine.display_name(), "Built-in archives");
        assert!(
            engine
                .edges()
                .iter()
                .any(|e| e.from.as_str() == "zip" && e.to.as_str() == "folder")
        );
    }

    #[test]
    fn a_pair_without_an_edge_is_refused() {
        let err = ArchiveEngine::new()
            .convert(
                &step("pdf", "png"),
                &mut NoOutputs,
                &|_| {},
                &CancelToken::new(),
            )
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Built-in archives cannot convert pdf to png"
        );
    }

    #[test]
    fn the_pack_part_is_still_a_stub() {
        let parts: [crate::parts::ConvertFn; 1] = [pack::convert];
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
            ArchiveEngine::new().status(),
            EngineStatus::Available {
                path: PathBuf::new(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            }
        );
    }
}
