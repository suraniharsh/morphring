//! `--keep-dates`: give each output the creation and modification times of its
//! source. Ported from Filee's `src/Filee.Core/Conversion/JobQueue.cs` (the
//! `OutputRule.KeepDates` step).
//!
//! P2-16 fills it. Until then the signature is here so `job.rs` has something
//! to call, and the call politely does nothing.

use std::path::{Path, PathBuf};

/// Copies the creation and modification times of `source` onto every path in
/// `outputs`. Best effort: a time that can't be copied is skipped.
pub fn copy_dates(_source: &Path, _outputs: &[PathBuf]) {}
