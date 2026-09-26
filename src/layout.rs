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

/// Overall flow direction of the diagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayoutDirection {
    /// Top to Bottom (hierarchical DAG standard).
    #[default]
    TopToBottom,
    /// Left to Right (horizontal pipelines, sequence flows).
    LeftToRight,
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
    /// Flow direction of the diagram.
    pub direction: LayoutDirection,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            rank_spacing: 36,
            node_spacing: 20,
            node_width: 110.0,
            node_height: 44.0,
            direction: LayoutDirection::TopToBottom,
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
/// Uses our deterministic layered layout engine with group awareness,
/// barycentric crossing minimization, and compact spacing.
/// Automatically normalizes coordinates to eliminate canvas whitespace wastage.
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

    // Run deterministic layered layout engine with group-aware stage ordering
    let mut result = layout_topological(compiled, config)?;

    // Normalize coordinates so the diagram starts cleanly near the top-left margin
    // without wasting huge canvas areas.
    if !result.positions.is_empty() {
        let min_x = result
            .positions
            .values()
            .map(|nl| nl.x)
            .fold(f64::MAX, f64::min);
        let min_y = result
            .positions
            .values()
            .map(|nl| nl.y)
            .fold(f64::MAX, f64::min);

        let target_min_x = 24.0_f64;
        let target_min_y = 28.0_f64;
        let dx = target_min_x - min_x;
        let dy = target_min_y - min_y;

        for nl in result.positions.values_mut() {
            nl.x += dx;
            nl.y += dy;
        }
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// Node Sizing & Text Wrapping
// ---------------------------------------------------------------------------

/// Wrap label into lines, respecting existing newlines and breaking on word boundaries.
/// Prevents orphan closing delimiters/brackets (like single `}`) from landing alone on a line.
pub fn wrap_label(label: &str, max_chars_per_line: usize) -> Vec<String> {
    let mut result = Vec::new();
    for raw_line in label.split('\n') {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.chars().count() <= max_chars_per_line {
            result.push(trimmed.to_string());
            continue;
        }

        let words: Vec<&str> = trimmed.split_whitespace().collect();
        if words.is_empty() {
            continue;
        }

        let mut current_line = String::new();
        for word in words {
            let is_closing = word
                .chars()
                .all(|c| matches!(c, '}' | ')' | ']' | '>' | ';' | ',' | '.' | ':'));

            if current_line.is_empty() {
                current_line.push_str(word);
            } else if is_closing
                || current_line.chars().count() + 1 + word.chars().count()
                    <= max_chars_per_line + if is_closing { 3 } else { 0 }
            {
                current_line.push(' ');
                current_line.push_str(word);
            } else {
                result.push(current_line);
                current_line = word.to_string();
            }
        }
        if !current_line.is_empty() {
            // Fold orphan single bracket/punctuation back into the preceding line
            let is_orphan = current_line
                .trim()
                .chars()
                .all(|c| matches!(c, '}' | ')' | ']' | '>' | ';' | ',' | '.' | ':'));
            if is_orphan && !result.is_empty() {
                let last = result.last_mut().unwrap();
                last.push(' ');
                last.push_str(current_line.trim());
            } else {
                result.push(current_line);
            }
        }
    }
    if result.is_empty() {
        vec![label.to_string()]
    } else {
        result
    }
}

/// Dynamically estimate the width and height of a node based on its label and shape.
pub fn estimate_node_size(
    label: &str,
    node_type: &str,
    min_width: f64,
    min_height: f64,
) -> (f64, f64) {
    let lines = wrap_label(label, 22);
    let max_chars = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);

    // Approximate ~7.0px per character at 12px font + 20px horizontal padding
    let mut width = (max_chars as f64 * 7.0 + 20.0).max(min_width).min(240.0);
    // Compact line heights: 16px title, 13px subtitles + 14px vertical padding
    let total_h = if lines.len() <= 1 {
        16.0 + 14.0
    } else {
        16.0 + (lines.len() - 1) as f64 * 13.0 + 14.0
    };
    let mut height = total_h.max(min_height);

    // Diamond shapes (decision, cache) need extra clearance to inscribe text
    match node_type.to_ascii_lowercase().as_str() {
        "decision" | "condition" | "cache" | "redis" | "memcache" => {
            width *= 1.30;
            height *= 1.30;
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

#[allow(dead_code)]
fn layout_with_layout_rs(compiled: &CompiledGraph, config: &LayoutConfig) -> Result<LayoutResult> {
    let orientation = match config.direction {
        LayoutDirection::TopToBottom => Orientation::TopToBottom,
        LayoutDirection::LeftToRight => Orientation::LeftToRight,
    };
    let mut vg = VisualGraph::new(orientation);

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
        let element = Element::create(shape, style, orientation, size);
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

    // Compute layers (group-aware stage assignment when groups are defined)
    let mut layer: HashMap<NodeIndex, usize> = HashMap::with_capacity(topo_order.len());

    let has_groups = !compiled.groups.is_empty();
    if has_groups {
        let mut node_to_group: HashMap<NodeIndex, usize> = HashMap::new();
        for (g_idx, group) in compiled.groups.iter().enumerate() {
            for node_id in &group.nodes {
                if let Some(&idx) = compiled.node_map.get(node_id) {
                    node_to_group.insert(idx, g_idx);
                }
            }
        }

        // Compute intra-group local layers so that nodes inside each group form a compact hierarchy
        let mut group_local_layer: HashMap<NodeIndex, usize> = HashMap::new();
        let mut group_depths: Vec<usize> = vec![0; compiled.groups.len()];

        for (g_idx, _group) in compiled.groups.iter().enumerate() {
            let mut max_local = 0;
            for &node in &topo_order {
                if node_to_group.get(&node) == Some(&g_idx) {
                    let local_pred_max = compiled
                        .graph
                        .neighbors_directed(node, Direction::Incoming)
                        .filter(|p| node_to_group.get(p) == Some(&g_idx))
                        .filter_map(|p| group_local_layer.get(&p).copied())
                        .max();
                    let l = match local_pred_max {
                        Some(m) => m + 1,
                        None => 0,
                    };
                    group_local_layer.insert(node, l);
                    max_local = max_local.max(l + 1);
                }
            }
            group_depths[g_idx] = max_local.max(1);
        }

        // Compute global base layer for each group so groups stack sequentially without overlap
        let mut group_base: Vec<usize> = Vec::with_capacity(compiled.groups.len());
        let mut acc = 0;
        for &d in &group_depths {
            group_base.push(acc);
            acc += d;
        }

        for &node in &topo_order {
            if let Some(&g_idx) = node_to_group.get(&node) {
                let local = group_local_layer.get(&node).copied().unwrap_or(0);
                layer.insert(node, group_base[g_idx] + local);
            } else {
                let pred_max = compiled
                    .graph
                    .neighbors_directed(node, Direction::Incoming)
                    .filter_map(|p| layer.get(&p).copied())
                    .max()
                    .unwrap_or(0);
                let l = if compiled.graph.neighbors_directed(node, Direction::Incoming).next().is_some() {
                    pred_max + 1
                } else {
                    0
                };
                layer.insert(node, l);
            }
        }
    } else {
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

    let mut positions: HashMap<NodeIndex, NodeLayout> =
        HashMap::with_capacity(compiled.graph.node_count());

    match config.direction {
        LayoutDirection::TopToBottom => {
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
        }
        LayoutDirection::LeftToRight => {
            let layer_heights: Vec<f64> = layer_buckets
                .iter()
                .map(|bucket| {
                    if bucket.is_empty() {
                        return 0.0;
                    }
                    let sum_h: f64 = bucket.iter().map(|n| node_sizes[n].1).sum();
                    let gaps = (bucket.len() - 1) as f64 * config.node_spacing as f64;
                    sum_h + gaps
                })
                .collect();

            let max_layer_height = layer_heights.iter().copied().fold(0.0_f64, f64::max);
            let mut current_x = 0.0_f64;

            for (col, bucket) in layer_buckets.iter().enumerate() {
                if bucket.is_empty() {
                    continue;
                }
                let total_h = layer_heights[col];
                let y_offset = (max_layer_height - total_h).max(0.0) / 2.0;
                let max_w_in_col = bucket.iter().map(|n| node_sizes[n].0).fold(0.0_f64, f64::max);

                let mut current_y = y_offset;
                for &node in bucket {
                    let (nw, nh) = node_sizes[&node];
                    let node_x = current_x + (max_w_in_col - nw) / 2.0;
                    positions.insert(
                        node,
                        NodeLayout {
                            x: node_x,
                            y: current_y,
                            width: nw,
                            height: nh,
                        },
                    );
                    current_y += nh + config.node_spacing as f64;
                }

                current_x += max_w_in_col + config.rank_spacing as f64;
            }
        }
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
            direction: None,
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
            direction: None,
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
        let (short_w, short_h) = estimate_node_size("API", "default", 120.0, 50.0);
        let (long_w, long_h) = estimate_node_size(
            "Extremely Long Microservice Component Name Across Architecture",
            "default",
            120.0,
            50.0,
        );
        assert!(long_w > short_w, "longer text must produce wider node");
        assert!(long_h > short_h, "multiline wrapped text must produce taller node");
        assert!(short_w >= 120.0);
        assert!(short_h >= 50.0);
    }

    #[test]
    fn test_left_to_right_layout() {
        let payload = two_node_payload();
        let compiled = build_graph(&payload).unwrap();
        let config = LayoutConfig {
            direction: LayoutDirection::LeftToRight,
            ..LayoutConfig::default()
        };
        let result = compute_layout(&compiled, &config).unwrap();
        let n1_pos = &result.positions[&compiled.node_map["n1"]];
        let n2_pos = &result.positions[&compiled.node_map["n2"]];
        assert!(n2_pos.x > n1_pos.x, "target node should be placed to the right of source");
    }
}
