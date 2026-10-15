//! Route tests over the real engine registry. Every hand-tuned edge cost is
//! here with the route it exists to produce (ENGINEERING.md section 3).
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
fn registry_order_is_magick_ffmpeg_soffice() {
    let ids: Vec<&str> = registry::all().iter().map(|e| e.id()).collect();
    assert_eq!(ids, ["magick", "ffmpeg", "soffice"]);
}

#[test]
fn mp4_to_jpg_goes_through_the_png_poster_frame() {
    // Filee's documented route: ffmpeg grabs a full-colour poster frame and
    // ImageMagick encodes it, rather than squeezing it through a 256-colour gif.
    let g = Graph::new();
    let route = g.plan("mp4", "jpg").expect("mp4 -> jpg has a route");
    assert_eq!(g.steps(&route), ["ffmpeg(mp4->png)", "magick(png->jpg)"]);
    assert_eq!(
        route.cost,
        10 + g.penalty("ffmpeg") + 10 + g.penalty("magick")
    );
}

#[test]
fn png_to_gif_is_magick_at_still_to_gif_cost() {
    let g = Graph::new();
    let route = g.plan("png", "gif").expect("png -> gif has a route");
    assert_eq!(g.steps(&route), ["magick(png->gif)"]);
    assert_eq!(route.cost, 12 + g.penalty("magick"));
    assert_eq!(route.cost, 12);
}

#[test]
fn mp4_to_gif_is_ffmpeg_at_video_to_gif_cost() {
    let g = Graph::new();
    let route = g.plan("mp4", "gif").expect("mp4 -> gif has a route");
    assert_eq!(g.steps(&route), ["ffmpeg(mp4->gif)"]);
    assert_eq!(route.cost, 15 + g.penalty("ffmpeg"));
    // Still direct even though the poster-frame detour (mp4->png->gif) exists:
    // a gif of a video should move, and 10 + 12 is more than 15 anyway.
    assert!(route.cost < 10 + g.penalty("ffmpeg") + 12 + g.penalty("magick"));
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

#[test]
fn png_to_png_is_a_single_magick_self_edge() {
    let g = Graph::new();
    let route = g.plan("png", "png").expect("png -> png re-encodes");
    assert_eq!(g.steps(&route), ["magick(png->png)"]);
    assert_eq!(route.cost, 10 + g.penalty("magick"));
}

#[test]
fn docx_to_png_has_no_route_in_v0_1_0() {
    // The natural route is docx -> pdf -> png, but nothing in v0.1.0 reads PDF:
    // ImageMagick would need Ghostscript and a looser policy, and the real PDF
    // rasteriser arrives in v0.2 (PLAN.md D4). Until then this is "no route".
    let g = Graph::new();
    assert_eq!(g.plan("docx", "png"), None);
}

#[test]
fn txt_to_mp3_has_no_route() {
    // Text-to-speech is not a file conversion, however hard you squint.
    let g = Graph::new();
    assert_eq!(g.plan("txt", "mp3"), None);
}
