//! Final-stage, exception-driven polish: small, guarded, deterministic adjustments to edge
//! ports and waypoints that a person would make by hand on an otherwise finished diagram.
//!
//! Layout and routing get the diagram *right* (no overlaps, no edge through a node, readable
//! arrow lengths); this pass gets it *tidy*. It scans the routed diagram for located
//! **exceptions** — a micro-jog (an interior segment too short to read as intentional), an
//! avoidable crossing — and for each one tries a short, fixed list of local fixes. A fix is
//! kept only if
//!
//! 1. it passes every hard **guard** (still orthogonal, clips no node or group title, leaves
//!    and enters its faces straight, keeps its stub legs, adds no bend or fold-back, keeps
//!    port spacing), and
//! 2. it strictly improves the affected edges' lexicographic **score**
//!    `(crossings, micro-jogs, near-parallel overlaps, bends, length)`.
//!
//! Both conditions together make the pass monotonic (it can never make the diagram worse by
//! its own measure) and idempotent (a second run finds nothing left it can improve). Edges
//! are visited in `EdgeIndex` order and candidate fixes are tried in a fixed order, so the
//! output is byte-identical run to run. Nodes are never moved here.

use std::collections::HashMap;

use petgraph::stable_graph::{EdgeIndex, NodeIndex};

use rdg_dispatch::RoutingAlgorithm;
use rdg_graph::CompiledGraph;
use rdg_layout::{DesignTokens, LayoutResult, NodeLayout};

use crate::routing::{
    EdgeRoutingPlan, GroupTitleZone, ObstacleRect, RouteEnv, Side, compute_group_title_zones,
    edge_full_path, first_clipping_segment, has_fold_back, node_obstacles, parallel_overlap_len,
    path_respects_faces, port_point, resolve_edge_layout, route_edge, simplify_orthogonal_polyline,
};

/// A point in canvas pixels.
pub type Pt = (f64, f64);

/// Tolerance for "this segment is axis-aligned" and "these coordinates coincide".
const TOL: f64 = 0.5;
/// Two near-parallel interior segments only count as one smeared line past this overlap.
const MIN_OVERLAP_LEN: f64 = 12.0;
/// Score length differences below this are noise, not improvement.
const LEN_EPS: f64 = 0.5;
/// Off-axis drift still treated as "along the face" when deciding whether a jog can be absorbed
/// by sliding a port.
const SKEW_TOL: f64 = 2.0;
/// A port never slides closer than this fraction to either end of its face.
const PORT_LO: f64 = 0.08;
const PORT_HI: f64 = 0.92;

// ---------------------------------------------------------------------------
// Geometry helpers (also used by `review::spacing_metrics`)
// ---------------------------------------------------------------------------

fn simplified(path: &[Pt]) -> Vec<Pt> {
    let mut p = path.to_vec();
    simplify_orthogonal_polyline(&mut p);
    p
}

fn seg_len(a: Pt, b: Pt) -> f64 {
    (b.0 - a.0).hypot(b.1 - a.1)
}

/// `Some(true)` for a vertical segment, `Some(false)` for horizontal, `None` for diagonal
/// or degenerate.
fn orientation(a: Pt, b: Pt) -> Option<bool> {
    let (dx, dy) = ((a.0 - b.0).abs(), (a.1 - b.1).abs());
    match (dx < TOL, dy < TOL) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        _ => None,
    }
}

/// Number of turns in a path (interior points of the simplified polyline).
pub fn bend_count(path: &[Pt]) -> usize {
    simplified(path).len().saturating_sub(2)
}

/// Indices (into the simplified path) of interior segments shorter than `min_jog` — the
/// short "step" between two parallel runs that reads as a kink rather than a deliberate
/// bend. The first and last segments are the exit/entry stubs and are never jogs.
pub fn micro_jog_segments(path: &[Pt], min_jog: f64) -> Vec<usize> {
    let p = simplified(path);
    if p.len() < 4 {
        return Vec::new();
    }
    (1..p.len() - 2).filter(|&i| seg_len(p[i], p[i + 1]) < min_jog).collect()
}

/// Points where a segment of `a` crosses a segment of `b` at a right angle, strictly inside
/// both segments (a shared endpoint or a T-junction at a bend is not a crossing).
pub fn crossing_points(a: &[Pt], b: &[Pt]) -> Vec<Pt> {
    const M: f64 = 1.0;
    let mut out = Vec::new();
    for wa in a.windows(2) {
        let Some(a_vert) = orientation(wa[0], wa[1]) else { continue };
        for wb in b.windows(2) {
            let Some(b_vert) = orientation(wb[0], wb[1]) else { continue };
            if a_vert == b_vert {
                continue;
            }
            let (v, h) = if a_vert { (wa, wb) } else { (wb, wa) };
            let (x, y) = (v[0].0, h[0].1);
            let (hx0, hx1) = (h[0].0.min(h[1].0), h[0].0.max(h[1].0));
            let (vy0, vy1) = (v[0].1.min(v[1].1), v[0].1.max(v[1].1));
            if hx0 + M < x && x < hx1 - M && vy0 + M < y && y < vy1 - M {
                out.push((x, y));
            }
        }
    }
    out
}

/// Whether two paths run on top of each other (collinear within 1.5px) for a real stretch on
/// *any* segment, stubs included — the same test `review::find_overlapping_edge_pairs` uses,
/// so polish and review can never disagree about what counts as two edges merged into one line.
fn merged(a: &[Pt], b: &[Pt]) -> bool {
    const T: f64 = 1.5;
    a.windows(2).any(|wa| {
        b.windows(2)
            .any(|wb| parallel_overlap_len(wa[0], wa[1], wb[0], wb[1], T, T) >= MIN_OVERLAP_LEN)
    })
}

/// Number of interior-segment pairs (never the stubs, which legitimately sit a port pitch
/// apart) of two paths that run alongside each other within `lane` for a real stretch.
fn interior_overlaps(a: &[Pt], b: &[Pt], lane: f64) -> usize {
    let (a, b) = (simplified(a), simplified(b));
    let (na, nb) = (a.len().saturating_sub(1), b.len().saturating_sub(1));
    let mut count = 0;
    for i in 1..na.saturating_sub(1) {
        for j in 1..nb.saturating_sub(1) {
            if parallel_overlap_len(a[i], a[i + 1], b[j], b[j + 1], TOL, lane) >= MIN_OVERLAP_LEN {
                count += 1;
            }
        }
    }
    count
}

// ---------------------------------------------------------------------------
// Score
// ---------------------------------------------------------------------------

/// Lexicographic quality measure for a set of edges; lower is better:
/// `(merged, crossings, jogs, overlaps, bends, length)`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Score {
    /// Pairs of edges drawn on top of each other. Worse than a crossing (the reader can't
    /// tell where one edge ends and the other begins), so it ranks first.
    merged: usize,
    crossings: usize,
    jogs: usize,
    /// Interior segments running within one lane of each other.
    overlaps: usize,
    bends: usize,
    length: f64,
}

impl Score {
    fn add(&mut self, o: &Score) {
        self.merged += o.merged;
        self.crossings += o.crossings;
        self.jogs += o.jogs;
        self.overlaps += o.overlaps;
        self.bends += o.bends;
        self.length += o.length;
    }

    fn better_than(&self, o: &Score) -> bool {
        (self.merged, self.crossings, self.jogs, self.overlaps, self.bends)
            .cmp(&(o.merged, o.crossings, o.jogs, o.overlaps, o.bends))
            .then(if self.length + LEN_EPS < o.length {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            })
            == std::cmp::Ordering::Less
    }
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

/// The kind of defect an [`Exception`] describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExceptionKind {
    /// An interior segment shorter than `polish_min_jog`.
    MicroJog,
    /// Two edges cross at a right angle.
    Crossing,
    /// Two edges' interior segments run alongside each other within one lane.
    NearParallel,
}

/// One located defect the pass found and (if `remaining`) could not fix.
#[derive(Debug, Clone)]
pub struct Exception {
    pub kind: ExceptionKind,
    pub edges: Vec<EdgeIndex>,
    pub at: Pt,
    pub detail: String,
}

/// Defect totals for the whole diagram.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    /// Pairs of edges drawn on top of each other (never introduced by polish).
    pub merged: usize,
    pub micro_jogs: usize,
    pub crossings: usize,
    pub near_parallel: usize,
}

/// What the pass did.
#[derive(Debug, Clone, Default)]
pub struct PolishReport {
    pub before: Totals,
    pub after: Totals,
    /// One line per committed fix, in the order applied.
    pub applied: Vec<String>,
    /// Defects still present after the pass.
    pub remaining: Vec<Exception>,
}

impl PolishReport {
    /// A one-line human summary, e.g. `polish: 31 jogs -> 3, 22 crossings -> 19 (34 fixes)`.
    pub fn summary(&self) -> String {
        format!(
            "polish: merged {} -> {}, micro-jogs {} -> {}, crossings {} -> {}, near-parallel {} -> {} ({} fixes)",
            self.before.merged,
            self.after.merged,
            self.before.micro_jogs,
            self.after.micro_jogs,
            self.before.crossings,
            self.after.crossings,
            self.before.near_parallel,
            self.after.near_parallel,
            self.applied.len()
        )
    }
}

// ---------------------------------------------------------------------------
// The pass
// ---------------------------------------------------------------------------

struct Candidate {
    plan: EdgeRoutingPlan,
    desc: String,
}

struct Polisher<'a> {
    compiled: &'a CompiledGraph,
    layout: &'a LayoutResult,
    tokens: &'a DesignTokens,
    obstacles: Vec<(NodeIndex, ObstacleRect)>,
    titles: Vec<GroupTitleZone>,
    algorithm: RoutingAlgorithm,
    plans: &'a mut HashMap<EdgeIndex, EdgeRoutingPlan>,
    paths: HashMap<EdgeIndex, Vec<Pt>>,
    ids: Vec<EdgeIndex>,
    applied: Vec<String>,
    /// Re-routes still allowed. Counted, not timed, so the cut-off (and therefore the
    /// output) is identical on every machine and every run.
    reroutes_left: std::cell::Cell<u32>,
}

/// Runs the polish pass over a routed diagram, mutating `edge_plans` in place. Does nothing
/// (and reports empty totals) when disabled or for sequence diagrams.
pub fn polish(
    compiled: &CompiledGraph,
    layout: &mut LayoutResult,
    edge_plans: &mut HashMap<EdgeIndex, EdgeRoutingPlan>,
    algorithm: RoutingAlgorithm,
    tokens: &DesignTokens,
) -> PolishReport {
    if !tokens.polish_enabled || layout.sequence_info.is_some() || edge_plans.is_empty() {
        return PolishReport::default();
    }
    let mut ids: Vec<EdgeIndex> = edge_plans.keys().copied().collect();
    ids.sort_by_key(|e| e.index());
    let paths: HashMap<EdgeIndex, Vec<Pt>> = ids
        .iter()
        .filter_map(|&e| edge_full_path(compiled, layout, e, &edge_plans[&e]).map(|p| (e, p)))
        .collect();

    let mut p = Polisher {
        compiled,
        layout,
        tokens,
        obstacles: node_obstacles(layout),
        titles: compute_group_title_zones(compiled, layout, tokens),
        algorithm,
        plans: edge_plans,
        paths,
        ids,
        applied: Vec::new(),
        reroutes_left: std::cell::Cell::new(tokens.polish_max_reroutes),
    };

    let before = p.totals();
    for _ in 0..tokens.polish_max_passes.max(1) {
        if !p.sweep() {
            break;
        }
    }
    let after = p.totals();
    let remaining = p.scan();
    let report = PolishReport { before, after, applied: std::mem::take(&mut p.applied), remaining };
    if std::env::var("RDG_DEBUG_POLISH").is_ok() {
        for line in &report.applied {
            eprintln!("  polish fix: {line}");
        }
        for e in &report.remaining {
            eprintln!("  polish left: [{:?}] {}", e.kind, e.detail);
        }
        eprintln!("{}", report.summary());
    }
    report
}

impl<'a> Polisher<'a> {
    fn lane(&self) -> f64 {
        self.tokens.px(1.25)
    }

    fn edge_label(&self, e: EdgeIndex) -> String {
        match self.compiled.graph.edge_endpoints(e) {
            Some((a, b)) => format!("{} -> {}", self.compiled.graph[a].id, self.compiled.graph[b].id),
            None => format!("edge {}", e.index()),
        }
    }

    /// Defect totals over the current paths.
    fn totals(&self) -> Totals {
        let min_jog = self.tokens.polish_min_jog();
        let lane = self.lane();
        let mut t = Totals::default();
        for (k, &a) in self.ids.iter().enumerate() {
            let Some(pa) = self.paths.get(&a) else { continue };
            t.micro_jogs += micro_jog_segments(pa, min_jog).len();
            for &b in &self.ids[k + 1..] {
                let Some(pb) = self.paths.get(&b) else { continue };
                t.merged += usize::from(merged(pa, pb));
                t.crossings += crossing_points(pa, pb).len();
                t.near_parallel += interior_overlaps(pa, pb, lane);
            }
        }
        t
    }

    /// Every defect currently present, in a deterministic order.
    fn scan(&self) -> Vec<Exception> {
        let min_jog = self.tokens.polish_min_jog();
        let lane = self.lane();
        let mut out = Vec::new();
        for (k, &a) in self.ids.iter().enumerate() {
            let Some(pa) = self.paths.get(&a) else { continue };
            let sp = simplified(pa);
            for i in micro_jog_segments(pa, min_jog) {
                out.push(Exception {
                    kind: ExceptionKind::MicroJog,
                    edges: vec![a],
                    at: sp[i],
                    detail: format!("{}: {:.0}px jog at ({:.0},{:.0})", self.edge_label(a), seg_len(sp[i], sp[i + 1]), sp[i].0, sp[i].1),
                });
            }
            for &b in &self.ids[k + 1..] {
                let Some(pb) = self.paths.get(&b) else { continue };
                for at in crossing_points(pa, pb) {
                    out.push(Exception {
                        kind: ExceptionKind::Crossing,
                        edges: vec![a, b],
                        at,
                        detail: format!("{} x {} at ({:.0},{:.0})", self.edge_label(a), self.edge_label(b), at.0, at.1),
                    });
                }
                if interior_overlaps(pa, pb, lane) > 0 {
                    out.push(Exception {
                        kind: ExceptionKind::NearParallel,
                        edges: vec![a, b],
                        at: pa[0],
                        detail: format!("{} || {}", self.edge_label(a), self.edge_label(b)),
                    });
                }
            }
        }
        out
    }

    /// One pass over every edge; returns whether anything was committed.
    fn sweep(&mut self) -> bool {
        let mut changed = false;
        for e in self.ids.clone() {
            if self.plans[&e].searched {
                continue;
            }
            // Bounded retries per edge: each committed fix removes a jog, so this cannot
            // loop, but the cap makes that obvious.
            for _ in 0..8 {
                if !self.try_fix_micro_jog(e) {
                    break;
                }
                changed = true;
            }
        }
        // Crossings, in pair order. Each commit strictly lowers the (crossings-first) score,
        // so the outer loop cannot cycle; the per-pass cap just bounds the work.
        for k in 0..self.ids.len() {
            for j in k + 1..self.ids.len() {
                let (a, b) = (self.ids[k], self.ids[j]);
                let crosses = match (self.paths.get(&a), self.paths.get(&b)) {
                    (Some(pa), Some(pb)) => !crossing_points(pa, pb).is_empty(),
                    _ => false,
                };
                let fixable = !self.plans[&a].searched && !self.plans[&b].searched;
                if crosses && fixable && self.try_fix_crossing(a, b) {
                    changed = true;
                }
            }
        }
        changed
    }

    // -- scoring ------------------------------------------------------------

    /// Score of the edges in `overrides` (using the override paths) against everything else.
    /// Each unordered pair is counted once; the edges' own jogs/bends/length are included.
    fn score_with(&self, overrides: &HashMap<EdgeIndex, Vec<Pt>>) -> Score {
        let min_jog = self.tokens.polish_min_jog();
        let lane = self.lane();
        let mut total = Score::default();
        let mut keys: Vec<&EdgeIndex> = overrides.keys().collect();
        keys.sort_by_key(|e| e.index());
        for &e in keys {
            let pe = &overrides[&e];
            let mut s = Score {
                jogs: micro_jog_segments(pe, min_jog).len(),
                bends: bend_count(pe),
                length: pe.windows(2).map(|w| seg_len(w[0], w[1])).sum(),
                ..Score::default()
            };
            for &o in &self.ids {
                if o == e {
                    continue;
                }
                let po = match overrides.get(&o) {
                    Some(p) => {
                        if o.index() < e.index() {
                            continue; // counted when `o` was the outer edge
                        }
                        p
                    }
                    None => match self.paths.get(&o) {
                        Some(p) => p,
                        None => continue,
                    },
                };
                s.merged += usize::from(merged(pe, po));
                s.crossings += crossing_points(pe, po).len();
                s.overlaps += interior_overlaps(pe, po, lane);
            }
            total.add(&s);
        }
        total
    }

    // -- guard --------------------------------------------------------------

    fn endpoints(&self, e: EdgeIndex) -> Option<(NodeIndex, NodeIndex, &NodeLayout, &NodeLayout)> {
        resolve_edge_layout(self.compiled, self.layout, e)
    }

    /// Fractions of every port currently on `side` of `node`, ignoring edge `skip`'s own,
    /// plus `extra` (the candidate's).
    fn face_ports(&self, node: NodeIndex, side: Side, skip: Option<EdgeIndex>, extra: &[f64]) -> Vec<f64> {
        let mut v: Vec<f64> = extra.to_vec();
        for (&oe, plan) in self.plans.iter() {
            if Some(oe) == skip {
                continue;
            }
            let Some((s, d, _, _)) = self.endpoints(oe) else { continue };
            if s == node && plan.src_side == side {
                v.push(plan.exit_port);
            }
            if d == node && plan.dst_side == side {
                v.push(plan.entry_port);
            }
        }
        v.sort_by(f64::total_cmp);
        v
    }

    fn min_pitch(fracs: &[f64], face_len: f64) -> f64 {
        fracs.windows(2).map(|w| (w[1] - w[0]) * face_len).fold(f64::INFINITY, f64::min)
    }

    /// Whether `plan` is an acceptable replacement for `old` on edge `e`; `Err` names the
    /// first invariant it breaks (shown under `RDG_DEBUG_POLISH`).
    fn guard(&self, e: EdgeIndex, plan: &EdgeRoutingPlan, old: &EdgeRoutingPlan) -> Result<(), &'static str> {
        let Some((s_idx, d_idx, src_nl, dst_nl)) = self.endpoints(e) else { return Err("edge has no layout") };
        let sc = self.tokens.stub_clearance();
        let p1 = port_point(src_nl, plan.src_side, plan.exit_port);
        let p2 = port_point(dst_nl, plan.dst_side, plan.entry_port);
        let mut pts = Vec::with_capacity(plan.waypoints.len() + 2);
        pts.push(p1);
        pts.extend_from_slice(&plan.waypoints);
        pts.push(p2);

        // Orthogonal throughout.
        if pts.windows(2).any(|w| orientation(w[0], w[1]).is_none() && seg_len(w[0], w[1]) > TOL) {
            return Err("not orthogonal");
        }
        // Clears every other node and every group title.
        let mut obstacles: Vec<ObstacleRect> = self
            .obstacles
            .iter()
            .filter(|(n, _)| *n != s_idx && *n != d_idx)
            .map(|(_, r)| r.clone())
            .collect();
        obstacles.extend(self.titles.iter().map(|t| ObstacleRect {
            x: t.min_x,
            y: t.min_y,
            w: t.max_x - t.min_x,
            h: t.max_y - t.min_y,
        }));
        if first_clipping_segment(p1, &plan.waypoints, p2, &obstacles).is_some() {
            return Err("clips a node or group title");
        }
        // Leaves/enters its own faces straight, and never crosses its own endpoints.
        let src_rect = ObstacleRect { x: src_nl.x, y: src_nl.y, w: src_nl.width, h: src_nl.height };
        let dst_rect = ObstacleRect { x: dst_nl.x, y: dst_nl.y, w: dst_nl.width, h: dst_nl.height };
        if !path_respects_faces(p1, plan.src_side, &plan.waypoints, p2, plan.dst_side, &src_rect, &dst_rect) {
            return Err("leaves/enters a face from the wrong side");
        }
        // Stub legs keep at least one stub (or what they already had).
        let old_path = edge_full_path(self.compiled, self.layout, e, old).unwrap_or_default();
        let (np, op) = (simplified(&pts), simplified(&old_path));
        let legs = |p: &[Pt]| {
            let n = p.len();
            if n < 2 { (f64::INFINITY, f64::INFINITY) } else { (seg_len(p[0], p[1]), seg_len(p[n - 2], p[n - 1])) }
        };
        let (nf, nl) = legs(&np);
        let (of, ol) = legs(&op);
        if nf < of.min(sc) - 0.01 || nl < ol.min(sc) - 0.01 {
            return Err("shortens a stub leg");
        }
        // No new fold-back, no extra bend.
        if has_fold_back(&np) && !has_fold_back(&op) {
            return Err("adds a fold-back");
        }
        if np.len() > op.len() {
            return Err("adds a bend");
        }
        let total = |p: &[Pt]| p.windows(2).map(|w| seg_len(w[0], w[1])).sum::<f64>();
        if total(&np) > total(&op) * self.tokens.polish_max_detour_ratio + self.tokens.px(5.0) {
            return Err("detours too far");
        }
        // Ports stay on the face and keep their spacing.
        for (node, side, frac, is_exit) in [
            (s_idx, plan.src_side, plan.exit_port, true),
            (d_idx, plan.dst_side, plan.entry_port, false),
        ] {
            if !(PORT_LO..=PORT_HI).contains(&frac) {
                let old_frac = if is_exit { old.exit_port } else { old.entry_port };
                if (frac - old_frac).abs() > 1e-9 {
                    return Err("port too close to a face corner");
                }
            }
            let nl = if is_exit { src_nl } else { dst_nl };
            let face_len = match side {
                Side::Top | Side::Bottom => nl.width,
                Side::Left | Side::Right => nl.height,
            };
            let old_fracs = self.face_ports(node, side, None, &[]);
            let old_pitch = Self::min_pitch(&old_fracs, face_len);
            // Both of this edge's ports may share a face; count each against the candidate.
            let mut extra = vec![];
            if s_idx == node && plan.src_side == side {
                extra.push(plan.exit_port);
            }
            if d_idx == node && plan.dst_side == side {
                extra.push(plan.entry_port);
            }
            let new_fracs = self.face_ports(node, side, Some(e), &extra);
            let new_pitch = Self::min_pitch(&new_fracs, face_len);
            if new_pitch + 0.01 < old_pitch.min(self.tokens.min_port_pitch()) {
                return Err("crowds a sibling port");
            }
        }
        Ok(())
    }

    // -- fixes --------------------------------------------------------------

    fn full_path_of(&self, e: EdgeIndex, plan: &EdgeRoutingPlan) -> Option<Vec<Pt>> {
        edge_full_path(self.compiled, self.layout, e, plan)
    }

    /// Re-routes `e` with the given ports, keeping its faces and corridor hints.
    fn rerouted(&self, e: EdgeIndex, base: &EdgeRoutingPlan, exit: f64, entry: f64) -> Option<EdgeRoutingPlan> {
        self.rerouted_on(e, base, (base.src_side, exit), (base.dst_side, entry))
    }

    /// Re-routes `e` between the given (face, port) pairs, keeping its corridor hints.
    fn rerouted_on(
        &self,
        e: EdgeIndex,
        base: &EdgeRoutingPlan,
        (src_side, exit): (Side, f64),
        (dst_side, entry): (Side, f64),
    ) -> Option<EdgeRoutingPlan> {
        let (s_idx, d_idx, src_nl, dst_nl) = self.endpoints(e)?;
        let left = self.reroutes_left.get();
        if left == 0 {
            return None;
        }
        self.reroutes_left.set(left - 1);
        let env = RouteEnv {
            all_obstacles: &self.obstacles,
            title_zones: &self.titles,
            algorithm: self.algorithm,
            tokens: self.tokens,
        };
        let waypoints = route_edge(
            &env,
            (s_idx, src_nl, src_side, exit),
            (d_idx, dst_nl, dst_side, entry),
            base.channel_y,
            base.corridor_x,
        );
        let mut plan = base.clone();
        plan.src_side = src_side;
        plan.dst_side = dst_side;
        plan.exit_port = exit;
        plan.entry_port = entry;
        plan.waypoints = waypoints;
        Some(plan)
    }

    /// Fraction delta that moves a port along `side` of `nl` by the component of `v` that
    /// runs along the face, or `None` if `v` isn't lateral to that face.
    fn along_face(nl: &NodeLayout, side: Side, v: Pt) -> Option<f64> {
        let (along, across, len) = match side {
            Side::Top | Side::Bottom => (v.0, v.1, nl.width),
            Side::Left | Side::Right => (v.1, v.0, nl.height),
        };
        // A little skew (routers can leave a 1px x/y mismatch between two corridors) still
        // counts as lateral: the candidate is re-routed from the new port, not patched.
        (across.abs() < SKEW_TOL && len > 1.0).then_some(along / len)
    }

    fn microjog_candidates(&self, e: EdgeIndex, plan: &EdgeRoutingPlan, jog: usize, path: &[Pt]) -> Vec<Candidate> {
        let mut out = Vec::new();
        let Some((_, _, src_nl, dst_nl)) = self.endpoints(e) else { return out };
        let m = path.len();
        let v = (path[jog + 1].0 - path[jog].0, path[jog + 1].1 - path[jog].1);
        let max_shift = self.tokens.polish_max_port_shift();
        let within = |delta_px: f64| delta_px.abs() <= max_shift;

        let exit_delta = (jog == 1).then(|| Self::along_face(src_nl, plan.src_side, v)).flatten();
        let entry_delta = (jog + 3 == m)
            .then(|| Self::along_face(dst_nl, plan.dst_side, (-v.0, -v.1)))
            .flatten();

        let face_px = |nl: &NodeLayout, side: Side, d: f64| match side {
            Side::Top | Side::Bottom => d * nl.width,
            Side::Left | Side::Right => d * nl.height,
        };

        if let Some(d) = exit_delta.filter(|&d| within(face_px(src_nl, plan.src_side, d))) {
            if let Some(p) = self.rerouted(e, plan, plan.exit_port + d, plan.entry_port) {
                out.push(Candidate { plan: p, desc: format!("slid exit port {:+.1}px", face_px(src_nl, plan.src_side, d)) });
            }
        }
        if let Some(d) = entry_delta.filter(|&d| within(face_px(dst_nl, plan.dst_side, d))) {
            if let Some(p) = self.rerouted(e, plan, plan.exit_port, plan.entry_port + d) {
                out.push(Candidate { plan: p, desc: format!("slid entry port {:+.1}px", face_px(dst_nl, plan.dst_side, d)) });
            }
        }
        if let (Some(de), Some(dn)) = (exit_delta, entry_delta) {
            if within(face_px(src_nl, plan.src_side, de) / 2.0) && within(face_px(dst_nl, plan.dst_side, dn) / 2.0) {
                if let Some(p) = self.rerouted(e, plan, plan.exit_port + de / 2.0, plan.entry_port + dn / 2.0) {
                    out.push(Candidate { plan: p, desc: "slid both ports to meet".to_string() });
                }
            }
        }

        // Interior jog (neither neighbour is a stub): slide one side's waypoint pair.
        if jog >= 2 && jog + 3 < m {
            for (label, idx, sign) in [("earlier", jog - 1, 1.0), ("later", jog + 1, -1.0)] {
                let mut q = path.to_vec();
                for k in [idx, idx + 1] {
                    q[k].0 += sign * v.0;
                    q[k].1 += sign * v.1;
                }
                let q = simplified(&q);
                if q.len() < 2 {
                    continue;
                }
                let mut np = plan.clone();
                np.waypoints = q[1..q.len() - 1].to_vec();
                out.push(Candidate { plan: np, desc: format!("slid {label} waypoint pair to absorb jog") });
            }
        }
        out
    }

    /// Faces the author pinned in the YAML (`source_port`/`target_port`): polish may slide a
    /// port along such a face but never moves the edge to another one.
    fn pinned(&self, e: EdgeIndex) -> (Option<Side>, Option<Side>) {
        let d = &self.compiled.graph[e];
        (
            d.source_port.as_deref().and_then(Side::parse),
            d.target_port.as_deref().and_then(Side::parse),
        )
    }

    /// The free fraction on `side` of `node` farthest from every port already there
    /// (ignoring edge `skip`'s own), or `None` if no spot keeps `min_port_pitch`.
    fn free_slot(&self, node: NodeIndex, nl: &NodeLayout, side: Side, skip: EdgeIndex) -> Option<f64> {
        let taken = self.face_ports(node, side, Some(skip), &[]);
        let len = match side {
            Side::Top | Side::Bottom => nl.width,
            Side::Left | Side::Right => nl.height,
        };
        let mut best: Option<(f64, f64)> = None;
        for k in 0..=14 {
            let f = 0.15 + 0.05 * k as f64;
            let gap = taken.iter().map(|t| (t - f).abs() * len).fold(f64::INFINITY, f64::min);
            if best.is_none_or(|(g, _)| gap > g + 1e-9) {
                best = Some((gap, f));
            }
        }
        best.filter(|&(gap, _)| gap >= self.tokens.min_port_pitch()).map(|(_, f)| f)
    }

    fn crossing_candidates(&self, a: EdgeIndex, b: EdgeIndex) -> Vec<(Vec<(EdgeIndex, EdgeRoutingPlan)>, String)> {
        let mut out = Vec::new();
        let (Some(pa), Some(pb)) = (self.plans.get(&a), self.plans.get(&b)) else { return out };
        let (Some((sa, da, _, _)), Some((sb, db, _, _))) = (self.endpoints(a), self.endpoints(b)) else { return out };

        // 1. Two edges sharing a face: swap their ports there and re-route both.
        let a_faces = [(sa, pa.src_side, pa.exit_port, true), (da, pa.dst_side, pa.entry_port, false)];
        let b_faces = [(sb, pb.src_side, pb.exit_port, true), (db, pb.dst_side, pb.entry_port, false)];
        for &(na, fa_side, fa, a_exit) in &a_faces {
            for &(nb, fb_side, fb, b_exit) in &b_faces {
                if na != nb || fa_side != fb_side || (fa - fb).abs() < 1e-6 {
                    continue;
                }
                let (mut ea, mut en_a) = (pa.exit_port, pa.entry_port);
                let (mut eb, mut en_b) = (pb.exit_port, pb.entry_port);
                if a_exit { ea = fb } else { en_a = fb }
                if b_exit { eb = fa } else { en_b = fa }
                if let (Some(qa), Some(qb)) = (self.rerouted(a, pa, ea, en_a), self.rerouted(b, pb, eb, en_b)) {
                    out.push((vec![(a, qa), (b, qb)], format!("swapped ports with {}", self.edge_label(b))));
                }
            }
        }

        // 2. Move one edge to a different pair of faces (respecting pinned faces).
        const SIDES: [Side; 4] = [Side::Top, Side::Bottom, Side::Left, Side::Right];
        for (e, base) in [(a, pa), (b, pb)] {
            let Some((s_idx, d_idx, src_nl, dst_nl)) = self.endpoints(e) else { continue };
            let (pin_s, pin_d) = self.pinned(e);
            for &ss in SIDES.iter().filter(|&&x| pin_s.is_none_or(|p| p == x)) {
                for &ds in SIDES.iter().filter(|&&x| pin_d.is_none_or(|p| p == x)) {
                    if ss == base.src_side && ds == base.dst_side {
                        continue;
                    }
                    let exit = if ss == base.src_side { Some(base.exit_port) } else { self.free_slot(s_idx, src_nl, ss, e) };
                    let entry = if ds == base.dst_side { Some(base.entry_port) } else { self.free_slot(d_idx, dst_nl, ds, e) };
                    let (Some(exit), Some(entry)) = (exit, entry) else { continue };
                    if let Some(q) = self.rerouted_on(e, base, (ss, exit), (ds, entry)) {
                        out.push((vec![(e, q)], format!("moved to {ss:?}->{ds:?}")));
                    }
                }
            }
        }
        out
    }

    /// Tries to remove the crossing between `a` and `b`. Returns whether a fix was committed.
    fn try_fix_crossing(&mut self, a: EdgeIndex, b: EdgeIndex) -> bool {
        let (Some(pa), Some(pb)) = (self.paths.get(&a).cloned(), self.paths.get(&b).cloned()) else { return false };
        let cur = self.score_with(&HashMap::from([(a, pa), (b, pb)]));
        let debug = std::env::var("RDG_DEBUG_POLISH").is_ok();
        for (changes, desc) in self.crossing_candidates(a, b) {
            let mut overrides: HashMap<EdgeIndex, Vec<Pt>> = HashMap::new();
            let mut ok = true;
            for (e, plan) in &changes {
                let old = self.plans[e].clone();
                if let Err(why) = self.guard(*e, plan, &old) {
                    if debug {
                        eprintln!("  polish reject: {} — {desc} ({why})", self.edge_label(*e));
                    }
                    ok = false;
                    break;
                }
                match self.full_path_of(*e, plan) {
                    Some(p) => {
                        overrides.insert(*e, p);
                    }
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if !ok {
                continue;
            }
            // Score the changed edges together with any partner not in this change set, so a
            // one-edge move is judged against the pair's *current* combined score.
            let mut judged = overrides.clone();
            for e in [a, b] {
                if !judged.contains_key(&e) {
                    judged.insert(e, self.paths[&e].clone());
                }
            }
            let new_score = self.score_with(&judged);
            if !new_score.better_than(&cur) {
                continue;
            }
            for (e, plan) in changes {
                self.applied.push(format!("{}: {desc}", self.edge_label(e)));
                self.paths.insert(e, overrides.remove(&e).unwrap());
                self.plans.insert(e, plan);
            }
            return true;
        }
        false
    }

    /// Tries to remove one micro-jog on `e`. Returns whether a fix was committed.
    fn try_fix_micro_jog(&mut self, e: EdgeIndex) -> bool {
        let Some(old_plan) = self.plans.get(&e).cloned() else { return false };
        let Some(cur_path) = self.paths.get(&e).cloned() else { return false };
        let sp = simplified(&cur_path);
        let Some(&jog) = micro_jog_segments(&cur_path, self.tokens.polish_min_jog()).first() else {
            return false;
        };

        let cur_score = self.score_with(&HashMap::from([(e, cur_path.clone())]));
        if std::env::var("RDG_DEBUG_POLISH").is_ok() {
            eprintln!(
                "  polish try: {} jog#{jog} of {} pts, sides {:?}->{:?}, path {:?}",
                self.edge_label(e), sp.len(), old_plan.src_side, old_plan.dst_side,
                sp.iter().map(|p| (p.0.round(), p.1.round())).collect::<Vec<_>>()
            );
        }
        for cand in self.microjog_candidates(e, &old_plan, jog, &sp) {
            let debug = std::env::var("RDG_DEBUG_POLISH").is_ok();
            if let Err(why) = self.guard(e, &cand.plan, &old_plan) {
                if debug {
                    eprintln!("  polish reject: {} — {} ({why})", self.edge_label(e), cand.desc);
                }
                continue;
            }
            let Some(new_path) = self.full_path_of(e, &cand.plan) else { continue };
            let new_score = self.score_with(&HashMap::from([(e, new_path.clone())]));
            if !new_score.better_than(&cur_score) {
                if debug {
                    eprintln!(
                        "  polish reject: {} — {} (not better: {:?} vs {:?})",
                        self.edge_label(e), cand.desc, new_score, cur_score
                    );
                }
                continue;
            }
            self.applied.push(format!("{}: {}", self.edge_label(e), cand.desc));
            self.plans.insert(e, cand.plan);
            self.paths.insert(e, new_path);
            return true;
        }
        false
    }
}


// ---------------------------------------------------------------------------
// Pixel snapping
// ---------------------------------------------------------------------------

/// Snaps every node box, port and waypoint to whole pixels, so the geometry the renderers
/// write is the geometry the router meant. draw.io rounds node `x/y/width/height` to
/// integers on its own but writes ports as fractions of the *rounded* box and waypoints
/// unrounded, which left up to ~0.6px between a port and the line leaving it — a hairline
/// kink on the exact draw.io SVG export.
///
/// Node boxes snap by their edges (`round(x)`, `round(x + w)`) so a face sits on the same
/// pixel the routed ports were computed against; each port then snaps to the nearest pixel
/// on its face; each waypoint coordinate rounds, with coordinates that were equal within
/// tolerance forced equal so straight segments stay straight. An edge whose snapped path
/// would clip a node or leave/enter a face wrongly keeps its original geometry.
///
/// Returns the number of edges snapped. Run after `finalize_canvas` so the canvas shift
/// itself is already whole-pixel.
pub fn snap_to_pixels(
    compiled: &CompiledGraph,
    layout: &mut LayoutResult,
    edge_plans: &mut HashMap<EdgeIndex, EdgeRoutingPlan>,
    tokens: &DesignTokens,
) -> usize {
    if !tokens.polish_enabled || !tokens.polish_snap || layout.sequence_info.is_some() {
        return 0;
    }
    let mut ids: Vec<EdgeIndex> = edge_plans.keys().copied().collect();
    ids.sort_by_key(|e| e.index());

    // Old (pre-snap) absolute paths.
    let old_paths: HashMap<EdgeIndex, Vec<Pt>> = ids
        .iter()
        .filter_map(|&e| edge_full_path(compiled, layout, e, &edge_plans[&e]).map(|p| (e, p)))
        .collect();

    // 1. Nodes, by edges.
    for nl in layout.positions.values_mut() {
        let (x0, y0) = (nl.x.round(), nl.y.round());
        let (x1, y1) = ((nl.x + nl.width).round(), (nl.y + nl.height).round());
        nl.x = x0;
        nl.y = y0;
        nl.width = (x1 - x0).max(1.0);
        nl.height = (y1 - y0).max(1.0);
    }
    let obstacles = node_obstacles(layout);
    let titles = compute_group_title_zones(compiled, layout, tokens);

    // 2. Edges.
    let mut snapped = 0;
    for e in ids {
        let (Some(old), Some((s_idx, d_idx, src_nl, dst_nl))) =
            (old_paths.get(&e), resolve_edge_layout(compiled, layout, e))
        else {
            continue;
        };
        let plan = edge_plans[&e].clone();
        let mut r: Vec<Pt> = old.iter().map(|&(x, y)| (x.round(), y.round())).collect();
        // Coordinates that were the same before must stay the same after: rounding two
        // values that differ by 0.02 can split them across a .5 boundary.
        let n = r.len();
        for pass in 0..2 {
            let order: Vec<usize> = if pass == 0 { (1..n).collect() } else { (0..n - 1).rev().collect() };
            for i in order {
                let j = if pass == 0 { i - 1 } else { i + 1 };
                if (old[i].0 - old[j].0).abs() < SKEW_TOL && (r[i].0 - r[j].0).abs() > 0.0 && i != 0 && i != n - 1 {
                    r[i].0 = r[j].0;
                }
                if (old[i].1 - old[j].1).abs() < SKEW_TOL && (r[i].1 - r[j].1).abs() > 0.0 && i != 0 && i != n - 1 {
                    r[i].1 = r[j].1;
                }
            }
        }
        let (p1, p2) = (r[0], r[n - 1]);

        // Port fractions such that the drawn port is exactly p1/p2 on the snapped faces.
        let frac = |p: Pt, side: Side, nl: &NodeLayout| -> Option<f64> {
            let (v, lo, len) = match side {
                Side::Top | Side::Bottom => (p.0, nl.x, nl.width),
                Side::Left | Side::Right => (p.1, nl.y, nl.height),
            };
            let f = (v - lo) / len;
            (0.0..=1.0).contains(&f).then_some(f)
        };
        let (Some(exit), Some(entry)) = (frac(p1, plan.src_side, src_nl), frac(p2, plan.dst_side, dst_nl)) else {
            continue;
        };
        let mut np = plan.clone();
        np.exit_port = exit;
        np.entry_port = entry;
        np.waypoints = r[1..n - 1].to_vec();

        // The port must land exactly on its face (it does by construction for the face
        // coordinate; verify), and the path must still be clean.
        if (port_point(src_nl, np.src_side, exit).0 - p1.0).abs() > 1e-6
            || (port_point(src_nl, np.src_side, exit).1 - p1.1).abs() > 1e-6
            || (port_point(dst_nl, np.dst_side, entry).0 - p2.0).abs() > 1e-6
            || (port_point(dst_nl, np.dst_side, entry).1 - p2.1).abs() > 1e-6
        {
            continue;
        }
        let mut pts = vec![p1];
        pts.extend_from_slice(&np.waypoints);
        pts.push(p2);
        if pts.windows(2).any(|w| orientation(w[0], w[1]).is_none() && seg_len(w[0], w[1]) > TOL) {
            continue;
        }
        let mut obs: Vec<ObstacleRect> = obstacles
            .iter()
            .filter(|(n, _)| *n != s_idx && *n != d_idx)
            .map(|(_, r)| r.clone())
            .collect();
        obs.extend(titles.iter().map(|t| ObstacleRect { x: t.min_x, y: t.min_y, w: t.max_x - t.min_x, h: t.max_y - t.min_y }));
        let src_rect = ObstacleRect { x: src_nl.x, y: src_nl.y, w: src_nl.width, h: src_nl.height };
        let dst_rect = ObstacleRect { x: dst_nl.x, y: dst_nl.y, w: dst_nl.width, h: dst_nl.height };
        if first_clipping_segment(p1, &np.waypoints, p2, &obs).is_some()
            || !path_respects_faces(p1, np.src_side, &np.waypoints, p2, np.dst_side, &src_rect, &dst_rect)
        {
            continue;
        }
        edge_plans.insert(e, np);
        snapped += 1;
    }
    snapped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_micro_jog_detected_only_between_stubs_and_below_threshold() {
        // Exit stub down, 6px lateral step, run down, ...: the 6px step is a jog.
        let p = [(0.0, 0.0), (0.0, 24.0), (6.0, 24.0), (6.0, 80.0)];
        assert_eq!(micro_jog_segments(&p, 12.0), vec![1]);
        // A 40px step is a deliberate bend.
        let q = [(0.0, 0.0), (0.0, 24.0), (40.0, 24.0), (40.0, 80.0)];
        assert!(micro_jog_segments(&q, 12.0).is_empty());
        // A straight 2-point line and an L have no interior segment.
        assert!(micro_jog_segments(&[(0.0, 0.0), (0.0, 50.0)], 12.0).is_empty());
        assert!(micro_jog_segments(&[(0.0, 0.0), (0.0, 50.0), (30.0, 50.0)], 12.0).is_empty());
    }

    #[test]
    fn test_crossing_points_perpendicular_only() {
        let a = [(0.0, 50.0), (100.0, 50.0)];
        let b = [(50.0, 0.0), (50.0, 100.0)];
        assert_eq!(crossing_points(&a, &b), vec![(50.0, 50.0)]);
        // T-junction at the end of one segment is not a crossing.
        let c = [(50.0, 50.0), (50.0, 100.0)];
        assert!(crossing_points(&a, &c).is_empty());
        // Parallel segments never cross.
        let d = [(0.0, 60.0), (100.0, 60.0)];
        assert!(crossing_points(&a, &d).is_empty());
    }

    // -- property tests over the bundled diagrams ----------------------------------

    use crate::review::{compute_reviewed_layout, spacing_metrics};
    use rdg_graph::build_graph;
    use rdg_layout::LayoutConfig;
    use rdg_schema::DiagramPayload;

    const FIXTURES: &[(&str, &str)] = &[
        ("social_graph_dense", include_str!("../../../examples/social_graph_dense/social_graph_dense.yaml")),
        ("netflix_architecture", include_str!("../../../examples/netflix_architecture/netflix_architecture.yaml")),
        ("kubernetes_cluster", include_str!("../../../examples/kubernetes_cluster/kubernetes_cluster.yaml")),
        ("youtube_architecture", include_str!("../../../examples/youtube_architecture/youtube_architecture.yaml")),
        ("architecture", include_str!("../../../docs/architecture.yaml")),
        ("ecommerce_erd", include_str!("../../../examples/ecommerce_erd/ecommerce_erd.yaml")),
    ];

    /// Lays out `yaml` with polish enabled or not, returning everything needed to re-run it.
    fn reviewed(yaml: &str, polish_on: bool) -> (CompiledGraph, crate::review::ReviewedLayout, LayoutConfig, rdg_dispatch::AlgorithmDecision) {
        let compiled = build_graph(&DiagramPayload::from_yaml(yaml).unwrap()).unwrap();
        let mut config = LayoutConfig::default();
        config.tokens.polish_enabled = polish_on;
        let topo = rdg_dispatch::analyze(&compiled, &config.tokens);
        let decision = rdg_dispatch::dispatch(&topo, &config.tokens);
        let r = compute_reviewed_layout(&compiled, &config, config.tokens.max_review_passes, &decision).unwrap();
        (compiled, r, config, decision)
    }

    #[test]
    fn test_polish_never_worsens_any_measure() {
        for (name, yaml) in FIXTURES {
            let (c0, off, cfg, _) = reviewed(yaml, false);
            let (c1, on, _, _) = reviewed(yaml, true);
            let m0 = spacing_metrics(&c0, &off.layout, &off.edge_plans, &cfg.tokens);
            let m1 = spacing_metrics(&c1, &on.layout, &on.edge_plans, &cfg.tokens);
            assert!(m1.micro_jogs <= m0.micro_jogs, "{name}: jogs {} -> {}", m0.micro_jogs, m1.micro_jogs);
            assert!(m1.crossings <= m0.crossings, "{name}: crossings {} -> {}", m0.crossings, m1.crossings);
            // Polish must not eat into the spacing guarantees layout and routing established.
            assert!(m1.min_arrow_len.unwrap_or(f64::MAX) + 0.01 >= m0.min_arrow_len.unwrap_or(0.0).min(cfg.tokens.stub_clearance()), "{name}: shortest arrow shrank");
            assert!(m1.min_port_pitch.unwrap_or(f64::MAX) + 0.01 >= m0.min_port_pitch.unwrap_or(0.0).min(cfg.tokens.min_port_pitch()), "{name}: port pitch shrank");
            let gap = |m: &crate::review::SpacingMetrics| m.min_node_gap.unwrap_or(0.0);
            // Nodes never move except by whole-pixel snapping, which shifts a box edge by at
            // most half a pixel — so a gap between two boxes changes by at most 1px.
            assert!((gap(&m1) - gap(&m0)).abs() <= 1.0 + 1e-6, "{name}: polish moved a node ({} -> {})", gap(&m0), gap(&m1));
        }
    }

    #[test]
    fn test_polish_is_idempotent() {
        for (name, yaml) in FIXTURES {
            let (compiled, mut r, cfg, decision) = reviewed(yaml, true);
            let before = r.edge_plans.iter().map(|(k, p)| (*k, p.waypoints.clone(), p.exit_port, p.entry_port)).collect::<Vec<_>>();
            let again = polish(&compiled, &mut r.layout, &mut r.edge_plans, decision.routing.algorithm, &cfg.tokens);
            assert!(again.applied.is_empty(), "{name}: second polish still applied {:?}", again.applied);
            let mut after = r.edge_plans.iter().map(|(k, p)| (*k, p.waypoints.clone(), p.exit_port, p.entry_port)).collect::<Vec<_>>();
            let mut before = before;
            before.sort_by_key(|e| e.0.index());
            after.sort_by_key(|e| e.0.index());
            assert_eq!(before, after, "{name}: second polish changed the plans");
        }
    }

    #[test]
    fn test_disabled_polish_is_a_noop() {
        let (compiled, mut r, mut cfg, decision) = reviewed(FIXTURES[0].1, false);
        cfg.tokens.polish_enabled = false;
        let snapshot: Vec<_> = r.edge_plans.iter().map(|(k, p)| (k.index(), p.waypoints.clone())).collect();
        let rep = polish(&compiled, &mut r.layout, &mut r.edge_plans, decision.routing.algorithm, &cfg.tokens);
        assert!(rep.applied.is_empty() && rep.remaining.is_empty());
        let mut now: Vec<_> = r.edge_plans.iter().map(|(k, p)| (k.index(), p.waypoints.clone())).collect();
        let mut snapshot = snapshot;
        now.sort_by_key(|e| e.0);
        snapshot.sort_by_key(|e| e.0);
        assert_eq!(snapshot, now);
    }

    #[test]
    fn test_score_is_lexicographic_with_length_last() {
        let base = Score { merged: 0, crossings: 1, jogs: 0, overlaps: 0, bends: 2, length: 100.0 };
        let fewer_crossings = Score { merged: 0, crossings: 0, jogs: 5, overlaps: 5, bends: 9, length: 900.0 };
        assert!(fewer_crossings.better_than(&base), "crossings dominate everything below them");
        let shorter = Score { length: 90.0, ..base };
        assert!(shorter.better_than(&base));
        let noise = Score { length: 99.8, ..base };
        assert!(!noise.better_than(&base), "sub-epsilon length change is not an improvement");
        assert!(!base.better_than(&base));
    }

    #[test]
    fn test_merged_lines_outrank_a_crossing() {
        // Trading a crossing for two edges drawn on top of each other must never look like
        // an improvement — that exact trade is what an earlier version of the score made.
        let crossing = Score { crossings: 1, ..Score::default() };
        let merged_pair = Score { merged: 1, ..Score::default() };
        assert!(crossing.better_than(&merged_pair));
        assert!(!merged_pair.better_than(&crossing));
    }

    #[test]
    fn test_merged_detects_stub_overlap_but_not_adjacent_ports() {
        // Two edges sharing a vertical run (including a stub) are merged...
        let a = [(50.0, 0.0), (50.0, 100.0)];
        let b = [(50.0, 40.0), (50.0, 140.0)];
        assert!(merged(&a, &b));
        // ...but stubs one port pitch apart (12px) are separate lines.
        let c = [(62.0, 0.0), (62.0, 100.0)];
        assert!(!merged(&a, &c));
    }
}
