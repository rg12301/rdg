//! Shared force-simulation core for the [`crate::force`] and [`crate::fcose`] engines.
//!
//! A hand-rolled Barnes-Hut quadtree (O(N log N) repulsion, rather than the O(N^2) of
//! a naive all-pairs simulation) plus a spring-attraction pass along real edges. Both
//! callers seed initial positions differently (a deterministic spiral for plain force
//! layout, a spectral MDS draft for fCoSE) and hand them to [`relax`], which then runs
//! identical physics either way — there's no reason for the two engines to duplicate
//! the simulation loop itself.
//!
//! Everything here is deterministic: the only inputs to the simulation are the fixed
//! iteration count, the graph's own structure, and the caller-supplied initial
//! positions — no random number generation anywhere, so the same graph always
//! relaxes to the same coordinates.

use std::collections::HashMap;

use petgraph::stable_graph::NodeIndex;
use petgraph::visit::{EdgeRef, IntoEdgeReferences};

use rdg_graph::CompiledGraph;

use crate::DesignTokens;

// Cohesion (pulling a grouped/clustered node toward its cluster's running centroid —
// the soft stand-in for fCoSE's "physical relaxation constrained by parent convex
// hulls") is deliberately stronger than plain edge attraction: group membership is
// meant to be a real visual constraint, not a tie-breaker. Two grouped nodes whose
// only *graph* edges run through an outside node (e.g. both call a shared external
// service, but don't call each other) must still end up visually clustered — that's
// the entire point of compound/group support — even though pure graph-distance
// forces would otherwise pull them toward that shared external node instead of each
// other. A hard convex-hull clamp would need point-in-polygon geometry and can
// produce discontinuous jumps; every group `rdg` renders is a rectangle downstream
// anyway (group boxes are derived from member bounding boxes, not from a hull
// polygon), so this soft cohesion spring achieves the same visual clustering while
// composing smoothly with the rest of the simulation. See `DesignTokens::cohesion_k`
// / `attraction_k` for the actual tunable values.

#[derive(Clone, Copy)]
struct Quad {
    cx: f64,
    cy: f64,
    half: f64,
}

impl Quad {
    fn containing(points: impl Iterator<Item = (f64, f64)>) -> Self {
        let (mut min_x, mut min_y, mut max_x, mut max_y) =
            (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        let mut any = false;
        for (x, y) in points {
            any = true;
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
        if !any {
            return Quad { cx: 0.0, cy: 0.0, half: 1.0 };
        }
        let half = ((max_x - min_x).max(max_y - min_y) / 2.0).max(1.0) * 1.1;
        Quad { cx: (min_x + max_x) / 2.0, cy: (min_y + max_y) / 2.0, half }
    }

    fn quadrant_of(&self, x: f64, y: f64) -> usize {
        match (y >= self.cy, x >= self.cx) {
            (false, false) => 0,
            (false, true) => 1,
            (true, false) => 2,
            (true, true) => 3,
        }
    }

    fn child(&self, i: usize) -> Quad {
        let h = self.half / 2.0;
        let (dx, dy) = match i {
            0 => (-h, -h),
            1 => (h, -h),
            2 => (-h, h),
            _ => (h, h),
        };
        Quad { cx: self.cx + dx, cy: self.cy + dy, half: h }
    }
}

enum QuadTree {
    Empty,
    Leaf { x: f64, y: f64, mass: f64 },
    Internal { size: f64, mass: f64, cx: f64, cy: f64, children: Box<[QuadTree; 4]> },
}

impl QuadTree {
    fn build(quad: Quad, points: &[(f64, f64)], depth: u32) -> Self {
        if points.is_empty() {
            return QuadTree::Empty;
        }
        if points.len() == 1 {
            let (x, y) = points[0];
            return QuadTree::Leaf { x, y, mass: 1.0 };
        }

        let mut buckets: [Vec<(f64, f64)>; 4] = Default::default();
        for &p in points {
            buckets[quad.quadrant_of(p.0, p.1)].push(p);
        }

        // Coincident (or near-coincident) points can keep landing in the same bucket
        // forever; past a sane depth, collapse them into one merged mass instead of
        // recursing until the quad half-width underflows to zero.
        let degenerate = depth > 24 || buckets.iter().any(|b| b.len() == points.len());
        let mass = points.len() as f64;
        let cx = points.iter().map(|p| p.0).sum::<f64>() / mass;
        let cy = points.iter().map(|p| p.1).sum::<f64>() / mass;
        if degenerate {
            return QuadTree::Leaf { x: cx, y: cy, mass };
        }

        let children = Box::new([
            QuadTree::build(quad.child(0), &buckets[0], depth + 1),
            QuadTree::build(quad.child(1), &buckets[1], depth + 1),
            QuadTree::build(quad.child(2), &buckets[2], depth + 1),
            QuadTree::build(quad.child(3), &buckets[3], depth + 1),
        ]);
        QuadTree::Internal { size: quad.half * 2.0, mass, cx, cy, children }
    }

    fn accumulate_repulsion(&self, x: f64, y: f64, repulsion_k: f64, theta: f64, out: &mut (f64, f64)) {
        match self {
            QuadTree::Empty => {}
            QuadTree::Leaf { x: lx, y: ly, mass } => {
                repel(x, y, *lx, *ly, *mass, repulsion_k, out);
            }
            QuadTree::Internal { size, mass, cx, cy, children } => {
                let d = ((cx - x).powi(2) + (cy - y).powi(2)).sqrt();
                if d > 1e-9 && size / d < theta {
                    repel(x, y, *cx, *cy, *mass, repulsion_k, out);
                } else {
                    for c in children.iter() {
                        c.accumulate_repulsion(x, y, repulsion_k, theta, out);
                    }
                }
            }
        }
    }
}

fn repel(x: f64, y: f64, ox: f64, oy: f64, mass: f64, repulsion_k: f64, out: &mut (f64, f64)) {
    let dx = x - ox;
    let dy = y - oy;
    let dist_sq = (dx * dx + dy * dy).max(1.0);
    if dist_sq < 1e-6 {
        return;
    }
    let dist = dist_sq.sqrt();
    let force = repulsion_k * mass / dist_sq;
    out.0 += force * dx / dist;
    out.1 += force * dy / dist;
}

/// Deterministic spiral seed: node i sits at radius proportional to sqrt(i) and angle
/// i * golden-angle — scatters points without initial overlap or clustering using
/// nothing but each node's own index, never RNG. Shared by `force.rs` (its only seed)
/// and `fcose.rs` (its fallback for graphs too large for the spectral draft).
pub(crate) fn spiral_seed(compiled: &CompiledGraph, ideal_len: f64) -> HashMap<NodeIndex, (f64, f64)> {
    let golden_angle = std::f64::consts::PI * (3.0 - 5f64.sqrt());
    compiled
        .graph
        .node_indices()
        .enumerate()
        .map(|(i, idx)| {
            let r = ideal_len * 0.6 * (i as f64 + 1.0).sqrt();
            let theta = i as f64 * golden_angle;
            (idx, (r * theta.cos(), r * theta.sin()))
        })
        .collect()
}

/// Run `iterations` steps of Barnes-Hut repulsion + edge-spring attraction (and, when
/// `clusters` is given, a soft cohesion pull toward each node's cluster centroid),
/// starting from `initial` positions (node centers, not top-left corners).
///
/// Deterministic: iterates nodes/edges in a fixed order (`compiled.graph.node_indices()`
/// / `edge_references()`, both stable for a given `CompiledGraph`) and never samples
/// randomness, so identical inputs always relax to identical output positions.
#[allow(clippy::too_many_arguments)]
pub(crate) fn relax(
    initial: HashMap<NodeIndex, (f64, f64)>,
    compiled: &CompiledGraph,
    ideal_len: f64,
    iterations: u32,
    clusters: Option<&HashMap<NodeIndex, usize>>,
    tokens: &DesignTokens,
) -> HashMap<NodeIndex, (f64, f64)> {
    let mut pos = initial;
    if pos.len() <= 1 {
        return pos;
    }

    let order: Vec<NodeIndex> = compiled.graph.node_indices().collect();
    let mut temperature = ideal_len.max(tokens.unit * 1.25);
    // Repulsion strength scales with `ideal_len` squared, keeping the force magnitude
    // at the target separation distance constant regardless of scale (dimensionally,
    // repulsion ~ k / d^2, so k ~ d^2 holds the force at d = ideal_len fixed). Without
    // this, repulsion was a flat constant independent of node size/spacing config,
    // meaning a diagram with large node boxes or generous spacing settings still
    // packed unconnected nodes almost on top of each other — real-fixture
    // verification against force-directed output showed this directly: widening
    // `ideal_len` alone (via edge attraction) barely helped overall crowding until
    // repulsion was tied to the same scale.
    let repulsion_k = ideal_len * ideal_len * tokens.repulsion_strength_factor;

    for _ in 0..iterations {
        let quad = Quad::containing(pos.values().copied());
        let pts: Vec<(f64, f64)> = order.iter().map(|n| pos[n]).collect();
        let tree = QuadTree::build(quad, &pts, 0);

        let mut forces: HashMap<NodeIndex, (f64, f64)> =
            order.iter().map(|&n| (n, (0.0, 0.0))).collect();

        for &idx in &order {
            let (x, y) = pos[&idx];
            let mut f = (0.0, 0.0);
            // A node's own position always ends up isolated in its own leaf of the
            // tree it queries (or merged into a same-point leaf if coincident with
            // another node). Either way `dx == dy == 0.0` exactly against its own
            // coordinates, so the direction components of `repel` are zero and the
            // self-term contributes nothing — no explicit exclusion needed.
            tree.accumulate_repulsion(x, y, repulsion_k, tokens.barnes_hut_theta, &mut f);
            forces.insert(idx, f);
        }

        for edge in compiled.graph.edge_references() {
            let (a, b) = (edge.source(), edge.target());
            let (ax, ay) = pos[&a];
            let (bx, by) = pos[&b];
            let dx = bx - ax;
            let dy = by - ay;
            let dist = (dx * dx + dy * dy).sqrt().max(1.0);
            let disp = dist - ideal_len;
            let k = tokens.attraction_k * disp / dist;
            let (fx, fy) = (k * dx, k * dy);
            if let Some(f) = forces.get_mut(&a) {
                f.0 += fx;
                f.1 += fy;
            }
            if let Some(f) = forces.get_mut(&b) {
                f.0 -= fx;
                f.1 -= fy;
            }
        }

        if let Some(clusters) = clusters {
            let mut centroids: HashMap<usize, (f64, f64, f64)> = HashMap::new();
            for &idx in &order {
                if let Some(&c) = clusters.get(&idx) {
                    let (x, y) = pos[&idx];
                    let e = centroids.entry(c).or_insert((0.0, 0.0, 0.0));
                    e.0 += x;
                    e.1 += y;
                    e.2 += 1.0;
                }
            }
            for &idx in &order {
                if let Some(&c) = clusters.get(&idx) {
                    let (sx, sy, n) = centroids[&c];
                    if n > 1.0 {
                        let (cx, cy) = (sx / n, sy / n);
                        let (x, y) = pos[&idx];
                        if let Some(f) = forces.get_mut(&idx) {
                            f.0 += (cx - x) * tokens.cohesion_k;
                            f.1 += (cy - y) * tokens.cohesion_k;
                        }
                    }
                }
            }
        }

        for &idx in &order {
            let (fx, fy) = forces[&idx];
            let mag = (fx * fx + fy * fy).sqrt().max(1e-6);
            let capped = mag.min(temperature);
            let (dx, dy) = (fx / mag * capped, fy / mag * capped);
            if let Some(p) = pos.get_mut(&idx) {
                p.0 += dx;
                p.1 += dy;
            }
        }

        temperature *= tokens.cooling_rate;
    }

    pos
}

#[cfg(test)]
mod tests {
    use super::*;
    use rdg_graph::build_graph;
    use rdg_schema::DiagramPayload;

    #[test]
    fn test_relax_two_connected_nodes_converge_near_ideal_length() {
        let payload = DiagramPayload::from_yaml(
            "nodes:\n  - id: a\n    label: A\n  - id: b\n    label: B\nedges:\n  - from: a\n    to: b\n",
        )
        .unwrap();
        let compiled = build_graph(&payload).unwrap();
        let a = compiled.node_map["a"];
        let b = compiled.node_map["b"];
        let mut initial = HashMap::new();
        initial.insert(a, (0.0, 0.0));
        initial.insert(b, (500.0, 0.0));
        let result = relax(initial, &compiled, 100.0, 200, None, &DesignTokens::default());
        let (ax, ay) = result[&a];
        let (bx, by) = result[&b];
        let dist = ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt();
        assert!((dist - 100.0).abs() < 30.0, "expected ~100px apart, got {dist}");
    }

    #[test]
    fn test_relax_never_produces_nan() {
        let payload = DiagramPayload::from_yaml(
            "nodes:\n  - id: a\n    label: A\n  - id: b\n    label: B\n  - id: c\n    label: C\nedges:\n  - from: a\n    to: b\n  - from: b\n    to: c\n  - from: a\n    to: c\n",
        )
        .unwrap();
        let compiled = build_graph(&payload).unwrap();
        // All three seeded at the exact same point — the degenerate case the quadtree
        // build's coincident-point guard exists for.
        let initial: HashMap<_, _> = compiled.graph.node_indices().map(|n| (n, (0.0, 0.0))).collect();
        let result = relax(initial, &compiled, 80.0, 50, None, &DesignTokens::default());
        for (x, y) in result.values() {
            assert!(x.is_finite() && y.is_finite(), "position must be finite");
        }
    }
}
