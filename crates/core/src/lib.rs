//! Core of MorphRing: the format registry, the converter contract, route
//! planning, output naming and the job runner. Nothing here launches a process;
//! engines live in `morphring-engines`.

pub mod contract;
pub mod error;
pub mod formats;
pub mod job;
pub mod output;
pub mod planner;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use contract::{
    CancelToken, ConvertOptions, Converter, Edge, EngineStatus, FileResult, FileState, FormatId,
    JobState, OutputAllocator, QualityLevel, Step,
};
pub use error::{EngineError, ErrorKey};
