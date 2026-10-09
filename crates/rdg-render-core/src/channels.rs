//! Channel router for layered layouts.
//!
//! The grid search in [`crate::ortho`] weighs a dozen soft costs per step, so two
//! routes of equal cost are picked by heap order and a 20px shift in the layout can
//! reshuffle every arrow. This router is deterministic and structural instead, the way
//! layered engines (ELK, dagre) draw arrows:
//!
//! * nodes form **bands** across the flow; the space between two bands is a
//!   **channel**, the space between two nodes of a band a **gutter**;
//! * an arrow leaves its source across the flow, runs along a **track** in the channel
//!   below, drops through gutter **lanes** to the channel above its target, runs along a
//!   track there and enters the target across the flow — 0, 2 or 4 bends, more only
//!   when no single gutter reaches through every band in between;
//! * tracks are shared out by the classic channel-routing rule (a segment whose legs
//!   would cross another's sits on the other side of it), lanes are packed a pitch
//!   apart in the order they continue, and ports spread across a face in the order
//!   their arrows head off, so nothing crosses at a face;
//! * arrows never run inside a group neither end belongs to, and keep a lane's width
//!   off every border.
//!
//! Back edges (target above source) leave by a side face, go up a gutter and enter the
//! target's side. Edges between two nodes of one band run straight across when nothing
//! is between them, else round through the channel below.
//!
//! Everything is computed in a flow frame (`y` along the flow), so a left-to-right
//! diagram is routed transposed. An edge the router cannot place without cutting a node
//! is left out of the result for the fallback router.

use std::collections::HashMap;

use petgraph::stable_graph::{EdgeIndex, NodeIndex};
use rdg_graph::CompiledGraph;
use rdg_layout::{DesignTokens, LayoutResult, NodeLayout};

use crate::routing::{EdgeRoutingPlan, Side, resolve_edge_layout};

type Pt = (f64, f64);

#[derive(Debug, Clone, Copy, PartialEq)]
struct Rect {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Rect {
    fn overlaps_y(&self, y0: f64, y1: f64) -> bool {
        self.y0 < y1 - 1e-6 && self.y1 > y0 + 1e-6
    }
    fn overlaps_x(&self, x0: f64, x1: f64) -> bool {
        self.x0 < x1 - 1e-6 && self.x1 > x0 + 1e-6
    }
    fn cx(&self) -> f64 {
        (self.x0 + self.x1) / 2.0
    }
}

/// Maps real coordinates to the flow frame and back (its own inverse).
#[derive(Clone, Copy)]
struct Frame {
    lr: bool,
}

impl Frame {
    fn pt(self, p: Pt) -> Pt {
        if self.lr { (p.1, p.0) } else { p }
    }
    fn rect(self, nl: &NodeLayout) -> Rect {
        let (x, y) = self.pt((nl.x, nl.y));
        let (w, h) = self.pt((nl.width, nl.height));
        Rect { x0: x, y0: y, x1: x + w, y1: y + h }
    }
    fn rect_of(self, x: f64, y: f64, w: f64, h: f64) -> Rect {
        let (x, y) = self.pt((x, y));
        let (w, h) = self.pt((w, h));
        Rect { x0: x, y0: y, x1: x + w, y1: y + h }
    }
    /// A frame side back to a real one.
    fn side(self, s: Side) -> Side {
        if !self.lr {
            return s;
        }
        match s {
            Side::Top => Side::Left,
            Side::Bottom => Side::Right,
            Side::Left => Side::Top,
            Side::Right => Side::Bottom,
        }
    }
}

/// A row of nodes across the flow and the free gutters between them.
struct Band {
    y0: f64,
    y1: f64,
    /// Merged obstacle spans across the flow, sorted.
    spans: Vec<(f64, f64)>,
}

impl Band {
    /// Free intervals across the flow (outer ones unbounded), each kept a clearance off
    /// the boxes beside it; too narrow ones dropped.
    fn gutters(&self, clearance: f64) -> Vec<(f64, f64)> {
        let mut out = Vec::new();
        let mut lo = f64::NEG_INFINITY;
        for &(a, b) in &self.spans {
            if a - clearance > lo + clearance || lo == f64::NEG_INFINITY {
                out.push((lo + clearance, a - clearance));
            }
            lo = b;
        }
        out.push((lo + clearance, f64::INFINITY));
        out.retain(|(a, b)| b - a >= 1.0);
        out
    }
}

/// Where an edge sits relative to the bands.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    /// Source band above target band.
    Down,
    /// Target band above source band.
    Up,
    /// One band: straight across, or round through the channel below.
    Lateral,
}

/// One edge under construction.
struct Work {
    e: EdgeIndex,
    s: NodeIndex,
    t: NodeIndex,
    sr: Rect,
    tr: Rect,
    bs: usize,
    bt: usize,
    kind: Kind,
    /// A lateral detour runs through the channel above the band, not below it.
    above: bool,
    /// The exit / entry port must stay where the plan put it: no track room next
    /// to that face, so the line meets the face itself.
    pinned_exit: bool,
    pinned_entry: bool,
    /// Groups either end is in (bit per group index, past 63 shared).
    groups: u64,
    /// Lane across the flow in every band the route passes beside or through, in
    /// travel order: for a `Down` edge the bands strictly between source and target;
    /// for `Up`, the source band through the target band; for a `Lateral` detour, the
    /// band itself twice (down beside the source, up beside the target).
    lanes: Vec<(usize, f64)>,
    /// The ports the route leaves and enters by, in frame coordinates.
    exit: (Side, Pt),
    entry: (Side, Pt),
    /// The y of every horizontal run between consecutive lanes, by channel, once
    /// tracks are assigned.
    tracks: HashMap<usize, f64>,
    /// The route failed a check; the fallback router takes it.
    failed: bool,
}

struct Ctx<'a> {
    compiled: &'a CompiledGraph,
    layout: &'a LayoutResult,
    tokens: &'a DesignTokens,
    frame: Frame,
    nodes: HashMap<NodeIndex, Rect>,
    /// Boxes a lane may not cross: nodes and group titles.
    obstacles: Vec<Rect>,
    groups: Vec<(usize, Rect)>,
    bands: Vec<Band>,
    clearance: f64,
    pitch: f64,
    border_lane: f64,
}

impl Ctx<'_> {
    /// How far a line through a gutter keeps off the boxes beside it: a stub's
    /// clearance, or a pitch where gutters are narrow.
    fn lane_clearance(&self) -> f64 {
        self.clearance.min(self.pitch)
    }

    fn band_of(&self, r: &Rect) -> usize {
        let c = (r.y0 + r.y1) / 2.0;
        self.bands.iter().position(|b| c >= b.y0 - 1e-6 && c <= b.y1 + 1e-6).unwrap_or(0)
    }

    /// Along the flow, where a track between band `upper` and band `lower` may run over
    /// the span `x0..x1`: below every box of the upper band the span overlaps and above
    /// every box of the lower band it overlaps — the bands' own extents where nothing
    /// overlaps — a stub's clearance (or a third of the room) off each. `lower` past
    /// the last band means the open canvas below. `None` when there is no room.
    fn track_range(&self, upper: usize, lower: usize, x0: f64, x1: f64) -> Option<(f64, f64)> {
        let (x0, x1) = (x0.min(x1) - self.pitch / 2.0, x0.max(x1) + self.pitch / 2.0);
        let within = |b: &Band, o: &Rect| o.y0 >= b.y0 - 1e-6 && o.y1 <= b.y1 + 1e-6;
        let top = &self.bands[upper];
        let lo = self
            .obstacles
            .iter()
            .filter(|o| within(top, o) && o.overlaps_x(x0, x1))
            .map(|o| o.y1)
            .fold(top.y0, f64::max);
        let hi = match self.bands.get(lower) {
            Some(bot) => self.obstacles.iter().filter(|o| within(bot, o) && o.overlaps_x(x0, x1)).map(|o| o.y0).fold(bot.y1, f64::min),
            None => top.y1 + self.pitch * 4.0,
        };
        let room = hi - lo;
        if room < self.pitch * 0.75 {
            return None;
        }
        let margin = self.clearance.min(room / 3.0);
        Some((lo + margin, hi - margin))
    }

    /// Intervals across the flow where a lane of an edge in `groups` may run through
    /// band `b`: the band's gutters, minus every group the edge is not in (plus a lane
    /// off its borders) and minus a lane either side of the borders of groups it is in.
    fn allowed(&self, b: usize, groups: u64) -> Vec<(f64, f64)> {
        let band = &self.bands[b];
        let mut ivs = band.gutters(self.lane_clearance());
        for &(g, r) in &self.groups {
            if !r.overlaps_y(band.y0, band.y1) {
                continue;
            }
            let bl = self.border_lane;
            let cuts: Vec<(f64, f64)> = if groups & group_bit(g) == 0 {
                vec![(r.x0 - bl, r.x1 + bl)]
            } else {
                vec![(r.x0 - bl, r.x0 + bl), (r.x1 - bl, r.x1 + bl)]
            };
            for (c0, c1) in cuts {
                ivs = ivs
                    .into_iter()
                    .flat_map(|(a, b)| {
                        let mut v = Vec::new();
                        if c1 <= a || c0 >= b {
                            v.push((a, b));
                        } else {
                            if c0 > a {
                                v.push((a, c0));
                            }
                            if c1 < b {
                                v.push((c1, b));
                            }
                        }
                        v
                    })
                    .collect();
            }
        }
        ivs.retain(|(a, b)| b - a >= 1.0);
        ivs
    }

    /// Whether a vertical run at `x` from `y0` to `y1` is clear of every obstacle.
    fn vertical_clear(&self, x: f64, y0: f64, y1: f64) -> bool {
        let (y0, y1) = (y0.min(y1), y0.max(y1));
        !self.obstacles.iter().any(|o| x > o.x0 + 1e-6 && x < o.x1 - 1e-6 && o.overlaps_y(y0, y1))
    }

    /// Whether a horizontal run at `y` from `x0` to `x1` is clear of every obstacle.
    fn horizontal_clear(&self, y: f64, x0: f64, x1: f64) -> bool {
        let (x0, x1) = (x0.min(x1), x0.max(x1));
        !self.obstacles.iter().any(|o| y > o.y0 + 1e-6 && y < o.y1 - 1e-6 && o.overlaps_x(x0, x1))
    }

    /// The port range of `n`'s `side` (frame side) in frame coordinates along the face.
    fn face_range(&self, n: NodeIndex, side: Side) -> (f64, f64) {
        let nl = &self.layout.positions[&n];
        let outline = crate::style::outline_of(&self.compiled.graph[n], self.tokens);
        let real = self.frame.side(side);
        let (lo, hi) = crate::ortho::port_range(&outline, real, nl);
        let r = self.nodes[&n];
        match side {
            Side::Top => {
                let (mut a, b) = (r.x0 + (r.x1 - r.x0) * lo, r.x0 + (r.x1 - r.x0) * hi);
                // A group title just above the node: an arrow dropping in must pass
                // beside it, so the face starts past the title's end.
                let band = &self.bands[self.band_of(&r)];
                let mut b = b;
                for o in &self.obstacles[self.nodes.len()..] {
                    if o.y1 <= r.y0 + 1e-6 && o.y1 >= band.y0 - 1e-6 && o.x1 > a && o.x0 < b {
                        a = o.x1 + self.pitch / 2.0;
                        // A card can still be entered to the right of the title; a
                        // mark under it cannot be entered from above at all (a > b).
                        if a > b && !matches!(outline, crate::style::Outline::Captioned { .. }) {
                            b = (r.x1 - self.pitch / 2.0).max(a);
                        }
                    }
                }
                (a, b)
            }
            Side::Bottom => (r.x0 + (r.x1 - r.x0) * lo, r.x0 + (r.x1 - r.x0) * hi),
            Side::Left | Side::Right => (r.y0 + (r.y1 - r.y0) * lo, r.y0 + (r.y1 - r.y0) * hi),
        }
    }
}

fn debug_fail(ctx: &Ctx, w: &Work, why: &str) {
    if std::env::var_os("RDG_DEBUG_CHANNELS").is_some() {
        eprintln!("channels: {} -> {} ({:?}): {why}", ctx.compiled.graph[w.s].id, ctx.compiled.graph[w.t].id, w.kind);
    }
}

fn group_bit(g: usize) -> u64 {
    1u64 << g.min(63)
}

fn nearest_in(ivs: &[(f64, f64)], x: f64) -> Option<f64> {
    ivs.iter()
        .map(|&(a, b)| if x < a { (a, a - x) } else if x > b { (b, x - b) } else { (x, 0.0) })
        .min_by(|p, q| p.1.total_cmp(&q.1))
        .map(|(v, _)| v)
}

fn contains(ivs: &[(f64, f64)], x: f64) -> bool {
    ivs.iter().any(|&(a, b)| x >= a - 1e-6 && x <= b + 1e-6)
}

/// The intervals common to every list.
fn intersect_all(lists: &[Vec<(f64, f64)>]) -> Vec<(f64, f64)> {
    let Some(first) = lists.first() else { return vec![(f64::NEG_INFINITY, f64::INFINITY)] };
    let mut acc = first.clone();
    for l in &lists[1..] {
        let mut next = Vec::new();
        for &(a, b) in &acc {
            for &(c, d) in l {
                let (lo, hi) = (a.max(c), b.min(d));
                if hi - lo >= 1.0 {
                    next.push((lo, hi));
                }
            }
        }
        acc = next;
    }
    acc
}

/// The lane chain through `bands` (consecutive, in travel order) with the fewest
/// shifts: one line through every band when some gutter reaches all the way, nearest
/// to `ideal`; else from `ideal`, keeping each line as long as a band allows it.
fn lane_chain(allowed: &[Vec<(f64, f64)>], ideal: f64, from_end: bool) -> Option<Vec<f64>> {
    if let Some(x) = nearest_in(&intersect_all(allowed), ideal) {
        return Some(vec![x; allowed.len()]);
    }
    let mut xs = vec![0.0; allowed.len()];
    let mut x = ideal;
    let order: Vec<usize> = if from_end { (0..allowed.len()).rev().collect() } else { (0..allowed.len()).collect() };
    for i in order {
        x = nearest_in(&allowed[i], x)?;
        xs[i] = x;
    }
    Some(xs)
}

/// Routes every edge it can; see the module docs. Edges missing from the result go to
/// the fallback router.
pub fn route_channels(compiled: &CompiledGraph, layout: &LayoutResult, tokens: &DesignTokens) -> HashMap<EdgeIndex, EdgeRoutingPlan> {
    let mut out = HashMap::new();
    // Only a layered layout has the bands and channels this router builds on; the flow
    // is the layout's own, not a guess from where the arrows point.
    let Some(direction) = layout.layered.filter(|_| !layout.positions.is_empty()) else {
        return out;
    };
    let frame = Frame { lr: direction == rdg_layout::LayoutDirection::LeftToRight };
    let clearance = tokens.stub_clearance();
    let pitch = tokens.channel_pitch().max(tokens.min_port_pitch()).max(8.0);
    let border_lane = tokens.channel_pitch().max(10.0) * 1.25;

    let nodes: HashMap<NodeIndex, Rect> = layout.positions.iter().map(|(&n, nl)| (n, frame.rect(nl))).collect();
    let mut obstacles: Vec<Rect> = {
        let mut ids: Vec<&NodeIndex> = nodes.keys().collect();
        ids.sort();
        ids.iter().map(|n| nodes[n]).collect()
    };
    for tz in crate::routing::compute_group_title_zones(compiled, layout, tokens) {
        obstacles.push(frame.rect_of(tz.min_x, tz.min_y, tz.max_x - tz.min_x, tz.max_y - tz.min_y));
    }
    let groups: Vec<(usize, Rect)> = rdg_layout::groups::group_rects(compiled, layout, tokens)
        .into_iter()
        .enumerate()
        .filter_map(|(g, r)| r.map(|(x, y, w, h)| (g, frame.rect_of(x, y, w, h))))
        .collect();
    let mut node_groups: HashMap<NodeIndex, u64> = HashMap::new();
    for (g, members) in compiled.group_nodes.iter().enumerate() {
        let mask = std::iter::successors(Some(g), |&p| compiled.group_tree.parent[p]).fold(0u64, |m, a| m | group_bit(a));
        for &n in members {
            *node_groups.entry(n).or_default() |= mask;
        }
    }

    // --- Bands: maximal runs of boxes overlapping along the flow -------------------
    let n_nodes = nodes.len();
    let mut sorted: Vec<Rect> = obstacles[..n_nodes].to_vec();
    sorted.sort_by(|a, b| a.y0.total_cmp(&b.y0));
    let mut bands: Vec<Band> = Vec::new();
    for r in &sorted {
        match bands.last_mut() {
            Some(b) if r.y0 < b.y1 - 1e-6 => {
                b.y1 = b.y1.max(r.y1);
                b.spans.push((r.x0, r.x1));
            }
            _ => bands.push(Band { y0: r.y0, y1: r.y1, spans: vec![(r.x0, r.x1)] }),
        }
    }
    // A group's title sits just above its first row: it joins that row's band (lanes
    // through the band keep off it; tracks stay above it), never a band of its own.
    for tz in &obstacles[n_nodes..] {
        let overlapping = bands.iter().position(|b| tz.y1 > b.y0 - 1e-6 && tz.y0 < b.y1 + 1e-6);
        let Some(bi) = overlapping.or_else(|| bands.iter().position(|b| b.y0 >= tz.y1 - 1e-6)) else { continue };
        bands[bi].y0 = bands[bi].y0.min(tz.y0);
        bands[bi].y1 = bands[bi].y1.max(tz.y1);
        bands[bi].spans.push((tz.x0, tz.x1));
    }
    // Attaching a title may have made two bands touch: merge those.
    let mut merged: Vec<Band> = Vec::new();
    for b in bands {
        match merged.last_mut() {
            Some(m) if b.y0 < m.y1 - 1e-6 => {
                m.y1 = m.y1.max(b.y1);
                m.spans.extend(b.spans);
            }
            _ => merged.push(b),
        }
    }
    let mut bands = merged;
    for b in &mut bands {
        b.spans.sort_by(|a, c| a.0.total_cmp(&c.0));
        let mut merged: Vec<(f64, f64)> = Vec::new();
        for &(a, c) in &b.spans {
            match merged.last_mut() {
                Some(m) if a <= m.1 => m.1 = m.1.max(c),
                _ => merged.push((a, c)),
            }
        }
        b.spans = merged;
    }
    if std::env::var_os("RDG_DEBUG_CHANNELS").is_some() {
        for (i, b) in bands.iter().enumerate() {
            let gap = bands.get(i + 1).map_or(String::new(), |n| format!(" gap {:.0}", n.y0 - b.y1));
            eprintln!("channels: band {i} y={:.0}..{:.0}{gap} spans={:?}", b.y0, b.y1, b.spans.iter().map(|s| (s.0.round(), s.1.round())).collect::<Vec<_>>());
        }
    }
    let ctx = Ctx { compiled, layout, tokens, frame, nodes, obstacles, groups, bands, clearance, pitch, border_lane };

    // --- Edges ---------------------------------------------------------------------
    let mut work: Vec<Work> = Vec::new();
    let mut edge_ids: Vec<EdgeIndex> = compiled.graph.edge_indices().collect();
    edge_ids.sort_by_key(|e| e.index());
    for e in edge_ids {
        let Some((s, t, _, _)) = resolve_edge_layout(compiled, layout, e) else { continue };
        if s == t {
            continue;
        }
        let data = &compiled.graph[e];
        if data.source_port.is_some() || data.target_port.is_some() {
            // Explicit faces: the search router honours them; mixing routers would
            // overlap, so the whole diagram goes to it.
            if std::env::var_os("RDG_DEBUG_CHANNELS").is_some() {
                eprintln!("channels: {} -> {} names a port: the whole diagram goes to the search router", compiled.graph[s].id, compiled.graph[t].id);
            }
            return HashMap::new();
        }
        let (sr, tr) = (ctx.nodes[&s], ctx.nodes[&t]);
        let (bs, bt) = (ctx.band_of(&sr), ctx.band_of(&tr));
        let kind = if bt > bs {
            Kind::Down
        } else if bt < bs {
            Kind::Up
        } else {
            Kind::Lateral
        };
        let groups = node_groups.get(&s).copied().unwrap_or(0) | node_groups.get(&t).copied().unwrap_or(0);
        work.push(Work {
            e,
            s,
            t,
            sr,
            tr,
            bs,
            bt,
            kind,
            above: false,
            pinned_exit: false,
            pinned_entry: false,
            groups,
            lanes: Vec::new(),
            exit: (Side::Bottom, (sr.cx(), sr.y1)),
            entry: (Side::Top, (tr.cx(), tr.y0)),
            tracks: HashMap::new(),
            failed: false,
        });
    }

    // --- Phase A: which gutter in every band between the ends ------------------------
    for w in &mut work {
        match w.kind {
            Kind::Down => plan_down(&ctx, w),
            Kind::Up => plan_up_best(&ctx, w),
            Kind::Lateral => plan_lateral(&ctx, w),
        }
    }

    // --- Phase B: pack the lanes of every gutter a pitch apart ------------------------
    pack_lanes(&ctx, &mut work);

    // --- Phase C: ports, spread across each face in the order the arrows head off ----
    // Twice: a port placed later can straighten the arrow's other end.
    assign_ports(&ctx, &mut work);
    assign_ports(&ctx, &mut work);
    snap_ports(&ctx, &mut work);
    snap_runs(&ctx, &mut work);

    // --- Phase D: tracks in every channel -------------------------------------------
    assign_tracks(&ctx, &mut work);

    // --- Phase E: polylines ------------------------------------------------------------
    // All or nothing: a diagram with one edge left to another router would have that
    // edge's ports and lanes placed without knowledge of the others.
    if work.iter().any(|w| w.failed) {
        return HashMap::new();
    }
    for w in &work {
        let pts = polyline(&ctx, w);
        if pts.len() < 2 || !path_clear(&ctx, &pts, w) {
            if std::env::var_os("RDG_DEBUG_CHANNELS").is_some() {
                eprintln!("channels: {} -> {} left to the fallback router ({pts:?})", compiled.graph[w.s].id, compiled.graph[w.t].id);
            }
            return HashMap::new();
        }
        if std::env::var_os("RDG_DEBUG_CHANNELS").is_some() {
            let shown: Vec<String> = pts.iter().map(|p| format!("({:.0},{:.0})", p.0, p.1)).collect();
            eprintln!("channels: {} -> {} {:?} {}", compiled.graph[w.s].id, compiled.graph[w.t].id, w.kind, shown.join(""));
        }
        let real: Vec<Pt> = pts.iter().map(|&p| frame.pt(p)).collect();
        let (src_side, dst_side) = (frame.side(w.exit.0), frame.side(w.entry.0));
        let waypoints: Vec<Pt> = real[1..real.len() - 1].to_vec();
        let (p0, pn) = (real[0], real[real.len() - 1]);
        let channel_y = waypoints.first().map_or((p0.1 + pn.1) / 2.0, |p| p.1);
        let corridor_x = waypoints.first().map_or((p0.0 + pn.0) / 2.0, |p| p.0);
        out.insert(
            w.e,
            EdgeRoutingPlan {
                src_side,
                dst_side,
                exit_port: face_frac(&layout.positions[&w.s], src_side, p0),
                entry_port: face_frac(&layout.positions[&w.t], dst_side, pn),
                channel_y,
                corridor_x,
                waypoints,
                corridor_bucket_size: 1,
                searched: true,
            },
        );
    }
    out
}

fn face_frac(nl: &NodeLayout, side: Side, p: Pt) -> f64 {
    match side {
        Side::Top | Side::Bottom => ((p.0 - nl.x) / nl.width).clamp(0.0, 1.0),
        Side::Left | Side::Right => ((p.1 - nl.y) / nl.height).clamp(0.0, 1.0),
    }
}

/// Lanes for a downward edge through the bands strictly between its ends — one line
/// where a gutter reaches through them all — then the faces: out of a side and
/// straight down when the line lies beyond the source, into a side when it lies beyond
/// the target, so an arrow heading off sideways takes one bend, not two.
fn plan_down(ctx: &Ctx, w: &mut Work) {
    let mids: Vec<usize> = (w.bs + 1..w.bt).collect();
    if !mids.is_empty() {
        let allowed: Vec<Vec<(f64, f64)>> = mids.iter().map(|&b| ctx.allowed(b, w.groups)).collect();
        let candidates = [lane_chain(&allowed, w.tr.cx(), true), lane_chain(&allowed, w.sr.cx(), false)];
        let score = |xs: &Vec<f64>| {
            let mut shifts = 0;
            let mut travel = (xs[0] - w.sr.cx()).abs() + (xs[xs.len() - 1] - w.tr.cx()).abs();
            for p in xs.windows(2) {
                if (p[1] - p[0]).abs() > 0.5 {
                    shifts += 1;
                    travel += (p[1] - p[0]).abs();
                }
            }
            (shifts, travel)
        };
        let best = candidates.iter().flatten().min_by(|a, b| {
            let (sa, sb) = (score(a), score(b));
            sa.0.cmp(&sb.0).then(sa.1.total_cmp(&sb.1))
        });
        match best {
            Some(xs) => w.lanes = mids.iter().copied().zip(xs.iter().copied()).collect(),
            None => {
                debug_fail(ctx, w, "no line through the bands between");
                w.failed = true;
                return;
            }
        }
    }
    side_faces(ctx, w);
}

/// Swaps a `Down` edge's bottom exit for a side exit when its first line lies beyond
/// the source box (the line then continues straight through the source's band), and
/// its top entry for a side entry when its last line lies beyond the target box.
fn side_faces(ctx: &Ctx, w: &mut Work) {
    // A line just past a box's face would hug it; past a diamond's corner it is clear
    // of the shape at once.
    let margin = |n: NodeIndex| {
        let outline = crate::style::outline_of(&ctx.compiled.graph[n], ctx.tokens);
        if matches!(outline, crate::style::Outline::Diamond) { 0.0 } else { ctx.pitch / 2.0 }
    };
    // The line the route leaves on, and arrives on.
    let first_x = w.lanes.first().map_or(w.tr.cx(), |l| l.1);
    let last_x = w.lanes.last().map_or(w.sr.cx(), |l| l.1);
    let side_of = |r: &Rect, margin: f64, x: f64| -> Option<Side> {
        if x > r.x1 + margin {
            Some(Side::Right)
        } else if x < r.x0 - margin {
            Some(Side::Left)
        } else {
            None
        }
    };
    // A side exit: the stub across to the line and the drop beside the box must be clear.
    if let Some(side) = side_of(&w.sr, margin(w.s), first_x).filter(|_| !w.pinned_exit) {
        let (ya, yb) = ctx.face_range(w.s, side);
        let y = (ya + yb) / 2.0;
        let fx = if side == Side::Right { w.sr.x1 } else { w.sr.x0 };
        let band = &ctx.bands[w.bs];
        let ok = (first_x - fx).abs() >= ctx.pitch
            && ctx.horizontal_clear(y, fx, first_x)
            && ctx.vertical_clear(first_x, y, band.y1 + 1.0)
            && (w.lanes.is_empty() || contains(&ctx.allowed(w.bs, w.groups), first_x) || (first_x - fx).abs() <= ctx.clearance * 1.5);
        if ok {
            w.exit = (side, (fx, y));
            if !w.lanes.is_empty() {
                w.lanes.insert(0, (w.bs, first_x));
            }
        }
    }
    // A top face a group title covers cannot be entered from above: in by the side
    // the route is on, up the gutter beside the target.
    if w.pinned_entry {
        return;
    }
    let (ta, tb) = ctx.face_range(w.t, Side::Top);
    if ta > tb {
        if let Some((side, x)) = lane_beside(ctx, w.bt, &w.tr, w.groups, last_x) {
            let (ya, yb) = ctx.face_range(w.t, side);
            w.entry = (side, (if side == Side::Right { w.tr.x1 } else { w.tr.x0 }, (ya + yb) / 2.0));
            w.lanes.push((w.bt, x));
            return;
        }
    }
    // With no lines between, a side exit already drops straight onto the target.
    if w.lanes.is_empty() && matches!(w.exit.0, Side::Left | Side::Right) {
        return;
    }
    if let Some(side) = side_of(&w.tr, margin(w.t), last_x).filter(|_| !w.pinned_entry) {
        let (ya, yb) = ctx.face_range(w.t, side);
        let y = (ya + yb) / 2.0;
        let fx = if side == Side::Right { w.tr.x1 } else { w.tr.x0 };
        let band = &ctx.bands[w.bt];
        let ok = (last_x - fx).abs() >= ctx.clearance
            && ctx.horizontal_clear(y, fx, last_x)
            && ctx.vertical_clear(last_x, band.y0 - 1.0, y)
            && (w.lanes.is_empty() || contains(&ctx.allowed(w.bt, w.groups), last_x) || (last_x - fx).abs() <= ctx.clearance * 1.5);
        if ok {
            w.entry = (side, (fx, y));
            if !w.lanes.is_empty() {
                w.lanes.push((w.bt, last_x));
            }
        }
    }
}

/// The gutter beside box `r` in band `b` on the side nearer `toward`, as the nearest
/// allowed lane; `None` when neither side has room.
fn lane_beside(ctx: &Ctx, b: usize, r: &Rect, groups: u64, toward: f64) -> Option<(Side, f64)> {
    let ivs = ctx.allowed(b, groups);
    // Gutters touching the box: the first allowed point past each face, at least a
    // stub's clearance off it.
    let right = ivs.iter().filter(|(_, c)| *c >= r.x1 + ctx.clearance - 1e-6).map(|&(a, _)| a.max(r.x1 + ctx.clearance)).fold(None, |m: Option<f64>, a| Some(m.map_or(a, |m| m.min(a))));
    let left = ivs.iter().filter(|(a, _)| *a <= r.x0 - ctx.clearance + 1e-6).map(|&(_, c)| c.min(r.x0 - ctx.clearance)).fold(None, |m: Option<f64>, c| Some(m.map_or(c, |m| m.max(c))));
    // Only a gutter directly beside the box (no other box between) will do.
    let right = right.filter(|&x| ctx.horizontal_clear((r.y0 + r.y1) / 2.0, r.x1, x) && x - r.x1 <= ctx.clearance * 8.0);
    let left = left.filter(|&x| ctx.horizontal_clear((r.y0 + r.y1) / 2.0, x, r.x0) && r.x0 - x <= ctx.clearance * 8.0);
    match (left, right) {
        (Some(l), Some(rt)) => Some(if (toward - rt).abs() <= (toward - l).abs() { (Side::Right, rt) } else { (Side::Left, l) }),
        (Some(l), None) => Some((Side::Left, l)),
        (None, Some(rt)) => Some((Side::Right, rt)),
        (None, None) => None,
    }
}

/// Bends a planned route will have, from its faces and line shifts, and how far it
/// travels across the flow — to compare two plans for one edge before tracks exist.
fn plan_cost(w: &Work) -> (usize, f64) {
    let st = stops(w);
    let mut bends = usize::from(matches!(w.exit.0, Side::Left | Side::Right)) + usize::from(matches!(w.entry.0, Side::Left | Side::Right));
    let mut travel = 0.0;
    if matches!(w.exit.0, Side::Left | Side::Right) {
        travel += st.first().map_or(0.0, |s| (s.1 - w.exit.1.0).abs());
    }
    for p in st.windows(2) {
        if (p[1].1 - p[0].1).abs() > 0.5 {
            bends += 2;
            travel += (p[1].1 - p[0].1).abs();
        }
    }
    if matches!(w.entry.0, Side::Left | Side::Right) {
        travel += st.last().map_or(0.0, |s| (s.1 - w.entry.1.0).abs());
    }
    (bends, travel)
}

/// A back edge, the better of two routes: round the side (out of a side face, up one
/// line, into the target's side — a loop drawn beside the flow, as in a flowchart) or
/// reversed (out of the top, through the channels, into the target's bottom — an
/// inheritance arrow pointing up its hierarchy). Fewest bends, then least travel.
fn plan_up_best(ctx: &Ctx, w: &mut Work) {
    plan_up(ctx, w);
    let side = if w.failed { None } else { Some((plan_cost(w), w.lanes.clone(), w.exit, w.entry, (false, false))) };
    // The reversed route: plan the edge as if it ran down from the target to the source.
    let mut rev = Work {
        e: w.e,
        s: w.t,
        t: w.s,
        sr: w.tr,
        tr: w.sr,
        bs: w.bt,
        bt: w.bs,
        kind: Kind::Down,
        above: false,
        pinned_exit: false,
        pinned_entry: false,
        groups: w.groups,
        lanes: Vec::new(),
        exit: (Side::Bottom, (w.tr.cx(), w.tr.y1)),
        entry: (Side::Top, (w.sr.cx(), w.sr.y0)),
        tracks: HashMap::new(),
        failed: false,
    };
    plan_down(ctx, &mut rev);
    let reversed = if rev.failed {
        None
    } else {
        let mut lanes = rev.lanes.clone();
        lanes.reverse();
        let back = Work {
            e: w.e,
            s: w.s,
            t: w.t,
            sr: w.sr,
            tr: w.tr,
            bs: w.bs,
            bt: w.bt,
            kind: Kind::Up,
            above: false,
            pinned_exit: rev.pinned_entry,
            pinned_entry: rev.pinned_exit,
            groups: w.groups,
            lanes,
            exit: rev.entry,
            entry: rev.exit,
            tracks: HashMap::new(),
            failed: false,
        };
        let cost = plan_cost(&back);
        Some((cost, back.lanes, back.exit, back.entry, (back.pinned_exit, back.pinned_entry)))
    };
    let pick = match (side, reversed) {
        (Some(a), Some(b)) => Some(if (b.0.0, b.0.1) < (a.0.0, a.0.1) { b } else { a }),
        (a, b) => a.or(b),
    };
    match pick {
        Some((_, lanes, exit, entry, (pe, pn))) => {
            w.failed = false;
            w.lanes = lanes;
            w.exit = exit;
            w.entry = entry;
            w.pinned_exit = pe;
            w.pinned_entry = pn;
        }
        None => {
            debug_fail(ctx, w, "neither the side route nor the reversed route fits");
            w.failed = true;
        }
    }
}

/// A back edge: out of a side of the source, up one line through every band to the
/// target's, into the target's side — whichever side of the source gives a line that
/// reaches all the way with the least travel; failing that, up the gutter beside the
/// source, shifting where a band blocks the line.
fn plan_up(ctx: &Ctx, w: &mut Work) {
    let bands: Vec<usize> = (w.bt..=w.bs).rev().collect();
    let allowed: Vec<Vec<(f64, f64)>> = bands.iter().map(|&b| ctx.allowed(b, w.groups)).collect();
    let common = intersect_all(&allowed);
    let sy = |side: Side| (ctx.face_range(w.s, side).0 + ctx.face_range(w.s, side).1) / 2.0;
    let ty = |side: Side| (ctx.face_range(w.t, side).0 + ctx.face_range(w.t, side).1) / 2.0;
    let mut best: Option<(f64, Side, f64)> = None;
    for side in [Side::Left, Side::Right] {
        let fx = if side == Side::Right { w.sr.x1 } else { w.sr.x0 };
        let x = common
            .iter()
            .filter(|&&(a, b)| if side == Side::Right { b > fx } else { a < fx })
            .map(|&(a, b)| if side == Side::Right { a.max(fx + ctx.clearance) } else { b.min(fx - ctx.clearance) })
            .filter(|&x| ctx.horizontal_clear(sy(side), fx, x))
            .min_by(|a, b| (a - fx).abs().total_cmp(&(b - fx).abs()));
        let Some(x) = x else { continue };
        let t_face = if x > w.tr.x1 { w.tr.x1 } else { w.tr.x0 };
        if !ctx.horizontal_clear(ty(if x > w.tr.x1 { Side::Right } else { Side::Left }), t_face, x) {
            continue;
        }
        let travel = (x - fx).abs() + (x - t_face).abs();
        if best.is_none_or(|(t, _, _)| travel < t) {
            best = Some((travel, side, x));
        }
    }
    let (s_side, lanes): (Side, Vec<(usize, f64)>) = match best {
        Some((_, side, x)) => (side, bands.iter().map(|&b| (b, x)).collect()),
        None => {
            let Some((s_side, x0)) = lane_beside(ctx, w.bs, &w.sr, w.groups, w.tr.cx()) else {
                debug_fail(ctx, w, "no gutter beside the source for the side route");
                w.failed = true;
                return;
            };
            let mut lanes = vec![(w.bs, x0)];
            let mut x = x0;
            for b in (w.bt + 1..w.bs).rev() {
                match nearest_in(&ctx.allowed(b, w.groups), x) {
                    Some(nx) => {
                        x = nx;
                        lanes.push((b, x));
                    }
                    None => {
                        debug_fail(ctx, w, &format!("no line through band {b} for the side route"));
                        w.failed = true;
                        return;
                    }
                }
            }
            let Some((_, xt)) = lane_beside(ctx, w.bt, &w.tr, w.groups, x) else {
                debug_fail(ctx, w, "no gutter beside the target for the side route");
                w.failed = true;
                return;
            };
            lanes.push((w.bt, xt));
            (s_side, lanes)
        }
    };
    let xt = lanes.last().map_or(w.tr.cx(), |l| l.1);
    let t_side = if xt > w.tr.x1 { Side::Right } else { Side::Left };
    w.lanes = lanes;
    w.exit = (s_side, (if s_side == Side::Right { w.sr.x1 } else { w.sr.x0 }, sy(s_side)));
    w.entry = (t_side, (if t_side == Side::Right { w.tr.x1 } else { w.tr.x0 }, ty(t_side)));
}

/// Two nodes of one band: straight across when nothing is between them, a vertical
/// drop when one sits over the other, else down beside the source, along the channel
/// below and up beside the target.
fn plan_lateral(ctx: &Ctx, w: &mut Work) {
    let (s, t) = (w.sr, w.tr);
    // One over the other.
    if t.y0 >= s.y1 - 1e-6 || s.y0 >= t.y1 - 1e-6 {
        let (down, upper, lower) = if t.y0 >= s.y1 - 1e-6 { (true, s, t) } else { (false, t, s) };
        let (lo, hi) = (upper.x0.max(lower.x0), upper.x1.min(lower.x1));
        if hi > lo {
            let x = (lo + hi) / 2.0;
            if ctx.vertical_clear(x, upper.y1, lower.y0) {
                if down {
                    w.exit = (Side::Bottom, (x, s.y1));
                    w.entry = (Side::Top, (x, t.y0));
                } else {
                    w.exit = (Side::Top, (x, s.y0));
                    w.entry = (Side::Bottom, (x, t.y1));
                }
                return;
            }
        }
        debug_fail(ctx, w, "one box over the other with no clear drop between");
        w.failed = true;
        return;
    }
    let s_right = t.x0 >= s.x1;
    let (s_side, t_side) = if s_right { (Side::Right, Side::Left) } else { (Side::Left, Side::Right) };
    let (sa, sb) = ctx.face_range(w.s, s_side);
    let (ta, tb) = ctx.face_range(w.t, t_side);
    let gap = if s_right { (s.x1, t.x0) } else { (t.x1, s.x0) };
    let sx = if s_side == Side::Right { s.x1 } else { s.x0 };
    let tx = if t_side == Side::Right { t.x1 } else { t.x0 };
    let (olo, ohi) = (sa.max(ta), sb.min(tb));
    if ohi >= olo {
        let sc = (sa + sb) / 2.0;
        let tc = (ta + tb) / 2.0;
        let y = if sc >= olo && sc <= ohi { sc } else if tc >= olo && tc <= ohi { tc } else { (olo + ohi) / 2.0 };
        if ctx.horizontal_clear(y, gap.0, gap.1) {
            w.exit = (s_side, (sx, y));
            w.entry = (t_side, (tx, y));
            return;
        }
    } else {
        // Faces at different heights: a Z through the middle of the gap.
        let (sy, ty) = ((sa + sb) / 2.0, (ta + tb) / 2.0);
        let mid = (gap.0 + gap.1) / 2.0;
        if ctx.horizontal_clear(sy, gap.0, mid) && ctx.horizontal_clear(ty, mid, gap.1) && ctx.vertical_clear(mid, sy, ty) {
            w.exit = (s_side, (sx, sy));
            w.entry = (t_side, (tx, ty));
            w.lanes = vec![(w.bs, mid)];
            return;
        }
    }
    // Something between them: round through the channel below.
    let (Some((ss, x0)), Some((ts, x1))) = (lane_beside(ctx, w.bs, &s, w.groups, t.cx()), lane_beside(ctx, w.bt, &t, w.groups, s.cx())) else {
        debug_fail(ctx, w, "no gutter beside an end for the detour");
        w.failed = true;
        return;
    };
    if ctx.track_range(w.bs, w.bs + 1, x0, x1).is_none() {
        if w.bs > 0 && ctx.track_range(w.bs - 1, w.bs, x0, x1).is_some() {
            w.above = true;
        } else {
            debug_fail(ctx, w, "no room beside the band for the detour");
            w.failed = true;
            return;
        }
    }
    w.lanes = vec![(w.bs, x0), (w.bs, x1)];
    let (sa, sb) = ctx.face_range(w.s, ss);
    let (ta, tb) = ctx.face_range(w.t, ts);
    w.exit = (ss, (if ss == Side::Right { s.x1 } else { s.x0 }, (sa + sb) / 2.0));
    w.entry = (ts, (if ts == Side::Right { t.x1 } else { t.x0 }, (ta + tb) / 2.0));
}

/// A maximal run of one route's lanes on one line through consecutive bands.
struct Run {
    wi: usize,
    /// Indices into the route's `lanes`.
    l0: usize,
    l1: usize,
    b0: usize,
    b1: usize,
    x: f64,
    /// The line may move within this interval and stay allowed in every band.
    lo: f64,
    hi: f64,
    /// Where the route goes next and came from, for the order across the gutter.
    next: f64,
    prev: f64,
}

/// Spreads the lines sharing a gutter a pitch apart — the whole run of each line at
/// once, so a line never steps sideways between bands — in the order their routes
/// carry on (a line heading further across the flow sits further across), centred on
/// where they wanted to be, inside the interval every one of them may use.
fn pack_lanes(ctx: &Ctx, work: &mut [Work]) {
    let mut runs: Vec<Run> = Vec::new();
    for (wi, w) in work.iter().enumerate() {
        if w.failed {
            continue;
        }
        let mut k = 0;
        while k < w.lanes.len() {
            let (b0, x) = w.lanes[k];
            let mut e = k;
            while e + 1 < w.lanes.len() && (w.lanes[e + 1].1 - x).abs() < 0.5 && w.lanes[e + 1].0.abs_diff(w.lanes[e].0) == 1 {
                e += 1;
            }
            // A line outside every allowed interval (a stub's own line beside its
            // node) stays where it is.
            let (mut lo, mut hi) = (f64::NEG_INFINITY, f64::INFINITY);
            for &(b, _) in &w.lanes[k..=e] {
                match ctx.allowed(b, w.groups).iter().find(|&&(a, c)| x >= a - 1e-6 && x <= c + 1e-6) {
                    Some(&(a, c)) => {
                        lo = lo.max(a);
                        hi = hi.min(c);
                    }
                    None => {
                        lo = x;
                        hi = x;
                    }
                }
            }
            let next = w.lanes.get(e + 1).map_or(w.entry.1.0, |l| l.1);
            let prev = if k == 0 { w.exit.1.0 } else { w.lanes[k - 1].1 };
            runs.push(Run { wi, l0: k, l1: e, b0: b0.min(w.lanes[e].0), b1: b0.max(w.lanes[e].0), x, lo, hi, next, prev });
            k = e + 1;
        }
    }
    // Columns: runs that overlap in bands and could collide.
    let n = runs.len();
    let mut comp: Vec<usize> = (0..n).collect();
    fn find(c: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while c[r] != r {
            r = c[r];
        }
        let mut y = x;
        while c[y] != r {
            let nxt = c[y];
            c[y] = r;
            y = nxt;
        }
        r
    }
    for a in 0..n {
        for b in a + 1..n {
            let (p, q) = (&runs[a], &runs[b]);
            let bands = p.b0 <= q.b1 && q.b0 <= p.b1;
            let room = p.lo.max(q.lo) < p.hi.min(q.hi);
            let near = (p.x - q.x).abs() < ctx.pitch * (n as f64).max(2.0);
            if bands && room && near {
                let (ra, rb) = (find(&mut comp, a), find(&mut comp, b));
                comp[ra.max(rb)] = ra.min(rb);
            }
        }
    }
    let mut members: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let r = find(&mut comp, i);
        members.entry(r).or_default().push(i);
    }
    let mut keys: Vec<usize> = members.keys().copied().collect();
    keys.sort_unstable();
    for key in keys {
        let mut m = members[&key].clone();
        if m.len() < 2 {
            continue;
        }
        m.sort_by(|&a, &b| {
            let (p, q) = (&runs[a], &runs[b]);
            p.x.total_cmp(&q.x).then(p.next.total_cmp(&q.next)).then(p.prev.total_cmp(&q.prev)).then(p.wi.cmp(&q.wi))
        });
        if std::env::var_os("RDG_DEBUG_CHANNELS").is_some() {
            let desc: Vec<String> = m.iter().map(|&i| format!("{}->{} b{}..{} x={:.0} [{:.0},{:.0}]", ctx.compiled.graph[work[runs[i].wi].s].id, ctx.compiled.graph[work[runs[i].wi].t].id, runs[i].b0, runs[i].b1, runs[i].x, runs[i].lo, runs[i].hi)).collect();
            eprintln!("channels: column {}", desc.join(" | "));
        }
        // Each line as close to where it wanted to be as its interval and a pitch from
        // the lines placed before it (in the sorted order) allow; lines pushed past
        // their interval are pulled back from the right, each a pitch short of the next.
        let n = m.len();
        let overlap = |a: usize, b: usize| runs[a].b0 <= runs[b].b1 && runs[b].b0 <= runs[a].b1;
        let mut xs = vec![0.0; n];
        for k in 0..n {
            let r = &runs[m[k]];
            let mut floor = r.lo;
            for j in 0..k {
                if overlap(m[j], m[k]) {
                    floor = floor.max(xs[j] + ctx.pitch);
                }
            }
            xs[k] = r.x.max(floor);
        }
        for k in (0..n).rev() {
            let r = &runs[m[k]];
            let mut ceil = r.hi;
            for j in k + 1..n {
                if overlap(m[j], m[k]) {
                    ceil = ceil.min(xs[j] - ctx.pitch);
                }
            }
            if xs[k] > ceil {
                xs[k] = ceil.max(r.lo);
            }
        }
        for k in 0..n {
            let r = &runs[m[k]];
            for l in r.l0..=r.l1 {
                work[r.wi].lanes[l].1 = xs[k];
            }
        }
    }
}

/// Ports: on each face, the arrows sorted by where they head, spread a pitch apart
/// around the face's middle — or, alone and able to go straight, on their own line.
fn assign_ports(ctx: &Ctx, work: &mut [Work]) {
    // (node, side) -> (work index, exit?, the x/y the arrow heads to)
    type FacePorts = HashMap<(NodeIndex, Side), Vec<(usize, bool, f64)>>;
    let mut faces: FacePorts = HashMap::new();
    for (wi, w) in work.iter().enumerate() {
        if w.failed {
            continue;
        }
        let along = |side: Side, p: Pt| if matches!(side, Side::Top | Side::Bottom) { p.0 } else { p.1 };
        // Where the arrow heads after leaving / before arriving: the first / last lane,
        // else the other port.
        let exit_to = match w.exit.0 {
            Side::Top | Side::Bottom => w.lanes.first().map_or(w.entry.1.0, |l| l.1),
            _ => w.lanes.first().map_or(w.entry.1.1, |_| along(w.exit.0, w.exit.1)),
        };
        let entry_from = match w.entry.0 {
            Side::Top | Side::Bottom => w.lanes.last().map_or(w.exit.1.0, |l| l.1),
            _ => w.lanes.last().map_or(w.exit.1.1, |_| along(w.entry.0, w.entry.1)),
        };
        faces.entry((w.s, w.exit.0)).or_default().push((wi, true, if w.pinned_exit { w.exit.1.0 } else { exit_to }));
        faces.entry((w.t, w.entry.0)).or_default().push((wi, false, if w.pinned_entry { w.entry.1.0 } else { entry_from }));
    }
    let mut keys: Vec<(NodeIndex, Side)> = faces.keys().copied().collect();
    keys.sort_by_key(|(n, s)| (n.index(), *s as u8));
    for key in keys {
        let (n, side) = key;
        let mut members = faces[&key].clone();
        let (lo, hi) = ctx.face_range(n, side);
        // A face nothing may enter from this side (a mark under a group title) still
        // takes its ports at one spot; the route was steered elsewhere already.
        let (lo, hi) = if lo > hi { ((lo + hi) / 2.0, (lo + hi) / 2.0) } else { (lo, hi) };
        let centre = (lo + hi) / 2.0;
        let horizontal_face = matches!(side, Side::Top | Side::Bottom);
        // Ports on a small mark (a tile, a logo) sit closer than a lane's pitch.
        let outline = crate::style::outline_of(&ctx.compiled.graph[n], ctx.tokens);
        let pitch = match outline {
            crate::style::Outline::Captioned { mark, .. } => ctx.pitch.min(mark / 3.5).max(8.0),
            _ => ctx.pitch,
        };
        if std::env::var_os("RDG_DEBUG_CHANNELS").is_some() {
            eprintln!("channels: face {}.{side:?} range {lo:.0}..{hi:.0} wants {:?}", ctx.compiled.graph[n].id, members.iter().map(|m| m.2.round()).collect::<Vec<_>>());
        }
        // Sort by where the arrows head; an exit and an entry heading the same way keep
        // their order by edge.
        members.sort_by(|a, b| a.2.total_cmp(&b.2).then(a.0.cmp(&b.0)));
        let n_ports = members.len();
        let mut xs: Vec<f64> = if n_ports == 1 {
            let want = members[0].2;
            vec![if want >= lo && want <= hi && (want - centre).abs() > 0.5 { want } else { centre }]
        } else {
            // Every port where its arrow wants to be when those spots all lie on the
            // face a pitch apart (straight lines); else each snapped into a cluster of
            // `n_ports` pitches around the face's middle, then kept a pitch apart.
            let wants: Vec<f64> = members.iter().map(|m| m.2).collect();
            let all_fit = wants.iter().all(|&x| x >= lo && x <= hi) && wants.windows(2).all(|p| p[1] - p[0] >= pitch - 1e-6);
            if all_fit {
                for ((wi, is_exit, _), x) in members.iter().zip(wants) {
                    let w = &mut work[*wi];
                    let p = if *is_exit { &mut w.exit.1 } else { &mut w.entry.1 };
                    if horizontal_face {
                        p.0 = x;
                    } else {
                        p.1 = x;
                    }
                }
                continue;
            }
            let half = n_ports as f64 * pitch / 2.0;
            let (clo, chi) = ((centre - half).max(lo), (centre + half).min(hi));
            let mut v: Vec<f64> = members.iter().map(|m| m.2.clamp(clo, chi)).collect();
            let spacing = pitch.min((chi - clo) / (n_ports as f64 - 1.0).max(1.0));
            for i in 1..v.len() {
                v[i] = v[i].max(v[i - 1] + spacing);
            }
            let over = v.last().map_or(0.0, |&l| (l - chi).max(0.0));
            for x in &mut v {
                *x -= over;
            }
            // Pulled back past the cluster's start: pack from there instead (the
            // spacing was chosen so that they fit).
            if let Some(first) = v.first_mut() {
                *first = first.max(clo);
            }
            for i in 1..v.len() {
                v[i] = v[i].max(v[i - 1] + spacing);
            }
            v
        };
        for x in &mut xs {
            *x = x.clamp(lo, hi);
        }
        // Pinned ports keep their spot; the others move a pitch off them.
        let pinned: Vec<(usize, f64)> = members
            .iter()
            .enumerate()
            .filter(|(_, (wi, is_exit, _))| if *is_exit { work[*wi].pinned_exit } else { work[*wi].pinned_entry })
            .map(|(k, m)| (k, m.2))
            .collect();
        for &(k, x) in &pinned {
            xs[k] = x;
        }
        for (k, x) in xs.iter_mut().enumerate() {
            if pinned.iter().any(|&(pk, _)| pk == k) {
                continue;
            }
            for &(_, px) in &pinned {
                if (*x - px).abs() < pitch {
                    let moved = if *x >= px { px + pitch } else { px - pitch };
                    *x = if moved >= lo && moved <= hi { moved } else if px + pitch <= hi { px + pitch } else { (px - pitch).max(lo) };
                }
            }
        }
        for ((wi, is_exit, _), x) in members.iter().zip(xs) {
            let w = &mut work[*wi];
            let p = if *is_exit { &mut w.exit.1 } else { &mut w.entry.1 };
            if horizontal_face {
                p.0 = x;
            } else {
                p.1 = x;
            }
        }
    }
}

/// A line a few px off the port it leads to or from would draw as a jog in the
/// channel between: move that run of the line onto the port's own x where every band
/// of the run allows it (lines converging on one face may then sit closer than a
/// pitch, which is how they meet the face anyway).
fn snap_runs(ctx: &Ctx, work: &mut [Work]) {
    for w in work.iter_mut() {
        if w.failed || w.lanes.is_empty() {
            continue;
        }
        for at_end in [false, true] {
            let (port_side, port_x) = if at_end { (w.entry.0, w.entry.1.0) } else { (w.exit.0, w.exit.1.0) };
            if !matches!(port_side, Side::Top | Side::Bottom) {
                continue;
            }
            let n = w.lanes.len();
            let idx: Vec<usize> = if at_end { (0..n).rev().collect() } else { (0..n).collect() };
            let x = w.lanes[idx[0]].1;
            let d = port_x - x;
            if d.abs() < 0.25 || d.abs() >= ctx.pitch {
                continue;
            }
            let run: Vec<usize> = idx.iter().copied().take_while(|&i| (w.lanes[i].1 - x).abs() < 0.5).collect();
            if run.iter().all(|&i| contains(&ctx.allowed(w.lanes[i].0, w.groups), port_x)) {
                for i in run {
                    w.lanes[i].1 = port_x;
                }
            }
        }
    }
}

/// Two ports a few px apart on an arrow with nothing between them would draw as a
/// jog: move one onto the other's line where its face allows, else both to the middle.
fn snap_ports(ctx: &Ctx, work: &mut [Work]) {
    let tol = ctx.pitch;
    let mut moves: Vec<(usize, f64)> = Vec::new();
    for (wi, w) in work.iter().enumerate() {
        if w.failed || !w.lanes.is_empty() || !matches!(w.exit.0, Side::Top | Side::Bottom) || !matches!(w.entry.0, Side::Top | Side::Bottom) || w.pinned_exit || w.pinned_entry {
            continue;
        }
        let (ex, en) = (w.exit.1.0, w.entry.1.0);
        if (ex - en).abs() < 0.25 || (ex - en).abs() > tol {
            continue;
        }
        let (slo, shi) = ctx.face_range(w.s, w.exit.0);
        let (tlo, thi) = ctx.face_range(w.t, w.entry.0);
        let x = if en >= slo && en <= shi {
            en
        } else if ex >= tlo && ex <= thi {
            ex
        } else {
            (ex + en) / 2.0
        };
        moves.push((wi, x));
    }
    for (wi, x) in moves {
        // Only when no other port on either face sits within a pitch of the new line.
        let w = &work[wi];
        let (s, t, es, en) = (w.s, w.t, w.exit.0, w.entry.0);
        let clash = work.iter().enumerate().any(|(oi, o)| {
            oi != wi
                && !o.failed
                && ((o.s == s && o.exit.0 == es && (o.exit.1.0 - x).abs() < ctx.pitch - 0.5)
                    || (o.t == s && o.entry.0 == es && (o.entry.1.0 - x).abs() < ctx.pitch - 0.5)
                    || (o.s == t && o.exit.0 == en && (o.exit.1.0 - x).abs() < ctx.pitch - 0.5)
                    || (o.t == t && o.entry.0 == en && (o.entry.1.0 - x).abs() < ctx.pitch - 0.5))
        });
        if !clash {
            work[wi].exit.1.0 = x;
            work[wi].entry.1.0 = x;
        }
    }
}

/// A horizontal run in a channel, with the x of its legs to the band above and below.
#[derive(Debug, Clone, Copy)]
struct Seg {
    wi: usize,
    /// The key a route stores its track under (see `Work::tracks`).
    channel: usize,
    /// The bands the run lies between (`lower` past the last band: the open canvas).
    upper: usize,
    lower: usize,
    x0: f64,
    x1: f64,
    top_x: f64,
    bot_x: f64,
    /// Where along the flow the run may lie.
    lo_y: f64,
    hi_y: f64,
}

impl Seg {
    fn lo(&self) -> f64 {
        self.x0.min(self.x1)
    }
    fn hi(&self) -> f64 {
        self.x0.max(self.x1)
    }
    fn spans(&self, x: f64) -> bool {
        x > self.lo() + 0.5 && x < self.hi() - 0.5
    }
}

/// The lane chain of a route as `(band or channel marker, x)` stops: the exit port, every
/// lane, the entry port. Horizontal runs happen between consecutive stops in the
/// channel between their bands.
fn stops(w: &Work) -> Vec<(usize, f64, f64)> {
    // (band, x, y-hint): y-hint is the stop's own y for ports. A side port is not a
    // stop: the route joins its first / last lane across the band instead.
    let mut v = Vec::new();
    if matches!(w.exit.0, Side::Top | Side::Bottom) {
        v.push((w.bs, w.exit.1.0, w.exit.1.1));
    }
    for &(b, x) in &w.lanes {
        v.push((b, x, f64::NAN));
    }
    if matches!(w.entry.0, Side::Top | Side::Bottom) {
        v.push((w.bt, w.entry.1.0, w.entry.1.1));
    }
    v
}

/// Tracks: every horizontal run gets a y in the room between the boxes it runs
/// between (see `Ctx::track_range`), ordered so that legs cross as little as possible,
/// kept a pitch apart where runs overlap, off every group border and out of groups the
/// edge is not in, then moved as a bundle toward the middle of its room.
fn assign_tracks(ctx: &Ctx, work: &mut [Work]) {
    let mut segs: Vec<Seg> = Vec::new();
    for (wi, w) in work.iter().enumerate() {
        if w.failed {
            continue;
        }
        let st = stops(w);
        for k in 0..st.len().saturating_sub(1) {
            let ((ba, xa, _), (bb, xb, _)) = (st[k], st[k + 1]);
            if (xa - xb).abs() < 0.5 {
                continue;
            }
            let (channel, upper, lower) = match w.kind {
                Kind::Down => (ba, ba, bb),
                Kind::Up => (bb, bb, ba),
                Kind::Lateral => {
                    if w.lanes.len() != 2 {
                        continue;
                    }
                    if w.above { (ba.saturating_sub(1), ba.saturating_sub(1), ba) } else { (ba, ba, ba + 1) }
                }
            };
            if ba == bb && w.kind != Kind::Lateral {
                continue;
            }
            let (top_x, bot_x) = if w.kind == Kind::Down { (xa, xb) } else { (xb, xa) };
            let Some((lo_y, hi_y)) = ctx.track_range(upper, lower, xa, xb) else {
                debug_fail(ctx, w, &format!("no room for a run between bands {upper} and {lower} over {:.0}..{:.0}", xa.min(xb), xa.max(xb)));
                segs.push(Seg { wi, channel, upper, lower, x0: xa, x1: xb, top_x, bot_x, lo_y: f64::NAN, hi_y: f64::NAN });
                continue;
            };
            segs.push(Seg { wi, channel, upper, lower, x0: xa, x1: xb, top_x, bot_x, lo_y, hi_y });
        }
    }
    for s in &segs {
        if s.lo_y.is_nan() {
            work[s.wi].failed = true;
        }
    }
    let mut channels: Vec<usize> = segs.iter().map(|s| s.channel).collect();
    channels.sort_unstable();
    channels.dedup();
    for ch in channels {
        let idx: Vec<usize> = (0..segs.len()).filter(|&i| segs[i].channel == ch && !segs[i].lo_y.is_nan()).collect();
        let n = idx.len();
        if n == 0 {
            continue;
        }
        // Preferred order: p before (above) q when that crosses less.
        let cross = |p: &Seg, q: &Seg| -> usize { usize::from(p.spans(q.top_x)) + usize::from(q.spans(p.bot_x)) };
        let mut above = vec![vec![false; n]; n];
        for a in 0..n {
            for b in 0..n {
                if a != b {
                    let (p, q) = (&segs[idx[a]], &segs[idx[b]]);
                    if cross(p, q) < cross(q, p) {
                        above[a][b] = true;
                    }
                }
            }
        }
        // Topological order (cycles broken by taking the earliest segment).
        let mut order: Vec<usize> = Vec::with_capacity(n);
        let mut placed = vec![false; n];
        while order.len() < n {
            let next = (0..n).find(|&b| !placed[b] && (0..n).all(|a| placed[a] || !above[a][b]));
            let b = next.unwrap_or_else(|| (0..n).find(|&b| !placed[b]).expect("unplaced segment"));
            placed[b] = true;
            order.push(b);
        }
        let allowed_y = |y: f64, s: &Seg, strict: bool| -> bool {
            if y < s.lo_y - 1e-6 || y > s.hi_y + 1e-6 {
                return false;
            }
            if !strict {
                return true;
            }
            // Off the borders the run crosses or runs beside, and out of groups the
            // edge is not in.
            let groups = work[s.wi].groups;
            !ctx.groups.iter().any(|&(g, r)| {
                if !r.overlaps_x(s.lo(), s.hi()) {
                    return false;
                }
                let near_border = (y - r.y0).abs() < ctx.border_lane || (y - r.y1).abs() < ctx.border_lane;
                let inside_foreign = groups & group_bit(g) == 0 && y > r.y0 && y < r.y1;
                near_border || inside_foreign
            })
        };
        let overlaps = |a: &Seg, b: &Seg| a.lo() < b.hi() + ctx.pitch && a.hi() > b.lo() - ctx.pitch;
        let mut ys: Vec<f64> = vec![f64::NAN; n];
        for &b in &order {
            let s = &segs[idx[b]];
            // Spacing: a pitch, or what the run's room allows for the runs it overlaps.
            let sharing = 1 + (0..n).filter(|&a| a != b && overlaps(&segs[idx[a]], s)).count();
            let step = ctx.pitch.min(((s.hi_y - s.lo_y) / sharing as f64).max(4.0));
            let fits = |y: f64, strict: bool, ys: &[f64]| -> bool {
                allowed_y(y, s, strict)
                    && (0..n).all(|a| {
                        if ys[a].is_nan() {
                            return true;
                        }
                        let o = &segs[idx[a]];
                        if above[a][b] {
                            ys[a] + step - 1e-6 <= y
                        } else if overlaps(o, s) {
                            (ys[a] - y).abs() >= step - 1e-6
                        } else {
                            true
                        }
                    })
            };
            let mut found = f64::NAN;
            'strictness: for strict in [true, false] {
                let mut y = s.lo_y;
                while y <= s.hi_y + 1e-6 {
                    if fits(y, strict, &ys) {
                        found = y;
                        break 'strictness;
                    }
                    y += step;
                }
            }
            ys[b] = found;
        }
        // Move the bundle toward the middle of its room: one shift every run allows.
        let placed_idx: Vec<usize> = (0..n).filter(|&b| !ys[b].is_nan()).collect();
        if !placed_idx.is_empty() {
            let want: f64 = placed_idx.iter().map(|&b| (segs[idx[b]].lo_y + segs[idx[b]].hi_y) / 2.0 - ys[b]).sum::<f64>() / placed_idx.len() as f64;
            let max_up = placed_idx.iter().map(|&b| segs[idx[b]].lo_y - ys[b]).fold(f64::NEG_INFINITY, f64::max);
            let max_down = placed_idx.iter().map(|&b| segs[idx[b]].hi_y - ys[b]).fold(f64::INFINITY, f64::min);
            let shift = want.clamp(max_up.min(max_down), max_down.max(max_up)).round();
            if shift.abs() > 0.5 && placed_idx.iter().all(|&b| allowed_y(ys[b] + shift, &segs[idx[b]], true)) {
                for &b in &placed_idx {
                    ys[b] += shift;
                }
            }
        }
        for (k, &b) in idx.iter().enumerate() {
            let y = ys[k];
            if y.is_nan() {
                debug_fail(ctx, &work[segs[b].wi], &format!("no track fits between bands {} and {}", segs[b].upper, segs[b].lower));
                work[segs[b].wi].failed = true;
            } else {
                work[segs[b].wi].tracks.insert(ch, y);
            }
        }
    }
}

/// The route's points: the exit port, a stub across to the first line for a side exit,
/// the lines through the bands with a track run in every channel where the line
/// shifts, a stub across from the last line for a side entry, the entry port.
fn polyline(_ctx: &Ctx, w: &Work) -> Vec<Pt> {
    let st = stops(w);
    let mut pts: Vec<Pt> = vec![w.exit.1];
    // No stops at all: two side ports facing each other across a band.
    let Some(&(_, mut x, _)) = st.first() else {
        pts.push(w.entry.1);
        return pts;
    };
    if matches!(w.exit.0, Side::Left | Side::Right) {
        pts.push((x, w.exit.1.1));
    }
    for k in 0..st.len().saturating_sub(1) {
        let (ba, _, _) = st[k];
        let (bb, xb, _) = st[k + 1];
        if (xb - x).abs() < 0.5 {
            continue;
        }
        let channel = match w.kind {
            Kind::Down => ba,
            Kind::Up => bb,
            Kind::Lateral => if w.above { ba.saturating_sub(1) } else { ba },
        };
        let y = match w.tracks.get(&channel) {
            Some(&y) => y,
            // A Z across one band: the stub runs at the port's own height.
            None if w.kind == Kind::Lateral && w.lanes.len() == 1 => if k == 0 { w.exit.1.1 } else { w.entry.1.1 },
            None => return Vec::new(),
        };
        pts.push((x, y));
        pts.push((xb, y));
        x = xb;
    }
    if matches!(w.entry.0, Side::Left | Side::Right) {
        pts.push((x, w.entry.1.1));
    }
    pts.push(w.entry.1);
    // Drop repeats and collinear points.
    let mut out: Vec<Pt> = Vec::with_capacity(pts.len());
    for p in pts {
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

/// Whether every interior segment of `pts` is clear of all boxes but the route's own
/// end nodes, and no segment is diagonal.
fn path_clear(ctx: &Ctx, pts: &[Pt], w: &Work) -> bool {
    for seg in pts.windows(2) {
        let (a, b) = (seg[0], seg[1]);
        if (a.0 - b.0).abs() > 0.01 && (a.1 - b.1).abs() > 0.01 {
            return false;
        }
        for o in &ctx.obstacles {
            if *o == w.sr || *o == w.tr {
                continue;
            }
            let hit = if (a.1 - b.1).abs() <= 0.01 {
                a.1 > o.y0 + 1e-6 && a.1 < o.y1 - 1e-6 && o.overlaps_x(a.0.min(b.0), a.0.max(b.0))
            } else {
                a.0 > o.x0 + 1e-6 && a.0 < o.x1 - 1e-6 && o.overlaps_y(a.1.min(b.1), a.1.max(b.1))
            };
            if hit {
                if std::env::var_os("RDG_DEBUG_CHANNELS").is_some() {
                    eprintln!("channels: {} -> {} segment ({:.0},{:.0})-({:.0},{:.0}) hits ({:.0},{:.0})-({:.0},{:.0})", ctx.compiled.graph[w.s].id, ctx.compiled.graph[w.t].id, a.0, a.1, b.0, b.1, o.x0, o.y0, o.x1, o.y1);
                }
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use rdg_layout::LayoutDirection;
    use rdg_schema::{DiagramPayload, EdgeDef, NodeDef};

    fn layout_of(nodes: &[(&str, f64, f64)], edges: &[(&str, &str)]) -> (CompiledGraph, LayoutResult) {
        let compiled = rdg_graph::build_graph(&DiagramPayload {
            nodes: nodes.iter().map(|(id, _, _)| NodeDef { id: (*id).into(), label: (*id).into(), ..Default::default() }).collect(),
            edges: edges.iter().map(|(a, b)| EdgeDef { from: (*a).into(), to: (*b).into(), ..Default::default() }).collect(),
            ..Default::default()
        })
        .unwrap();
        let mut positions = HashMap::new();
        for (id, x, y) in nodes {
            positions.insert(compiled.node_map[*id], NodeLayout { x: *x, y: *y, width: 120.0, height: 60.0 });
        }
        (compiled, LayoutResult { positions, layered: Some(LayoutDirection::TopToBottom), ..Default::default() })
    }

    fn plan<'a>(compiled: &'a CompiledGraph, plans: &'a HashMap<EdgeIndex, EdgeRoutingPlan>, from: &str, to: &str) -> &'a EdgeRoutingPlan {
        let e = compiled
            .graph
            .edge_indices()
            .find(|&e| {
                let (s, t) = compiled.graph.edge_endpoints(e).unwrap();
                compiled.graph[s].id == from && compiled.graph[t].id == to
            })
            .unwrap();
        &plans[&e]
    }

    #[test]
    fn an_offset_target_takes_a_track_between_the_rows_and_centred_ports() {
        // `b` overlaps `a` across the flow (its middle is on `a`'s right edge), so the
        // arrow leaves the bottom, shifts along a track between the rows and enters
        // the top — both ports at the middle of their face.
        let (compiled, layout) = layout_of(&[("a", 0.0, 0.0), ("b", 60.0, 200.0)], &[("a", "b")]);
        let plans = route_channels(&compiled, &layout, &DesignTokens::default());
        let p = plan(&compiled, &plans, "a", "b");
        assert_eq!((p.src_side, p.dst_side), (Side::Bottom, Side::Top), "{p:?}");
        assert!((p.exit_port - 0.5).abs() < 1e-6 && (p.entry_port - 0.5).abs() < 1e-6, "{p:?}");
        assert_eq!(p.waypoints.len(), 2, "one horizontal run: {:?}", p.waypoints);
        let y = p.waypoints[0].1;
        assert!(y > 60.0 && y < 200.0 && (p.waypoints[1].1 - y).abs() < 1e-6, "{:?}", p.waypoints);
        assert!((p.waypoints[0].0 - 60.0).abs() < 1e-6 && (p.waypoints[1].0 - 120.0).abs() < 1e-6, "{:?}", p.waypoints);
    }

    #[test]
    fn a_line_beyond_the_source_leaves_by_its_side_and_drops_straight() {
        // `b` sits under `a`; `c` is below `b`, to the right: the lane runs beside `b`,
        // beyond `a`'s right face, so the arrow leaves `a` sideways and drops into `c`.
        let (compiled, layout) = layout_of(&[("a", 0.0, 0.0), ("b", 0.0, 200.0), ("c", 300.0, 400.0)], &[("a", "c"), ("a", "b")]);
        let plans = route_channels(&compiled, &layout, &DesignTokens::default());
        let p = plan(&compiled, &plans, "a", "c");
        assert_eq!(p.src_side, Side::Right, "{p:?}");
        assert_eq!(p.dst_side, Side::Top);
        assert_eq!(p.waypoints.len(), 1, "one bend: {:?}", p.waypoints);
        assert!((p.waypoints[0].1 - 30.0).abs() < 1e-6, "leaves at the face's middle: {:?}", p.waypoints);
        let straight = plan(&compiled, &plans, "a", "b");
        assert!(straight.waypoints.is_empty(), "{straight:?}");
    }

    #[test]
    fn a_back_edge_with_nothing_between_runs_straight_up() {
        let (compiled, layout) = layout_of(&[("a", 0.0, 0.0), ("b", 0.0, 200.0)], &[("b", "a")]);
        let plans = route_channels(&compiled, &layout, &DesignTokens::default());
        let p = plan(&compiled, &plans, "b", "a");
        assert_eq!((p.src_side, p.dst_side), (Side::Top, Side::Bottom), "{p:?}");
        assert!(p.waypoints.is_empty(), "{:?}", p.waypoints);
    }

    #[test]
    fn a_diagram_naming_a_port_is_left_to_the_search_router() {
        let (compiled, layout) = layout_of(&[("a", 0.0, 0.0), ("b", 0.0, 200.0)], &[("a", "b")]);
        let mut compiled = compiled;
        let e = compiled.graph.edge_indices().next().unwrap();
        compiled.graph[e].source_port = Some("left".into());
        assert!(route_channels(&compiled, &layout, &DesignTokens::default()).is_empty());
    }
}
