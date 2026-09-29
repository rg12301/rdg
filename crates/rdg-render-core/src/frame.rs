//! Framing: where the title (with its description) and the legend go, and how big the
//! canvas is.
//!
//! The diagram is laid out and routed first, with no band reserved for either block.
//! Then everything drawn — node boxes, containers, arrow paths, edge labels, flow
//! badges — becomes an obstacle, and each block is fitted into the **largest white
//! patch** that can hold it: a few box shapes are tried (the description wrapped at
//! different widths, the legend at different row lengths), every candidate position
//! scanned over the diagram's extent, and a spot kept only if it clears every obstacle
//! by [`separation`]. Among those, the one sitting in the biggest free area wins; ties
//! go to reading order (top, then left). When no patch fits, the title goes above the
//! diagram and the legend below it. The canvas margin is then applied evenly around
//! everything, blocks included.
//!
//! Placement is translation-invariant (the scan is anchored to the content's own
//! corner), so [`place_frame`] can shift the scene onto the margin and the renderers
//! recompute the very same frame with [`compute_frame`].

use std::collections::HashMap;

use petgraph::stable_graph::EdgeIndex;

use rdg_graph::CompiledGraph;
use rdg_layout::{DesignTokens, LayoutResult, wrap_label};

use crate::look::{LegendItem, legend_enabled, legend_items, legend_size};
use crate::routing::EdgeRoutingPlan;
use crate::theme::Theme;

/// An axis-aligned rectangle `(x0, y0, x1, y1)`.
type Rect = (f64, f64, f64, f64);

/// How a block's lines line up inside it — matched to where it sits on the canvas
/// (see [`align_to_canvas`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

impl Align {
    /// x of a `line_w`-wide line inside a block spanning `[x, x + w]`.
    pub fn line_x(self, x: f64, w: f64, line_w: f64) -> f64 {
        match self {
            Align::Left => x,
            Align::Center => x + (w - line_w) / 2.0,
            Align::Right => x + w - line_w,
        }
    }
}

/// The title block: bold title line(s), then the description's lines.
#[derive(Debug, Clone, PartialEq)]
pub struct TitleBlock {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub align: Align,
    pub title_lines: Vec<String>,
    pub description_lines: Vec<String>,
    /// Line heights: title lines, description lines, and the gap between the two.
    pub title_lh: f64,
    pub description_lh: f64,
    pub gap: f64,
}

/// The legend block: drawn at `(x, y)`, its rows wrapped at `max_w`.
#[derive(Debug, Clone)]
pub struct LegendBlock {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub align: Align,
    pub max_w: f64,
    pub items: Vec<LegendItem>,
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub title: Option<TitleBlock>,
    pub legend: Option<LegendBlock>,
    /// Extent of everything drawn, blocks included.
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl Frame {
    /// Canvas size: the content plus a right/bottom margin equal to the left/top one.
    pub fn canvas_size(&self) -> (f64, f64) {
        (self.max_x + self.min_x.max(0.0), self.max_y + self.min_y.max(0.0))
    }
}

/// Minimum clear space between a block and anything drawn.
pub fn separation(tokens: &DesignTokens) -> f64 {
    tokens.px(3.0)
}

/// Obstacles: everything the diagram draws, as rectangles (arrow segments as thin ones).
fn obstacles(compiled: &CompiledGraph, layout: &LayoutResult, plans: &HashMap<EdgeIndex, EdgeRoutingPlan>, tokens: &DesignTokens) -> Vec<Rect> {
    let mut out: Vec<Rect> = layout.positions.values().map(|n| (n.x, n.y, n.x + n.width, n.y + n.height)).collect();
    for (x, y, w, h) in crate::canvas::group_rects(compiled, layout, tokens) {
        out.push((x, y, x + w, y + h));
    }
    for &e in &compiled.edge_order {
        if let Some(pts) = crate::annotate::edge_polyline(compiled, layout, e, plans.get(&e), tokens) {
            for w in pts.windows(2) {
                out.push((w[0].0.min(w[1].0), w[0].1.min(w[1].1), w[0].0.max(w[1].0), w[0].1.max(w[1].1)));
            }
        }
    }
    let ann = crate::annotate::place_edge_annotations(compiled, layout, plans, tokens);
    for spot in ann.labels.values() {
        let (x, y, w, h) = spot.rect();
        out.push((x, y, x + w, y + h));
    }
    let r = tokens.badge_radius;
    for &(x, y) in ann.badges.values() {
        out.push((x - r, y - r, x + r, y + r));
    }
    out
}

/// Distance between two rectangles (0 when they touch or overlap).
fn rect_dist(a: Rect, b: Rect) -> f64 {
    let dx = (b.0 - a.2).max(a.0 - b.2).max(0.0);
    let dy = (b.1 - a.3).max(a.1 - b.3).max(0.0);
    dx.hypot(dy)
}

/// How big the white patch around `r` is: the larger of the two free rectangles grown
/// out from it (sideways first then up/down, and up/down first then sideways), inside
/// `area`.
fn patch_area(r: Rect, obs: &[Rect], area: Rect) -> f64 {
    let overlaps_y = |o: &Rect, y0: f64, y1: f64| o.1 < y1 && o.3 > y0;
    let overlaps_x = |o: &Rect, x0: f64, x1: f64| o.0 < x1 && o.2 > x0;
    let sideways = |y0: f64, y1: f64| {
        let left = obs.iter().filter(|o| o.2 <= r.0 && overlaps_y(o, y0, y1)).map(|o| o.2).fold(area.0, f64::max);
        let right = obs.iter().filter(|o| o.0 >= r.2 && overlaps_y(o, y0, y1)).map(|o| o.0).fold(area.2, f64::min);
        (left, right)
    };
    let vertical = |x0: f64, x1: f64| {
        let up = obs.iter().filter(|o| o.3 <= r.1 && overlaps_x(o, x0, x1)).map(|o| o.3).fold(area.1, f64::max);
        let down = obs.iter().filter(|o| o.1 >= r.3 && overlaps_x(o, x0, x1)).map(|o| o.1).fold(area.3, f64::min);
        (up, down)
    };
    let (l, rr) = sideways(r.1, r.3);
    let (u, d) = vertical(l, rr);
    let a = (rr - l).max(0.0) * (d - u).max(0.0);
    let (u2, d2) = vertical(r.0, r.2);
    let (l2, r2) = sideways(u2, d2);
    let b = (r2 - l2).max(0.0) * (d2 - u2).max(0.0);
    a.max(b)
}

/// Best spot for a `w`×`h` block inside `area`, clear of `obs` by `sep`: one in the
/// biggest free patch (patch sizes compared in `4 × step` px buckets, so spots within
/// one patch tie), then the highest, then the leftmost — the patch's top-left corner.
/// Returns `(score, x, y)`; `None` when nothing fits.
fn best_spot(w: f64, h: f64, obs: &[Rect], area: Rect, sep: f64, step: f64) -> Option<(f64, f64, f64)> {
    let bucket = 4.0 * step;
    let (x_lo, y_lo, x_hi, y_hi) = (area.0, area.1, area.2 - w, area.3 - h);
    if x_hi < x_lo || y_hi < y_lo {
        return None;
    }
    let mut best: Option<(f64, f64, f64)> = None; // (score, x, y)
    let nx = ((x_hi - x_lo) / step).floor() as usize;
    let ny = ((y_hi - y_lo) / step).floor() as usize;
    for j in 0..=ny {
        let y = y_lo + j as f64 * step;
        for i in 0..=nx {
            let x = x_lo + i as f64 * step;
            let r = (x, y, x + w, y + h);
            if obs.iter().any(|&o| rect_dist(r, o) < sep) {
                continue;
            }
            // Score by the free area around (in px², square-rooted to a length), so a
            // spot in a big empty region beats a snug one in a small gap.
            let score = (patch_area(r, obs, area).sqrt() / bucket).round() * bucket;
            // Scanned top-down, left-right: a tie keeps the earlier (higher, then left).
            if best.is_none_or(|(s, _, _)| score > s + 1e-6) {
                best = Some((score, x, y));
            }
        }
    }
    best
}

/// Align a block to the canvas edge it sits nearest, the layout convention of anchoring
/// text to the frame it shares with the content: in the left third of the canvas it
/// reads left and moves flush to the left edge, in the right third it reads right and
/// moves flush to the right edge, in between it is centred on the canvas. It only
/// slides along its own row, as far as the free space there allows.
fn align_to_canvas(x: f64, y: f64, w: f64, h: f64, obs: &[Rect], area: Rect, sep: f64) -> (f64, Align) {
    let rows = |o: &&Rect| o.1 < y + h + sep && o.3 > y - sep;
    let fl = obs.iter().filter(rows).filter(|o| o.2 <= x + 0.5).map(|o| o.2 + sep).fold(area.0, f64::max);
    let fr = obs.iter().filter(rows).filter(|o| o.0 >= x + w - 0.5).map(|o| o.0 - sep).fold(area.2, f64::min);
    let rel = (x + w / 2.0 - area.0) / (area.2 - area.0).max(1.0);
    if rel < 1.0 / 3.0 {
        (fl, Align::Left)
    } else if rel > 2.0 / 3.0 {
        ((fr - w).max(fl), Align::Right)
    } else {
        (((area.0 + area.2 - w) / 2.0).clamp(fl, (fr - w).max(fl)), Align::Center)
    }
}

/// Title block shapes to try: the description on one line, then wrapped narrower.
fn title_shapes(title: &str, description: Option<&str>, theme: &Theme, tokens: &DesignTokens) -> Vec<TitleBlock> {
    let f = &theme.font;
    let (tcw, dcw) = (tokens.char_width(f.title_size), tokens.char_width(f.description_size));
    let title_lines: Vec<String> = title.lines().map(str::to_string).collect();
    let title_w = title_lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as f64 * tcw;
    let (title_lh, description_lh) = (tokens.line_height(f.title_size), tokens.line_height(f.description_size));
    let gap = tokens.px(0.25);
    let mut out = Vec::new();
    let widths: Vec<Option<usize>> = match description {
        Some(d) => {
            let n = d.chars().count();
            // One line; as wide as the title; then progressively narrower columns.
            let at_title = ((title_w / dcw) as usize).max(24);
            let mut ws = vec![None, Some(at_title), Some(60), Some(44), Some(32)];
            ws.retain(|w| w.is_none_or(|w| w < n));
            ws.dedup();
            ws
        }
        None => vec![None],
    };
    for wrap in widths {
        let description_lines: Vec<String> = match (description, wrap) {
            (Some(d), Some(w)) => wrap_label(d, w),
            (Some(d), None) => d.lines().map(str::to_string).collect(),
            (None, _) => Vec::new(),
        };
        let desc_w = description_lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as f64 * dcw;
        let h = title_lines.len() as f64 * title_lh + if description_lines.is_empty() { 0.0 } else { gap + description_lines.len() as f64 * description_lh };
        out.push(TitleBlock {
            x: 0.0,
            y: 0.0,
            w: title_w.max(desc_w),
            h,
            align: Align::Left,
            title_lines: title_lines.clone(),
            description_lines,
            title_lh,
            description_lh,
            gap,
        });
    }
    out
}

/// Where the title and legend go, and the resulting extent. Pure: no geometry moves.
pub fn compute_frame(compiled: &CompiledGraph, layout: &LayoutResult, plans: &HashMap<EdgeIndex, EdgeRoutingPlan>, theme: &Theme, tokens: &DesignTokens) -> Option<Frame> {
    let b = crate::canvas::content_bounds(compiled, layout, plans, tokens)?;
    let sep = separation(tokens);
    let step = tokens.px(1.0);
    let mut obs = obstacles(compiled, layout, plans, tokens);
    // The white patches are inside the diagram's own extent.
    let area: Rect = (b.min_x, b.min_y, b.max_x, b.max_y);
    let (mut min_x, mut min_y, mut max_x, mut max_y) = area;

    // --- Title + description --------------------------------------------------------
    let title = compiled.title.as_deref().map(|t| {
        let shapes = title_shapes(t, compiled.description.as_deref(), theme, tokens);
        // Fewer, longer lines read better: each extra wrap costs a little.
        let fit = shapes
            .iter()
            .enumerate()
            .filter_map(|(i, s)| best_spot(s.w, s.h, &obs, area, sep, step).map(|(score, x, y)| (score - i as f64 * tokens.px(2.0), x, y, i)))
            .max_by(|a, b| a.0.total_cmp(&b.0).then(b.2.total_cmp(&a.2)).then(b.1.total_cmp(&a.1)));
        let mut block = match fit {
            Some((_, x, y, i)) => {
                let mut s = shapes[i].clone();
                (s.x, s.y) = (x, y);
                s
            }
            // No patch holds it: above the diagram, a separation clear of it. Keep
            // the description on one line up to the diagram's width.
            None => {
                let mut s = shapes.iter().find(|s| s.w <= (area.2 - area.0).max(tokens.px(40.0))).unwrap_or(&shapes[shapes.len() - 1]).clone();
                (s.x, s.y) = (area.0, area.1 - sep - s.h);
                s
            }
        };
        (block.x, block.align) = align_to_canvas(block.x, block.y, block.w, block.h, &obs, area, sep);
        block.x = block.x.round();
        block.y = block.y.round();
        obs.push((block.x, block.y, block.x + block.w, block.y + block.h));
        block
    });
    if let Some(t) = &title {
        (min_x, min_y, max_x, max_y) = (min_x.min(t.x), min_y.min(t.y), max_x.max(t.x + t.w), max_y.max(t.y + t.h));
    }

    // --- Legend -----------------------------------------------------------------------
    let legend = legend_enabled(theme, compiled).then(|| legend_items(theme, compiled)).filter(|v| !v.is_empty()).map(|items| {
        let full = (area.2 - area.0).max(tokens.px(40.0));
        let mut shapes: Vec<(f64, f64, f64)> = Vec::new(); // (max_w, w, h)
        for frac in [1.0, 0.6, 0.4, 0.25] {
            let (w, h) = legend_size(&items, theme, full * frac);
            if !shapes.iter().any(|s| (s.1 - w).abs() < 1.0 && (s.2 - h).abs() < 1.0) {
                shapes.push((full * frac, w, h));
            }
        }
        let fit = shapes
            .iter()
            .filter_map(|&(mw, w, h)| best_spot(w, h, &obs, area, sep, step).map(|(score, x, y)| (score, x, y, mw, w, h)))
            .max_by(|a, b| a.0.total_cmp(&b.0).then(a.2.total_cmp(&b.2)).then(b.1.total_cmp(&a.1)));
        let (x, y, mw, w, h) = match fit {
            Some((_, x, y, mw, w, h)) => (x, y, mw, w, h),
            None => {
                let (mw, w, h) = shapes[0];
                (area.0, max_y.max(area.3) + sep, mw, w, h)
            }
        };
        let (mut x, mut align) = align_to_canvas(x, y, w, h, &obs, area, sep);
        // Stacked right under (or over) the title: share its alignment and edge, so the
        // two read as one block — when that spot is just as clear.
        if let Some(t) = &title {
            let stacked = x < t.x + t.w && x + w > t.x && (y - (t.y + t.h)).abs().min((t.y - (y + h)).abs()) < 3.0 * sep;
            let snapped = t.align.line_x(t.x, t.w, w);
            let r = (snapped, y, snapped + w, y + h);
            let own = (t.x, t.y, t.x + t.w, t.y + t.h);
            if stacked && obs.iter().filter(|&&o| o != own).all(|&o| rect_dist(r, o) >= sep) && snapped >= area.0 && snapped + w <= area.2.max(t.x + t.w) {
                (x, align) = (snapped, t.align);
            }
        }
        LegendBlock { x: x.round(), y: y.round(), w, h, align, max_w: mw, items }
    });
    if let Some(l) = &legend {
        (min_x, min_y, max_x, max_y) = (min_x.min(l.x), min_y.min(l.y), max_x.max(l.x + l.w), max_y.max(l.y + l.h));
    }
    Some(Frame { title, legend, min_x, min_y, max_x, max_y })
}

/// Place the blocks, then translate the scene so everything — blocks included — starts
/// at `(margin_x, margin_y)`; the canvas mirrors that margin on the right and bottom.
pub fn place_frame(
    compiled: &CompiledGraph,
    layout: &mut LayoutResult,
    plans: &mut HashMap<EdgeIndex, EdgeRoutingPlan>,
    theme: &Theme,
    tokens: &DesignTokens,
    margin_x: f64,
    margin_y: f64,
) -> Option<Frame> {
    if layout.sequence_info.is_some() {
        return None;
    }
    let f = compute_frame(compiled, layout, plans, theme, tokens)?;
    let (dx, dy) = ((margin_x - f.min_x).round(), (margin_y - f.min_y).round());
    for nl in layout.positions.values_mut() {
        nl.x += dx;
        nl.y += dy;
    }
    for plan in plans.values_mut() {
        for p in &mut plan.waypoints {
            p.0 += dx;
            p.1 += dy;
        }
        plan.channel_y += dy;
        plan.corridor_x += dx;
    }
    compute_frame(compiled, layout, plans, theme, tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_prefers_the_bigger_gap() {
        // Two free regions: a small one at the top-left, a big one at the right.
        let area = (0.0, 0.0, 1000.0, 400.0);
        let obs = vec![(0.0, 120.0, 400.0, 400.0), (400.0, 0.0, 420.0, 400.0)];
        let (_, x, _) = best_spot(150.0, 40.0, &obs, area, 24.0, 8.0).unwrap();
        assert!(x > 420.0, "picked the large right-hand patch, got x={x}");
    }

    #[test]
    fn nothing_fits_when_the_area_is_full() {
        let area = (0.0, 0.0, 300.0, 200.0);
        assert!(best_spot(150.0, 40.0, &[(0.0, 0.0, 300.0, 200.0)], area, 24.0, 8.0).is_none());
    }
}
