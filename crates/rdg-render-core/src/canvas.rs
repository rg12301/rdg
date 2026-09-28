//! Canvas framing: the one place that decides where a diagram's content actually starts
//! and ends, and that puts the configured margin on all four sides of it.
//!
//! Before this module, "the canvas" was three unrelated guesses: layout placed nodes at
//! `margin` (left/top only), the SVG renderer bounded nodes + group boxes and added a
//! hardcoded right/bottom margin, and edge routes, label pills and the title were not
//! counted at all — so a detour or a wide label could poke past the edge while a user's
//! `canvas.margin` only ever moved two sides. [`content_bounds`] measures everything that
//! is really drawn; [`finalize_canvas`] translates the scene so its top-left content
//! corner sits exactly on the margin, which also makes right/bottom margin the same by
//! construction (renderers size the canvas as `max + min`).

use std::collections::HashMap;

use petgraph::stable_graph::EdgeIndex;

use rdg_graph::CompiledGraph;
use rdg_layout::{DesignTokens, LayoutResult};

use crate::routing::EdgeRoutingPlan;

/// Axis-aligned extent of everything drawn on the canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl Bounds {
    fn empty() -> Self {
        Self { min_x: f64::MAX, min_y: f64::MAX, max_x: f64::MIN, max_y: f64::MIN }
    }

    fn add_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        self.min_x = self.min_x.min(x);
        self.min_y = self.min_y.min(y);
        self.max_x = self.max_x.max(x + w);
        self.max_y = self.max_y.max(y + h);
    }

    fn is_empty(&self) -> bool {
        self.min_x > self.max_x
    }
}

/// Font size of the diagram title (renderers draw it at this size, bold).
const TITLE_FONT_SIZE: f64 = 15.0;

/// A group container's drawn rectangle: member bounding box plus content-aware padding
/// and the title header row.
pub fn group_rects(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    tokens: &DesignTokens,
) -> Vec<(f64, f64, f64, f64)> {
    let mut out = Vec::new();
    for group in &compiled.groups {
        let mut b = Bounds::empty();
        for node_id in &group.nodes {
            if let Some(nl) = compiled.node_map.get(node_id).and_then(|i| layout.positions.get(i)) {
                b.add_rect(nl.x, nl.y, nl.width, nl.height);
            }
        }
        if b.is_empty() {
            continue;
        }
        let (cw, ch) = (b.max_x - b.min_x, b.max_y - b.min_y);
        let pad = tokens.group_pad_for(cw, ch);
        let pad_top = tokens.group_pad_top_for(cw, ch);
        out.push((b.min_x - pad, b.min_y - pad_top, cw + pad * 2.0, ch + pad_top + pad));
    }
    out
}

fn edge_label_pill(text: &str, tokens: &DesignTokens) -> (f64, f64) {
    let font = tokens.font_size * 0.83;
    let w = (text.chars().count() as f64 * tokens.char_width(font) + tokens.px(1.0)).max(tokens.px(2.5));
    (w, tokens.line_height(font))
}

/// Extent of everything drawn: node boxes, group containers, routed waypoints, edge
/// label pills (at each path's midpoint), and the title band. `None` when the
/// layout has no nodes.
pub fn content_bounds(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    edge_plans: &HashMap<EdgeIndex, EdgeRoutingPlan>,
    tokens: &DesignTokens,
) -> Option<Bounds> {
    if layout.positions.is_empty() {
        return None;
    }
    let mut b = Bounds::empty();
    for nl in layout.positions.values() {
        b.add_rect(nl.x, nl.y, nl.width, nl.height);
    }
    for (x, y, w, h) in group_rects(compiled, layout, tokens) {
        b.add_rect(x, y, w, h);
    }
    for (edge_idx, plan) in edge_plans {
        for &(x, y) in &plan.waypoints {
            b.add_rect(x, y, 0.0, 0.0);
        }
        // The label pill is centred on the path's midpoint (where both renderers anchor
        // it), so that — not every waypoint — is where it can poke past the content.
        let Some(label) = compiled.graph.edge_weight(*edge_idx).and_then(|e| e.label.as_deref()) else {
            continue;
        };
        let Some((_, _, src_nl, dst_nl)) = crate::routing::resolve_edge_layout(compiled, layout, *edge_idx) else {
            continue;
        };
        let p1 = crate::routing::port_point(src_nl, plan.src_side, plan.exit_port);
        let p2 = crate::routing::port_point(dst_nl, plan.dst_side, plan.entry_port);
        let (mx, my) = crate::routing::polyline_midpoint(p1, &plan.waypoints, p2);
        let (w, h) = edge_label_pill(label, tokens);
        b.add_rect(mx - w / 2.0, my - h / 2.0, w, h);
    }
    if let Some(title) = &compiled.title {
        b.min_y -= tokens.title_band();
        let title_w = title.chars().count() as f64 * tokens.char_width(TITLE_FONT_SIZE) * 1.1;
        b.max_x = b.max_x.max(b.min_x + title_w);
    }
    Some(b)
}

/// Canvas size for `bounds`: content extent plus a margin on the right/bottom equal to
/// the one on the left/top (which [`finalize_canvas`] already set to the configured
/// margin). Uses `min` as the margin so a scene that skipped finalization still gets a
/// symmetric frame.
pub fn canvas_size(bounds: &Bounds) -> (f64, f64) {
    (bounds.max_x + bounds.min_x.max(0.0), bounds.max_y + bounds.min_y.max(0.0))
}

/// Translates layout and routes so the content's top-left corner (title band included)
/// sits exactly on `(margin_x, margin_y)`. Sequence diagrams have their own lifeline
/// geometry and are left alone.
pub fn finalize_canvas(
    compiled: &CompiledGraph,
    layout: &mut LayoutResult,
    edge_plans: &mut HashMap<EdgeIndex, EdgeRoutingPlan>,
    margin_x: f64,
    margin_y: f64,
    tokens: &DesignTokens,
) {
    if layout.sequence_info.is_some() {
        return;
    }
    let Some(b) = content_bounds(compiled, layout, edge_plans, tokens) else {
        return;
    };
    // Whole pixels: a fractional shift would put snapped nodes, ports and waypoints back
    // off the pixel grid the polish stage just put them on.
    let (dx, dy) = ((margin_x - b.min_x).round(), (margin_y - b.min_y).round());
    if dx == 0.0 && dy == 0.0 {
        return;
    }
    for nl in layout.positions.values_mut() {
        nl.x += dx;
        nl.y += dy;
    }
    for plan in edge_plans.values_mut() {
        for p in &mut plan.waypoints {
            p.0 += dx;
            p.1 += dy;
        }
        plan.channel_y += dy;
        plan.corridor_x += dx;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rdg_graph::build_graph;
    use rdg_layout::NodeLayout;
    use rdg_schema::{DiagramPayload, EdgeDef, NodeDef};

    fn compiled_two() -> CompiledGraph {
        build_graph(&DiagramPayload {
            nodes: vec![
                NodeDef { id: "a".into(), label: "A".into(), ..Default::default() },
                NodeDef { id: "b".into(), label: "B".into(), ..Default::default() },
            ],
            edges: vec![EdgeDef { from: "a".into(), to: "b".into(), ..Default::default() }],
            ..Default::default()
        })
        .unwrap()
    }

    fn layout_of(compiled: &CompiledGraph, at: [(f64, f64); 2]) -> LayoutResult {
        let mut positions = HashMap::new();
        for (idx, (x, y)) in compiled.graph.node_indices().zip(at) {
            positions.insert(idx, NodeLayout { x, y, width: 100.0, height: 40.0 });
        }
        LayoutResult { positions, sequence_info: None }
    }

    #[test]
    fn test_finalize_puts_margin_on_all_four_sides() {
        let compiled = compiled_two();
        let tokens = DesignTokens::default();
        let mut layout = layout_of(&compiled, [(-30.0, 5.0), (200.0, 300.0)]);
        let mut plans = HashMap::new();
        finalize_canvas(&compiled, &mut layout, &mut plans, 40.0, 50.0, &tokens);
        let b = content_bounds(&compiled, &layout, &plans, &tokens).unwrap();
        assert_eq!((b.min_x, b.min_y), (40.0, 50.0));
        let (w, h) = canvas_size(&b);
        assert_eq!(w - b.max_x, 40.0, "right margin equals left margin");
        assert_eq!(h - b.max_y, 50.0, "bottom margin equals top margin");
    }

    #[test]
    fn test_waypoints_outside_nodes_count_as_content() {
        let compiled = compiled_two();
        let tokens = DesignTokens::default();
        let mut layout = layout_of(&compiled, [(24.0, 24.0), (24.0, 200.0)]);
        let edge = compiled.graph.edge_indices().next().unwrap();
        let mut plans = HashMap::new();
        plans.insert(
            edge,
            EdgeRoutingPlan {
                src_side: crate::routing::Side::Left,
                dst_side: crate::routing::Side::Left,
                exit_port: 0.5,
                entry_port: 0.5,
                channel_y: 0.0,
                corridor_x: -8.0,
                waypoints: vec![(-8.0, 44.0), (-8.0, 220.0)],
                corridor_bucket_size: 1,
            },
        );
        finalize_canvas(&compiled, &mut layout, &mut plans, 24.0, 24.0, &tokens);
        assert_eq!(plans[&edge].waypoints[0].0, 24.0, "detour shifted onto the margin");
        let node = layout.positions.values().map(|n| n.x).fold(f64::MIN, f64::max);
        assert_eq!(node, 56.0, "nodes shifted by the same amount");
    }
}
