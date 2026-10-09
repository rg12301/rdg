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
    /// The rule under the header band: `(x0, x1, y)`.
    pub rule: Option<(f64, f64, f64)>,
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
    let area: Rect = (b.min_x, b.min_y, b.max_x, b.max_y);

    // Candidate shapes: the title's description on one line, then wrapped narrower; the
    // legend in one row, then in narrower columns.
    let titles = compiled.title.as_deref().map(|t| title_shapes(t, compiled.description.as_deref(), theme, tokens)).unwrap_or_default();
    let items = legend_enabled(theme, compiled).then(|| legend_items(theme, compiled)).filter(|v| !v.is_empty());
    // --- A header band above the diagram: title left, legend right, a rule below ----
    // The band spans the diagram's width. Title and legend share a row when both fit;
    // otherwise the legend wraps onto its own row under the title. The rule separates
    // the band from the drawing, and the drawing starts `separation` below it.
    let width = (area.2 - area.0).max(tokens.px(40.0));
    let gap = tokens.px(2.0);
    let title_shape = titles.iter().find(|t| t.w <= width).or(titles.last()).cloned();
    let legend_shape = items.as_ref().map(|items| {
        let (w, h) = legend_size(items, theme, width);
        (width, w, h)
    });
    let title_w = title_shape.as_ref().map_or(0.0, |t| t.w);
    let title_h = title_shape.as_ref().map_or(0.0, |t| t.h);
    let same_row = match (&title_shape, &legend_shape) {
        (Some(_), Some(l)) => title_w + 2.0 * gap + l.1 <= width,
        _ => false,
    };
    let legend_h = legend_shape.as_ref().map_or(0.0, |l| l.2);
    let band_h = if title_shape.is_some() && legend_shape.is_some() {
        if same_row { title_h.max(legend_h) } else { title_h + gap + legend_h }
    } else {
        title_h + legend_h
    };
    let (mut title, mut legend, mut rule) = (None, None, None);
    if band_h > 0.0 {
        let rule_y = (area.1 - sep).round();
        let band_top = (rule_y - tokens.px(1.5) - band_h).round();
        let band_right = area.0 + width.max(title_w);
        if let Some(t) = &title_shape {
            title = Some(TitleBlock { x: area.0.round(), y: band_top, align: Align::Left, ..t.clone() });
        }
        if let Some(&(max_w, w, h)) = legend_shape.as_ref() {
            let (x, y, align) = if title_shape.is_none() {
                (area.0, band_top, Align::Left)
            } else if same_row {
                (band_right - w, band_top, Align::Right)
            } else {
                (area.0, band_top + title_h + gap, Align::Left)
            };
            legend = Some(LegendBlock { x: x.round(), y: y.round(), w, h, align, max_w, items: items.clone().unwrap_or_default() });
        }
        rule = Some((area.0.round(), band_right.round(), rule_y));
    }

    let (mut min_x, mut min_y, mut max_x, mut max_y) = area;
    for (x, y, w, h) in title.iter().map(|t| (t.x, t.y, t.w, t.h)).chain(legend.iter().map(|l| (l.x, l.y, l.w, l.h))) {
        (min_x, min_y, max_x, max_y) = (min_x.min(x), min_y.min(y), max_x.max(x + w), max_y.max(y + h));
    }
    if let Some((x0, x1, _)) = rule {
        (min_x, max_x) = (min_x.min(x0), max_x.max(x1));
    }
    Some(Frame { title, legend, rule, min_x, min_y, max_x, max_y })
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
    fn the_band_sits_above_the_diagram_with_a_rule_between() {
        use rdg_schema::{DiagramPayload, EdgeDef, NodeDef};
        let node = |id: &str| NodeDef { id: id.into(), label: id.to_uppercase(), node_type: "service".into(), ..Default::default() };
        let compiled = rdg_graph::build_graph(&DiagramPayload {
            title: Some("Checkout".into()),
            description: Some("Orders and payments".into()),
            nodes: vec![node("a"), node("b")],
            edges: vec![EdgeDef { from: "a".into(), to: "b".into(), ..Default::default() }],
            ..Default::default()
        })
        .unwrap();
        let config = rdg_layout::LayoutConfig::default();
        let layout = rdg_layout::compute_layout(&compiled, &config).unwrap();
        let plans = crate::routing::plan_all_edge_routes(&compiled, &layout, rdg_dispatch::RoutingAlgorithm::VisibilityGraphAStar, &config.tokens);
        let theme = Theme::builtin("light").unwrap();
        let f = compute_frame(&compiled, &layout, &plans, &theme, &config.tokens).unwrap();
        let t = f.title.as_ref().unwrap();
        let (_, _, rule_y) = f.rule.unwrap();
        let top = layout.positions.values().map(|n| n.y).fold(f64::INFINITY, f64::min);
        assert!(t.y + t.h <= rule_y && rule_y < top, "title {}, rule {rule_y}, diagram {top}", t.y + t.h);
        if let Some(l) = &f.legend {
            assert!(l.y + l.h <= rule_y + 0.5);
        }
    }
}
