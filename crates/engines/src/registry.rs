//! The list of engines MorphRing knows about, in priority order.
//!
//! Ported from Filee's `src/Filee.Engines/EngineRegistry.cs` (`CreateAll`),
//! trimmed to the three external tools v0.1.0 ships with.

use morphring_core::{Converter, EngineStatus};

use crate::ffmpeg::FfmpegEngine;
use crate::magick::MagickEngine;
use crate::soffice::SofficeEngine;

/// Every engine, in priority order (earlier wins ties in the planner).
///
/// The order is ImageMagick, FFmpeg, LibreOffice, as in Filee. Engines are
/// returned whether or not their program is installed; the planner checks
/// [`Converter::status`] itself.
pub fn all() -> Vec<Box<dyn Converter>> {
    // Filee's order, minus the 20 engines we haven't written yet. Native Rust
    // engines will cut in line ahead of these in v0.2; LibreOffice stays last
    // because it's the "fine, I'll do it" engine for everything office-shaped.
    vec![
        Box::new(MagickEngine::new()),
        Box::new(FfmpegEngine::new()),
        Box::new(SofficeEngine::new()),
    ]
}

/// The id and status of every engine, for `morphring doctor`, in the same
/// order as [`all`].
pub fn statuses() -> Vec<(&'static str, EngineStatus)> {
    all()
        .iter()
        .map(|engine| (engine.id(), engine.status()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engines_are_in_filee_priority_order() {
        let ids: Vec<&str> = all().iter().map(|e| e.id()).collect();
        assert_eq!(ids, ["magick", "ffmpeg", "soffice"]);
    }

    #[test]
    fn statuses_follow_the_engine_order() {
        let ids: Vec<&str> = statuses().into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, ["magick", "ffmpeg", "soffice"]);
    }
}
