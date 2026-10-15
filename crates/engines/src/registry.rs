//! The list of engines MorphRing knows about, in priority order.

use morphring_core::{Converter, EngineStatus};

/// Every engine, in priority order (earlier wins ties in the planner).
pub fn all() -> Vec<Box<dyn Converter>> {
    // TODO(T30): magick, ffmpeg, soffice, in Filee's order. Until then the
    // planner gets an empty list and everything is "no route", which is at
    // least honest.
    Vec::new()
}

/// The id and status of every engine, for `morphring doctor`.
pub fn statuses() -> Vec<(&'static str, EngineStatus)> {
    // TODO(T30): built from all().
    Vec::new()
}
