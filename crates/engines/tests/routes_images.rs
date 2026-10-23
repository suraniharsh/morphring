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
use morphring_core::{ConvertOptions, Converter, FormatId, Resize};
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
        Self::build(options, &[])
    }

    /// The default graph on a machine without the engines in `missing`: each
    /// one keeps its place in the list but brings no edges.
    fn without(missing: &[&str]) -> Self {
        Self::build(&ConvertOptions::default(), missing)
    }

    fn build(options: &ConvertOptions, missing: &[&str]) -> Self {
        let engines: Vec<Box<dyn Converter>> = registry::all()
            .into_iter()
            .map(|e| {
                if e.accepts(options) && !missing.contains(&e.id()) {
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

// FX-03: the built-in engine decodes one picture per file, so a multi-page
// TIFF or an animated GIF sent its way lost every page after the first, and
// said "Converted 1 of 1" with a straight face. With magick installed, those
// sources must go to magick, whose frame policy keeps every page.

#[test]
fn multi_page_tiff_to_png_goes_to_magick() {
    let g = Graph::new();
    let route = g.plan("tiff", "png").expect("tiff -> png has a route");
    assert_eq!(g.steps(&route), ["magick(tiff->png)"]);
}

#[test]
fn animated_gif_to_png_and_tiff_go_to_magick() {
    let g = Graph::new();
    for to in ["png", "jpg", "bmp", "tga", "ppm", "tiff"] {
        let route = g.plan("gif", to).expect("gif has a route");
        assert_eq!(
            g.steps(&route),
            [format!("magick(gif->{to})")],
            "gif -> {to}"
        );
    }
    // GIF to GIF is the one road the built-in engine carries every frame on.
    let route = g.plan("gif", "gif").expect("gif -> gif has a route");
    assert_eq!(g.steps(&route), ["image(gif->gif)"]);
}

#[test]
fn every_tiff_target_magick_writes_goes_to_magick() {
    let g = Graph::new();
    for to in ["jpg", "bmp", "gif", "tga", "ppm", "ico", "tiff"] {
        let route = g.plan("tiff", to).expect("tiff has a route");
        assert_eq!(
            g.steps(&route),
            [format!("magick(tiff->{to})")],
            "tiff -> {to}"
        );
    }
}

#[test]
fn a_half_size_tiff_to_tiff_goes_to_magick() {
    let mut options = ConvertOptions::default();
    options.image.resize = Resize::Percent;
    options.image.resize_percent = 50;
    let g = Graph::for_options(&options);
    let route = g.plan("tiff", "tiff").expect("tiff -> tiff has a route");
    assert_eq!(g.steps(&route), ["magick(tiff->tiff)"]);
}

#[test]
fn tiff_and_gif_to_qoi_split_the_pages_in_magick_first() {
    // magick writes no QOI, so it splits the pages into PNGs and the
    // built-in engine encodes each one.
    let g = Graph::new();
    for from in ["tiff", "gif"] {
        let route = g.plan(from, "qoi").expect("-> qoi has a route");
        assert_eq!(
            g.steps(&route),
            [format!("magick({from}->png)"), "image(png->qoi)".to_owned()],
            "{from} -> qoi"
        );
    }
}

#[test]
fn still_formats_stay_on_the_builtin_engine() {
    let g = Graph::new();
    for (from, to) in [("png", "jpg"), ("jpg", "tiff"), ("bmp", "png")] {
        let route = g.plan(from, to).expect("still has a route");
        assert_eq!(g.steps(&route), [format!("image({from}->{to})")]);
    }
}

#[test]
fn without_magick_tiff_and_gif_still_go_straight_to_the_builtin_engine() {
    // No detour through a PDF and back just to avoid the pricier edge: the
    // built-in engine takes the job and refuses it plainly if it has pages.
    let g = Graph::without(&["magick"]);
    for (from, to) in [
        ("tiff", "png"),
        ("tiff", "jpg"),
        ("gif", "png"),
        ("tiff", "tiff"),
    ] {
        let route = g.plan(from, to).expect("still has a route");
        assert_eq!(
            g.steps(&route),
            [format!("image({from}->{to})")],
            "{from} -> {to}"
        );
        assert_eq!(route.cost, 22 + g.penalty("image"));
    }
}
