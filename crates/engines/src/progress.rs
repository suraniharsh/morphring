//! Estimated progress for tools that report none (LibreOffice, ImageMagick,
//! Ghostscript and friends), so the ring has something better to show than a
//! bar frozen at 10 %.
//!
//! Ported from Filee's `src/Filee.Engines/Infrastructure/ProgressEstimate.cs`.
//! This is the skeleton: [`Estimate::start`] reports nothing yet (P2-21 adds
//! the curve), but its signature is final, so engines can call it today.

use std::time::Duration;

/// Starts estimated progress reports for a tool that prints none.
pub struct Estimate;

impl Estimate {
    /// Starts reporting estimated progress to `progress`, assuming the work
    /// usually takes about `typical`. Reports stop when the returned guard is
    /// dropped; the engine reports `1.0` itself when it is done.
    // The signature is frozen: engine tasks are written against it while P2-21
    // fills in the body, so changing it would break everyone at once.
    pub fn start(progress: &(dyn Fn(f32) + Sync), typical: Duration) -> EstimateGuard {
        // Nothing to estimate yet, but keep the arguments honest so the guard
        // looks exactly like the real one from the outside.
        let _ = (progress, typical);
        EstimateGuard { _private: () }
    }
}

/// Keeps estimated progress running; dropping it stops the reports.
#[must_use = "progress estimates stop as soon as the guard is dropped"]
pub struct EstimateGuard {
    _private: (),
}

impl Drop for EstimateGuard {
    fn drop(&mut self) {
        // The real guard stops and joins its ticker thread here. This one has
        // no thread, so it just enjoys the moment.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn the_stub_reports_nothing_and_stops_on_drop() {
        let seen = Mutex::new(Vec::new());
        let report = |value: f32| seen.lock().unwrap().push(value);
        let guard = Estimate::start(&report, Duration::from_millis(10));
        drop(guard);
        assert!(seen.lock().unwrap().is_empty());
    }
}
