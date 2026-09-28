//! Deterministic local coordinate refinement — the Phase-3 "stabilization" pass for
//! the force-directed and fCoSE engines.
//!
//! The spec calls for Stochastic Hill Climbing. Genuine RNG-driven moves would break
//! `rdg`'s core guarantee that identical YAML always produces byte-identical output
//! (the same guarantee a real bug fix in this codebase's history was about preserving),
//! so this is a **deterministic** variant: each node tries a fixed, order-independent
//! set of candidate offsets and only keeps a move that strictly reduces a local cost
//! function (edge length + overlap). The "stochastic" character — many small greedy
//! local perturbations rather than a global recompute — is preserved; the randomness
//! is not.
//!
//! `sugiyama.rs`'s layered layout already has its own analogous stabilization
//! (`straighten_layers`) and doesn't use this — it's specific to the messier raw
//! output of the force/fCoSE engines.

use std::collections::HashMap;

use petgraph::stable_graph::NodeIndex;
use petgraph::visit::EdgeRef;
use petgraph::Direction;

use rdg_graph::CompiledGraph;

use crate::{DesignTokens, NodeLayout};

/// Above this node count, hill climbing is skipped entirely. Its cost function scans
/// every other node for overlaps, so a full pass is O(N^2); at the scale where
/// `ForceDirected` is chosen for being "massive" (tens of thousands of nodes), local
/// per-node polish is not worth its own runtime — the Barnes-Hut placement itself is
/// the thing keeping that case tractable. A performance safety valve, not a design
/// value, so it stays a plain constant rather than a token.
pub(crate) const MAX_NODES: usize = 500;

/// Cost charged per square pixel of AABB overlap. Must be proportional to overlap
/// *area*, not a flat per-overlap penalty — a flat penalty gives hill climbing no
/// gradient to follow out of a deep overlap (every partial step still "fully
/// overlaps", so no candidate move looks better than any other) and is not just a
/// tests-only concern, since a bad enough starting position from the raw
/// force/fCoSE placement could suffer the same total-overlap stall in real output.
/// A cost-function weight relative to other terms in [`node_cost`], not a pixel
/// measurement, so it stays a plain constant rather than a token.
const OVERLAP_PENALTY_PER_AREA: f64 = 50.0;

/// Fixed-magnitude nudges for fine-tuning once a node is already roughly clear of
/// overlap, in units of [`DesignTokens::unit`]. On their own these are too small to
/// escape a *deep* overlap (a node buried 100+px inside another box needs many
/// single-digit passes just to reach the edge) — see [`overlap_resolution_push`] for
/// the move that actually closes that gap.
fn candidate_offsets(tokens: &DesignTokens) -> [(f64, f64); 12] {
    let small = tokens.px(1.75);
    let diag = tokens.px(1.25);
    let big = tokens.px(5.0);
    [
        (small, 0.0), (-small, 0.0), (0.0, small), (0.0, -small),
        (diag, diag), (-diag, diag), (diag, -diag), (-diag, -diag),
        (big, 0.0), (-big, 0.0), (0.0, big), (0.0, -big),
    ]
}

fn center(nl: &NodeLayout) -> (f64, f64) {
    (nl.x + nl.width / 2.0, nl.y + nl.height / 2.0)
}

/// Normalizes a node pair so `extra_clearance` lookups/inserts don't care which side
/// is `a`/`b` — `refine`'s caller may build the map from either edge direction.
fn normalized_pair(a: NodeIndex, b: NodeIndex) -> (NodeIndex, NodeIndex) {
    if a.index() <= b.index() { (a, b) } else { (b, a) }
}

/// Inserts (or accumulates, via `max` — several independent bottlenecks can name the
/// same pair) an extra-clearance request into a map `refine` will read. The public
/// entry point for building `refine`'s `extra_clearance` argument, so a caller building
/// it from `review::RoutingBottleneck`s never has to reason about pair ordering itself.
pub fn request_extra_clearance(
    map: &mut HashMap<(NodeIndex, NodeIndex), f64>,
    a: NodeIndex,
    b: NodeIndex,
    extra: f64,
) {
    let entry = map.entry(normalized_pair(a, b)).or_insert(0.0);
    *entry = entry.max(extra);
}

fn extra_clearance_for(extra_clearance: &HashMap<(NodeIndex, NodeIndex), f64>, a: NodeIndex, b: NodeIndex) -> f64 {
    extra_clearance.get(&normalized_pair(a, b)).copied().unwrap_or(0.0)
}

/// Minimum gap two boxes should keep apart, proportional to their own average size
/// (see `DesignTokens::node_clearance_fraction`) and floored by
/// `DesignTokens::overlap_clearance()` so small boxes still get a sane minimum —
/// bigger components get proportionally more breathing room instead of every pair
/// sharing one flat pixel gap regardless of how large they actually are. `extra` folds
/// in any [`request_extra_clearance`] request a routing-feedback pass made for this
/// specific pair, on top of that proportional baseline — 0.0 (today's exact behavior)
/// for every pair nothing asked anything extra of.
fn min_clearance(a: &NodeLayout, b: &NodeLayout, tokens: &DesignTokens, extra: f64) -> f64 {
    let scale = (a.width.min(a.height) + b.width.min(b.height)) * 0.5;
    // Also floored at `min_rank_gap()`: two boxes any closer than two clearance stubs
    // can't fit an edge's exit stub *and* entry stub between them, which is what left
    // dense force-directed layouts with arrows a couple of pixels long.
    (scale * tokens.node_clearance_fraction)
        .max(tokens.overlap_clearance())
        .max(tokens.min_rank_gap())
        + extra
}

/// AABB "overlap" area after inflating both boxes by half of [`min_clearance`] on
/// every side — equivalently, positive whenever the two boxes are closer than
/// `min_clearance` apart (which includes genuine intersection, just inflated
/// further). This is what actually gives `refine` a gradient to push boxes apart
/// until they have real visual breathing room, not just until they stop literally
/// intersecting: with a plain (non-inflated) overlap test, two boxes that end up
/// exactly flush — touching but not overlapping — score identically to two boxes
/// with a huge gap between them (both zero), so nothing in the cost function ever
/// preferred the gap. Confirmed directly on a real diagram: several node pairs in a
/// force-directed layout settled edge-to-edge, with an edge label crammed into the
/// shared boundary, despite the layout reporting zero overlaps.
fn overlap_area(a: &NodeLayout, b: &NodeLayout, tokens: &DesignTokens, extra: f64) -> f64 {
    let clearance = min_clearance(a, b, tokens, extra);
    let ox = (a.x + a.width).min(b.x + b.width) - a.x.max(b.x) + clearance;
    let oy = (a.y + a.height).min(b.y + b.height) - a.y.max(b.y) + clearance;
    if ox > 0.0 && oy > 0.0 {
        ox * oy
    } else {
        0.0
    }
}

// `positions` is a `HashMap`, whose iteration order is randomized per *process*
// (Rust's default hasher seeds itself from the OS on each run) — summing floating
// point contributions in that order, as both `node_cost` and
// `overlap_resolution_push` do below, is not associative, so two runs of the exact
// same binary on the exact same YAML could silently converge to measurably different
// final coordinates purely from HashMap iteration order, never from anything in the
// diagram itself. This broke `rdg`'s core "same input always produces the same
// output" guarantee (confirmed directly: 3 back-to-back runs of the same command on
// a disconnected-components sample produced 2 distinct outputs). Both functions take
// `order` — the caller's already-sorted-by-`NodeIndex` node list — and iterate that
// instead of the map directly, so the summation order is fixed regardless of hash
// seed.
fn node_cost(
    idx: NodeIndex,
    positions: &HashMap<NodeIndex, NodeLayout>,
    compiled: &CompiledGraph,
    order: &[NodeIndex],
    tokens: &DesignTokens,
    extra_clearance: &HashMap<(NodeIndex, NodeIndex), f64>,
) -> f64 {
    let nl = &positions[&idx];
    let (cx, cy) = center(nl);
    let mut cost = 0.0;

    for edge in compiled.graph.edges_directed(idx, Direction::Outgoing) {
        if let Some(onl) = positions.get(&edge.target()) {
            let (ox, oy) = center(onl);
            cost += ((cx - ox).powi(2) + (cy - oy).powi(2)).sqrt();
        }
    }
    for edge in compiled.graph.edges_directed(idx, Direction::Incoming) {
        if let Some(onl) = positions.get(&edge.source()) {
            let (ox, oy) = center(onl);
            cost += ((cx - ox).powi(2) + (cy - oy).powi(2)).sqrt();
        }
    }

    for &other in order {
        if other != idx {
            let extra = extra_clearance_for(extra_clearance, idx, other);
            cost += overlap_area(nl, &positions[&other], tokens, extra) * OVERLAP_PENALTY_PER_AREA;
        }
    }

    cost
}

/// The single-shot separating-axis push that would clear all of `idx`'s current AABB
/// overlaps in one move: for each overlapping neighbor, push away along whichever axis
/// has the smaller penetration depth (standard SAT resolution), then sum those pushes.
/// [`CANDIDATE_OFFSETS`]' fixed small/medium nudges converge fine once a node is
/// merely *close* to another box, but a node buried deep inside one (the common case
/// coming out of the force/fCoSE point-mass simulation, which has no notion of box
/// extents at all) needs a move sized to the actual penetration, not a handful of
/// 14-40px steps — this is that move, offered as one more candidate alongside the
/// fixed ones so `refine` still only ever takes a strictly cost-reducing step.
fn overlap_resolution_push(
    idx: NodeIndex,
    positions: &HashMap<NodeIndex, NodeLayout>,
    tokens: &DesignTokens,
    order: &[NodeIndex],
    extra_clearance: &HashMap<(NodeIndex, NodeIndex), f64>,
) -> (f64, f64) {
    let nl = &positions[&idx];
    let (mut push_x, mut push_y) = (0.0, 0.0);

    for &other in order {
        let onl = &positions[&other];
        if other == idx {
            continue;
        }
        // Same clearance-inflated test as `overlap_area` — a `min_clearance`-sized
        // margin on top of the exact penetration depth (or the exact shortfall, for
        // two boxes that are merely close but not literally overlapping) so the
        // result has genuine visual breathing room instead of merely clearing a
        // strict-inequality overlap check by a hairline. See `overlap_area`'s doc
        // comment for the real diagram that surfaced the gap between those two.
        let clearance = min_clearance(nl, onl, tokens, extra_clearance_for(extra_clearance, idx, other));
        let ox = (nl.x + nl.width).min(onl.x + onl.width) - nl.x.max(onl.x) + clearance;
        let oy = (nl.y + nl.height).min(onl.y + onl.height) - nl.y.max(onl.y) + clearance;
        if ox <= 0.0 || oy <= 0.0 {
            continue;
        }
        let (cx, cy) = center(nl);
        let (ocx, ocy) = center(onl);
        if ox < oy {
            push_x += ox * if cx >= ocx { 1.0 } else { -1.0 };
        } else {
            push_y += oy * if cy >= ocy { 1.0 } else { -1.0 };
        }
    }

    (push_x, push_y)
}

/// Refine `positions` in place — called by `force.rs`/`fcose.rs` after their own
/// simulation with an empty `extra_clearance` (today's exact behavior), and by
/// `rdg-render-core::review`'s incremental feedback pass (via the `refine_positions`
/// re-export) with a populated one. Only below [`MAX_NODES`]. `extra_clearance` (build
/// with [`request_extra_clearance`]) folds in any routing-feedback bottleneck requests
/// for specific node pairs; this is the "push these two nodes apart" half of the
/// layout↔routing feedback loop, acting on `positions` in place from wherever they
/// already are rather than a full re-layout, which is what keeps a nudge pass small and
/// "incremental" (preserving the diagram's overall shape) instead of reshuffling
/// everything.
pub fn refine(
    positions: &mut HashMap<NodeIndex, NodeLayout>,
    compiled: &CompiledGraph,
    tokens: &DesignTokens,
    extra_clearance: &HashMap<(NodeIndex, NodeIndex), f64>,
) {
    if positions.len() > MAX_NODES {
        return;
    }

    let mut order: Vec<NodeIndex> = compiled.graph.node_indices().collect();
    order.sort_by_key(|n| n.index());
    let offsets = candidate_offsets(tokens);

    for _ in 0..tokens.max_hillclimb_passes {
        let mut improved = false;

        for &idx in &order {
            let current = node_cost(idx, positions, compiled, &order, tokens, extra_clearance);
            let mut best = (current, 0.0_f64, 0.0_f64);

            let push = overlap_resolution_push(idx, positions, tokens, &order, extra_clearance);
            let mut candidates: Vec<(f64, f64)> = offsets.to_vec();
            if push.0 != 0.0 || push.1 != 0.0 {
                candidates.push(push);
            }

            for (dx, dy) in candidates {
                if let Some(nl) = positions.get_mut(&idx) {
                    nl.x += dx;
                    nl.y += dy;
                }
                let candidate_cost = node_cost(idx, positions, compiled, &order, tokens, extra_clearance);
                if let Some(nl) = positions.get_mut(&idx) {
                    nl.x -= dx;
                    nl.y -= dy;
                }
                if candidate_cost < best.0 {
                    best = (candidate_cost, dx, dy);
                }
            }

            if best.1 != 0.0 || best.2 != 0.0 {
                if let Some(nl) = positions.get_mut(&idx) {
                    nl.x += best.1;
                    nl.y += best.2;
                }
                improved = true;
            }
        }

        if !improved {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rdg_graph::build_graph;
    use rdg_schema::DiagramPayload;

    fn total_cost(
        positions: &HashMap<NodeIndex, NodeLayout>,
        compiled: &CompiledGraph,
        tokens: &DesignTokens,
    ) -> f64 {
        let mut order: Vec<NodeIndex> = positions.keys().copied().collect();
        order.sort_by_key(|n| n.index());
        let no_extra = HashMap::new();
        order.iter().map(|&idx| node_cost(idx, positions, compiled, &order, tokens, &no_extra)).sum()
    }

    #[test]
    fn test_refine_resolves_a_forced_overlap() {
        let payload = DiagramPayload::from_yaml(
            "nodes:\n  - id: a\n    label: A\n  - id: b\n    label: B\nedges:\n  - from: a\n    to: b\n",
        )
        .unwrap();
        let compiled = build_graph(&payload).unwrap();
        let a = compiled.node_map["a"];
        let b = compiled.node_map["b"];
        let mut positions = HashMap::new();
        positions.insert(a, NodeLayout { x: 0.0, y: 0.0, width: 100.0, height: 40.0 });
        positions.insert(b, NodeLayout { x: 10.0, y: 5.0, width: 100.0, height: 40.0 });

        let tokens = DesignTokens::default();
        assert!(overlap_area(&positions[&a], &positions[&b], &tokens, 0.0) > 0.0);
        refine(&mut positions, &compiled, &tokens, &HashMap::new());
        assert!(
            overlap_area(&positions[&a], &positions[&b], &tokens, 0.0) == 0.0,
            "overlap should be resolved: {:?} vs {:?}",
            positions[&a],
            positions[&b]
        );
    }

    #[test]
    fn test_refine_never_increases_total_cost() {
        let payload = DiagramPayload::from_yaml(
            "nodes:\n  - id: a\n    label: A\n  - id: b\n    label: B\n  - id: c\n    label: C\nedges:\n  - from: a\n    to: b\n  - from: b\n    to: c\n",
        )
        .unwrap();
        let compiled = build_graph(&payload).unwrap();
        let mut positions = HashMap::new();
        positions.insert(compiled.node_map["a"], NodeLayout { x: 0.0, y: 0.0, width: 100.0, height: 40.0 });
        positions.insert(compiled.node_map["b"], NodeLayout { x: 300.0, y: 300.0, width: 100.0, height: 40.0 });
        positions.insert(compiled.node_map["c"], NodeLayout { x: 50.0, y: 500.0, width: 100.0, height: 40.0 });

        let tokens = DesignTokens::default();
        let before = total_cost(&positions, &compiled, &tokens);
        refine(&mut positions, &compiled, &tokens, &HashMap::new());
        let after = total_cost(&positions, &compiled, &tokens);
        assert!(after <= before, "cost regressed: {before} -> {after}");
    }

    #[test]
    fn test_extra_clearance_reaches_overlap_area() {
        // Two boxes a little wider apart than the baseline `min_clearance` (the
        // two-stub `min_rank_gap` floor for these box sizes) — no `extra` requested, so
        // `overlap_area` is zero; a
        // large `extra` request for the same pair should push the inflated-clearance
        // test back into "overlap" territory. This isolates the plumbing (does `extra`
        // actually reach the clearance calculation) from `refine`'s search dynamics.
        let a = NodeLayout { x: 0.0, y: 0.0, width: 100.0, height: 40.0 };
        let tokens = DesignTokens::default();
        let b = NodeLayout { x: 100.0 + tokens.min_rank_gap() + 5.0, y: 0.0, width: 100.0, height: 40.0 };
        assert_eq!(overlap_area(&a, &b, &tokens, 0.0), 0.0, "gap above the floor should already clear the baseline");
        assert!(
            overlap_area(&a, &b, &tokens, tokens.px(20.0)) > 0.0,
            "a large extra-clearance request should turn the same gap into an overlap"
        );
    }

    #[test]
    fn test_refine_with_extra_clearance_settles_further_apart_than_baseline() {
        // Two *unconnected* nodes (no edge, so no attraction term competing in
        // `node_cost` — isolates the overlap-avoidance behavior) forced to overlap by
        // the same amount in both cases; the run with an explicit `extra_clearance`
        // request between them must settle with more daylight between the boxes than
        // the unconstrained run, proving the feedback map reaches `refine`'s actual
        // search, not just the cost function in isolation.
        let payload = DiagramPayload::from_yaml(
            "nodes:\n  - id: a\n    label: A\n  - id: b\n    label: B\n",
        )
        .unwrap();
        let compiled = build_graph(&payload).unwrap();
        let a = compiled.node_map["a"];
        let b = compiled.node_map["b"];
        let tokens = DesignTokens::default();

        let mut baseline_positions = HashMap::new();
        baseline_positions.insert(a, NodeLayout { x: 0.0, y: 0.0, width: 100.0, height: 40.0 });
        baseline_positions.insert(b, NodeLayout { x: 10.0, y: 5.0, width: 100.0, height: 40.0 });
        refine(&mut baseline_positions, &compiled, &tokens, &HashMap::new());
        let baseline_dist = {
            let (ca, cb) = (center(&baseline_positions[&a]), center(&baseline_positions[&b]));
            ((ca.0 - cb.0).powi(2) + (ca.1 - cb.1).powi(2)).sqrt()
        };

        let mut nudged_positions = HashMap::new();
        nudged_positions.insert(a, NodeLayout { x: 0.0, y: 0.0, width: 100.0, height: 40.0 });
        nudged_positions.insert(b, NodeLayout { x: 10.0, y: 5.0, width: 100.0, height: 40.0 });
        let mut extra_clearance = HashMap::new();
        request_extra_clearance(&mut extra_clearance, a, b, tokens.px(20.0));
        refine(&mut nudged_positions, &compiled, &tokens, &extra_clearance);
        let nudged_dist = {
            let (ca, cb) = (center(&nudged_positions[&a]), center(&nudged_positions[&b]));
            ((ca.0 - cb.0).powi(2) + (ca.1 - cb.1).powi(2)).sqrt()
        };

        assert!(
            nudged_dist > baseline_dist,
            "extra_clearance should settle further apart (center distance): baseline {baseline_dist} vs nudged {nudged_dist}"
        );
    }

    #[test]
    fn test_refine_skips_above_max_nodes() {
        // MAX_NODES + 1 trivial nodes, plus one deliberately overlapping pair. Below
        // the cap, `refine` resolves that overlap (proven by the earlier test); above
        // it, it must return immediately and leave the overlap untouched.
        let mut yaml = String::from("nodes:\n");
        for i in 0..=MAX_NODES {
            yaml.push_str(&format!("  - id: n{i}\n    label: N{i}\n"));
        }
        let payload = DiagramPayload::from_yaml(&yaml).unwrap();
        let compiled = build_graph(&payload).unwrap();

        let mut positions: HashMap<NodeIndex, NodeLayout> = compiled
            .graph
            .node_indices()
            .enumerate()
            .map(|(i, idx)| (idx, NodeLayout { x: 1000.0 + i as f64, y: 1000.0, width: 10.0, height: 10.0 }))
            .collect();
        let a = compiled.node_map["n0"];
        let b = compiled.node_map["n1"];
        positions.insert(a, NodeLayout { x: 0.0, y: 0.0, width: 100.0, height: 40.0 });
        positions.insert(b, NodeLayout { x: 10.0, y: 5.0, width: 100.0, height: 40.0 });
        let tokens = DesignTokens::default();
        assert!(overlap_area(&positions[&a], &positions[&b], &tokens, 0.0) > 0.0);

        refine(&mut positions, &compiled, &tokens, &HashMap::new());

        assert!(
            overlap_area(&positions[&a], &positions[&b], &tokens, 0.0) > 0.0,
            "above MAX_NODES, refine should be a no-op"
        );
    }
}
