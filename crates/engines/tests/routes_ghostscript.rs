//! Route pins for the Ghostscript edges over the real engine registry.
//! P2-26 owns this file.
//!
//! Nothing here launches a program: the "dream" planner assumes every engine
//! is installed, and the "installed" one gets a registry whose statuses are
//! faked, so the answers don't depend on what this machine happens to have.

use std::path::PathBuf;

use morphring_core::planner::{Planner, Route};
use morphring_core::{
    CancelToken, Converter, Edge, EngineError, EngineStatus, FormatId, OutputAllocator, Step,
};
use morphring_engines::registry;

/// A registry engine with its status overridden: present unless its id is in
/// the missing list.
struct Pretend {
    inner: Box<dyn Converter>,
    missing: bool,
}

impl Converter for Pretend {
    fn id(&self) -> &'static str {
        self.inner.id()
    }

    fn display_name(&self) -> &'static str {
        self.inner.display_name()
    }

    fn edges(&self) -> &[Edge] {
        self.inner.edges()
    }

    fn status(&self) -> EngineStatus {
        if self.missing {
            EngineStatus::Missing {
                hint: "pretend".to_string(),
            }
        } else {
            EngineStatus::Available {
                path: PathBuf::from("/pretend"),
                version: "1".to_string(),
            }
        }
    }

    fn convert(
        &self,
        _step: &Step,
        _out: &mut dyn OutputAllocator,
        _progress: &dyn Fn(f32),
        _cancel: &CancelToken,
    ) -> Result<Vec<PathBuf>, EngineError> {
        unreachable!("route tests never convert")
    }
}

/// The registry's engines plus a planner over them.
struct Graph {
    engines: Vec<Pretend>,
    planner: Planner,
}

impl Graph {
    /// Every engine counts as installed (the dream planner).
    fn dream() -> Self {
        Self::build(&[], true)
    }

    /// Only the engines not in `missing` count as installed.
    fn installed_without(missing: &[&str]) -> Self {
        Self::build(missing, false)
    }

    fn build(missing: &[&str], assume_all_available: bool) -> Self {
        let engines: Vec<Pretend> = registry::all()
            .into_iter()
            .map(|inner| {
                let missing = missing.contains(&inner.id());
                Pretend { inner, missing }
            })
            .collect();
        let refs: Vec<&dyn Converter> = engines.iter().map(|e| e as &dyn Converter).collect();
        let planner = Planner::new(&refs, assume_all_available);
        Self { engines, planner }
    }

    fn plan(&self, from: &'static str, to: &'static str) -> Option<Route> {
        self.planner.plan(FormatId::new(from), FormatId::new(to))
    }

    /// The route as `engine(from->to)` strings, so a failing assert reads like a plan.
    fn steps(&self, from: &'static str, to: &'static str) -> Vec<String> {
        let route = self
            .plan(from, to)
            .unwrap_or_else(|| panic!("{from} -> {to} has no route"));
        route
            .hops
            .iter()
            .map(|h| format!("{}({}->{})", self.engines[h.engine].id(), h.from, h.to))
            .collect()
    }
}

#[test]
fn eps_ps_and_ai_render_straight_through_ghostscript() {
    let g = Graph::dream();
    for from in ["eps", "ps", "ai"] {
        for to in ["png", "jpg", "tiff"] {
            assert_eq!(g.steps(from, to), [format!("ghostscript({from}->{to})")]);
        }
    }
}

#[test]
fn poppler_wins_pdf_rendering_when_both_are_installed() {
    let g = Graph::dream();
    for to in ["png", "jpg", "tiff"] {
        assert_eq!(g.steps("pdf", to), [format!("poppler(pdf->{to})")]);
    }
}

#[test]
fn with_poppler_missing_pdf_to_png_uses_ghostscript() {
    let installed = Graph::installed_without(&["poppler"]);
    for to in ["png", "jpg", "tiff"] {
        assert_eq!(
            installed.steps("pdf", to),
            [format!("ghostscript(pdf->{to})")]
        );
    }
    // The dream planner still knows Poppler would have done it.
    assert_eq!(Graph::dream().steps("pdf", "png"), ["poppler(pdf->png)"]);
}

#[test]
fn with_ghostscript_missing_eps_has_no_installed_route() {
    let installed = Graph::installed_without(&["ghostscript"]);
    assert!(installed.plan("eps", "png").is_none());
    assert!(Graph::dream().plan("eps", "png").is_some());
}

#[test]
fn eps_to_webp_renders_a_png_first() {
    let g = Graph::dream();
    let steps = g.steps("eps", "webp");
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert_eq!(steps[0], "ghostscript(eps->png)");
}

#[test]
fn eps_ps_and_ai_become_pdf_through_ghostscript_pdfwrite() {
    let g = Graph::dream();
    for from in ["eps", "ps", "ai"] {
        // Straight through pdfwrite, not a PNG wrapped in a PDF.
        assert_eq!(g.steps(from, "pdf"), [format!("ghostscript({from}->pdf)")]);
    }
}

#[test]
fn eps_ps_and_ai_to_txt_go_through_a_vector_pdf() {
    let g = Graph::dream();
    for from in ["eps", "ps", "ai"] {
        let steps = g.steps(from, "txt");
        assert_eq!(steps.len(), 2, "{steps:?}");
        assert_eq!(steps[0], format!("ghostscript({from}->pdf)"));
        assert!(steps[1].ends_with("(pdf->txt)"), "{steps:?}");
    }
}
