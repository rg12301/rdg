//! Compound graph layout: 2D group grid placement for visual (group/swimlane) containers,
//! targeting a canvas aspect ratio close to 1:1.

use anyhow::Result;
use petgraph::stable_graph::NodeIndex;
use petgraph::visit::{EdgeRef, IntoEdgeReferences};
use std::collections::{HashMap, HashSet};

use rdg_graph::CompiledGraph;

use crate::sugiyama::minimise_crossings;
use crate::{LayoutConfig, LayoutResult, NodeLayout};

/// Weight applied to inter-group edge "distance" (grid Euclidean distance) in the
/// grid-search score, relative to the `ln(aspect ratio)` term. Kept small and positive
/// so aspect ratio dominates the choice between candidate grids, and edge distance only
/// acts as a tie-breaker that favors keeping connected groups close together.
const EDGE_DISTANCE_WEIGHT: f64 = 0.02;

#[derive(Debug, Clone)]
struct GroupInfo {
    idx: usize,
    width: f64,
    height: f64,
    /// Member nodes' combined bounding box *before* padding — kept alongside the
    /// already-padded `width`/`height` so the per-node placement pass below can
    /// re-derive the same content-aware padding `compute_group_local` used, rather
    /// than a mismatched flat one.
    content_w: f64,
    content_h: f64,
    nodes: Vec<NodeIndex>,
    local_pos: HashMap<NodeIndex, (f64, f64)>,
}

fn compute_group_local(
    g_idx: usize,
    group: &rdg_schema::GroupDef,
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
        node_sizes.insert(u, crate::layout_node_size(compiled, u, config));
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
    let gaps = crate::gaps::rank_gaps(compiled, &local_layer, config);
    let gap_after = |rank: usize| gaps.get(rank).copied().unwrap_or(config.rank_spacing as f64);
    let mut current_y = 0.0_f64;
    let mut last_gap = 0.0_f64;
    let mut local_pos: HashMap<NodeIndex, (f64, f64)> = HashMap::new();

    for (r, bucket) in buckets.iter().enumerate() {
        if bucket.is_empty() {
            continue;
        }
        let lw = layer_widths[r];
        let x_offset = (max_content_w - lw).max(0.0) / 2.0;
        let max_h = bucket
            .iter()
            .map(|n| node_sizes[n].1)
            .fold(0.0_f64, f64::max);

        let mut current_x = x_offset;
        for &u in bucket {
            let (nw, nh) = node_sizes[&u];
            let ny = current_y + (max_h - nh) / 2.0;
            local_pos.insert(u, (current_x, ny));
            current_x += nw + config.node_spacing as f64;
        }
        last_gap = gap_after(r);
        current_y += max_h + last_gap;
    }

    let content_h = if current_y > last_gap { current_y - last_gap } else { current_y };

    let total_w = max_content_w + 2.0 * config.tokens.group_pad_for(max_content_w, content_h);
    let total_h = content_h
        + config.tokens.group_pad_top_for(max_content_w, content_h)
        + config.tokens.group_pad_for(max_content_w, content_h);

    Some(GroupInfo {
        idx: g_idx,
        width: (total_w / 4.0).ceil() * 4.0,
        height: (total_h / 4.0).ceil() * 4.0,
        content_w: max_content_w,
        content_h,
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
    margin_x: f64,
    margin_y: f64,
    group_gap_x: f64,
    group_gap_y: f64,
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

            let tot_w = if w1 > 0.0 {
                w0 + self.group_gap_x + w1
            } else {
                w0
            };
            let tot_h: f64 =
                row_h.iter().sum::<f64>() + (num_r.saturating_sub(1)) as f64 * self.group_gap_y;
            let ratio = (tot_w + 2.0 * self.margin_x) / (tot_h + 2.0 * self.margin_y);

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

            let score = ratio.ln().abs() + EDGE_DISTANCE_WEIGHT * edge_dist;
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
pub(crate) fn layout_compound(
    compiled: &CompiledGraph,
    config: &LayoutConfig,
) -> Result<LayoutResult> {
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
        return crate::sugiyama::layout_topological(compiled, config);
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
        + (num_groups.saturating_sub(1)) as f64 * config.group_gap_y;
    let ratio_1col = (w_1col + 2.0 * config.margin_x) / (h_1col + 2.0 * config.margin_y);
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
            margin_x: config.margin_x,
            margin_y: config.margin_y,
            group_gap_x: config.group_gap_x,
            group_gap_y: config.group_gap_y,
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
        w_col0 + config.group_gap_x + w_col1
    } else {
        w_col0
    };

    let mut row_y = vec![0.0_f64; num_rows];
    let mut cur_y = config.margin_y;
    for r in 0..num_rows {
        row_y[r] = cur_y;
        cur_y += row_heights[r] + config.group_gap_y;
    }

    let mut positions: HashMap<NodeIndex, NodeLayout> =
        HashMap::with_capacity(compiled.graph.node_count());

    for info in &groups {
        let (r, c) = chosen_grid[info.idx];
        let gy = row_y[r] + (row_heights[r] - info.height) / 2.0;

        let is_sole_in_row = groups.iter().filter(|g| chosen_grid[g.idx].0 == r).count() == 1;

        let gx = if is_sole_in_row && w_col1 > 0.0 {
            config.margin_x + (total_content_w - info.width).max(0.0) / 2.0
        } else if c == 0 {
            config.margin_x + (w_col0 - info.width).max(0.0) / 2.0
        } else {
            config.margin_x + w_col0 + config.group_gap_x + (w_col1 - info.width).max(0.0) / 2.0
        };

        for &u in &info.nodes {
            let (lx, ly) = info.local_pos[&u];
            let (nw, nh) = crate::layout_node_size(compiled, u, config);
            positions.insert(
                u,
                NodeLayout {
                    x: gx + config.tokens.group_pad_for(info.content_w, info.content_h) + lx,
                    y: gy + config.tokens.group_pad_top_for(info.content_w, info.content_h) + ly,
                    width: nw,
                    height: nh,
                },
            );
        }
    }

    Ok(LayoutResult {
        positions,
        sequence_info: None,
    })
}
