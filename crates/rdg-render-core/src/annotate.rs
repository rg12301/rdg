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

use petgraph::stable_graph::EdgeIndex;

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
    /// Badge centre per numbered edge.
    pub badges: HashMap<EdgeIndex, Pt>,
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

/// Start/end/choice markers draw their label *below* the shape, so edges leave from
/// further down (draw.io `exitY`, SVG `marker_clearance`).
pub fn is_marker_type(node_type: &str) -> bool {
    matches!(
        node_type.to_ascii_lowercase().as_str(),
        "start" | "start_state" | "initial" | "initial_state" | "end" | "end_state" | "final" | "final_state"
            | "choice" | "branch"
    )
}

/// The polyline an edge is drawn along: source port, waypoints, destination port — the
/// same points both backends draw (including the marker-exit push-down).
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
    let (x1, y1) = if src_side == Side::Bottom && is_marker_type(&compiled.graph[s_idx].node_type) {
        // Pushed down past the marker's own label (bounding-box point, as drawn).
        let (x, y) = crate::routing::port_point(src_nl, src_side, exit);
        (x, y + src_nl.height * (tokens.marker_label_clearance_ratio - 1.0))
    } else {
        node_attach_point(compiled, s_idx, src_nl, src_side, exit, tokens)
    };
    let p2 = node_attach_point(compiled, d_idx, dst_nl, dst_side, plan.map_or(0.5, |p| p.entry_port), tokens);
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
    paths: Vec<(EdgeIndex, Vec<Pt>)>,
    /// Badges and labels placed so far; the flag marks labels.
    placed: Vec<(EdgeIndex, bool, Rect)>,
}

// Penalty weights. Anything that makes text unreadable or ambiguous (sitting on a box,
// a line or other text) costs per px² / px of intrusion and dwarfs the soft
// preferences, which only break ties between clean spots.
const W_AREA: f64 = 60.0;
const W_LINE: f64 = 80.0;
const W_BORDER: f64 = 30.0;
const W_NEAR_LINE: f64 = 4.0;

impl Scene {
    /// `on_own_line`: the box deliberately sits on its own line (its background masks
    /// the stroke), so only other lines count against it.
    ///
    /// `gap`: distance from the box to its own line. Another line within about that
    /// distance makes the label ambiguous, so the "keep clear" band grows with it.
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
        for (e, path) in &self.paths {
            if on_own_line && *e == own {
                continue;
            }
            // Own line only needs to stay out of the box itself; a foreign line must
            // also keep a small gap so the label doesn't read as belonging to it.
            let (hard, near) = if *e == own { (inflate(r, 1.0), None) } else { (inflate(r, 3.0), Some(inflate(r, (gap + 8.0).max(12.0)))) };
            for w in path.windows(2) {
                c += W_LINE * clip_len(w[0], w[1], hard);
                if let Some(nr) = near {
                    c += W_NEAR_LINE * clip_len(w[0], w[1], nr);
                }
            }
        }
        for &(a, b) in &self.borders {
            c += W_BORDER * clip_len(a, b, padded);
        }
        c
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
        // Brand/tech icon straddling the card's top-left corner (22px, see backends).
        if compiled.graph[n].icon.is_some() {
            nodes.push((nl.x - 11.0, nl.y - 11.0, 22.0, 22.0));
        }
    }
    let titles = crate::routing::compute_group_title_zones(compiled, layout, tokens)
        .iter()
        .map(|t| (t.min_x, t.min_y, t.max_x - t.min_x, t.max_y - t.min_y))
        .collect();
    let mut borders = Vec::new();
    for (x, y, w, h) in crate::canvas::group_rects(compiled, layout, tokens) {
        let (a, b, c, d) = ((x, y), (x + w, y), (x + w, y + h), (x, y + h));
        borders.extend([(a, b), (b, c), (c, d), (d, a)]);
    }
    let mut scene = Scene { nodes, titles, borders, paths, placed: Vec::new() };

    // --- Badges ---------------------------------------------------------------
    let mut numbered: Vec<(u32, usize)> = scene
        .paths
        .iter()
        .enumerate()
        .filter_map(|(i, (e, _))| compiled.graph[*e].step.map(|s| (s, i)))
        .collect();
    numbered.sort();
    for (_, i) in numbered {
        let (edge_idx, path) = scene.paths[i].clone();
        let total = path_len(&path);
        let r = tokens.badge_radius;
        let off = r + 5.0;
        let d_max = (total * 0.5).clamp(16.0, 96.0);
        let mut best: Option<(f64, Pt)> = None;
        let mut d = 16.0;
        while d <= d_max + 1e-9 {
            let (p, dir, _) = point_at(&path, d.min(total));
            // Right-hand side of travel first (screen coords, y down) — a stable tie-break.
            for (k, sign) in [(0.0, 1.0), (3.0, -1.0)] {
                let c = (p.0 - dir.1 * off * sign, p.1 + dir.0 * off * sign);
                let rect = (c.0 - r, c.1 - r, 2.0 * r, 2.0 * r);
                let cost = scene.cost(rect, edge_idx, false, 0.0) + d * 0.6 + k;
                if best.is_none_or(|(b, _)| cost < b - 1e-9) {
                    best = Some((cost, c));
                }
            }
            d += 6.0;
        }
        if let Some((_, c)) = best {
            scene.placed.push((edge_idx, false, (c.0 - r, c.1 - r, 2.0 * r, 2.0 * r)));
            out.badges.insert(edge_idx, c);
        }
    }

    // --- Labels ---------------------------------------------------------------
    let mut order: Vec<(f64, usize)> = scene
        .paths
        .iter()
        .enumerate()
        .filter(|(_, (e, _))| compiled.graph[*e].label.as_deref().is_some_and(|l| !l.trim().is_empty()))
        .map(|(i, (_, p))| (path_len(p), i))
        .collect();
    order.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));

    let place = |scene: &Scene, i: usize| -> Option<LabelSpot> {
        let (edge_idx, path) = &scene.paths[i];
        let text = compiled.graph[*edge_idx].label.as_deref()?.trim();
        let total = path_len(path);
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
        // Distance between the line and the near edge of the box: snug first, a little
        // further out (less clearly attached, so penalised) when snug collides.
        // Last resort: centred on the line itself, the label background masking the
        // stroke — still clearly its own, and better than covering another line.
        const ON_LINE: f64 = -1.0;
        let gaps = [(4.0, 0.0), (12.0, 40.0), (24.0, 120.0), (ON_LINE, 260.0)];
        let mut best: Option<(f64, LabelSpot)> = None;
        for (s0, s1) in segment_spans(path) {
            let len = s1 - s0;
            if len < 1.0 {
                continue;
            }
            // mxGraph attaches a label sitting exactly on a bend to the *next* segment,
            // which would swing the perpendicular offset onto the wrong axis — so stay a
            // couple of px inside the segment.
            let lo = s0.max(end_clear) + 2.0_f64.min(len / 2.0);
            let hi = s1.min(total - end_clear) - 2.0_f64.min(len / 2.0);
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
                            let spot = LabelSpot { text: variant.clone(), cx, cy, w, h, anchor: p, dir, along: s, total };
                            let soft = if horizontal { 0.0 } else { 30.0 }
                                + 0.1 * (s - seg_mid).abs()
                                + 0.05 * (s - total / 2.0).abs()
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
    fn drawio_geometry_round_trips_perpendicular_offset() {
        // Horizontal rightward leg, label 12px above: mxGraph moves up for positive gy.
        let s = LabelSpot { text: String::new(), cx: 50.0, cy: -12.0, w: 20.0, h: 10.0, anchor: (50.0, 0.0), dir: (1.0, 0.0), along: 50.0, total: 100.0 };
        assert_eq!(s.drawio_geometry(), (0.0, 12.0));
        // Downward leg, label 20px to the right: positive gy moves right.
        let s = LabelSpot { text: String::new(), cx: 20.0, cy: 75.0, w: 20.0, h: 10.0, anchor: (0.0, 75.0), dir: (0.0, 1.0), along: 75.0, total: 100.0 };
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
    fn labels_and_badges_stay_off_their_own_line() {
        let (c, l, p, t) = compile();
        let ann = place_edge_annotations(&c, &l, &p, &t);
        assert_eq!(ann.labels.len(), 2);
        assert_eq!(ann.badges.len(), 2);
        for e in c.graph.edge_indices() {
            let path = edge_polyline(&c, &l, e, p.get(&e), &t).unwrap();
            let lr = ann.labels[&e].rect();
            let b = ann.badges[&e];
            let r = t.badge_radius;
            let br = (b.0 - r, b.1 - r, 2.0 * r, 2.0 * r);
            for w in path.windows(2) {
                assert_eq!(clip_len(w[0], w[1], lr), 0.0, "label on its own line");
                assert_eq!(clip_len(w[0], w[1], br), 0.0, "badge on its own line");
            }
            assert_eq!(overlap_area(lr, br), 0.0);
        }
    }
}
