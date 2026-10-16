//! The list of engines MorphRing knows about, in priority order.
//!
//! Ported from Filee's `src/Filee.Engines/EngineRegistry.cs` (`CreateAll`),
//! with the native Rust engines moved to the front of the queue.

use morphring_core::{Converter, EngineStatus};

#[cfg(feature = "archive")]
use crate::archive::ArchiveEngine;
use crate::ffmpeg::FfmpegEngine;
use crate::ghostscript::GhostscriptEngine;
use crate::magick::MagickEngine;
#[cfg(feature = "img")]
use crate::native_image::ImageEngine;
use crate::pandoc::PandocEngine;
#[cfg(feature = "pdf")]
use crate::pdf::PdfEngine;
use crate::poppler::PopplerEngine;
use crate::soffice::SofficeEngine;
#[cfg(feature = "svg")]
use crate::svg::SvgEngine;

/// Every engine, in priority order (earlier wins ties in the planner).
///
/// The order is image, resvg, pdf, magick, poppler, archive, ffmpeg, pandoc,
/// ghostscript, soffice. The built-in engines (image, resvg, pdf, archive)
/// are only present when their cargo feature is on. Engines are returned
/// whether or not their program is installed; the planner checks
/// [`Converter::status`] itself.
pub fn all() -> Vec<Box<dyn Converter>> {
    // The planner charges min(index, 9) per step, so this list is a price list
    // in disguise. It is set once for all of v0.2: native engines first (no
    // process to launch, no policy.xml to argue with), poppler ahead of
    // Ghostscript for PDFs, and LibreOffice still last because it's the
    // "fine, I'll do it" engine for everything office-shaped.
    vec![
        #[cfg(feature = "img")]
        Box::new(ImageEngine::new()),
        #[cfg(feature = "svg")]
        Box::new(SvgEngine::new()),
        #[cfg(feature = "pdf")]
        Box::new(PdfEngine::new()),
        Box::new(MagickEngine::new()),
        Box::new(PopplerEngine::new()),
        #[cfg(feature = "archive")]
        Box::new(ArchiveEngine::new()),
        Box::new(FfmpegEngine::new()),
        Box::new(PandocEngine::new()),
        Box::new(GhostscriptEngine::new()),
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

    /// The v0.2 order, minus whichever built-in engines this build left out.
    fn expected() -> Vec<&'static str> {
        [
            ("image", cfg!(feature = "img")),
            ("resvg", cfg!(feature = "svg")),
            ("pdf", cfg!(feature = "pdf")),
            ("magick", true),
            ("poppler", true),
            ("archive", cfg!(feature = "archive")),
            ("ffmpeg", true),
            ("pandoc", true),
            ("ghostscript", true),
            ("soffice", true),
        ]
        .into_iter()
        .filter_map(|(id, on)| on.then_some(id))
        .collect()
    }

    #[test]
    fn engines_are_in_v0_2_priority_order() {
        let ids: Vec<&str> = all().iter().map(|e| e.id()).collect();
        assert_eq!(ids, expected());
    }

    #[test]
    fn statuses_follow_the_engine_order() {
        let ids: Vec<&str> = statuses().into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, expected());
    }

    #[cfg(all(feature = "img", feature = "svg", feature = "pdf", feature = "archive"))]
    #[test]
    fn the_default_build_has_all_ten_engines() {
        let ids: Vec<&str> = all().iter().map(|e| e.id()).collect();
        assert_eq!(
            ids,
            [
                "image",
                "resvg",
                "pdf",
                "magick",
                "poppler",
                "archive",
                "ffmpeg",
                "pandoc",
                "ghostscript",
                "soffice"
            ]
        );
    }
}
