//! Framing: where the title (with its description) and the legend go, and how big the
//! canvas is.
//!
//! The diagram is laid out and routed first, with no band reserved for either block.
//! Then everything drawn — node boxes, containers, arrow paths, edge labels, flow
//! badges — becomes an obstacle, and the blocks go into a **corner** of the diagram's
//! extent, flush to its two edges: as far from the diagram as the canvas allows, where
//! a reader looks for a title, never in a gap between groups. Corners are tried in
//! reading order — top-left, bottom-left (a caption), top-right, bottom-right — first
//! for the title with the legend stacked under it, then for each block on its own; a
//! spot counts only if it clears every obstacle by [`separation`]. A block no corner can
//! hold goes outside the diagram: the title above it, the legend below it. The canvas
//! margin is then applied evenly around everything, blocks included.
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
    for (x, y, w, h) in rdg_layout::groups::group_rects(compiled, layout, tokens).into_iter().flatten() {
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
    for (&e, &(x, y)) in &ann.badges {
        let (w, h) = crate::annotate::badge_size(compiled.graph[e].step.as_deref().unwrap_or_default(), tokens);
        out.push((x - w / 2.0, y - h / 2.0, x + w / 2.0, y + h / 2.0));
    }
    out
}

/// Distance between two rectangles (0 when they touch or overlap).
fn rect_dist(a: Rect, b: Rect) -> f64 {
    let dx = (b.0 - a.2).max(a.0 - b.2).max(0.0);
    let dy = (b.1 - a.3).max(a.1 - b.3).max(0.0);
    dx.hypot(dy)
}

/// Corners a block may sit in, in the order a reader looks for a title: top-left, then
/// bottom-left (where a caption goes), top-right, bottom-right.
const CORNERS: [(bool, bool); 4] = [(false, false), (false, true), (true, false), (true, true)];

/// Top-left of a `w`×`h` block in a corner of `area` (`right`, `bottom` pick which),
/// and the side its lines align to.
fn corner_spot((right, bottom): (bool, bool), w: f64, h: f64, area: Rect) -> (f64, f64, Align) {
    let x = if right { area.2 - w } else { area.0 };
    let y = if bottom { area.3 - h } else { area.1 };
    (x, y, if right { Align::Right } else { Align::Left })
}

/// Distance from `r` to the nearest obstacle.
fn clearance(r: Rect, obs: &[Rect]) -> f64 {
    obs.iter().map(|&o| rect_dist(r, o)).fold(f64::MAX, f64::min)
}

/// Rows a legend shape takes when wrapped at its width.
fn legend_line_count(&(max_w, _, _): &(f64, f64, f64), items: &[LegendItem], theme: &Theme) -> usize {
    crate::look::legend_rows(items, theme, max_w).len().max(1)
}

/// Whether `r` clears every obstacle by `sep`.
fn clear(r: Rect, obs: &[Rect], sep: f64) -> bool {
    obs.iter().all(|&o| rect_dist(r, o) >= sep)
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
    let obs = obstacles(compiled, layout, plans, tokens);
    let area: Rect = (b.min_x, b.min_y, b.max_x, b.max_y);

    // Candidate shapes: the title's description on one line, then wrapped narrower; the
    // legend in one row, then in narrower columns.
    let titles = compiled.title.as_deref().map(|t| title_shapes(t, compiled.description.as_deref(), theme, tokens)).unwrap_or_default();
    let items = legend_enabled(theme, compiled).then(|| legend_items(theme, compiled)).filter(|v| !v.is_empty());
    let legends: Vec<(f64, f64, f64)> = items.as_ref().map_or_else(Vec::new, |items| {
        let full = (area.2 - area.0).max(tokens.px(40.0));
        let mut shapes: Vec<(f64, f64, f64)> = Vec::new(); // (max_w, w, h)
        for frac in [1.0, 0.6, 0.4, 0.3, 0.2, 0.12] {
            let (w, h) = legend_size(items, theme, full * frac);
            if !shapes.iter().any(|s| (s.1 - w).abs() < 1.0 && (s.2 - h).abs() < 1.0) {
                shapes.push((full * frac, w, h));
            }
        }
        shapes
    });
    let stack_gap = tokens.px(2.0);
    // Clearance beyond this reads as "apart from the diagram"; each extra line of text,
    // and each step down the corner order, costs a little.
    let (comfort, line_cost, corner_cost) = (tokens.px(10.0), tokens.px(0.75), tokens.px(0.5));
    let place_title = |s: &TitleBlock, x: f64, y: f64, align: Align| TitleBlock { x: x.round(), y: y.round(), align, ..s.clone() };
    let place_legend = |&(max_w, w, h): &(f64, f64, f64), x: f64, y: f64, align: Align| LegendBlock {
        x: x.round(),
        y: y.round(),
        w,
        h,
        align,
        max_w,
        items: items.clone().unwrap_or_default(),
    };

    // --- Together: the title with the legend under it, in one corner -----------------
    // Every corner × every wrapping is scored: clearance from the diagram first (up to a
    // comfortable distance — a long one-line description or one-row legend reaching back
    // over the diagram loses to a narrower, taller block tucked into the corner), then
    // fewer lines, then reading order among corners.
    let mut title: Option<TitleBlock> = None;
    let mut legend: Option<LegendBlock> = None;
    let score = |r: Rect, obs: &[Rect], lines: usize, corner: usize| clearance(r, obs).min(comfort) - lines as f64 * line_cost - corner as f64 * corner_cost;
    if !titles.is_empty() && !legends.is_empty() {
        let mut best: Option<(f64, TitleBlock, LegendBlock)> = None;
        for (ci, &corner) in CORNERS.iter().enumerate() {
            for t in &titles {
                for l in &legends {
                    let (w, h) = (t.w.max(l.1), t.h + stack_gap + l.2);
                    let (x, y, align) = corner_spot(corner, w, h, area);
                    let r = (x, y, x + w, y + h);
                    if !clear(r, &obs, sep) {
                        continue;
                    }
                    let sc = score(r, &obs, t.description_lines.len() + legend_line_count(l, items.as_deref().unwrap_or_default(), theme), ci);
                    if best.as_ref().is_none_or(|b| sc > b.0 + 1e-6) {
                        best = Some((sc, place_title(t, align.line_x(x, w, t.w), y, align), place_legend(l, align.line_x(x, w, l.1), y + t.h + stack_gap, align)));
                    }
                }
            }
        }
        if let Some((_, t, l)) = best {
            (title, legend) = (Some(t), Some(l));
        }
    }

    // --- Otherwise each on its own: a free corner, else outside the diagram ----------
    let mut obs = obs;
    if title.is_none() && !titles.is_empty() {
        let spot = CORNERS
            .iter()
            .enumerate()
            .flat_map(|(ci, &c)| titles.iter().map(move |t| (ci, c, t)))
            .filter_map(|(ci, c, t)| {
                let (x, y, align) = corner_spot(c, t.w, t.h, area);
                let r = (x, y, x + t.w, y + t.h);
                clear(r, &obs, sep).then(|| (score(r, &obs, t.description_lines.len(), ci), place_title(t, x, y, align)))
            })
            .fold(None::<(f64, TitleBlock)>, |b, c| if b.as_ref().is_none_or(|b| c.0 > b.0 + 1e-6) { Some(c) } else { b })
            .map(|(_, t)| t);
        // Above the diagram, the description on one line up to the diagram's width.
        title = spot.or_else(|| {
            let t = titles.iter().find(|s| s.w <= (area.2 - area.0).max(tokens.px(40.0))).unwrap_or(&titles[titles.len() - 1]);
            Some(place_title(t, area.0, area.1 - sep - t.h, Align::Left))
        });
        if let Some(t) = &title {
            obs.push((t.x, t.y, t.x + t.w, t.y + t.h));
        }
    }
    if legend.is_none() && !legends.is_empty() {
        let spot = CORNERS
            .iter()
            .enumerate()
            .flat_map(|(ci, &c)| legends.iter().map(move |l| (ci, c, l)))
            .filter_map(|(ci, c, l)| {
                let (x, y, align) = corner_spot(c, l.1, l.2, area);
                let r = (x, y, x + l.1, y + l.2);
                clear(r, &obs, sep).then(|| (score(r, &obs, legend_line_count(l, items.as_deref().unwrap_or_default(), theme), ci), place_legend(l, x, y, align)))
            })
            .fold(None::<(f64, LegendBlock)>, |b, c| if b.as_ref().is_none_or(|b| c.0 > b.0 + 1e-6) { Some(c) } else { b })
            .map(|(_, l)| l);
        // Below the diagram.
        legend = spot.or_else(|| Some(place_legend(&legends[0], area.0, area.3 + sep, Align::Left)));
    }

    let (mut min_x, mut min_y, mut max_x, mut max_y) = area;
    for (x, y, w, h) in title.iter().map(|t| (t.x, t.y, t.w, t.h)).chain(legend.iter().map(|l| (l.x, l.y, l.w, l.h))) {
        (min_x, min_y, max_x, max_y) = (min_x.min(x), min_y.min(y), max_x.max(x + w), max_y.max(y + h));
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
    fn corners_are_tried_in_reading_order() {
        let area = (0.0, 0.0, 1000.0, 400.0);
        // The top-left is taken: the block goes to the bottom-left, flush to both edges.
        let obs = [(0.0, 0.0, 300.0, 200.0)];
        let spot = CORNERS.iter().map(|&c| corner_spot(c, 150.0, 40.0, area)).find(|&(x, y, _)| clear((x, y, x + 150.0, y + 40.0), &obs, 24.0));
        assert_eq!(spot, Some((0.0, 360.0, Align::Left)));
        // Both left corners taken: top-right, aligned right.
        let obs = [(0.0, 0.0, 300.0, 400.0)];
        let spot = CORNERS.iter().map(|&c| corner_spot(c, 150.0, 40.0, area)).find(|&(x, y, _)| clear((x, y, x + 150.0, y + 40.0), &obs, 24.0));
        assert_eq!(spot, Some((850.0, 0.0, Align::Right)));
    }
}
