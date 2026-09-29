//! Orthogonal edge router: search-chosen faces, rip-up-and-reroute, nudged channels.
//!
//! The older planner in [`crate::routing`] commits every edge to a pair of faces up
//! front and then patches the fallout (corridor buckets, doorways, de-overlap passes).
//! Reviewing a hand-corrected real diagram showed where that goes wrong: an arrow that
//! should leave a gateway's side and drop straight into a service instead leaves the
//! bottom and doglegs, parallel arrows get stacked on the same line, and routes hug node
//! outlines instead of running down the middle of the gap.
//!
//! This router is the standard remedy (libavoid / ELK style), kept deterministic:
//!
//! 1. **Grid.** Candidate coordinates are obstacle outlines (every node inflated by a
//!    clearance, plus group title bands), the midpoints of the gaps between them, group
//!    borders, and every candidate port position. Moves run along these lines between
//!    neighbouring coordinates and never enter an obstacle.
//! 2. **Search.** Each edge is an A* search from *every* candidate port on *every* face
//!    of its source to any on its target — so the faces fall out of the search instead of
//!    being guessed. Cost = length + bends + crossings with already-routed edges + running
//!    in another edge's lane + riding a group border + reusing a taken port.
//! 3. **Rip-up and reroute.** Edges are routed shortest-first, then each is routed
//!    again with all the others in place, so early edges stop blocking better paths.
//! 4. **Nudging.** Interior segments slide to the centre of their free corridor, and
//!    segments sharing a corridor are fanned out into evenly spaced parallel lanes, in the
//!    order that avoids crossing where they peel off.
//!
//! Output is the same [`EdgeRoutingPlan`] every downstream stage (review, polish, both
//! render backends) already consumes. Edges it can't route (self-loops, a node boxed in
//! completely) are left to the caller's fallback.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use petgraph::stable_graph::{EdgeIndex, NodeIndex};

use rdg_graph::CompiledGraph;
use rdg_layout::{DesignTokens, LayoutResult, NodeLayout};

use crate::routing::{EdgeRoutingPlan, Side, resolve_edge_layout};

type Pt = (f64, f64);
/// Candidate ports on one node face: fraction along it, port point, grid point.
type FacePins = HashMap<(NodeIndex, Side), Vec<(f64, Pt, Pt)>>;

// Cost weights, in px-equivalents: a bend "costs" as much as 40px of extra length, a
// crossing as much as 60px — so a route takes a modest detour to save a crossing but not
// a lap of the canvas.
const W_BEND: f64 = 40.0;
const W_CROSS: f64 = 60.0;
/// Per px of running in the same lane as another edge.
const W_SHARE: f64 = 1.5;
/// Per px of running within a lane's width of a parallel route (a corridor filling up).
const W_CROWD: f64 = 0.6;
/// Parallel lines closer than this are drawn as one smeared line.
const SAME_LANE: f64 = 4.0;
/// Per px of running along a group's dashed border.
const W_ALONG_BORDER: f64 = 2.0;
/// A crossing within [`NEAR_BEND`] of a bend or port of either route — where a reader
/// can't tell which line turns and which carries on.
const W_CROSS_NEAR_BEND: f64 = 70.0;
const NEAR_BEND: f64 = 20.0;
/// Each time a route crosses a group border.
const W_BORDER_CROSS: f64 = 12.0;
/// Two edges using the same port, or ports closer than the minimum pitch.
const W_PORT_TAKEN: f64 = 400.0;
/// Per px a port sits off its face's centre (tie-break toward centred ports).
const W_OFF_CENTRE: f64 = 0.08;
/// Leaving/entering a node against the diagram's main flow (e.g. a side face in a
/// top-to-bottom diagram) — small, so geometry still decides.
const W_SIDE: f64 = 6.0;
/// Attaching to an icon node's bottom face, under its label.
const W_UNDER_LABEL: f64 = 700.0;
/// Rip-up-and-reroute rounds after the initial pass.
const REROUTE_ROUNDS: usize = 2;
/// Refuse (and let the caller fall back) past this many grid points.
const MAX_GRID_POINTS: usize = 1_500_000;

#[derive(Debug, Clone, Copy)]
struct Rect {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Rect {
    fn of(nl: &NodeLayout) -> Self {
        Rect { x0: nl.x, y0: nl.y, x1: nl.x + nl.width, y1: nl.y + nl.height }
    }
    fn inflate(self, d: f64) -> Self {
        Rect { x0: self.x0 - d, y0: self.y0 - d, x1: self.x1 + d, y1: self.y1 + d }
    }
    fn strictly_contains(&self, p: Pt) -> bool {
        p.0 > self.x0 + 1e-6 && p.0 < self.x1 - 1e-6 && p.1 > self.y0 + 1e-6 && p.1 < self.y1 - 1e-6
    }
}

/// Direction of travel. `Right`/`Left` are horizontal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dir {
    Right = 0,
    Down = 1,
    Left = 2,
    Up = 3,
}

impl Dir {
    const ALL: [Dir; 4] = [Dir::Right, Dir::Down, Dir::Left, Dir::Up];
    fn horizontal(self) -> bool {
        matches!(self, Dir::Right | Dir::Left)
    }
    fn opposite(self) -> Dir {
        Dir::ALL[(self as usize + 2) % 4]
    }
    fn step(self) -> (isize, isize) {
        match self {
            Dir::Right => (1, 0),
            Dir::Down => (0, 1),
            Dir::Left => (-1, 0),
            Dir::Up => (0, -1),
        }
    }
    /// Direction pointing out of `side`.
    fn out_of(side: Side) -> Dir {
        match side {
            Side::Top => Dir::Up,
            Side::Bottom => Dir::Down,
            Side::Left => Dir::Left,
            Side::Right => Dir::Right,
        }
    }
}

/// One candidate attachment: which face, where along it (fraction), and the grid point
/// just outside the node's clearance where the route proper starts/ends.
#[derive(Debug, Clone, Copy)]
struct Pin {
    node: NodeIndex,
    side: Side,
    port: Pt,
    grid: (usize, usize),
    /// Fixed cost of using this pin (stub length, off-centre, off-flow face).
    cost: f64,
}

struct Grid {
    xs: Vec<f64>,
    ys: Vec<f64>,
    /// Point is strictly inside an obstacle.
    blocked: Vec<bool>,
    /// Move from (i, j) to (i + 1, j) passes through an obstacle.
    hblock: Vec<bool>,
    /// Move from (i, j) to (i, j + 1) passes through an obstacle.
    vblock: Vec<bool>,
    /// Point lies on a group's vertical / horizontal border line.
    on_vborder: Vec<bool>,
    on_hborder: Vec<bool>,
    /// Point lies within a lane's width of a group's vertical / horizontal border line
    /// (on it included) — running parallel there reads as tracing the container.
    near_vborder: Vec<bool>,
    near_hborder: Vec<bool>,
}

impl Grid {
    fn id(&self, i: usize, j: usize) -> usize {
        j * self.xs.len() + i
    }
    fn pt(&self, i: usize, j: usize) -> Pt {
        (self.xs[i], self.ys[j])
    }
    fn index_of(v: &[f64], x: f64) -> Option<usize> {
        let k = v.partition_point(|&a| a < x - 0.25);
        (k < v.len() && (v[k] - x).abs() <= 0.25).then_some(k)
    }
}

/// Usage left behind by routed edges, consulted by later searches.
#[derive(Default, Clone)]
struct Occupancy {
    /// Per grid point: routes passing straight through horizontally / vertically (a bend
    /// counts as both).
    h_pass: HashMap<usize, u32>,
    v_pass: HashMap<usize, u32>,
    /// Per grid move (from the lower-index end): routes using it.
    h_move: HashMap<usize, u32>,
    v_move: HashMap<usize, u32>,
    /// Per grid point: routes bending there, or starting/ending there (pins).
    bends: HashMap<usize, u32>,
    /// Ports in use, per node face: their position along the face.
    ports: HashMap<(NodeIndex, Side), Vec<f64>>,
}

impl Occupancy {
    /// Ports on `pin`'s face closer than `pitch` to it (the same port counts too).
    fn ports_near(&self, pin: &Pin, pitch: f64) -> usize {
        let at = along(pin);
        self.ports.get(&(pin.node, pin.side)).map_or(0, |v| v.iter().filter(|&&p| (p - at).abs() < pitch).count())
    }
}

/// A pin's position along its face (x for top/bottom, y for left/right).
fn along(pin: &Pin) -> f64 {
    match pin.side {
        Side::Top | Side::Bottom => pin.port.0,
        Side::Left | Side::Right => pin.port.1,
    }
}

/// A routed edge as grid indices, pin to pin (inclusive), plus the pins used.
#[derive(Debug, Clone)]
struct Route {
    cells: Vec<(usize, usize)>,
    src: Pin,
    dst: Pin,
}

fn apply(occ: &mut Occupancy, grid: &Grid, r: &Route, delta: i32) {
    let bump = |m: &mut HashMap<usize, u32>, k: usize| {
        let e = m.entry(k).or_insert(0);
        *e = (*e as i32 + delta).max(0) as u32;
    };
    for (k, &(i, j)) in r.cells.iter().enumerate() {
        let id = grid.id(i, j);
        let prev = k.checked_sub(1).map(|p| r.cells[p]);
        let next = r.cells.get(k + 1).copied();
        let horiz_in = prev.is_some_and(|p| p.1 == j);
        let horiz_out = next.is_some_and(|n| n.1 == j);
        let vert_in = prev.is_some_and(|p| p.0 == i);
        let vert_out = next.is_some_and(|n| n.0 == i);
        if horiz_in || horiz_out {
            bump(&mut occ.h_pass, id);
        }
        let turns = (horiz_in && vert_out) || (vert_in && horiz_out);
        if turns || k == 0 || k + 1 == r.cells.len() {
            bump(&mut occ.bends, id);
        }
        if vert_in || vert_out {
            bump(&mut occ.v_pass, id);
        }
        if let Some(n) = next {
            if n.1 == j {
                bump(&mut occ.h_move, grid.id(i.min(n.0), j));
            } else {
                bump(&mut occ.v_move, grid.id(i, j.min(n.1)));
            }
        }
    }
    for pin in [&r.src, &r.dst] {
        let v = occ.ports.entry((pin.node, pin.side)).or_default();
        if delta > 0 {
            v.push(along(pin));
        } else if let Some(k) = v.iter().position(|&p| (p - along(pin)).abs() < 1e-9) {
            v.remove(k);
        }
    }
}

#[derive(Copy, Clone, PartialEq)]
struct Open {
    f: f64,
    g: f64,
    state: usize,
}
impl Eq for Open {}
impl Ord for Open {
    fn cmp(&self, o: &Self) -> Ordering {
        // Min-heap on f, then g (prefer deeper), then state for determinism.
        o.f.total_cmp(&self.f).then(self.g.total_cmp(&o.g)).then(o.state.cmp(&self.state))
    }
}
impl PartialOrd for Open {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

struct Router<'a> {
    grid: Grid,
    tokens: &'a DesignTokens,
    clearance: f64,
    /// Minimum distance between two ports on one face.
    pitch: f64,
}

impl Router<'_> {
    /// Cost per px of running parallel to routes on other grid lines within a lane
    /// width: lines under [`SAME_LANE`] apart read as one line, so they cost like sharing
    /// it outright; further ones cost the lighter crowding rate.
    fn crowding(&self, i: usize, j: usize, ni: usize, nj: usize, horizontal: bool, occ: &Occupancy) -> f64 {
        let g = &self.grid;
        let lane = self.tokens.channel_pitch().max(10.0);
        let rate = |d: f64, k: u32| k as f64 * if d < SAME_LANE { W_SHARE } else { W_CROWD };
        let mut c = 0.0;
        if horizontal {
            let (a, y) = (i.min(ni), g.ys[j]);
            for dir in [-1isize, 1] {
                let mut jj = j as isize + dir;
                while jj >= 0 && (jj as usize) < g.ys.len() && (g.ys[jj as usize] - y).abs() < lane {
                    c += rate((g.ys[jj as usize] - y).abs(), occ.h_move.get(&g.id(a, jj as usize)).copied().unwrap_or(0));
                    jj += dir;
                }
            }
        } else {
            let (b, x) = (j.min(nj), g.xs[i]);
            for dir in [-1isize, 1] {
                let mut ii = i as isize + dir;
                while ii >= 0 && (ii as usize) < g.xs.len() && (g.xs[ii as usize] - x).abs() < lane {
                    c += rate((g.xs[ii as usize] - x).abs(), occ.v_move.get(&g.id(ii as usize, b)).copied().unwrap_or(0));
                    ii += dir;
                }
            }
        }
        c
    }

    /// Whether some route bends or ends within [`NEAR_BEND`] of grid point `(i, j)`.
    fn bend_near(&self, i: usize, j: usize, occ: &Occupancy) -> bool {
        let g = &self.grid;
        let (x, y) = g.pt(i, j);
        let hit = |ii: usize, jj: usize| occ.bends.get(&g.id(ii, jj)).is_some_and(|&k| k > 0);
        if hit(i, j) {
            return true;
        }
        for dir in [-1isize, 1] {
            let mut ii = i as isize + dir;
            while ii >= 0 && (ii as usize) < g.xs.len() && (g.xs[ii as usize] - x).abs() < NEAR_BEND {
                if hit(ii as usize, j) {
                    return true;
                }
                ii += dir;
            }
            let mut jj = j as isize + dir;
            while jj >= 0 && (jj as usize) < g.ys.len() && (g.ys[jj as usize] - y).abs() < NEAR_BEND {
                if hit(i, jj as usize) {
                    return true;
                }
                jj += dir;
            }
        }
        false
    }

    /// A* from any `srcs` pin to any `dsts` pin. `None` when unreachable.
    fn search(&self, srcs: &[Pin], dsts: &[Pin], occ: &Occupancy) -> Option<Route> {
        let g = &self.grid;
        let (nx, ny) = (g.xs.len(), g.ys.len());
        let n_states = nx * ny * 4;
        let mut best = vec![f64::INFINITY; n_states];
        let mut parent: Vec<u32> = vec![u32::MAX; n_states];
        let mut origin: Vec<u16> = vec![u16::MAX; n_states];
        let mut heap = BinaryHeap::new();

        // Terminal lookup: grid point -> (pin index, inward direction).
        let mut goal: HashMap<usize, Vec<usize>> = HashMap::new();
        for (k, p) in dsts.iter().enumerate() {
            goal.entry(g.id(p.grid.0, p.grid.1)).or_default().push(k);
        }
        let h = |i: usize, j: usize| {
            let (x, y) = g.pt(i, j);
            dsts.iter()
                .map(|p| (x - g.xs[p.grid.0]).abs() + (y - g.ys[p.grid.1]).abs())
                .fold(f64::INFINITY, f64::min)
        };
        let state = |i: usize, j: usize, d: Dir| (g.id(i, j)) * 4 + d as usize;

        for (k, p) in srcs.iter().enumerate() {
            let d = Dir::out_of(p.side);
            let taken = occ.ports_near(p, self.pitch) as f64 * W_PORT_TAKEN;
            let s = state(p.grid.0, p.grid.1, d);
            let cost = p.cost + taken;
            if cost < best[s] {
                best[s] = cost;
                origin[s] = k as u16;
                heap.push(Open { f: cost + h(p.grid.0, p.grid.1), g: cost, state: s });
            }
        }

        let mut done: Option<(f64, usize, usize)> = None; // (total, state, dst pin)
        while let Some(Open { f, g: gc, state: s }) = heap.pop() {
            if gc > best[s] + 1e-9 {
                continue;
            }
            if done.is_some_and(|(t, _, _)| f >= t - 1e-9) {
                break;
            }
            let cell = s / 4;
            let d = Dir::ALL[s % 4];
            let (i, j) = (cell % nx, cell / nx);

            if let Some(pins) = goal.get(&cell) {
                for &k in pins {
                    let p = &dsts[k];
                    let inward = Dir::out_of(p.side).opposite();
                    if d == inward.opposite() {
                        continue;
                    }
                    let taken = occ.ports_near(p, self.pitch) as f64 * W_PORT_TAKEN;
                    let total = gc + p.cost + taken + if d != inward { W_BEND } else { 0.0 };
                    if done.is_none_or(|(t, _, _)| total < t - 1e-9) {
                        done = Some((total, s, k));
                    }
                }
            }

            for nd in Dir::ALL {
                if nd == d.opposite() {
                    continue;
                }
                let (di, dj) = nd.step();
                let (ni, nj) = (i as isize + di, j as isize + dj);
                if ni < 0 || nj < 0 || ni as usize >= nx || nj as usize >= ny {
                    continue;
                }
                let (ni, nj) = (ni as usize, nj as usize);
                let nid = g.id(ni, nj);
                if g.blocked[nid] {
                    continue;
                }
                let (blocked, shared) = if nd.horizontal() {
                    let m = g.id(i.min(ni), j);
                    (g.hblock[m], occ.h_move.get(&m))
                } else {
                    let m = g.id(i, j.min(nj));
                    (g.vblock[m], occ.v_move.get(&m))
                };
                if blocked {
                    continue;
                }
                let len = (g.xs[ni] - g.xs[i]).abs() + (g.ys[nj] - g.ys[j]).abs();
                let mut c = len;
                if nd != d {
                    c += W_BEND;
                }
                if let Some(&k) = shared {
                    c += W_SHARE * len * k as f64;
                }
                // Crowding: parallel routes on nearby grid lines within one lane width.
                c += len * self.crowding(i, j, ni, nj, nd.horizontal(), occ);
                // Crossing: another route passes perpendicular through the point we reach.
                let perp = if nd.horizontal() { occ.v_pass.get(&nid) } else { occ.h_pass.get(&nid) };
                if let Some(&k) = perp.filter(|&&k| k > 0) {
                    c += W_CROSS * k as f64;
                    // …worse when it lands right at a bend or port (theirs), or just
                    // after our own bend.
                    if self.bend_near(ni, nj, occ) || (nd != d && len < NEAR_BEND) {
                        c += W_CROSS_NEAR_BEND;
                    }
                }
                // Turning on top of another route that passes straight through here.
                if nd != d {
                    let here = g.id(i, j);
                    let through = occ.h_pass.get(&here).copied().unwrap_or(0) + occ.v_pass.get(&here).copied().unwrap_or(0);
                    if through > 0 {
                        c += W_CROSS_NEAR_BEND;
                    }
                }
                if nd.horizontal() {
                    if g.on_vborder[nid] {
                        c += W_BORDER_CROSS;
                    }
                    if g.near_hborder[nid] && g.near_hborder[g.id(i, j)] {
                        c += W_ALONG_BORDER * len;
                    }
                } else {
                    if g.on_hborder[nid] {
                        c += W_BORDER_CROSS;
                    }
                    if g.near_vborder[nid] && g.near_vborder[g.id(i, j)] {
                        c += W_ALONG_BORDER * len;
                    }
                }
                let ns = state(ni, nj, nd);
                let ng = gc + c;
                if ng < best[ns] - 1e-9 {
                    best[ns] = ng;
                    parent[ns] = s as u32;
                    origin[ns] = origin[s];
                    heap.push(Open { f: ng + h(ni, nj), g: ng, state: ns });
                }
            }
        }

        let (_, end, k) = done?;
        let mut cells = Vec::new();
        let mut s = end;
        loop {
            let cell = s / 4;
            cells.push((cell % nx, cell / nx));
            if parent[s] == u32::MAX {
                break;
            }
            s = parent[s] as usize;
        }
        cells.reverse();
        cells.dedup();
        let src = srcs[origin[end] as usize];
        Some(Route { cells, src, dst: dsts[k] })
    }
}

/// Candidate pins on every face of `nl`, keeping `pitch` between neighbouring ports.
/// `align` holds coordinates worth offering as a port (e.g. the other endpoint's centre,
/// for a straight line) when they fall on the face.
fn face_fracs(len: f64, pitch: f64, align: &[f64], start: f64, (lo, hi): (f64, f64)) -> Vec<f64> {
    let n = (((hi - lo) * len / pitch).floor() as usize).clamp(0, 6);
    let mut out = vec![0.5];
    if n >= 2 {
        for k in 0..=n {
            out.push(lo + (hi - lo) * k as f64 / n as f64);
        }
    }
    for &a in align {
        let f = (a - start) / len;
        if (lo - 0.03..=hi + 0.03).contains(&f) {
            out.push(f);
        }
    }
    out.sort_by(f64::total_cmp);
    out.dedup_by(|a, b| (*a - *b).abs() * len < 1.0);
    out
}

/// Routes every edge it can. Returns plans keyed by edge; edges missing from the result
/// (self-loops, unreachable, oversize grid) should go to a fallback router.
pub fn route_orthogonal(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    tokens: &DesignTokens,
) -> HashMap<EdgeIndex, EdgeRoutingPlan> {
    let clearance = tokens.stub_clearance();
    let pitch = tokens.min_port_pitch().max(tokens.px(2.0));
    let tb = !layout.positions.is_empty() && is_top_to_bottom(compiled, layout);

    // --- Edges to route --------------------------------------------------------
    let mut todo: Vec<(EdgeIndex, NodeIndex, NodeIndex)> = Vec::new();
    for e in compiled.graph.edge_indices() {
        if let Some((s, d, _, _)) = resolve_edge_layout(compiled, layout, e) {
            if s != d {
                todo.push((e, s, d));
            }
        }
    }
    if todo.is_empty() {
        return HashMap::new();
    }

    // --- Obstacles -----------------------------------------------------------------
    let mut node_ids: Vec<NodeIndex> = layout.positions.keys().copied().collect();
    node_ids.sort();
    let mut obstacles: Vec<Rect> = node_ids.iter().map(|n| Rect::of(&layout.positions[n]).inflate(clearance)).collect();
    for tz in crate::routing::compute_group_title_zones(compiled, layout, tokens) {
        obstacles.push(Rect { x0: tz.min_x, y0: tz.min_y, x1: tz.max_x, y1: tz.max_y - tokens.px(0.5) });
    }
    let groups: Vec<Rect> = rdg_layout::groups::group_rects(compiled, &layout.positions, tokens)
        .into_iter()
        .flatten()
        .map(|(x, y, w, h)| Rect { x0: x, y0: y, x1: x + w, y1: y + h })
        .collect();

    // --- Candidate pins per node face -------------------------------------------------
    // Alignment hints: for each node, the centres of the nodes it connects to.
    let mut align_x: HashMap<NodeIndex, Vec<f64>> = HashMap::new();
    let mut align_y: HashMap<NodeIndex, Vec<f64>> = HashMap::new();
    for &(_, s, d) in &todo {
        let (a, b) = (&layout.positions[&s], &layout.positions[&d]);
        for (n, o) in [(s, b), (d, a)] {
            align_x.entry(n).or_default().push(o.x + o.width / 2.0);
            align_y.entry(n).or_default().push(o.y + o.height / 2.0);
        }
        // Where the two boxes' spans overlap, the middle of that overlap is a port both
        // faces can use — a dead-straight arrow instead of a jog between two nearly
        // aligned ports.
        let (ox0, ox1) = (a.x.max(b.x), (a.x + a.width).min(b.x + b.width));
        if ox1 > ox0 {
            for n in [s, d] {
                align_x.entry(n).or_default().push((ox0 + ox1) / 2.0);
            }
        }
        let (oy0, oy1) = (a.y.max(b.y), (a.y + a.height).min(b.y + b.height));
        if oy1 > oy0 {
            for n in [s, d] {
                align_y.entry(n).or_default().push((oy0 + oy1) / 2.0);
            }
        }
    }
    let mut face_pts: FacePins = HashMap::new();
    let mut xs: Vec<f64> = Vec::new();
    let mut ys: Vec<f64> = Vec::new();
    for &n in &node_ids {
        let nl = &layout.positions[&n];
        let r = Rect::of(nl);
        let ax = align_x.get(&n).map_or(&[][..], |v| v.as_slice());
        let ay = align_y.get(&n).map_or(&[][..], |v| v.as_slice());
        for side in [Side::Top, Side::Bottom, Side::Left, Side::Right] {
            let horizontal_face = matches!(side, Side::Top | Side::Bottom);
            // Where along this face a port may sit: near the middle of a curved face (an
            // arrow far off-centre meets an ellipse or diamond at a glancing angle), and on
            // a cylinder's straight body — below its top cap, above its bottom curve.
            let range = match crate::style::outline_of(&compiled.graph[n], tokens) {
                crate::style::Outline::Ellipse | crate::style::Outline::Diamond => (0.3, 0.7),
                crate::style::Outline::Cylinder { cap } if !horizontal_face => {
                    (((2.0 * cap + 6.0) / nl.height).max(0.15), (1.0 - (cap + 6.0) / nl.height).min(0.85))
                }
                crate::style::Outline::Captioned { mark, .. } => {
                    // The middle of the mark's face (where a curve is near-vertical /
                    // near-horizontal); the bottom under the caption keeps to its middle.
                    let r = mark / 2.0;
                    match side {
                        Side::Left | Side::Right => (((r - 0.6 * r) / nl.height).max(0.02), ((r + 0.6 * r) / nl.height).min(0.95)),
                        Side::Top => {
                            let half = (0.6 * r / nl.width).min(0.4);
                            (0.5 - half, 0.5 + half)
                        }
                        Side::Bottom => (0.35, 0.65),
                    }
                }
                _ => (0.15, 0.85),
            };
            let fracs = if horizontal_face {
                face_fracs(nl.width, pitch, ax, nl.x, range)
            } else {
                face_fracs(nl.height, pitch, ay, nl.y, range)
            };
            let mut v = Vec::new();
            for f in fracs {
                let (port, gp) = match side {
                    Side::Top => ((r.x0 + nl.width * f, r.y0), (r.x0 + nl.width * f, r.y0 - clearance)),
                    Side::Bottom => ((r.x0 + nl.width * f, r.y1), (r.x0 + nl.width * f, r.y1 + clearance)),
                    Side::Left => ((r.x0, r.y0 + nl.height * f), (r.x0 - clearance, r.y0 + nl.height * f)),
                    Side::Right => ((r.x1, r.y0 + nl.height * f), (r.x1 + clearance, r.y0 + nl.height * f)),
                };
                // A pin inside some other obstacle (a neighbour too close) is unusable.
                if obstacles.iter().any(|o| o.strictly_contains(gp)) {
                    continue;
                }
                v.push((f, port, gp));
            }
            face_pts.insert((n, side), v);
        }
    }
    // Ports a pixel or two apart on different nodes would give two stubs on grid lines
    // that close — drawn, one thick smeared line the search can't see as shared. Merge
    // along-face coordinates within `SNAP` into one line (moving a port ≤ SNAP/2 px is
    // invisible), so the router charges for — and avoids — actually sharing it.
    const SNAP: f64 = 3.0;
    let snap_table = |vals: Vec<f64>| -> Vec<(f64, f64)> {
        let mut v = vals;
        v.sort_by(f64::total_cmp);
        let mut out: Vec<(f64, f64)> = Vec::new();
        let mut k = 0;
        while k < v.len() {
            let mut e = k;
            while e + 1 < v.len() && v[e + 1] - v[k] <= SNAP {
                e += 1;
            }
            let rep = v[k..=e].iter().sum::<f64>() / (e - k + 1) as f64;
            for &x in &v[k..=e] {
                out.push((x, rep));
            }
            k = e + 1;
        }
        out
    };
    let lookup = |t: &[(f64, f64)], x: f64| t.iter().find(|(a, _)| (a - x).abs() < 1e-9).map_or(x, |&(_, r)| r);
    let along_x = snap_table(face_pts.iter().filter(|((_, s), _)| matches!(s, Side::Top | Side::Bottom)).flat_map(|(_, v)| v.iter().map(|p| p.2.0)).collect());
    let along_y = snap_table(face_pts.iter().filter(|((_, s), _)| matches!(s, Side::Left | Side::Right)).flat_map(|(_, v)| v.iter().map(|p| p.2.1)).collect());
    for ((n, side), v) in face_pts.iter_mut() {
        let nl = &layout.positions[n];
        for (f, port, gp) in v.iter_mut() {
            if matches!(side, Side::Top | Side::Bottom) {
                gp.0 = lookup(&along_x, gp.0);
                port.0 = gp.0;
                *f = (gp.0 - nl.x) / nl.width;
            } else {
                gp.1 = lookup(&along_y, gp.1);
                port.1 = gp.1;
                *f = (gp.1 - nl.y) / nl.height;
            }
            xs.push(gp.0);
            ys.push(gp.1);
        }
    }

    // --- Grid coordinates -------------------------------------------------------------
    for o in &obstacles {
        xs.extend([o.x0, o.x1]);
        ys.extend([o.y0, o.y1]);
    }
    for gr in &groups {
        xs.extend([gr.x0, gr.x1]);
        ys.extend([gr.y0, gr.y1]);
    }
    let (min_x, max_x) = xs.iter().fold((f64::MAX, f64::MIN), |(a, b), &x| (a.min(x), b.max(x)));
    let (min_y, max_y) = ys.iter().fold((f64::MAX, f64::MIN), |(a, b), &y| (a.min(y), b.max(y)));
    let outer = tokens.px(2.0);
    xs.extend([min_x - outer, max_x + outer]);
    ys.extend([min_y - outer, max_y + outer]);
    // Pin coordinates must survive exactly (they are where arrows attach).
    let pin_xs: Vec<f64> = face_pts.values().flat_map(|v| v.iter().map(|p| p.2.0)).collect();
    let pin_ys: Vec<f64> = face_pts.values().flat_map(|v| v.iter().map(|p| p.2.1)).collect();
    let with_midpoints = |mut v: Vec<f64>, pins: &[f64]| {
        v.sort_by(f64::total_cmp);
        v.dedup_by(|a, b| (*a - *b).abs() < 0.5);
        let mids: Vec<f64> = v.windows(2).filter(|w| w[1] - w[0] > 2.0 * clearance).map(|w| (w[0] + w[1]) / 2.0).collect();
        v.extend(mids);
        v.sort_by(f64::total_cmp);
        // Lines a few px apart would let two routes run visibly on top of each other
        // while the search sees separate lanes; collapse each such cluster to one line —
        // a pin's exact coordinate when the cluster has one, else its middle.
        let mut out: Vec<f64> = Vec::with_capacity(v.len());
        let mut k = 0;
        while k < v.len() {
            let mut e = k;
            while e + 1 < v.len() && v[e + 1] - v[k] <= 4.0 {
                e += 1;
            }
            let cluster = &v[k..=e];
            let pinned: Vec<f64> = cluster.iter().copied().filter(|c| pins.iter().any(|p| (p - c).abs() < 0.25)).collect();
            if pinned.is_empty() {
                out.push((cluster[0] + cluster[cluster.len() - 1]) / 2.0);
            } else {
                out.extend(pinned);
            }
            k = e + 1;
        }
        out.dedup_by(|a, b| (*a - *b).abs() < 0.25);
        out
    };
    let xs = with_midpoints(xs, &pin_xs);
    let ys = with_midpoints(ys, &pin_ys);
    if xs.len() * ys.len() > MAX_GRID_POINTS {
        return HashMap::new();
    }

    let (nx, ny) = (xs.len(), ys.len());
    let mut blocked = vec![false; nx * ny];
    let mut hblock = vec![false; nx * ny];
    let mut vblock = vec![false; nx * ny];
    let mut on_vborder = vec![false; nx * ny];
    let mut on_hborder = vec![false; nx * ny];
    let mut near_vborder = vec![false; nx * ny];
    let mut near_hborder = vec![false; nx * ny];
    let border_lane = tokens.channel_pitch().max(10.0);
    for j in 0..ny {
        for i in 0..nx {
            let id = j * nx + i;
            let p = (xs[i], ys[j]);
            blocked[id] = obstacles.iter().any(|o| o.strictly_contains(p));
            if i + 1 < nx {
                let m = ((xs[i] + xs[i + 1]) / 2.0, ys[j]);
                hblock[id] = obstacles.iter().any(|o| o.strictly_contains(m));
            }
            if j + 1 < ny {
                let m = (xs[i], (ys[j] + ys[j + 1]) / 2.0);
                vblock[id] = obstacles.iter().any(|o| o.strictly_contains(m));
            }
            on_vborder[id] = groups.iter().any(|g| ((p.0 - g.x0).abs() < 0.5 || (p.0 - g.x1).abs() < 0.5) && p.1 >= g.y0 && p.1 <= g.y1);
            on_hborder[id] = groups.iter().any(|g| ((p.1 - g.y0).abs() < 0.5 || (p.1 - g.y1).abs() < 0.5) && p.0 >= g.x0 && p.0 <= g.x1);
            near_vborder[id] = groups.iter().any(|g| {
                ((p.0 - g.x0).abs() < border_lane || (p.0 - g.x1).abs() < border_lane) && p.1 >= g.y0 && p.1 <= g.y1
            });
            near_hborder[id] = groups.iter().any(|g| {
                ((p.1 - g.y0).abs() < border_lane || (p.1 - g.y1).abs() < border_lane) && p.0 >= g.x0 && p.0 <= g.x1
            });
        }
    }
    let grid = Grid { xs, ys, blocked, hblock, vblock, on_vborder, on_hborder, near_vborder, near_hborder };
    let router = Router { grid, tokens, clearance, pitch };

    let pins_for = |n: NodeIndex| -> Vec<Pin> {
        let nl = &layout.positions[&n];
        let mut out = Vec::new();
        for side in [Side::Top, Side::Bottom, Side::Left, Side::Right] {
            let face_len = if matches!(side, Side::Top | Side::Bottom) { nl.width } else { nl.height };
            let off_flow = match (tb, side) {
                (true, Side::Left | Side::Right) | (false, Side::Top | Side::Bottom) => W_SIDE,
                _ => 0.0,
            };
            // A captioned node's bottom face is under its caption: a last resort for a logo
            // node; for a flowchart marker (a short caption, and the way the flow runs) worth
            // one bend at most.
            let under_label = match (side, crate::style::outline_of(&compiled.graph[n], tokens)) {
                (Side::Bottom, crate::style::Outline::Captioned { .. }) if compiled.graph[n].shape.as_deref() == Some("icon") => W_UNDER_LABEL,
                (Side::Bottom, crate::style::Outline::Captioned { .. }) => W_BEND,
                _ => 0.0,
            };
            for &(frac, port, gp) in face_pts.get(&(n, side)).map_or(&[][..], |v| v.as_slice()) {
                let (Some(i), Some(j)) = (Grid::index_of(&router.grid.xs, gp.0), Grid::index_of(&router.grid.ys, gp.1)) else {
                    continue;
                };
                out.push(Pin {
                    node: n,
                    side,
                    port,
                    grid: (i, j),
                    cost: clearance + W_OFF_CENTRE * (frac - 0.5).abs() * face_len + off_flow + under_label,
                });
            }
        }
        out
    };

    // --- Route: initial pass shortest-first, then rip-up-and-reroute -----------------------
    let centre = |n: NodeIndex| {
        let nl = &layout.positions[&n];
        (nl.x + nl.width / 2.0, nl.y + nl.height / 2.0)
    };
    todo.sort_by(|a, b| {
        let da = { let (p, q) = (centre(a.1), centre(a.2)); (p.0 - q.0).abs() + (p.1 - q.1).abs() };
        let db = { let (p, q) = (centre(b.1), centre(b.2)); (p.0 - q.0).abs() + (p.1 - q.1).abs() };
        da.total_cmp(&db).then(a.0.cmp(&b.0))
    });
    let mut occ = Occupancy::default();
    let mut routes: HashMap<EdgeIndex, Route> = HashMap::new();
    for round in 0..=REROUTE_ROUNDS {
        for &(e, s, d) in &todo {
            if round > 0 {
                if let Some(old) = routes.get(&e) {
                    apply(&mut occ, &router.grid, old, -1);
                }
            }
            // An explicit `source_port`/`target_port` restricts that end to one face.
            let only = |pins: Vec<Pin>, side: Option<Side>| -> Vec<Pin> {
                let kept: Vec<Pin> = pins.iter().copied().filter(|p| side.is_none_or(|sd| p.side == sd)).collect();
                if kept.is_empty() { pins } else { kept }
            };
            let data = &compiled.graph[e];
            let src_pins = only(pins_for(s), data.source_port.as_deref().and_then(Side::parse));
            let dst_pins = only(pins_for(d), data.target_port.as_deref().and_then(Side::parse));
            if let Some(r) = router.search(&src_pins, &dst_pins, &occ) {
                apply(&mut occ, &router.grid, &r, 1);
                routes.insert(e, r);
            } else if let Some(old) = routes.get(&e) {
                apply(&mut occ, &router.grid, old, 1);
            }
        }
    }

    // --- Polylines, nudged ---------------------------------------------------------------
    let mut paths: Vec<(EdgeIndex, Vec<Pt>, Pin, Pin)> = Vec::new();
    for &(e, _, _) in &todo {
        let Some(r) = routes.get(&e) else { continue };
        let mut pts: Vec<Pt> = vec![r.src.port];
        pts.extend(r.cells.iter().map(|&(i, j)| router.grid.pt(i, j)));
        pts.push(r.dst.port);
        paths.push((e, simplify(pts), r.src, r.dst));
    }
    // Lanes may run closer to a node than a route's search clearance: that clearance
    // keeps stubs readable, but a nudged lane only needs to stay visibly off the box.
    let lane_margin = tokens.channel_pitch().max(10.0);
    let mut nudge_obstacles: Vec<Rect> =
        node_ids.iter().map(|n| Rect::of(&layout.positions[n]).inflate(lane_margin)).collect();
    nudge_obstacles.extend_from_slice(&obstacles[node_ids.len()..]);
    remove_jogs(&mut paths, &nudge_obstacles, layout, tokens.polish_min_jog(), pitch);
    let debug = std::env::var_os("RDG_DEBUG_ORTHO").is_some();
    let before: Vec<Vec<Pt>> = if debug { paths.iter().map(|p| p.1.clone()).collect() } else { Vec::new() };
    let borders: Vec<(Pt, Pt)> = groups
        .iter()
        .flat_map(|g| {
            let (a, b, c, d) = ((g.x0, g.y0), (g.x1, g.y0), (g.x1, g.y1), (g.x0, g.y1));
            [(a, b), (b, c), (d, c), (a, d)]
        })
        .collect();
    nudge(&mut paths, &nudge_obstacles, &borders, &router, pitch);
    if debug {
        for ((e, after, _, _), b) in paths.iter().zip(&before) {
            let (s, d) = compiled.graph.edge_endpoints(*e).unwrap();
            eprintln!("ortho {} -> {}\n  routed {:?}\n  nudged {:?}", compiled.graph[s].id, compiled.graph[d].id, b, after);
        }
    }

    let mut out = HashMap::new();
    for (e, pts, src, dst) in paths {
        let waypoints: Vec<Pt> = pts[1..pts.len() - 1].to_vec();
        let channel_y = waypoints.first().map_or((src.port.1 + dst.port.1) / 2.0, |p| p.1);
        let corridor_x = waypoints.first().map_or((src.port.0 + dst.port.0) / 2.0, |p| p.0);
        out.insert(
            e,
            EdgeRoutingPlan {
                src_side: src.side,
                dst_side: dst.side,
                exit_port: face_frac(&layout.positions[&src.node], src.side, pts[0]),
                entry_port: face_frac(&layout.positions[&dst.node], dst.side, pts[pts.len() - 1]),
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

/// Whether the layout mostly flows downward (more edges go down than sideways).
fn is_top_to_bottom(compiled: &CompiledGraph, layout: &LayoutResult) -> bool {
    let (mut down, mut side) = (0, 0);
    for e in compiled.graph.edge_indices() {
        if let Some((_, _, a, b)) = resolve_edge_layout(compiled, layout, e) {
            let dy = ((b.y + b.height / 2.0) - (a.y + a.height / 2.0)).abs();
            let dx = ((b.x + b.width / 2.0) - (a.x + a.width / 2.0)).abs();
            if dy >= dx { down += 1 } else { side += 1 }
        }
    }
    down >= side
}

/// Drops collinear interior points and zero-length segments.
fn simplify(pts: Vec<Pt>) -> Vec<Pt> {
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

/// One interior segment of a routed path that may slide perpendicular to itself.
struct Seg {
    path: usize,
    /// Index of the segment's first point in the path (segment is `k..=k+1`).
    k: usize,
    horizontal: bool,
    /// Fixed coordinate (y for horizontal) and span along the other axis.
    at: f64,
    lo: f64,
    hi: f64,
    /// Free range for `at`.
    min: f64,
    max: f64,
}

/// Slides interior segments to the middle of their free corridor, and fans segments that
/// share a corridor out into evenly spaced lanes. First and last segments (the stubs
/// into ports) are left alone; only segments whose both ends are bends move.
fn nudge(paths: &mut [(EdgeIndex, Vec<Pt>, Pin, Pin)], obstacles: &[Rect], borders: &[(Pt, Pt)], router: &Router, pitch: f64) {
    let lane = router.tokens.channel_pitch().max(8.0);
    let min_leg = router.clearance * 0.6;
    for horizontal in [true, false] {
        let mut segs: Vec<Seg> = Vec::new();
        for (pi, (_, pts, _, _)) in paths.iter().enumerate() {
            if pts.len() < 4 {
                continue;
            }
            for k in 1..pts.len() - 2 {
                let (a, b) = (pts[k], pts[k + 1]);
                let is_h = (a.1 - b.1).abs() < 0.01;
                if is_h != horizontal {
                    continue;
                }
                let (at, lo, hi) = if horizontal { (a.1, a.0.min(b.0), a.0.max(b.0)) } else { (a.0, a.1.min(b.1), a.1.max(b.1)) };
                // Free range: nearest obstacle on each side across the segment's span.
                let (mut min, mut max) = (f64::NEG_INFINITY, f64::INFINITY);
                for o in obstacles {
                    let (s0, s1, c0, c1) = if horizontal { (o.x0, o.x1, o.y0, o.y1) } else { (o.y0, o.y1, o.x0, o.x1) };
                    if s1 < lo - 0.01 || s0 > hi + 0.01 {
                        continue;
                    }
                    if c1 <= at + 0.01 {
                        min = min.max(c1);
                    } else if c0 >= at - 0.01 {
                        max = max.min(c0);
                    }
                }
                // Don't fold the neighbouring legs: stay short of their far ends.
                // A leg ending at a port keeps its full stub length.
                let last = pts.len() - 1;
                for far_i in [k - 1, k + 2] {
                    let far = pts[far_i];
                    let leg = if far_i == 0 || far_i == last { router.clearance } else { min_leg };
                    let f = if horizontal { far.1 } else { far.0 };
                    if f < at {
                        min = min.max(f + leg);
                    } else {
                        max = max.min(f - leg);
                    }
                }
                // Other paths' fixed segments (port stubs) on this axis are lanes too.
                for (qi, (_, qpts, _, _)) in paths.iter().enumerate() {
                    if qi == pi || qpts.len() < 2 {
                        continue;
                    }
                    let last = qpts.len() - 2;
                    for fk in [0, last] {
                        if (1..qpts.len() - 2).contains(&fk) {
                            continue;
                        }
                        let (c, d) = (qpts[fk], qpts[fk + 1]);
                        if ((c.1 - d.1).abs() < 0.01) != horizontal {
                            continue;
                        }
                        let (fat, flo, fhi) = if horizontal { (c.1, c.0.min(d.0), c.0.max(d.0)) } else { (c.0, c.1.min(d.1), c.1.max(d.1)) };
                        if fhi < lo + 0.5 || flo > hi - 0.5 {
                            continue;
                        }
                        if fat <= at {
                            min = min.max(fat + lane);
                        } else {
                            max = max.min(fat - lane);
                        }
                    }
                }
                // Group borders parallel to this segment are lanes to keep off, too.
                for &(c, d) in borders {
                    if ((c.1 - d.1).abs() < 0.01) != horizontal {
                        continue;
                    }
                    let (fat, flo, fhi) = if horizontal { (c.1, c.0.min(d.0), c.0.max(d.0)) } else { (c.0, c.1.min(d.1), c.1.max(d.1)) };
                    if fhi < lo + 0.5 || flo > hi - 0.5 || (fat - at).abs() >= lane * 2.0 {
                        continue;
                    }
                    if fat <= at {
                        min = min.max(fat + lane);
                    } else {
                        max = max.min(fat - lane);
                    }
                }
                if !min.is_finite() || !max.is_finite() || min > max {
                    continue;
                }
                segs.push(Seg { path: pi, k, horizontal, at, lo, hi, min, max });
            }
        }
        if segs.is_empty() {
            continue;
        }
        // Group segments that overlap along the axis and share free space.
        let n = segs.len();
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
                let (s, t) = (&segs[a], &segs[b]);
                let overlap = s.lo.max(t.lo) < s.hi.min(t.hi) - 1.0;
                let shared_space = s.min.max(t.min) < s.max.min(t.max);
                if overlap && shared_space && (s.at - t.at).abs() < (s.max - s.min).max(t.max - t.min) {
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
        let mut new_at: Vec<f64> = segs.iter().map(|s| s.at).collect();
        for key in keys {
            let mut m = members[&key].clone();
            // Start from the router's order, then swap neighbours whenever the other
            // order makes their legs cross less where they join/leave the bundle.
            m.sort_by(|&a, &b| segs[a].at.total_cmp(&segs[b].at).then(segs[a].path.cmp(&segs[b].path)));
            let lane_gap = lane.max(pitch);
            for _ in 0..m.len() {
                let mut swapped = false;
                for k in 0..m.len().saturating_sub(1) {
                    let (a, b) = (m[k], m[k + 1]);
                    let mid = (segs[a].at + segs[b].at) / 2.0;
                    let keep = pair_crossings(&segs[a], &segs[b], paths, mid - lane_gap / 2.0, mid + lane_gap / 2.0);
                    let swap = pair_crossings(&segs[b], &segs[a], paths, mid - lane_gap / 2.0, mid + lane_gap / 2.0);
                    if swap < keep {
                        m.swap(k, k + 1);
                        swapped = true;
                    }
                }
                if !swapped {
                    break;
                }
            }
            let lo = m.iter().map(|&i| segs[i].min).fold(f64::NEG_INFINITY, f64::max);
            let hi = m.iter().map(|&i| segs[i].max).fold(f64::INFINITY, f64::min);
            let count = m.len() as f64;
            if hi > lo {
                let spacing = ((hi - lo) / (count + 1.0)).min(lane.max(pitch));
                let mid = (lo + hi) / 2.0;
                for (r, &i) in m.iter().enumerate() {
                    let target = mid + (r as f64 - (count - 1.0) / 2.0) * spacing;
                    new_at[i] = target.clamp(segs[i].min, segs[i].max);
                }
            } else {
                // No common free space: centre each in its own range only if alone.
                if m.len() == 1 {
                    let s = &segs[m[0]];
                    new_at[m[0]] = (s.min + s.max) / 2.0;
                }
            }
        }
        for (i, s) in segs.iter().enumerate() {
            let v = (new_at[i] * 2.0).round() / 2.0;
            let pts = &mut paths[s.path].1;
            if s.horizontal {
                pts[s.k].1 = v;
                pts[s.k + 1].1 = v;
            } else {
                pts[s.k].0 = v;
                pts[s.k + 1].0 = v;
            }
        }
    }
}

/// Crossings between two bundled segments' local geometry (the segment plus the legs
/// joining it at each end) when `first` sits at lane `at_first` and `second` at
/// `at_second`.
fn pair_crossings(first: &Seg, second: &Seg, paths: &[(EdgeIndex, Vec<Pt>, Pin, Pin)], at_first: f64, at_second: f64) -> usize {
    let local = |s: &Seg, at: f64| -> Vec<Pt> {
        let pts = &paths[s.path].1;
        let mut v = vec![pts[s.k - 1], pts[s.k], pts[s.k + 1], pts[s.k + 2]];
        if s.horizontal {
            v[1].1 = at;
            v[2].1 = at;
        } else {
            v[1].0 = at;
            v[2].0 = at;
        }
        v
    };
    let (pa, pb) = (local(first, at_first), local(second, at_second));
    let mut n = 0;
    for wa in pa.windows(2) {
        for wb in pb.windows(2) {
            if segments_touch(wa[0], wa[1], wb[0], wb[1]) {
                n += 1;
            }
        }
    }
    n
}

/// Whether two axis-aligned segments intersect or overlap (touching counts).
fn segments_touch(a0: Pt, a1: Pt, b0: Pt, b1: Pt) -> bool {
    let (ax0, ax1) = (a0.0.min(a1.0), a0.0.max(a1.0));
    let (ay0, ay1) = (a0.1.min(a1.1), a0.1.max(a1.1));
    let (bx0, bx1) = (b0.0.min(b1.0), b0.0.max(b1.0));
    let (by0, by1) = (b0.1.min(b1.1), b0.1.max(b1.1));
    ax0 <= bx1 + 0.01 && bx0 <= ax1 + 0.01 && ay0 <= by1 + 0.01 && by0 <= ay1 + 0.01
}

/// Fraction along `side` of `nl` at which point `p` sits.
fn face_frac(nl: &NodeLayout, side: Side, p: Pt) -> f64 {
    match side {
        Side::Top | Side::Bottom => (p.0 - nl.x) / nl.width,
        Side::Left | Side::Right => (p.1 - nl.y) / nl.height,
    }
}

/// Removes jogs — interior segments shorter than `min_jog`, i.e. two parallel legs a few
/// pixels apart joined by a stub — by moving one leg onto the other's line: an interior
/// leg only where the new line is clear of obstacles, a port leg only by sliding its port
/// along the face (kept inside it and `pitch` clear of the face's other ports).
fn remove_jogs(paths: &mut [(EdgeIndex, Vec<Pt>, Pin, Pin)], obstacles: &[Rect], layout: &LayoutResult, min_jog: f64, pitch: f64) {
    let clear = |a: Pt, b: Pt| {
        let n = 8;
        (0..=n).all(|k| {
            let t = k as f64 / n as f64;
            let p = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            !obstacles.iter().any(|o| o.strictly_contains(p))
        })
    };
    for pi in 0..paths.len() {
        for _ in 0..8 {
            let pts = paths[pi].1.clone();
            let n = pts.len();
            if n < 4 {
                break;
            }
            let Some(k) = (1..n - 2).find(|&k| {
                let (a, b) = (pts[k], pts[k + 1]);
                ((a.0 - b.0).abs() + (a.1 - b.1).abs()) < min_jog
            }) else {
                break;
            };
            // Legs: segment k-1 (pts[k-1]..pts[k]) and k+1 (pts[k+1]..pts[k+2]).
            let jog_horizontal = (pts[k].1 - pts[k + 1].1).abs() < 0.01;
            let coord = |p: Pt| if jog_horizontal { p.0 } else { p.1 };
            let set = |p: &mut Pt, v: f64| if jog_horizontal { p.0 = v } else { p.1 = v };
            let mut done = false;
            // Try moving the later leg onto the earlier one's line, then the reverse.
            for (leg_start, target) in [(k + 1, coord(pts[k])), (k - 1, coord(pts[k + 1]))] {
                let mut cand = pts.clone();
                set(&mut cand[leg_start], target);
                set(&mut cand[leg_start + 1], target);
                let is_port_leg = leg_start == 0 || leg_start + 1 == n - 1;
                if is_port_leg {
                    let (pin, port) = if leg_start == 0 { (&paths[pi].2, cand[0]) } else { (&paths[pi].3, cand[n - 1]) };
                    let nl = &layout.positions[&pin.node];
                    let f = face_frac(nl, pin.side, port);
                    if !(0.1..=0.9).contains(&f) {
                        continue;
                    }
                    let taken = paths.iter().enumerate().any(|(qi, (_, q, s, d))| {
                        qi != pi
                            && [(s, q[0]), (d, q[q.len() - 1])].iter().any(|(pp, qport)| {
                                pp.node == pin.node && pp.side == pin.side && (coord(*qport) - target).abs() < pitch
                            })
                    });
                    if taken {
                        continue;
                    }
                } else if !clear(cand[leg_start], cand[leg_start + 1]) {
                    continue;
                }
                paths[pi].1 = simplify(cand);
                done = true;
                break;
            }
            if !done {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        (compiled, LayoutResult { positions, sequence_info: None })
    }

    #[test]
    fn stacked_nodes_get_a_straight_line() {
        let (c, l) = layout_of(&[("a", 100.0, 100.0), ("b", 100.0, 300.0)], &[("a", "b")]);
        let plans = route_orthogonal(&c, &l, &DesignTokens::default());
        let p = plans.values().next().unwrap();
        assert_eq!((p.src_side, p.dst_side), (Side::Bottom, Side::Top));
        assert!(p.waypoints.is_empty(), "{p:?}");
    }

    #[test]
    fn offset_target_takes_one_bend_from_the_side() {
        // Target well below-left: leave the side, drop straight in — one bend.
        let (c, l) = layout_of(&[("a", 500.0, 100.0), ("b", 100.0, 400.0)], &[("a", "b")]);
        let plans = route_orthogonal(&c, &l, &DesignTokens::default());
        let p = plans.values().next().unwrap();
        assert_eq!(p.waypoints.len(), 1, "{p:?}");
        assert_eq!(p.dst_side, Side::Top);
    }

    #[test]
    fn routes_around_a_blocking_node() {
        let (c, l) = layout_of(&[("a", 100.0, 100.0), ("m", 100.0, 250.0), ("b", 100.0, 400.0)], &[("a", "b")]);
        let plans = route_orthogonal(&c, &l, &DesignTokens::default());
        let e = c.graph.edge_indices().next().unwrap();
        let p = &plans[&e];
        let m = &l.positions[&c.node_map["m"]];
        let src = crate::routing::port_point(&l.positions[&c.node_map["a"]], p.src_side, p.exit_port);
        let dst = crate::routing::port_point(&l.positions[&c.node_map["b"]], p.dst_side, p.entry_port);
        let mut pts = vec![src];
        pts.extend(&p.waypoints);
        pts.push(dst);
        for w in pts.windows(2) {
            let mid = ((w[0].0 + w[1].0) / 2.0, (w[0].1 + w[1].1) / 2.0);
            assert!(!Rect::of(m).strictly_contains(mid), "segment through blocker: {pts:?}");
        }
    }

    #[test]
    fn explicit_port_sides_are_honoured() {
        let compiled = rdg_graph::build_graph(&DiagramPayload {
            nodes: ["a", "b"].iter().map(|id| NodeDef { id: (*id).into(), label: (*id).into(), ..Default::default() }).collect(),
            edges: vec![EdgeDef {
                from: "a".into(),
                to: "b".into(),
                source_port: Some("right".into()),
                target_port: Some("right".into()),
                ..Default::default()
            }],
            ..Default::default()
        })
        .unwrap();
        let mut positions = HashMap::new();
        positions.insert(compiled.node_map["a"], NodeLayout { x: 100.0, y: 100.0, width: 120.0, height: 60.0 });
        positions.insert(compiled.node_map["b"], NodeLayout { x: 100.0, y: 300.0, width: 120.0, height: 60.0 });
        let plans = route_orthogonal(&compiled, &LayoutResult { positions, sequence_info: None }, &DesignTokens::default());
        let p = plans.values().next().unwrap();
        assert_eq!((p.src_side, p.dst_side), (Side::Right, Side::Right));
    }

    #[test]
    fn two_parallel_edges_do_not_share_a_lane() {
        let (c, l) = layout_of(
            &[("a", 100.0, 100.0), ("b", 260.0, 100.0), ("c", 100.0, 400.0), ("d", 260.0, 400.0)],
            &[("a", "d"), ("b", "c")],
        );
        let plans = route_orthogonal(&c, &l, &DesignTokens::default());
        let hs: Vec<f64> = plans.values().filter_map(|p| p.waypoints.first().map(|w| w.1)).collect();
        if hs.len() == 2 {
            assert!((hs[0] - hs[1]).abs() > 1.0, "{plans:?}");
        }
    }
}
