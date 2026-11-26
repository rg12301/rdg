//! Spreads parallel runs of *different* arrows a lane apart.
//!
//! Both routers keep two arrows off one line, but a lane is only a soft preference to
//! the grid search (its grid lines sit wherever ports and lanes happen to fall, often
//! half a lane apart) and the channel router packs lanes per gutter without seeing
//! the search's stubs. The result a reader notices: two lines 9px apart for 300px,
//! which read as one thick line — or as a ribbon whose strands can't be followed.
//!
//! This pass runs on the finished plans. Every leg may slide sideways: an interior
//! leg between its two bends, an end leg by sliding its port along the face it meets
//! (within the face's port range, a pitch from the face's other ports). Legs of
//! different arrows that run alongside each other for a real stretch are pushed to a
//! full lane apart, each moved by as little as possible and never into a node, a
//! title, a group border's band, or a new crossing.

use std::collections::HashMap;

use petgraph::stable_graph::{EdgeIndex, NodeIndex};

use rdg_graph::CompiledGraph;
use rdg_layout::{DesignTokens, LayoutResult};

use crate::routing::{EdgeRoutingPlan, Side, edge_full_path, resolve_edge_layout};

type Pt = (f64, f64);

/// Two legs must run side by side at least this far before they read as parallel.
pub(crate) const MIN_PARALLEL: f64 = 12.0;
/// Side by side this far, two legs want more than a lane between them.
const LONG_RUN: f64 = 160.0;

/// One leg of one path, in the axis it can slide along.
#[derive(Debug, Clone, Copy)]
struct Leg {
    path: usize,
    /// Segment index: from `pts[k]` to `pts[k + 1]`.
    k: usize,
    /// Last segment of the run this leg stands for: `k` for one segment; `k + 2` when
    /// two legs on this axis joined by a jog shorter than a readable one move as one
    /// (moving only half of such a pair leaves the other half where it was).
    k2: usize,
    /// The coordinate across the leg (x for a vertical leg).
    at: f64,
    /// Extent along the leg.
    lo: f64,
    hi: f64,
    /// Where it may slide to; `(at, at)` when it can't.
    range: (f64, f64),
}

impl Leg {
    fn movable(&self) -> bool {
        self.range.1 - self.range.0 > 0.5
    }
}

fn overlap(a: &Leg, b: &Leg) -> f64 {
    a.hi.min(b.hi) - a.lo.max(b.lo)
}

/// A port on one face of one node: `(node, side, coordinate along the face)`.
#[derive(Debug, Clone, Copy)]
struct Port {
    node: NodeIndex,
    side: Side,
    at: f64,
}

/// What the pass knows about every path besides its points.
pub(crate) struct Ends {
    /// Start and end port of each path.
    ports: Vec<[Port; 2]>,
    /// Allowed span of each path's start / end port along its face, absolute.
    spans: Vec<[(f64, f64); 2]>,
    /// Port pitch on each path's start / end face.
    pitch: Vec<[f64; 2]>,
}

impl Ends {
    /// The interval the port of path `pi` (start when `start`) may take: its face's
    /// range, kept a pitch from every other port on the same face.
    fn port_range(&self, pi: usize, start: bool) -> (f64, f64) {
        let which = usize::from(!start);
        let me = self.ports[pi][which];
        let (mut lo, mut hi) = self.spans[pi][which];
        let pitch = self.pitch[pi][which];
        for (pj, ps) in self.ports.iter().enumerate() {
            for (w, other) in ps.iter().enumerate() {
                if (pj == pi && w == which) || other.node != me.node || other.side != me.side {
                    continue;
                }
                if other.at <= me.at {
                    lo = lo.max(other.at + pitch);
                } else {
                    hi = hi.min(other.at - pitch);
                }
            }
        }
        if lo <= hi { (lo.min(me.at), hi.max(me.at)) } else { (me.at, me.at) }
    }
}

/// Keeps the leg's neighbour pointing the same way with a readable length: a stub
/// its clearance, any other leg the jog minimum (or what it has now, when shorter).
fn neighbour_bound(at: f64, other: f64, is_stub: bool, min_len: f64, stub: f64, lo: &mut f64, hi: &mut f64) {
    let want = if is_stub { stub } else { min_len };
    let m = want.min((at - other).abs()).max(1.0);
    if at > other {
        *lo = lo.max(other + m);
    } else {
        *hi = hi.min(other - m);
    }
}

/// The legs of every path along `vertical`, with their sliding ranges.
fn legs(paths: &[Vec<Pt>], ends: Option<&Ends>, vertical: bool, min_len: f64, stub: f64) -> Vec<Leg> {
    let axis = |p: Pt| if vertical { p.0 } else { p.1 };
    let along = |p: Pt| if vertical { p.1 } else { p.0 };
    let mut out = Vec::new();
    for (pi, pts) in paths.iter().enumerate() {
        let n = pts.len();
        for k in 0..n.saturating_sub(1) {
            let (a, b) = (pts[k], pts[k + 1]);
            let is_vertical = (a.0 - b.0).abs() < 0.5 && (a.1 - b.1).abs() >= 0.5;
            let is_horizontal = (a.1 - b.1).abs() < 0.5 && (a.0 - b.0).abs() >= 0.5;
            if (vertical && !is_vertical) || (!vertical && !is_horizontal) {
                continue;
            }
            let at = axis(a);
            let (lo, hi) = (along(a).min(along(b)), along(a).max(along(b)));
            let (mut lo_r, mut hi_r) = (f64::NEG_INFINITY, f64::INFINITY);
            let mut fixed = n < 3;
            if k == 0 {
                // An end leg slides by its port, along the face it meets — a vertical
                // leg leaves a top or bottom face.
                match ends {
                    Some(e) if matches!(e.ports[pi][0].side, Side::Top | Side::Bottom) == vertical => {
                        let r = e.port_range(pi, true);
                        lo_r = lo_r.max(r.0);
                        hi_r = hi_r.min(r.1);
                    }
                    _ => fixed = true,
                }
            } else {
                // The leg before is a real stub only when it stands off its face: a
                // leg running along the face it ends on is a jog, not a stub.
                let real_stub = k - 1 == 0 && ends.is_none_or(|e| matches!(e.ports[pi][0].side, Side::Left | Side::Right) == vertical);
                neighbour_bound(at, axis(pts[k - 1]), real_stub, min_len, stub, &mut lo_r, &mut hi_r);
            }
            if k + 2 == n {
                match ends {
                    Some(e) if matches!(e.ports[pi][1].side, Side::Top | Side::Bottom) == vertical => {
                        let r = e.port_range(pi, false);
                        lo_r = lo_r.max(r.0);
                        hi_r = hi_r.min(r.1);
                    }
                    _ => fixed = true,
                }
            } else {
                let real_stub = k + 2 == n - 1 && ends.is_none_or(|e| matches!(e.ports[pi][1].side, Side::Left | Side::Right) == vertical);
                neighbour_bound(at, axis(pts[k + 2]), real_stub, min_len, stub, &mut lo_r, &mut hi_r);
            }
            let range = if fixed || lo_r > hi_r { (at, at) } else { (lo_r.min(at), hi_r.max(at)) };
            out.push(Leg { path: pi, k, k2: k, at, lo, hi, range });
        }
    }
    // Two legs of one path on this axis joined by a jog shorter than a readable one are
    // one leg to the pass: the span of both, moved together within both ranges.
    if min_len > 0.0 {
        let mut merged: Vec<Leg> = Vec::with_capacity(out.len());
        let mut i = 0;
        while i < out.len() {
            let a = out[i];
            if let Some(b) = out.get(i + 1).copied() {
                let jog = (a.at - b.at).abs();
                if b.path == a.path && b.k == a.k + 2 && jog < min_len && jog > 0.5 && a.movable() && b.movable() {
                    let at = if a.hi - a.lo >= b.hi - b.lo { a.at } else { b.at };
                    let (lo_r, hi_r) = (a.range.0.max(b.range.0), a.range.1.min(b.range.1));
                    let range = if lo_r > hi_r { (at, at) } else { (lo_r.min(at), hi_r.max(at)) };
                    merged.push(Leg { path: a.path, k: a.k, k2: b.k, at, lo: a.lo.min(b.lo), hi: a.hi.max(b.hi), range });
                    i += 2;
                    continue;
                }
            }
            merged.push(a);
            i += 1;
        }
        out = merged;
    }
    out
}

/// Whether two legs of different paths run side by side (overlap along their length
/// by [`MIN_PARALLEL`]) without sharing a *port* — two arrows forking from one port,
/// or joining at one, share a leg on purpose. Legs meeting at a bend are not excused:
/// two arrows that cross, turn at one point and run on as one line are the worst
/// kind of close pair.
fn alongside(paths: &[Vec<Pt>], a: &Leg, b: &Leg) -> bool {
    if a.path == b.path || overlap(a, b) < MIN_PARALLEL {
        return false;
    }
    let (pa, pb) = (&paths[a.path], &paths[b.path]);
    let same = |p: Pt, q: Pt| (p.0 - q.0).abs() < 1.5 && (p.1 - q.1).abs() < 1.5;
    let shared_port = |p: Pt| same(p, pa[0]) || same(p, pa[pa.len() - 1]) || same(p, pb[0]) || same(p, pb[pb.len() - 1]);
    let ends_a = [pa[a.k], pa[a.k + 1]];
    let ends_b = [pb[b.k], pb[b.k + 1]];
    !ends_a.iter().any(|&p| ends_b.iter().any(|&q| same(p, q) && shared_port(p)))
}

/// One pair of legs of different arrows running side by side closer than a lane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ClosePair {
    pub a: usize,
    pub b: usize,
    /// The legs (segment indices) of `a` and `b`.
    pub ka: usize,
    pub kb: usize,
    /// Distance across and overlap along the legs.
    pub dist: f64,
    pub overlap: f64,
    /// The middle of the shared run.
    pub at: Pt,
}

/// Pairs of legs of different arrows running side by side closer than `pitch`, for
/// the metrics.
pub(crate) fn close_pairs(paths: &[Vec<Pt>], pitch: f64) -> Vec<ClosePair> {
    let mut out = Vec::new();
    for vertical in [true, false] {
        let ls = legs(paths, None, vertical, 0.0, 0.0);
        for (i, a) in ls.iter().enumerate() {
            for b in &ls[i + 1..] {
                let d = (a.at - b.at).abs();
                if d < pitch - 1.0 && alongside(paths, a, b) {
                    let across = (a.at + b.at) / 2.0;
                    let along = (a.lo.max(b.lo) + a.hi.min(b.hi)) / 2.0;
                    let at = if vertical { (across, along) } else { (along, across) };
                    let (a, b) = if a.path <= b.path { (a, b) } else { (b, a) };
                    out.push(ClosePair { a: a.path, b: b.path, ka: a.k, kb: b.k, dist: d, overlap: overlap(a, b), at });
                }
            }
        }
    }
    out
}

/// Axis-aligned box.
#[derive(Debug, Clone, Copy)]
struct Box2 {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Box2 {
    /// Whether the leg at `at` spanning `lo..hi` (vertical or not) passes through.
    fn crossed_by(&self, vertical: bool, at: f64, lo: f64, hi: f64) -> bool {
        if vertical {
            at > self.x0 && at < self.x1 && hi > self.y0 && lo < self.y1
        } else {
            at > self.y0 && at < self.y1 && hi > self.x0 && lo < self.x1
        }
    }
}

/// Whether the neighbour leg that joins the sliding leg at `corner` — running across
/// the slide axis from `from` to the leg's new place `at` — would pass through `b`.
fn neighbour_crosses(b: &Box2, vertical: bool, corner: Pt, from: f64, at: f64) -> bool {
    let (lo, hi) = (from.min(at), from.max(at));
    b.crossed_by(!vertical, if vertical { corner.1 } else { corner.0 }, lo, hi)
}

/// Right-angle crossings between two paths.
fn crossings(a: &[Pt], b: &[Pt]) -> usize {
    crate::polish::crossing_points(a, b).len()
}

/// Port pitch on a face: a lane, or on a small mark (a tile, a logo) what the mark
/// allows — the rule the channel router assigns ports by. (A pitch from the count of
/// ports on the face, a full lane when it fits, was measured worse.)
fn face_pitch(outline: &crate::style::Outline, lane: f64) -> f64 {
    match outline {
        crate::style::Outline::Captioned { mark, .. } => lane.min(mark / 3.5).max(8.0),
        _ => lane,
    }
}

/// Every plan with a path: the paths, their edges, and their ends (ports, spans and
/// pitches). Plans without a path (self-loops, missing ends) are left out.
pub(crate) fn collect_paths(compiled: &CompiledGraph, layout: &LayoutResult, plans: &HashMap<EdgeIndex, EdgeRoutingPlan>, tokens: &DesignTokens) -> (Vec<Vec<Pt>>, Vec<EdgeIndex>, Ends) {
    let lane = tokens.channel_pitch().max(10.0);
    let mut ids: Vec<EdgeIndex> = plans.keys().copied().collect();
    ids.sort_by_key(|e| e.index());
    let mut paths: Vec<Vec<Pt>> = Vec::new();
    let mut path_edge: Vec<EdgeIndex> = Vec::new();
    let mut ends = Ends { ports: Vec::new(), spans: Vec::new(), pitch: Vec::new() };
    for e in ids {
        let plan = &plans[&e];
        let (Some(p), Some((s, d, snl, dnl))) = (edge_full_path(compiled, layout, e, plan), resolve_edge_layout(compiled, layout, e)) else {
            continue;
        };
        let along = |side: Side, q: Pt| if matches!(side, Side::Top | Side::Bottom) { q.0 } else { q.1 };
        let span = |n: NodeIndex, nl: &rdg_layout::NodeLayout, side: Side| {
            let outline = crate::style::outline_of(&compiled.graph[n], tokens);
            let (lo, hi) = crate::ortho::port_range(&outline, side, nl);
            let (lo, hi) = if lo > hi { ((lo + hi) / 2.0, (lo + hi) / 2.0) } else { (lo, hi) };
            let (start, len) = if matches!(side, Side::Top | Side::Bottom) { (nl.x, nl.width) } else { (nl.y, nl.height) };
            ((start + lo * len, start + hi * len), face_pitch(&outline, lane))
        };
        let (ss, sp) = span(s, snl, plan.src_side);
        let (ds, dp) = span(d, dnl, plan.dst_side);
        ends.ports.push([
            Port { node: s, side: plan.src_side, at: along(plan.src_side, p[0]) },
            Port { node: d, side: plan.dst_side, at: along(plan.dst_side, p[p.len() - 1]) },
        ]);
        ends.spans.push([ss, ds]);
        ends.pitch.push([sp, dp]);
        paths.push(p);
        path_edge.push(e);
    }
    (paths, path_edge, ends)
}

/// [`close_pairs`] over the plans, less the pairs that are so by design: two end legs
/// off one face of one node, standing the pitch apart that face allows. Each pair
/// names its edges.
pub(crate) fn close_pairs_of(compiled: &CompiledGraph, layout: &LayoutResult, plans: &HashMap<EdgeIndex, EdgeRoutingPlan>, tokens: &DesignTokens) -> Vec<(EdgeIndex, EdgeIndex, ClosePair)> {
    let (paths, path_edge, ends) = collect_paths(compiled, layout, plans, tokens);
    let pitch = tokens.channel_pitch().max(10.0);
    close_pairs(&paths, pitch)
        .into_iter()
        .filter(|c| {
            let end = |pi: usize, k: usize| -> Option<(Port, f64)> {
                let n = paths[pi].len();
                if k == 0 {
                    Some((ends.ports[pi][0], ends.pitch[pi][0]))
                } else if k + 2 == n {
                    Some((ends.ports[pi][1], ends.pitch[pi][1]))
                } else {
                    None
                }
            };
            match (end(c.a, c.ka), end(c.b, c.kb)) {
                (Some((pa, pitch_a)), Some((pb, _))) if pa.node == pb.node && pa.side == pb.side => c.dist < pitch_a - 1.0,
                _ => true,
            }
        })
        .map(|c| (path_edge[c.a], path_edge[c.b], c))
        .collect()
}

/// Spreads the plans' parallel legs apart (see the module doc). Plans without a path
/// (self-loops, missing ends) are left alone.
pub fn spread_parallels(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    plans: &mut HashMap<EdgeIndex, EdgeRoutingPlan>,
    tokens: &DesignTokens,
) {
    // A px over the lane: positions are rounded to whole px, and a pair rounded to a
    // hair under the lane would still count as close.
    let pitch = tokens.channel_pitch().max(10.0) + 1.0;
    let min_len = tokens.polish_min_jog();
    let stub = tokens.stub_clearance();
    let lane_margin = pitch;
    let border_lane = pitch * 1.25;

    let (mut paths, path_edge, mut ends) = collect_paths(compiled, layout, plans, tokens);
    if paths.len() < 2 {
        return;
    }

    let mut node_ids: Vec<_> = layout.positions.keys().copied().collect();
    node_ids.sort();
    let mut boxes: Vec<Box2> = node_ids
        .iter()
        .map(|n| {
            let nl = &layout.positions[n];
            Box2 { x0: nl.x - lane_margin, y0: nl.y - lane_margin, x1: nl.x + nl.width + lane_margin, y1: nl.y + nl.height + lane_margin }
        })
        .collect();
    boxes.extend(
        crate::routing::compute_group_title_zones(compiled, layout, tokens)
            .iter()
            .map(|t| Box2 { x0: t.min_x - 2.0, y0: t.min_y - 2.0, x1: t.max_x + 2.0, y1: t.max_y + 2.0 }),
    );
    // A band either side of every group border line: `(vertical line?, at, lo, hi)`.
    let mut borders: Vec<(bool, f64, f64, f64)> = Vec::new();
    for (x, y, w, h) in rdg_layout::groups::group_rects(compiled, layout, tokens).into_iter().flatten() {
        borders.push((true, x, y, y + h));
        borders.push((true, x + w, y, y + h));
        borders.push((false, y, x, x + w));
        borders.push((false, y + h, x, x + w));
    }
    let debug = std::env::var_os("RDG_DEBUG_SPREAD").is_some();
    let mut moved_any = false;
    let mut paths_out: Option<Vec<Vec<Pt>>> = None;

    for round in 0..4 {
        let vertical = round % 2 == 0;
        let mut ls = legs(&paths, Some(&ends), vertical, min_len, stub);
        // A leg's range stops at the nearest box or border band on either side: the
        // relaxation then shares a deficit only between moves that can be made,
        // instead of pushing one leg into a wall and leaving the pair as it was.
        for l in &mut ls {
            if !l.movable() {
                continue;
            }
            let (mut lo, mut hi) = l.range;
            for b in &boxes {
                let (s0, s1, c0, c1) = if vertical { (b.y0, b.y1, b.x0, b.x1) } else { (b.x0, b.x1, b.y0, b.y1) };
                if s1 <= l.lo || s0 >= l.hi || (l.at > c0 && l.at < c1) {
                    continue;
                }
                if c1 <= l.at {
                    lo = lo.max(c1);
                } else {
                    hi = hi.min(c0);
                }
            }
            l.range = (lo.min(l.at), hi.max(l.at));
        }
        // Border bands a leg must not land in: the packing hops over them.
        let bands_for = |l: &Leg| -> Vec<(f64, f64)> {
            borders
                .iter()
                .filter(|b| b.0 == vertical && l.hi.min(b.3) - l.lo.max(b.2) > MIN_PARALLEL && (b.1 - l.at).abs() >= border_lane)
                .map(|b| (b.1 - border_lane, b.1 + border_lane))
                .collect()
        };
        let hop = |l: &Leg, v: f64, up: bool| -> f64 {
            let mut v = v;
            for _ in 0..4 {
                match bands_for(l).into_iter().find(|&(a, b)| v > a && v < b) {
                    Some((a, b)) => v = if up { b } else { a },
                    None => break,
                }
            }
            v
        };
        let mut target: Vec<f64> = ls.iter().map(|l| l.at).collect();
        // Two legs running together for a long stretch read as a ribbon at one lane;
        // they get half a lane more.
        let need = |a: &Leg, b: &Leg| if overlap(a, b) > LONG_RUN { pitch * 1.5 } else { pitch };
        // Clusters of legs that run alongside one another, chained; each is packed
        // left to right (or top to bottom) within every leg's own range: forward,
        // each leg a lane past the one before; backward, pulling legs that overshot
        // their range back and the ones before them with it. A fixed leg anchors.
        let mut parent: Vec<usize> = (0..ls.len()).collect();
        fn find(p: &mut [usize], i: usize) -> usize {
            let mut r = i;
            while p[r] != r {
                r = p[r];
            }
            let mut j = i;
            while p[j] != r {
                let n = p[j];
                p[j] = r;
                j = n;
            }
            r
        }
        let mut any_close = false;
        for i in 0..ls.len() {
            for j in i + 1..ls.len() {
                if alongside(&paths, &ls[i], &ls[j]) && (ls[i].at - ls[j].at).abs() < need(&ls[i], &ls[j]) + pitch {
                    if (ls[i].at - ls[j].at).abs() < need(&ls[i], &ls[j]) - 0.5 {
                        any_close = true;
                    }
                    let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                    if a != b {
                        parent[a] = b;
                    }
                }
            }
        }
        if debug {
            for i in 0..ls.len() {
                for j in i + 1..ls.len() {
                    if alongside(&paths, &ls[i], &ls[j]) && (ls[i].at - ls[j].at).abs() < need(&ls[i], &ls[j]) - 0.5 {
                        eprintln!("pair: path {} leg {} at {:.0} range {:.0}..{:.0} | path {} leg {} at {:.0} range {:.0}..{:.0}", ls[i].path, ls[i].k, ls[i].at, ls[i].range.0, ls[i].range.1, ls[j].path, ls[j].k, ls[j].at, ls[j].range.0, ls[j].range.1);
                    }
                }
            }
        }
        // A leg running along a group border within the border lane (and not on it)
        // moves off to the band's edge on its own side: a line that close to a border
        // reads as part of the box.
        let mut any_hug = false;
        let mut hug_target: Vec<Option<f64>> = vec![None; ls.len()];
        for (i, l) in ls.iter().enumerate() {
            if !l.movable() {
                continue;
            }
            for b in &borders {
                let d = l.at - b.1;
                if b.0 == vertical && l.hi.min(b.3) - l.lo.max(b.2) > MIN_PARALLEL && d.abs() < border_lane && d.abs() > 0.5 {
                    let want = (b.1 + d.signum() * border_lane).clamp(l.range.0, l.range.1);
                    if (want - l.at).abs() > 0.5 {
                        hug_target[i] = Some(want);
                        any_hug = true;
                    }
                }
            }
        }
        if !any_close && !any_hug {
            continue;
        }
        let mut clusters: HashMap<usize, Vec<usize>> = HashMap::new();
        for i in 0..ls.len() {
            let r = find(&mut parent, i);
            clusters.entry(r).or_default().push(i);
        }
        let mut roots: Vec<usize> = clusters.keys().copied().collect();
        roots.sort_unstable();
        for r in roots {
            let mut members = clusters[&r].clone();
            if members.len() < 2 {
                continue;
            }
            let gap_to = |i: usize, j: usize| if alongside(&paths, &ls[i], &ls[j]) { need(&ls[i], &ls[j]) } else { 0.0 };
            let pack = |members: &[usize]| -> Vec<f64> {
                let mut x: Vec<f64> = members.iter().map(|&i| ls[i].at).collect();
                for k in 0..members.len() {
                    let i = members[k];
                    let mut v = ls[i].range.0.max(ls[i].at.min(ls[i].range.1));
                    if k > 0 {
                        // Past every earlier leg it runs alongside, not just the previous.
                        for q in 0..k {
                            v = v.max(x[q] + gap_to(members[q], i));
                        }
                    }
                    x[k] = hop(&ls[i], v, true).clamp(ls[i].range.0, ls[i].range.1);
                }
                for k in (0..members.len()).rev() {
                    let i = members[k];
                    if k + 1 < members.len() {
                        for q in k + 1..members.len() {
                            let limit = x[q] - gap_to(i, members[q]);
                            x[k] = x[k].min(limit);
                        }
                    }
                    x[k] = hop(&ls[i], x[k], false).clamp(ls[i].range.0, ls[i].range.1);
                }
                x
            };
            // Packed in the order they stand today. (Packing them in the order of their
            // ranges' middles, so two pinned stubs could pass each other, was measured
            // worse, with or without judging a pending move by its target: the pair's
            // moves are applied one by one and the second is refused.)
            members.sort_by(|&a, &b| ls[a].at.total_cmp(&ls[b].at).then(a.cmp(&b)));
            let x = pack(&members);
            for (k, &i) in members.iter().enumerate() {
                target[i] = x[k];
            }
        }
        for (i, t) in hug_target.into_iter().enumerate() {
            if let Some(want) = t {
                if (target[i] - ls[i].at).abs() <= 0.5 {
                    target[i] = want;
                }
            }
        }
        // Apply each move that stays clear of nodes and borders and adds no crossing.
        // Moves are checked against where the other legs are *now* (an earlier move
        // may have been refused), so a leg never lands on one that stayed behind.
        let mut cur: Vec<f64> = ls.iter().map(|l| l.at).collect();
        let mut order: Vec<usize> = (0..ls.len()).filter(|&i| ls[i].movable() && (target[i] - ls[i].at).abs() > 0.5).collect();
        order.sort_by(|&i, &j| ls[i].path.cmp(&ls[j].path).then(ls[i].k.cmp(&ls[j].k)));
        // (A second try at the refused moves once the others have moved changed nothing.)
        for i in order {
            let l = ls[i];
            let want = target[i].round();
            let too_close = |at: f64| {
                ls.iter().enumerate().any(|(j, m)| {
                    j != i && alongside(&paths, &l, m) && {
                        let before = (l.at - cur[j]).abs();
                        let after = (at - cur[j]).abs();
                        after < (pitch - 1.0).min(before) - 0.5
                    }
                })
            };
            let pts = paths[l.path].clone();
            let n = pts.len();
            let axis_of = |p: Pt| if vertical { p.0 } else { p.1 };
            let before_n = (l.k > 0).then(|| axis_of(pts[l.k - 1]));
            let after_n = (l.k2 + 2 < n).then(|| axis_of(pts[l.k2 + 2]));
            // Boxes and borders the leg (and its grown neighbours) already touch don't
            // count; new ones do.
            let touches = |at: f64| -> (Vec<usize>, Vec<usize>) {
                let box_hits: Vec<usize> = boxes
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| {
                        b.crossed_by(vertical, at, l.lo, l.hi)
                            || before_n.is_some_and(|p0| neighbour_crosses(b, vertical, pts[l.k], p0, at))
                            || after_n.is_some_and(|p3| neighbour_crosses(b, vertical, pts[l.k2 + 1], p3, at))
                    })
                    .map(|(i, _)| i)
                    .collect();
                let border_hits: Vec<usize> = borders
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| b.0 == vertical && (b.1 - at).abs() < border_lane && l.hi.min(b.3) - l.lo.max(b.2) > MIN_PARALLEL)
                    .map(|(i, _)| i)
                    .collect();
                (box_hits, border_hits)
            };
            let (boxes_now, borders_now) = touches(l.at);
            let ok = |at: f64| -> bool {
                let (b, r) = touches(at);
                b.iter().all(|x| boxes_now.contains(x)) && r.iter().all(|x| borders_now.contains(x)) && !too_close(at)
            };
            // Nearest clear spot to the target, searched a px at a time back toward the
            // leg's present place; nothing clear means it stays.
            let dir = if want > l.at { -1.0 } else { 1.0 };
            let mut at = want.clamp(l.range.0, l.range.1);
            let mut found = None;
            while (at - l.at) * dir < 0.0 {
                if ok(at) {
                    found = Some(at);
                    break;
                }
                at += dir;
            }
            if debug && found != Some(want.clamp(l.range.0, l.range.1)) {
                let probe = want.clamp(l.range.0, l.range.1);
                let (b, r) = touches(probe);
                let new_b: Vec<String> = b.iter().filter(|x| !boxes_now.contains(x)).map(|&i| format!("box{i}=({:.0},{:.0})-({:.0},{:.0})", boxes[i].x0, boxes[i].y0, boxes[i].x1, boxes[i].y1)).collect();
                let new_r: Vec<String> = r.iter().filter(|x| !borders_now.contains(x)).map(|&i| format!("border{i}@{:.0}", borders[i].1)).collect();
                let near: Vec<String> = ls
                    .iter()
                    .enumerate()
                    .filter(|(j, m)| *j != i && alongside(&paths, &l, m) && (probe - cur[*j]).abs() < (pitch - 1.0).min((l.at - cur[*j]).abs()) - 0.5)
                    .map(|(j, m)| format!("leg(path {} k {} at {:.0})", m.path, m.k, cur[j]))
                    .collect();
                let (s, d) = compiled.graph.edge_endpoints(path_edge[l.path]).map_or(("?", "?"), |(a, b)| (compiled.graph[a].id.as_str(), compiled.graph[b].id.as_str()));
                eprintln!("spread: {s} -> {d} (path {}) leg {} at {:.0} blocked at {probe:.0}: {} {} too-close:{}", l.path, l.k, l.at, new_b.join(" "), new_r.join(" "), near.join(","));
            }
            let Some(at) = found else { continue };
            let mut new_pts = pts.clone();
            for p in &mut new_pts[l.k..=l.k2 + 1] {
                if vertical {
                    p.0 = at;
                } else {
                    p.1 = at;
                }
            }
            // A leg lying on another arrow's leg (drawn as one line) counts as a crossing
            // it already has, so lifting it off may cost one.
            let laid_on = |at: f64| -> usize {
                ls.iter().enumerate().filter(|(j, m)| *j != i && alongside(&paths, &l, m) && (at - cur[*j]).abs() < 1.5).count()
            };
            let before: usize = paths.iter().enumerate().filter(|(j, _)| *j != l.path).map(|(_, q)| crossings(&pts, q)).sum::<usize>() + laid_on(l.at);
            let after: usize = paths.iter().enumerate().filter(|(j, _)| *j != l.path).map(|(_, q)| crossings(&new_pts, q)).sum::<usize>() + laid_on(at);
            if after > before {
                if debug {
                    let (s, d) = compiled.graph.edge_endpoints(path_edge[l.path]).map_or(("?", "?"), |(a, b)| (compiled.graph[a].id.as_str(), compiled.graph[b].id.as_str()));
                    eprintln!("spread: {s} -> {d} (path {}) leg {} {:.0} -> {at:.0} refused: crossings {before} -> {after}", l.path, l.k, l.at);
                }
                continue;
            }
            if debug {
                let (s, d) = compiled.graph.edge_endpoints(path_edge[l.path]).map_or(("?", "?"), |(a, b)| (compiled.graph[a].id.as_str(), compiled.graph[b].id.as_str()));
                eprintln!("spread: {s} -> {d} leg {} ({}) {:.0} -> {at:.0} (want {want:.0}, range {:.0}..{:.0}, span {:.0}..{:.0})", l.k, if vertical { "x" } else { "y" }, l.at, l.range.0, l.range.1, l.lo, l.hi);
            }
            if l.k == 0 {
                ends.ports[l.path][0].at = at;
            }
            if l.k2 + 2 == n {
                ends.ports[l.path][1].at = at;
            }
            cur[i] = at;
            paths[l.path] = new_pts;
            moved_any = true;
        }
    }
    // A route that zigzags (four bends or more) is replaced by a two-bend one when a
    // clear line exists: the search settles for a zigzag when the straight way was
    // a lane too close to something at the time, and spreading has since moved it.
    if resimplify(&paths, &ends, &boxes, &borders, pitch, stub, border_lane, debug, &mut paths_out) {
        moved_any = true;
    }
    let paths = paths_out.take().unwrap_or(paths);
    let mut paths = paths;

    // Bends on shared lines: every sliding leg snaps to the lane grid of the channel
    // it runs in (the midline between the node rows or columns either side, a pitch
    // apart), when the spot is clear and keeps a lane from every other leg. Routes
    // that turn in one channel then turn on the same few lines instead of each at
    // its own height.
    let node_rects: Vec<Box2> = node_ids
        .iter()
        .map(|n| {
            let nl = &layout.positions[n];
            Box2 { x0: nl.x, y0: nl.y, x1: nl.x + nl.width, y1: nl.y + nl.height }
        })
        .collect();
    for vertical in [false, true] {
        let ls = legs(&paths, Some(&ends), vertical, min_len, stub);
        let mut cur: Vec<f64> = ls.iter().map(|l| l.at).collect();
        // Interior legs only: an end leg's place is its port's business.
        let order: Vec<usize> = (0..ls.len())
            .filter(|&i| ls[i].movable() && ls[i].k >= 1 && ls[i].k + 2 < paths[ls[i].path].len() && ls[i].hi - ls[i].lo >= MIN_PARALLEL)
            .collect();
        for i in order {
            let l = ls[i];
            // The channel: nearest node edge on each side across the leg, over its span.
            let (mut below, mut above) = (f64::INFINITY, f64::NEG_INFINITY);
            for b in &node_rects {
                let (s0, s1, c0, c1) = if vertical { (b.y0, b.y1, b.x0, b.x1) } else { (b.x0, b.x1, b.y0, b.y1) };
                if s1 <= l.lo || s0 >= l.hi {
                    continue;
                }
                if c1 <= l.at + 0.5 {
                    above = above.max(c1);
                } else if c0 >= l.at - 0.5 {
                    below = below.min(c0);
                }
            }
            if !above.is_finite() || !below.is_finite() || below - above < 2.0 * pitch {
                continue;
            }
            let mid = ((above + below) / 2.0).round();
            let want = (mid + ((l.at - mid) / pitch).round() * pitch).clamp(l.range.0, l.range.1).round();
            if (want - l.at).abs() < 0.5 || (want - l.at).abs() > pitch {
                continue;
            }
            // A lane from every leg it runs alongside, where those are now.
            if ls.iter().enumerate().any(|(j, m)| j != i && alongside(&paths, &l, m) && (want - cur[j]).abs() < pitch - 1.0) {
                continue;
            }
            let pts = paths[l.path].clone();
            let n = pts.len();
            let mut new_pts = pts.clone();
            for p in &mut new_pts[l.k..=l.k2 + 1] {
                if vertical {
                    p.0 = want;
                } else {
                    p.1 = want;
                }
            }
            // Clear of boxes it did not already touch, no new crossing, no new close pair.
            let axis_of = |p: Pt| if vertical { p.0 } else { p.1 };
            let touches = |at: f64| -> Vec<usize> {
                boxes
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| {
                        b.crossed_by(vertical, at, l.lo, l.hi)
                            || (l.k > 0 && neighbour_crosses(b, vertical, pts[l.k], axis_of(pts[l.k - 1]), at))
                            || (l.k2 + 2 < n && neighbour_crosses(b, vertical, pts[l.k2 + 1], axis_of(pts[l.k2 + 2]), at))
                    })
                    .map(|(i, _)| i)
                    .collect()
            };
            let now = touches(l.at);
            if !touches(want).iter().all(|x| now.contains(x)) {
                continue;
            }
            let border_hits = |at: f64| -> Vec<usize> {
                borders
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| b.0 == vertical && (b.1 - at).abs() < border_lane && l.hi.min(b.3) - l.lo.max(b.2) > MIN_PARALLEL)
                    .map(|(i, _)| i)
                    .collect()
            };
            let borders_now = border_hits(l.at);
            if !border_hits(want).iter().all(|x| borders_now.contains(x)) {
                continue;
            }
            let before: usize = paths.iter().enumerate().filter(|(j, _)| *j != l.path).map(|(_, q)| crossings(&pts, q)).sum();
            let after: usize = paths.iter().enumerate().filter(|(j, _)| *j != l.path).map(|(_, q)| crossings(&new_pts, q)).sum();
            if after > before {
                continue;
            }
            if debug {
                eprintln!("align: path {} leg {} ({}) {:.0} -> {want:.0}", l.path, l.k, if vertical { "x" } else { "y" }, l.at);
            }
            cur[i] = want;
            paths[l.path] = new_pts;
            moved_any = true;
        }
    }
    if !moved_any {
        return;
    }
    for (pi, (p, e)) in paths.iter().zip(&path_edge).enumerate() {
        let plan = plans.get_mut(e).expect("path came from a plan");
        let Some((_, _, snl, dnl)) = resolve_edge_layout(compiled, layout, *e) else { continue };
        let frac = |nl: &rdg_layout::NodeLayout, side: Side, q: Pt| match side {
            Side::Top | Side::Bottom => (q.0 - nl.x) / nl.width,
            Side::Left | Side::Right => (q.1 - nl.y) / nl.height,
        };
        plan.exit_port = frac(snl, ends.ports[pi][0].side, p[0]);
        plan.entry_port = frac(dnl, ends.ports[pi][1].side, p[p.len() - 1]);
        let simplified = simplify(p);
        plan.waypoints = simplified[1..simplified.len() - 1].to_vec();
    }
}

/// Replaces every route of four or more bends by a two-bend route (a Z between two
/// facing faces, an L between a vertical and a horizontal face) when one is clear of
/// boxes and border bands, crosses no more routes and runs alongside none within a
/// lane. Writes the new paths to `out`; returns whether any route changed.
#[allow(clippy::too_many_arguments)]
fn resimplify(
    paths: &[Vec<Pt>],
    ends: &Ends,
    boxes: &[Box2],
    borders: &[(bool, f64, f64, f64)],
    pitch: f64,
    stub: f64,
    border_lane: f64,
    debug: bool,
    out: &mut Option<Vec<Vec<Pt>>>,
) -> bool {
    let mut work = paths.to_vec();
    let mut changed = false;
    let is_v = |side: Side| matches!(side, Side::Top | Side::Bottom);
    let sign = |side: Side| match side {
        Side::Top | Side::Left => -1.0,
        Side::Bottom | Side::Right => 1.0,
    };
    for pi in 0..work.len() {
        let pts = work[pi].clone();
        if pts.len() < 6 {
            continue;
        }
        let (p0, pn) = (pts[0], pts[pts.len() - 1]);
        let (s, d) = (ends.ports[pi][0].side, ends.ports[pi][1].side);
        let (ss, ds) = (sign(s), sign(d));
        // Candidate routes, nearest the midpoint first.
        let mut cands: Vec<Vec<Pt>> = Vec::new();
        match (is_v(s), is_v(d)) {
            (true, true) => {
                // Z: a horizontal middle leg at y, beyond both stubs in their directions.
                let (lo, hi) = {
                    let a = if ss > 0.0 { (p0.1 + stub, f64::INFINITY) } else { (f64::NEG_INFINITY, p0.1 - stub) };
                    let b = if ds > 0.0 { (pn.1 + stub, f64::INFINITY) } else { (f64::NEG_INFINITY, pn.1 - stub) };
                    (a.0.max(b.0), a.1.min(b.1))
                };
                if lo <= hi {
                    let mid = ((p0.1 + pn.1) / 2.0).clamp(lo.min(hi), hi.max(lo));
                    for k in 0..12 {
                        for dir in [1.0, -1.0] {
                            let y = (mid + dir * k as f64 * pitch).round();
                            if y >= lo && y <= hi && (k > 0 || dir > 0.0) {
                                cands.push(vec![p0, (p0.0, y), (pn.0, y), pn]);
                            }
                        }
                    }
                }
            }
            (false, false) => {
                let (lo, hi) = {
                    let a = if ss > 0.0 { (p0.0 + stub, f64::INFINITY) } else { (f64::NEG_INFINITY, p0.0 - stub) };
                    let b = if ds > 0.0 { (pn.0 + stub, f64::INFINITY) } else { (f64::NEG_INFINITY, pn.0 - stub) };
                    (a.0.max(b.0), a.1.min(b.1))
                };
                if lo <= hi {
                    let mid = ((p0.0 + pn.0) / 2.0).clamp(lo.min(hi), hi.max(lo));
                    for k in 0..12 {
                        for dir in [1.0, -1.0] {
                            let x = (mid + dir * k as f64 * pitch).round();
                            if x >= lo && x <= hi && (k > 0 || dir > 0.0) {
                                cands.push(vec![p0, (x, p0.1), (x, pn.1), pn]);
                            }
                        }
                    }
                }
            }
            (true, false) => {
                // L: down/up from the source, then across into the side face.
                let corner = (p0.0, pn.1);
                if (corner.1 - p0.1) * ss >= stub && (corner.0 - pn.0) * ds >= stub {
                    cands.push(vec![p0, corner, pn]);
                }
            }
            (false, true) => {
                let corner = (pn.0, p0.1);
                if (corner.0 - p0.0) * ss >= stub && (corner.1 - pn.1) * ds >= stub {
                    cands.push(vec![p0, corner, pn]);
                }
            }
        }
        let others: Vec<&Vec<Pt>> = work.iter().enumerate().filter(|(j, _)| *j != pi).map(|(_, q)| q).collect();
        let before: usize = others.iter().map(|q| crossings(&pts, q)).sum();
        let leg_clear = |a: Pt, b: Pt, first: bool, last: bool| -> bool {
            let vertical = (a.0 - b.0).abs() < 0.5;
            let (at, mut lo, mut hi) = if vertical { (a.0, a.1.min(b.1), a.1.max(b.1)) } else { (a.1, a.0.min(b.0), a.0.max(b.0)) };
            // The ends of a port leg sit inside their own node's inflated box.
            let trim = pitch + 2.0;
            if first {
                if (vertical && a.1 < b.1) || (!vertical && a.0 < b.0) { lo += trim } else { hi -= trim }
            }
            if last {
                if (vertical && b.1 < a.1) || (!vertical && b.0 < a.0) { lo += trim } else { hi -= trim }
            }
            if hi <= lo {
                return true;
            }
            if boxes.iter().any(|bx| bx.crossed_by(vertical, at, lo, hi)) {
                return false;
            }
            !borders.iter().any(|bd| bd.0 == vertical && (bd.1 - at).abs() < border_lane && hi.min(bd.3) - lo.max(bd.2) > MIN_PARALLEL)
        };
        let alongside_any = |cand: &[Pt]| -> bool {
            for (k, w) in cand.windows(2).enumerate() {
                if k == 0 || k + 2 == cand.len() {
                    continue;
                }
                let vertical = (w[0].0 - w[1].0).abs() < 0.5;
                let (at, lo, hi) = if vertical { (w[0].0, w[0].1.min(w[1].1), w[0].1.max(w[1].1)) } else { (w[0].1, w[0].0.min(w[1].0), w[0].0.max(w[1].0)) };
                for q in &others {
                    for v in q.windows(2) {
                        let ov = (v[0].0 - v[1].0).abs() < 0.5 && (v[0].1 - v[1].1).abs() >= 0.5;
                        let oh = (v[0].1 - v[1].1).abs() < 0.5 && (v[0].0 - v[1].0).abs() >= 0.5;
                        if (vertical && !ov) || (!vertical && !oh) {
                            continue;
                        }
                        let (oat, olo, ohi) = if vertical { (v[0].0, v[0].1.min(v[1].1), v[0].1.max(v[1].1)) } else { (v[0].1, v[0].0.min(v[1].0), v[0].0.max(v[1].0)) };
                        if (oat - at).abs() < pitch - 1.0 && hi.min(ohi) - lo.max(olo) >= MIN_PARALLEL {
                            return true;
                        }
                    }
                }
            }
            false
        };
        let (mut n_box, mut n_cross, mut n_along) = (0, 0, 0);
        let n_cands = cands.len();
        let chosen = cands.into_iter().find(|cand| {
            let n = cand.len();
            if !cand.windows(2).enumerate().all(|(k, w)| leg_clear(w[0], w[1], k == 0, k + 2 == n)) {
                n_box += 1;
                return false;
            }
            if others.iter().map(|q| crossings(cand, q)).sum::<usize>() > before {
                n_cross += 1;
                return false;
            }
            if alongside_any(cand) {
                n_along += 1;
                return false;
            }
            true
        });
        if debug && chosen.is_none() {
            eprintln!("resimplify: path {pi} ({} bends, {s:?}->{d:?}) kept: {n_cands} candidates, {n_box} blocked, {n_cross} cross more, {n_along} alongside", pts.len() - 2);
        }
        if let Some(cand) = chosen {
            if debug {
                eprintln!("resimplify: path {pi} {} bends -> {}", pts.len() - 2, cand.len() - 2);
            }
            work[pi] = cand;
            changed = true;
        }
    }
    if changed {
        *out = Some(work);
    }
    changed
}

/// Drops collinear interior points and zero-length segments.
fn simplify(pts: &[Pt]) -> Vec<Pt> {
    let mut out: Vec<Pt> = Vec::with_capacity(pts.len());
    for &p in pts {
        if out.last().is_some_and(|q| (q.0 - p.0).abs() < 0.01 && (q.1 - p.1).abs() < 0.01) {
            continue;
        }
        if out.len() >= 2 {
            let (a, b) = (out[out.len() - 2], out[out.len() - 1]);
            let collinear = ((a.0 - b.0).abs() < 0.01 && (b.0 - p.0).abs() < 0.01) || ((a.1 - b.1).abs() < 0.01 && (b.1 - p.1).abs() < 0.01);
            if collinear {
                out.pop();
            }
        }
        out.push(p);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_pairs_sees_parallel_legs_of_different_paths_only() {
        let a = vec![(0.0, 0.0), (0.0, 50.0), (100.0, 50.0), (100.0, 100.0)];
        let b = vec![(10.0, 0.0), (10.0, 50.0), (110.0, 50.0), (110.0, 100.0)];
        // Two vertical runs 10px apart and one horizontal one 0px apart.
        let pairs = close_pairs(&[a.clone(), b], 18.0);
        assert_eq!(pairs.len(), 3, "{pairs:?}");
        // Sharing the first leg end to end is a fork, not a clash.
        let fork = vec![(0.0, 0.0), (0.0, 50.0), (-100.0, 50.0), (-100.0, 100.0)];
        assert!(close_pairs(&[a, fork], 18.0).is_empty());
    }

    #[test]
    fn a_middle_leg_slides_within_its_neighbours_and_end_legs_only_by_their_ports() {
        let paths = vec![vec![(0.0, 0.0), (0.0, 40.0), (100.0, 40.0), (100.0, 100.0)]];
        let ls = legs(&paths, None, false, 6.0, 12.0);
        let mid = ls.iter().find(|l| l.movable()).expect("the horizontal leg can slide");
        assert_eq!(mid.k, 1);
        // Stubs of 12px above and below bound the slide.
        assert_eq!(mid.range, (12.0, 88.0));
        // Without port information the vertical end legs are fixed.
        assert!(legs(&paths, None, true, 6.0, 12.0).iter().all(|l| !l.movable()));
        // With it, the start leg slides along its bottom face, a pitch from the
        // face's other port at x=20 and never closer than a jog to the next bend.
        let n = NodeIndex::new(0);
        let ends = Ends {
            ports: vec![
                [Port { node: n, side: Side::Bottom, at: 0.0 }, Port { node: n, side: Side::Top, at: 100.0 }],
                [Port { node: n, side: Side::Bottom, at: 20.0 }, Port { node: NodeIndex::new(1), side: Side::Top, at: 0.0 }],
            ],
            spans: vec![[(-30.0, 30.0), (90.0, 110.0)], [(-30.0, 30.0), (0.0, 0.0)]],
            pitch: vec![[12.0; 2], [12.0; 2]],
        };
        let paths2 = vec![paths[0].clone(), vec![(20.0, 0.0), (20.0, 10.0)]];
        let ls = legs(&paths2, Some(&ends), true, 6.0, 12.0);
        let first = ls.iter().find(|l| l.path == 0 && l.k == 0).unwrap();
        assert_eq!(first.range, (-30.0, 8.0));
    }
}
