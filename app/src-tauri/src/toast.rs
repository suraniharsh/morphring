//! The toast window for jobs that outlive the ring.
//!
//! Ported (and slimmed down) from Filee's `Views/ToastWindow.axaml.cs` and
//! `ViewModels/JobViewModel.cs`. The cards live in `ui/toast.js`, which is
//! driven only by the `job-*` events documented in `commands.rs`; this module
//! owns the timing rules and the window itself:
//!
//! - [`show_after`]: a job earns a card once it still runs 1.5 s after the
//!   ring hid (or 1.5 s after it started, if the ring was hidden already).
//! - [`dismiss_after`]: finished cards leave after 5 s, or 15 s when something
//!   went wrong, so there is time to read the bad news.
//! - [`place`]: bottom-right of the primary monitor's work area, 12 px in.
//!
//! The webview asks [`toast_check`] whether a job's card may show yet,
//! [`toast_dismiss_delay`] how long a finished card stays, and hands its
//! height to [`toast_layout`], which sizes, places, shows or hides the window.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, PhysicalRect, PhysicalSize,
    Runtime, WebviewWindow,
};

use crate::commands::{Outcome, RING_WINDOW};

/// Label of the toast window.
pub const TOAST_WINDOW: &str = "toast";

/// Event sent to the toast window with a job id when that job's card should go.
pub const TOAST_DISMISS: &str = "toast-dismiss";

/// Width of the toast window, in logical pixels.
pub const TOAST_WIDTH: f64 = 360.0;

/// Gap between the toast and the edges of the work area, in logical pixels.
pub const TOAST_MARGIN: f64 = 12.0;

/// How long a job must keep running, with the ring hidden, before it gets a card.
pub const SHOW_DELAY: Duration = Duration::from_millis(1500);

/// How long a finished card stays when the job went fine (or was cancelled).
pub const DISMISS_OK: Duration = Duration::from_secs(5);

/// How long a finished card stays when at least one file failed.
pub const DISMISS_ERROR: Duration = Duration::from_secs(15);

/// How long until a running job's card may show.
///
/// `running_for` is how long the job has run; `ring_hidden_for` is how long
/// the ring has been hidden, or `None` while it is on screen. Returns `None`
/// while the ring shows (it already tells the story, ask again later),
/// `Some(Duration::ZERO)` when the card should show now, and otherwise the
/// time left. The clock starts at whichever came last: the job's start or the
/// ring hiding.
pub fn show_after(running_for: Duration, ring_hidden_for: Option<Duration>) -> Option<Duration> {
    let hidden_for = ring_hidden_for?;
    Some(SHOW_DELAY.saturating_sub(running_for.min(hidden_for)))
}

/// How long the card of a job that ended with `outcome` stays on screen.
pub fn dismiss_after(outcome: Outcome) -> Duration {
    match outcome {
        Outcome::Failed => DISMISS_ERROR,
        // A cancel is the user's own doing; no need to dwell on it.
        Outcome::Succeeded | Outcome::Cancelled => DISMISS_OK,
    }
}

/// Where the toast's top-left corner goes: bottom-right of `work_area`,
/// `margin` physical pixels in from both edges. Never left of or above the
/// work area, even for a toast bigger than the screen.
pub fn place(
    work_area: PhysicalRect<i32, u32>,
    size: PhysicalSize<u32>,
    margin: u32,
) -> PhysicalPosition<i32> {
    let room = |extent: u32, wanted: u32| -> i32 {
        let offset = i64::from(extent) - i64::from(wanted) - i64::from(margin);
        i32::try_from(offset.max(0)).unwrap_or(i32::MAX)
    };
    PhysicalPosition::new(
        work_area
            .position
            .x
            .saturating_add(room(work_area.size.width, size.width)),
        work_area
            .position
            .y
            .saturating_add(room(work_area.size.height, size.height)),
    )
}

/// Remembers since when the ring has been hidden, from looks at its visibility.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RingWatch {
    hidden_since: Option<Instant>,
}

impl RingWatch {
    /// Records that the ring was (or was not) visible at `now`, and returns how
    /// long it has been hidden, `None` while it shows.
    pub fn observe(&mut self, visible: bool, now: Instant) -> Option<Duration> {
        if visible {
            self.hidden_since = None;
            return None;
        }
        let since = *self.hidden_since.get_or_insert(now);
        Some(now.saturating_duration_since(since))
    }
}

/// The app-wide ring watch. A static rather than managed state: only this
/// module reads it, and it keeps `lib.rs` out of the toast's business.
static RING_WATCH: Mutex<RingWatch> = Mutex::new(RingWatch { hidden_since: None });

fn ring_visible<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.get_webview_window(RING_WINDOW)
        .and_then(|ring| ring.is_visible().ok())
        .unwrap_or(false)
}

/// How many milliseconds until the card of a job that has run for `running_ms`
/// may show: `0` means now, `null` means the ring is on screen (ask again later).
#[tauri::command]
pub fn toast_check<R: Runtime>(app: AppHandle<R>, running_ms: u64) -> Option<u64> {
    let visible = ring_visible(&app);
    // A poisoned watch still holds a perfectly good timestamp; carry on.
    let hidden_for = RING_WATCH
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .observe(visible, Instant::now());
    show_after(Duration::from_millis(running_ms), hidden_for)
        .map(|left| u64::try_from(left.as_millis()).unwrap_or(u64::MAX))
}

/// Reads an outcome the way `job-finished` spells it (`succeeded`, `failed`,
/// `cancelled`). Anything else counts as a failure: better to linger over a
/// mystery than to whisk it away.
pub fn parse_outcome(outcome: &str) -> Outcome {
    match outcome {
        "succeeded" => Outcome::Succeeded,
        "cancelled" => Outcome::Cancelled,
        _ => Outcome::Failed,
    }
}

/// How many milliseconds a finished card stays for a job that ended with
/// `outcome`, as spelled in the `job-finished` summary.
#[tauri::command]
pub fn toast_dismiss_delay(outcome: String) -> u64 {
    u64::try_from(dismiss_after(parse_outcome(&outcome)).as_millis()).unwrap_or(u64::MAX)
}

/// Sizes the toast window to `height` logical pixels, places it and shows it;
/// a `height` of 0 (no cards left) hides it.
#[tauri::command]
pub fn toast_layout<R: Runtime>(app: AppHandle<R>, height: f64) -> Result<(), String> {
    let Some(toast) = app.get_webview_window(TOAST_WINDOW) else {
        return Ok(());
    };
    if height <= 0.0 || !height.is_finite() {
        return toast
            .hide()
            .map_err(|e| format!("Could not hide the toast: {e}"));
    }
    toast
        .set_size(LogicalSize::new(TOAST_WIDTH, height.ceil()))
        .map_err(|e| format!("Could not resize the toast: {e}"))?;
    move_to_corner(&app, &toast);
    if !toast.is_visible().unwrap_or(false) {
        toast
            .show()
            .map_err(|e| format!("Could not show the toast: {e}"))?;
    }
    Ok(())
}

/// Puts the toast in the bottom-right corner of the primary monitor.
fn move_to_corner<R: Runtime>(app: &AppHandle<R>, toast: &WebviewWindow<R>) {
    // On Wayland nobody gets to pick their own position; the compositor puts
    // the toast wherever it likes and we smile and nod. Hence debug, not warn.
    let monitor = match app.primary_monitor() {
        Ok(Some(monitor)) => monitor,
        Ok(None) => return,
        Err(e) => {
            tracing::debug!("no primary monitor for the toast: {e}");
            return;
        }
    };
    let size = match toast.outer_size() {
        Ok(size) => size,
        Err(e) => {
            tracing::debug!("could not read the toast size: {e}");
            return;
        }
    };
    let margin = (TOAST_MARGIN * monitor.scale_factor()).round() as u32;
    let at = place(*monitor.work_area(), size, margin);
    if let Err(e) = toast.set_position(at) {
        tracing::debug!("could not place the toast: {e}");
    }
}

/// Removes the card of job `job_id` from the toast.
#[tauri::command]
pub fn toast_dismiss<R: Runtime>(app: AppHandle<R>, job_id: u64) -> Result<(), String> {
    app.emit_to(TOAST_WINDOW, TOAST_DISMISS, job_id)
        .map_err(|e| format!("Could not dismiss the job: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn no_toast_while_the_ring_shows() {
        assert_eq!(show_after(ms(0), None), None);
        assert_eq!(show_after(ms(60_000), None), None);
    }

    #[test]
    fn a_job_started_with_the_ring_hidden_waits_from_its_start() {
        // The ring hid ages ago; the job is fresh.
        assert_eq!(show_after(ms(0), Some(ms(90_000))), Some(ms(1500)));
        assert_eq!(show_after(ms(1000), Some(ms(90_000))), Some(ms(500)));
        assert_eq!(show_after(ms(1500), Some(ms(90_000))), Some(Duration::ZERO));
    }

    #[test]
    fn a_job_that_outlives_the_ring_waits_from_the_hide() {
        // Ten seconds in, the user closes the ring: another 1.5 s of patience.
        assert_eq!(show_after(ms(10_000), Some(ms(0))), Some(ms(1500)));
        assert_eq!(show_after(ms(11_000), Some(ms(1000))), Some(ms(500)));
        assert_eq!(show_after(ms(12_000), Some(ms(2000))), Some(Duration::ZERO));
    }

    #[test]
    fn errors_linger_longer_than_good_news() {
        assert_eq!(dismiss_after(Outcome::Succeeded), ms(5000));
        assert_eq!(dismiss_after(Outcome::Cancelled), ms(5000));
        assert_eq!(dismiss_after(Outcome::Failed), ms(15_000));
    }

    #[test]
    fn outcomes_are_read_as_the_events_spell_them() {
        // The spelling comes from `Outcome`'s serde rename; keep the two in step.
        for outcome in [Outcome::Succeeded, Outcome::Failed, Outcome::Cancelled] {
            let spelled = serde_json::to_value(outcome).unwrap();
            assert_eq!(parse_outcome(spelled.as_str().unwrap()), outcome);
        }
        assert_eq!(parse_outcome("exploded"), Outcome::Failed);
        assert_eq!(toast_dismiss_delay("failed".into()), 15_000);
        assert_eq!(toast_dismiss_delay("succeeded".into()), 5000);
    }

    #[test]
    fn ring_watch_counts_from_the_first_hidden_look() {
        let t0 = Instant::now();
        let mut watch = RingWatch::default();
        assert_eq!(watch.observe(true, t0), None);
        assert_eq!(watch.observe(false, t0 + ms(100)), Some(Duration::ZERO));
        assert_eq!(watch.observe(false, t0 + ms(600)), Some(ms(500)));
        // The ring comes back: the clock resets.
        assert_eq!(watch.observe(true, t0 + ms(700)), None);
        assert_eq!(watch.observe(false, t0 + ms(900)), Some(Duration::ZERO));
    }

    fn rect(x: i32, y: i32, w: u32, h: u32) -> PhysicalRect<i32, u32> {
        PhysicalRect {
            position: PhysicalPosition::new(x, y),
            size: PhysicalSize::new(w, h),
        }
    }

    #[test]
    fn the_toast_sits_in_the_bottom_right_corner() {
        // 1920x1080 with a 40 px panel at the bottom.
        let at = place(rect(0, 0, 1920, 1040), PhysicalSize::new(360, 120), 12);
        assert_eq!(at, PhysicalPosition::new(1920 - 360 - 12, 1040 - 120 - 12));
    }

    #[test]
    fn the_corner_follows_an_offset_work_area() {
        // A panel on the left and top, on a monitor right of another one.
        let at = place(rect(2000, 30, 1000, 800), PhysicalSize::new(720, 240), 24);
        assert_eq!(
            at,
            PhysicalPosition::new(2000 + 1000 - 720 - 24, 30 + 800 - 240 - 24)
        );
    }

    #[test]
    fn an_oversized_toast_stays_inside_the_work_area() {
        let at = place(rect(100, 50, 300, 200), PhysicalSize::new(360, 400), 12);
        assert_eq!(at, PhysicalPosition::new(100, 50));
    }
}
