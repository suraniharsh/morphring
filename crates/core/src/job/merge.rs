//! Merge mode: many inputs, one output (Merge PDF). Ported from Filee's
//! `src/Filee.Core/Conversion/JobQueue.cs`, the merge branch of `RunAsync`.
//!
//! For now it politely declines every job, so [`run_job`](super::run_job)
//! carries on file by file. P2-17 teaches it to actually merge, using the
//! helpers `job.rs` keeps `pub(super)` for it: `plan_file`, `run_route`,
//! `final_allocator`, `TempAllocator` and `remove_path`.

use std::path::PathBuf;

use super::Job;
use crate::contract::FileResult;

/// Runs the whole job as one merge when its options ask for it and an engine
/// can do it. `None` means "not a merge job"; the caller converts file by file.
pub(super) fn try_run(_job: &Job<'_>, _files: &[PathBuf]) -> Option<Vec<FileResult>> {
    None
}
