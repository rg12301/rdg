//! Placement of what is drawn *on* an edge: its text label and its flow-number badge.
//!
//! Both used to be pinned to a fixed spot — the label at the path's arc-length midpoint
//! lifted a flat 10px, the badge 14px straight out along the exit direction — so on a
//! vertical leg the label slid along its own line instead of off it, and every badge
//! sat squarely on its own arrow. Reviewing a hand-corrected real diagram showed what a
//! person does instead: slide each label along the path to a clear straight stretch and
//! set it just *beside* the line, and put each badge next to the line near its source.
//!
//! This module does that search deterministically: every candidate spot along the path
//! (both sides of the line) is scored against everything already on the canvas — node
//! boxes, group titles and borders, every edge's segments, and the labels and badges
//! placed so far — and the cheapest wins. Both render backends consume the same result,
//! so draw.io and SVG can't disagree about where a label goes, and canvas bounds use it
//! too so a label moved to a path's far end still fits on the canvas.

use std::collections::HashMap;

use petgraph::stable_graph::{EdgeIndex, NodeIndex};

use rdg_graph::CompiledGraph;
use rdg_layout::{DesignTokens, LayoutResult};

use crate::routing::{EdgeRoutingPlan, Side, node_attach_point, resolve_edge_layout};

type Pt = (f64, f64);
type Rect = (f64, f64, f64, f64);


/// Where one edge's label goes.
#[derive(Debug, Clone, PartialEq)]
pub struct LabelSpot {
    /// The label text as it should be drawn — possibly wrapped onto two lines (`\n`)
    /// when that's what it took to find a clear spot.
    pub text: String,
    /// Centre of the label box.
    pub cx: f64,
    pub cy: f64,
    pub w: f64,
    pub h: f64,
    /// The point on the path the label is attached to, and the unit direction of the
    /// segment it sits on — enough for draw.io's relative edge-label geometry.
    pub anchor: Pt,
    pub dir: Pt,
    /// Arc length from the path start to `anchor`, and the path's total length.
    pub along: f64,
    pub total: f64,
    /// The label stands off its line (no spot beside it could be owned), so a thin
    /// leader from `anchor` to the box says whose it is.
    pub leader: bool,
}

impl LabelSpot {
    /// draw.io's relative edge-label geometry `(x, y)`: `x` in `[-1, 1]` is the position
    /// along the path, `y` the signed perpendicular offset in px (mxGraph `getPoint`:
    /// `x += (dy/len)·gy`, `y -= (dx/len)·gy`).
    pub fn drawio_geometry(&self) -> (f64, f64) {
        let gx = if self.total > 0.0 { 2.0 * self.along / self.total - 1.0 } else { 0.0 };
        let (vx, vy) = (self.cx - self.anchor.0, self.cy - self.anchor.1);
        let gy = if self.dir.1.abs() > self.dir.0.abs() { vx / self.dir.1 } else { -vy / self.dir.0 };
        (gx, gy)
    }

    pub fn rect(&self) -> Rect {
        (self.cx - self.w / 2.0, self.cy - self.h / 2.0, self.w, self.h)
    }
}

/// Result of [`place_edge_annotations`].
#[derive(Debug, Clone, Default)]
pub struct EdgeAnnotations {
    pub labels: HashMap<EdgeIndex, LabelSpot>,
    /// Badge centre per numbered edge (its size: [`badge_size`] of the edge's `step`).
    pub badges: HashMap<EdgeIndex, Pt>,
    /// A second, smaller badge before the arrowhead of a numbered edge longer than
    /// [`END_BADGE_FROM`] (its size: [`end_badge_size`]), so the number can be read at
    /// the far end of a long arrow too.
    pub end_badges: HashMap<EdgeIndex, Pt>,
    /// Labels that still break the ownership rule (another arrow less than a lane
    /// further away than the label's own line), one line each, for `RDG_DEBUG_SPACING`.
    pub floating: Vec<String>,
    /// Edges whose label is deliberately not drawn: three or more arrows of one node
    /// with the same text say it once, on the one in the middle of the fan (or on the
    /// bundle's trunk), and a renderer must not fall back to drawing it itself.
    pub folded: std::collections::HashSet<EdgeIndex>,
}

/// Size of an edge label's box (text plus the thin background pad both backends draw).
pub fn edge_label_size(text: &str, tokens: &DesignTokens) -> (f64, f64) {
    let lines: Vec<&str> = text.lines().collect();
    let longest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as f64;
    // Text width at the theme's edge-label size and glyph width, plus the background pad.
    let font = tokens.edge_label_font_size;
    let w = longest * tokens.char_width(font) + 6.0;
    let h = lines.len().max(1) as f64 * tokens.line_height(font) + 2.0;
    (w.max(tokens.px(2.5)), h)
}

/// The label split onto two lines at the space nearest its middle, if it has one and
/// is long enough for wrapping to help.
fn wrapped_in_two(text: &str) -> Option<String> {
    if text.contains('\n') || text.chars().count() < 14 {
        return None;
    }
    let mid = text.chars().count() / 2;
    let (i, _) = text
        .char_indices()
        .filter(|&(_, c)| c == ' ')
        .min_by_key(|&(i, _)| (text[..i].chars().count() as i64 - mid as i64).abs())?;
    Some(format!("{}\n{}", &text[..i], &text[i + 1..]))
}

/// The label split onto three roughly equal lines at spaces, for gaps too narrow even
/// for two lines.
fn wrapped_in_three(text: &str) -> Option<String> {
    if text.contains('\n') || text.chars().count() < 24 {
        return None;
    }
    let target = text.chars().count() as f64 / 3.0;
    let mut lines: Vec<String> = vec![String::new()];
    for word in text.split(' ') {
        let n = lines.len();
        let cur_len = lines[n - 1].chars().count();
        if cur_len > 0 && n < 3 && (cur_len + 1 + word.chars().count()) as f64 > target + 2.0 {
            lines.push(word.to_string());
        } else {
            let cur = &mut lines[n - 1];
            if cur_len > 0 {
                cur.push(' ');
            }
            cur.push_str(word);
        }
    }
    (lines.len() == 3).then(|| lines.join("\n"))
}

/// Size of a flow-step badge: a circle for one or two characters, a pill as wide as
/// the text beyond that (`16a`, `3.1`).
pub fn badge_size(step: &str, tokens: &DesignTokens) -> (f64, f64) {
    let d = 2.0 * tokens.badge_radius;
    let text_w = step.chars().count() as f64 * tokens.char_width(tokens.badge_font_size) + tokens.badge_radius;
    (d.max(text_w.ceil()), d)
}

/// Size of the second badge at the far end of a long arrow: [`badge_size`] scaled by
/// [`END_BADGE_SCALE`].
pub fn end_badge_size(step: &str, tokens: &DesignTokens) -> (f64, f64) {
    let (w, h) = badge_size(step, tokens);
    ((w * END_BADGE_SCALE).ceil(), (h * END_BADGE_SCALE).ceil())
}

/// An arrow at least this long carries a second badge before its arrowhead.
pub const END_BADGE_FROM: f64 = 600.0;
/// The far-end badge's size, relative to the one at the source.
pub const END_BADGE_SCALE: f64 = 0.8;

/// Sort key putting steps in reading order: by number, then branch (`3` < `3a` < `3b` < `4`).
fn step_order(step: &str) -> (u32, String) {
    (rdg_schema::step_major(step).unwrap_or(u32::MAX), step.to_string())
}

/// The polyline an edge is drawn along: source port, waypoints, destination port — the
/// same points both backends draw.
pub fn edge_polyline(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    edge_idx: EdgeIndex,
    plan: Option<&EdgeRoutingPlan>,
    tokens: &DesignTokens,
) -> Option<Vec<Pt>> {
    let (s_idx, d_idx, src_nl, dst_nl) = resolve_edge_layout(compiled, layout, edge_idx)?;
    let src_side = plan.map_or(Side::Bottom, |p| p.src_side);
    let dst_side = plan.map_or(Side::Top, |p| p.dst_side);
    let exit = plan.map_or(0.5, |p| p.exit_port);
    let (x1, y1) = plan.and_then(|p| p.start).unwrap_or_else(|| node_attach_point(compiled, s_idx, src_nl, src_side, exit, tokens));
    let p2 = plan.and_then(|p| p.junction).unwrap_or_else(|| node_attach_point(compiled, d_idx, dst_nl, dst_side, plan.map_or(0.5, |p| p.entry_port), tokens));
    let mut pts = vec![(x1, y1)];
    pts.extend(plan.map_or(&[][..], |p| p.waypoints.as_slice()).iter().copied());
    pts.push(p2);
    pts.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6);
    Some(pts)
}

fn inflate(r: Rect, d: f64) -> Rect {
    (r.0 - d, r.1 - d, r.2 + 2.0 * d, r.3 + 2.0 * d)
}

fn overlap_area(a: Rect, b: Rect) -> f64 {
    let ox = (a.0 + a.2).min(b.0 + b.2) - a.0.max(b.0);
    let oy = (a.1 + a.3).min(b.1 + b.3) - a.1.max(b.1);
    if ox > 0.0 && oy > 0.0 { ox * oy } else { 0.0 }
}

/// Length of segment `a→b` inside rect `r` (Liang–Barsky clip).
fn clip_len(a: Pt, b: Pt, r: Rect) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    for (p, q) in [(-dx, a.0 - r.0), (dx, r.0 + r.2 - a.0), (-dy, a.1 - r.1), (dy, r.1 + r.3 - a.1)] {
        if p.abs() < 1e-12 {
            if q < 0.0 {
                return 0.0;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
            if t0 > t1 {
                return 0.0;
            }
        }
    }
    (t1 - t0) * (dx * dx + dy * dy).sqrt()
}

/// Everything a label or badge must stay clear of.
struct Scene {
    nodes: Vec<Rect>,
    titles: Vec<Rect>,
    borders: Vec<(Pt, Pt)>,
    /// Group boxes, each with the bit of its group (see [`group_bit`]).
    groups: Vec<(Rect, u64)>,
    paths: Vec<(EdgeIndex, Vec<Pt>)>,
    /// Badges and labels placed so far; the flag marks labels.
    placed: Vec<(EdgeIndex, bool, Rect)>,
    /// One lane: the room every other arrow keeps from a label's own line.
    lane: f64,
}

// Penalty weights. Anything that makes text unreadable or ambiguous (sitting on a box,
// a line or other text) costs per px² / px of intrusion and dwarfs the soft
// preferences, which only break ties between clean spots.
const W_AREA: f64 = 60.0;
const W_LINE: f64 = 80.0;
const W_BORDER: f64 = 30.0;
/// Per px of another arrow running inside the ownership band, at full strength when
/// it touches the box and fading to nothing at the band's edge.
const W_NEAR_LINE: f64 = 8.0;
/// A label on a leg outside the span of its own two nodes (a loop round the drawing):
/// nothing there tells the reader whose arrow it is.
const W_PERIMETER: f64 = 600.0;
/// A label inside a container neither of its nodes belongs to reads as that
/// container's.
const W_FOREIGN_BOX: f64 = 300.0;
/// A branch label (an edge leaving a decision) further than this from the decision
/// costs [`W_BRANCH_AWAY`] plus a steep drift.
const BRANCH_REACH: f64 = 40.0;
const W_BRANCH_AWAY: f64 = 200.0;
/// A label's own nodes' box grows by this much on every side before a leg counts
/// as outside it (room for the stubs and one lane of detour).
const SPAN_PAD: f64 = 64.0;
/// A label keeps this far from a bend of its own line.
const BEND_CLEAR: f64 = 10.0;
/// A label this far or further from its line gets a leader line to it.
const LEADER_FROM: f64 = 16.0;
/// A badge nearer another arrow's line than its own, by less than this margin, reads
/// as that arrow's number.
const BADGE_OWN_MARGIN: f64 = 6.0;
const W_BADGE_OWN: f64 = 150.0;
/// Half-side of the zone round another arrow's head or tail a badge keeps out of: a
/// number beside an arrowhead is read as that arrow's.
const END_ZONE: f64 = 10.0;
/// How far down its arrow a badge may go when the source is crowded (it costs 0.6/px).
const BADGE_REACH: f64 = 600.0;
/// A badge on its own line (the plate hides the line under it) costs this: the spot
/// of last resort when both sides of the arrow are taken for its whole reach.
const BADGE_ON_LINE: f64 = 45.0;
/// Air a badge keeps from every node box (beyond the 2 px of any annotation).
const BADGE_NODE_AIR: f64 = 8.0;
/// Badges of arrows leaving one face of one node line up: per px their distances
/// from the face differ, and per side they disagree on.
const W_BADGE_FAN: f64 = 0.5;
const W_BADGE_FAN_SIDE: f64 = 6.0;

/// Bit of group `g` in a membership mask (groups past 63 share the last bit).
fn group_bit(g: usize) -> u64 {
    1u64 << g.min(63)
}

/// Arc length of the prefix two paths share (the run before they part), measured along
/// `a`; 0 when only their first point is common.
pub fn shared_prefix_len(a: &[Pt], b: &[Pt]) -> f64 {
    let same = |p: Pt, q: Pt| (p.0 - q.0).abs() < 0.5 && (p.1 - q.1).abs() < 0.5;
    let common = a.iter().zip(b).take_while(|(p, q)| same(**p, **q)).count();
    if common < 2 {
        return 0.0;
    }
    a[..common].windows(2).map(|w| ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt()).sum()
}

/// Distance from segment `a→b` to rect `r` (0 when they touch).
fn seg_rect_dist(a: Pt, b: Pt, r: Rect) -> f64 {
    let gap = |lo: f64, hi: f64, p0: f64, p1: f64| (lo - p1).max(p0 - hi).max(0.0);
    if (a.1 - b.1).abs() < 0.5 || (a.0 - b.0).abs() < 0.5 {
        // Axis-aligned: the segment is a degenerate box, and box-to-box distance is exact.
        let dx = gap(r.0, r.0 + r.2, a.0.min(b.0), a.0.max(b.0));
        let dy = gap(r.1, r.1 + r.3, a.1.min(b.1), a.1.max(b.1));
        return (dx * dx + dy * dy).sqrt();
    }
    (0..=8)
        .map(|k| {
            let t = k as f64 / 8.0;
            let p = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            let dx = gap(r.0, r.0 + r.2, p.0, p.0);
            let dy = gap(r.1, r.1 + r.3, p.1, p.1);
            (dx * dx + dy * dy).sqrt()
        })
        .fold(f64::MAX, f64::min)
}

impl Scene {
    /// The ownership band: with the label `gap` px off its own line, every other line
    /// must stay a full lane further away than that, or the label reads as the other
    /// line's (ELK keeps 2 px to the own edge against 10 px to any other).
    fn band(&self, gap: f64) -> f64 {
        // A label on a leader is tied to its line by the leader itself: other lines
        // need only keep a lane off the box.
        if gap >= LEADER_FROM { self.lane } else { gap.max(0.0) + self.lane }
    }

    /// `on_own_line`: the box deliberately sits on its own line (its background masks
    /// the stroke), so only other lines count against it.
    ///
    /// `gap`: distance from the box to its own line (see [`Scene::band`]).
    fn cost(&self, r: Rect, own: EdgeIndex, on_own_line: bool, gap: f64) -> f64 {
        let mut c = 0.0;
        let padded = inflate(r, 2.0);
        for &n in &self.nodes {
            c += W_AREA * overlap_area(padded, n);
        }
        for &t in &self.titles {
            c += W_AREA * overlap_area(padded, t);
        }
        for &(_, _, p) in &self.placed {
            c += W_AREA * overlap_area(inflate(r, 3.0), p);
        }
        let band = self.band(gap);
        let near = inflate(r, band);
        for (e, path) in &self.paths {
            if on_own_line && *e == own {
                continue;
            }
            if *e == own {
                // Own line only needs to stay out of the box itself.
                let hard = inflate(r, 1.0);
                for w in path.windows(2) {
                    c += W_LINE * clip_len(w[0], w[1], hard);
                }
                continue;
            }
            // A foreign line must not touch the box, and inside the ownership band it
            // costs by how far in it comes: of the two sides of a parallel pair, the
            // outer one (further from the other line) is the cheaper.
            let hard = inflate(r, 3.0);
            for w in path.windows(2) {
                c += W_LINE * clip_len(w[0], w[1], hard);
                let inside = clip_len(w[0], w[1], near);
                if inside > 0.0 {
                    let d = seg_rect_dist(w[0], w[1], r);
                    c += W_NEAR_LINE * inside * (1.0 - (d / band).min(1.0));
                }
            }
        }
        for &(a, b) in &self.borders {
            c += W_BORDER * clip_len(a, b, padded);
        }
        if std::env::var_os("RDG_DEBUG_COST").is_some() && c > 0.0 {
            let nodes: f64 = self.nodes.iter().map(|&n| W_AREA * overlap_area(padded, n)).sum();
            let titles: f64 = self.titles.iter().map(|&t| W_AREA * overlap_area(padded, t)).sum();
            let placed: f64 = self.placed.iter().map(|&(_, _, p)| W_AREA * overlap_area(inflate(r, 3.0), p)).sum();
            let borders: f64 = self.borders.iter().map(|&(a, b)| W_BORDER * clip_len(a, b, padded)).sum();
            let own: f64 = self.paths.iter().filter(|(e, _)| *e == own && !on_own_line).map(|(_, p)| p.windows(2).map(|w| W_LINE * clip_len(w[0], w[1], inflate(r, 1.0))).sum::<f64>()).sum();
            eprintln!("cost r=({:.0},{:.0},{:.0},{:.0}) total={c:.0} nodes={nodes:.0} titles={titles:.0} placed={placed:.0} borders={borders:.0} own={own:.0} lines={:.0}", r.0, r.1, r.2, r.3, c - nodes - titles - placed - borders - own);
        }
        c
    }

    /// Nearest foreign line to `r`, and the distance to the edge's own line.
    fn ownership(&self, r: Rect, own: EdgeIndex) -> (f64, f64) {
        let (mut foreign, mut own_d) = (f64::MAX, f64::MAX);
        for (e, path) in &self.paths {
            for w in path.windows(2) {
                let d = seg_rect_dist(w[0], w[1], r);
                if *e == own {
                    own_d = own_d.min(d);
                } else {
                    foreign = foreign.min(d);
                }
            }
        }
        (foreign, own_d)
    }
}

/// Point and unit direction at arc length `s` along `path`, plus the segment index.
fn point_at(path: &[Pt], s: f64) -> (Pt, Pt, usize) {
    let mut rem = s;
    for (i, w) in path.windows(2).enumerate() {
        let len = ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt();
        if len <= 0.0 {
            continue;
        }
        let dir = ((w[1].0 - w[0].0) / len, (w[1].1 - w[0].1) / len);
        if rem <= len || i + 2 == path.len() {
            let t = rem.clamp(0.0, len);
            return ((w[0].0 + dir.0 * t, w[0].1 + dir.1 * t), dir, i);
        }
        rem -= len;
    }
    (path[0], (0.0, 1.0), 0)
}

fn path_len(path: &[Pt]) -> f64 {
    path.windows(2).map(|w| ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt()).sum()
}

/// Segment boundaries (arc lengths) of `path`.
fn segment_spans(path: &[Pt]) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    let mut s = 0.0;
    for w in path.windows(2) {
        let len = ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt();
        out.push((s, s + len));
        s += len;
    }
    out
}

/// Places every edge's label and flow-number badge. Deterministic: badges first (they
/// hug their source, so they get first pick of the space there), in step order; then
/// labels shortest-path-first (fewest options), followed by one refinement sweep where
/// each label is re-placed knowing where every other one ended up.
pub fn place_edge_annotations(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    edge_plans: &HashMap<EdgeIndex, EdgeRoutingPlan>,
    tokens: &DesignTokens,
) -> EdgeAnnotations {
    let mut out = EdgeAnnotations::default();
    if layout.sequence_info.is_some() {
        return out;
    }

    let mut paths = Vec::new();
    for edge_idx in compiled.graph.edge_indices() {
        if let Some(p) = edge_polyline(compiled, layout, edge_idx, edge_plans.get(&edge_idx), tokens) {
            if p.len() >= 2 {
                paths.push((edge_idx, p));
            }
        }
    }

    let mut nodes: Vec<Rect> = Vec::new();
    let mut node_ids: Vec<_> = layout.positions.keys().copied().collect();
    node_ids.sort();
    for n in node_ids {
        let nl = &layout.positions[&n];
        nodes.push((nl.x, nl.y, nl.width, nl.height));
    }
    let titles = crate::routing::compute_group_title_zones(compiled, layout, tokens)
        .iter()
        .map(|t| (t.min_x, t.min_y, t.max_x - t.min_x, t.max_y - t.min_y))
        .collect();
    let mut borders = Vec::new();
    let mut groups = Vec::new();
    for (g, (x, y, w, h)) in rdg_layout::groups::group_rects(compiled, layout, tokens).into_iter().enumerate().filter_map(|(g, r)| r.map(|r| (g, r))) {
        let (a, b, c, d) = ((x, y), (x + w, y), (x + w, y + h), (x, y + h));
        borders.extend([(a, b), (b, c), (c, d), (d, a)]);
        groups.push(((x, y, w, h), group_bit(g)));
    }
    // Every group a node is in, outermost included, as a bit mask.
    let mut node_groups: HashMap<NodeIndex, u64> = HashMap::new();
    for (g, members) in compiled.group_nodes.iter().enumerate() {
        let mask = std::iter::successors(Some(g), |&p| compiled.group_tree.parent[p]).fold(0u64, |m, a| m | group_bit(a));
        for &n in members {
            *node_groups.entry(n).or_default() |= mask;
        }
    }
    let sources: HashMap<EdgeIndex, NodeIndex> = paths.iter().filter_map(|(e, _)| resolve_edge_layout(compiled, layout, *e).map(|(s, ..)| (*e, s))).collect();
    let mut scene = Scene { nodes, titles, borders, groups, paths, placed: Vec::new(), lane: tokens.channel_pitch().max(10.0) };

    // --- Badges ---------------------------------------------------------------
    // A badge sits beside its arrow's first stretch, as near the source as the scene
    // allows, on the right hand of travel (the label takes the other side); never on a
    // node, a title, a line, a label or another badge; nearer its own line than any
    // other and clear of every other arrow's head and tail, where it would read as that
    // arrow's number; badges of arrows leaving one face of one node line up; a long
    // arrow gets a second, smaller badge before its arrowhead.
    let src_faces: HashMap<EdgeIndex, (NodeIndex, Side)> = scene
        .paths
        .iter()
        .filter_map(|(e, _)| Some((*e, (*sources.get(e)?, edge_plans.get(e).map_or(Side::Bottom, |p| p.src_side)))))
        .collect();
    let mut fan_badges: Vec<((NodeIndex, Side), f64, f64)> = Vec::new();
    // A set's members share one step: the badge goes on the trunk only.
    let on_trunk = |e: EdgeIndex| compiled.graph[e].set.is_none() || edge_plans.get(&e).is_none_or(|p| p.junction.is_none() && p.start.is_none());
    let mut numbered: Vec<((u32, String), usize)> = scene
        .paths
        .iter()
        .enumerate()
        .filter(|(_, (e, _))| on_trunk(*e))
        .filter_map(|(i, (e, _))| compiled.graph[*e].step.as_deref().map(|s| (step_order(s), i)))
        .collect();
    numbered.sort();
    let debug = std::env::var_os("RDG_DEBUG_BADGES").is_some();
    for ((_, step), i) in numbered {
        let (edge_idx, path) = scene.paths[i].clone();
        let total = path_len(&path);
        let face = src_faces.get(&edge_idx).copied();
        // What a badge costs at `r`, `d` from the face on side `sign`, beyond the scene.
        let badge_cost = |scene: &Scene, fan_badges: &[((NodeIndex, Side), f64, f64)], r: Rect, d: f64, sign: f64, at_start: bool| -> f64 {
            let mut c = scene.cost(r, edge_idx, false, 0.0);
            let (foreign, own_d) = scene.ownership(r, edge_idx);
            if foreign < own_d + BADGE_OWN_MARGIN {
                c += W_BADGE_OWN;
            }
            let zone = |p: Pt| (p.0 - END_ZONE, p.1 - END_ZONE, 2.0 * END_ZONE, 2.0 * END_ZONE);
            for (e, other) in &scene.paths {
                if *e == edge_idx || other.len() < 2 {
                    continue;
                }
                c += W_AREA * (overlap_area(inflate(r, 3.0), zone(other[0])) + overlap_area(inflate(r, 3.0), zone(other[other.len() - 1])));
            }
            for &n in &scene.nodes {
                c += W_AREA * overlap_area(inflate(r, BADGE_NODE_AIR), n);
            }
            if at_start {
                if let Some(f) = face {
                    for &(of, od, os) in fan_badges {
                        if of == f {
                            c += W_BADGE_FAN * (d - od).abs() + if os != sign { W_BADGE_FAN_SIDE } else { 0.0 };
                        }
                    }
                }
            }
            c
        };
        // The badge at the source: anywhere along a short arrow (past the arrowhead),
        // the first half of a long one, up to [`BADGE_REACH`] — the drift cost keeps it
        // near the source unless the source is crowded.
        let (bw, bh) = badge_size(&step, tokens);
        let badge = |c: Pt| (c.0 - bw / 2.0, c.1 - bh / 2.0, bw, bh);
        let d_max = if total < 140.0 { (total - 16.0).max(16.0) } else { (total * 0.5).clamp(16.0, BADGE_REACH) };
        let mut best: Option<(f64, Pt, f64, f64)> = None;
        let mut d = 16.0;
        while d <= d_max + 1e-9 {
            let (p, dir, _) = point_at(&path, d.min(total));
            // Beside the line: half the badge's extent across it, plus a little air.
            let off = if dir.0.abs() >= dir.1.abs() { bh / 2.0 } else { bw / 2.0 } + 5.0;
            // Right-hand side of travel first (screen coords, y down) — a stable tie-break.
            // A second spot a little further out lets the badge step off a line that
            // crosses its arrow right there.
            for (k, sign) in [(0.0, 1.0), (3.0, -1.0)] {
                for extra in [0.0, 12.0] {
                    let o = off + extra;
                    let c = (p.0 - dir.1 * o * sign, p.1 + dir.0 * o * sign);
                    let cost = badge_cost(&scene, &fan_badges, badge(c), d, sign, true) + d * 0.6 + k + extra * 1.5;
                    if debug {
                        eprintln!("badge {step}: d={d:.0} sign={sign} extra={extra} at=({:.0},{:.0}) cost={cost:.0} scene={:.0}", c.0, c.1, scene.cost(badge(c), edge_idx, false, 0.0));
                    }
                    if best.is_none_or(|(b, ..)| cost < b - 1e-9) {
                        best = Some((cost, c, d, sign));
                    }
                }
            }
            // On the line itself, when both sides are taken: on a straight stretch only,
            // never over a bend.
            let (_, _, seg) = point_at(&path, d.min(total));
            let half = bw.max(bh) / 2.0 + BEND_CLEAR;
            let straight = point_at(&path, (d - half).max(0.0)).2 == seg && point_at(&path, (d + half).min(total)).2 == seg;
            if straight {
                let on = scene.cost(badge(p), edge_idx, true, 0.0) + BADGE_ON_LINE + d * 0.6;
                if debug {
                    eprintln!("badge {step}: d={d:.0} on-line at=({:.0},{:.0}) cost={on:.0}", p.0, p.1);
                }
                if best.is_none_or(|(b, ..)| on < b - 1e-9) {
                    best = Some((on, p, d, 0.0));
                }
            }
            d += 6.0;
        }
        if let Some((_, c, d, sign)) = best {
            scene.placed.push((edge_idx, false, badge(c)));
            out.badges.insert(edge_idx, c);
            if let Some(f) = face {
                fan_badges.push((f, d, sign));
            }
        }
        // The far-end badge of a long arrow: before the arrowhead, same side rule.
        if total >= END_BADGE_FROM {
            let (ew, eh) = end_badge_size(&step, tokens);
            let ebadge = |c: Pt| (c.0 - ew / 2.0, c.1 - eh / 2.0, ew, eh);
            let mut best: Option<(f64, Pt)> = None;
            let mut d = 16.0;
            while d <= 96.0 + 1e-9 {
                let (p, dir, _) = point_at(&path, total - d);
                let off = if dir.0.abs() >= dir.1.abs() { eh / 2.0 } else { ew / 2.0 } + 5.0;
                for (k, sign) in [(0.0, 1.0), (3.0, -1.0)] {
                    for extra in [0.0, 12.0] {
                        let o = off + extra;
                        let c = (p.0 - dir.1 * o * sign, p.1 + dir.0 * o * sign);
                        let cost = badge_cost(&scene, &fan_badges, ebadge(c), d, sign, false) + d * 0.6 + k + extra * 1.5;
                        if best.is_none_or(|(b, _)| cost < b - 1e-9) {
                            best = Some((cost, c));
                        }
                    }
                }
                d += 6.0;
            }
            if let Some((_, c)) = best {
                scene.placed.push((edge_idx, false, ebadge(c)));
                out.end_badges.insert(edge_idx, c);
            }
        }
    }

    // --- Labels ---------------------------------------------------------------
    // Repeated labels: the same text on three or more arrows leaving (or entering)
    // one node is said once, on the arrow in the middle of the fan — or, in a bundle,
    // on the trunk (the one that still reaches its target).
    let lr = layout.layered == Some(rdg_layout::LayoutDirection::LeftToRight);
    // An arrow written with several nodes at one end says its label once, on the
    // trunk (the member that still reaches the other end), whatever the set's size;
    // its members take no part in the fan fold below.
    for k in 0..compiled.sets.len() {
        let members: Vec<usize> = scene.paths.iter().enumerate().filter(|(_, (e, _))| compiled.graph[*e].set == Some(k)).map(|(i, _)| i).collect();
        let keep = members.iter().copied().find(|&i| edge_plans.get(&scene.paths[i].0).is_none_or(|p| p.junction.is_none() && p.start.is_none())).or(members.first().copied());
        for &i in &members {
            if Some(i) != keep {
                out.folded.insert(scene.paths[i].0);
            }
        }
    }
    let mut fans: HashMap<(petgraph::stable_graph::NodeIndex, bool, String), Vec<usize>> = HashMap::new();
    for (i, (e, _)) in scene.paths.iter().enumerate() {
        if compiled.graph[*e].set.is_some() {
            continue;
        }
        let Some(text) = compiled.graph[*e].label.as_deref().map(str::trim).filter(|l| !l.is_empty()) else { continue };
        let Some((s, t, ..)) = resolve_edge_layout(compiled, layout, *e) else { continue };
        fans.entry((s, true, text.to_string())).or_default().push(i);
        fans.entry((t, false, text.to_string())).or_default().push(i);
    }
    let mut fan_keys: Vec<_> = fans.keys().cloned().collect();
    fan_keys.sort_by(|a, b| a.0.index().cmp(&b.0.index()).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    for key in fan_keys {
        let members = &fans[&key];
        if members.len() < 3 {
            continue;
        }
        let far = |i: usize| -> Option<Pt> {
            let (s, t, snl, dnl) = resolve_edge_layout(compiled, layout, scene.paths[i].0)?;
            let nl = if key.1 { (t, dnl) } else { (s, snl) }.1;
            Some((nl.x + nl.width / 2.0, nl.y + nl.height / 2.0))
        };
        let reaching: Vec<usize> = members.iter().copied().filter(|&i| edge_plans.get(&scene.paths[i].0).is_none_or(|p| p.junction.is_none() && p.start.is_none())).collect();
        let pool = if reaching.is_empty() { members.clone() } else { reaching };
        let mut ordered: Vec<(f64, usize)> = pool.iter().filter_map(|&i| far(i).map(|p| (if lr { p.1 } else { p.0 }, i))).collect();
        ordered.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let Some(&(_, keep)) = ordered.get(ordered.len() / 2) else { continue };
        for &i in members {
            if i != keep {
                out.folded.insert(scene.paths[i].0);
            }
        }
    }
    let mut order: Vec<(f64, usize)> = scene
        .paths
        .iter()
        .enumerate()
        .filter(|(_, (e, _))| !out.folded.contains(e) && compiled.graph[*e].label.as_deref().is_some_and(|l| !l.trim().is_empty()))
        .map(|(i, (_, p))| (path_len(p), i))
        .collect();
    order.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));

    let place = |scene: &Scene, i: usize| -> Option<LabelSpot> {
        let (edge_idx, path) = &scene.paths[i];
        let data = &compiled.graph[*edge_idx];
        let text = data.label.as_deref()?.trim();
        let total = path_len(path);
        let (s_idx, d_idx, src_nl, dst_nl) = resolve_edge_layout(compiled, layout, *edge_idx)?;
        // A branch label ("yes", "declined") belongs next to the decision it leaves,
        // read together with the question: within reach of the corner, or it pays.
        let from_decision = {
            let n = &compiled.graph[s_idx];
            n.shape.as_deref() == Some("diamond") || matches!(n.node_type.as_str(), "decision" | "choice")
        };
        // The span of the edge's own nodes: a leg outside it is a detour round the
        // drawing, where nothing tells the reader whose arrow the label belongs to.
        let span = {
            let x0 = src_nl.x.min(dst_nl.x) - SPAN_PAD;
            let y0 = src_nl.y.min(dst_nl.y) - SPAN_PAD;
            let x1 = (src_nl.x + src_nl.width).max(dst_nl.x + dst_nl.width) + SPAN_PAD;
            let y1 = (src_nl.y + src_nl.height).max(dst_nl.y + dst_nl.height) + SPAN_PAD;
            (x0, y0, x1, y1)
        };
        let own_groups = node_groups.get(&s_idx).copied().unwrap_or(0) | node_groups.get(&d_idx).copied().unwrap_or(0);
        // Branches of one decision may leave by one corner and part a little further on;
        // the reach is measured from where this branch has a line of its own.
        let branch_from = scene
            .paths
            .iter()
            .filter(|(o, _)| *o != *edge_idx && sources.get(o) == Some(&s_idx))
            .map(|(_, other)| shared_prefix_len(path, other))
            .fold(0.0, f64::max);
        // Keep clear of the arrowhead and the badge/stub at the start.
        let end_clear = (total * 0.2).min(14.0);
        // Text variants, each with its own penalty: as written, then wrapped onto two
        // lines — narrower boxes fit gaps a single long line can't.
        // A label longer than the theme's `wrap_chars` prefers two lines — one long line
        // eats the horizontal room arrows need.
        let long = !text.contains('\n') && text.chars().count() > tokens.edge_label_wrap_chars;
        let mut variants = vec![(text.to_string(), if long { 60.0 } else { 0.0 })];
        if let Some(two) = wrapped_in_two(text) {
            variants.push((two, if long { 0.0 } else { 60.0 }));
        }
        if let Some(three) = wrapped_in_three(text) {
            variants.push((three, 110.0));
        }
        // Distance between the line and the near edge of the box: snug beside the
        // line, else centred on the line itself with the background masking the
        // stroke. Further out only with a leader line (see `LabelSpot::leader`), and
        // at a price: a label a lane away reads as orphaned without one.
        const ON_LINE: f64 = -1.0;
        let gaps = [(4.0, 0.0), (ON_LINE, 45.0), (LEADER_FROM, 90.0), (LEADER_FROM + 12.0, 110.0), (LEADER_FROM + 24.0, 130.0)];
        let mut best: Option<(f64, LabelSpot)> = None;
        let spans = segment_spans(path);
        let longest = spans.iter().map(|(a, b)| b - a).fold(0.0, f64::max);
        for &(s0, s1) in &spans {
            let len = s1 - s0;
            if len < 1.0 {
                continue;
            }
            // Clear of the bends: a label at a corner reads as belonging to either leg
            // (and mxGraph would attach it to the next segment, swinging the offset).
            let lo = s0.max(end_clear) + BEND_CLEAR.min(len / 2.0);
            let hi = s1.min(total - end_clear) - BEND_CLEAR.min(len / 2.0);
            if lo > hi {
                continue;
            }
            let seg_mid = (s0 + s1) / 2.0;
            let n = ((hi - lo) / 10.0).ceil().max(1.0) as usize;
            let mut samples: Vec<f64> = (0..=n).map(|k| lo + (hi - lo) * k as f64 / n as f64).collect();
            samples.push(seg_mid.clamp(lo, hi));
            for s in samples {
                let (p, dir, _) = point_at(path, s);
                let horizontal = dir.0.abs() >= dir.1.abs();
                let perimeter = p.0 < span.0 || p.0 > span.2 || p.1 < span.1 || p.1 > span.3;
                let reach = branch_from + BRANCH_REACH;
                let branch_pen = if from_decision && s > reach { W_BRANCH_AWAY + 2.0 * (s - reach) } else { 0.0 };
                for (variant, wrap_pen) in &variants {
                    let (w, h) = edge_label_size(variant, tokens);
                    for &(gap, gap_pen) in &gaps {
                        // Perpendicular distance from the line to the label centre.
                        let on_line = gap == ON_LINE;
                        let off = if on_line { 0.0 } else if horizontal { h / 2.0 + gap } else { w / 2.0 + gap };
                        for (k, sign) in [(0.0, 1.0), (2.0, -1.0)] {
                            if on_line && sign < 0.0 {
                                continue;
                            }
                            // Above a horizontal leg / right of a vertical leg first.
                            let (cx, cy) = if horizontal { (p.0, p.1 - off * sign) } else { (p.0 + off * sign, p.1) };
                            let spot = LabelSpot { text: variant.clone(), cx, cy, w, h, anchor: p, dir, along: s, total, leader: gap >= LEADER_FROM };
                            // Where a reader looks for the label: near the source, where
                            // they start following the arrow (a branch label right at
                            // the decision); on a long straight stretch of it; inside
                            // the span of its own two nodes and its own containers.
                            let foreign_box = scene.groups.iter().any(|&((gx, gy, gw, gh), bit)| {
                                own_groups & bit == 0 && cx > gx && cx < gx + gw && cy > gy && cy < gy + gh
                            });
                            let soft = if horizontal { 0.0 } else { 30.0 }
                                + 0.1 * (s - seg_mid).abs()
                                + 0.1 * (longest - len)
                                + 0.15 * s
                                + branch_pen
                                + if perimeter { W_PERIMETER } else { 0.0 }
                                + if foreign_box { W_FOREIGN_BOX } else { 0.0 }
                                + wrap_pen
                                + gap_pen
                                + k;
                            if best.as_ref().is_some_and(|(b, _)| soft >= *b) {
                                continue;
                            }
                            let cost = scene.cost(spot.rect(), *edge_idx, on_line, gap.max(0.0)) + soft;
                            if best.as_ref().is_none_or(|(b, _)| cost < b - 1e-9) {
                                best = Some((cost, spot));
                            }
                        }
                    }
                }
            }
        }
        if std::env::var_os("RDG_DEBUG_ANNOTATE").is_some() {
            if let Some((c, s)) = &best {
                eprintln!("label {:?} cost={c:.1} rect={:?} path={:?}", text, s.rect(), path);
            }
        }
        best.map(|(_, s)| s)
    };

    for &(_, i) in &order {
        if let Some(spot) = place(&scene, i) {
            let e = scene.paths[i].0;
            scene.placed.push((e, true, spot.rect()));
            out.labels.insert(e, spot);
        }
    }
    // Refinement: each label re-placed against the final positions of all the others.
    for &(_, i) in &order {
        let e = scene.paths[i].0;
        let Some(pos) = scene.placed.iter().position(|&(pe, is_label, _)| pe == e && is_label) else {
            continue;
        };
        let removed = scene.placed.remove(pos);
        match place(&scene, i) {
            Some(spot) => {
                scene.placed.push((e, true, spot.rect()));
                out.labels.insert(e, spot);
            }
            None => scene.placed.push(removed),
        }
    }
    // Report what still floats: a label whose nearest foreign line is inside its
    // ownership band (rule A1). Deterministic order: by edge index.
    let mut labelled: Vec<(&EdgeIndex, &LabelSpot)> = out.labels.iter().collect();
    labelled.sort_by_key(|(e, _)| e.index());
    for (e, spot) in labelled {
        if spot.leader {
            continue;
        }
        let (foreign, own_d) = scene.ownership(spot.rect(), *e);
        let band = scene.band(own_d);
        if foreign < band {
            let name = compiled.graph.edge_endpoints(*e).map_or(String::from("?"), |(s, t)| format!("{} -> {}", compiled.graph[s].id, compiled.graph[t].id));
            out.floating.push(format!("{:?} on {name}: another arrow {foreign:.0}px away (own {own_d:.0}px, needs {band:.0}px)", spot.text));
        }
    }
    if std::env::var_os("RDG_DEBUG_ANNOTATE").is_some() {
        for f in &out.floating {
            eprintln!("floating label {f}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_len_measures_inside_portion() {
        let r = (0.0, 0.0, 10.0, 10.0);
        assert!((clip_len((-5.0, 5.0), (15.0, 5.0), r) - 10.0).abs() < 1e-9);
        assert_eq!(clip_len((-5.0, 20.0), (15.0, 20.0), r), 0.0);
        assert!((clip_len((5.0, 5.0), (5.0, 50.0), r) - 5.0).abs() < 1e-9);
    }

    #[test]
    fn seg_rect_dist_is_zero_on_touch_and_exact_off_axis_aligned_legs() {
        let r = (10.0, 10.0, 20.0, 10.0);
        assert_eq!(seg_rect_dist((0.0, 15.0), (40.0, 15.0), r), 0.0);
        // Horizontal leg 6px under the box, overlapping it in x.
        assert!((seg_rect_dist((0.0, 26.0), (40.0, 26.0), r) - 6.0).abs() < 1e-9);
        // Vertical leg 5px right of the box.
        assert!((seg_rect_dist((35.0, 0.0), (35.0, 40.0), r) - 5.0).abs() < 1e-9);
        // Diagonal leg ending 3px above the box's top-left corner.
        assert!((seg_rect_dist((0.0, 0.0), (10.0, 7.0), r) - 3.0).abs() < 1e-6);
    }

    #[test]
    fn a_label_between_two_parallel_arrows_moves_to_the_outer_side() {
        // Two horizontal arrows one lane (18px) apart; the lower one is labelled. The
        // label must sit under it (away from the upper arrow), not between the two.
        let e0 = EdgeIndex::new(0);
        let e1 = EdgeIndex::new(1);
        let scene = Scene {
            nodes: vec![],
            titles: vec![],
            borders: vec![],
            groups: vec![],
            paths: vec![(e0, vec![(0.0, 100.0), (300.0, 100.0)]), (e1, vec![(0.0, 118.0), (300.0, 118.0)])],
            placed: vec![],
            lane: 18.0,
        };
        let (w, h) = (40.0, 12.0);
        let above = (130.0, 118.0 - 4.0 - h, w, h);
        let below = (130.0, 118.0 + 4.0, w, h);
        assert!(scene.cost(below, e1, false, 4.0) < scene.cost(above, e1, false, 4.0));
        // And the upper arrow's label goes above it.
        let up_above = (130.0, 100.0 - 4.0 - h, w, h);
        let up_below = (130.0, 100.0 + 4.0, w, h);
        assert!(scene.cost(up_above, e0, false, 4.0) < scene.cost(up_below, e0, false, 4.0));
    }

    #[test]
    fn drawio_geometry_round_trips_perpendicular_offset() {
        // Horizontal rightward leg, label 12px above: mxGraph moves up for positive gy.
        let s = LabelSpot { text: String::new(), cx: 50.0, cy: -12.0, w: 20.0, h: 10.0, anchor: (50.0, 0.0), dir: (1.0, 0.0), along: 50.0, total: 100.0, leader: false };
        assert_eq!(s.drawio_geometry(), (0.0, 12.0));
        // Downward leg, label 20px to the right: positive gy moves right.
        let s = LabelSpot { text: String::new(), cx: 20.0, cy: 75.0, w: 20.0, h: 10.0, anchor: (0.0, 75.0), dir: (0.0, 1.0), along: 75.0, total: 100.0, leader: false };
        assert_eq!(s.drawio_geometry(), (0.5, 20.0));
    }

    fn compile() -> (CompiledGraph, LayoutResult, HashMap<EdgeIndex, EdgeRoutingPlan>, DesignTokens) {
        use rdg_schema::{DiagramPayload, EdgeDef, NodeDef};
        let node = |id: &str| NodeDef { id: id.into(), label: id.to_uppercase(), ..Default::default() };
        let edge = |to: &str, label: &str| EdgeDef {
            from: "a".into(),
            to: to.into(),
            label: Some(label.into()),
            ..Default::default()
        };
        let compiled = rdg_graph::build_graph(&DiagramPayload {
            numbered: Some(true),
            nodes: vec![node("a"), node("b"), node("c")],
            edges: vec![edge("b", "HTTPS request"), edge("c", "SQL")],
            ..Default::default()
        })
        .unwrap();
        let cfg = rdg_layout::LayoutConfig::default();
        let layout = rdg_layout::compute_layout(&compiled, &cfg).unwrap();
        let plans = crate::routing::plan_all_edge_routes(
            &compiled,
            &layout,
            crate::routing::RoutingAlgorithm::CornerHeuristic,
            &cfg.tokens,
        );
        (compiled, layout, plans, cfg.tokens)
    }

    #[test]
    fn a_branch_label_stays_within_reach_of_its_decision() {
        use rdg_schema::{DiagramPayload, EdgeDef, NodeDef};
        let node = |id: &str, ty: &str| NodeDef { id: id.into(), label: id.to_uppercase(), node_type: ty.into(), ..Default::default() };
        let edge = |from: &str, to: &str, label: &str| EdgeDef { from: from.into(), to: to.into(), label: Some(label.into()), ..Default::default() };
        let compiled = rdg_graph::build_graph(&DiagramPayload {
            diagram_type: "flowchart".into(),
            nodes: vec![node("q", "decision"), node("a", "function"), node("b", "function"), node("c", "function")],
            edges: vec![edge("q", "a", "yes"), edge("q", "b", "no"), edge("b", "c", "then")],
            ..Default::default()
        })
        .unwrap();
        let cfg = rdg_layout::LayoutConfig::default();
        let layout = rdg_layout::compute_layout(&compiled, &cfg).unwrap();
        let plans = crate::routing::plan_all_edge_routes(&compiled, &layout, crate::routing::RoutingAlgorithm::CornerHeuristic, &cfg.tokens);
        let ann = place_edge_annotations(&compiled, &layout, &plans, &cfg.tokens);
        let path_of = |e| edge_polyline(&compiled, &layout, e, plans.get(&e), &cfg.tokens).unwrap();
        for e in compiled.graph.edge_indices() {
            let data = &compiled.graph[e];
            let spot = &ann.labels[&e];
            if data.label.as_deref() != Some("then") {
                // The two branches may share a stub out of the diamond's corner.
                let shared = compiled.graph.edge_indices().filter(|&o| o != e && data.label.as_deref() != compiled.graph[o].label.as_deref() && compiled.graph[o].label.as_deref() != Some("then")).map(|o| shared_prefix_len(&path_of(e), &path_of(o))).fold(0.0, f64::max);
                assert!(spot.along <= shared + BRANCH_REACH + 1e-6, "{:?} sits {:.0}px down its arrow (shared stub {shared:.0}px)", data.label, spot.along);
            }
            assert!(spot.along >= BEND_CLEAR - 1e-6);
        }
        assert!(ann.floating.is_empty(), "{:?}", ann.floating);
    }

    #[test]
    fn a_badge_keeps_its_air_from_every_node_and_a_short_arrow_has_no_end_badge() {
        let (c, l, p, t) = compile();
        let ann = place_edge_annotations(&c, &l, &p, &t);
        assert!(ann.end_badges.is_empty(), "{:?}", ann.end_badges);
        for e in c.graph.edge_indices() {
            let b = ann.badges[&e];
            let (bw, bh) = badge_size(c.graph[e].step.as_deref().unwrap(), &t);
            let br = (b.0 - bw / 2.0, b.1 - bh / 2.0, bw, bh);
            for n in l.positions.values() {
                assert_eq!(overlap_area(inflate(br, BADGE_NODE_AIR - 0.5), (n.x, n.y, n.width, n.height)), 0.0, "badge {b:?} within {BADGE_NODE_AIR}px of a node");
            }
        }
    }

    #[test]
    fn a_long_arrow_carries_a_second_badge_before_its_head() {
        use rdg_schema::{DiagramPayload, EdgeDef, NodeDef};
        let node = |id: &str| NodeDef { id: id.into(), label: id.to_uppercase(), ..Default::default() };
        let compiled = rdg_graph::build_graph(&DiagramPayload {
            numbered: Some(true),
            nodes: vec![node("a"), node("b")],
            edges: vec![EdgeDef { from: "a".into(), to: "b".into(), ..Default::default() }],
            ..Default::default()
        })
        .unwrap();
        let cfg = rdg_layout::LayoutConfig { rank_spacing: END_BADGE_FROM as u32 + 100, ..Default::default() };
        let layout = rdg_layout::compute_layout(&compiled, &cfg).unwrap();
        let plans = crate::routing::plan_all_edge_routes(&compiled, &layout, crate::routing::RoutingAlgorithm::CornerHeuristic, &cfg.tokens);
        let ann = place_edge_annotations(&compiled, &layout, &plans, &cfg.tokens);
        let e = compiled.graph.edge_indices().next().unwrap();
        let path = edge_polyline(&compiled, &layout, e, plans.get(&e), &cfg.tokens).unwrap();
        assert!(path_len(&path) >= END_BADGE_FROM, "{}", path_len(&path));
        let head = path[path.len() - 1];
        let start = ann.badges[&e];
        let end = ann.end_badges.get(&e).copied().expect("an end badge");
        let dist = |p: Pt, q: Pt| (p.0 - q.0).hypot(p.1 - q.1);
        assert!(dist(end, head) <= 96.0 + 20.0, "end badge {end:?} is {:.0}px from the head {head:?}", dist(end, head));
        assert!(dist(start, path[0]) < dist(end, path[0]), "the start badge stays at the source");
    }

    #[test]
    fn labels_and_badges_stay_off_their_own_line() {
        let (c, l, p, t) = compile();
        let ann = place_edge_annotations(&c, &l, &p, &t);
        assert_eq!(ann.labels.len(), 2);
        assert_eq!(ann.badges.len(), 2);
        for e in c.graph.edge_indices() {
            let path = edge_polyline(&c, &l, e, p.get(&e), &t).unwrap();
            let lr = ann.labels[&e].rect();
            let b = ann.badges[&e];
            let (bw, bh) = badge_size(c.graph[e].step.as_deref().unwrap(), &t);
            let br = (b.0 - bw / 2.0, b.1 - bh / 2.0, bw, bh);
            for w in path.windows(2) {
                assert_eq!(clip_len(w[0], w[1], lr), 0.0, "label on its own line");
                assert_eq!(clip_len(w[0], w[1], br), 0.0, "badge on its own line");
            }
            assert_eq!(overlap_area(lr, br), 0.0);
        }
    }
}
