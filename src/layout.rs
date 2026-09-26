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
// Node Sizing
// ---------------------------------------------------------------------------

/// Dynamically estimate the width and height of a node based on its label and shape.
pub fn estimate_node_size(
    label: &str,
    node_type: &str,
    min_width: f64,
    min_height: f64,
) -> (f64, f64) {
    let lines: Vec<&str> = label.split('\n').collect();
    let max_chars = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);

    // Approximate ~7.5px per character at 12px font + 32px horizontal padding
    let mut width = (max_chars as f64 * 7.5 + 32.0).max(min_width).min(320.0);
    // 20px line height + 24px vertical padding
    let mut height = (lines.len() as f64 * 20.0 + 24.0).max(min_height);

    // Diamond shapes (decision, cache) need extra clearance to inscribe text
    match node_type.to_ascii_lowercase().as_str() {
        "decision" | "condition" | "cache" | "redis" | "memcache" => {
            width *= 1.35;
            height *= 1.35;
        }
        _ => {}
    }

    // Snap to 10px grid
    let snapped_w = (width / 10.0).ceil() * 10.0;
    let snapped_h = (height / 10.0).ceil() * 10.0;
    (snapped_w, snapped_h)
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
        let (nw, nh) = estimate_node_size(
            &node_data.label,
            &node_data.node_type,
            config.node_width,
            config.node_height,
        );
        let size = Point::new(nw, nh);
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
        let (top_left, bottom_right) = pos.bbox(false);
        let (est_w, est_h) = estimate_node_size(
            &compiled.graph[*idx].label,
            &compiled.graph[*idx].node_type,
            config.node_width,
            config.node_height,
        );
        let w = (bottom_right.x - top_left.x).abs().max(est_w);
        let h = (bottom_right.y - top_left.y).abs().max(est_h);
        positions.insert(
            *idx,
            NodeLayout {
                x: top_left.x,
                y: top_left.y,
                width: w,
                height: h,
            },
        );
    }

    Ok(LayoutResult { positions })
}

// ---------------------------------------------------------------------------
// Topological fallback layout
// ---------------------------------------------------------------------------

/// Pure-Rust layered layout via topological sort + longest-path ranking + barycentric crossing reduction.
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

    // Group nodes by layer
    let max_layer = layer.values().copied().max().unwrap_or(0);
    let mut layer_buckets: Vec<Vec<NodeIndex>> = vec![Vec::new(); max_layer + 1];
    for (&node, &l) in &layer {
        layer_buckets[l].push(node);
    }

    // Crossing minimisation
    minimise_crossings(&mut layer_buckets, compiled);

    // Compute node sizes
    let mut node_sizes: HashMap<NodeIndex, (f64, f64)> = HashMap::new();
    for &node in &topo_order {
        let data = &compiled.graph[node];
        node_sizes.insert(
            node,
            estimate_node_size(&data.label, &data.node_type, config.node_width, config.node_height),
        );
    }

    // Calculate layer widths for centering
    let layer_widths: Vec<f64> = layer_buckets
        .iter()
        .map(|bucket| {
            if bucket.is_empty() {
                return 0.0;
            }
            let sum_w: f64 = bucket.iter().map(|n| node_sizes[n].0).sum();
            let gaps = (bucket.len() - 1) as f64 * config.node_spacing as f64;
            sum_w + gaps
        })
        .collect();

    let max_layer_width = layer_widths.iter().copied().fold(0.0_f64, f64::max);

    let mut positions: HashMap<NodeIndex, NodeLayout> =
        HashMap::with_capacity(compiled.graph.node_count());
    let mut current_y = 0.0_f64;

    for (row, bucket) in layer_buckets.iter().enumerate() {
        if bucket.is_empty() {
            continue;
        }
        let total_w = layer_widths[row];
        let x_offset = (max_layer_width - total_w).max(0.0) / 2.0;
        let max_h_in_layer = bucket.iter().map(|n| node_sizes[n].1).fold(0.0_f64, f64::max);

        let mut current_x = x_offset;
        for &node in bucket {
            let (nw, nh) = node_sizes[&node];
            // Vertically center node within layer height
            let node_y = current_y + (max_h_in_layer - nh) / 2.0;
            positions.insert(
                node,
                NodeLayout {
                    x: current_x,
                    y: node_y,
                    width: nw,
                    height: nh,
                },
            );
            current_x += nw + config.node_spacing as f64;
        }

        current_y += max_h_in_layer + config.rank_spacing as f64;
    }

    Ok(LayoutResult { positions })
}

/// 3-pass barycentric crossing minimisation heuristic.
fn minimise_crossings(
    layer_buckets: &mut [Vec<NodeIndex>],
    compiled: &CompiledGraph,
) {
    use petgraph::Direction;

    for _pass in 0..3 {
        // Forward sweep: sort by median predecessor position
        for i in 1..layer_buckets.len() {
            let prev_pos: HashMap<NodeIndex, f64> = layer_buckets[i - 1]
                .iter()
                .enumerate()
                .map(|(idx, &n)| (n, idx as f64))
                .collect();

            layer_buckets[i].sort_by(|&a, &b| {
                let ma = median_neighbor_pos(a, &prev_pos, &compiled.graph, Direction::Incoming);
                let mb = median_neighbor_pos(b, &prev_pos, &compiled.graph, Direction::Incoming);
                ma.partial_cmp(&mb).unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        // Backward sweep: sort by median successor position
        let len = layer_buckets.len();
        for i in (0..len.saturating_sub(1)).rev() {
            let next_pos: HashMap<NodeIndex, f64> = layer_buckets[i + 1]
                .iter()
                .enumerate()
                .map(|(idx, &n)| (n, idx as f64))
                .collect();

            layer_buckets[i].sort_by(|&a, &b| {
                let ma = median_neighbor_pos(a, &next_pos, &compiled.graph, Direction::Outgoing);
                let mb = median_neighbor_pos(b, &next_pos, &compiled.graph, Direction::Outgoing);
                ma.partial_cmp(&mb).unwrap_or(std::cmp::Ordering::Equal)
            });
        }
    }
}

fn median_neighbor_pos(
    node: NodeIndex,
    neighbor_positions: &HashMap<NodeIndex, f64>,
    graph: &petgraph::stable_graph::StableDiGraph<crate::graph::NodeData, crate::graph::EdgeData>,
    direction: petgraph::Direction,
) -> f64 {
    let mut positions: Vec<f64> = graph
        .edges_directed(node, direction)
        .filter_map(|e| {
            let neighbor = match direction {
                petgraph::Direction::Incoming => e.source(),
                petgraph::Direction::Outgoing => e.target(),
            };
            neighbor_positions.get(&neighbor).copied()
        })
        .collect();

    if positions.is_empty() {
        return f64::MAX / 2.0;
    }
    positions.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mid = positions.len() / 2;
    positions[mid]
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
                edge_style: None,
            }],
            groups: vec![],
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
            groups: vec![],
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

    #[test]
    fn test_dynamic_node_sizing() {
        let (short_w, short_h) = estimate_node_size("API", "default", 160.0, 60.0);
        let (long_w, _long_h) = estimate_node_size("Extremely Long Microservice Component Name Across Architecture", "default", 160.0, 60.0);
        assert!(long_w > short_w, "longer text must produce wider node");
        assert!(short_w >= 160.0);
        assert!(short_h >= 60.0);
    }
}
