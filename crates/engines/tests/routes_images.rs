//! Image route pins over the real engine registry: the poster frame, still to
//! gif, and the re-encode self edge. P2-31 owns this file, so the native image
//! engine can move these routes off ImageMagick without touching routes.rs.
//!
//! The planner is built with `assume_all_available`, so nothing here launches
//! a program.

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
}

#[test]
fn png_to_png_is_a_single_magick_self_edge() {
    let g = Graph::new();
    let route = g.plan("png", "png").expect("png -> png re-encodes");
    assert_eq!(g.steps(&route), ["magick(png->png)"]);
    assert_eq!(route.cost, 10 + g.penalty("magick"));
}
