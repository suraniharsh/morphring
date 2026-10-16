//! Document route pins for the Pandoc engine over the real registry. P2-36
//! owns this file, so pandoc's edges can be pinned without touching routes.rs.
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
fn md_to_docx_is_pandoc() {
    let g = Graph::new();
    let route = g.plan("md", "docx").expect("md -> docx has a route");
    assert_eq!(g.steps(&route), ["pandoc(md->docx)"]);
    assert_eq!(route.cost, 10 + g.penalty("pandoc"));
}

#[test]
fn docx_to_md_is_pandoc() {
    let g = Graph::new();
    let route = g.plan("docx", "md").expect("docx -> md has a route");
    assert_eq!(g.steps(&route), ["pandoc(docx->md)"]);
}

#[test]
fn docx_to_pdf_is_still_soffice() {
    // Pandoc has no PDF writer (that would need TeX), so LibreOffice keeps it.
    let g = Graph::new();
    let route = g.plan("docx", "pdf").expect("docx -> pdf has a route");
    assert_eq!(g.steps(&route), ["soffice(docx->pdf)"]);
}

#[test]
fn html_to_docx_prefers_pandoc_over_the_fallback() {
    let g = Graph::new();
    let route = g.plan("html", "docx").expect("html -> docx has a route");
    assert_eq!(g.steps(&route), ["pandoc(html->docx)"]);
    assert_eq!(route.cost, 14 + g.penalty("pandoc"));
}

#[test]
fn tex_to_epub_is_pandoc() {
    let g = Graph::new();
    let route = g.plan("tex", "epub").expect("tex -> epub has a route");
    assert_eq!(g.steps(&route), ["pandoc(tex->epub)"]);
}
