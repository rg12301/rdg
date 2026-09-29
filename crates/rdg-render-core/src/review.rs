//! Deterministic self-review: detects geometric anomalies in a computed layout (overlaps,
//! edges cutting through unrelated nodes, labels too big for their box, a canvas so
//! lopsided it can't be a sane diagram) and retries layout with wider spacing until they
//! clear or a retry budget runs out.
//!
//! This is not an LLM-based review — `rdg` stays deterministic and offline. It's a
//! geometry linter with an auto-fix loop, in the same spirit as the FAS cycle-breaker: a
//! mechanical safety net under the layout algorithm, not a second opinion from a model.

use std::collections::HashMap;

use anyhow::Result;
use petgraph::stable_graph::{EdgeIndex, NodeIndex};

use rdg_dispatch::{AlgorithmDecision, LayoutFramework};
use rdg_graph::CompiledGraph;
use rdg_layout::{
    DesignTokens, LayoutConfig, LayoutResult, NodeLayout, compute_fcose_layout, compute_force_layout,
    compute_layout, compute_sequence_layout, refine_positions, request_extra_clearance,
};

use crate::routing::{
    EdgeRoutingPlan, ObstacleRect, edge_full_path, first_clipping_segment, plan_all_edge_routes, port_point,
};


/// The kind of geometric problem an [`Anomaly`] describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnomalyKind {
    /// Two node boxes overlap.
    NodeOverlap,
    /// An edge's routed path cuts through a node it doesn't connect to.
    EdgeThroughNode,
    /// A node's content needs more room than its (usually explicitly overridden) box gives it.
    LabelOverflow,
    /// The overall canvas is too far from a sane aspect ratio to read as a diagram.
    ExtremeAspectRatio,
    /// A shared routing corridor carries more parallel edges than can read cleanly.
    CongestedCorridor,
    /// An edge has far more turns than its face-pair geometrically needs.
    ExcessiveBend,
    /// An edge's path doesn't leave/enter its own endpoint from outside the face it is
    /// attached to — e.g. it arrives at a face from the far side, crossing the node's body.
    EdgeThroughOwnNode,
}

/// A routing bottleneck a layout retry can act on directly — unlike [`Anomaly`] (a
/// human-readable report line), this names the specific node pair a fix should push
/// apart and by how much.
#[derive(Debug, Clone)]
pub struct RoutingBottleneck {
    pub node_a: NodeIndex,
    pub node_b: NodeIndex,
    /// Extra clearance, beyond the normal minimum, this bottleneck is requesting
    /// between `node_a` and `node_b`, in px.
    pub extra_gap: f64,
}

/// A single detected geometric anomaly.
#[derive(Debug, Clone)]
pub struct Anomaly {
    pub kind: AnomalyKind,
    pub description: String,
}

fn rects_overlap(a: &NodeLayout, b: &NodeLayout) -> bool {
    a.x < b.x + b.width && a.x + a.width > b.x && a.y < b.y + b.height && a.y + a.height > b.y
}

/// Runs every geometric check against a computed `layout`/`edge_plans` pair.
pub fn detect_anomalies(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    edge_plans: &HashMap<EdgeIndex, EdgeRoutingPlan>,
    canvas_w: f64,
    canvas_h: f64,
    tokens: &DesignTokens,
) -> Vec<Anomaly> {
    let mut anomalies = Vec::new();

    // Sequence diagrams have their own lifeline/message geometry with different invariants
    // (participants are deliberately laid out in a single row) — none of these checks apply.
    if layout.sequence_info.is_some() {
        return anomalies;
    }

    detect_node_overlaps(compiled, layout, &mut anomalies);
    detect_edges_through_nodes(compiled, layout, edge_plans, &mut anomalies);
    detect_label_overflow(compiled, layout, &mut anomalies, tokens);
    detect_extreme_aspect_ratio(canvas_w, canvas_h, &mut anomalies, tokens);
    detect_congested_corridor(compiled, edge_plans, &mut anomalies, tokens);
    detect_overlapping_edge_paths(compiled, layout, edge_plans, &mut anomalies);
    detect_excessive_bend(compiled, layout, edge_plans, &mut anomalies, tokens);
    detect_edges_through_own_nodes(compiled, layout, edge_plans, &mut anomalies);

    anomalies
}

/// Resolves an edge to its (possibly cycle-reversed) endpoint node ids, for anomaly
/// descriptions — mirrors `routing::resolve_edge_layout`'s reversal handling without
/// needing the laid-out boxes it also returns.
fn edge_endpoint_ids(compiled: &CompiledGraph, edge_idx: EdgeIndex) -> Option<(NodeIndex, NodeIndex)> {
    let (src, dst) = compiled.graph.edge_endpoints(edge_idx)?;
    let edge_data = &compiled.graph[edge_idx];
    Some(if edge_data.reversed { (dst, src) } else { (src, dst) })
}

/// The minimum waypoint count a direct Manhattan path between `plan`'s two faces
/// needs, given where `src_nl`/`dst_nl` actually sit. Zero only for the two face-pairs
/// *and* actually aligned along their shared axis (`Side::Bottom`→`Side::Top` with
/// matching x, `Side::Right`→`Side::Left` with matching y — see
/// `ideal_waypoints_for_faces`'s same-column/same-row branches, the only cases that
/// return no intermediate points at all); two for every other pairing, or an aligned
/// pair whose ports just don't happen to land on the same line, which always needs at
/// least an entry and exit stub corner.
fn min_bend_waypoints(plan: &EdgeRoutingPlan, src_nl: &NodeLayout, dst_nl: &NodeLayout) -> usize {
    use crate::routing::{Side, port_point};
    match (plan.src_side, plan.dst_side) {
        (Side::Bottom, Side::Top) => {
            let (x1, _) = port_point(src_nl, plan.src_side, plan.exit_port);
            let (x2, _) = port_point(dst_nl, plan.dst_side, plan.entry_port);
            if (x1 - x2).abs() < 1.5 { 0 } else { 2 }
        }
        (Side::Right, Side::Left) => {
            let (_, y1) = port_point(src_nl, plan.src_side, plan.exit_port);
            let (_, y2) = port_point(dst_nl, plan.dst_side, plan.entry_port);
            if (y1 - y2).abs() < 1.5 { 0 } else { 2 }
        }
        _ => 2,
    }
}

/// Minimum collinear-overlap length, in px, before two independently-routed edges'
/// segments are treated as "drawn on top of each other" rather than a coincidental
/// hairline touch — the same order of magnitude as a stub clearance, since anything
/// shorter isn't going to read as an overlap at normal zoom.
const OVERLAP_SEGMENT_MIN_LEN: f64 = 12.0;

/// Above this many edges, the O(E² · segments²) all-pairs scan in
/// `detect_overlapping_edge_paths` is skipped — a safety valve for very large diagrams,
/// not a design value tied to any particular diagram size.
const OVERLAP_SCAN_MAX_EDGES: usize = 400;

/// `Some(true)` if the segment is vertical (within `tol`), `Some(false)` if horizontal,
/// `None` if it's neither (a diagonal segment shouldn't occur in orthogonal routing,
/// but this stays total rather than panicking if one ever does).
fn segment_orientation(a: (f64, f64), b: (f64, f64), tol: f64) -> Option<bool> {
    if (a.0 - b.0).abs() < tol {
        Some(true)
    } else if (a.1 - b.1).abs() < tol {
        Some(false)
    } else {
        None
    }
}

/// How far two segments overlap when they're collinear (both vertical at nearly the
/// same x, or both horizontal at nearly the same y) — zero when they aren't collinear,
/// or are collinear but don't share any span. This is what actually distinguishes two
/// edges that happen to cross at a point (fine, ordinary orthogonal routing) from two
/// edges whose paths run on top of each other for a real stretch (the signature of
/// independently-routed edges — most often two A*-routed ones, which have no awareness
/// of each other's chosen path — visually merging into a single thick, illegible line).
fn collinear_overlap_len(a0: (f64, f64), a1: (f64, f64), b0: (f64, f64), b1: (f64, f64), tol: f64) -> f64 {
    match (segment_orientation(a0, a1, tol), segment_orientation(b0, b1, tol)) {
        (Some(true), Some(true)) if (a0.0 - b0.0).abs() < tol => {
            let (a_lo, a_hi) = (a0.1.min(a1.1), a0.1.max(a1.1));
            let (b_lo, b_hi) = (b0.1.min(b1.1), b0.1.max(b1.1));
            (a_hi.min(b_hi) - a_lo.max(b_lo)).max(0.0)
        }
        (Some(false), Some(false)) if (a0.1 - b0.1).abs() < tol => {
            let (a_lo, a_hi) = (a0.0.min(a1.0), a0.0.max(a1.0));
            let (b_lo, b_hi) = (b0.0.min(b1.0), b0.0.max(b1.0));
            (a_hi.min(b_hi) - a_lo.max(b_lo)).max(0.0)
        }
        _ => 0.0,
    }
}

/// Finds every pair of *different* edges whose routed paths run collinear-overlapping
/// (see [`collinear_overlap_len`]) for at least [`OVERLAP_SEGMENT_MIN_LEN`], returning
/// `(edge_a, edge_b, worst_overlap_px)` — the gap [`detect_congested_corridor`] leaves
/// uncaught: that check only sees edges Step 4 deliberately grouped into one shared
/// corridor bucket (the `CornerHeuristic` router's own bookkeeping), while the
/// `VisibilityGraphAStar` router picks each edge's path independently with no such
/// bookkeeping at all, so two of its edges can end up drawing on top of each other in
/// open space without either ever being flagged. Shared by [`detect_overlapping_edge_paths`]
/// (reports it) and [`collect_routing_bottlenecks`] (acts on it) so the scan itself
/// isn't duplicated.
fn find_overlapping_edge_pairs(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    edge_plans: &HashMap<EdgeIndex, EdgeRoutingPlan>,
) -> Vec<(EdgeIndex, EdgeIndex, f64)> {
    if edge_plans.len() > OVERLAP_SCAN_MAX_EDGES {
        return Vec::new();
    }
    let tol = 1.5;
    let paths: Vec<(EdgeIndex, Vec<(f64, f64)>)> = edge_plans
        .iter()
        .filter_map(|(&edge_idx, plan)| edge_full_path(compiled, layout, edge_idx, plan).map(|p| (edge_idx, p)))
        .collect();

    let mut pairs = Vec::new();
    for i in 0..paths.len() {
        for j in (i + 1)..paths.len() {
            let (edge_a, path_a) = &paths[i];
            let (edge_b, path_b) = &paths[j];
            let mut worst = 0.0_f64;
            for wa in path_a.windows(2) {
                for wb in path_b.windows(2) {
                    worst = worst.max(collinear_overlap_len(wa[0], wa[1], wb[0], wb[1], tol));
                }
            }
            if worst >= OVERLAP_SEGMENT_MIN_LEN {
                pairs.push((*edge_a, *edge_b, worst));
            }
        }
    }
    pairs
}

/// Reports [`find_overlapping_edge_pairs`]' findings as [`Anomaly`]s. Reuses
/// [`AnomalyKind::CongestedCorridor`] — a real, unbucketed overlap is the same reader-
/// facing problem as a bucketed one, just a different reason it happened.
fn detect_overlapping_edge_paths(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    edge_plans: &HashMap<EdgeIndex, EdgeRoutingPlan>,
    out: &mut Vec<Anomaly>,
) {
    for (edge_a, edge_b, worst) in find_overlapping_edge_pairs(compiled, layout, edge_plans) {
        let Some((sa, da)) = edge_endpoint_ids(compiled, edge_a) else { continue };
        let Some((sb, db)) = edge_endpoint_ids(compiled, edge_b) else { continue };
        out.push(Anomaly {
            kind: AnomalyKind::CongestedCorridor,
            description: format!(
                "edges '{}' -> '{}' and '{}' -> '{}' overlap for {worst:.0}px",
                compiled.graph[sa].id, compiled.graph[da].id, compiled.graph[sb].id, compiled.graph[db].id
            ),
        });
    }
}

/// Flags any edge whose routed path shares its Step-4 corridor bucket (see
/// `EdgeRoutingPlan::corridor_bucket_size`) with more than
/// `tokens.congested_corridor_threshold` siblings — a shared channel with that many
/// parallel lines packed into it reads as a tangle no matter how cleanly each
/// individual edge is routed.
fn detect_congested_corridor(
    compiled: &CompiledGraph,
    edge_plans: &HashMap<EdgeIndex, EdgeRoutingPlan>,
    out: &mut Vec<Anomaly>,
    tokens: &DesignTokens,
) {
    for (&edge_idx, plan) in edge_plans {
        if plan.corridor_bucket_size <= tokens.congested_corridor_threshold {
            continue;
        }
        let Some((s_idx, d_idx)) = edge_endpoint_ids(compiled, edge_idx) else {
            continue;
        };
        let src_id = &compiled.graph[s_idx].id;
        let dst_id = &compiled.graph[d_idx].id;
        out.push(Anomaly {
            kind: AnomalyKind::CongestedCorridor,
            description: format!(
                "edge '{src_id}' -> '{dst_id}' shares a routing corridor with {} other edge(s)",
                plan.corridor_bucket_size - 1
            ),
        });
    }
}

/// Flags any edge whose waypoint count exceeds what its face-pair geometrically needs
/// (see [`min_bend_waypoints`]) by more than `tokens.excessive_bend_slack` — a sign the
/// route had to zigzag around obstacles/other edges rather than take a clean path.
fn detect_excessive_bend(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    edge_plans: &HashMap<EdgeIndex, EdgeRoutingPlan>,
    out: &mut Vec<Anomaly>,
    tokens: &DesignTokens,
) {
    let title_zones = crate::routing::compute_group_title_zones(compiled, layout, tokens);
    for (&edge_idx, plan) in edge_plans {
        // The search router already takes the fewest bends the obstacles allow; a bend
        // count above this naive face-pair estimate there means a node is in the way,
        // which wider spacing doesn't remove.
        if plan.searched {
            continue;
        }
        let Some((s_idx, d_idx)) = edge_endpoint_ids(compiled, edge_idx) else {
            continue;
        };
        let (Some(src_nl), Some(dst_nl)) =
            (layout.positions.get(&s_idx), layout.positions.get(&d_idx))
        else {
            continue;
        };
        let min_bends = min_bend_waypoints(plan, src_nl, dst_nl);
        // Stepping around a group title costs up to two extra bends and, unlike a
        // node in the way, can't be cleared by widening spacing (the title has a fixed
        // size) — so a title sitting in the corridor between the two endpoints earns
        // that much extra allowance instead of triggering a pointless retry.
        let (cx0, cx1) = (src_nl.x.min(dst_nl.x), (src_nl.x + src_nl.width).max(dst_nl.x + dst_nl.width));
        let (cy0, cy1) = (src_nl.y.min(dst_nl.y), (src_nl.y + src_nl.height).max(dst_nl.y + dst_nl.height));
        let title_detours = title_zones
            .iter()
            .filter(|z| z.min_x < cx1 && z.max_x > cx0 && z.min_y < cy1 && z.max_y > cy0)
            .count();
        if plan.waypoints.len() <= min_bends + tokens.excessive_bend_slack + 2 * title_detours {
            continue;
        }
        let src_id = &compiled.graph[s_idx].id;
        let dst_id = &compiled.graph[d_idx].id;
        out.push(Anomaly {
            kind: AnomalyKind::ExcessiveBend,
            description: format!(
                "edge '{src_id}' -> '{dst_id}' has {} waypoints (expected around {min_bends})",
                plan.waypoints.len()
            ),
        });
    }
}

/// Flags edges whose path crosses the body of their own source or destination node, or
/// doesn't leave/enter it straight out of its face. `detect_edges_through_nodes` can't see
/// this — it deliberately skips an edge's own endpoints, since the exit/entry stub touches
/// them by design.
fn detect_edges_through_own_nodes(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    edge_plans: &HashMap<EdgeIndex, EdgeRoutingPlan>,
    out: &mut Vec<Anomaly>,
) {
    let rect = |nl: &NodeLayout| ObstacleRect { x: nl.x, y: nl.y, w: nl.width, h: nl.height };
    let mut ids: Vec<&EdgeIndex> = edge_plans.keys().collect();
    ids.sort_by_key(|e| e.index());
    for &edge_idx in ids {
        let plan = &edge_plans[&edge_idx];
        let Some((s_idx, d_idx)) = edge_endpoint_ids(compiled, edge_idx) else { continue };
        if s_idx == d_idx {
            continue;
        }
        let (Some(src_nl), Some(dst_nl)) = (layout.positions.get(&s_idx), layout.positions.get(&d_idx)) else {
            continue;
        };
        let p1 = port_point(src_nl, plan.src_side, plan.exit_port);
        let p2 = port_point(dst_nl, plan.dst_side, plan.entry_port);
        if !crate::routing::path_respects_faces(
            p1,
            plan.src_side,
            &plan.waypoints,
            p2,
            plan.dst_side,
            &rect(src_nl),
            &rect(dst_nl),
        ) {
            out.push(Anomaly {
                kind: AnomalyKind::EdgeThroughOwnNode,
                description: format!(
                    "edge '{}' -> '{}' doesn't leave/enter its endpoint straight out of the attached face ({:?}->{:?}: {})",
                    compiled.graph[s_idx].id,
                    compiled.graph[d_idx].id,
                    plan.src_side,
                    plan.dst_side,
                    std::iter::once(p1)
                        .chain(plan.waypoints.iter().copied())
                        .chain(std::iter::once(p2))
                        .map(|p| format!("({:.0},{:.0})", p.0, p.1))
                        .collect::<String>()
                ),
            });
        }
    }
}

/// Collects the subset of anomalies that name a specific, actionable node pair —
/// [`compute_reviewed_layout`]'s incremental feedback pass consumes this directly
/// instead of re-deriving it from `Anomaly`'s free-text descriptions.
pub fn collect_routing_bottlenecks(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    edge_plans: &HashMap<EdgeIndex, EdgeRoutingPlan>,
    tokens: &DesignTokens,
) -> Vec<RoutingBottleneck> {
    let mut bottlenecks = Vec::new();
    for (&edge_idx, plan) in edge_plans {
        let Some((s_idx, d_idx)) = edge_endpoint_ids(compiled, edge_idx) else {
            continue;
        };
        let (Some(src_nl), Some(dst_nl)) =
            (layout.positions.get(&s_idx), layout.positions.get(&d_idx))
        else {
            continue;
        };
        let congested = plan.corridor_bucket_size > tokens.congested_corridor_threshold;
        let min_bends = min_bend_waypoints(plan, src_nl, dst_nl);
        let bent = plan.waypoints.len() > min_bends + tokens.excessive_bend_slack;
        if !congested && !bent {
            continue;
        }
        // A flat extra push proportional to how far over the threshold this edge is —
        // enough for `hillclimb::refine`'s incremental nudging to actually open the
        // channel up, not so much it fights the rest of the layout in one jump.
        let overshoot = if congested {
            (plan.corridor_bucket_size - tokens.congested_corridor_threshold) as f64
        } else {
            (plan.waypoints.len() - min_bends - tokens.excessive_bend_slack) as f64
        };
        bottlenecks.push(RoutingBottleneck {
            node_a: s_idx,
            node_b: d_idx,
            extra_gap: tokens.px(2.5) * overshoot,
        });
    }

    // Two edges overlapping in open space (see `find_overlapping_edge_pairs`) most
    // often means whatever nodes bound that shared corridor need more room between
    // them — approximated here by requesting extra clearance between the two edges'
    // same-role endpoints (source-to-source, destination-to-destination), the pair
    // most likely to be sitting in the same rank/column actually causing it.
    for (edge_a, edge_b, worst) in find_overlapping_edge_pairs(compiled, layout, edge_plans) {
        let (Some((sa, da)), Some((sb, db))) =
            (edge_endpoint_ids(compiled, edge_a), edge_endpoint_ids(compiled, edge_b))
        else {
            continue;
        };
        let extra_gap = tokens.px(2.5) * (worst / OVERLAP_SEGMENT_MIN_LEN).max(1.0);
        if sa != sb {
            bottlenecks.push(RoutingBottleneck { node_a: sa, node_b: sb, extra_gap });
        }
        if da != db {
            bottlenecks.push(RoutingBottleneck { node_a: da, node_b: db, extra_gap });
        }
    }
    bottlenecks
}

fn detect_node_overlaps(compiled: &CompiledGraph, layout: &LayoutResult, out: &mut Vec<Anomaly>) {
    let entries: Vec<_> = layout.positions.iter().collect();
    for i in 0..entries.len() {
        for j in (i + 1)..entries.len() {
            let (idx_a, a) = entries[i];
            let (idx_b, b) = entries[j];
            if rects_overlap(a, b) {
                let id_a = &compiled.graph[*idx_a].id;
                let id_b = &compiled.graph[*idx_b].id;
                out.push(Anomaly {
                    kind: AnomalyKind::NodeOverlap,
                    description: format!("nodes '{id_a}' and '{id_b}' overlap"),
                });
            }
        }
    }
}

fn detect_edges_through_nodes(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    edge_plans: &HashMap<EdgeIndex, EdgeRoutingPlan>,
    out: &mut Vec<Anomaly>,
) {
    for (&edge_idx, plan) in edge_plans {
        let Some((src, dst)) = compiled.graph.edge_endpoints(edge_idx) else {
            continue;
        };
        let edge_data = &compiled.graph[edge_idx];
        let (s_idx, d_idx) = if edge_data.reversed {
            (dst, src)
        } else {
            (src, dst)
        };
        let (Some(src_nl), Some(dst_nl)) =
            (layout.positions.get(&s_idx), layout.positions.get(&d_idx))
        else {
            continue;
        };
        let p1 = port_point(src_nl, plan.src_side, plan.exit_port);
        let p2 = port_point(dst_nl, plan.dst_side, plan.entry_port);

        for (&other_idx, other_nl) in &layout.positions {
            if other_idx == s_idx || other_idx == d_idx {
                continue;
            }
            let obstacle = ObstacleRect {
                x: other_nl.x,
                y: other_nl.y,
                w: other_nl.width,
                h: other_nl.height,
            };
            if first_clipping_segment(p1, &plan.waypoints, p2, std::slice::from_ref(&obstacle))
                .is_some()
            {
                let src_id = &compiled.graph[s_idx].id;
                let dst_id = &compiled.graph[d_idx].id;
                let through_id = &compiled.graph[other_idx].id;
                out.push(Anomaly {
                    kind: AnomalyKind::EdgeThroughNode,
                    description: format!(
                        "edge '{src_id}' -> '{dst_id}' cuts through node '{through_id}'"
                    ),
                });
            }
        }
    }
}

fn detect_label_overflow(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    out: &mut Vec<Anomaly>,
    tokens: &DesignTokens,
) {
    for (&idx, nl) in &layout.positions {
        let data = &compiled.graph[idx];
        // Only meaningful when the node's box came from an explicit width/height override —
        // otherwise the box *is* the estimate, so it can never "overflow" itself.
        if data.width.is_none() && data.height.is_none() {
            continue;
        }
        // What the content needs with the override lifted — measured from the text as
        // drawn (explicit title/description included) and the icon actually shown.
        let free = rdg_graph::NodeData { width: None, height: None, ..data.clone() };
        let (natural_w, natural_h) = rdg_layout::estimate_node_box(&free, data.icon.is_some(), 0.0, 0.0, tokens);
        if nl.width + 0.5 < natural_w || nl.height + 0.5 < natural_h {
            out.push(Anomaly {
                kind: AnomalyKind::LabelOverflow,
                description: format!(
                    "node '{}' content needs about {natural_w:.0}x{natural_h:.0}px but its box is {:.0}x{:.0}px",
                    data.id, nl.width, nl.height
                ),
            });
        }
    }
}

fn detect_extreme_aspect_ratio(canvas_w: f64, canvas_h: f64, out: &mut Vec<Anomaly>, tokens: &DesignTokens) {
    if canvas_w <= 0.0 || canvas_h <= 0.0 {
        return;
    }
    let ratio = canvas_w / canvas_h;
    if !(tokens.min_aspect_ratio..=tokens.max_aspect_ratio).contains(&ratio) {
        out.push(Anomaly {
            kind: AnomalyKind::ExtremeAspectRatio,
            description: format!("canvas is {canvas_w:.0}x{canvas_h:.0}px (aspect ratio {ratio:.2}), too lopsided to read as a diagram"),
        });
    }
}

/// Estimates final canvas bounds from a computed layout — good enough for the aspect-ratio
/// check, doesn't need to match a renderer's own margin/padding math exactly.
fn estimate_canvas_bounds(layout: &LayoutResult) -> (f64, f64) {
    let mut max_x = 0.0_f64;
    let mut max_y = 0.0_f64;
    for nl in layout.positions.values() {
        max_x = max_x.max(nl.x + nl.width);
        max_y = max_y.max(nl.y + nl.height);
    }
    (max_x, max_y)
}

/// Whitespace measurements for a computed layout/routing — the numbers behind "arrows
/// too short" and "components too close", so spacing work can be judged by measurement
/// instead of by eye. `None` fields mean there was nothing to measure (no edges, one node).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpacingMetrics {
    /// Shortest final segment (the one carrying the arrowhead) over all non-self-loop edges.
    pub min_arrow_len: Option<f64>,
    /// Smallest edge-to-edge distance between any two node boxes (0 when they touch/overlap).
    pub min_node_gap: Option<f64>,
    /// Smallest distance between two ports sharing one face of one node.
    pub min_port_pitch: Option<f64>,
    /// `from -> to` of the edge behind [`Self::min_arrow_len`], for diagnostics.
    pub min_arrow_edge: Option<String>,
    /// `node.face` behind [`Self::min_port_pitch`], for diagnostics.
    pub min_pitch_face: Option<String>,
    /// Interior segments shorter than `polish_min_jog` (see `polish::micro_jog_segments`).
    pub micro_jogs: usize,
    /// Right-angle crossings between different edges.
    pub crossings: usize,
    /// Crossings within 16px of a bend or an end of either edge — where a reader can't
    /// tell which line turns and which carries on.
    pub crossings_near_bend: usize,
    /// Edge segments running parallel within 8px of a group border for over 24px.
    pub border_hugs: usize,
}

fn min_opt(cur: Option<f64>, v: f64) -> Option<f64> {
    Some(cur.map_or(v, |c| c.min(v)))
}

fn rect_gap(a: &NodeLayout, b: &NodeLayout) -> f64 {
    let dx = (a.x - (b.x + b.width)).max(b.x - (a.x + a.width)).max(0.0);
    let dy = (a.y - (b.y + b.height)).max(b.y - (a.y + a.height)).max(0.0);
    dx.hypot(dy)
}

/// Measures the whitespace of a laid-out, routed diagram. Sequence diagrams (lifeline
/// geometry, no routed edges) yield an empty result.
pub fn spacing_metrics(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    edge_plans: &HashMap<EdgeIndex, EdgeRoutingPlan>,
    tokens: &DesignTokens,
) -> SpacingMetrics {
    let mut m = SpacingMetrics::default();
    if layout.sequence_info.is_some() {
        return m;
    }

    let mut ids: Vec<&NodeIndex> = layout.positions.keys().collect();
    ids.sort_by_key(|n| n.index());
    for (i, a) in ids.iter().enumerate() {
        for b in &ids[i + 1..] {
            m.min_node_gap = min_opt(m.min_node_gap, rect_gap(&layout.positions[a], &layout.positions[b]));
        }
    }

    // Micro-jogs and crossings, over every edge's full path in a fixed (EdgeIndex) order.
    let mut edge_ids: Vec<&EdgeIndex> = edge_plans.keys().collect();
    edge_ids.sort_by_key(|e| e.index());
    let paths: Vec<Vec<(f64, f64)>> = edge_ids
        .iter()
        .filter_map(|&&e| edge_full_path(compiled, layout, e, &edge_plans[&e]))
        .collect();
    let min_jog = tokens.polish_min_jog();
    for (i, pa) in paths.iter().enumerate() {
        m.micro_jogs += crate::polish::micro_jog_segments(pa, min_jog).len();
        for pb in &paths[i + 1..] {
            let pts = crate::polish::crossing_points(pa, pb);
            m.crossings += pts.len();
            // Every path point is a bend or an end.
            m.crossings_near_bend += pts
                .iter()
                .filter(|x| pa.iter().chain(pb).any(|c| (c.0 - x.0).abs() + (c.1 - x.1).abs() < 16.0))
                .count();
        }
    }
    for (gx, gy, gw, gh) in rdg_layout::groups::group_rects(compiled, &layout.positions, tokens).into_iter().flatten() {
        let lines = [(true, gy, gx, gx + gw), (true, gy + gh, gx, gx + gw), (false, gx, gy, gy + gh), (false, gx + gw, gy, gy + gh)];
        for p in &paths {
            for w in p.windows(2) {
                let (a, b) = (w[0], w[1]);
                let horiz = (a.1 - b.1).abs() < 0.5;
                for &(lh, at, lo, hi) in &lines {
                    if lh != horiz {
                        continue;
                    }
                    let (c, s0, s1) = if horiz { (a.1, a.0.min(b.0), a.0.max(b.0)) } else { (a.0, a.1.min(b.1), a.1.max(b.1)) };
                    if (c - at).abs() < 8.0 && s1.min(hi) - s0.max(lo) > 24.0 {
                        m.border_hugs += 1;
                    }
                }
            }
        }
    }

    let mut face_ports: HashMap<(NodeIndex, crate::routing::Side), Vec<f64>> = HashMap::new();
    for (&edge_idx, plan) in edge_plans {
        let Some((s_idx, d_idx)) = edge_endpoint_ids(compiled, edge_idx) else { continue };
        if s_idx == d_idx {
            continue;
        }
        if let Some(mut pts) = edge_full_path(compiled, layout, edge_idx, plan) {
            // The arrow as drawn: into the outline (a round shape, a logo's halo), not
            // just to the bounding box.
            if let (Some(last), Some(nl)) = (pts.last_mut(), layout.positions.get(&d_idx)) {
                *last = crate::routing::node_attach_point(compiled, d_idx, nl, plan.dst_side, plan.entry_port, tokens);
            }
            if let [.., a, b] = pts.as_slice() {
                let len = (b.0 - a.0).hypot(b.1 - a.1);
                if m.min_arrow_len.is_none_or(|c| len < c) {
                    let path: Vec<String> = pts.iter().map(|p| format!("({:.0},{:.0})", p.0, p.1)).collect();
                    m.min_arrow_edge = Some(format!(
                        "{} -{:?}->{:?}- {} via {}",
                        compiled.graph[s_idx].id,
                        plan.src_side,
                        plan.dst_side,
                        compiled.graph[d_idx].id,
                        path.join("")
                    ));
                }
                m.min_arrow_len = min_opt(m.min_arrow_len, len);
            }
        }
        face_ports.entry((s_idx, plan.src_side)).or_default().push(plan.exit_port);
        face_ports.entry((d_idx, plan.dst_side)).or_default().push(plan.entry_port);
    }
    for ((node, side), mut fracs) in face_ports {
        let Some(nl) = layout.positions.get(&node) else { continue };
        fracs.sort_by(f64::total_cmp);
        let face_len = match side {
            crate::routing::Side::Top | crate::routing::Side::Bottom => nl.width,
            crate::routing::Side::Left | crate::routing::Side::Right => nl.height,
        };
        for w in fracs.windows(2) {
            let pitch = (w[1] - w[0]) * face_len;
            if m.min_port_pitch.is_none_or(|c| pitch < c) {
                m.min_pitch_face = Some(format!("{}.{:?}", compiled.graph[node].id, side));
            }
            m.min_port_pitch = min_opt(m.min_port_pitch, pitch);
        }
    }
    m
}

/// One layout attempt's outcome, for the stderr diagnostic the CLI prints.
#[derive(Debug, Clone)]
pub struct PassReport {
    pub attempt: u32,
    pub anomaly_count: usize,
    pub rank_spacing: u32,
    pub node_spacing: u32,
}

/// The result of [`compute_reviewed_layout`]: the best layout/routing found, plus a record
/// of every attempt made.
pub struct ReviewedLayout {
    pub layout: LayoutResult,
    pub edge_plans: HashMap<EdgeIndex, EdgeRoutingPlan>,
    pub passes: Vec<PassReport>,
    /// What the final polish pass changed (empty when polish is disabled).
    pub polish: crate::polish::PolishReport,
}

impl ReviewedLayout {
    /// Anomalies remaining in the final chosen layout (empty if a pass fully cleared).
    pub fn remaining_anomalies(&self, compiled: &CompiledGraph, tokens: &DesignTokens) -> Vec<Anomaly> {
        let (w, h) = estimate_canvas_bounds(&self.layout);
        detect_anomalies(compiled, &self.layout, &self.edge_plans, w, h, tokens)
    }
}

/// Computes a layout, plans its edge routes, and checks both for geometric anomalies —
/// retrying with progressively wider spacing (up to `max_passes` attempts) until a pass
/// comes back clean or the budget runs out, in which case the fewest-anomaly attempt seen
/// is returned. Widening spacing mainly helps [`AnomalyKind::NodeOverlap`] and
/// [`AnomalyKind::EdgeThroughNode`] — [`AnomalyKind::LabelOverflow`] (an explicit size
/// override too small for its content) and [`AnomalyKind::ExtremeAspectRatio`] usually
/// need a YAML change instead, so they may legitimately still be present in the returned
/// report even after every retry; that's the report doing its job, not a bug.
///
/// `decision` (from `rdg_dispatch::dispatch`) selects which layout framework and edge
/// router this runs — sequence diagrams are the one exception, keeping their existing
/// unconditional lifeline layout regardless of what `decision` says, exactly as
/// [`rdg_layout::compute_layout`] itself special-cases them. Widening `rank_spacing`/
/// `node_spacing` on retry generalizes across all three frameworks for free: Sugiyama
/// reads them directly, and the force-directed/fCoSE engines derive their ideal edge
/// length from the same two fields, so one retry mechanism serves every framework
/// without needing framework-specific scaling logic.
///
/// # Errors
///
/// Returns an error only if the underlying layout computation itself fails.
pub fn compute_reviewed_layout(
    compiled: &CompiledGraph,
    base_config: &LayoutConfig,
    max_passes: u32,
    decision: &AlgorithmDecision,
) -> Result<ReviewedLayout> {
    let mut config = base_config.clone();
    let mut passes = Vec::new();
    let mut best: Option<(LayoutResult, HashMap<EdgeIndex, EdgeRoutingPlan>, usize)> = None;

    for attempt in 1..=max_passes.max(1) {
        let layout = if compiled.diagram_type == "sequence" {
            compute_sequence_layout(compiled, &config)?
        } else {
            match decision.framework {
                LayoutFramework::Sugiyama => compute_layout(compiled, &config)?,
                LayoutFramework::ForceDirected => compute_force_layout(compiled, &config)?,
                LayoutFramework::FCose => compute_fcose_layout(compiled, &config)?,
            }
        };
        let edge_plans = if layout.sequence_info.is_none() {
            plan_all_edge_routes(compiled, &layout, decision.routing.algorithm, &config.tokens)
        } else {
            HashMap::new()
        };
        let (canvas_w, canvas_h) = estimate_canvas_bounds(&layout);
        let anomalies = detect_anomalies(compiled, &layout, &edge_plans, canvas_w, canvas_h, &config.tokens);
        let count = anomalies.len();
        if std::env::var("RDG_DEBUG_ANOMALIES").is_ok() {
            for a in &anomalies {
                eprintln!("  pass {attempt}: [{:?}] {}", a.kind, a.description);
            }
        }

        // Incremental routing-feedback nudge: force-directed/fCoSE attempts only —
        // `hillclimb::refine` (the incremental local-search nudger this pushes into via
        // `refine_positions`) doesn't run in the Sugiyama path at all (it has its own
        // `straighten_layers` stabilizer instead; see `hillclimb`'s own module doc), so
        // a per-pair clearance request has nothing to act on there. Cheap relative to a
        // full retry pass above: it starts from this attempt's own positions rather
        // than recomputing layout from scratch, which is what keeps it "incremental" —
        // only kept if it strictly reduces this attempt's anomaly count.
        let (layout, edge_plans, count) = if count > 0
            && matches!(decision.framework, LayoutFramework::ForceDirected | LayoutFramework::FCose)
        {
            let bottlenecks = collect_routing_bottlenecks(compiled, &layout, &edge_plans, &config.tokens);
            if bottlenecks.is_empty() {
                (layout, edge_plans, count)
            } else {
                let mut extra_clearance = HashMap::new();
                for b in &bottlenecks {
                    request_extra_clearance(&mut extra_clearance, b.node_a, b.node_b, b.extra_gap);
                }
                let mut nudged_positions = layout.positions.clone();
                refine_positions(&mut nudged_positions, compiled, &config.tokens, &extra_clearance);
                let nudged_layout = LayoutResult { positions: nudged_positions, sequence_info: None };
                let nudged_edge_plans =
                    plan_all_edge_routes(compiled, &nudged_layout, decision.routing.algorithm, &config.tokens);
                let (nw, nh) = estimate_canvas_bounds(&nudged_layout);
                let nudged_count =
                    detect_anomalies(compiled, &nudged_layout, &nudged_edge_plans, nw, nh, &config.tokens).len();
                if nudged_count < count {
                    (nudged_layout, nudged_edge_plans, nudged_count)
                } else {
                    (layout, edge_plans, count)
                }
            }
        } else {
            (layout, edge_plans, count)
        };

        passes.push(PassReport {
            attempt,
            anomaly_count: count,
            rank_spacing: config.rank_spacing,
            node_spacing: config.node_spacing,
        });

        let is_better = best
            .as_ref()
            .is_none_or(|(_, _, best_count)| count < *best_count);
        if is_better {
            best = Some((layout, edge_plans, count));
        }
        if count == 0 {
            break;
        }

        let factor = config.tokens.retry_spacing_factor;
        config.rank_spacing = ((config.rank_spacing as f64) * factor).round() as u32;
        config.node_spacing = ((config.node_spacing as f64) * factor).round() as u32;
    }

    let (mut layout, mut edge_plans, _) = best.expect("the loop always runs at least one pass");
    // Final polish: small guarded port/waypoint adjustments on the chosen attempt. Runs
    // before `finalize_canvas` (which shifts everything onto the margin) so any geometry it
    // touches is covered by that translation, and before anomalies are re-checked so
    // `--strict` judges the polished result.
    let polish = crate::polish::polish(compiled, &mut layout, &mut edge_plans, decision.routing.algorithm, &config.tokens);
    // Whole-pixel geometry, *before* the canvas shift; that shift is itself rounded to whole
    // pixels (see `finalize_canvas`), so it can't undo the snapping.
    crate::polish::snap_to_pixels(compiled, &mut layout, &mut edge_plans, &config.tokens);
    crate::canvas::finalize_canvas(
        compiled,
        &mut layout,
        &mut edge_plans,
        config.margin_x,
        config.margin_y,
        &config.tokens,
    );
    Ok(ReviewedLayout {
        layout,
        edge_plans,
        passes,
        polish,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rdg_graph::build_graph;
    use rdg_layout::NodeLayout as NL;
    use rdg_schema::{DiagramPayload, EdgeDef, NodeDef};
    use std::collections::HashMap as Map;

    fn dispatch_for(compiled: &CompiledGraph) -> AlgorithmDecision {
        let tokens = DesignTokens::default();
        rdg_dispatch::dispatch(&rdg_dispatch::analyze(compiled, &tokens), &tokens)
    }

    fn payload_two_nodes() -> DiagramPayload {
        DiagramPayload {
            nodes: vec![
                NodeDef {
                    id: "a".into(),
                    label: "A".into(),
                    ..Default::default()
                },
                NodeDef {
                    id: "b".into(),
                    label: "B".into(),
                    ..Default::default()
                },
            ],
            edges: vec![EdgeDef {
                from: "a".into(),
                to: "b".into(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn test_no_anomalies_on_a_normal_layout() {
        let payload = payload_two_nodes();
        let compiled = build_graph(&payload).unwrap();
        let decision = dispatch_for(&compiled);
        let reviewed =
            compute_reviewed_layout(&compiled, &LayoutConfig::default(), LayoutConfig::default().tokens.max_review_passes, &decision)
                .unwrap();
        assert!(reviewed.remaining_anomalies(&compiled, &LayoutConfig::default().tokens).is_empty());
        assert_eq!(
            reviewed.passes.len(),
            1,
            "a clean first pass shouldn't retry"
        );
    }

    #[test]
    fn test_detects_forced_node_overlap() {
        let compiled = build_graph(&payload_two_nodes()).unwrap();
        let idx_a = compiled.node_map["a"];
        let idx_b = compiled.node_map["b"];
        let mut positions = Map::new();
        positions.insert(
            idx_a,
            NL {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 50.0,
            },
        );
        positions.insert(
            idx_b,
            NL {
                x: 50.0,
                y: 10.0,
                width: 100.0,
                height: 50.0,
            },
        );
        let layout = LayoutResult {
            positions,
            sequence_info: None,
        };
        let anomalies = detect_anomalies(&compiled, &layout, &Map::new(), 200.0, 100.0, &DesignTokens::default());
        assert!(anomalies.iter().any(|a| a.kind == AnomalyKind::NodeOverlap));
    }

    #[test]
    fn test_detects_label_overflow_only_with_explicit_override() {
        let mut payload = payload_two_nodes();
        payload.nodes[0].width = Some(5.0);
        payload.nodes[0].height = Some(5.0);
        let compiled = build_graph(&payload).unwrap();
        let decision = dispatch_for(&compiled);
        let reviewed = compute_reviewed_layout(&compiled, &LayoutConfig::default(), 1, &decision).unwrap();
        let anomalies = reviewed.remaining_anomalies(&compiled, &LayoutConfig::default().tokens);
        assert!(
            anomalies
                .iter()
                .any(|a| a.kind == AnomalyKind::LabelOverflow)
        );
    }

    #[test]
    fn test_no_label_overflow_without_override() {
        let compiled = build_graph(&payload_two_nodes()).unwrap();
        let decision = dispatch_for(&compiled);
        let reviewed = compute_reviewed_layout(&compiled, &LayoutConfig::default(), 1, &decision).unwrap();
        let anomalies = reviewed.remaining_anomalies(&compiled, &LayoutConfig::default().tokens);
        assert!(
            !anomalies
                .iter()
                .any(|a| a.kind == AnomalyKind::LabelOverflow)
        );
    }

    #[test]
    fn test_extreme_aspect_ratio_flagged() {
        let mut anomalies = Vec::new();
        detect_extreme_aspect_ratio(2000.0, 50.0, &mut anomalies, &DesignTokens::default());
        assert_eq!(anomalies.len(), 1);
        assert_eq!(anomalies[0].kind, AnomalyKind::ExtremeAspectRatio);
    }

    #[test]
    fn test_sane_aspect_ratio_not_flagged() {
        let mut anomalies = Vec::new();
        detect_extreme_aspect_ratio(800.0, 600.0, &mut anomalies, &DesignTokens::default());
        assert!(anomalies.is_empty());
    }

    #[test]
    fn test_edge_through_unrelated_node_detected() {
        // a -> c routed straight through b, which sits directly between them.
        let payload = DiagramPayload {
            nodes: vec![
                NodeDef {
                    id: "a".into(),
                    label: "A".into(),
                    ..Default::default()
                },
                NodeDef {
                    id: "b".into(),
                    label: "B".into(),
                    ..Default::default()
                },
                NodeDef {
                    id: "c".into(),
                    label: "C".into(),
                    ..Default::default()
                },
            ],
            edges: vec![EdgeDef {
                from: "a".into(),
                to: "c".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let compiled = build_graph(&payload).unwrap();
        let idx_a = compiled.node_map["a"];
        let idx_b = compiled.node_map["b"];
        let idx_c = compiled.node_map["c"];
        let mut positions = Map::new();
        positions.insert(
            idx_a,
            NL {
                x: 0.0,
                y: 0.0,
                width: 60.0,
                height: 40.0,
            },
        );
        positions.insert(
            idx_b,
            NL {
                x: 0.0,
                y: 60.0,
                width: 60.0,
                height: 40.0,
            },
        );
        positions.insert(
            idx_c,
            NL {
                x: 0.0,
                y: 120.0,
                width: 60.0,
                height: 40.0,
            },
        );
        let layout = LayoutResult {
            positions,
            sequence_info: None,
        };
        let edge_idx = compiled.edge_order[0];
        let mut edge_plans = Map::new();
        edge_plans.insert(
            edge_idx,
            EdgeRoutingPlan {
                src_side: crate::routing::Side::Bottom,
                dst_side: crate::routing::Side::Top,
                exit_port: 0.5,
                entry_port: 0.5,
                channel_y: 0.0,
                corridor_x: 0.0,
                waypoints: vec![],
                corridor_bucket_size: 1,
                searched: false,
            },
        );
        let anomalies = detect_anomalies(&compiled, &layout, &edge_plans, 60.0, 160.0, &DesignTokens::default());
        assert!(
            anomalies
                .iter()
                .any(|a| a.kind == AnomalyKind::EdgeThroughNode)
        );
    }

    /// A dense, near-mesh graph (same shape as the `social_graph_dense` sample that
    /// surfaced the gap `detect_overlapping_edge_paths` closes) — enough nodes/edges to
    /// dispatch to `ForceDirected` and produce genuinely overlapping A*-routed edges.
    fn dense_mesh_payload() -> DiagramPayload {
        let ids = ["n0", "n1", "n2", "n3", "n4", "n5", "n6", "n7", "n8", "n9", "n10", "n11"];
        let mut yaml = String::from("nodes:\n");
        for id in ids {
            yaml.push_str(&format!("  - id: {id}\n    label: {id}\n"));
        }
        yaml.push_str("edges:\n");
        // Every node to its next 5 neighbors (wrapping) — dense (well above the
        // density_threshold that routes this to ForceDirected) without being a full
        // mesh, so it's a nontrivial routing problem rather than a degenerate one.
        for i in 0..ids.len() {
            for k in 1..=5 {
                let j = (i + k) % ids.len();
                yaml.push_str(&format!("  - {{from: {}, to: {}}}\n", ids[i], ids[j]));
            }
        }
        DiagramPayload::from_yaml(&yaml).unwrap()
    }

    #[test]
    fn test_incremental_nudge_reduces_anomalies_on_congested_force_directed_layout() {
        let compiled = build_graph(&dense_mesh_payload()).unwrap();
        let decision = dispatch_for(&compiled);
        assert_eq!(
            decision.framework,
            LayoutFramework::ForceDirected,
            "fixture should dispatch to ForceDirected for this test to be meaningful"
        );

        // Polish is a separate, later stage that also edits routes; this test isolates the
        // review loop's own incremental nudge, so it runs with polish off.
        let mut config = LayoutConfig::default();
        config.tokens.polish_enabled = false;
        let tokens = config.tokens;

        // The raw first attempt, with no incremental nudge applied — what
        // `compute_reviewed_layout`'s pass 1 would have reported before this
        // milestone's feedback step.
        let raw_layout = compute_force_layout(&compiled, &config).unwrap();
        let raw_edge_plans = plan_all_edge_routes(&compiled, &raw_layout, decision.routing.algorithm, &tokens);
        let (raw_w, raw_h) = estimate_canvas_bounds(&raw_layout);
        let raw_count = detect_anomalies(&compiled, &raw_layout, &raw_edge_plans, raw_w, raw_h, &tokens).len();

        let reviewed = compute_reviewed_layout(&compiled, &config, 1, &decision).unwrap();
        let nudged_count = reviewed.remaining_anomalies(&compiled, &tokens).len();

        assert!(
            nudged_count < raw_count,
            "incremental nudge should reduce anomalies on a single pass: raw {raw_count} vs nudged {nudged_count}"
        );
    }
}
