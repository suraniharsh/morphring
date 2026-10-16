//! Image route pins over the real engine registry: the poster frame, still to
//! gif, the re-encode self edge, and the fall back to ImageMagick when a
//! preset asks for something the built-in engine does not do. P2-31 owns this
//! file, so the native image engine moves these routes off ImageMagick
//! without touching routes.rs.
//!
//! The planner is built with `assume_all_available`, so nothing here launches
//! a program.

use morphring_core::planner::{MAX_PRIORITY_PENALTY, Planner, Route};
use morphring_core::testing::FakeConverter;
use morphring_core::{ConvertOptions, Converter, FormatId};
use morphring_engines::registry;

/// The registry's engines plus a planner that pretends they're all installed.
struct Graph {
    engines: Vec<Box<dyn Converter>>,
    planner: Planner,
}

impl Graph {
    fn new() -> Self {
        Self::for_options(&ConvertOptions::default())
    }

    /// The graph a job with `options` plans over. Like the job runner, an
    /// engine that declines the options is swapped for a stand-in with no
    /// edges, so every other engine keeps its place (and its penalty).
    fn for_options(options: &ConvertOptions) -> Self {
        let engines: Vec<Box<dyn Converter>> = registry::all()
            .into_iter()
            .map(|e| {
                if e.accepts(options) {
                    e
                } else {
                    Box::new(FakeConverter::new(e.id())) as Box<dyn Converter>
                }
            })
            .collect();
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
fn png_to_jpg_is_the_builtin_engine() {
    let g = Graph::new();
    let route = g.plan("png", "jpg").expect("png -> jpg has a route");
    assert_eq!(g.steps(&route), ["image(png->jpg)"]);
    assert_eq!(route.cost, 8 + g.penalty("image"));
}

#[test]
fn png_to_png_is_a_single_builtin_self_edge() {
    let g = Graph::new();
    let route = g.plan("png", "png").expect("png -> png re-encodes");
    assert_eq!(g.steps(&route), ["image(png->png)"]);
    assert_eq!(route.cost, 8 + g.penalty("image"));
}

#[test]
fn png_to_gif_is_the_builtin_engine_at_still_to_gif_cost() {
    let g = Graph::new();
    let route = g.plan("png", "gif").expect("png -> gif has a route");
    assert_eq!(g.steps(&route), ["image(png->gif)"]);
    assert_eq!(route.cost, 12 + g.penalty("image"));
}

#[test]
fn mp4_to_jpg_goes_through_the_png_poster_frame() {
    // Filee's documented route: ffmpeg grabs a full-colour poster frame and an
    // image engine encodes it, rather than squeezing it through a 256-colour
    // gif. The image engine is now the built-in one.
    let g = Graph::new();
    let route = g.plan("mp4", "jpg").expect("mp4 -> jpg has a route");
    assert_eq!(g.steps(&route), ["ffmpeg(mp4->png)", "image(png->jpg)"]);
    assert_eq!(
        route.cost,
        10 + g.penalty("ffmpeg") + 8 + g.penalty("image")
    );
}

#[test]
fn formats_the_builtin_engine_lacks_stay_on_magick() {
    let g = Graph::new();
    let route = g.plan("png", "webp").expect("png -> webp has a route");
    assert_eq!(g.steps(&route), ["magick(png->webp)"]);
}

#[test]
fn a_preset_the_builtin_engine_declines_goes_to_magick() {
    let mut options = ConvertOptions::default();
    options.image.dpi = Some(300);
    let g = Graph::for_options(&options);
    let route = g.plan("png", "jpg").expect("png -> jpg has a route");
    assert_eq!(g.steps(&route), ["magick(png->jpg)"]);
    assert_eq!(route.cost, 10 + g.penalty("magick"));
}
