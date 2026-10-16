//! Route pins for the Poppler render edges over the real engine registry.
//! P2-25 owns this file.
//!
//! The planner is built with `assume_all_available`, so nothing here launches
//! a program.

use morphring_core::planner::{Planner, Route};
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

    fn plan(&self, from: &'static str, to: &'static str) -> Route {
        self.planner
            .plan(FormatId::new(from), FormatId::new(to))
            .unwrap_or_else(|| panic!("{from} -> {to} has no route"))
    }

    /// The route as `engine(from->to)` strings, so a failing assert reads like a plan.
    fn steps(&self, route: &Route) -> Vec<String> {
        route
            .hops
            .iter()
            .map(|h| format!("{}({}->{})", self.engines[h.engine].id(), h.from, h.to))
            .collect()
    }
}

#[test]
fn pdf_to_png_jpg_and_tiff_are_poppler() {
    let g = Graph::new();
    for to in ["png", "jpg", "tiff"] {
        let route = g.plan("pdf", to);
        assert_eq!(g.steps(&route), [format!("poppler(pdf->{to})")]);
    }
}

#[test]
fn pdf_to_webp_renders_png_then_hands_it_to_an_image_engine() {
    // pdftoppm can't write webp, so the planner renders a png and lets the
    // image engines finish the job; which one depends on the build features.
    let g = Graph::new();
    let steps = g.steps(&g.plan("pdf", "webp"));
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert_eq!(steps[0], "poppler(pdf->png)");
    assert!(
        steps[1] == "image(png->webp)" || steps[1] == "magick(png->webp)",
        "{steps:?}"
    );
}

#[test]
fn docx_to_png_goes_through_a_soffice_pdf() {
    let g = Graph::new();
    let route = g.plan("docx", "png");
    assert_eq!(g.steps(&route), ["soffice(docx->pdf)", "poppler(pdf->png)"]);
}
