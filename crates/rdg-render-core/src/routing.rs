//! Obstacle-aware orthogonal edge routing, shared by every render backend.
//!
//! Produces waypoint coordinates and attachment faces/ports; each backend encodes those
//! into its own output (draw.io `mxPoint`s vs an SVG path `d` attribute).

use std::collections::HashMap;

use petgraph::stable_graph::{EdgeIndex, NodeIndex};

use rdg_graph::{CompiledGraph, EdgeData};
use rdg_layout::{DesignTokens, LayoutResult, NodeLayout};

pub use rdg_dispatch::RoutingAlgorithm;

/// Bounding face for node connector port attachment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    Top,
    Bottom,
    Left,
    Right,
}

impl Side {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "top" | "north" | "n" => Some(Side::Top),
            "bottom" | "south" | "s" => Some(Side::Bottom),
            "left" | "west" | "w" => Some(Side::Left),
            "right" | "east" | "e" => Some(Side::Right),
            _ => None,
        }
    }
}

impl std::str::FromStr for Side {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Side::parse(s).ok_or(())
    }
}

/// The absolute `(x, y)` point where a connector attaches to a node's face, at
/// `port_frac` along that face (0.0 = one end, 1.0 = the other).
///
/// Shared by both the routing planner (below) and the SVG backend, which previously
/// each carried their own copy of this exact four-way match.
pub fn port_point(nl: &NodeLayout, side: Side, port_frac: f64) -> (f64, f64) {
    match side {
        Side::Bottom => (nl.x + nl.width * port_frac, nl.y + nl.height),
        Side::Top => (nl.x + nl.width * port_frac, nl.y),
        Side::Left => (nl.x, nl.y + nl.height * port_frac),
        Side::Right => (nl.x + nl.width, nl.y + nl.height * port_frac),
    }
}

/// Where an arrow attached at `port_frac` along `side` actually meets the node's drawn
/// `outline` — the bounding-box point [`port_point`] gives, pushed inward along the face
/// normal onto the curve for round shapes, so arrows touch an ellipse or a cylinder cap
/// instead of stopping in the empty corner of its bounding box.
pub fn attach_point(nl: &NodeLayout, outline: crate::style::Outline, side: Side, port_frac: f64) -> (f64, f64) {
    use crate::style::{MarkShape, Outline};
    let (x, y) = port_point(nl, side, port_frac);
    let (rx, ry) = (nl.width / 2.0, nl.height / 2.0);
    let (cx, cy) = (nl.x + rx, nl.y + ry);
    // Normalised offset along the face from its centre, in [-1, 1].
    let t = match side {
        Side::Top | Side::Bottom => ((x - cx) / rx).clamp(-1.0, 1.0),
        Side::Left | Side::Right => ((y - cy) / ry).clamp(-1.0, 1.0),
    };
    // Inward depth from the bounding box to the outline.
    let depth = match (outline, side) {
        (Outline::Rect, _) => 0.0,
        (Outline::Ellipse, Side::Top | Side::Bottom) => ry * (1.0 - (1.0 - t * t).sqrt()),
        (Outline::Ellipse, _) => rx * (1.0 - (1.0 - t * t).sqrt()),
        (Outline::Diamond, Side::Top | Side::Bottom) => ry * t.abs(),
        (Outline::Diamond, _) => rx * t.abs(),
        (Outline::Cylinder { cap }, Side::Top | Side::Bottom) => cap * (1.0 - (1.0 - t * t).sqrt()),
        (Outline::Cylinder { .. }, _) => 0.0,
        // The mark is centred at the top: the sides and the top meet it (on its curve or
        // slant); the bottom face is under the caption.
        (Outline::Captioned { mark, shape }, Side::Left | Side::Right) => {
            let r = mark / 2.0;
            let dy = (y - (nl.y + r)).abs().min(r);
            let reach = match shape {
                MarkShape::Circle => (r * r - dy * dy).sqrt(),
                MarkShape::Square => r,
                MarkShape::Diamond => r - dy,
            };
            (rx - reach).max(0.0)
        }
        (Outline::Captioned { mark, shape }, Side::Top) => {
            let r = mark / 2.0;
            let dx = (x - cx).abs().min(r);
            match shape {
                MarkShape::Circle => r - (r * r - dx * dx).sqrt(),
                MarkShape::Square => 0.0,
                MarkShape::Diamond => dx,
            }
        }
        // Under the caption: stop short of the text, so the line doesn't grow out of it.
        (Outline::Captioned { .. }, Side::Bottom) => -CAPTION_CLEARANCE,
    };
    match side {
        Side::Top => (x, y + depth),
        Side::Bottom => (x, y - depth),
        Side::Left => (x + depth, y),
        Side::Right => (x - depth, y),
    }
}

/// Air between a captioned node's caption and an arrow leaving or entering under it, px.
pub const CAPTION_CLEARANCE: f64 = 6.0;

/// [`attach_point`] for node `idx` of `compiled`.
pub fn node_attach_point(
    compiled: &CompiledGraph,
    idx: NodeIndex,
    nl: &NodeLayout,
    side: Side,
    port_frac: f64,
    tokens: &DesignTokens,
) -> (f64, f64) {
    attach_point(nl, crate::style::outline_of(&compiled.graph[idx], tokens), side, port_frac)
}

/// How many ports fit on `side` of `nl` while keeping [`DesignTokens::min_port_pitch`]
/// between them, given the widest span ports are spread over (`[0.15, 0.85]`, see
/// `canonical_port_fractions` use in `plan_all_edge_routes`). At least 1.
fn face_capacity(nl: &NodeLayout, side: Side, tokens: &DesignTokens) -> usize {
    let len = match side {
        Side::Top | Side::Bottom => nl.width,
        Side::Left | Side::Right => nl.height,
    };
    // n ports over span S sit S/(n+1) apart, so pitch >= p  <=>  n <= S/p - 1.
    (((len * 0.7) / tokens.min_port_pitch()).floor() as usize).saturating_sub(1).max(1)
}

/// Every node's box as a routing obstacle, sorted by `NodeIndex`. `layout.positions` is a
/// `HashMap` (randomized iteration order per process), and this router is documented as
/// deterministic, so every order-sensitive obstacle scan goes through this sorted list.
pub(crate) fn node_obstacles(layout: &LayoutResult) -> Vec<(NodeIndex, ObstacleRect)> {
    let mut all: Vec<(NodeIndex, ObstacleRect)> = layout
        .positions
        .iter()
        .map(|(&ni, nl)| (ni, ObstacleRect { x: nl.x, y: nl.y, w: nl.width, h: nl.height }))
        .collect();
    all.sort_by_key(|(ni, _)| ni.index());
    all
}

/// Everything [`route_edge`] needs besides the edge itself: the obstacle field, the group
/// title boxes, and which router to use. Shared by the initial planner and the polish pass,
/// so a re-routed edge is routed exactly the way it originally was.
pub(crate) struct RouteEnv<'a> {
    pub all_obstacles: &'a [(NodeIndex, ObstacleRect)],
    pub title_zones: &'a [GroupTitleZone],
    pub algorithm: RoutingAlgorithm,
    pub tokens: &'a DesignTokens,
}

/// Routes one edge between two already-chosen faces and ports and returns its waypoints
/// (excluding the two port points). `channel_y`/`corridor_x` are the shared-corridor hints
/// from planning Steps 4/5.
pub(crate) fn route_edge(
    env: &RouteEnv<'_>,
    (s_idx, src_nl, src_side, exit_port): (NodeIndex, &NodeLayout, Side, f64),
    (d_idx, dst_nl, dst_side, entry_port): (NodeIndex, &NodeLayout, Side, f64),
    channel_y: f64,
    corridor_x: f64,
) -> Vec<(f64, f64)> {
    let tokens = env.tokens;
    let (x1, y1) = port_point(src_nl, src_side, exit_port);
    let (x2, y2) = port_point(dst_nl, dst_side, entry_port);

    // Per-edge obstacle list: exclude source and destination nodes so their face-stubs are
    // not treated as blocked. Group title text is an obstacle too: a line drawn through
    // "Persistent Storage Tier" is as unreadable as one through a node. Only the title's
    // own text box (not the whole container), so edges still cross group borders freely.
    let mut edge_obstacles: Vec<ObstacleRect> = env
        .all_obstacles
        .iter()
        .filter(|(ni, _)| *ni != s_idx && *ni != d_idx)
        .map(|(_, obs)| obs.clone())
        .collect();
    edge_obstacles.extend(env.title_zones.iter().map(|tz| ObstacleRect {
        x: tz.min_x,
        y: tz.min_y,
        w: tz.max_x - tz.min_x,
        h: tz.max_y - tz.min_y,
    }));
    let src_rect = ObstacleRect { x: src_nl.x, y: src_nl.y, w: src_nl.width, h: src_nl.height };
    let dst_rect = ObstacleRect { x: dst_nl.x, y: dst_nl.y, w: dst_nl.width, h: dst_nl.height };

    // A* between the two clearance stubs (not the raw ports): A* itself knows nothing about
    // faces, so routed port-to-port it happily arrives sideways along a face (or with a 1px
    // jog right at the port), giving an arrow with no perpendicular run-in at all.
    // Starting/ending one stub out from each face, then re-attaching the stub, guarantees
    // every edge leaves and enters its face straight for at least `stub_clearance`.
    let astar_route = |obstacles: &[ObstacleRect]| -> Option<Vec<(f64, f64)>> {
        let sc = tokens.stub_clearance();
        let s1 = stub_point((x1, y1), src_side, clear_stub_len((x1, y1), src_side, sc, obstacles));
        let s2 = stub_point((x2, y2), dst_side, clear_stub_len((x2, y2), dst_side, sc, obstacles));
        compute_edge_waypoints_astar(s1, s2, obstacles, tokens).map(|mut inner| {
            inner.insert(0, s1);
            inner.push(s2);
            simplify_orthogonal_polyline(&mut inner);
            inner
        })
    };
    let corner_route = || {
        compute_edge_waypoints_with_obstacles_and_clearance(
            (x1, y1),
            src_side,
            (x2, y2),
            dst_side,
            channel_y,
            corridor_x,
            &edge_obstacles,
            Some(&src_rect),
            Some(&dst_rect),
            tokens,
        )
    };
    let mut waypoints = match env.algorithm {
        RoutingAlgorithm::CornerHeuristic => corner_route(),
        RoutingAlgorithm::VisibilityGraphAStar => astar_route(&edge_obstacles).unwrap_or_else(corner_route),
    };

    // The corridor heuristic's own-node exemptions (first leg vs. the source box, last leg
    // vs. the destination box) assume those legs are proper stubs leaving/entering their
    // face from outside. After a bypass detour that isn't guaranteed: a route can arrive
    // at, say, a Right face from the far side, running the whole width of the destination's
    // body. Re-route those through A* with both endpoint boxes solid (the stubs start
    // outside them, so that's satisfiable) and keep the result if it is genuinely clean.
    if !path_respects_faces((x1, y1), src_side, &waypoints, (x2, y2), dst_side, &src_rect, &dst_rect) {
        let mut solid = edge_obstacles.clone();
        solid.push(src_rect.clone());
        solid.push(dst_rect.clone());
        if let Some(repaired) = astar_route(&solid).filter(|w| {
            path_respects_faces((x1, y1), src_side, w, (x2, y2), dst_side, &src_rect, &dst_rect)
        }) {
            waypoints = repaired;
        }
    }
    waypoints
}

/// Whether a routed path really leaves its source face and enters its destination face
/// from outside: the first leg must run out along the source face's normal, the last leg
/// must arrive along the destination face's normal from its outward side, and no *other*
/// leg may cut through either box. (`first_clipping_segment` exempts the first/last leg
/// from the endpoint boxes on the assumption they are exactly such stubs.)
pub(crate) fn path_respects_faces(
    p1: (f64, f64),
    src_side: Side,
    waypoints: &[(f64, f64)],
    p2: (f64, f64),
    dst_side: Side,
    src_rect: &ObstacleRect,
    dst_rect: &ObstacleRect,
) -> bool {
    const EPS: f64 = 0.75;
    let mut pts = Vec::with_capacity(waypoints.len() + 2);
    pts.push(p1);
    pts.extend_from_slice(waypoints);
    pts.push(p2);
    let n = pts.len() - 1;

    // `to` must lie straight out from `port` along the face's outward normal.
    let outward = |port: (f64, f64), to: (f64, f64), side: Side| -> bool {
        match side {
            Side::Bottom => (to.0 - port.0).abs() < EPS && to.1 > port.1 + EPS,
            Side::Top => (to.0 - port.0).abs() < EPS && to.1 < port.1 - EPS,
            Side::Right => (to.1 - port.1).abs() < EPS && to.0 > port.0 + EPS,
            Side::Left => (to.1 - port.1).abs() < EPS && to.0 < port.0 - EPS,
        }
    };
    // A direct port-to-port line (no waypoints) is judged by the source face only when
    // the two faces are collinear; skip the strict normal test there.
    if n >= 2 && (!outward(pts[0], pts[1], src_side) || !outward(pts[n], pts[n - 1], dst_side)) {
        return false;
    }
    for i in 0..n {
        let (a, b) = (pts[i], pts[i + 1]);
        if i != 0 && src_rect.clips_segment(a.0, a.1, b.0, b.1, 6.0) {
            return false;
        }
        if i != n - 1 && dst_rect.clips_segment(a.0, a.1, b.0, b.1, 6.0) {
            return false;
        }
    }
    true
}

/// The point `distance` px straight out from a port, along its face's outward normal.
fn stub_point(port: (f64, f64), side: Side, distance: f64) -> (f64, f64) {
    match side {
        Side::Bottom => (port.0, port.1 + distance),
        Side::Top => (port.0, port.1 - distance),
        Side::Left => (port.0 - distance, port.1),
        Side::Right => (port.0 + distance, port.1),
    }
}

/// How far a stub of nominal length `sc` can extend straight out of `port` before it
/// would run into an obstacle (leaving a few px of air) — `sc` when the way is clear,
/// shorter when a neighbouring box sits closer than one stub, so the stub itself never
/// pokes into another node.
fn clear_stub_len(port: (f64, f64), side: Side, sc: f64, obstacles: &[ObstacleRect]) -> f64 {
    const AIR: f64 = 4.0;
    let mut len = sc;
    for o in obstacles {
        let hit = match side {
            Side::Bottom if port.0 > o.x && port.0 < o.x + o.w && o.y >= port.1 - 0.5 => Some(o.y - port.1),
            Side::Top if port.0 > o.x && port.0 < o.x + o.w && o.y + o.h <= port.1 + 0.5 => Some(port.1 - (o.y + o.h)),
            Side::Right if port.1 > o.y && port.1 < o.y + o.h && o.x >= port.0 - 0.5 => Some(o.x - port.0),
            Side::Left if port.1 > o.y && port.1 < o.y + o.h && o.x + o.w <= port.0 + 0.5 => Some(port.0 - (o.x + o.w)),
            _ => None,
        };
        if let Some(d) = hit {
            len = len.min((d - AIR).max(0.0));
        }
    }
    len
}

/// Assigns port fractions for `n` sibling edges sharing one (node, side), in the
/// caller's already crossing-free sort order (sorted by the target/source coordinate
/// each edge is headed to or from — so slot order still tracks "where the connecting
/// component actually is", the sort is just no longer what sets the *exact* fraction):
/// evenly spaced canonical positions — mid for one edge, thirds for two, quarters for
/// three, and so on, via `frac(i) = lo + span * (i+1)/(n+1)` — rather than each edge's
/// own raw "point straight at my target's coordinate" fraction.
///
/// The raw version (this replaces) kept losing to face selection: `choose_edge_sides`
/// picks a face independently of what fraction port assignment then produces, so
/// whenever a sibling's target sat far enough to one side, its raw fraction clamped to
/// the extreme edge of `[lo, hi]` — and that extreme port then fought the face pairing
/// it was paired with, forcing a multi-point detour a more centered port wouldn't have
/// needed (this is what was actually happening on `etcd`'s single `watch` edge: with
/// only one sibling, its "ideal" fraction should have had no reason to sit anywhere but
/// dead center, but the raw target-projection formula pinned it to the clamp ceiling
/// instead). Canonical, count-based positions also just read as *designed* — a diagram
/// whose ports land on a small, predictable set of positions rather than wherever each
/// target's raw coordinate happened to project to.
fn canonical_port_fractions(n: usize, lo: f64, hi: f64) -> Vec<f64> {
    if n == 0 {
        return vec![];
    }
    let span = hi - lo;
    (0..n).map(|i| lo + span * (i + 1) as f64 / (n + 1) as f64).collect()
}

/// Resolves an edge to its (possibly cycle-reversed) endpoints and their laid-out
/// boxes, or `None` if the edge or either endpoint's position is unavailable.
///
/// This replaces five near-identical inline blocks that each re-derived
/// `(src, dst) = edge_endpoints(...).unwrap()` / `layout.positions.get(...).unwrap()`
/// across [`plan_all_edge_routes`] — consolidating them means a missing position is
/// handled once, safely, instead of panicking deep inside whichever loop hits it first.
pub fn resolve_edge_layout<'a>(
    compiled: &CompiledGraph,
    layout: &'a LayoutResult,
    edge_idx: EdgeIndex,
) -> Option<(NodeIndex, NodeIndex, &'a NodeLayout, &'a NodeLayout)> {
    let (src, dst) = compiled.graph.edge_endpoints(edge_idx)?;
    let edge_data = &compiled.graph[edge_idx];
    let (s_idx, d_idx) = if edge_data.reversed {
        (dst, src)
    } else {
        (src, dst)
    };
    let src_nl = layout.positions.get(&s_idx)?;
    let dst_nl = layout.positions.get(&d_idx)?;
    Some((s_idx, d_idx, src_nl, dst_nl))
}

/// Metadata bounding container title zones to prevent edge cutting through group titles.
#[derive(Debug, Clone)]
pub struct GroupTitleZone {
    pub min_x: f64,
    pub max_x: f64,
    pub min_y: f64,
    pub max_y: f64,
    pub node_ids: std::collections::HashSet<String>,
}

/// Each group's title box (see [`rdg_layout::groups::group_title_rect`]) with the ids of
/// every node inside the group, nested groups included.
pub fn compute_group_title_zones(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    tokens: &DesignTokens,
) -> Vec<GroupTitleZone> {
    let rects = rdg_layout::groups::group_rects(compiled, layout, tokens);
    rects
        .iter()
        .enumerate()
        .filter_map(|(g, r)| {
            let (x, y, w, h) = rdg_layout::groups::group_title_rect(&compiled.groups[g], (*r)?, tokens);
            Some(GroupTitleZone {
                min_x: x,
                max_x: x + w,
                min_y: y,
                max_y: y + h,
                node_ids: compiled.nodes_within(g).into_iter().map(|n| compiled.graph[n].id.clone()).collect(),
            })
        })
        .collect()
}

/// Determines the exit side and entry side for an edge, evaluating Euclidean face distance
/// as the starting candidate, while adjusting for geometric flow, obstacle avoidance,
/// and container title banners.
#[allow(clippy::too_many_arguments)]
pub fn choose_edge_sides(
    src_nl: &NodeLayout,
    src_idx: NodeIndex,
    src_id: &str,
    dst_nl: &NodeLayout,
    dst_idx: NodeIndex,
    dst_id: &str,
    edge_data: &EdgeData,
    title_zones: &[GroupTitleZone],
    face_usage: &mut HashMap<(NodeIndex, Side), usize>,
    tokens: &DesignTokens,
) -> (Side, Side) {
    let explicit_src = edge_data.source_port.as_deref().and_then(Side::parse);
    let explicit_dst = edge_data.target_port.as_deref().and_then(Side::parse);

    if let (Some(s), Some(d)) = (explicit_src, explicit_dst) {
        *face_usage.entry((src_idx, s)).or_insert(0) += 1;
        *face_usage.entry((dst_idx, d)).or_insert(0) += 1;
        return (s, d);
    }

    let src_cx = src_nl.x + src_nl.width * 0.5;
    let src_cy = src_nl.y + src_nl.height * 0.5;
    let dst_cx = dst_nl.x + dst_nl.width * 0.5;
    let dst_cy = dst_nl.y + dst_nl.height * 0.5;

    let src_faces = [
        (Side::Bottom, (src_cx, src_nl.y + src_nl.height)),
        (Side::Right, (src_nl.x + src_nl.width, src_cy)),
        (Side::Top, (src_cx, src_nl.y)),
        (Side::Left, (src_nl.x, src_cy)),
    ];

    let dst_faces = [
        (Side::Top, (dst_cx, dst_nl.y)),
        (Side::Left, (dst_nl.x, dst_cy)),
        (Side::Bottom, (dst_cx, dst_nl.y + dst_nl.height)),
        (Side::Right, (dst_nl.x + dst_nl.width, dst_cy)),
    ];

    let mut best_pair = (Side::Bottom, Side::Top);
    let mut best_score = f64::MAX;

    for &(s_side, s_pt) in &src_faces {
        if let Some(es) = explicit_src {
            if es != s_side {
                continue;
            }
        }
        for &(d_side, d_pt) in &dst_faces {
            if let Some(ed) = explicit_dst {
                if ed != d_side {
                    continue;
                }
            }

            let mut penalty = 0.0_f64;

            // 1. Natural launch direction penalties: avoid sharp 180° backward launch
            match s_side {
                Side::Bottom => {
                    if dst_nl.y + dst_nl.height < src_nl.y {
                        penalty += 800.0;
                    }
                }
                Side::Top => {
                    if dst_nl.y > src_nl.y + src_nl.height {
                        penalty += 800.0;
                    }
                }
                Side::Right => {
                    if dst_nl.x + dst_nl.width < src_nl.x {
                        penalty += 800.0;
                    }
                }
                Side::Left => {
                    if dst_nl.x > src_nl.x + src_nl.width {
                        penalty += 800.0;
                    }
                }
            }

            // 2. Natural arrival direction penalties: mirror the launch check's full-separation
            // ("truly 180° backward") semantics above, rather than a tight +/-10px edge
            // tolerance. The old tolerance fired on any partial bounding-box overlap along the
            // cross axis, so a source only slightly overlapping its destination's span (a very
            // common case for diagonally-offset boxes of unequal size) got a full 800 penalty
            // and lost to a far worse, longer-distance attachment pair. Diagonal placements
            // should be decided by distance/flow bonus below, not vetoed by a hair's overlap.
            match d_side {
                Side::Top => {
                    if src_nl.y > dst_nl.y + dst_nl.height {
                        penalty += 800.0;
                    }
                }
                Side::Bottom => {
                    if src_nl.y + src_nl.height < dst_nl.y {
                        penalty += 800.0;
                    }
                }
                Side::Left => {
                    if src_nl.x > dst_nl.x + dst_nl.width {
                        penalty += 800.0;
                    }
                }
                Side::Right => {
                    if src_nl.x + src_nl.width < dst_nl.x {
                        penalty += 800.0;
                    }
                }
            }

            // 3. Container title banner collision avoidance.
            // Only penalise Top entry into a group if:
            //   (a) the destination is inside the group AND the source is OUTSIDE the group,
            //       and there is insufficient whitespace above the group title (< 40px gap).
            // Intra-group edges (where both source and destination are inside the same group)
            // are entirely below the group title banner and must NOT be penalised.
            for tz in title_zones {
                if tz.node_ids.contains(dst_id)
                    && !tz.node_ids.contains(src_id)
                    && d_side == Side::Top
                {
                    let gap_above_group = tz.min_y - (src_nl.y + src_nl.height);
                    if gap_above_group < 40.0 {
                        penalty += 15000.0;
                    }
                }
                if tz.node_ids.contains(src_id)
                    && !tz.node_ids.contains(dst_id)
                    && s_side == Side::Top
                {
                    let gap_above_group = tz.min_y - (dst_nl.y + dst_nl.height);
                    if gap_above_group < 40.0 {
                        penalty += 15000.0;
                    }
                }
            }

            // 4. Primary orientation flow bonuses
            let mut bonus = 0.0_f64;
            let is_horizontal_flow = (dst_nl.x - src_nl.x) > (dst_nl.y - src_nl.y).abs() * 1.2;
            let is_vertical_flow = (dst_nl.y - src_nl.y) > (dst_nl.x - src_nl.x).abs() * 1.2;

            if (is_vertical_flow && s_side == Side::Bottom && d_side == Side::Top)
                || (is_horizontal_flow && s_side == Side::Right && d_side == Side::Left)
            {
                bonus += 60.0;
            }

            let dx = d_pt.0 - s_pt.0;
            let dy = d_pt.1 - s_pt.1;
            let dist = (dx * dx + dy * dy).sqrt();

            // 5. Route-complexity penalty: some face-pairs only get a direct, one-bend
            // path when the two boxes actually sit in the geometric relationship that
            // pair implies (e.g. Bottom→Left needs the destination genuinely
            // below-left of the source's exit) — otherwise `ideal_waypoints_for_faces`
            // falls through to a multi-point detour. Distance and the flow bonus above
            // are blind to that: a pair with a shorter raw face-distance (or that
            // happens to match the diagram's overall horizontal/vertical drift) can
            // still lose badly on actual path complexity once routed. A real render
            // surfaced this directly — two boxes sitting corner-to-corner (one just
            // below and to the right of the other) scored Right→Left as "best" purely
            // on distance and the horizontal-flow bonus, despite the boxes being too
            // close together horizontally for Right→Left's clean case, producing a
            // wide U-shaped detour where Bottom→Left would have been a single clean
            // bend. Weighted well above the flow bonus so it isn't just a tie-breaker.
            //
            // This is deliberately a cheap analytic proxy, not a real obstacle-aware
            // routing trial: an earlier version of this function actually called
            // `compute_edge_waypoints_with_obstacles` per candidate and scored by the
            // real path's length/bend count, which sounds strictly more accurate but
            // isn't — it evaluates face-*center* points, not the real ports a
            // sibling-crowded face will get once Step 2/3 assign them, and it has no
            // visibility into Step 4/5's later corridor coordination between sibling
            // edges. On a real dense fixture that mismatch let a face pair the trial
            // scored as a clean 1-bend path go on to clip four unrelated nodes once the
            // full pipeline (real ports + corridor hints + own-node clearance) actually
            // routed it — confidently wrong is worse than approximately right here.
            penalty += estimate_extra_bends(s_side, d_side, s_pt, d_pt, tokens) * 150.0;

            // 6. Face-congestion penalty: a face already carrying other edges reads as
            // cluttered even when each individual edge on it is cleanly routed, and an
            // unrelated flow sharing a face with this one is a worse reader experience
            // than using a node's otherwise-empty side. Weighted just above the flow
            // bonus (60) so it wins a close call — e.g. a slightly-longer, unaligned
            // face that's currently bare beats a shorter, flow-aligned one that's
            // already carrying a different edge — but doesn't override a genuinely
            // better geometric match (the 800-point backward-launch veto, or a face
            // pairing that's the only clean, non-detouring option).
            let s_usage = *face_usage.get(&(src_idx, s_side)).unwrap_or(&0);
            let d_usage = *face_usage.get(&(dst_idx, d_side)).unwrap_or(&0);
            penalty += (s_usage + d_usage) as f64 * 55.0;

            // 7. Face-capacity penalty: past `face_capacity` ports the face can't keep
            // `min_port_pitch` between them, so arrows merge into a thick bar. Steep
            // enough to spill the overflow onto another face, still under the 800-point
            // backward-launch veto so a genuinely wrong direction isn't chosen to avoid it.
            let s_cap = face_capacity(src_nl, s_side, tokens);
            let d_cap = face_capacity(dst_nl, d_side, tokens);
            penalty += (s_usage + 1).saturating_sub(s_cap) as f64 * 400.0;
            penalty += (d_usage + 1).saturating_sub(d_cap) as f64 * 400.0;

            let score = dist + penalty - bonus;
            if score < best_score {
                best_score = score;
                best_pair = (s_side, d_side);
            }
        }
    }

    *face_usage.entry((src_idx, best_pair.0)).or_insert(0) += 1;
    *face_usage.entry((dst_idx, best_pair.1)).or_insert(0) += 1;

    best_pair
}

/// Routing plan containing resolved attachment faces, distributed ports, channels, and collision-free waypoints.
#[derive(Debug, Clone)]
pub struct EdgeRoutingPlan {
    pub src_side: Side,
    pub dst_side: Side,
    pub exit_port: f64,
    pub entry_port: f64,
    pub channel_y: f64,
    pub corridor_x: f64,
    pub waypoints: Vec<(f64, f64)>,
    /// How many sibling edges shared this edge's Step 4 corridor bucket (1 = this edge
    /// had that shared vertical channel to itself). `review::detect_congested_corridor`
    /// reads this to flag an over-shared channel without re-deriving the bucketing key
    /// Step 4 already computed.
    pub corridor_bucket_size: usize,
    /// Planned by the search router ([`crate::ortho`]), which already optimises bends,
    /// crossings and lanes globally — the polish pass leaves such edges alone.
    pub searched: bool,
}

/// Computes intelligent, obstacle-aware routing plans for all edges in the graph.
///
/// Features:
/// 1. Group title collision avoidance (enters Side::Left or Side::Right instead of cutting title banners).
/// 2. Port sorting monotonically by target entry coordinates (zero self-crossings among siblings).
/// 3. Multi-channel corridor staggering (parallel horizontal segments have dedicated channels, zero overlapping lines).
/// 4. Obstacle-aware vertical corridor allocation (prevents lines from routing through intermediate components).
pub fn plan_all_edge_routes(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    algorithm: RoutingAlgorithm,
    tokens: &DesignTokens,
) -> HashMap<EdgeIndex, EdgeRoutingPlan> {
    // The search-based router (`crate::ortho`) plans every edge it can; the heuristic
    // planner below stays as the fallback for anything it leaves out (self-loops, a node
    // boxed in completely, an oversize grid). `RDG_LEGACY_ROUTER=1` skips the new router,
    // for A/B comparison.
    let mut plans = plan_all_edge_routes_legacy(compiled, layout, algorithm, tokens);
    if std::env::var_os("RDG_LEGACY_ROUTER").is_none() {
        plans.extend(crate::ortho::route_orthogonal(compiled, layout, tokens));
    }
    plans
}

/// The original face-first heuristic planner — see [`plan_all_edge_routes`].
fn plan_all_edge_routes_legacy(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    algorithm: RoutingAlgorithm,
    tokens: &DesignTokens,
) -> HashMap<EdgeIndex, EdgeRoutingPlan> {
    let title_zones = compute_group_title_zones(compiled, layout, tokens);
    let mut initial_sides: HashMap<EdgeIndex, (Side, Side)> = HashMap::new();
    let mut face_usage: HashMap<(NodeIndex, Side), usize> = HashMap::new();

    for edge_idx in compiled.graph.edge_indices() {
        let Some((s_idx, d_idx, src_nl, dst_nl)) = resolve_edge_layout(compiled, layout, edge_idx)
        else {
            continue;
        };
        let edge_data = &compiled.graph[edge_idx];
        let src_id = &compiled.graph[s_idx].id;
        let dst_id = &compiled.graph[d_idx].id;
        let sides = choose_edge_sides(
            src_nl,
            s_idx,
            src_id,
            dst_nl,
            d_idx,
            dst_id,
            edge_data,
            &title_zones,
            &mut face_usage,
            tokens,
        );
        initial_sides.insert(edge_idx, sides);
    }

    // Steps 2-3: Distribute exit AND entry ports without crossing, jointly per face.
    // Sort key is each edge's neighbour-center coordinate along the face — so slot order
    // tracks where the connecting component actually is, preserving cross-prevention —
    // but the fraction itself is a canonical, count-based position (mid/thirds/quarters/
    // ...) rather than a raw projection of that coordinate; see
    // `canonical_port_fractions`'s own doc comment for why.
    //
    // Exits and entries used to be spread independently, so a face carrying one outgoing
    // and one incoming edge gave both the exact same port (0.5) — two connectors
    // starting/ending at the very same point. Merging them into one ordered list per
    // face makes every port on a face distinct. A face carrying only exits keeps the
    // [0.1, 0.9] range and one carrying only entries the narrower [0.2, 0.8] (which keeps
    // an arrowhead clear of the corner), exactly as before; a mixed face uses the
    // in-between [0.15, 0.85].
    struct FaceEdge {
        edge_idx: EdgeIndex,
        is_exit: bool,
        key: f64,
    }
    let mut face_groups: HashMap<(NodeIndex, Side), Vec<FaceEdge>> = HashMap::new();
    for (&edge_idx, &(src_side, dst_side)) in &initial_sides {
        let Some((s_idx, d_idx, src_nl, dst_nl)) = resolve_edge_layout(compiled, layout, edge_idx)
        else {
            continue;
        };
        let axis_center = |nl: &NodeLayout, side: Side| match side {
            Side::Top | Side::Bottom => nl.x + nl.width * 0.5,
            Side::Left | Side::Right => nl.y + nl.height * 0.5,
        };
        face_groups.entry((s_idx, src_side)).or_default().push(FaceEdge {
            edge_idx,
            is_exit: true,
            key: axis_center(dst_nl, src_side),
        });
        face_groups.entry((d_idx, dst_side)).or_default().push(FaceEdge {
            edge_idx,
            is_exit: false,
            key: axis_center(src_nl, dst_side),
        });
    }

    let mut exit_ports: HashMap<EdgeIndex, f64> = HashMap::new();
    let mut entry_ports: HashMap<EdgeIndex, f64> = HashMap::new();
    for (_, mut edges) in face_groups {
        edges.sort_by(|a, b| {
            a.key
                .partial_cmp(&b.key)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.is_exit.cmp(&a.is_exit))
                .then_with(|| a.edge_idx.cmp(&b.edge_idx))
        });
        let has_exit = edges.iter().any(|e| e.is_exit);
        let has_entry = edges.iter().any(|e| !e.is_exit);
        let (lo, hi) = match (has_exit, has_entry) {
            (true, true) => (0.15, 0.85),
            (true, false) => (0.1, 0.9),
            _ => (0.2, 0.8),
        };
        let assigned = canonical_port_fractions(edges.len(), lo, hi);
        for (fe, port) in edges.into_iter().zip(assigned) {
            if fe.is_exit {
                exit_ports.insert(fe.edge_idx, port);
            } else {
                entry_ports.insert(fe.edge_idx, port);
            }
        }
    }

    // Build a complete obstacle list from all node positions — used to avoid routing through
    // them. `layout.positions` is a `HashMap`, whose iteration order is randomized per process;
    // sorting once here (by `NodeIndex`) makes every order-sensitive obstacle scan below
    // (this function is documented as deterministic — "same YAML input always produces the
    // same output" — so it must not depend on that randomized order) give the same answer on
    // every run for the same input, rather than picking a different "first" colliding obstacle
    // each time.
    let all_obstacles = node_obstacles(layout);

    // Step 4: Multi-channel corridor allocation for parallel horizontal segments
    let mut corridor_buckets: HashMap<(i32, i32), Vec<(EdgeIndex, f64)>> = HashMap::new();
    let mut channel_y_map: HashMap<EdgeIndex, f64> = HashMap::new();
    let mut corridor_bucket_size_map: HashMap<EdgeIndex, usize> = HashMap::new();

    for (&edge_idx, &(src_side, dst_side)) in &initial_sides {
        let Some((s_idx, d_idx, src_nl, dst_nl)) = resolve_edge_layout(compiled, layout, edge_idx)
        else {
            continue;
        };

        if src_side == Side::Bottom && dst_side == Side::Top {
            let y1 = src_nl.y + src_nl.height;
            let y2 = dst_nl.y;
            if y2 > y1 + tokens.px(1.25) {
                let bucket = tokens.px(5.625);
                let key = ((y1 / bucket).round() as i32, (y2 / bucket).round() as i32);
                let mid_x = (src_nl.x + dst_nl.x) / 2.0;
                corridor_buckets
                    .entry(key)
                    .or_default()
                    .push((edge_idx, mid_x));
                continue;
            }
        }

        // Default channel height — obstacle-aware: find a y between the two nodes that
        // avoids any other node sitting in the x-span between them, same
        // interval-avoidance `nearest_clear_position` uses for Step 5's vertical
        // corridors below, applied on the y axis here instead. Kept strictly within
        // `[y1, y2]` for the same reason Step 5 doesn't widen past `x1`/`x2` — `y1` is
        // the source's own bottom edge and `y2` the destination's own top edge, so
        // stepping past either lands inside one of the two boxes this edge is actually
        // connecting, not in open space.
        let y1 = src_nl.y + src_nl.height;
        let y2 = dst_nl.y;
        let x_min = src_nl.x.min(dst_nl.x);
        let x_max = (src_nl.x + src_nl.width).max(dst_nl.x + dst_nl.width);
        let clearance = tokens.px(1.5);
        let mut blocked: Vec<(f64, f64)> = all_obstacles
            .iter()
            .filter(|(ni, _)| *ni != s_idx && *ni != d_idx)
            .filter(|(_, r)| r.x + r.w >= x_min && r.x <= x_max)
            .map(|(_, r)| (r.y - clearance, r.y + r.h + clearance))
            .collect();
        let ch_y = nearest_clear_position(y1, y2, y1.min(y2), y1.max(y2), &mut blocked);
        channel_y_map.insert(edge_idx, ch_y);
    }

    for (_, mut edges) in corridor_buckets {
        edges.sort_by(|a, b| {
            a.1.partial_cmp(&b.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        let m = edges.len();
        let Some(&(first_edge, _)) = edges.first() else {
            continue;
        };
        let Some((s_idx, d_idx, src_nl, dst_nl)) = resolve_edge_layout(compiled, layout, first_edge)
        else {
            continue;
        };
        let y1 = src_nl.y + src_nl.height;
        let y2 = dst_nl.y;
        // Same obstacle-aware search as the default (non-bucketed) case above, mirroring
        // how Step 5's own bucketed path centers its spread on obstacle-avoided
        // positions rather than the naive midpoint — see its comment for why.
        let x_min = src_nl.x.min(dst_nl.x);
        let x_max = (src_nl.x + src_nl.width).max(dst_nl.x + dst_nl.width);
        let clearance = tokens.px(1.5);
        let mut blocked: Vec<(f64, f64)> = all_obstacles
            .iter()
            .filter(|(ni, _)| *ni != s_idx && *ni != d_idx)
            .filter(|(_, r)| r.x + r.w >= x_min && r.x <= x_max)
            .map(|(_, r)| (r.y - clearance, r.y + r.h + clearance))
            .collect();
        let base_ymid = nearest_clear_position(y1, y2, y1, y2, &mut blocked);
        let max_spread = (y2 - y1 - tokens.px(2.5)).max(0.0);
        // Spread parallel edges across however much vertical room is actually available
        // (dividing it into `m + 1` slots so there's margin at both ends too), rather than
        // a flat gap regardless of whether the ranks are close or far apart. Floored so a
        // tight rank gap never crams edges closer than that, ceilinged so a wide-open rank
        // gap doesn't spread a couple of edges implausibly far apart.
        let gap = (max_spread / (m as f64 + 1.0)).clamp(tokens.px(1.5), tokens.px(5.0));

        for (k, (edge_idx, _)) in edges.into_iter().enumerate() {
            let ch_y = base_ymid + (k as f64 - (m - 1) as f64 * 0.5) * gap;
            channel_y_map.insert(edge_idx, ch_y);
            corridor_bucket_size_map.insert(edge_idx, m);
        }
    }

    // Protect all group title banners: if any horizontal segment intersects a container title banner,
    // shift channel_y above the container to preserve header legibility.
    for (&edge_idx, ch_y) in channel_y_map.iter_mut() {
        let Some((_s_idx, _d_idx, src_nl, dst_nl)) =
            resolve_edge_layout(compiled, layout, edge_idx)
        else {
            continue;
        };

        let x_span_min = src_nl.x.min(dst_nl.x);
        let x_span_max = (src_nl.x + src_nl.width).max(dst_nl.x + dst_nl.width);

        for tz in &title_zones {
            if x_span_max >= tz.min_x
                && x_span_min <= tz.max_x + tokens.px(7.5)
                && *ch_y >= tz.min_y - tokens.px(1.0)
                && *ch_y <= tz.max_y + tokens.px(1.5)
            {
                *ch_y = tz.min_y - tokens.px(2.0);
            }
        }
    }

    // Step 5: Obstacle-aware vertical corridor allocation for horizontal transitions,
    // with multi-channel separation for edges sharing a similar corridor — mirrors
    // Step 4's bucketing for Bottom→Top edges. Without this, two Left/Right edges
    // between similarly-positioned source/destination pairs (e.g. two sibling nodes
    // both routing into the same downstream node) each independently computed nearly
    // the same `best_x` and drew directly on top of each other for their entire
    // vertical run. A real render surfaced this exactly: two edges from two sibling
    // source nodes into the same destination, sharing corridor for 185px.
    let mut corridor_x_buckets: HashMap<(i32, i32), Vec<(EdgeIndex, f64)>> = HashMap::new();
    let mut corridor_x_map: HashMap<EdgeIndex, f64> = HashMap::new();
    for (&edge_idx, &(src_side, dst_side)) in &initial_sides {
        let Some((s_idx, d_idx, src_nl, dst_nl)) = resolve_edge_layout(compiled, layout, edge_idx)
        else {
            continue;
        };

        if (src_side == Side::Right && dst_side == Side::Left)
            || (src_side == Side::Left && dst_side == Side::Right)
        {
            let x1 = if src_side == Side::Right {
                src_nl.x + src_nl.width
            } else {
                src_nl.x
            };
            let x2 = if dst_side == Side::Left {
                dst_nl.x
            } else {
                dst_nl.x + dst_nl.width
            };
            let y1 = src_nl.y + src_nl.height * 0.5;
            let y2 = dst_nl.y + dst_nl.height * 0.5;
            let y_min = y1.min(y2);
            let y_max = y1.max(y2);

            // Every intermediate node whose y-range overlaps this corridor's vertical
            // span blocks the x-range it occupies (plus clearance) — collected as
            // intervals so `nearest_clear_position` can find a corridor x clear of *all*
            // of them at once, rather than reacting to just one at a time (see its own
            // doc comment for why that used to still leave the corridor blocked).
            //
            // The search stays strictly within `[x1, x2]` — not widened past either
            // port's own coordinate — because past `x1` is inside the source's own row
            // and past `x2` is inside the destination's own box (its left/right edge
            // *is* `x2`; stepping past it lands inside the box, not in open space next
            // to it). A real fixture surfaced this directly: widening here once let the
            // "obstacle-clear" search escape a few px past the destination's own edge,
            // landing the corridor inside the destination's box — which Phase 3's own-
            // node clearance then had to bypass *again*, undoing the clean corridor this
            // was trying to produce. Unlike Step 4's `channel_y` below (genuinely open
            // space above/below both nodes), there's no analogous safe "outside" here.
            let clearance = tokens.px(1.5);
            let mut blocked: Vec<(f64, f64)> = all_obstacles
                .iter()
                .filter(|(ni, _)| *ni != s_idx && *ni != d_idx)
                .filter(|(_, r)| r.y + r.h >= y_min && r.y <= y_max)
                .map(|(_, r)| (r.x - clearance, r.x + r.w + clearance))
                .collect();
            let best_x = nearest_clear_position(x1, x2, x1.min(x2), x1.max(x2), &mut blocked);

            let bucket = tokens.px(5.625);
            let key = ((x1 / bucket).round() as i32, (x2 / bucket).round() as i32);
            corridor_x_buckets.entry(key).or_default().push((edge_idx, best_x));
        }
    }

    for (_, mut edges) in corridor_x_buckets {
        if edges.len() == 1 {
            let (edge_idx, x) = edges[0];
            corridor_x_map.insert(edge_idx, x);
            continue;
        }
        edges.sort_by(|a, b| {
            a.1.partial_cmp(&b.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        let m = edges.len();
        let Some(&(first_edge, _)) = edges.first() else {
            continue;
        };
        let Some((_s_idx, _d_idx, src_nl, dst_nl)) = resolve_edge_layout(compiled, layout, first_edge)
        else {
            continue;
        };
        let (src_side, dst_side) = initial_sides[&first_edge];
        let x1 = if src_side == Side::Right { src_nl.x + src_nl.width } else { src_nl.x };
        let x2 = if dst_side == Side::Left { dst_nl.x } else { dst_nl.x + dst_nl.width };
        // Center the spread on the *obstacle-avoided* positions (the `best_x` each
        // edge already computed above), not the naive (x1+x2)/2 midpoint — a naive
        // midpoint can sit inside a real obstacle between the two ranks, and since
        // Phase 2's per-edge bypass has no awareness of siblings either, every edge in
        // the bucket would independently reroute around that same obstacle to the same
        // spot, silently undoing this spread. A real render surfaced exactly this: two
        // edges into the same destination stayed perfectly overlapped even after this
        // bucketing landed, because both bypassed the same in-between obstacle
        // identically.
        let base_xmid = edges.iter().map(|&(_, x)| x).sum::<f64>() / m as f64;
        // Same "however much room is actually available, floored/ceilinged" spread as
        // Step 4 — see its own comment for why a flat gap regardless of corridor width
        // isn't the right default.
        let max_spread = ((x2 - x1).abs() - tokens.px(2.5)).max(0.0);
        let gap = (max_spread / (m as f64 + 1.0)).clamp(tokens.px(1.5), tokens.px(5.0));

        for (k, (edge_idx, _)) in edges.into_iter().enumerate() {
            let cx = base_xmid + (k as f64 - (m - 1) as f64 * 0.5) * gap;
            corridor_x_map.insert(edge_idx, cx);
            corridor_bucket_size_map.insert(edge_idx, m);
        }
    }

    let env = RouteEnv { all_obstacles: &all_obstacles, title_zones: &title_zones, algorithm, tokens };
    let mut plans = HashMap::new();
    for (edge_idx, (src_side, dst_side)) in initial_sides {
        let Some((s_idx, d_idx, src_nl, dst_nl)) = resolve_edge_layout(compiled, layout, edge_idx)
        else {
            continue;
        };

        let exit_port = exit_ports.get(&edge_idx).copied().unwrap_or(0.5);
        let entry_port = entry_ports.get(&edge_idx).copied().unwrap_or(0.5);
        let channel_y = channel_y_map
            .get(&edge_idx)
            .copied()
            .unwrap_or((src_nl.y + dst_nl.y) / 2.0);
        let corridor_x = corridor_x_map.get(&edge_idx).copied().unwrap_or(0.0);

        let waypoints = route_edge(
            &env,
            (s_idx, src_nl, src_side, exit_port),
            (d_idx, dst_nl, dst_side, entry_port),
            channel_y,
            corridor_x,
        );

        let corridor_bucket_size = corridor_bucket_size_map.get(&edge_idx).copied().unwrap_or(1);

        plans.insert(
            edge_idx,
            EdgeRoutingPlan {
                src_side,
                dst_side,
                exit_port,
                entry_port,
                channel_y,
                corridor_x,
                waypoints,
                corridor_bucket_size,
                searched: false,
            },
        );
    }

    deoverlap_edge_plans(&mut plans, compiled, layout, &all_obstacles, tokens);

    plans
}

/// Above this many edges, the O(E² · segments²) all-pairs scan in
/// [`deoverlap_edge_plans`] is skipped — a safety valve for very large diagrams, not a
/// design value tied to any particular diagram size.
const DEOVERLAP_SCAN_MAX_EDGES: usize = 400;

/// Minimum collinear-overlap length, in px, before two edges' segments are nudged
/// apart — shorter than this isn't worth the risk of a cosmetic shift.
const DEOVERLAP_MIN_LEN: f64 = 12.0;

fn segment_orientation(a: (f64, f64), b: (f64, f64), tol: f64) -> Option<bool> {
    if (a.0 - b.0).abs() < tol {
        Some(true) // vertical
    } else if (a.1 - b.1).abs() < tol {
        Some(false) // horizontal
    } else {
        None
    }
}

/// Length over which two axis-parallel segments overlap along their shared axis, counting
/// them as overlapping when they run within `lane` of each other (not just exactly on top
/// of each other; see `review::collinear_overlap_len` for the exact-coincidence detector) —
/// `tol` stays the tolerance for deciding a segment is axis-aligned at all. Lines a few
/// px apart read as one thick smear, so the lane spacing, not exact coincidence, is the
/// real readability threshold. Zero when the segments aren't both axis-aligned the same
/// way or don't share a span, which distinguishes two edges that merely cross at a point
/// from two that run alongside each other for a real stretch.
pub(crate) fn parallel_overlap_len(
    a0: (f64, f64),
    a1: (f64, f64),
    b0: (f64, f64),
    b1: (f64, f64),
    tol: f64,
    lane: f64,
) -> f64 {
    match (segment_orientation(a0, a1, tol), segment_orientation(b0, b1, tol)) {
        (Some(true), Some(true)) if (a0.0 - b0.0).abs() < lane => {
            let (a_lo, a_hi) = (a0.1.min(a1.1), a0.1.max(a1.1));
            let (b_lo, b_hi) = (b0.1.min(b1.1), b0.1.max(b1.1));
            (a_hi.min(b_hi) - a_lo.max(b_lo)).max(0.0)
        }
        (Some(false), Some(false)) if (a0.1 - b0.1).abs() < lane => {
            let (a_lo, a_hi) = (a0.0.min(a1.0), a0.0.max(a1.0));
            let (b_lo, b_hi) = (b0.0.min(b1.0), b0.0.max(b1.0));
            (a_hi.min(b_hi) - a_lo.max(b_lo)).max(0.0)
        }
        _ => 0.0,
    }
}

/// Finds the position closest to the midpoint of `a` and `b`, within `[lo, hi]`, that
/// doesn't fall inside any of `blocked` (a list of `(start, end)` intervals along the
/// same axis — x for a vertical corridor search, y for a horizontal one). Merges
/// overlapping/adjacent blocked intervals first, so stepping just outside the merged
/// interval containing the midpoint is guaranteed clear of every interval that
/// overlapped it — no repeated re-checking needed, unlike a single pass that reacts to
/// one obstacle at a time and never re-verifies an earlier one against the new position
/// (the bug this replaces: with two or more obstacles stacked across the corridor,
/// whichever was checked last could still collide, and the result depended on arbitrary
/// obstacle iteration order). Falls back to the midpoint unchanged if the corridor is
/// too fully blocked to find any clear gap within `[lo, hi]` — the caller's own
/// reactive per-segment bypass remains the final safety net either way; this only has
/// to give it an easier starting point. `a`/`b` (rather than a single target) let a
/// caller widen `[lo, hi]` past the direct `a..b` span (a densely-packed corridor may
/// have no clear gap within it at all) while `nearest_clear_position` still knows what
/// midpoint the search is actually centered on.
fn nearest_clear_position(a: f64, b: f64, lo: f64, hi: f64, blocked: &mut Vec<(f64, f64)>) -> f64 {
    let target = (a + b) / 2.0;
    if blocked.is_empty() {
        return target;
    }
    blocked.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut merged: Vec<(f64, f64)> = Vec::with_capacity(blocked.len());
    for &(s, e) in blocked.iter() {
        if let Some(last) = merged.last_mut() {
            if s <= last.1 {
                last.1 = last.1.max(e);
                continue;
            }
        }
        merged.push((s, e));
    }

    let Some(&(bs, be)) = merged.iter().find(|&&(s, e)| target > s && target < e) else {
        return target;
    };

    // `bs`/`be` are the boundaries of the merged interval containing `target` — since
    // merged intervals are disjoint by construction, a point exactly at either boundary
    // is clear of every blocked interval, not just this one. Each is only a valid escape
    // if there's actually room for it within `[lo, hi]`.
    let before = if bs > lo { Some(bs) } else { None };
    let after = if be < hi { Some(be) } else { None };

    match (before, after) {
        (Some(bf), Some(af)) => {
            if (target - bf).abs() <= (af - target).abs() {
                bf
            } else {
                af
            }
        }
        (Some(bf), None) => bf,
        (None, Some(af)) => af,
        (None, None) => target,
    }
}

pub(crate) fn edge_full_path(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    edge_idx: EdgeIndex,
    plan: &EdgeRoutingPlan,
) -> Option<Vec<(f64, f64)>> {
    let (_s_idx, _d_idx, src_nl, dst_nl) = resolve_edge_layout(compiled, layout, edge_idx)?;
    let p1 = port_point(src_nl, plan.src_side, plan.exit_port);
    let p2 = port_point(dst_nl, plan.dst_side, plan.entry_port);
    let mut pts = Vec::with_capacity(plan.waypoints.len() + 2);
    pts.push(p1);
    pts.extend_from_slice(&plan.waypoints);
    pts.push(p2);
    Some(pts)
}

/// Last-mile cleanup: nudges apart any two *different* edges' segments left running
/// collinear-overlapping after independent routing — the router-agnostic safety net for
/// a failure mode neither per-edge obstacle bypass nor the Step 4/5 corridor bucketing
/// can fully prevent on their own. Bypassing a shared obstacle has exactly two fixed
/// detour points (one per side, from `compute_bypass`), so any number of sibling edges
/// bypassing the *same* obstacle on the *same* side converge on the identical point
/// regardless of how far apart Step 4/5 spread their starting corridor hints — a real
/// render surfaced this directly on two edges into the same destination, still perfectly
/// overlapped even after Step 5 bucketing landed. Purely geometric and applied last,
/// after every other routing decision, so it's agnostic to which router or which phase
/// produced the overlap.
///
/// Deterministic: edges are processed in a fixed (`EdgeIndex`-sorted) order, and only an
/// interior segment (never the legitimate entry/exit stub) of the *later* edge in each
/// overlapping pair is shifted — tried in both perpendicular directions, skipped
/// entirely if neither clears without newly clipping an obstacle, since an overlap is a
/// cosmetic problem but cutting through a node is a correctness one.
fn deoverlap_edge_plans(
    plans: &mut HashMap<EdgeIndex, EdgeRoutingPlan>,
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    all_obstacles: &[(NodeIndex, ObstacleRect)],
    tokens: &DesignTokens,
) {
    if plans.len() > DEOVERLAP_SCAN_MAX_EDGES {
        return;
    }
    let mut idxs: Vec<EdgeIndex> = plans.keys().copied().collect();
    idxs.sort_by_key(|e| e.index());

    // A handful of passes, each nudging a little further, so a pair that needed more
    // than one nudge's worth of separation (or a shift that only reveals a *new*
    // overlap against a third edge) still settles, instead of stopping after
    // whatever one static pass happened to catch. Stops early once a pass makes no
    // changes at all — the common case for an already-clean diagram.
    for pass in 0..3 {
        let nudge = tokens.px(3.0) * (pass as f64 + 1.0);
        let changed = deoverlap_pass(plans, compiled, layout, all_obstacles, &idxs, nudge, tokens.stub_clearance(), tokens.px(1.25));
        if !changed {
            break;
        }
    }
}

/// Whether any two consecutive segments run along the same axis in opposite directions
/// — the line doubling back over itself. Such a path draws as a shorter one (the retraced
/// stretch cancels visually), which is how a shift that flipped a tiny jog's direction
/// turned a normal arrow run-in into a couple of pixels.
pub(crate) fn has_fold_back(pts: &[(f64, f64)]) -> bool {
    const TOL: f64 = 0.5;
    pts.windows(3).any(|w| {
        let (d1x, d1y) = (w[1].0 - w[0].0, w[1].1 - w[0].1);
        let (d2x, d2y) = (w[2].0 - w[1].0, w[2].1 - w[1].1);
        let horizontal = d1y.abs() < TOL && d2y.abs() < TOL && d1x * d2x < 0.0;
        let vertical = d1x.abs() < TOL && d2x.abs() < TOL && d1y * d2y < 0.0;
        (horizontal && d1x.abs() > TOL && d2x.abs() > TOL) || (vertical && d1y.abs() > TOL && d2y.abs() > TOL)
    })
}

/// One sweep of [`deoverlap_edge_plans`]' all-pairs scan; returns whether any edge's
/// waypoints were changed.
fn deoverlap_pass(
    plans: &mut HashMap<EdgeIndex, EdgeRoutingPlan>,
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    all_obstacles: &[(NodeIndex, ObstacleRect)],
    idxs: &[EdgeIndex],
    nudge: f64,
    sc: f64,
    lane: f64,
) -> bool {
    let tol = 1.5;
    let mut changed = false;

    for i in 0..idxs.len() {
        for j in (i + 1)..idxs.len() {
            let (edge_a, edge_b) = (idxs[i], idxs[j]);
            let Some(path_a) = edge_full_path(compiled, layout, edge_a, &plans[&edge_a]) else {
                continue;
            };
            let Some(path_b) = edge_full_path(compiled, layout, edge_b, &plans[&edge_b]) else {
                continue;
            };
            if path_b.len() < 3 {
                continue; // just a stub-to-stub segment — no interior segment to shift
            }

            // Find edge_b's worst-overlapping interior segment (never index 0 or the
            // last segment — those are edge_b's own legitimate stubs).
            let mut worst = (0.0_f64, usize::MAX);
            for (k, wb) in path_b.windows(2).enumerate() {
                if k == 0 || k == path_b.len() - 2 {
                    continue;
                }
                for wa in path_a.windows(2) {
                    let len = parallel_overlap_len(wa[0], wa[1], wb[0], wb[1], tol, lane);
                    if len > worst.0 {
                        worst = (len, k);
                    }
                }
            }
            if worst.0 < DEOVERLAP_MIN_LEN {
                continue;
            }
            let k = worst.1;
            let vertical = (path_b[k].0 - path_b[k + 1].0).abs() < tol;

            let Some((s_idx, d_idx, _, _)) = resolve_edge_layout(compiled, layout, edge_b) else {
                continue;
            };

            let try_shift = |delta: f64| -> Option<Vec<(f64, f64)>> {
                let mut shifted = path_b.clone();
                if vertical {
                    shifted[k].0 += delta;
                    shifted[k + 1].0 += delta;
                } else {
                    shifted[k].1 += delta;
                    shifted[k + 1].1 += delta;
                }
                let clips = all_obstacles.iter().any(|(ni, obs)| {
                    *ni != s_idx
                        && *ni != d_idx
                        && obs.clips_segment(shifted[k].0, shifted[k].1, shifted[k + 1].0, shifted[k + 1].1, 6.0)
                });
                // Sliding an interior segment also lengthens/shortens the two legs beside
                // it — never let that eat into the exit or entry leg below one stub (it
                // was how a 24px arrow run-in became 2px), unless it was already shorter.
                let seg_len = |p: &[(f64, f64)], i: usize| (p[i + 1].0 - p[i].0).hypot(p[i + 1].1 - p[i].1);
                let (first, last) = (0, path_b.len() - 2);
                let legs_kept = seg_len(&shifted, first) >= seg_len(&path_b, first).min(sc) - 0.01
                    && seg_len(&shifted, last) >= seg_len(&path_b, last).min(sc) - 0.01;
                // ...and never push a corridor past a face: a shift that makes the path
                // enter/leave a node from its far side (through its body) is worse than the
                // overlap it was resolving.
                let faces_ok = match (
                    plans.get(&edge_b),
                    all_obstacles.iter().find(|(ni, _)| *ni == s_idx),
                    all_obstacles.iter().find(|(ni, _)| *ni == d_idx),
                ) {
                    (Some(plan), Some((_, sr)), Some((_, dr))) => path_respects_faces(
                        shifted[0],
                        plan.src_side,
                        &shifted[1..shifted.len() - 1],
                        shifted[shifted.len() - 1],
                        plan.dst_side,
                        sr,
                        dr,
                    ),
                    _ => true,
                };
                if clips || !legs_kept || !faces_ok || (has_fold_back(&shifted) && !has_fold_back(&path_b)) {
                    None
                } else {
                    Some(shifted)
                }
            };

            let Some(mut new_path_b) = try_shift(nudge).or_else(|| try_shift(-nudge)) else {
                continue;
            };

            simplify_orthogonal_polyline(&mut new_path_b);
            let new_waypoints = if new_path_b.len() > 2 {
                new_path_b[1..new_path_b.len() - 1].to_vec()
            } else {
                vec![]
            };
            if let Some(plan_b) = plans.get_mut(&edge_b) {
                plan_b.waypoints = new_waypoints;
                changed = true;
            }
        }
    }

    changed
}

// Minimum straight clearance stub extending perpendicularly from any component face
// before any bend now lives on `DesignTokens::stub_clearance()` rather than as a
// free-standing constant here — every call site below takes `tokens: &DesignTokens`
// and reads it from there.

/// A simple AABB obstacle used for arrow routing.
#[derive(Debug, Clone)]
pub struct ObstacleRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl ObstacleRect {
    /// Returns true if axis-aligned segment from (ax,ay)→(bx,by) clips through this rect.
    /// Segment must be purely horizontal or purely vertical.
    pub(crate) fn clips_segment(&self, ax: f64, ay: f64, bx: f64, by: f64, m: f64) -> bool {
        let (rx0, rx1) = (self.x - m, self.x + self.w + m);
        let (ry0, ry1) = (self.y - m, self.y + self.h + m);

        if (ay - by).abs() < 0.5 {
            // Horizontal segment
            let y = ay;
            if y <= ry0 || y >= ry1 {
                return false;
            }
            let (sx0, sx1) = (ax.min(bx), ax.max(bx));
            sx1 > rx0 && sx0 < rx1
        } else {
            // Vertical segment
            let x = ax;
            if x <= rx0 || x >= rx1 {
                return false;
            }
            let (sy0, sy1) = (ay.min(by), ay.max(by));
            sy1 > ry0 && sy0 < ry1
        }
    }
}

/// Pulls a vertical corridor hint into `[lo + sc, hi - sc]` — the gap the source and
/// destination faces look across, at least one stub off each. A hint *outside* that gap
/// is never valid for a Right→Left/Left→Right pair: past the destination face the path
/// would enter it from the far side, straight through the node's body (and before the
/// source face it would leave backwards through its own). Step 5 can propose such a spot
/// when the gap is crowded, since its blocked-interval search excludes the two endpoint
/// nodes; if the clamped corridor then clips something, the bypass/repair passes deal
/// with it. Left unchanged only when the gap is too narrow to fit two stubs.
fn keep_stub_clear(hint: f64, lo: f64, hi: f64, sc: f64) -> f64 {
    if hi - lo >= 2.0 * sc {
        hint.clamp(lo + sc, hi - sc)
    } else {
        hint
    }
}

/// Leg lengths `(first, final)` of the single-corner L-path between two faces — the
/// run out of the source face and the run into the destination face (the one carrying
/// the arrowhead) — or `None` if the pair isn't an L-pair. Negative means the geometry
/// points the wrong way for an L (the destination lies behind the exit direction).
///
/// One definition shared by [`estimate_extra_bends`] (face scoring) and
/// [`ideal_waypoints_for_faces`] (waypoint generation) so the two cannot disagree about
/// whether an L fits — they used to, each with its own copy of "is the direction right",
/// and neither checked the legs were long enough to leave a visible arrow.
fn l_bend_legs(src_side: Side, dst_side: Side, p1: (f64, f64), p2: (f64, f64)) -> Option<(f64, f64)> {
    let (x1, y1) = p1;
    let (x2, y2) = p2;
    Some(match (src_side, dst_side) {
        (Side::Bottom, Side::Left) => (y2 - y1, x2 - x1),
        (Side::Bottom, Side::Right) => (y2 - y1, x1 - x2),
        (Side::Top, Side::Left) => (y1 - y2, x2 - x1),
        (Side::Top, Side::Right) => (y1 - y2, x1 - x2),
        (Side::Right, Side::Top) => (x2 - x1, y2 - y1),
        (Side::Left, Side::Top) => (x1 - x2, y2 - y1),
        (Side::Left, Side::Bottom) => (x1 - x2, y1 - y2),
        (Side::Right, Side::Bottom) => (x2 - x1, y1 - y2),
        _ => return None,
    })
}

/// Whether the L-path fits with both legs at least one clearance stub long — the exit
/// leg so the edge visibly leaves its face, the final leg so the arrowhead has a real
/// shaft rather than sitting on the corner.
fn l_bend_fits(src_side: Side, dst_side: Side, p1: (f64, f64), p2: (f64, f64), sc: f64) -> bool {
    l_bend_legs(src_side, dst_side, p1, p2).is_some_and(|(first, last)| first >= sc && last >= sc)
}

/// Cheaply estimates how many *extra* bends a face-pair would cost, for
/// [`choose_edge_sides`]'s scoring — mirrors each branch's own "clean vs. detour"
/// condition in [`ideal_waypoints_for_faces`] below (kept in sync with it deliberately;
/// a change to one of those conditions should be reflected here too) without generating
/// the actual waypoints, since side selection happens before ports are assigned and
/// only needs to know which branch a pair would fall into, not its exact geometry — the
/// branch conditions themselves depend on the boxes' positions, not the exact port
/// fraction within a face. `p1`/`p2` can be any reasonable estimate of the two exit/
/// entry points (`choose_edge_sides` uses its own center-based candidates).
fn estimate_extra_bends(src_side: Side, dst_side: Side, p1: (f64, f64), p2: (f64, f64), tokens: &DesignTokens) -> f64 {
    let (x1, y1) = p1;
    let (x2, y2) = p2;
    let sc = tokens.stub_clearance();
    // Corner-bend (L-path) cases are clean only when *both* legs are at least one
    // stub long (see `l_bend_fits`). This used to accept any positive leg, which is how
    // an arrow ended up 2px long: the corner sat right against the destination face.
    let clean = match (src_side, dst_side) {
        (Side::Bottom, Side::Top) => (x2 - x1).abs() < 1.5 || y2 > y1,
        (Side::Top, Side::Bottom) => true,
        (Side::Right, Side::Left) => (y2 - y1).abs() < 1.5 || x2 >= x1 + sc * 2.0,
        (Side::Left, Side::Right) => (y2 - y1).abs() < 1.5 || x1 >= x2 + sc * 2.0,
        (Side::Bottom, Side::Left)
        | (Side::Bottom, Side::Right)
        | (Side::Top, Side::Left)
        | (Side::Top, Side::Right)
        | (Side::Right, Side::Top)
        | (Side::Left, Side::Top)
        | (Side::Left, Side::Bottom)
        | (Side::Right, Side::Bottom) => l_bend_fits(src_side, dst_side, p1, p2, sc),
        // Same-side exit/entry always loops out and back — always the detour case.
        (Side::Right, Side::Right)
        | (Side::Left, Side::Left)
        | (Side::Bottom, Side::Bottom)
        | (Side::Top, Side::Top) => false,
    };

    if clean { 0.0 } else { 2.0 }
}

/// Generate the ideal orthogonal waypoints for a given face-pair, with guaranteed
/// STUB_CLEARANCE perpendicular stubs at source and destination.
///
/// The `channel_y` and `corridor_x` hints are used for the shared-channel slot so
/// parallel sibling edges don't overlap.  The result is a polyline of intermediate
/// waypoints (not including p1/p2 themselves).
fn ideal_waypoints_for_faces(
    p1: (f64, f64),
    src_side: Side,
    p2: (f64, f64),
    dst_side: Side,
    channel_y: f64,
    corridor_x: f64,
    tokens: &DesignTokens,
) -> Vec<(f64, f64)> {
    let (x1, y1) = p1;
    let (x2, y2) = p2;
    let sc = tokens.stub_clearance();
    // Side-detour offset when two faces are too close for a clean stub-based bend.
    let detour = tokens.px(4.0);

    match (src_side, dst_side) {
        // ── Straight down → straight up  (most common flow-diagram case) ──────
        (Side::Bottom, Side::Top) => {
            if (x2 - x1).abs() < 1.5 {
                // Same column — no waypoints needed (straight vertical line)
                vec![]
            } else if y2 > y1 {
                // Any positive vertical gap fits a single horizontal jog. Below 2*sc
                // of room, shrink the clearance margin proportionally instead of
                // falling through to the side-excursion branch: with less than 2*sc
                // to work with, that excursion's up/down stubs end up inverted
                // (barely any — or negative — vertical separation between them),
                // which reads as a knotted hairpin once corner rounding is applied.
                // A real render surfaced this directly: two sibling nodes ~44px
                // apart vertically (just under the old 2*sc≈47.6px threshold) both
                // produced a visible hook right above the destination box.
                let margin = sc.min((y2 - y1) / 2.0);
                let min_ch = y1 + margin;
                let max_ch = y2 - margin;
                let ch_y = channel_y.clamp(min_ch, max_ch);
                vec![(x1, ch_y), (x2, ch_y)]
            } else {
                // Destination sits at or above the source's exit point — genuinely
                // no vertical room to drop straight down; go around the side.
                // The excursion must detour further in the *same* direction the
                // destination already is, not away from it — otherwise the path
                // travels away from the destination first and then has to double
                // back past its own starting x, which is the same hairpin shape
                // this branch exists to avoid.
                let y_down = y1 + sc;
                let y_up = y2 - sc;
                let side_x = if corridor_x > 0.0 {
                    corridor_x
                } else if x2 >= x1 {
                    x1.max(x2) + detour
                } else {
                    x1.min(x2) - detour
                };
                vec![(x1, y_down), (side_x, y_down), (side_x, y_up), (x2, y_up)]
            }
        }

        // ── Right → Left  (horizontal same-level flow) ───────────────────────
        (Side::Right, Side::Left) => {
            if (y2 - y1).abs() < 1.5 {
                vec![]
            } else if x2 >= x1 + sc * 2.0 {
                // A `corridor_x` hint is trusted as-is, not re-clamped into `[x1+sc,
                // x2-sc]` — Step 5 already computed it obstacle-aware, and a corridor
                // packed densely enough can legitimately need to sit outside the direct
                // x1..x2 span to actually clear what's in the way (that's the whole
                // reason Step 5 widens its own search past that span). Re-clamping here
                // silently undid that and put the corridor right back where it had come
                // from avoiding. Only the naive no-hint fallback needs bounding.
                let cr_x = if corridor_x > 0.0 {
                    keep_stub_clear(corridor_x, x1, x2, sc)
                } else {
                    ((x1 + x2) / 2.0).clamp(x1 + sc, x2 - sc)
                };
                vec![(cr_x, y1), (cr_x, y2)]
            } else {
                // Nodes overlap/too close horizontally — route around vertically
                let cr_x = if corridor_x > 0.0 {
                    corridor_x
                } else {
                    x1.max(x2) + detour
                };
                vec![(cr_x, y1), (cr_x, y2)]
            }
        }

        // ── Left → Right  (reverse horizontal — mirrors Right→Left above) ────
        (Side::Left, Side::Right) => {
            if (y2 - y1).abs() < 1.5 {
                vec![]
            } else if x1 >= x2 + sc * 2.0 {
                // Room between the two boxes (dst's right edge at x2, src's left
                // edge at x1): put the corridor in that gap. The previous
                // unconditional `x1.min(x2) - detour` always treated this as the
                // tight/overlapping case below, which — whenever the destination's
                // own right edge was the smaller of the two (the common case, dst
                // to the left of src) — placed the corridor *inside the
                // destination's own box* rather than in the gap outside it. That
                // went undetected by obstacle bypass, since a destination node is
                // excluded from its own incoming edge's obstacle list, and showed
                // up on a real render as an incoming edge's line cutting straight
                // across its own destination node's label.
                // See Right→Left above for why a provided `corridor_x` hint isn't
                // re-clamped here.
                let cr_x = if corridor_x > 0.0 {
                    keep_stub_clear(corridor_x, x2, x1, sc)
                } else {
                    ((x1 + x2) / 2.0).clamp(x2 + sc, x1 - sc)
                };
                vec![(cr_x, y1), (cr_x, y2)]
            } else {
                // Nodes overlap/too close horizontally — route around outside both.
                let cr_x = if corridor_x > 0.0 {
                    corridor_x
                } else {
                    x1.min(x2) - detour
                };
                vec![(cr_x, y1), (cr_x, y2)]
            }
        }

        // ── Up → Down  (back-edge) ────────────────────────────────────────────
        (Side::Top, Side::Bottom) => {
            if (x2 - x1).abs() < 1.5 {
                vec![]
            } else {
                let ch_y = if channel_y > 0.0 {
                    channel_y
                } else {
                    (y1 + y2) / 2.0
                };
                let ch_y = ch_y.min(y1 - sc).max(y2 + sc);
                vec![(x1, ch_y), (x2, ch_y)]
            }
        }

        // ── Bottom → Left  (turn right-down-left) ────────────────────────────
        (Side::Bottom, Side::Left) => {
            if l_bend_fits(src_side, dst_side, p1, p2, sc) {
                // Clean L-bend: one corner. Only the turn *direction* needs to
                // be right here — the corner's stub can be short and Phase
                // 2/3 obstacle bypass will detour it if it actually clips a
                // real nearby node, so this doesn't need its own sc margin.
                vec![(x1, y2)]
            } else {
                let y_stub = y1 + sc;
                let x_stub = (x2 - sc).min(x1 - sc);
                vec![(x1, y_stub), (x_stub, y_stub), (x_stub, y2)]
            }
        }

        // ── Bottom → Right  (turn left-down-right) ───────────────────────────
        (Side::Bottom, Side::Right) => {
            if l_bend_fits(src_side, dst_side, p1, p2, sc) {
                vec![(x1, y2)]
            } else {
                let y_stub = y1 + sc;
                let x_stub = (x2 + sc).max(x1 + sc);
                vec![(x1, y_stub), (x_stub, y_stub), (x_stub, y2)]
            }
        }

        // ── Top → Left ────────────────────────────────────────────────────────
        (Side::Top, Side::Left) => {
            let y_stub = y1 - sc;
            if l_bend_fits(src_side, dst_side, p1, p2, sc) {
                vec![(x1, y2)]
            } else {
                let x_stub = (x2 - sc).min(x1 - sc);
                vec![(x1, y_stub), (x_stub, y_stub), (x_stub, y2)]
            }
        }

        // ── Top → Right ───────────────────────────────────────────────────────
        (Side::Top, Side::Right) => {
            let y_stub = y1 - sc;
            if l_bend_fits(src_side, dst_side, p1, p2, sc) {
                vec![(x1, y2)]
            } else {
                let x_stub = (x2 + sc).max(x1 + sc);
                vec![(x1, y_stub), (x_stub, y_stub), (x_stub, y2)]
            }
        }

        // ── Right → Top ───────────────────────────────────────────────────────
        (Side::Right, Side::Top) => {
            if l_bend_fits(src_side, dst_side, p1, p2, sc) {
                vec![(x2, y1)]
            } else {
                let x_stub = x1 + sc;
                let y_stub = (y2 - sc).min(y1 - sc);
                vec![(x_stub, y1), (x_stub, y_stub), (x2, y_stub)]
            }
        }

        // ── Left → Top ────────────────────────────────────────────────────────
        (Side::Left, Side::Top) => {
            let x_stub = x1 - sc;
            if l_bend_fits(src_side, dst_side, p1, p2, sc) {
                vec![(x2, y1)]
            } else {
                let y_stub = (y2 - sc).min(y1 - sc);
                vec![(x_stub, y1), (x_stub, y_stub), (x2, y_stub)]
            }
        }

        // ── Left → Bottom ─────────────────────────────────────────────────────
        (Side::Left, Side::Bottom) => {
            let x_stub = x1 - sc;
            if l_bend_fits(src_side, dst_side, p1, p2, sc) {
                vec![(x2, y1)]
            } else {
                let y_stub = (y2 + sc).max(y1 + sc);
                vec![(x_stub, y1), (x_stub, y_stub), (x2, y_stub)]
            }
        }

        // ── Right → Bottom ────────────────────────────────────────────────────
        (Side::Right, Side::Bottom) => {
            if l_bend_fits(src_side, dst_side, p1, p2, sc) {
                vec![(x2, y1)]
            } else {
                let x_stub = x1 + sc;
                let y_stub = (y2 + sc).max(y1 + sc);
                vec![(x_stub, y1), (x_stub, y_stub), (x2, y_stub)]
            }
        }

        // ── Same-side exit/entry  (e.g. Right→Right, Bottom→Bottom) ──────────
        (Side::Right, Side::Right) => {
            let stub_x = x1.max(x2) + sc;
            vec![(stub_x, y1), (stub_x, y2)]
        }
        (Side::Left, Side::Left) => {
            let stub_x = x1.min(x2) - sc;
            vec![(stub_x, y1), (stub_x, y2)]
        }
        (Side::Bottom, Side::Bottom) => {
            let stub_y = y1.max(y2) + sc;
            vec![(x1, stub_y), (x2, stub_y)]
        }
        (Side::Top, Side::Top) => {
            let stub_y = y1.min(y2) - sc;
            vec![(x1, stub_y), (x2, stub_y)]
        }
    }
}

/// Check whether a polyline p1 → waypoints → p2 passes through any obstacle.
/// Returns the index of the first clipping waypoint-segment, or None.
pub(crate) fn first_clipping_segment(
    p1: (f64, f64),
    waypoints: &[(f64, f64)],
    p2: (f64, f64),
    obstacles: &[ObstacleRect],
) -> Option<usize> {
    let mut all_pts = Vec::with_capacity(waypoints.len() + 2);
    all_pts.push(p1);
    all_pts.extend_from_slice(waypoints);
    all_pts.push(p2);

    for (i, w) in all_pts.windows(2).enumerate() {
        let (ax, ay) = w[0];
        let (bx, by) = w[1];
        for obs in obstacles {
            if obs.clips_segment(ax, ay, bx, by, 6.0) {
                return Some(i);
            }
        }
    }
    None
}

/// Compute collision-free, isolated orthogonal waypoints between start point and end point.
///
/// Algorithm:
/// 1. Generate ideal waypoints for the face-pair (with guaranteed STUB_CLEARANCE stubs).
/// 2. Check each segment of the ideal path against the obstacle list.
/// 3. If any segment clips an obstacle, add bypass legs that route the offending segment
///    around the obstacle via open whitespace (choosing the less-blocked side).
/// 4. Repeat up to 3 times to handle cascading obstacles.
pub fn compute_edge_waypoints(
    p1: (f64, f64),
    src_side: Side,
    p2: (f64, f64),
    dst_side: Side,
    channel_y: f64,
    corridor_x: f64,
    tokens: &DesignTokens,
) -> Vec<(f64, f64)> {
    compute_edge_waypoints_with_obstacles(p1, src_side, p2, dst_side, channel_y, corridor_x, &[], tokens)
}

/// Simplifies an orthogonal polyline by removing consecutive duplicate points
/// and merging collinear consecutive segments into single straight segments.
pub fn simplify_orthogonal_polyline(pts: &mut Vec<(f64, f64)>) {
    if pts.len() <= 2 {
        return;
    }
    // Pass 1: Deduplicate consecutive near-identical points
    let mut deduped: Vec<(f64, f64)> = Vec::with_capacity(pts.len());
    for &p in pts.iter() {
        if let Some(&last) = deduped.last() {
            if (p.0 - last.0).abs() < 0.5 && (p.1 - last.1).abs() < 0.5 {
                continue;
            }
        }
        deduped.push(p);
    }

    // Pass 2: Merge collinear consecutive segments (e.g. (x, y1) -> (x, y2) -> (x, y3))
    let mut no_collinear: Vec<(f64, f64)> = Vec::with_capacity(deduped.len());
    for &p in &deduped {
        if no_collinear.len() >= 2 {
            let prev = no_collinear[no_collinear.len() - 1];
            let prev2 = no_collinear[no_collinear.len() - 2];
            let vert = (prev2.0 - prev.0).abs() < 0.5 && (prev.0 - p.0).abs() < 0.5;
            let horiz = (prev2.1 - prev.1).abs() < 0.5 && (prev.1 - p.1).abs() < 0.5;
            if vert || horiz {
                no_collinear.pop();
            }
        }
        no_collinear.push(p);
    }

    *pts = no_collinear;
}

/// Computes a two-point bypass detour around `obs` for the clipping segment
/// `(ax,ay)-(bx,by)`: above/below for a horizontal segment, left/right for a
/// vertical one. Prefers whichever side doesn't also clip anything in
/// `avoid`, and when both or neither side is clear, prefers whichever is
/// closer to `hint_y`/`hint_x` (taken from `channel_y`/`corridor_x` when
/// positive, else from `p2`) — factored out of the phase-2 bypass loop below
/// so the phase-3 own-node clearance pass can reuse the exact same detour
/// geometry rather than duplicating it.
#[allow(clippy::too_many_arguments)]
fn compute_bypass(
    ax: f64,
    ay: f64,
    bx: f64,
    by: f64,
    obs: &ObstacleRect,
    avoid: &[ObstacleRect],
    channel_y: f64,
    corridor_x: f64,
    p2: (f64, f64),
    tokens: &DesignTokens,
) -> Vec<(f64, f64)> {
    if (ay - by).abs() < 0.5 {
        // Horizontal segment — detour above or below
        let above_y = obs.y - tokens.stub_clearance();
        let below_y = obs.y + obs.h + tokens.stub_clearance();

        let above_clips = avoid.iter().any(|o| o.clips_segment(ax, above_y, bx, above_y, 6.0));
        let below_clips = avoid.iter().any(|o| o.clips_segment(ax, below_y, bx, below_y, 6.0));

        let use_above = if above_clips != below_clips {
            !above_clips
        } else if channel_y > 0.0 {
            (above_y - channel_y).abs() < (below_y - channel_y).abs()
        } else {
            (above_y - p2.1).abs() < (below_y - p2.1).abs()
        };
        let det_y = if use_above { above_y } else { below_y };
        vec![(ax, det_y), (bx, det_y)]
    } else {
        // Vertical segment — detour left or right
        let left_x = obs.x - tokens.stub_clearance();
        let right_x = obs.x + obs.w + tokens.stub_clearance();

        let left_clips = avoid.iter().any(|o| o.clips_segment(left_x, ay, left_x, by, 6.0));
        let right_clips = avoid.iter().any(|o| o.clips_segment(right_x, ay, right_x, by, 6.0));

        let use_left = if left_clips != right_clips {
            !left_clips
        } else if corridor_x > 0.0 {
            (left_x - corridor_x).abs() < (right_x - corridor_x).abs()
        } else {
            (left_x - p2.0).abs() < (right_x - p2.0).abs()
        };
        let det_x = if use_left { left_x } else { right_x };
        vec![(det_x, ay), (det_x, by)]
    }
}

/// Full obstacle-aware variant of `compute_edge_waypoints`.
///
/// `src_rect`/`dst_rect`, when given, are the source/destination node's own
/// boxes — *not* included in `obstacles` (an edge always starts/ends flush
/// against its own endpoints, so treating them as ordinary obstacles would
/// flag the legitimate exit/entry stub as a collision). They're used only in
/// phase 3, and only against segments that aren't that literal stub, to stop
/// a specific real failure mode: phase 2 resolves one clipping segment at a
/// time against *other* obstacles, so two independent bypasses (e.g. around
/// two different nearby nodes) can each nudge the path clear of those
/// obstacles without ever checking whether the result still runs
/// uncomfortably close to the source/destination's own border. A real
/// render surfaced this exactly — a two-pass bypass around a neighboring
/// node left a long vertical run only a few px from the source node's own
/// edge, reading as a thin hairpin hugging its own box.
#[allow(clippy::too_many_arguments)]
pub fn compute_edge_waypoints_with_obstacles(
    p1: (f64, f64),
    src_side: Side,
    p2: (f64, f64),
    dst_side: Side,
    channel_y: f64,
    corridor_x: f64,
    obstacles: &[ObstacleRect],
    tokens: &DesignTokens,
) -> Vec<(f64, f64)> {
    compute_edge_waypoints_with_obstacles_and_clearance(
        p1, src_side, p2, dst_side, channel_y, corridor_x, obstacles, None, None, tokens,
    )
}

/// See [`compute_edge_waypoints_with_obstacles`] — this is the full version
/// with the phase-3 own-node clearance pass; the public function above just
/// calls this with `src_rect`/`dst_rect` set to `None` for callers (tests,
/// the no-plan SVG fallback) that don't have the node layouts on hand.
#[allow(clippy::too_many_arguments)]
pub(crate) fn compute_edge_waypoints_with_obstacles_and_clearance(
    p1: (f64, f64),
    src_side: Side,
    p2: (f64, f64),
    dst_side: Side,
    channel_y: f64,
    corridor_x: f64,
    obstacles: &[ObstacleRect],
    src_rect: Option<&ObstacleRect>,
    dst_rect: Option<&ObstacleRect>,
    tokens: &DesignTokens,
) -> Vec<(f64, f64)> {
    // Phase 1: ideal analytical waypoints
    let mut waypoints =
        ideal_waypoints_for_faces(p1, src_side, p2, dst_side, channel_y, corridor_x, tokens);

    if obstacles.is_empty() {
        let mut all_pts = Vec::with_capacity(waypoints.len() + 2);
        all_pts.push(p1);
        all_pts.extend_from_slice(&waypoints);
        all_pts.push(p2);
        simplify_orthogonal_polyline(&mut all_pts);
        if all_pts.len() > 2 {
            return all_pts[1..all_pts.len() - 1].to_vec();
        }
        return vec![];
    }

    // Phase 2: iterative obstacle bypass (max 3 passes to avoid infinite loops)
    for _pass in 0..3 {
        let Some(clip_seg) = first_clipping_segment(p1, &waypoints, p2, obstacles) else {
            break; // No more clipping segments — done
        };

        // Reconstruct the full point list so we can address the clipping segment
        let mut all_pts = Vec::with_capacity(waypoints.len() + 2);
        all_pts.push(p1);
        all_pts.extend_from_slice(&waypoints);
        all_pts.push(p2);

        let (ax, ay) = all_pts[clip_seg];
        let (bx, by) = all_pts[clip_seg + 1];

        // Find the clipping obstacle (first one that clips this segment)
        let clipping_obs = obstacles
            .iter()
            .find(|obs| obs.clips_segment(ax, ay, bx, by, 6.0));
        let Some(obs) = clipping_obs else {
            break;
        };

        // When the clipping segment is the edge's own exit stub (segment 0, starting
        // at p1) or entry stub (the last segment, ending at p2), bypassing it as-is
        // jogs sideways at the exact border coordinate — the line leaves the port and
        // turns immediately, with no stub, reading as if it runs along the node's own
        // edge. Every clean-path branch in `ideal_waypoints_for_faces` guarantees at
        // least `stub_clearance` of straight travel before a turn; a real render
        // surfaced this exactly (an edge whose direct channel clipped a node sitting
        // between source and target detoured flush against the source's own border,
        // 80+px along it, before finally turning down). Nudging the near end(s) of the
        // segment `stub_clearance` further along its own original direction before
        // bypassing preserves that guarantee — at the cost of one extra waypoint per
        // end nudged — whenever the segment is long enough to support it.
        let sc = tokens.stub_clearance();
        let leaving_p1 = clip_seg == 0;
        let entering_p2 = clip_seg == all_pts.len() - 2;
        let seg_len = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
        let min_len = if leaving_p1 && entering_p2 { sc * 2.0 } else { sc };
        let can_nudge = seg_len > min_len;
        let vertical = (ax - bx).abs() < 0.5;

        let (eff_ax, eff_ay, lead_in) = if leaving_p1 && can_nudge {
            let pt = if vertical { (ax, ay + sc * (by - ay).signum()) } else { (ax + sc * (bx - ax).signum(), ay) };
            (pt.0, pt.1, Some(pt))
        } else {
            (ax, ay, None)
        };
        let (eff_bx, eff_by, lead_out) = if entering_p2 && can_nudge {
            let pt = if vertical { (bx, by - sc * (by - ay).signum()) } else { (bx - sc * (bx - ax).signum(), by) };
            (pt.0, pt.1, Some(pt))
        } else {
            (bx, by, None)
        };

        let bypass =
            compute_bypass(eff_ax, eff_ay, eff_bx, eff_by, obs, obstacles, channel_y, corridor_x, p2, tokens);

        // Rebuild all points: keep up to clip_seg (inclusive), splice in the leave-the-
        // border stub (if any), inject the bypass, splice in the enter-the-border stub
        // (if any), keep from clip_seg + 1.
        let mut new_all_pts = Vec::with_capacity(all_pts.len() + 4);
        new_all_pts.extend_from_slice(&all_pts[..=clip_seg]);
        if let Some(pt) = lead_in {
            new_all_pts.push(pt);
        }
        new_all_pts.extend_from_slice(&bypass);
        if let Some(pt) = lead_out {
            new_all_pts.push(pt);
        }
        new_all_pts.extend_from_slice(&all_pts[clip_seg + 1..]);

        simplify_orthogonal_polyline(&mut new_all_pts);
        if new_all_pts.len() > 2 {
            waypoints = new_all_pts[1..new_all_pts.len() - 1].to_vec();
        } else {
            waypoints = vec![];
        }
    }

    // Phase 3: own-node clearance. Segment index 0 is exempt from `src_rect`
    // (it's the legitimate exit stub) and the last segment is exempt from
    // `dst_rect` (the legitimate entry stub) — every other segment, including
    // ones a phase-2 bypass just introduced, must clear both boxes.
    if let (Some(sr), Some(dr)) = (src_rect, dst_rect) {
        for _pass in 0..2 {
            let mut all_pts = Vec::with_capacity(waypoints.len() + 2);
            all_pts.push(p1);
            all_pts.extend_from_slice(&waypoints);
            all_pts.push(p2);
            let n_segs = all_pts.len() - 1;
            if n_segs < 2 {
                break; // Only the direct stub(s) — nothing else to check.
            }

            let mut found: Option<(usize, &ObstacleRect)> = None;
            for i in 0..n_segs {
                let (ax, ay) = all_pts[i];
                let (bx, by) = all_pts[i + 1];
                if i != 0 && sr.clips_segment(ax, ay, bx, by, 6.0) {
                    found = Some((i, sr));
                    break;
                }
                if i != n_segs - 1 && dr.clips_segment(ax, ay, bx, by, 6.0) {
                    found = Some((i, dr));
                    break;
                }
            }
            let Some((clip_seg, obs)) = found else {
                break;
            };

            let (ax, ay) = all_pts[clip_seg];
            let (bx, by) = all_pts[clip_seg + 1];
            let mut avoid: Vec<ObstacleRect> = obstacles.to_vec();
            avoid.push(sr.clone());
            avoid.push(dr.clone());
            let bypass = compute_bypass(ax, ay, bx, by, obs, &avoid, channel_y, corridor_x, p2, tokens);

            let mut new_all_pts = Vec::with_capacity(all_pts.len() + 2);
            new_all_pts.extend_from_slice(&all_pts[..=clip_seg]);
            new_all_pts.extend_from_slice(&bypass);
            new_all_pts.extend_from_slice(&all_pts[clip_seg + 1..]);

            simplify_orthogonal_polyline(&mut new_all_pts);
            waypoints = if new_all_pts.len() > 2 {
                new_all_pts[1..new_all_pts.len() - 1].to_vec()
            } else {
                vec![]
            };
        }
    }

    simplify_orthogonal_polyline(&mut waypoints);
    waypoints
}

// ---------------------------------------------------------------------------
// Visibility-graph A* router (`RoutingAlgorithm::VisibilityGraphAStar`)
// ---------------------------------------------------------------------------
//
// The native Rust equivalent of what `libavoid` does in C++ (no Rust binding exists
// for it — see the topology-dispatcher design notes): build an orthogonal visibility
// graph over obstacle corners, expand it with a per-arrival-direction state so a bend
// penalty can be charged on direction changes, and run A* (`petgraph::algo::astar`)
// with a cost function that also penalizes hugging an obstacle's clearance boundary.
// Unlike the corridor heuristic above, this reasons about the *whole* obstacle field
// at once rather than reacting to one clipped segment at a time, which is what makes
// it worth the extra cost on obstacle-dense diagrams.

/// (grid clearance, segment-validation margin) pairs tried in order. A
/// coordinate-compressed grid (lines only at obstacle boundaries, not a dense uniform
/// mesh) can end up fully disconnected between the start and end regions once
/// obstacles are packed tightly enough — real verification against a moderately dense
/// force-directed layout hit this on the majority of edges at a flat 10px clearance.
/// Shrinking clearance and retrying, the way a real orthogonal router relaxes
/// constraints under pressure rather than giving up outright, finds a real (if
/// tighter) path in cases a single fixed clearance can't. The first two tiers keep the
/// segment margin at 6.0 — the same threshold `detect_edges_through_nodes` judges
/// "clean" by — so a route accepted there is guaranteed clean; only the final,
/// last-resort tier loosens the margin to match a very tight clearance, trading
/// perfect cleanliness for finding *any* connected route, which is still a strictly
/// better outcome than falling all the way back to the corridor heuristic (which
/// verification showed performs far worse on non-layered layouts regardless).
const ASTAR_ATTEMPTS: &[(f64, f64)] = &[(10.0, 6.0), (6.5, 6.0), (3.0, 3.0)];

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum AstarDir {
    Start,
    Up,
    Down,
    Left,
    Right,
}

/// Compute obstacle-aware orthogonal waypoints via a visibility-graph A* search,
/// retrying with progressively tighter clearance if the grid comes out disconnected.
/// Returns `None` (letting the caller fall back to the corridor heuristic) when no
/// obstacles are nearby — there's nothing for a global search to do better than a
/// direct path in that case — or if every clearance still fails to find a path.
fn compute_edge_waypoints_astar(
    p1: (f64, f64),
    p2: (f64, f64),
    obstacles: &[ObstacleRect],
    tokens: &DesignTokens,
) -> Option<Vec<(f64, f64)>> {
    ASTAR_ATTEMPTS.iter().find_map(|&(clearance, segment_margin)| {
        compute_edge_waypoints_astar_with_clearance(p1, p2, obstacles, clearance, segment_margin, tokens)
    })
}

fn compute_edge_waypoints_astar_with_clearance(
    p1: (f64, f64),
    p2: (f64, f64),
    obstacles: &[ObstacleRect],
    clearance: f64,
    segment_margin: f64,
    tokens: &DesignTokens,
) -> Option<Vec<(f64, f64)>> {
    let margin = tokens.px(18.75);
    let (bx0, by0) = (p1.0.min(p2.0) - margin, p1.1.min(p2.1) - margin);
    let (bx1, by1) = (p1.0.max(p2.0) + margin, p1.1.max(p2.1) + margin);

    let mid = ((p1.0 + p2.0) / 2.0, (p1.1 + p2.1) / 2.0);
    let mut relevant: Vec<&ObstacleRect> = obstacles
        .iter()
        .filter(|o| o.x < bx1 && o.x + o.w > bx0 && o.y < by1 && o.y + o.h > by0)
        .collect();
    if relevant.is_empty() {
        return None;
    }
    relevant.sort_by(|a, b| {
        let da = (a.x + a.w / 2.0 - mid.0).powi(2) + (a.y + a.h / 2.0 - mid.1).powi(2);
        let db = (b.x + b.w / 2.0 - mid.0).powi(2) + (b.y + b.h / 2.0 - mid.1).powi(2);
        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
    });
    relevant.truncate(tokens.astar_max_obstacles);

    let mut xs = vec![p1.0, p2.0];
    let mut ys = vec![p1.1, p2.1];
    let mut x_lines = Vec::new();
    let mut y_lines = Vec::new();
    for o in &relevant {
        let (lx0, lx1) = (o.x - clearance, o.x + o.w + clearance);
        let (ly0, ly1) = (o.y - clearance, o.y + o.h + clearance);
        xs.push(lx0);
        xs.push(lx1);
        ys.push(ly0);
        ys.push(ly1);
        x_lines.push(lx0);
        x_lines.push(lx1);
        y_lines.push(ly0);
        y_lines.push(ly1);
    }
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    xs.dedup_by(|a, b| (*a - *b).abs() < 1.0);
    ys.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    ys.dedup_by(|a, b| (*a - *b).abs() < 1.0);

    let inside_any = |x: f64, y: f64| {
        relevant.iter().any(|o| x > o.x + 0.5 && x < o.x + o.w - 0.5 && y > o.y + 0.5 && y < o.y + o.h - 0.5)
    };

    let (nx, ny) = (xs.len(), ys.len());
    let mut grid_index: HashMap<(usize, usize), usize> = HashMap::new();
    let mut points: Vec<(f64, f64)> = Vec::new();
    for (ix, &x) in xs.iter().enumerate() {
        for (iy, &y) in ys.iter().enumerate() {
            if !inside_any(x, y) {
                grid_index.insert((ix, iy), points.len());
                points.push((x, y));
            }
        }
    }

    let find_col = |target: f64| xs.iter().position(|&x| (x - target).abs() < 1.0);
    let find_row = |target: f64| ys.iter().position(|&y| (y - target).abs() < 1.0);
    let (Some(sx), Some(sy)) = (find_col(p1.0), find_row(p1.1)) else { return None };
    let (Some(ex), Some(ey)) = (find_col(p2.0), find_row(p2.1)) else { return None };

    let start_p = *grid_index.entry((sx, sy)).or_insert_with(|| {
        points.push(p1);
        points.len() - 1
    });
    let end_p = *grid_index.entry((ex, ey)).or_insert_with(|| {
        points.push(p2);
        points.len() - 1
    });

    let hug = |coord: f64, lines: &[f64]| lines.iter().filter(|&&l| (l - coord).abs() < 1.0).count() as f64 * tokens.astar_hug_penalty;

    let mut graph = petgraph::graph::DiGraph::<(usize, AstarDir), f64>::new();
    let mut state_index: HashMap<(usize, AstarDir), petgraph::graph::NodeIndex> = HashMap::new();
    let all_dirs = [AstarDir::Start, AstarDir::Up, AstarDir::Down, AstarDir::Left, AstarDir::Right];

    let connect = |graph: &mut petgraph::graph::DiGraph<(usize, AstarDir), f64>,
                        state_index: &mut HashMap<(usize, AstarDir), petgraph::graph::NodeIndex>,
                        from_p: usize,
                        to_p: usize,
                        dir: AstarDir,
                        length: f64,
                        hug_cost: f64| {
        let (tx, ty) = points[to_p];
        for &d_in in &all_dirs {
            let from_state = *state_index.entry((from_p, d_in)).or_insert_with(|| graph.add_node((from_p, d_in)));
            let bend = if d_in == AstarDir::Start || d_in == dir { 0.0 } else { tokens.astar_bend_penalty };
            let to_state = *state_index.entry((to_p, dir)).or_insert_with(|| graph.add_node((to_p, dir)));
            let _ = (tx, ty);
            graph.add_edge(from_state, to_state, length + bend + hug_cost);
        }
    };

    // A grid neighbor pair being individually outside every obstacle does NOT mean the
    // straight segment between them is clear — on a coordinate-compressed grid (lines
    // only at obstacle boundaries, not a dense uniform mesh), a row or column derived
    // from avoiding one obstacle can still cut straight through a *different* one that
    // just happens to sit between two otherwise-valid neighboring grid points. Every
    // candidate edge must be checked against the obstacle list directly, exactly like
    // the corner-heuristic router already does via `clips_segment` — this is not
    // optional, real-fixture verification caught a majority of edges silently cutting
    // through unrelated nodes before this check was added.
    let segment_clear = |ax: f64, ay: f64, bx: f64, by: f64| !relevant.iter().any(|o| o.clips_segment(ax, ay, bx, by, segment_margin));

    for ix in 0..nx {
        for iy in 0..ny {
            let Some(&p) = grid_index.get(&(ix, iy)) else { continue };
            if ix + 1 < nx {
                if let Some(&q) = grid_index.get(&(ix + 1, iy)) {
                    let (px, py) = points[p];
                    let (qx, qy) = points[q];
                    if segment_clear(px, py, qx, qy) {
                        let length = (qx - px).abs();
                        let hug_cost = hug(py, &y_lines);
                        connect(&mut graph, &mut state_index, p, q, AstarDir::Right, length, hug_cost);
                        connect(&mut graph, &mut state_index, q, p, AstarDir::Left, length, hug_cost);
                    }
                }
            }
            if iy + 1 < ny {
                if let Some(&q) = grid_index.get(&(ix, iy + 1)) {
                    let (px, py) = points[p];
                    let (qx, qy) = points[q];
                    if segment_clear(px, py, qx, qy) {
                        let length = (qy - py).abs();
                        let hug_cost = hug(px, &x_lines);
                        connect(&mut graph, &mut state_index, p, q, AstarDir::Down, length, hug_cost);
                        connect(&mut graph, &mut state_index, q, p, AstarDir::Up, length, hug_cost);
                    }
                }
            }
        }
    }

    let start_node = *state_index.entry((start_p, AstarDir::Start)).or_insert_with(|| graph.add_node((start_p, AstarDir::Start)));

    let result = petgraph::algo::astar(
        &graph,
        start_node,
        |n| graph[n].0 == end_p,
        |e| *e.weight(),
        |n| {
            let (p, _) = graph[n];
            let (x, y) = points[p];
            ((x - p2.0).powi(2) + (y - p2.1).powi(2)).sqrt()
        },
    );

    let (_, path) = result?;
    let mut waypoints: Vec<(f64, f64)> = path.iter().map(|&n| points[graph[n].0]).collect();
    // Endpoints are exact by construction (both p1 and p2 were inserted as grid points
    // above), but replace them explicitly anyway so tiny grid-snapping error never
    // shows up as a visible kink right at the node face.
    if let Some(first) = waypoints.first_mut() {
        *first = p1;
    }
    if let Some(last) = waypoints.last_mut() {
        *last = p2;
    }
    simplify_orthogonal_polyline(&mut waypoints);

    // Merging collinear hops geometrically covers the same continuous line the
    // individually-validated hops did, so this should never fire — but grid subtleties
    // (a "relevant" obstacle spatially filtered out, floating-point snapping at a
    // shared boundary between two unrelated obstacles) are hard to fully rule out by
    // construction, and this is the same check `detect_edges_through_nodes` will run
    // regardless. Verifying the actual final output directly, against the *full*
    // obstacle list (not just this attempt's spatially-filtered `relevant` subset), is
    // a strictly stronger guarantee than trusting the grid was built correctly, and
    // it's what lets this function's contract ("what it returns is obstacle-clean")
    // hold even if some future change to the grid logic reintroduces a gap.
    let inner = &waypoints[..];
    if first_clipping_segment(p1, inner, p2, obstacles).is_some() {
        return None;
    }

    if waypoints.len() > 2 {
        Some(waypoints[1..waypoints.len() - 1].to_vec())
    } else {
        Some(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nearest_clear_position_no_obstacles_returns_target() {
        let mut blocked = vec![];
        assert_eq!(nearest_clear_position(50.0, 50.0, 0.0, 100.0, &mut blocked), 50.0);
    }

    #[test]
    fn test_nearest_clear_position_target_outside_blocked_unchanged() {
        let mut blocked = vec![(40.0, 60.0)];
        assert_eq!(nearest_clear_position(10.0, 10.0, 0.0, 100.0, &mut blocked), 10.0);
    }

    #[test]
    fn test_nearest_clear_position_shifts_to_nearer_edge_of_single_obstacle() {
        let mut blocked = vec![(40.0, 60.0)];
        // 45 is closer to the obstacle's start (40) than its end (60).
        assert_eq!(nearest_clear_position(45.0, 45.0, 0.0, 100.0, &mut blocked), 40.0);
        let mut blocked = vec![(40.0, 60.0)];
        assert_eq!(nearest_clear_position(55.0, 55.0, 0.0, 100.0, &mut blocked), 60.0);
    }

    #[test]
    fn test_nearest_clear_position_merges_overlapping_obstacles() {
        // Two overlapping blocked intervals merge into one (40..70); a target inside
        // the merge must escape past the *merged* boundary, not just the first one.
        let mut blocked = vec![(40.0, 60.0), (55.0, 70.0)];
        assert_eq!(nearest_clear_position(50.0, 50.0, 0.0, 100.0, &mut blocked), 40.0);
    }

    #[test]
    fn test_nearest_clear_position_finds_gap_between_two_obstacles() {
        let mut blocked = vec![(0.0, 40.0), (60.0, 100.0)];
        // 41 is inside the gap [40, 60] already — unchanged.
        assert_eq!(nearest_clear_position(41.0, 41.0, 0.0, 100.0, &mut blocked), 41.0);
    }

    #[test]
    fn test_nearest_clear_position_fully_blocked_falls_back_to_target() {
        let mut blocked = vec![(-10.0, 200.0)];
        assert_eq!(nearest_clear_position(50.0, 50.0, 0.0, 100.0, &mut blocked), 50.0);
    }

    #[test]
    fn test_nearest_clear_position_widened_bounds_still_find_a_gap() {
        // A corridor blocked across its entire direct a..b span still has open space
        // just past the obstacles' own footprint — this is what callers widen `[lo, hi]`
        // for (see Step 5's own comment), rather than reporting "fully blocked" the way
        // `test_nearest_clear_position_fully_blocked_falls_back_to_target` does when the
        // search is kept strictly within `[a, b]`.
        let mut blocked = vec![(0.0, 100.0)];
        // Midpoint (20) is much closer to the low escape (0) than the high one (100).
        assert_eq!(nearest_clear_position(10.0, 30.0, -50.0, 150.0, &mut blocked), 0.0);
    }

    #[test]
    fn test_canonical_port_fractions_single_edge_is_dead_center() {
        let assigned = canonical_port_fractions(1, 0.1, 0.9);
        assert_eq!(assigned, vec![0.5]);
    }

    #[test]
    fn test_canonical_port_fractions_two_edges_are_thirds() {
        let assigned = canonical_port_fractions(2, 0.1, 0.9);
        assert!((assigned[0] - (0.1 + 0.8 / 3.0)).abs() < 1e-9);
        assert!((assigned[1] - (0.1 + 0.8 * 2.0 / 3.0)).abs() < 1e-9);
    }

    #[test]
    fn test_canonical_port_fractions_three_edges_are_quarters() {
        let assigned = canonical_port_fractions(3, 0.1, 0.9);
        assert!((assigned[0] - 0.3).abs() < 1e-9);
        assert!((assigned[1] - 0.5).abs() < 1e-9);
        assert!((assigned[2] - 0.7).abs() < 1e-9);
    }

    #[test]
    fn test_canonical_port_fractions_always_in_range_and_evenly_spaced() {
        let assigned = canonical_port_fractions(6, 0.1, 0.9);
        assert_eq!(assigned.len(), 6);
        for &p in &assigned {
            assert!((0.1..=0.9).contains(&p), "port {p} out of range");
        }
        let gaps: Vec<f64> = assigned.windows(2).map(|w| w[1] - w[0]).collect();
        for w in gaps.windows(2) {
            assert!((w[0] - w[1]).abs() < 1e-9, "gaps should be equal: {gaps:?}");
        }
    }

    #[test]
    fn test_side_parse() {
        assert_eq!(Side::parse("Top"), Some(Side::Top));
        assert_eq!(Side::parse("south"), Some(Side::Bottom));
        assert_eq!(Side::parse("e"), Some(Side::Right));
        assert_eq!(Side::parse("nonsense"), None);
    }

    #[test]
    fn test_port_point_matches_each_face() {
        let nl = NodeLayout {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 40.0,
        };
        assert_eq!(port_point(&nl, Side::Top, 0.5), (60.0, 20.0));
        assert_eq!(port_point(&nl, Side::Bottom, 0.5), (60.0, 60.0));
        assert_eq!(port_point(&nl, Side::Left, 0.0), (10.0, 20.0));
        assert_eq!(port_point(&nl, Side::Right, 1.0), (110.0, 60.0));
    }

    #[test]
    fn test_straight_vertical_needs_no_waypoints() {
        let wps = compute_edge_waypoints(
            (50.0, 10.0),
            Side::Bottom,
            (50.0, 100.0),
            Side::Top,
            0.0,
            0.0,
            &DesignTokens::default(),
        );
        assert!(wps.is_empty());
    }

    #[test]
    fn test_obstacle_bypass_avoids_clipping() {
        let obstacles = vec![ObstacleRect {
            x: 40.0,
            y: 40.0,
            w: 20.0,
            h: 20.0,
        }];
        let wps = compute_edge_waypoints_with_obstacles(
            (50.0, 10.0),
            Side::Bottom,
            (50.0, 100.0),
            Side::Top,
            50.0,
            0.0,
            &obstacles,
            &DesignTokens::default(),
        );
        assert!(first_clipping_segment((50.0, 10.0), &wps, (50.0, 100.0), &obstacles).is_none());
    }

    #[test]
    fn test_obstacle_bypass_with_multiple_stacked_obstacles() {
        // Two obstacles stacked in the vertical span between source and destination — the
        // bypass loop gets up to 3 passes specifically so cascading obstacles like this
        // resolve rather than only dodging the first one found.
        let obstacles = vec![
            ObstacleRect {
                x: 30.0,
                y: 30.0,
                w: 40.0,
                h: 15.0,
            },
            ObstacleRect {
                x: 30.0,
                y: 60.0,
                w: 40.0,
                h: 15.0,
            },
        ];
        let wps = compute_edge_waypoints_with_obstacles(
            (50.0, 10.0),
            Side::Bottom,
            (50.0, 100.0),
            Side::Top,
            50.0,
            0.0,
            &obstacles,
            &DesignTokens::default(),
        );
        assert!(
            first_clipping_segment((50.0, 10.0), &wps, (50.0, 100.0), &obstacles).is_none(),
            "route must clear both stacked obstacles: {wps:?}"
        );
    }

}
