//! Route planning: the cheapest chain of at most three engine steps from one
//! format to another.
//!
//! Filee runs a hop-capped Dijkstra (`src/Filee.Core/Conversion/RoutePlanner.cs`),
//! which is not exact: a cheap but long path can claim an intermediate format
//! and then hit the hop cap, hiding a valid route behind it. The graph here is
//! tiny, so we simply walk every simple path of up to three hops and keep the
//! cheapest. Exact, deterministic, and short enough to read over coffee.
//!
//! Two of Filee's refinements ride along: edge flags that keep image tracing
//! away from anything that was not a raster image to begin with
//! (RoutePlanner.cs:62-81), and [`EngineTuning`], the user's engine priority
//! and per-edge cost overrides (ConverterCatalog.cs:53-69).

use std::collections::HashMap;

use crate::contract::{Converter, EdgeFlags, FormatId};
use crate::formats::{self, Category};

/// Longest route the planner returns. Longer chains are slow and lossy.
pub const MAX_HOPS: usize = 3;

/// Cap on the per-step priority penalty, so priority only breaks ties between
/// engines and never makes a longer route win.
pub const MAX_PRIORITY_PENALTY: u32 = 9;

/// One step of a route.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Hop {
    /// Position of the engine in the slice the planner was built from.
    pub engine: usize,
    pub from: FormatId,
    pub to: FormatId,
    /// Edge cost plus the engine's priority penalty.
    pub cost: u32,
}

/// A planned conversion: one to `MAX_HOPS` hops, in order.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Route {
    pub hops: Vec<Hop>,
    /// Sum of the hop costs.
    pub cost: u32,
}

impl Route {
    /// Engine indices of the hops, in route order (an engine may repeat).
    pub fn engines(&self) -> impl Iterator<Item = usize> + '_ {
        self.hops.iter().map(|hop| hop.engine)
    }
}

/// How the user bent the planner's defaults: which engine wins a tie, and
/// edges whose cost they want different.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EngineTuning {
    /// Engine ids, first wins. Empty means registry order. Engines missing
    /// from a non-empty list rank after every listed one, like Filee's.
    pub priority: Vec<String>,
    /// `(engine id, from, to, cost)`: that engine's `from -> to` edge costs
    /// `cost` instead of what the engine says. The priority penalty still applies.
    pub cost_overrides: Vec<(String, String, String, u32)>,
}

impl EngineTuning {
    /// The priority penalty of the engine with id `id`, registered at
    /// position `index`, capped at [`MAX_PRIORITY_PENALTY`].
    pub fn penalty(&self, id: &str, index: usize) -> u32 {
        let rank = if self.priority.is_empty() {
            index
        } else {
            self.priority
                .iter()
                .position(|p| p.eq_ignore_ascii_case(id))
                .unwrap_or(self.priority.len())
        };
        u32::try_from(rank)
            .unwrap_or(u32::MAX)
            .min(MAX_PRIORITY_PENALTY)
    }

    /// The cost the user wants for engine `id`'s `from -> to` edge, if any.
    /// The last matching override wins.
    pub fn cost_override(&self, id: &str, from: FormatId, to: FormatId) -> Option<u32> {
        self.cost_overrides
            .iter()
            .rev()
            .find(|(engine, f, t, _)| {
                engine.eq_ignore_ascii_case(id)
                    && f.eq_ignore_ascii_case(from.as_str())
                    && t.eq_ignore_ascii_case(to.as_str())
            })
            .map(|(.., cost)| *cost)
    }
}

/// Plans routes over a fixed set of engines. Build one per job.
#[derive(Clone, Debug, Default)]
pub struct Planner {
    /// Every usable edge, in registration order (engine order, then edge order).
    edges: Vec<Hop>,
    /// For each source format, the cheapest edge to each target format, ordered
    /// by the target's first appearance among that source's edges. Index 0 is
    /// for routes from a non-image source, index 1 for routes from a raster
    /// image: each only holds the edges its flags allow.
    adjacency: [HashMap<FormatId, Vec<usize>>; 2],
}

impl Planner {
    /// Collects the edges of every available engine, or of every engine when
    /// `assume_all_available` is set (used to explain a missing engine).
    ///
    /// An engine's priority is its position in `engines`: earlier engines win ties.
    pub fn new(engines: &[&dyn Converter], assume_all_available: bool) -> Self {
        Self::with_tuning(engines, assume_all_available, &EngineTuning::default())
    }

    /// Like [`Planner::new`], with the user's engine priority and cost
    /// overrides applied. `Hop::engine` still indexes into `engines`.
    pub fn with_tuning(
        engines: &[&dyn Converter],
        assume_all_available: bool,
        tuning: &EngineTuning,
    ) -> Self {
        let mut edges = Vec::new();
        // Flags only matter while building the adjacency, so `Hop` stays a
        // plain step and they live here, index for index with `edges`.
        let mut flags: Vec<EdgeFlags> = Vec::new();
        for (index, engine) in engines.iter().enumerate() {
            if !assume_all_available && !engine.status().is_available() {
                continue;
            }
            let id = engine.id();
            let penalty = tuning.penalty(id, index);
            for edge in engine.edges() {
                let cost = tuning
                    .cost_override(id, edge.from, edge.to)
                    .unwrap_or(edge.cost);
                edges.push(Hop {
                    engine: index,
                    from: edge.from,
                    to: edge.to,
                    cost: cost.saturating_add(penalty),
                });
                flags.push(edge.flags);
            }
        }

        // Parallel edges (two engines doing png -> jpg) collapse into the
        // cheapest one, first registered on a tie. Any optimal path can swap in
        // that edge for free, so the search stays exact while the walk shrinks
        // from "every engine combination" to "every format combination". Done
        // once per kind of source, because a flagged edge may be the cheapest
        // for images and not even allowed for anything else.
        let adjacency = [false, true].map(|image_source| {
            let mut adjacency: HashMap<FormatId, Vec<usize>> = HashMap::new();
            for (index, edge) in edges.iter().enumerate() {
                if !flags[index].allows(image_source) {
                    continue;
                }
                let out = adjacency.entry(edge.from).or_default();
                match out.iter_mut().find(|i| edges[**i].to == edge.to) {
                    Some(best) if edges[*best].cost <= edge.cost => {}
                    Some(best) => *best = index,
                    None => out.push(index),
                }
            }
            adjacency
        });

        Self { edges, adjacency }
    }

    /// The cheapest route from `from` to `to`, or `None` when no route of at
    /// most `MAX_HOPS` hops exists.
    ///
    /// Same-format targets only use an explicit self edge. On equal cost the
    /// route with fewer hops wins, then the one found first in registration order.
    /// Edge flags are judged by `from`: whether it is a raster image.
    pub fn plan(&self, from: FormatId, to: FormatId) -> Option<Route> {
        if from == to {
            return self.self_edge(from).map(|hop| Route {
                hops: vec![hop],
                cost: hop.cost,
            });
        }

        let mut best: Option<(u32, Vec<usize>)> = None;
        self.walk(from, &mut |path, cost| {
            let reaches = path.last().is_some_and(|i| self.edges[*i].to == to);
            if reaches && best.as_ref().is_none_or(|b| better(cost, path, b)) {
                best = Some((cost, path.to_vec()));
            }
        });
        best.map(|(cost, path)| Route {
            hops: path.iter().map(|i| self.edges[*i]).collect(),
            cost,
        })
    }

    /// True when `plan(from, to)` finds a route.
    pub fn can_convert(&self, from: FormatId, to: FormatId) -> bool {
        self.plan(from, to).is_some()
    }

    /// Every format reachable from `from`, with the cost of its cheapest route,
    /// sorted by cost and then by the order the target first appears among the
    /// engines' edges. `from` itself is listed only when a self edge exists.
    pub fn targets_from(&self, from: FormatId) -> Vec<(FormatId, u32)> {
        let mut costs: HashMap<FormatId, u32> = HashMap::new();
        if let Some(hop) = self.self_edge(from) {
            costs.insert(from, hop.cost);
        }
        self.walk(from, &mut |path, cost| {
            if let Some(last) = path.last() {
                let slot = costs.entry(self.edges[*last].to).or_insert(cost);
                *slot = (*slot).min(cost);
            }
        });

        // Registration order of targets: first time each format shows up as an
        // edge's `to`. Every reachable target has such an edge by definition.
        let mut order: HashMap<FormatId, usize> = HashMap::new();
        for edge in &self.edges {
            let next = order.len();
            order.entry(edge.to).or_insert(next);
        }

        let mut targets: Vec<(FormatId, u32)> = costs.into_iter().collect();
        targets.sort_by_key(|(format, cost)| (*cost, order.get(format).copied()));
        targets
    }

    /// The cheapest `from -> from` edge, first registered on a tie.
    fn self_edge(&self, format: FormatId) -> Option<Hop> {
        self.edges
            .iter()
            .filter(|edge| edge.from == format && edge.to == format)
            .fold(None, |best: Option<Hop>, edge| match best {
                Some(b) if b.cost <= edge.cost => Some(b),
                _ => Some(*edge),
            })
    }

    /// Calls `visit` with every simple path of 1 to `MAX_HOPS` hops starting at
    /// `from` (no format visited twice, no self edges), depth first in
    /// registration order, using only the edges whose flags allow a route
    /// from `from`. Paths are edge indices; the cost is their sum.
    fn walk(&self, from: FormatId, visit: &mut dyn FnMut(&[usize], u32)) {
        let adjacency = &self.adjacency[usize::from(is_image(from))];
        let mut path = Vec::with_capacity(MAX_HOPS);
        let mut seen = Vec::with_capacity(MAX_HOPS + 1);
        seen.push(from);
        self.walk_from(adjacency, from, 0, &mut path, &mut seen, visit);
    }

    fn walk_from(
        &self,
        adjacency: &HashMap<FormatId, Vec<usize>>,
        node: FormatId,
        cost: u32,
        path: &mut Vec<usize>,
        seen: &mut Vec<FormatId>,
        visit: &mut dyn FnMut(&[usize], u32),
    ) {
        if path.len() == MAX_HOPS {
            return;
        }
        let Some(out) = adjacency.get(&node) else {
            return;
        };
        for &index in out {
            let edge = self.edges[index];
            // Revisiting a format can never beat skipping the loop (costs are
            // never negative), and it also rules out self edges mid-route.
            if seen.contains(&edge.to) {
                continue;
            }
            let cost = cost.saturating_add(edge.cost);
            path.push(index);
            seen.push(edge.to);
            visit(path, cost);
            self.walk_from(adjacency, edge.to, cost, path, seen, visit);
            seen.pop();
            path.pop();
        }
    }
}

/// True when `format` is a raster image, which is what edge flags ask about.
/// Unknown formats are not images (fake test formats land here, harmlessly).
fn is_image(format: FormatId) -> bool {
    formats::find_by_id(format.as_str()).is_some_and(|f| f.category == Category::Image)
}

/// True when the candidate path strictly beats the best so far: cheaper, or as
/// cheap with fewer hops. Equal candidates keep the one found first.
fn better(cost: u32, path: &[usize], best: &(u32, Vec<usize>)) -> bool {
    (cost, path.len()) < (best.0, best.1.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::Edge;
    use crate::testing::FakeConverter;

    fn f(id: &'static str) -> FormatId {
        FormatId::new(id)
    }

    fn planner(engines: &[FakeConverter]) -> Planner {
        let refs: Vec<&dyn Converter> = engines.iter().map(|e| e as &dyn Converter).collect();
        Planner::new(&refs, false)
    }

    fn planner_all(engines: &[FakeConverter]) -> Planner {
        let refs: Vec<&dyn Converter> = engines.iter().map(|e| e as &dyn Converter).collect();
        Planner::new(&refs, true)
    }

    fn steps(route: &Route) -> Vec<String> {
        route
            .hops
            .iter()
            .map(|h| format!("{}→{}", h.from, h.to))
            .collect()
    }

    // --- Ported from tests/Filee.Core.Tests/RoutePlannerTests.cs ---

    #[test]
    fn direct_edge_is_used() {
        let engines = [FakeConverter::new("magick").edge("jpg", "png", 10)];
        let route = planner(&engines).plan(f("jpg"), f("png")).unwrap();
        assert_eq!(route.hops.len(), 1);
        assert_eq!(route.hops[0].engine, 0);
        assert_eq!(route.cost, 10);
    }

    #[test]
    fn two_step_route_is_found() {
        let engines = [
            FakeConverter::new("rhwp").edge("hwpx", "pdf", 10),
            FakeConverter::new("pdfium").edge("pdf", "png", 10),
        ];
        let route = planner(&engines).plan(f("hwpx"), f("png")).unwrap();
        assert_eq!(steps(&route), ["hwpx→pdf", "pdf→png"]);
        assert_eq!(route.engines().collect::<Vec<_>>(), [0, 1]);
        // 10 + 10 plus a priority penalty of 1 for the second engine.
        assert_eq!(route.cost, 21);
    }

    #[test]
    fn cheaper_route_wins() {
        let engines = [
            FakeConverter::new("slow").edge("docx", "pdf", 30),
            FakeConverter::new("fast").edge("docx", "pdf", 10),
        ];
        let route = planner(&engines).plan(f("docx"), f("pdf")).unwrap();
        assert_eq!(engines[route.hops[0].engine].id(), "fast");
    }

    #[test]
    fn priority_breaks_ties() {
        // Filee passes a penalty function; here the slice order is the priority.
        let engines = [
            FakeConverter::new("libreoffice").edge("docx", "pdf", 10),
            FakeConverter::new("word").edge("docx", "pdf", 10),
        ];
        let route = planner(&engines).plan(f("docx"), f("pdf")).unwrap();
        assert_eq!(engines[route.hops[0].engine].id(), "libreoffice");
        assert_eq!(route.cost, 10);

        let swapped = [engines[1].clone(), engines[0].clone()];
        let route = planner(&swapped).plan(f("docx"), f("pdf")).unwrap();
        assert_eq!(swapped[route.hops[0].engine].id(), "word");
    }

    #[test]
    fn same_format_requires_self_edge() {
        let with_self = [FakeConverter::new("magick").edge("jpg", "jpg", 10)];
        let without_self = [FakeConverter::new("other")
            .edge("jpg", "png", 10)
            .edge("png", "jpg", 10)];
        let route = planner(&with_self).plan(f("jpg"), f("jpg")).unwrap();
        assert_eq!(steps(&route), ["jpg→jpg"]);
        assert!(planner(&without_self).plan(f("jpg"), f("jpg")).is_none());
    }

    #[test]
    fn impossible_conversion_is_none() {
        let engines = [FakeConverter::new("magick").edge("jpg", "png", 10)];
        assert!(planner(&engines).plan(f("docx"), f("png")).is_none());
        assert!(!planner(&engines).can_convert(f("docx"), f("png")));
    }

    #[test]
    fn four_hop_routes_are_rejected() {
        let engines = [FakeConverter::new("chain")
            .edge("a", "b", 10)
            .edge("b", "c", 10)
            .edge("c", "d", 10)
            .edge("d", "e", 10)];
        let p = planner(&engines);
        assert_eq!(p.plan(f("a"), f("d")).unwrap().hops.len(), 3);
        assert!(p.plan(f("a"), f("e")).is_none());
    }

    #[test]
    fn unavailable_engines_are_excluded() {
        let engines = [FakeConverter::new("missing")
            .edge("docx", "hwpx", 10)
            .missing("pacman -S missing")];
        assert!(!planner(&engines).can_convert(f("docx"), f("hwpx")));
    }

    #[test]
    fn assume_all_available_includes_missing_engines() {
        let engines = [
            FakeConverter::new("office")
                .edge("xlsx", "pdf", 10)
                .missing("pacman -S libreoffice-fresh"),
            FakeConverter::new("images").edge("pdf", "png", 10),
        ];
        assert!(!planner(&engines).can_convert(f("xlsx"), f("png")));
        let route = planner_all(&engines).plan(f("xlsx"), f("png")).unwrap();
        assert_eq!(route.engines().collect::<Vec<_>>(), [0, 1]);
    }

    // --- MorphRing's own ---

    /// Filee's algorithm (RoutePlanner.cs:51-93), kept only to prove the
    /// regression graph below really breaks it. Returns the route cost.
    fn filee_dijkstra(edges: &[(&str, &str, u32)], from: &str, to: &str) -> Option<u32> {
        let mut dist: HashMap<&str, u32> = HashMap::from([(from, 0)]);
        let mut hops: HashMap<&str, usize> = HashMap::from([(from, 0)]);
        let mut queue = vec![(0u32, from)];
        while !queue.is_empty() {
            queue.sort_by_key(|(cost, _)| std::cmp::Reverse(*cost));
            let (cost, node) = queue.pop().unwrap();
            if cost > dist[node] {
                continue;
            }
            if node == to {
                break;
            }
            if hops[node] >= MAX_HOPS {
                continue;
            }
            for &(a, b, c) in edges.iter().filter(|e| e.0 == node) {
                let next = cost + c;
                if dist.get(b).is_some_and(|known| *known <= next) {
                    continue;
                }
                dist.insert(b, next);
                hops.insert(b, hops[a] + 1);
                queue.push((next, b));
            }
        }
        dist.get(to).copied()
    }

    #[test]
    fn a_cheap_long_path_to_an_intermediate_does_not_hide_the_target() {
        // a→x→y→b reaches b for 3 but uses up every hop, so Dijkstra never
        // expands b again; a→c→b→t is a perfectly good 3-hop route for 21.
        let graph = [
            ("a", "x", 1),
            ("x", "y", 1),
            ("y", "b", 1),
            ("a", "c", 10),
            ("c", "b", 10),
            ("b", "t", 1),
        ];
        assert_eq!(filee_dijkstra(&graph, "a", "t"), None);

        let engine = graph
            .iter()
            .fold(FakeConverter::new("graph"), |e, (a, b, c)| e.edge(a, b, *c));
        let route = planner(&[engine]).plan(f("a"), f("t")).unwrap();
        assert_eq!(steps(&route), ["a→c", "c→b", "b→t"]);
        assert_eq!(route.cost, 21);
    }

    #[test]
    fn identical_inputs_always_give_the_identical_route() {
        let engines = [
            FakeConverter::new("one")
                .edge("a", "b", 5)
                .edge("b", "d", 5)
                .edge("a", "c", 4),
            FakeConverter::new("two")
                .edge("a", "b", 4)
                .edge("c", "d", 5)
                .edge("a", "d", 11),
            FakeConverter::new("three").edge("b", "d", 4),
        ];
        let first = planner(&engines).plan(f("a"), f("d")).unwrap();
        for _ in 0..100 {
            assert_eq!(planner(&engines).plan(f("a"), f("d")).unwrap(), first);
        }
    }

    #[test]
    fn equal_cost_prefers_fewer_hops_then_registration_order() {
        let engines = [FakeConverter::new("e")
            .edge("a", "b", 5)
            .edge("b", "c", 5)
            .edge("a", "c", 10)
            .edge("a", "d", 5)
            .edge("d", "c", 5)];
        let route = planner(&engines).plan(f("a"), f("c")).unwrap();
        assert_eq!(steps(&route), ["a→c"]);

        let two_hop_only = [FakeConverter::new("e")
            .edge("a", "b", 5)
            .edge("b", "c", 5)
            .edge("a", "d", 5)
            .edge("d", "c", 5)];
        let route = planner(&two_hop_only).plan(f("a"), f("c")).unwrap();
        assert_eq!(steps(&route), ["a→b", "b→c"]);
    }

    #[test]
    fn priority_penalty_is_capped_at_nine() {
        let mut engines: Vec<FakeConverter> = (0..12).map(|_| FakeConverter::new("idle")).collect();
        engines.push(FakeConverter::new("last").edge("a", "b", 10));
        let route = planner(&engines).plan(f("a"), f("b")).unwrap();
        assert_eq!(route.hops[0].engine, 12);
        assert_eq!(route.cost, 10 + MAX_PRIORITY_PENALTY);
    }

    #[test]
    fn targets_are_sorted_by_cost_then_registration_order() {
        let engines = [FakeConverter::new("e")
            .edge("png", "png", 8)
            .edge("png", "webp", 10)
            .edge("png", "jpg", 10)
            .edge("png", "gif", 12)
            .edge("jpg", "pdf", 2)
            .edge("pdf", "txt", 30)
            .edge("txt", "mp3", 1)
            .edge("mp3", "wav", 1)];
        let targets = planner(&engines).targets_from(f("png"));
        assert_eq!(
            targets,
            [
                (f("png"), 8),
                (f("webp"), 10),
                (f("jpg"), 10),
                (f("gif"), 12),
                (f("pdf"), 12),
                (f("txt"), 42),
            ]
        );
        assert!(planner(&engines).targets_from(f("docx")).is_empty());
    }

    /// A routing-only engine with hand-made edges, flags and all. It never
    /// converts anything; the planner doesn't ask it to.
    struct Edges(&'static str, Vec<Edge>);

    impl Converter for Edges {
        fn id(&self) -> &'static str {
            self.0
        }
        fn display_name(&self) -> &'static str {
            self.0
        }
        fn edges(&self) -> &[Edge] {
            &self.1
        }
        fn status(&self) -> crate::contract::EngineStatus {
            crate::contract::EngineStatus::Available {
                path: "/usr/bin/edges".into(),
                version: "1".into(),
            }
        }
        fn convert(
            &self,
            _step: &crate::contract::Step,
            _out: &mut dyn crate::contract::OutputAllocator,
            _progress: &dyn Fn(f32),
            _cancel: &crate::contract::CancelToken,
        ) -> Result<Vec<std::path::PathBuf>, crate::error::EngineError> {
            unreachable!("routing only")
        }
    }

    fn flagged(engines: &[Edges]) -> Planner {
        let refs: Vec<&dyn Converter> = engines.iter().map(|e| e as &dyn Converter).collect();
        Planner::new(&refs, false)
    }

    // Ported from RoutePlannerTests.cs:29-51.
    #[test]
    fn edges_for_image_sources_only_are_skipped_for_other_sources() {
        let engines = [
            Edges(
                "trace",
                vec![Edge::with_cost("png", "svg", 40).image_sources_only()],
            ),
            Edges(
                "render",
                vec![Edge::new("pdf", "png"), Edge::new("icns", "png")],
            ),
        ];
        let p = flagged(&engines);
        assert_eq!(steps(&p.plan(f("png"), f("svg")).unwrap()), ["png→svg"]);
        // An ICNS is a raster image too, so it may be traced via PNG.
        assert_eq!(
            steps(&p.plan(f("icns"), f("svg")).unwrap()),
            ["icns→png", "png→svg"]
        );
        // A document rendered to PNG is never traced.
        assert!(p.plan(f("pdf"), f("svg")).is_none());
        assert!(!p.targets_from(f("pdf")).iter().any(|(t, _)| *t == f("svg")));
    }

    #[test]
    fn edges_not_for_image_sources_leave_images_to_tracing() {
        let engines = [
            Edges(
                "trace",
                vec![Edge::with_cost("png", "svg", 40).image_sources_only()],
            ),
            Edges(
                "pdf",
                vec![
                    Edge::new("png", "pdf"),
                    Edge::with_cost("pdf", "svg", 25).not_for_image_sources(),
                ],
            ),
        ];
        let p = flagged(&engines);
        // png→pdf→svg would cost 11 + 26 = 37 with penalties, less than
        // tracing's 40, but the flag keeps the photo away from LibreOffice's
        // vector export.
        assert_eq!(steps(&p.plan(f("png"), f("svg")).unwrap()), ["png→svg"]);
        assert_eq!(steps(&p.plan(f("pdf"), f("svg")).unwrap()), ["pdf→svg"]);
    }

    #[test]
    fn a_flagged_edge_does_not_shadow_a_parallel_plain_one() {
        // The cheap edge is image-only; a PDF must still get the plain one,
        // even though both collapse to the same format pair.
        let engines = [
            Edges(
                "trace",
                vec![Edge::with_cost("png", "svg", 5).image_sources_only()],
            ),
            Edges(
                "plain",
                vec![Edge::new("pdf", "png"), Edge::with_cost("png", "svg", 30)],
            ),
        ];
        let p = flagged(&engines);
        let route = p.plan(f("pdf"), f("svg")).unwrap();
        assert_eq!(route.engines().collect::<Vec<_>>(), [1, 1]);
        assert_eq!(p.plan(f("png"), f("svg")).unwrap().cost, 5);
    }

    fn tuned(engines: &[FakeConverter], tuning: &EngineTuning) -> Planner {
        let refs: Vec<&dyn Converter> = engines.iter().map(|e| e as &dyn Converter).collect();
        Planner::with_tuning(&refs, false, tuning)
    }

    #[test]
    fn default_tuning_is_registry_order() {
        let engines = [
            FakeConverter::new("libreoffice").edge("docx", "pdf", 10),
            FakeConverter::new("word").edge("docx", "pdf", 10),
        ];
        let default = tuned(&engines, &EngineTuning::default());
        assert_eq!(
            default.plan(f("docx"), f("pdf")),
            planner(&engines).plan(f("docx"), f("pdf"))
        );
    }

    #[test]
    fn reordering_the_priority_flips_a_tie() {
        let engines = [
            FakeConverter::new("libreoffice").edge("docx", "pdf", 10),
            FakeConverter::new("word").edge("docx", "pdf", 10),
        ];
        let tuning = EngineTuning {
            priority: vec!["WORD".into(), "libreoffice".into()],
            ..EngineTuning::default()
        };
        let route = tuned(&engines, &tuning).plan(f("docx"), f("pdf")).unwrap();
        assert_eq!(engines[route.hops[0].engine].id(), "word");
        assert_eq!(route.cost, 10);
    }

    #[test]
    fn engines_missing_from_the_priority_rank_after_the_listed_ones() {
        let tuning = EngineTuning {
            priority: vec!["a".into(), "b".into()],
            ..EngineTuning::default()
        };
        assert_eq!(tuning.penalty("a", 5), 0);
        assert_eq!(tuning.penalty("b", 0), 1);
        assert_eq!(tuning.penalty("stranger", 0), 2);
        let long = EngineTuning {
            priority: (0..20).map(|i| format!("e{i}")).collect(),
            ..EngineTuning::default()
        };
        assert_eq!(long.penalty("e15", 0), MAX_PRIORITY_PENALTY);
        assert_eq!(EngineTuning::default().penalty("any", 3), 3);
        assert_eq!(
            EngineTuning::default().penalty("any", 30),
            MAX_PRIORITY_PENALTY
        );
    }

    #[test]
    fn a_cost_override_flips_a_route() {
        let engines = [
            FakeConverter::new("soffice").edge("pptx", "pdf", 30),
            FakeConverter::new("other").edge("pptx", "pdf", 10),
        ];
        let route = tuned(&engines, &EngineTuning::default())
            .plan(f("pptx"), f("pdf"))
            .unwrap();
        assert_eq!(engines[route.hops[0].engine].id(), "other");

        let tuning = EngineTuning {
            cost_overrides: vec![("soffice".into(), "pptx".into(), "pdf".into(), 0)],
            ..EngineTuning::default()
        };
        let route = tuned(&engines, &tuning).plan(f("pptx"), f("pdf")).unwrap();
        assert_eq!(engines[route.hops[0].engine].id(), "soffice");
        assert_eq!(route.cost, 0);
        // Only the named engine's edge moves.
        assert_eq!(tuning.cost_override("other", f("pptx"), f("pdf")), None);
    }

    #[test]
    fn targets_agree_with_plan() {
        let engines = [
            FakeConverter::new("a")
                .edge("mp4", "png", 10)
                .edge("mp4", "gif", 15),
            FakeConverter::new("b")
                .edge("png", "jpg", 10)
                .edge("png", "gif", 12),
        ];
        let p = planner(&engines);
        for (target, cost) in p.targets_from(f("mp4")) {
            assert_eq!(p.plan(f("mp4"), target).unwrap().cost, cost);
        }
    }
}
