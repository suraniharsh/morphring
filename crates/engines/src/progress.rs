//! Estimated progress for tools that report none (LibreOffice, ImageMagick,
//! Ghostscript and friends), so the ring has something better to show than a
//! bar frozen at 10 %.
//!
//! Ported from Filee's `src/Filee.Engines/Infrastructure/ProgressEstimate.cs`.
//! The estimate starts at [`FROM`], rises quickly at first and ever slower
//! towards [`TO`], which it never reaches: the engine reports `1.0` itself when
//! the work is really done.
//!
//! Use [`Estimate::during`] around the silent work. It runs the work on a
//! scoped thread and ticks every [`INTERVAL`] on the caller's thread, which is
//! the only place the engine's (non-`Sync`, borrowed) progress callback may be
//! called from.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

/// How often the estimate is reported.
pub const INTERVAL: Duration = Duration::from_millis(150);

/// Where the estimate starts.
pub const FROM: f32 = 0.1;

/// The bound the estimate approaches but never reaches.
pub const TO: f32 = 0.9;

// Filee clamps the typical duration to 100 ms so a zero can't divide the
// curve into a NaN. Same floor here, same reason.
const MIN_TYPICAL: Duration = Duration::from_millis(100);

/// The estimate after `elapsed`, for work that usually takes `typical`:
/// `from + (to - from) * (1 - exp(-elapsed / typical))`.
///
/// At `elapsed == typical` it has covered about 63 % of the way from `from` to
/// `to`; it never passes `to`.
pub fn estimate_at(elapsed: Duration, typical: Duration, from: f32, to: f32) -> f32 {
    let typical = typical.max(MIN_TYPICAL).as_secs_f64();
    let covered = 1.0 - (-elapsed.as_secs_f64() / typical).exp();
    let value = f64::from(from) + f64::from(to - from) * covered;
    // f64 -> f32 rounding could nudge a value a hair past `to` after a very
    // long run; the bound is a promise, so keep it.
    (value as f32).clamp(from.min(to), from.max(to))
}

/// Starts estimated progress reports for a tool that prints none.
pub struct Estimate;

impl Estimate {
    /// Runs `work` while reporting an estimate to `progress` every
    /// [`INTERVAL`], assuming the work usually takes about `typical`. The
    /// reports rise from [`FROM`] towards [`TO`] and never go down. They stop
    /// (and the work thread is joined) before this returns; the engine reports
    /// `1.0` itself afterwards.
    ///
    /// A panic in `work` is passed on to the caller.
    pub fn during<T, W>(progress: &dyn Fn(f32), typical: Duration, work: W) -> T
    where
        T: Send,
        W: FnOnce() -> T + Send,
    {
        let started = Instant::now();
        let mut last = f32::NEG_INFINITY;
        thread::scope(|scope| {
            let (done_tx, done) = mpsc::channel::<()>();
            let worker = scope.spawn(move || {
                let result = work();
                // The receiver only hangs up once we're joined, so this can't
                // fail in any way worth crying about.
                let _ = done_tx.send(());
                result
            });
            // recv_timeout doubles as the ticker's sleep, and wakes up the
            // moment the work finishes instead of dozing on for up to 150 ms.
            // A panicking worker drops the sender, which also ends the loop.
            while let Err(RecvTimeoutError::Timeout) = done.recv_timeout(INTERVAL) {
                let value = estimate_at(started.elapsed(), typical, FROM, TO);
                // The curve only rises, but clocks and float rounding have
                // opinions; the ring is promised values that never go down.
                if value > last {
                    last = value;
                    progress(value);
                }
            }
            match worker.join() {
                Ok(result) => result,
                Err(panic) => std::panic::resume_unwind(panic),
            }
        })
    }

    /// Reports the starting estimate ([`FROM`]) to `progress` and returns a
    /// guard for the work that follows.
    ///
    /// For reports that keep rising while the work runs, use
    /// [`Estimate::during`]: safe Rust can't hand a borrowed callback to a
    /// thread that outlives this call, so the guard has no ticker of its own.
    // The signature is frozen (P2-03a): engine tasks are written against it,
    // so changing it would break everyone at once. It stays honest instead of
    // clever: a free-running thread holding `progress` would need `unsafe`, and
    // `mem::forget(guard)` would turn that into a use-after-free. No thanks.
    pub fn start(progress: &(dyn Fn(f32) + Sync), typical: Duration) -> EstimateGuard {
        let _ = typical;
        progress(FROM);
        EstimateGuard { _private: () }
    }
}

/// Returned by [`Estimate::start`]; dropping it ends the estimate.
#[must_use = "progress estimates stop as soon as the guard is dropped"]
pub struct EstimateGuard {
    _private: (),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const SECOND: Duration = Duration::from_secs(1);

    #[test]
    fn progress_curve_starts_at_from() {
        assert_eq!(estimate_at(Duration::ZERO, SECOND, FROM, TO), FROM);
    }

    #[test]
    fn progress_curve_covers_63_percent_at_typical() {
        let typical = Duration::from_secs(4);
        let value = estimate_at(typical, typical, FROM, TO);
        let covered = (value - FROM) / (TO - FROM);
        assert!((covered - 0.632).abs() < 0.001, "covered {covered}");
    }

    #[test]
    fn progress_curve_is_monotonic_and_bounded() {
        let typical = Duration::from_secs(2);
        let mut last = FROM;
        for ms in (0..600_000).step_by(37) {
            let value = estimate_at(Duration::from_millis(ms), typical, FROM, TO);
            assert!(value >= last, "dropped from {last} to {value} at {ms} ms");
            assert!((FROM..=TO).contains(&value), "{value} out of bounds");
            last = value;
        }
        // An absurdly long run still stays under the bound.
        let forever = estimate_at(Duration::from_secs(1 << 40), typical, FROM, TO);
        assert!(forever <= TO);
    }

    #[test]
    fn progress_curve_survives_a_zero_typical() {
        let value = estimate_at(SECOND, Duration::ZERO, FROM, TO);
        assert!(value.is_finite());
        assert!((FROM..=TO).contains(&value));
    }

    #[test]
    fn progress_during_ticks_upwards_and_returns_the_result() {
        let seen = Mutex::new(Vec::new());
        let report = |value: f32| seen.lock().unwrap().push(value);
        let answer = Estimate::during(&report, Duration::from_millis(200), || {
            thread::sleep(Duration::from_millis(700));
            42
        });
        assert_eq!(answer, 42);
        let seen = seen.into_inner().unwrap();
        // ~4 ticks in 700 ms; a loaded machine may manage fewer, never zero.
        assert!(!seen.is_empty(), "no ticks at all");
        assert!(seen.windows(2).all(|w| w[0] < w[1]), "not rising: {seen:?}");
        assert!(seen.iter().all(|v| (FROM..TO).contains(v)), "{seen:?}");
    }

    #[test]
    fn progress_during_stops_when_the_work_ends() {
        let seen = Mutex::new(Vec::new());
        let report = |value: f32| seen.lock().unwrap().push(value);
        let started = Instant::now();
        Estimate::during(&report, SECOND, || ());
        // Quick work returns right away: no waiting out a tick, no reports.
        assert!(started.elapsed() < INTERVAL);
        assert!(seen.lock().unwrap().is_empty());
        thread::sleep(INTERVAL * 2);
        assert!(seen.lock().unwrap().is_empty(), "ticked after returning");
    }

    #[test]
    fn progress_during_passes_panics_on() {
        let report = |_: f32| {};
        let caught = std::panic::catch_unwind(|| {
            Estimate::during(&report, SECOND, || -> () { panic!("tool exploded") })
        });
        assert!(caught.is_err());
    }

    #[test]
    fn progress_start_reports_the_starting_value() {
        let seen = Mutex::new(Vec::new());
        let report = |value: f32| seen.lock().unwrap().push(value);
        let guard = Estimate::start(&report, Duration::from_millis(10));
        drop(guard);
        assert_eq!(*seen.lock().unwrap(), [FROM]);
    }
}
