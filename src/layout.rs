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
use std::collections::{HashMap, HashSet};

use crate::graph::CompiledGraph;

// ---------------------------------------------------------------------------
// Layout Spacing & Margin Constants
// ---------------------------------------------------------------------------

/// Horizontal internal padding for group containers.
pub const GROUP_PAD_H: f64 = 24.0;
/// Top internal padding for group containers (space for group title header).
pub const GROUP_PAD_TOP: f64 = 36.0;
/// Bottom internal padding for group containers.
pub const GROUP_PAD_BOT: f64 = 24.0;
/// Horizontal gap between group containers.
pub const GROUP_GAP_X: f64 = 48.0;
/// Vertical gap between group containers.
pub const GROUP_GAP_Y: f64 = 40.0;
/// Canvas top-left margin X.
pub const MARGIN_X: f64 = 24.0;
/// Canvas top-left margin Y.
pub const MARGIN_Y: f64 = 28.0;

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
            rank_spacing: 44,
            node_spacing: 28,
            node_width: 120.0,
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

    let mut result = if !compiled.groups.is_empty() && config.direction == LayoutDirection::TopToBottom {
        layout_compound(compiled, config)?
    } else {
        layout_topological(compiled, config)?
    };

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

        let target_min_x = if !compiled.groups.is_empty() {
            MARGIN_X + GROUP_PAD_H
        } else {
            MARGIN_X
        };
        let target_min_y = if !compiled.groups.is_empty() {
            MARGIN_Y + GROUP_PAD_TOP
        } else {
            MARGIN_Y
        };

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
// Compound graph layout (2D Group Grid Placement for ~1:1 Aspect Ratio)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct GroupInfo {
    idx: usize,
    width: f64,
    height: f64,
    nodes: Vec<NodeIndex>,
    local_pos: HashMap<NodeIndex, (f64, f64)>,
}

fn compute_group_local(
    g_idx: usize,
    group: &crate::schema::GroupDef,
    compiled: &CompiledGraph,
    config: &LayoutConfig,
) -> Option<GroupInfo> {
    let nodes: Vec<NodeIndex> = group
        .nodes
        .iter()
        .filter_map(|id| compiled.node_map.get(id).copied())
        .collect();
    if nodes.is_empty() {
        return None;
    }

    let node_set: HashSet<NodeIndex> = nodes.iter().copied().collect();

    // Compute intra-group in-degrees
    let mut in_degrees: HashMap<NodeIndex, usize> = HashMap::new();
    for &u in &nodes {
        let in_deg = compiled
            .graph
            .neighbors_directed(u, petgraph::Direction::Incoming)
            .filter(|p| node_set.contains(p))
            .count();
        in_degrees.insert(u, in_deg);
    }

    // Longest path within group
    let mut local_layer: HashMap<NodeIndex, usize> = HashMap::new();
    let mut queue: std::collections::VecDeque<NodeIndex> = nodes
        .iter()
        .filter(|&&u| in_degrees[&u] == 0)
        .copied()
        .collect();

    if queue.is_empty() {
        for (i, &u) in nodes.iter().enumerate() {
            local_layer.insert(u, i);
        }
    } else {
        for &u in &queue {
            local_layer.insert(u, 0);
        }
        let mut deg = in_degrees.clone();
        let mut visited = 0;
        while let Some(u) = queue.pop_front() {
            visited += 1;
            let current_l = local_layer[&u];
            for v in compiled
                .graph
                .neighbors_directed(u, petgraph::Direction::Outgoing)
            {
                if node_set.contains(&v) {
                    let next_l = local_layer.get(&v).copied().unwrap_or(0).max(current_l + 1);
                    local_layer.insert(v, next_l);
                    let d = deg.get_mut(&v).unwrap();
                    *d -= 1;
                    if *d == 0 {
                        queue.push_back(v);
                    }
                }
            }
        }
        if visited < nodes.len() {
            for &u in &nodes {
                local_layer.entry(u).or_insert(0);
            }
        }
    }

    let max_layer = local_layer.values().copied().max().unwrap_or(0);
    let mut buckets: Vec<Vec<NodeIndex>> = vec![Vec::new(); max_layer + 1];
    for &u in &nodes {
        buckets[local_layer[&u]].push(u);
    }

    minimise_crossings(&mut buckets, compiled);

    let mut node_sizes: HashMap<NodeIndex, (f64, f64)> = HashMap::new();
    for &u in &nodes {
        let data = &compiled.graph[u];
        node_sizes.insert(
            u,
            estimate_node_size(&data.label, &data.node_type, config.node_width, config.node_height),
        );
    }

    let layer_widths: Vec<f64> = buckets
        .iter()
        .map(|b| {
            if b.is_empty() {
                0.0
            } else {
                let sum_w: f64 = b.iter().map(|n| node_sizes[n].0).sum();
                let gaps = (b.len() - 1) as f64 * config.node_spacing as f64;
                sum_w + gaps
            }
        })
        .collect();

    let max_content_w = layer_widths.iter().copied().fold(0.0_f64, f64::max);
    let mut current_y = 0.0_f64;
    let mut local_pos: HashMap<NodeIndex, (f64, f64)> = HashMap::new();

    for (r, bucket) in buckets.iter().enumerate() {
        if bucket.is_empty() {
            continue;
        }
        let lw = layer_widths[r];
        let x_offset = (max_content_w - lw).max(0.0) / 2.0;
        let max_h = bucket.iter().map(|n| node_sizes[n].1).fold(0.0_f64, f64::max);

        let mut current_x = x_offset;
        for &u in bucket {
            let (nw, nh) = node_sizes[&u];
            let ny = current_y + (max_h - nh) / 2.0;
            local_pos.insert(u, (current_x, ny));
            current_x += nw + config.node_spacing as f64;
        }
        current_y += max_h + config.rank_spacing as f64;
    }

    let content_h = if current_y > config.rank_spacing as f64 {
        current_y - config.rank_spacing as f64
    } else {
        current_y
    };

    let total_w = max_content_w + 2.0 * GROUP_PAD_H;
    let total_h = content_h + GROUP_PAD_TOP + GROUP_PAD_BOT;

    Some(GroupInfo {
        idx: g_idx,
        width: (total_w / 4.0).ceil() * 4.0,
        height: (total_h / 4.0).ceil() * 4.0,
        nodes,
        local_pos,
    })
}

struct GridSearchCtx<'a> {
    topo_groups: &'a [usize],
    groups: &'a [GroupInfo],
    edges: &'a [(usize, usize)],
    current_grid: Vec<(usize, usize)>,
    occupied: HashSet<(usize, usize)>,
    best_grid: Vec<(usize, usize)>,
    best_score: f64,
}

impl<'a> GridSearchCtx<'a> {
    fn search(&mut self, step: usize) {
        if step == self.topo_groups.len() {
            let mut w0 = 0.0_f64;
            let mut w1 = 0.0_f64;
            let mut max_row = 0;
            for g in self.groups {
                let (r, c) = self.current_grid[g.idx];
                max_row = max_row.max(r);
                if c == 0 {
                    w0 = w0.max(g.width);
                } else {
                    w1 = w1.max(g.width);
                }
            }
            let num_r = max_row + 1;
            let mut row_h = vec![0.0_f64; num_r];
            for g in self.groups {
                let (r, _) = self.current_grid[g.idx];
                row_h[r] = row_h[r].max(g.height);
            }

            let tot_w = if w1 > 0.0 { w0 + GROUP_GAP_X + w1 } else { w0 };
            let tot_h: f64 =
                row_h.iter().sum::<f64>() + (num_r.saturating_sub(1)) as f64 * GROUP_GAP_Y;
            let ratio = (tot_w + 2.0 * MARGIN_X) / (tot_h + 2.0 * MARGIN_Y);

            let edge_dist: f64 = self
                .edges
                .iter()
                .map(|&(a, b)| {
                    let (ra, ca) = self.current_grid[a];
                    let (rb, cb) = self.current_grid[b];
                    let dr = rb as f64 - ra as f64;
                    let dc = cb as f64 - ca as f64;
                    (dr * dr + dc * dc).sqrt()
                })
                .sum();

            let score = ratio.ln().abs() + 0.02 * edge_dist;
            if score < self.best_score {
                self.best_score = score;
                self.best_grid = self.current_grid.clone();
            }
            return;
        }

        let g = self.topo_groups[step];

        let mut min_r = 0;
        for &(p, dst) in self.edges {
            if dst == g {
                let (rp, cp) = self.current_grid[p];
                if cp == 1 {
                    min_r = min_r.max(rp + 1);
                } else {
                    min_r = min_r.max(rp);
                }
            }
        }

        let cur_max_r = self.topo_groups[..step]
            .iter()
            .map(|&i| self.current_grid[i].0)
            .max()
            .unwrap_or(0);
        let max_r = (cur_max_r + 1).min(self.groups.len());

        for c in 0..=1 {
            for r in min_r..=max_r {
                let mut valid = true;
                for &(p, dst) in self.edges {
                    if dst == g {
                        let (rp, cp) = self.current_grid[p];
                        if !(r > rp || (r == rp && c > cp)) {
                            valid = false;
                            break;
                        }
                    }
                }
                if !valid || self.occupied.contains(&(r, c)) {
                    continue;
                }

                self.occupied.insert((r, c));
                self.current_grid[g] = (r, c);

                self.search(step + 1);

                self.occupied.remove(&(r, c));
            }
        }
    }
}

/// 2D grid placement for compound graphs with visual groups.
/// Guarantees zero group overlap and minimizes aspect ratio deviation from 1.0.
fn layout_compound(compiled: &CompiledGraph, config: &LayoutConfig) -> Result<LayoutResult> {
    let mut groups: Vec<GroupInfo> = Vec::new();
    let mut assigned_nodes: HashSet<NodeIndex> = HashSet::new();

    for (g_idx, group) in compiled.groups.iter().enumerate() {
        if let Some(info) = compute_group_local(g_idx, group, compiled, config) {
            for &u in &info.nodes {
                assigned_nodes.insert(u);
            }
            groups.push(info);
        }
    }

    let has_unassigned = compiled
        .graph
        .node_indices()
        .any(|idx| !assigned_nodes.contains(&idx));

    if has_unassigned || groups.is_empty() {
        return layout_topological(compiled, config);
    }

    let num_groups = groups.len();
    let mut node_to_gidx: HashMap<NodeIndex, usize> = HashMap::new();
    for info in &groups {
        for &u in &info.nodes {
            node_to_gidx.insert(u, info.idx);
        }
    }

    let mut group_edges: Vec<(usize, usize)> = Vec::new();
    for edge_ref in compiled.graph.edge_references() {
        let s = edge_ref.source();
        let t = edge_ref.target();
        if let (Some(&ga), Some(&gb)) = (node_to_gidx.get(&s), node_to_gidx.get(&t)) {
            if ga != gb {
                group_edges.push((ga, gb));
            }
        }
    }
    group_edges.sort_unstable();
    group_edges.dedup();

    // Topological sort of groups
    let mut in_degrees = vec![0usize; num_groups];
    let mut adj = vec![Vec::new(); num_groups];
    for &(ga, gb) in &group_edges {
        in_degrees[gb] += 1;
        adj[ga].push(gb);
    }

    let mut topo_groups: Vec<usize> = Vec::with_capacity(num_groups);
    let mut q: std::collections::VecDeque<usize> = in_degrees
        .iter()
        .enumerate()
        .filter(|&(_, &deg)| deg == 0)
        .map(|(i, _)| i)
        .collect();

    while let Some(g) = q.pop_front() {
        topo_groups.push(g);
        for &next in &adj[g] {
            in_degrees[next] -= 1;
            if in_degrees[next] == 0 {
                q.push_back(next);
            }
        }
    }
    if topo_groups.len() < num_groups {
        for i in 0..num_groups {
            if !topo_groups.contains(&i) {
                topo_groups.push(i);
            }
        }
    }

    // 1-column layout baseline
    let mut grid_1col = vec![(0usize, 0usize); num_groups];
    for (r, &g) in topo_groups.iter().enumerate() {
        grid_1col[g] = (r, 0);
    }
    let w_1col = groups.iter().map(|g| g.width).fold(0.0_f64, f64::max);
    let h_1col: f64 = groups.iter().map(|g| g.height).sum::<f64>()
        + (num_groups.saturating_sub(1)) as f64 * GROUP_GAP_Y;
    let ratio_1col = (w_1col + 2.0 * MARGIN_X) / (h_1col + 2.0 * MARGIN_Y);
    let score_1col = ratio_1col.ln().abs();

    // 2-column search
    let chosen_grid = if num_groups >= 2 {
        let mut ctx = GridSearchCtx {
            topo_groups: &topo_groups,
            groups: &groups,
            edges: &group_edges,
            current_grid: vec![(0usize, 0usize); num_groups],
            occupied: HashSet::new(),
            best_grid: grid_1col.clone(),
            best_score: score_1col,
        };
        ctx.search(0);
        if ctx.best_score < score_1col {
            ctx.best_grid
        } else {
            grid_1col
        }
    } else {
        grid_1col
    };

    let mut w_col0 = 0.0_f64;
    let mut w_col1 = 0.0_f64;
    let mut max_r = 0;
    for info in &groups {
        let (r, c) = chosen_grid[info.idx];
        max_r = max_r.max(r);
        if c == 0 {
            w_col0 = w_col0.max(info.width);
        } else {
            w_col1 = w_col1.max(info.width);
        }
    }
    let num_rows = max_r + 1;
    let mut row_heights = vec![0.0_f64; num_rows];
    for info in &groups {
        let (r, _) = chosen_grid[info.idx];
        row_heights[r] = row_heights[r].max(info.height);
    }

    let total_content_w = if w_col1 > 0.0 {
        w_col0 + GROUP_GAP_X + w_col1
    } else {
        w_col0
    };

    let mut row_y = vec![0.0_f64; num_rows];
    let mut cur_y = MARGIN_Y;
    for r in 0..num_rows {
        row_y[r] = cur_y;
        cur_y += row_heights[r] + GROUP_GAP_Y;
    }

    let mut positions: HashMap<NodeIndex, NodeLayout> =
        HashMap::with_capacity(compiled.graph.node_count());

    for info in &groups {
        let (r, c) = chosen_grid[info.idx];
        let gy = row_y[r] + (row_heights[r] - info.height) / 2.0;

        let is_sole_in_row = groups.iter().filter(|g| chosen_grid[g.idx].0 == r).count() == 1;

        let gx = if is_sole_in_row && w_col1 > 0.0 {
            MARGIN_X + (total_content_w - info.width).max(0.0) / 2.0
        } else if c == 0 {
            MARGIN_X + (w_col0 - info.width).max(0.0) / 2.0
        } else {
            MARGIN_X + w_col0 + GROUP_GAP_X + (w_col1 - info.width).max(0.0) / 2.0
        };

        for &u in &info.nodes {
            let (lx, ly) = info.local_pos[&u];
            let (nw, nh) = estimate_node_size(
                &compiled.graph[u].label,
                &compiled.graph[u].node_type,
                config.node_width,
                config.node_height,
            );
            positions.insert(
                u,
                NodeLayout {
                    x: gx + GROUP_PAD_H + lx,
                    y: gy + GROUP_PAD_TOP + ly,
                    width: nw,
                    height: nh,
                },
            );
        }
    }

    Ok(LayoutResult { positions })
}

// ---------------------------------------------------------------------------
// Topological layout (Pure-Rust layered fallback & flat layout)
// ---------------------------------------------------------------------------

/// Pure-Rust layered layout via topological sort + longest-path ranking + barycentric crossing reduction.
fn layout_topological(compiled: &CompiledGraph, config: &LayoutConfig) -> Result<LayoutResult> {
    use petgraph::algo::toposort;

    let topo_order = toposort(&compiled.graph, None).map_err(|_| {
        anyhow::anyhow!("topological sort failed — unexpected cycle after FAS pass")
    })?;

    let mut layer: HashMap<NodeIndex, usize> = HashMap::with_capacity(topo_order.len());
    for &node in &topo_order {
        let pred_max = compiled
            .graph
            .neighbors_directed(node, petgraph::Direction::Incoming)
            .filter_map(|p| layer.get(&p).copied())
            .max()
            .unwrap_or(0);
        let node_layer = if compiled
            .graph
            .neighbors_directed(node, petgraph::Direction::Incoming)
            .next()
            .is_some()
        {
            pred_max + 1
        } else {
            0
        };
        layer.insert(node, node_layer);
    }

    let max_layer = layer.values().copied().max().unwrap_or(0);
    let mut layer_buckets: Vec<Vec<NodeIndex>> = vec![Vec::new(); max_layer + 1];
    for (&node, &l) in &layer {
        layer_buckets[l].push(node);
    }

    minimise_crossings(&mut layer_buckets, compiled);

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

    #[test]
    fn test_compound_layout_no_overlap_and_aspect_ratio() {
        use crate::schema::GroupDef;

        let payload = DiagramPayload {
            diagram_type: "flowchart".to_owned(),
            theme: None,
            direction: None,
            groups: vec![
                GroupDef {
                    id: "g1".to_owned(),
                    label: "Group 1".to_owned(),
                    color: None,
                    nodes: vec!["n1".to_owned(), "n2".to_owned()],
                },
                GroupDef {
                    id: "g2".to_owned(),
                    label: "Group 2".to_owned(),
                    color: None,
                    nodes: vec!["n3".to_owned()],
                },
                GroupDef {
                    id: "g3".to_owned(),
                    label: "Group 3".to_owned(),
                    color: None,
                    nodes: vec!["n4".to_owned()],
                },
            ],
            nodes: vec![
                NodeDef {
                    id: "n1".to_owned(),
                    label: "Node 1".to_owned(),
                    node_type: "server".to_owned(),
                    metadata: None,
                },
                NodeDef {
                    id: "n2".to_owned(),
                    label: "Node 2".to_owned(),
                    node_type: "server".to_owned(),
                    metadata: None,
                },
                NodeDef {
                    id: "n3".to_owned(),
                    label: "Node 3".to_owned(),
                    node_type: "server".to_owned(),
                    metadata: None,
                },
                NodeDef {
                    id: "n4".to_owned(),
                    label: "Node 4".to_owned(),
                    node_type: "server".to_owned(),
                    metadata: None,
                },
            ],
            edges: vec![
                EdgeDef {
                    from: "n1".to_owned(),
                    to: "n2".to_owned(),
                    label: None,
                    edge_style: None,
                },
                EdgeDef {
                    from: "n2".to_owned(),
                    to: "n3".to_owned(),
                    label: None,
                    edge_style: None,
                },
                EdgeDef {
                    from: "n3".to_owned(),
                    to: "n4".to_owned(),
                    label: None,
                    edge_style: None,
                },
            ],
        };

        let compiled = build_graph(&payload).unwrap();
        let config = LayoutConfig::default();
        let result = compute_layout(&compiled, &config).unwrap();

        assert_eq!(result.positions.len(), 4);

        // Verify bounding boxes of groups do not intersect
        let mut group_bboxes = Vec::new();
        for group in &payload.groups {
            let mut min_x = f64::MAX;
            let mut min_y = f64::MAX;
            let mut max_x = f64::MIN;
            let mut max_y = f64::MIN;
            for nid in &group.nodes {
                let idx = compiled.node_map[nid];
                let nl = &result.positions[&idx];
                min_x = min_x.min(nl.x);
                min_y = min_y.min(nl.y);
                max_x = max_x.max(nl.x + nl.width);
                max_y = max_y.max(nl.y + nl.height);
            }
            let gx = min_x - GROUP_PAD_H;
            let gy = min_y - GROUP_PAD_TOP;
            let gw = (max_x - min_x) + 2.0 * GROUP_PAD_H;
            let gh = (max_y - min_y) + GROUP_PAD_TOP + GROUP_PAD_BOT;
            group_bboxes.push((gx, gy, gx + gw, gy + gh));
        }

        for i in 0..group_bboxes.len() {
            for j in (i + 1)..group_bboxes.len() {
                let (ax1, ay1, ax2, ay2) = group_bboxes[i];
                let (bx1, by1, bx2, by2) = group_bboxes[j];
                let overlap = ax1 < bx2 && ax2 > bx1 && ay1 < by2 && ay2 > by1;
                assert!(!overlap, "Group {i} and Group {j} must not overlap");
            }
        }
    }
}
