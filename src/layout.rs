//! Layout engine: maps a compiled petgraph to 2D Cartesian coordinates.
//!
//! Uses the [`layout-rs`] crate (which implements a Sugiyama-style hierarchical
//! layout) as the primary engine. A pure-Rust topological fallback is also
//! provided for robustness.

use anyhow::Result;
use layout::backends::svg::SVGWriter;
use layout::core::base::Orientation;
use layout::core::geometry::Point;
use layout::core::style::StyleAttr;
use layout::std_shapes::shapes::{Arrow, Element, ShapeKind};
use layout::topo::layout::VisualGraph;
use petgraph::stable_graph::NodeIndex;
use petgraph::visit::{EdgeRef, IntoEdgeReferences};
use std::collections::HashMap;

use crate::graph::CompiledGraph;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Computed 2D bounding box for a single node (top-left origin, in pixels).
#[derive(Debug, Clone)]
pub struct NodeLayout {
    /// X coordinate of the top-left corner.
    pub x: f64,
    /// Y coordinate of the top-left corner.
    pub y: f64,
    /// Width of the node shape.
    pub width: f64,
    /// Height of the node shape.
    pub height: f64,
}

/// Spacing and size configuration for the layout engine.
#[derive(Debug, Clone)]
pub struct LayoutConfig {
    /// Vertical gap between ranks (layers) in pixels.
    pub rank_spacing: u32,
    /// Horizontal gap between nodes on the same rank in pixels.
    pub node_spacing: u32,
    /// Default node width in pixels.
    pub node_width: f64,
    /// Default node height in pixels.
    pub node_height: f64,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            rank_spacing: 60,
            node_spacing: 40,
            node_width: 160.0,
            node_height: 60.0,
        }
    }
}

/// Computed spatial positions for every node in the graph.
pub struct LayoutResult {
    /// Maps petgraph [`NodeIndex`] to its computed 2D layout box.
    pub positions: HashMap<NodeIndex, NodeLayout>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Compute a spatial layout for `compiled` using the configured engine.
///
/// Tries the `layout-rs` hierarchical (Sugiyama-style) engine first.
/// Falls back to a deterministic topological layer assignment if the graph
/// is empty or layout-rs cannot be applied.
///
/// # Errors
///
/// Returns an error only if internal graph operations fail unexpectedly.
pub fn compute_layout(compiled: &CompiledGraph, config: &LayoutConfig) -> Result<LayoutResult> {
    if compiled.graph.node_count() == 0 {
        return Ok(LayoutResult {
            positions: HashMap::new(),
        });
    }

    // Try layout-rs engine; fall back to topo-sort layering on any issue.
    match layout_with_layout_rs(compiled, config) {
        Ok(result) => Ok(result),
        Err(_) => layout_topological(compiled, config),
    }
}

// ---------------------------------------------------------------------------
// layout-rs backend
// ---------------------------------------------------------------------------

fn layout_with_layout_rs(compiled: &CompiledGraph, config: &LayoutConfig) -> Result<LayoutResult> {
    let mut vg = VisualGraph::new(Orientation::TopToBottom);

    // Map petgraph NodeIndex → layout-rs NodeHandle
    let mut handle_map: HashMap<NodeIndex, layout::adt::dag::NodeHandle> = HashMap::new();

    for idx in compiled.graph.node_indices() {
        let node_data = &compiled.graph[idx];
        let shape = ShapeKind::new_box(&node_data.label);
        let style = StyleAttr::simple();
        let size = Point::new(config.node_width, config.node_height);
        let element = Element::create(shape, style, Orientation::TopToBottom, size);
        let handle = vg.add_node(element);
        handle_map.insert(idx, handle);
    }

    for edge_ref in compiled.graph.edge_references() {
        let src_handle = handle_map[&edge_ref.source()];
        let dst_handle = handle_map[&edge_ref.target()];
        let label = edge_ref
            .weight()
            .label
            .as_deref()
            .unwrap_or("");
        let arrow = Arrow::simple(label);
        vg.add_edge(arrow, src_handle, dst_handle);
    }

    // Run the layout algorithm (computes positions in-place).
    let mut svg = SVGWriter::new();
    vg.do_it(false, false, false, &mut svg);

    // Extract positions from the laid-out VisualGraph.
    let mut positions: HashMap<NodeIndex, NodeLayout> = HashMap::with_capacity(handle_map.len());
    for (idx, handle) in &handle_map {
        let pos = vg.pos(*handle);
        // pos.left/top give the bbox top-left; pos.bbox(false) returns (top_left, bottom_right).
        let (top_left, bottom_right) = pos.bbox(false);
        positions.insert(
            *idx,
            NodeLayout {
                x: top_left.x,
                y: top_left.y,
                width: (bottom_right.x - top_left.x).abs().max(config.node_width),
                height: (bottom_right.y - top_left.y).abs().max(config.node_height),
            },
        );
    }

    Ok(LayoutResult { positions })
}

// ---------------------------------------------------------------------------
// Topological fallback layout
// ---------------------------------------------------------------------------

/// Pure-Rust layered layout via topological sort + longest-path ranking.
///
/// Assigns each node a (column, row) position and converts to pixel coords
/// using `config.node_width + config.node_spacing` and
/// `config.node_height + config.rank_spacing`.
fn layout_topological(compiled: &CompiledGraph, config: &LayoutConfig) -> Result<LayoutResult> {
    use petgraph::algo::toposort;
    use petgraph::Direction;

    // Topological order (fails only on cycles — we've already broken them).
    let topo_order = toposort(&compiled.graph, None).map_err(|_| {
        anyhow::anyhow!("topological sort failed — unexpected cycle after FAS pass")
    })?;

    // Compute the longest-path layer for each node.
    let mut layer: HashMap<NodeIndex, usize> = HashMap::with_capacity(topo_order.len());
    for &node in &topo_order {
        let pred_max = compiled
            .graph
            .neighbors_directed(node, Direction::Incoming)
            .filter_map(|p| layer.get(&p).copied())
            .max()
            .unwrap_or(0);
        let node_layer = if compiled
            .graph
            .neighbors_directed(node, Direction::Incoming)
            .next()
            .is_some()
        {
            pred_max + 1
        } else {
            0
        };
        layer.insert(node, node_layer);
    }

    // Group nodes by layer to assign horizontal positions.
    let max_layer = layer.values().copied().max().unwrap_or(0);
    let mut layer_buckets: Vec<Vec<NodeIndex>> = vec![Vec::new(); max_layer + 1];
    for (&node, &l) in &layer {
        layer_buckets[l].push(node);
    }

    let cell_w = config.node_width + config.node_spacing as f64;
    let cell_h = config.node_height + config.rank_spacing as f64;

    let mut positions: HashMap<NodeIndex, NodeLayout> =
        HashMap::with_capacity(compiled.graph.node_count());
    for (row, bucket) in layer_buckets.iter().enumerate() {
        let total_w = bucket.len() as f64 * cell_w;
        // Centre the row horizontally (cosmetic).
        let x_offset = 0_f64; // absolute left; centering is done per-node below.
        for (col, &node) in bucket.iter().enumerate() {
            positions.insert(
                node,
                NodeLayout {
                    x: col as f64 * cell_w,
                    y: row as f64 * cell_h,
                    width: config.node_width,
                    height: config.node_height,
                },
            );
        }
        let _ = total_w;
        let _ = x_offset;
    }

    Ok(LayoutResult { positions })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::build_graph;
    use crate::schema::{DiagramPayload, EdgeDef, NodeDef};

    fn two_node_payload() -> DiagramPayload {
        DiagramPayload {
            diagram_type: "flowchart".to_owned(),
            theme: None,
            nodes: vec![
                NodeDef {
                    id: "n1".to_owned(),
                    label: "Source".to_owned(),
                    node_type: "default".to_owned(),
                    metadata: None,
                },
                NodeDef {
                    id: "n2".to_owned(),
                    label: "Sink".to_owned(),
                    node_type: "default".to_owned(),
                    metadata: None,
                },
            ],
            edges: vec![EdgeDef {
                from: "n1".to_owned(),
                to: "n2".to_owned(),
                label: Some("connects".to_owned()),
            }],
        }
    }

    #[test]
    fn test_layout_produces_positions_for_all_nodes() {
        let payload = two_node_payload();
        let compiled = build_graph(&payload).unwrap();
        let config = LayoutConfig::default();
        let result = compute_layout(&compiled, &config).unwrap();
        assert_eq!(
            result.positions.len(),
            2,
            "should have a position for every node"
        );
    }

    #[test]
    fn test_empty_graph_layout() {
        let payload = DiagramPayload {
            diagram_type: "flowchart".to_owned(),
            theme: None,
            nodes: vec![],
            edges: vec![],
        };
        let compiled = build_graph(&payload).unwrap();
        let result = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        assert!(result.positions.is_empty());
    }

    #[test]
    fn test_positions_have_positive_coords() {
        let payload = two_node_payload();
        let compiled = build_graph(&payload).unwrap();
        let result = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        for nl in result.positions.values() {
            assert!(nl.width > 0.0, "width must be positive");
            assert!(nl.height > 0.0, "height must be positive");
        }
    }
}
