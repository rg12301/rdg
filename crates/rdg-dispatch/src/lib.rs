//! Topology analyzer and algorithm dispatcher.
//!
//! `rdg` originally ran every input graph through the same Sugiyama-style layered
//! layout regardless of its shape. This crate extracts a small set of topological
//! metrics from a [`rdg_graph::CompiledGraph`] (Phase 1) and maps them, via a strict
//! decision tree (Phase 2), to the layout framework and routing algorithm best suited
//! to that shape. It has no knowledge of *how* a framework computes coordinates —
//! that lives in `rdg-layout` and `rdg-render-core` — this crate only decides *which*
//! one to use and reports the decision in a form the CLI can print and downstream
//! crates can act on (Phase 4's output structs).
//!
//! Phase 3 ("stabilization / stochastic hill climbing") has no home here: it's a
//! post-processing step over already-computed coordinates, implemented in
//! `rdg-layout::hillclimb`, not a dispatch decision.

use std::collections::{HashMap, HashSet};

use petgraph::visit::{EdgeRef, IntoEdgeReferences, IntoNodeIdentifiers};
use rdg_graph::CompiledGraph;
use rdg_layout::DesignTokens;

/// Number of weakly-connected components in a graph, treating every edge as
/// undirected. `StableDiGraph` doesn't implement petgraph's `NodeCompactIndexable`
/// (node indices aren't guaranteed compact after removals), so
/// `petgraph::algo::connected_components` isn't usable directly here — this is a
/// small hand-rolled union-find instead.
fn weakly_connected_components(compiled: &CompiledGraph) -> usize {
    let mut parent: HashMap<_, _> =
        compiled.graph.node_identifiers().map(|n| (n, n)).collect();

    fn find(parent: &mut HashMap<petgraph::graph::NodeIndex, petgraph::graph::NodeIndex>, x: petgraph::graph::NodeIndex) -> petgraph::graph::NodeIndex {
        let p = parent[&x];
        if p != x {
            let root = find(parent, p);
            parent.insert(x, root);
            root
        } else {
            x
        }
    }

    for edge in compiled.graph.edge_references() {
        let (a, b) = (find(&mut parent, edge.source()), find(&mut parent, edge.target()));
        if a != b {
            parent.insert(a, b);
        }
    }

    compiled.graph.node_identifiers().map(|n| find(&mut parent, n)).collect::<HashSet<_>>().len()
}

// The density/node-count thresholds this module used to declare as its own
// constants (`DENSITY_THRESHOLD`, `MASSIVE_NODE_THRESHOLD`,
// `OBSTACLE_DENSE_NODE_THRESHOLD`) now live on `DesignTokens` — see that struct's
// docs in `rdg-layout` — so every `analyze`/`dispatch` call below takes `tokens:
// &DesignTokens` and reads them from there instead.

/// Phase 1 — topological signature extracted from a compiled graph.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TopologySignature {
    pub node_count: usize,
    pub edge_count: usize,
    /// `edge_count as f64 / node_count as f64`. `0.0` for an empty graph.
    pub density: f64,
    /// `density > `[`DesignTokens::density_threshold`].
    pub is_dense: bool,
    /// Whether the raw input graph (before FAS cycle-breaking) contained a cycle.
    pub has_cycles: bool,
    /// `true` when edges are predominantly ordered/numbered (a chronological process
    /// flow); `false` for an unordered dependency graph. A graph with no edges is
    /// treated as `false` (there is no chronology to speak of).
    pub is_chronological: bool,
    /// Whether the diagram declares visual groups (compound structure).
    pub has_compound_structure: bool,
    /// Number of weakly-connected components (edges treated as undirected).
    pub component_count: usize,
}

/// Phase 1 — extract a [`TopologySignature`] from a compiled graph.
pub fn analyze(compiled: &CompiledGraph, tokens: &DesignTokens) -> TopologySignature {
    let node_count = compiled.graph.node_count();
    let edge_count = compiled.graph.edge_count();

    let density = if node_count == 0 { 0.0 } else { edge_count as f64 / node_count as f64 };

    let numbered_edges =
        compiled.graph.edge_references().filter(|e| e.weight().step.is_some()).count();
    let is_chronological = edge_count > 0 && (numbered_edges as f64 / edge_count as f64) > 0.5;

    let component_count = weakly_connected_components(compiled);

    TopologySignature {
        node_count,
        edge_count,
        density,
        is_dense: density > tokens.density_threshold,
        has_cycles: compiled.had_cycles,
        is_chronological,
        has_compound_structure: !compiled.groups.is_empty(),
        component_count,
    }
}

/// Phase 2 — which layout framework a diagram's node placement should use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutFramework {
    /// Hierarchical / process-flow layered layout (feasible-tree ranking, dummy-node
    /// long-edge injection, barycenter crossing reduction). The existing default.
    Sugiyama,
    /// Barnes-Hut force-directed layout for dense or massive graphs.
    ForceDirected,
    /// fCoSE-style compound spring embedder for nested/disconnected structure.
    FCose,
}

/// Which edge-routing algorithm to use once node positions are fixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingAlgorithm {
    /// The existing fixed corridor/doorway heuristic router.
    CornerHeuristic,
    /// Orthogonal visibility graph + A*, penalizing bends and obstacle proximity.
    VisibilityGraphAStar,
}

/// A rough asymptotic-complexity classification for the chosen framework, purely for
/// reporting to the user — not used to make further decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BigO {
    Linear,
    NLogN,
    NSquared,
}

impl std::fmt::Display for BigO {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            BigO::Linear => "O(N)",
            BigO::NLogN => "O(N log N)",
            BigO::NSquared => "O(N^2)",
        };
        f.write_str(s)
    }
}

/// Routing rules attached to a dispatch decision.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoutingRules {
    pub algorithm: RoutingAlgorithm,
    /// Extra path cost charged per direction change, in the same units as segment
    /// length (pixels). Only meaningful for [`RoutingAlgorithm::VisibilityGraphAStar`].
    pub bend_penalty: f64,
    /// Minimum clearance, in pixels, routing should try to keep from obstacle edges.
    pub obstacle_clearance: f64,
}

/// Phase 4 — the full output of the dispatcher: everything downstream crates need to
/// execute the chosen strategy and everything the CLI needs to report it.
#[derive(Debug, Clone, PartialEq)]
pub struct AlgorithmDecision {
    pub framework: LayoutFramework,
    pub complexity_estimate: BigO,
    /// Human-readable preprocessing steps, for the CLI's stderr dispatch report.
    pub preprocessing_steps: Vec<String>,
    pub routing: RoutingRules,
}

/// Phase 2 — map a [`TopologySignature`] to an [`AlgorithmDecision`].
///
/// Priority order (the source spec lists four conditions but does not resolve their
/// overlaps, so one is fixed here explicitly):
///
/// 1. `node_count > `[`DesignTokens::massive_node_threshold`] → `ForceDirected`. A hard safety valve:
///    regardless of shape, a graph this large cannot be laid out hierarchically in
///    reasonable time or produce a legible result.
/// 2. else `component_count > 1` → `FCose`. Genuinely disconnected structure is the one
///    shape `Sugiyama` (and its compound-grid variant) has **no** handling for at all —
///    it assumes a single connected structure to rank. This is deliberately narrower
///    than "has compound structure": a real-fixture regression surfaced during
///    verification showed that routing every grouped-but-connected diagram to `FCose`
///    made things *worse* — `Sugiyama`'s existing compound grid-search (a mature,
///    already-proven discrete placement search) handles ordinary flat visual groups
///    on a connected graph better than the spring embedder does, so those diagrams
///    stay on `Sugiyama`, which still applies its own compound-aware layout internally
///    when groups are present. `FCose` is reserved for the case nothing else covers.
/// 3. else `is_dense` → `ForceDirected`. Dense but not massive is still a poor fit for
///    layered ranking (too many crossing long edges).
/// 4. else → `Sugiyama`. The common case: a dependency DAG or chronological process
///    flow, which is what the vast majority of `rdg` diagrams are — including grouped
///    ones, per point 2 above.
///
/// Routing is decided independently of the framework, based only on obstacle density
/// (`node_count`), since it governs edge routing cost regardless of how nodes were
/// placed.
pub fn dispatch(sig: &TopologySignature, tokens: &DesignTokens) -> AlgorithmDecision {
    let mut steps = Vec::new();

    let framework = if sig.node_count > tokens.massive_node_threshold {
        steps.push(format!(
            "graph has {} nodes (> {}) — massive-graph threshold triggers force-directed layout",
            sig.node_count, tokens.massive_node_threshold
        ));
        LayoutFramework::ForceDirected
    } else if sig.component_count > 1 {
        steps.push(format!(
            "graph has {} disconnected components — bridging with dummy nodes",
            sig.component_count
        ));
        steps.push("spectral draft layout (classical MDS) + convex-hull-constrained relaxation"
            .into());
        LayoutFramework::FCose
    } else if sig.is_dense {
        steps.push(format!(
            "density {:.2} edges/node exceeds {:.1} — too dense for layered ranking",
            sig.density, tokens.density_threshold
        ));
        LayoutFramework::ForceDirected
    } else {
        steps.push(if sig.is_chronological {
            "predominantly numbered edges — chronological process flow".into()
        } else {
            "acyclic-after-FAS dependency structure".into()
        });
        if sig.has_compound_structure {
            steps.push("visual groups present — compound grid-search layout applied".into());
        }
        LayoutFramework::Sugiyama
    };

    if sig.has_cycles && framework == LayoutFramework::Sugiyama {
        steps.push("cycles detected — greedy feedback-arc-set cycle breaking applied upstream"
            .into());
    }

    let complexity_estimate = match framework {
        LayoutFramework::Sugiyama => BigO::NLogN,
        LayoutFramework::ForceDirected => BigO::NLogN,
        LayoutFramework::FCose => BigO::NSquared,
    };

    // The corner heuristic's channel/corridor logic assumes layered, rank-based
    // positions (predictable "Bottom -> Top" and "Left -> Right" transitions between
    // ranks) — real-fixture verification confirmed it produces many edge-through-node
    // anomalies on force-directed/fCoSE output even at small node counts, because
    // those engines scatter nodes with no such structure at all. So non-Sugiyama
    // frameworks always get the globally obstacle-aware A* router regardless of size,
    // not just once a diagram crosses the node-count threshold.
    let algorithm = if framework != LayoutFramework::Sugiyama
        || sig.node_count > tokens.obstacle_dense_node_threshold
    {
        RoutingAlgorithm::VisibilityGraphAStar
    } else {
        RoutingAlgorithm::CornerHeuristic
    };

    AlgorithmDecision {
        framework,
        complexity_estimate,
        preprocessing_steps: steps,
        routing: RoutingRules {
            algorithm,
            bend_penalty: tokens.astar_bend_penalty,
            obstacle_clearance: tokens.stub_clearance(),
        },
    }
}

/// Distinct node ids referenced by a group's membership list that also exist in the
/// graph. Exposed for callers (e.g. `rdg-layout::fcose`) that need to know which
/// nodes are "grouped" without re-deriving it from `CompiledGraph::groups` themselves.
pub fn grouped_node_ids(compiled: &CompiledGraph) -> HashSet<String> {
    compiled.groups.iter().flat_map(|g| g.nodes.iter().cloned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rdg_graph::build_graph;
    use rdg_schema::DiagramPayload;

    fn compiled(yaml: &str) -> CompiledGraph {
        let payload = DiagramPayload::from_yaml(yaml).expect("valid yaml");
        build_graph(&payload).expect("graph builds")
    }

    #[test]
    fn test_empty_graph_signature() {
        let sig = analyze(&compiled("nodes: []\nedges: []\n"), &DesignTokens::default());
        assert_eq!(sig.node_count, 0);
        assert_eq!(sig.edge_count, 0);
        assert_eq!(sig.density, 0.0);
        assert!(!sig.is_dense);
        assert!(!sig.is_chronological);
        assert_eq!(sig.component_count, 0);
    }

    #[test]
    fn test_acyclic_chain_has_no_cycles() {
        let sig = analyze(&compiled(
            "nodes:\n  - id: a\n    label: A\n  - id: b\n    label: B\nedges:\n  - from: a\n    to: b\n",
        ), &DesignTokens::default());
        assert!(!sig.has_cycles);
    }

    #[test]
    fn test_cyclic_graph_flags_cycles() {
        let sig = analyze(&compiled(
            "nodes:\n  - id: a\n    label: A\n  - id: b\n    label: B\nedges:\n  - from: a\n    to: b\n  - from: b\n    to: a\n",
        ), &DesignTokens::default());
        assert!(sig.has_cycles);
    }

    #[test]
    fn test_sparse_chain_is_not_dense() {
        let sig = analyze(&compiled(
            "nodes:\n  - id: a\n    label: A\n  - id: b\n    label: B\n  - id: c\n    label: C\nedges:\n  - from: a\n    to: b\n  - from: b\n    to: c\n",
        ), &DesignTokens::default());
        assert!(!sig.is_dense);
    }

    #[test]
    fn test_dense_mesh_is_dense() {
        // 4 nodes, fully connected both directions = 12 edges, density 3.0 > threshold.
        let yaml = "nodes:\n  - id: a\n    label: A\n  - id: b\n    label: B\n  - id: c\n    label: C\n  - id: d\n    label: D\nedges:\n  - {from: a, to: b}\n  - {from: a, to: c}\n  - {from: a, to: d}\n  - {from: b, to: a}\n  - {from: b, to: c}\n  - {from: b, to: d}\n  - {from: c, to: a}\n  - {from: c, to: b}\n  - {from: c, to: d}\n  - {from: d, to: a}\n  - {from: d, to: b}\n  - {from: d, to: c}\n";
        let sig = analyze(&compiled(yaml), &DesignTokens::default());
        assert!(sig.is_dense, "density was {}", sig.density);
    }

    #[test]
    fn test_flat_graph_has_no_compound_structure() {
        let sig = analyze(&compiled(
            "nodes:\n  - id: a\n    label: A\n  - id: b\n    label: B\nedges:\n  - from: a\n    to: b\n",
        ), &DesignTokens::default());
        assert!(!sig.has_compound_structure);
    }

    #[test]
    fn test_grouped_graph_has_compound_structure() {
        let yaml = "nodes:\n  - id: a\n    label: A\n  - id: b\n    label: B\ngroups:\n  - id: g1\n    label: Group\n    nodes: [a, b]\nedges:\n  - from: a\n    to: b\n";
        let sig = analyze(&compiled(yaml), &DesignTokens::default());
        assert!(sig.has_compound_structure);
        assert_eq!(grouped_node_ids(&compiled(yaml)).len(), 2);
    }

    #[test]
    fn test_single_component_count() {
        let sig = analyze(&compiled(
            "nodes:\n  - id: a\n    label: A\n  - id: b\n    label: B\nedges:\n  - from: a\n    to: b\n",
        ), &DesignTokens::default());
        assert_eq!(sig.component_count, 1);
    }

    #[test]
    fn test_disconnected_graph_multiple_components() {
        let yaml = "nodes:\n  - id: a\n    label: A\n  - id: b\n    label: B\n  - id: c\n    label: C\n  - id: d\n    label: D\nedges:\n  - from: a\n    to: b\n  - from: c\n    to: d\n";
        let sig = analyze(&compiled(yaml), &DesignTokens::default());
        assert_eq!(sig.component_count, 2);
    }

    #[test]
    fn test_numbered_edges_are_chronological() {
        let yaml = "numbered: true\nnodes:\n  - id: a\n    label: A\n  - id: b\n    label: B\n  - id: c\n    label: C\nedges:\n  - from: a\n    to: b\n  - from: b\n    to: c\n";
        let sig = analyze(&compiled(yaml), &DesignTokens::default());
        assert!(sig.is_chronological);
    }

    #[test]
    fn test_unnumbered_edges_are_not_chronological() {
        let sig = analyze(&compiled(
            "nodes:\n  - id: a\n    label: A\n  - id: b\n    label: B\nedges:\n  - from: a\n    to: b\n",
        ), &DesignTokens::default());
        assert!(!sig.is_chronological);
    }

    fn signature(node_count: usize, edge_count: usize, dense: bool, cycles: bool, chrono: bool, compound: bool, components: usize) -> TopologySignature {
        TopologySignature {
            node_count,
            edge_count,
            density: edge_count as f64 / node_count.max(1) as f64,
            is_dense: dense,
            has_cycles: cycles,
            is_chronological: chrono,
            has_compound_structure: compound,
            component_count: components,
        }
    }

    #[test]
    fn test_dispatch_massive_graph_always_forces_force_directed() {
        let sig = signature(20_000, 20_000, false, false, false, true, 5);
        let decision = dispatch(&sig, &DesignTokens::default());
        assert_eq!(decision.framework, LayoutFramework::ForceDirected);
    }

    #[test]
    fn test_dispatch_connected_compound_graph_stays_on_sugiyama() {
        // A single connected component with visual groups is exactly the shape
        // `Sugiyama`'s own compound grid-search already handles well — routing it to
        // `FCose` instead was a real regression caught by re-rendering rdg's own
        // `docs/*.yaml` fixtures during verification, not a hypothetical.
        let sig = signature(50, 60, false, false, false, true, 1);
        let decision = dispatch(&sig, &DesignTokens::default());
        assert_eq!(decision.framework, LayoutFramework::Sugiyama);
    }

    #[test]
    fn test_dispatch_dense_takes_priority_over_connected_compound() {
        let sig = signature(50, 200, true, false, false, true, 1);
        let decision = dispatch(&sig, &DesignTokens::default());
        assert_eq!(decision.framework, LayoutFramework::ForceDirected);
    }

    #[test]
    fn test_dispatch_multi_component_dispatches_fcose() {
        let sig = signature(10, 8, false, false, false, false, 3);
        let decision = dispatch(&sig, &DesignTokens::default());
        assert_eq!(decision.framework, LayoutFramework::FCose);
    }

    #[test]
    fn test_dispatch_dense_flat_graph_dispatches_force_directed() {
        let sig = signature(20, 60, true, false, false, false, 1);
        let decision = dispatch(&sig, &DesignTokens::default());
        assert_eq!(decision.framework, LayoutFramework::ForceDirected);
    }

    #[test]
    fn test_dispatch_default_case_is_sugiyama() {
        let sig = signature(5, 4, false, false, false, false, 1);
        let decision = dispatch(&sig, &DesignTokens::default());
        assert_eq!(decision.framework, LayoutFramework::Sugiyama);
    }

    #[test]
    fn test_dispatch_small_graph_uses_corner_heuristic_routing() {
        let sig = signature(5, 4, false, false, false, false, 1);
        let decision = dispatch(&sig, &DesignTokens::default());
        assert_eq!(decision.routing.algorithm, RoutingAlgorithm::CornerHeuristic);
    }

    #[test]
    fn test_dispatch_obstacle_dense_graph_uses_astar_routing() {
        let sig = signature(30, 40, false, false, false, false, 1);
        let decision = dispatch(&sig, &DesignTokens::default());
        assert_eq!(decision.routing.algorithm, RoutingAlgorithm::VisibilityGraphAStar);
    }

    #[test]
    fn test_dispatch_preprocessing_steps_are_non_empty() {
        let sig = signature(5, 4, false, false, false, false, 1);
        let decision = dispatch(&sig, &DesignTokens::default());
        assert!(!decision.preprocessing_steps.is_empty());
    }

    #[test]
    fn test_big_o_display() {
        assert_eq!(BigO::NLogN.to_string(), "O(N log N)");
        assert_eq!(BigO::NSquared.to_string(), "O(N^2)");
        assert_eq!(BigO::Linear.to_string(), "O(N)");
    }
}
