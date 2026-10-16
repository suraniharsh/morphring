//! Route tests over the real engine registry: the pins that stay true for all
//! of v0.2. Every hand-tuned edge cost lives next to the route it exists to
//! produce (ENGINEERING.md section 3), but each engine task pins its own routes
//! in its own `tests/routes_<engine>.rs`, so nobody has to queue up for this
//! file. Image routes are in `routes_images.rs`.
//!
//! The planner is built with `assume_all_available`, so these run the same on
//! a bare CI box as on a machine with every tool installed: nothing here
//! launches a program.

use morphring_core::planner::{MAX_PRIORITY_PENALTY, Planner, Route};
use morphring_core::{Converter, FormatId};
use morphring_engines::registry;

/// The registry's engines plus a planner that pretends they're all installed.
struct Graph {
    engines: Vec<Box<dyn Converter>>,
    planner: Planner,
}

impl Graph {
    fn new() -> Self {
        let engines = registry::all();
        let refs: Vec<&dyn Converter> = engines.iter().map(|e| e.as_ref()).collect();
        let planner = Planner::new(&refs, true);
        Self { engines, planner }
    }

    fn plan(&self, from: &'static str, to: &'static str) -> Option<Route> {
        self.planner.plan(FormatId::new(from), FormatId::new(to))
    }

    /// The route as `engine(from->to)` strings, so a failing assert reads like a plan.
    fn steps(&self, route: &Route) -> Vec<String> {
        route
            .hops
            .iter()
            .map(|h| format!("{}({}->{})", self.engines[h.engine].id(), h.from, h.to))
            .collect()
    }

    /// The priority penalty the planner adds to every step of `id`.
    fn penalty(&self, id: &str) -> u32 {
        let Some(index) = self.engines.iter().position(|e| e.id() == id) else {
            panic!("{id} is not in the registry");
        };
        u32::try_from(index).map_or(MAX_PRIORITY_PENALTY, |i| i.min(MAX_PRIORITY_PENALTY))
    }
}

#[test]
fn registry_order_is_the_v0_2_order() {
    let ids: Vec<&str> = registry::all().iter().map(|e| e.id()).collect();
    let expected: Vec<&str> = [
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
    .collect();
    assert_eq!(ids, expected);
}

#[test]
fn mp4_to_gif_is_ffmpeg_at_video_to_gif_cost() {
    let g = Graph::new();
    let route = g.plan("mp4", "gif").expect("mp4 -> gif has a route");
    assert_eq!(g.steps(&route), ["ffmpeg(mp4->gif)"]);
    assert_eq!(route.cost, 15 + g.penalty("ffmpeg"));
    // Still direct even though the poster-frame detour (mp4->png->gif) exists:
    // a gif of a video should move, and 10 + 12 is more than 15 anyway. The
    // detour's second hop is magick today and the native image engine once it
    // grows edges; the direct route has to beat both.
    assert!(route.cost < 10 + g.penalty("ffmpeg") + 12 + g.penalty("magick"));
    if cfg!(feature = "img") {
        assert!(route.cost < 10 + g.penalty("ffmpeg") + 12 + g.penalty("image"));
    }
}

#[test]
fn pptx_to_pdf_is_soffice_for_free() {
    let g = Graph::new();
    let route = g.plan("pptx", "pdf").expect("pptx -> pdf has a route");
    assert_eq!(g.steps(&route), ["soffice(pptx->pdf)"]);
    assert_eq!(route.hops[0].cost, g.penalty("soffice"));
    assert_eq!(route.cost, g.penalty("soffice"));
}

#[test]
fn docx_to_pdf_is_the_soffice_fallback_cost_plus_priority() {
    let g = Graph::new();
    let route = g.plan("docx", "pdf").expect("docx -> pdf has a route");
    assert_eq!(g.steps(&route), ["soffice(docx->pdf)"]);
    assert_eq!(route.cost, 25 + g.penalty("soffice"));
}

// docx -> png used to be pinned here as "no route": v0.1.0 had nothing that
// reads PDF. P2-25 pins the real docx -> pdf -> png route in routes_poppler.rs.

#[test]
fn txt_to_mp3_has_no_route() {
    // Text-to-speech is not a file conversion, however hard you squint.
    let g = Graph::new();
    assert_eq!(g.plan("txt", "mp3"), None);
}
