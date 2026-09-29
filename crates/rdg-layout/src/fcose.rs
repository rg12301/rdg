//! fCoSE-style compound spring embedder — Condition C of the topology dispatcher
//! (compound/nested or disconnected graphs).
//!
//! No Rust implementation of fCoSE exists anywhere (it's a Cytoscape.js extension with
//! no port); this follows the published algorithm's phases directly rather than
//! porting existing code:
//!
//! 1. **Bridge disconnected components** — a virtual edge between each component's
//!    anchor node (its lowest-index member) and the next component's anchor, used only
//!    for the distance computation below, never rendered or added to the real graph.
//!    This stands in for literal dummy-node insertion (which would need mutable
//!    `StableDiGraph` surgery for no numerically meaningful difference: a direct
//!    anchor-to-anchor edge and an anchor-dummy-anchor path differ by exactly one hop).
//! 2. **Spectral draft layout** — classical multidimensional scaling: an all-pairs
//!    unweighted shortest-path (BFS hop-count) distance matrix over the bridged graph,
//!    double-centered, then the top-2 eigenvectors of the resulting Gram matrix (via
//!    `nalgebra`'s deterministic `SymmetricEigen`) give draft 2D coordinates that
//!    approximate those graph distances.
//! 3. **Physical relaxation constrained by parent convex hulls** — implemented as a
//!    soft cohesion force in [`crate::physics::relax`] pulling each grouped/clustered
//!    node toward its cluster's running centroid, rather than a hard convex-hull
//!    polygon clamp (every group `rdg` renders is a rectangle derived from member
//!    positions downstream anyway, so a soft pull that clusters members spatially
//!    achieves the same visual outcome without brittle point-in-polygon geometry).
//! 4. A cluster-separation pass (below) then nudges apart any two groups/components
//!    whose derived bounding boxes still collide after relaxation, and
//!    [`crate::hillclimb::refine`] does final local polish.

use std::collections::{HashMap, VecDeque};

use anyhow::Result;
use nalgebra::{DMatrix, SymmetricEigen};
use petgraph::stable_graph::NodeIndex;
use petgraph::visit::{EdgeRef, IntoEdgeReferences};

use rdg_graph::CompiledGraph;

use crate::{compute_node_sizes, hillclimb, ideal_edge_length, normalize_positions, physics, DesignTokens, LayoutConfig, LayoutResult, NodeLayout};

/// Above this node count, the O(N^3) dense eigendecomposition is skipped in favor of
/// the same deterministic spiral seed `force.rs` uses — `dispatch()` only sends
/// genuinely massive graphs to `ForceDirected`, but a compound graph that happens to
/// be large should still degrade gracefully rather than stall on a cubic solve.
const SPECTRAL_MAX_NODES: usize = 300;

fn weakly_connected_components(compiled: &CompiledGraph) -> Vec<Vec<NodeIndex>> {
    let mut parent: HashMap<NodeIndex, NodeIndex> =
        compiled.graph.node_indices().map(|n| (n, n)).collect();

    fn find(parent: &mut HashMap<NodeIndex, NodeIndex>, x: NodeIndex) -> NodeIndex {
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

    let mut groups: HashMap<NodeIndex, Vec<NodeIndex>> = HashMap::new();
    for n in compiled.graph.node_indices() {
        let root = find(&mut parent, n);
        groups.entry(root).or_default().push(n);
    }
    let mut components: Vec<Vec<NodeIndex>> = groups.into_values().collect();
    for c in &mut components {
        c.sort_by_key(|n| n.index());
    }
    components.sort_by_key(|c| c[0].index());
    components
}

/// Cluster id per node for the cohesion force: one id per declared group, plus one id
/// per multi-node connected component for nodes not otherwise grouped. Singleton
/// ungrouped nodes are left out entirely — hill climbing already resolves ordinary
/// overlaps, so there's no need to fabricate a one-node "cluster" for every node.
fn build_clusters(compiled: &CompiledGraph, components: &[Vec<NodeIndex>]) -> HashMap<NodeIndex, usize> {
    let mut clusters = HashMap::new();
    let mut next_id = 0usize;

    for group in &compiled.groups {
        let id = next_id;
        next_id += 1;
        for node_id in &group.nodes {
            if let Some(&idx) = compiled.node_map.get(node_id) {
                clusters.insert(idx, id);
            }
        }
    }

    for component in components {
        if component.len() > 1 {
            let id = next_id;
            next_id += 1;
            for &idx in component {
                clusters.entry(idx).or_insert(id);
            }
        }
    }

    clusters
}

/// BFS hop-count distance matrix over the real graph edges (undirected) plus one
/// bridge edge chaining each component's anchor to the next, so every node is
/// reachable from every other even when the real graph is disconnected.
fn bridged_distance_matrix(compiled: &CompiledGraph, components: &[Vec<NodeIndex>], order: &[NodeIndex]) -> DMatrix<f64> {
    let index_of: HashMap<NodeIndex, usize> = order.iter().enumerate().map(|(i, &n)| (n, i)).collect();
    let n = order.len();

    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); n];
    for edge in compiled.graph.edge_references() {
        let (a, b) = (index_of[&edge.source()], index_of[&edge.target()]);
        adjacency[a].push(b);
        adjacency[b].push(a);
    }
    for pair in components.windows(2) {
        let (a, b) = (index_of[&pair[0][0]], index_of[&pair[1][0]]);
        adjacency[a].push(b);
        adjacency[b].push(a);
    }

    let mut dist = DMatrix::<f64>::zeros(n, n);
    for start in 0..n {
        let mut visited = vec![false; n];
        let mut queue = VecDeque::new();
        visited[start] = true;
        queue.push_back((start, 0.0));
        while let Some((node, d)) = queue.pop_front() {
            dist[(start, node)] = d;
            for &next in &adjacency[node] {
                if !visited[next] {
                    visited[next] = true;
                    queue.push_back((next, d + 1.0));
                }
            }
        }
        // Every node is reachable via the bridge chain, but guard against any residual
        // gap (e.g. a component with no edges of its own at all) with a finite fallback
        // rather than ever leaving a zero that would collapse two unrelated nodes onto
        // the same spectral coordinate.
        for node in 0..n {
            if !visited[node] {
                dist[(start, node)] = n as f64;
            }
        }
    }
    dist
}

/// Classical multidimensional scaling: double-center the squared distance matrix and
/// take the top-2 eigenvectors of the resulting Gram matrix, scaled by the square root
/// of their eigenvalues, as draft 2D coordinates.
fn classical_mds(dist: &DMatrix<f64>) -> Vec<(f64, f64)> {
    let n = dist.nrows();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![(0.0, 0.0)];
    }

    let d2 = dist.map(|d| d * d);
    let ones = DMatrix::<f64>::from_element(n, n, 1.0 / n as f64);
    let identity = DMatrix::<f64>::identity(n, n);
    let j = identity - ones;
    let b = &j * &d2 * &j * -0.5;
    let b_sym = (&b + b.transpose()) * 0.5;

    let eigen = SymmetricEigen::new(b_sym);
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &c| eigen.eigenvalues[c].partial_cmp(&eigen.eigenvalues[a]).unwrap());

    let axis = |k: usize| -> Vec<f64> {
        let idx = order[k];
        let val = eigen.eigenvalues[idx].max(0.0).sqrt();
        eigen.eigenvectors.column(idx).iter().map(|v| v * val).collect()
    };
    let xs = axis(0);
    let ys = if n > 1 { axis(1) } else { vec![0.0; n] };

    (0..n).map(|i| (xs[i], ys[i])).collect()
}

/// Compute an fCoSE-style compound spring-embedder layout for `compiled`.
///
/// # Errors
///
/// Returns an error only if internal graph operations fail unexpectedly (kept for
/// signature parity with [`crate::compute_layout`]; the simulation itself is
/// infallible).
pub fn compute_fcose_layout(compiled: &CompiledGraph, config: &LayoutConfig) -> Result<LayoutResult> {
    if compiled.graph.node_count() == 0 {
        return Ok(LayoutResult { positions: HashMap::new(), ..Default::default() });
    }

    let sizes = compute_node_sizes(compiled, config);
    let ideal_len = ideal_edge_length(&sizes, config);

    let order: Vec<NodeIndex> = {
        let mut v: Vec<NodeIndex> = compiled.graph.node_indices().collect();
        v.sort_by_key(|n| n.index());
        v
    };
    let components = weakly_connected_components(compiled);
    let clusters = build_clusters(compiled, &components);

    let initial: HashMap<NodeIndex, (f64, f64)> = if order.len() <= SPECTRAL_MAX_NODES {
        let dist = bridged_distance_matrix(compiled, &components, &order);
        let draft = classical_mds(&dist);

        let mut edge_dist_sum = 0.0;
        let mut edge_count = 0usize;
        let pos_of: HashMap<NodeIndex, (f64, f64)> =
            order.iter().copied().zip(draft.iter().copied()).collect();
        for edge in compiled.graph.edge_references() {
            let (ax, ay) = pos_of[&edge.source()];
            let (bx, by) = pos_of[&edge.target()];
            edge_dist_sum += ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt();
            edge_count += 1;
        }
        let avg_dist = if edge_count > 0 { edge_dist_sum / edge_count as f64 } else { 0.0 };
        let scale = if avg_dist > 1e-6 { ideal_len / avg_dist } else { 1.0 };

        pos_of.into_iter().map(|(n, (x, y))| (n, (x * scale, y * scale))).collect()
    } else {
        physics::spiral_seed(compiled, ideal_len)
    };

    let relaxed = physics::relax(
        initial, compiled, ideal_len, config.tokens.physics_iterations, Some(&clusters), &config.tokens,
    );

    let mut positions = HashMap::new();
    for (idx, (cx, cy)) in relaxed {
        let (w, h) = sizes[&idx];
        positions.insert(idx, NodeLayout { x: cx - w / 2.0, y: cy - h / 2.0, width: w, height: h });
    }

    separate_overlapping_clusters(&mut positions, &clusters, config);
    hillclimb::refine(&mut positions, compiled, &config.tokens, &HashMap::new());
    // Refinement can nudge nodes back toward a neighbouring group; re-establish the
    // group gap before components are packed.
    separate_overlapping_clusters(&mut positions, &clusters, config);
    pack_components_into_grid(&mut positions, &components, config, !compiled.groups.is_empty());

    let mut result = LayoutResult { positions, ..Default::default() };
    normalize_positions(&mut result, config, !compiled.groups.is_empty());
    Ok(result)
}

fn cluster_bbox(positions: &HashMap<NodeIndex, NodeLayout>, members: &[NodeIndex]) -> (f64, f64, f64, f64) {
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for &idx in members {
        let nl = &positions[&idx];
        min_x = min_x.min(nl.x);
        min_y = min_y.min(nl.y);
        max_x = max_x.max(nl.x + nl.width);
        max_y = max_y.max(nl.y + nl.height);
    }
    (min_x, min_y, max_x, max_y)
}

/// A cluster's bounding box as it will be *drawn*: member nodes plus the group
/// container's content-aware padding on every side and the title row on top (see
/// `DesignTokens::group_pad_for`/`group_pad_top_for`) — so spacing decisions are made on
/// what the eye sees as "the group", not on the bare node cluster inside it.
fn drawn_cluster_bbox(
    positions: &HashMap<NodeIndex, NodeLayout>,
    members: &[NodeIndex],
    tokens: &DesignTokens,
) -> (f64, f64, f64, f64) {
    let (min_x, min_y, max_x, max_y) = cluster_bbox(positions, members);
    let (w, h) = (max_x - min_x, max_y - min_y);
    let pad = tokens.group_pad_for(w, h);
    let pad_top = tokens.group_pad_top_for(w, h);
    (min_x - pad, min_y - pad_top, max_x + pad, max_y + pad)
}

/// Push apart any two clusters whose *drawn* boxes (node bbox + group padding + title
/// row) are closer than `group_gap` — not just strictly overlapping, and not just the
/// bare node boxes, which is how two group containers ended up touching or overlapping
/// even though no two *nodes* did. Each pair is resolved along the axis of least
/// penetration by exactly the missing distance (split between the two), so one pass
/// settles a pair instead of creeping apart in fixed steps. Bounded, deterministic
/// (clusters visited in ascending id order every pass).
fn separate_overlapping_clusters(
    positions: &mut HashMap<NodeIndex, NodeLayout>,
    clusters: &HashMap<NodeIndex, usize>,
    config: &LayoutConfig,
) {
    if clusters.is_empty() {
        return;
    }
    let tokens = &config.tokens;
    let mut members: HashMap<usize, Vec<NodeIndex>> = HashMap::new();
    for (&idx, &c) in clusters {
        members.entry(c).or_default().push(idx);
    }
    let mut ids: Vec<usize> = members.keys().copied().collect();
    ids.sort_unstable();
    if ids.len() < 2 {
        return;
    }
    // Sort members once so per-cluster iteration order never depends on hash order.
    for m in members.values_mut() {
        m.sort_by_key(|n| n.index());
    }

    for _ in 0..tokens.max_hillclimb_passes {
        let mut moved = false;
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                let a = drawn_cluster_bbox(positions, &members[&ids[i]], tokens);
                let b = drawn_cluster_bbox(positions, &members[&ids[j]], tokens);
                // Required clearance between drawn boxes, per axis.
                let need_x = config.group_gap_x;
                let need_y = config.group_gap_y;
                // Positive on an axis = how far the boxes intrude into the required gap.
                let pen_x = a.2.min(b.2) - a.0.max(b.0) + need_x;
                let pen_y = a.3.min(b.3) - a.1.max(b.1) + need_y;
                if pen_x <= 0.0 || pen_y <= 0.0 {
                    continue;
                }
                let (acx, acy) = ((a.0 + a.2) / 2.0, (a.1 + a.3) / 2.0);
                let (bcx, bcy) = ((b.0 + b.2) / 2.0, (b.1 + b.3) / 2.0);
                let (dx, dy) = if pen_x <= pen_y {
                    (if bcx >= acx { pen_x } else { -pen_x }, 0.0)
                } else {
                    (0.0, if bcy >= acy { pen_y } else { -pen_y })
                };
                for &idx in &members[&ids[i]] {
                    if let Some(nl) = positions.get_mut(&idx) {
                        nl.x -= dx / 2.0;
                        nl.y -= dy / 2.0;
                    }
                }
                for &idx in &members[&ids[j]] {
                    if let Some(nl) = positions.get_mut(&idx) {
                        nl.x += dx / 2.0;
                        nl.y += dy / 2.0;
                    }
                }
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
}

/// Re-arranges whole weakly-connected components into a compact row-major grid,
/// preserving each component's own internal layout (a rigid translation only — the
/// physics/hillclimb result for how nodes sit *relative to each other* within a
/// component is left alone).
///
/// The physics simulation has no notion of "tidy overall composition" — nothing
/// pulls separate components toward each other once they're far enough apart that
/// [`separate_overlapping_clusters`] has nothing left to resolve, so genuinely
/// disconnected subsystems (the exact case this layout exists for) settled wherever
/// their mutual repulsion happened to leave them: scattered at wildly different
/// distances with huge dead canvas space between them, sometimes diagonally, rather
/// than composed as a coherent whole. Confirmed directly by visually reviewing a
/// disconnected-subsystems sample diagram, not a hypothetical — three components
/// ended up in three corners of a mostly-empty canvas. A deliberate grid tiling (the
/// same instinct a human laying out several unrelated boxes on a page would reach
/// for) replaces that arbitrary physics equilibrium for *inter*-component placement.
fn pack_components_into_grid(
    positions: &mut HashMap<NodeIndex, NodeLayout>,
    components: &[Vec<NodeIndex>],
    config: &LayoutConfig,
    has_groups: bool,
) {
    if components.len() < 2 {
        return;
    }

    // `components` is already sorted by each component's lowest node index
    // (`weakly_connected_components`), so this ordering — and thus the grid it
    // produces — is deterministic.
    let bboxes: Vec<(f64, f64, f64, f64)> =
        components.iter().map(|c| cluster_bbox(positions, c)).collect();
    // With group containers, each component is drawn inside a padded box (title row on
    // top), so grid cells are sized and spaced from that drawn box; the nodes then sit
    // inset within their cell by exactly that padding. Without groups there is no box
    // and cells stay the bare node bounds.
    let insets: Vec<(f64, f64, f64)> = bboxes
        .iter()
        .map(|&(min_x, min_y, max_x, max_y)| {
            if has_groups {
                let (w, h) = (max_x - min_x, max_y - min_y);
                (config.tokens.group_pad_for(w, h), config.tokens.group_pad_top_for(w, h), config.tokens.group_pad_for(w, h))
            } else {
                (0.0, 0.0, 0.0)
            }
        })
        .collect();

    // A roughly-square grid reads as a deliberate, balanced composition for any
    // component count without special-casing small counts — 2 components make a
    // 2x1 row, 3-4 make a 2x2, 5-6 a 3x2, and so on.
    let cols = (components.len() as f64).sqrt().ceil() as usize;

    let gap_x = config.group_gap_x;
    let gap_y = config.group_gap_y;

    let mut row_heights: Vec<f64> = Vec::new();
    for (i, &(_, min_y, _, max_y)) in bboxes.iter().enumerate() {
        let row = i / cols;
        let h = max_y - min_y + insets[i].1 + insets[i].2;
        if row == row_heights.len() {
            row_heights.push(h);
        } else {
            row_heights[row] = row_heights[row].max(h);
        }
    }

    let mut col_widths: Vec<f64> = Vec::new();
    for (i, &(min_x, _, max_x, _)) in bboxes.iter().enumerate() {
        let col = i % cols;
        let w = max_x - min_x + 2.0 * insets[i].0;
        if col >= col_widths.len() {
            col_widths.resize(col + 1, 0.0);
        }
        col_widths[col] = col_widths[col].max(w);
    }

    let mut col_x = vec![0.0; col_widths.len() + 1];
    for c in 0..col_widths.len() {
        col_x[c + 1] = col_x[c] + col_widths[c] + gap_x;
    }
    let mut row_y = vec![0.0; row_heights.len() + 1];
    for r in 0..row_heights.len() {
        row_y[r + 1] = row_y[r] + row_heights[r] + gap_y;
    }

    for (i, component) in components.iter().enumerate() {
        let (row, col) = (i / cols, i % cols);
        let (min_x, min_y, _, _) = bboxes[i];
        let (target_x, target_y) = (col_x[col] + insets[i].0, row_y[row] + insets[i].1);
        let (dx, dy) = (target_x - min_x, target_y - min_y);
        for &idx in component {
            if let Some(nl) = positions.get_mut(&idx) {
                nl.x += dx;
                nl.y += dy;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rdg_graph::build_graph;
    use rdg_schema::DiagramPayload;

    fn boxes_overlap(a: &NodeLayout, b: &NodeLayout) -> bool {
        a.x < b.x + b.width && a.x + a.width > b.x && a.y < b.y + b.height && a.y + a.height > b.y
    }

    #[test]
    fn test_empty_graph_returns_empty_positions() {
        let payload = DiagramPayload::default();
        let compiled = build_graph(&payload).unwrap();
        let result = compute_fcose_layout(&compiled, &LayoutConfig::default()).unwrap();
        assert!(result.positions.is_empty());
    }

    #[test]
    fn test_disconnected_components_do_not_overlap() {
        let yaml = "nodes:\n  - id: a\n    label: A\n  - id: b\n    label: B\n  - id: c\n    label: C\n  - id: d\n    label: D\nedges:\n  - from: a\n    to: b\n  - from: c\n    to: d\n";
        let compiled = build_graph(&DiagramPayload::from_yaml(yaml).unwrap()).unwrap();
        let result = compute_fcose_layout(&compiled, &LayoutConfig::default()).unwrap();
        let boxes: Vec<_> = result.positions.values().collect();
        for i in 0..boxes.len() {
            for j in (i + 1)..boxes.len() {
                assert!(!boxes_overlap(boxes[i], boxes[j]), "nodes {i}/{j} overlap");
            }
        }
    }

    #[test]
    fn test_grouped_nodes_end_up_closer_to_each_other_than_to_outsiders() {
        // Wide ideal spacing relative to node box size gives the cohesion spring (which
        // pulls a/b toward zero separation, floored by proper non-overlap resolution
        // once they'd otherwise collide) room to produce a clearly smaller a-b distance
        // than the real-edge-driven a-c/b-c distances, which settle near the much
        // larger ideal length.
        let yaml = "nodes:\n  - id: a\n    label: A\n  - id: b\n    label: B\n  - id: c\n    label: C\ngroups:\n  - id: g1\n    label: Group\n    nodes: [a, b]\nedges:\n  - from: a\n    to: c\n  - from: b\n    to: c\n";
        let compiled = build_graph(&DiagramPayload::from_yaml(yaml).unwrap()).unwrap();
        let config = LayoutConfig { rank_spacing: 300, node_spacing: 300, ..LayoutConfig::default() };
        let result = compute_fcose_layout(&compiled, &config).unwrap();
        let a = &result.positions[&compiled.node_map["a"]];
        let b = &result.positions[&compiled.node_map["b"]];
        let c = &result.positions[&compiled.node_map["c"]];
        let center = |nl: &NodeLayout| (nl.x + nl.width / 2.0, nl.y + nl.height / 2.0);
        let dist = |p: (f64, f64), q: (f64, f64)| ((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2)).sqrt();
        let (ac, bc, cc) = (center(a), center(b), center(c));
        assert!(
            dist(ac, bc) < dist(ac, cc) * 0.6,
            "grouped nodes a/b (dist {}) should end up much closer together than a is to outsider c (dist {})",
            dist(ac, bc),
            dist(ac, cc)
        );
    }

    #[test]
    fn test_all_positions_are_finite() {
        let yaml = "nodes:\n  - id: a\n    label: A\n  - id: b\n    label: B\n  - id: c\n    label: C\ngroups:\n  - id: g1\n    label: Group\n    nodes: [a, b, c]\nedges:\n  - from: a\n    to: b\n  - from: b\n    to: c\n";
        let compiled = build_graph(&DiagramPayload::from_yaml(yaml).unwrap()).unwrap();
        let result = compute_fcose_layout(&compiled, &LayoutConfig::default()).unwrap();
        for nl in result.positions.values() {
            assert!(nl.x.is_finite() && nl.y.is_finite());
        }
    }

    #[test]
    fn test_layout_is_deterministic_across_runs() {
        let yaml = "nodes:\n  - id: a\n    label: A\n  - id: b\n    label: B\n  - id: c\n    label: C\n  - id: d\n    label: D\ngroups:\n  - id: g1\n    label: Group\n    nodes: [a, b]\nedges:\n  - from: a\n    to: c\n  - from: b\n    to: d\n";
        let payload = DiagramPayload::from_yaml(yaml).unwrap();
        let compiled = build_graph(&payload).unwrap();
        let config = LayoutConfig::default();
        let r1 = compute_fcose_layout(&compiled, &config).unwrap();
        let r2 = compute_fcose_layout(&compiled, &config).unwrap();
        for (idx, nl1) in &r1.positions {
            let nl2 = &r2.positions[idx];
            assert_eq!(nl1.x, nl2.x);
            assert_eq!(nl1.y, nl2.y);
        }
    }

    #[test]
    fn test_single_node_gets_a_position() {
        let payload = DiagramPayload::from_yaml("nodes:\n  - id: solo\n    label: Solo\n").unwrap();
        let compiled = build_graph(&payload).unwrap();
        let result = compute_fcose_layout(&compiled, &LayoutConfig::default()).unwrap();
        assert_eq!(result.positions.len(), 1);
    }
}
