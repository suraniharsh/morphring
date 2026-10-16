//! Windows Job Objects, so killing a tool also kills everything it started.
//!
//! This is the stub: [`attach`] never makes a job, and the runner falls back
//! to killing just the child. XP-03 fills it in (see
//! `docs/research/windows-support.md`, section 2, option A).

use std::process::Child;

/// A Job Object holding one child process and its descendants.
// Nobody builds one until XP-03 teaches attach how, so on Windows the compiler
// would otherwise remind us of that fact on every single build.
#[allow(dead_code, reason = "constructed once XP-03 implements attach")]
pub(crate) struct Job {
    _private: (),
}

impl Job {
    /// Kills every process in the job.
    #[allow(dead_code, reason = "called once XP-03 implements attach")]
    pub(crate) fn terminate(&self) {}
}

/// Puts `child` in a new Job Object that kills its processes when closed.
/// Returns `None` when that is not possible; the stub always does.
pub(crate) fn attach(_child: &Child) -> Option<Job> {
    None
}
