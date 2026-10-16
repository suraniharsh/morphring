//! Core of MorphRing: the format registry, the converter contract, route
//! planning, output naming and the job runner. Nothing here launches a process;
//! engines live in `morphring-engines`.

pub mod builtin;
pub mod contract;
pub mod dates;
pub mod error;
pub mod formats;
pub mod history;
pub mod job;
pub mod options;
pub mod output;
pub mod page_range;
pub mod planner;
pub mod preset;
pub mod profiles;
pub mod ring;
pub mod settings;
pub mod slices;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
pub mod watch;

pub use contract::{
    CancelToken, ConvertOptions, Converter, Edge, EngineStatus, FileResult, FileState, FormatId,
    JobState, Merger, OutputAllocator, QualityLevel, Step,
};
pub use error::{EngineError, ErrorKey};
pub use job::Target;
pub use options::*;
