//! Compound graph layout for diagrams with visual groups (containers/swimlanes).
//!
//! Each group is first laid out on its own (layers, crossing minimisation). Groups —
//! plus a borderless one-node "unit" for every ungrouped node — are then stacked in
//! rows by the flow between them, the way a person arranges tiers: a group sits one
//! row below whatever feeds it, groups that call each other both ways share a row, and
//! a group that only sends is pulled down to just above what it calls. Within a row,
//! every unit is slid horizontally toward the nodes its edges connect to, and every
//! node is slid within its group toward its own neighbours, so arrows between tiers run
//! short and straight instead of wrapping around the canvas.

use anyhow::Result;
use petgraph::stable_graph::NodeIndex;
use petgraph::visit::{EdgeRef, IntoEdgeReferences};
use std::collections::{HashMap, HashSet};

use rdg_graph::CompiledGraph;

use crate::sugiyama::minimise_crossings;
use crate::{LayoutConfig, LayoutResult, NodeLayout};

/// A row may grow to this multiple of the widest single unit before the next unit
/// wraps onto a row of its own.
const ROW_WIDTH_SLACK: f64 = 1.3;
/// Placement sweeps (units, then nodes within units) — enough for positions to settle.
const SWEEPS: usize = 4;
/// A gap widened for crossing edge labels grows to at most this many `group_gap_x`.
const MAX_LABEL_GAP_FACTOR: f64 = 5.0;

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
    /// Member nodes per local layer, left to right.
    layers: Vec<Vec<NodeIndex>>,
    /// `false` for the borderless single-node unit wrapping an ungrouped node.
    bordered: bool,
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
        layers: buckets.into_iter().filter(|b| !b.is_empty()).collect(),
        bordered: true,
    })
}


/// Padding between a unit's box and its content: the group's left and top padding, or
/// none for a lone ungrouped node.
fn unit_pad(u: &GroupInfo, config: &LayoutConfig) -> (f64, f64) {
    if u.bordered {
        (
            config.tokens.group_pad_for(u.content_w, u.content_h),
            config.tokens.group_pad_top_for(u.content_w, u.content_h),
        )
    } else {
        (0.0, 0.0)
    }
}

/// Weighted isotonic placement of boxes in a fixed left-to-right order: minimises
/// `Σ weight·(x - desired)²` subject to box `i+1` starting at least `gaps[i]` after
/// box `i` ends (pool-adjacent-violators on gap-shifted coordinates), then clamps
/// into `[lo, hi]` (on the shifted axis, when bounds are given). Returns left edges.
fn place_in_order(widths: &[f64], desired: &[f64], weights: &[f64], gaps: &[f64], bounds: Option<(f64, f64)>) -> Vec<f64> {
    let n = widths.len();
    let mut prefix = vec![0.0; n];
    for i in 1..n {
        prefix[i] = prefix[i - 1] + widths[i - 1] + gaps[i - 1];
    }
    // Blocks of (weight sum, weighted target sum, count).
    let mut blocks: Vec<(f64, f64, usize)> = Vec::with_capacity(n);
    for i in 0..n {
        let w = weights[i].max(1e-6);
        blocks.push((w, w * (desired[i] - prefix[i]), 1));
        while blocks.len() >= 2 {
            let (w1, s1, c1) = blocks[blocks.len() - 1];
            let (w0, s0, c0) = blocks[blocks.len() - 2];
            if s0 / w0 > s1 / w1 {
                blocks.pop();
                *blocks.last_mut().unwrap() = (w0 + w1, s0 + s1, c0 + c1);
            } else {
                break;
            }
        }
    }
    let mut out = Vec::with_capacity(n);
    for (w, s, c) in blocks {
        let mut y = s / w;
        if let Some((lo, hi)) = bounds {
            y = y.clamp(lo, hi.max(lo));
        }
        for _ in 0..c {
            let i = out.len();
            out.push(y + prefix[i]);
        }
    }
    out
}

/// Longest-path rank per unit over the unit graph, with mutually-reachable units
/// (strongly connected components) sharing one rank, and pure sources pulled down to
/// sit directly above the earliest unit they feed.
fn unit_ranks(n: usize, edges: &[(usize, usize)]) -> Vec<usize> {
    use petgraph::algo::tarjan_scc;
    use petgraph::graph::DiGraph;
    let mut g: DiGraph<(), ()> = DiGraph::new();
    let idx: Vec<_> = (0..n).map(|_| g.add_node(())).collect();
    for &(a, b) in edges {
        g.add_edge(idx[a], idx[b], ());
    }
    // tarjan_scc returns components in reverse topological order.
    let sccs = tarjan_scc(&g);
    let mut comp = vec![0usize; n];
    for (ci, c) in sccs.iter().enumerate() {
        for v in c {
            comp[v.index()] = ci;
        }
    }
    let nc = sccs.len();
    let mut preds: Vec<HashSet<usize>> = vec![HashSet::new(); nc];
    let mut succs: Vec<HashSet<usize>> = vec![HashSet::new(); nc];
    for &(a, b) in edges {
        let (ca, cb) = (comp[a], comp[b]);
        if ca != cb {
            preds[cb].insert(ca);
            succs[ca].insert(cb);
        }
    }
    let mut rank = vec![0usize; nc];
    for c in (0..nc).rev() {
        rank[c] = preds[c].iter().map(|&p| rank[p] + 1).max().unwrap_or(0);
    }
    for c in 0..nc {
        if preds[c].is_empty() {
            if let Some(m) = succs[c].iter().map(|&s| rank[s]).min() {
                rank[c] = m.saturating_sub(1);
            }
        }
    }
    (0..n).map(|u| rank[comp[u]]).collect()
}

/// Row-based placement for compound graphs with visual groups — see the module docs.
pub(crate) fn layout_compound(compiled: &CompiledGraph, config: &LayoutConfig) -> Result<LayoutResult> {
    let mut units: Vec<GroupInfo> = Vec::new();
    let mut unit_of: HashMap<NodeIndex, usize> = HashMap::new();
    for (g_idx, group) in compiled.groups.iter().enumerate() {
        if let Some(mut info) = compute_group_local(g_idx, group, compiled, config) {
            // A node listed in two groups stays in the first.
            info.nodes.retain(|n| !unit_of.contains_key(n));
            if info.nodes.is_empty() {
                continue;
            }
            for &n in &info.nodes {
                unit_of.insert(n, units.len());
            }
            info.idx = units.len();
            units.push(info);
        }
    }
    let mut ungrouped: Vec<NodeIndex> = compiled.graph.node_indices().filter(|n| !unit_of.contains_key(n)).collect();
    ungrouped.sort();
    for n in ungrouped {
        let (w, h) = crate::layout_node_size(compiled, n, config);
        unit_of.insert(n, units.len());
        units.push(GroupInfo {
            idx: units.len(),
            width: w,
            height: h,
            content_w: w,
            content_h: h,
            nodes: vec![n],
            local_pos: HashMap::from([(n, (0.0, 0.0))]),
            layers: vec![vec![n]],
            bordered: false,
        });
    }
    let nu = units.len();
    let sizes: HashMap<NodeIndex, (f64, f64)> =
        compiled.graph.node_indices().map(|n| (n, crate::layout_node_size(compiled, n, config))).collect();

    // Cross-unit edges, node level (for positions) and unit level (for ranks).
    let mut cross: Vec<(NodeIndex, NodeIndex)> = Vec::new();
    let mut unit_edges: Vec<(usize, usize)> = Vec::new();
    // Labelled cross-unit edges and the width their label needs (a long one-line label
    // is drawn on two, see `wrap_chars`) — a gap between side-by-side units they cross
    // must fit it beside the arrows' stubs.
    let mut labelled: Vec<(usize, usize, f64)> = Vec::new();
    let label_cw = config.tokens.char_width(config.tokens.edge_label_font_size);
    for e in compiled.graph.edge_references() {
        let (a, b) = (e.source(), e.target());
        let (ua, ub) = (unit_of[&a], unit_of[&b]);
        if let Some(label) = e.weight().label.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
            if ua != ub {
                let mut longest_line = label.lines().map(|l| l.chars().count()).max().unwrap_or(0);
                if !label.contains('\n') && longest_line > config.tokens.edge_label_wrap_chars {
                    let longest_word = label.split_whitespace().map(|w| w.chars().count()).max().unwrap_or(0);
                    longest_line = longest_line.div_ceil(2).max(longest_word) + 2;
                }
                labelled.push((ua, ub, longest_line as f64 * label_cw));
            }
        }
        if ua != ub {
            cross.push((a, b));
            unit_edges.push((ua, ub));
        }
    }
    let rank = unit_ranks(nu, &unit_edges);

    // --- Rows: one per rank, wrapped when too wide ------------------------------
    let gap_x = config.group_gap_x;
    let max_w = units.iter().map(|u| u.width).fold(0.0, f64::max);
    let area: f64 = units.iter().map(|u| u.width * u.height).sum();
    let row_budget = (max_w * ROW_WIDTH_SLACK).max((area * 1.5).sqrt());
    let max_rank = rank.iter().copied().max().unwrap_or(0);
    let mut rows: Vec<Vec<usize>> = Vec::new();
    for r in 0..=max_rank {
        let mut members: Vec<usize> = (0..nu).filter(|&u| rank[u] == r).collect();
        if members.is_empty() {
            continue;
        }
        // Keep the units most tied to the rows above; wrap the rest below them.
        let from_above = |u: usize| unit_edges.iter().filter(|&&(a, b)| b == u && rank[a] < r).count();
        members.sort_by_key(|&u| (std::cmp::Reverse(from_above(u)), u));
        let mut row: Vec<usize> = Vec::new();
        let mut w = 0.0;
        for u in members {
            let add = if row.is_empty() { units[u].width } else { gap_x + units[u].width };
            if !row.is_empty() && w + add > row_budget {
                row.sort_unstable();
                rows.push(std::mem::take(&mut row));
                w = 0.0;
                row.push(u);
                w += units[u].width;
            } else {
                row.push(u);
                w += add;
            }
        }
        row.sort_unstable();
        rows.push(row);
    }
    let mut row_of = vec![0usize; nu];
    for (ri, row) in rows.iter().enumerate() {
        for &u in row {
            row_of[u] = ri;
        }
    }

    // --- Horizontal placement ---------------------------------------------------
    let mut unit_x = vec![0.0_f64; nu];
    for row in &rows {
        let mut x = 0.0;
        for &u in row {
            unit_x[u] = x;
            x += units[u].width + gap_x;
        }
    }
    let node_cx = |n: NodeIndex, units: &[GroupInfo], unit_x: &[f64]| {
        let u = &units[unit_of[&n]];
        unit_x[u.idx] + unit_pad(u, config).0 + u.local_pos[&n].0 + sizes[&n].0 / 2.0
    };
    let node_gap = config.node_spacing as f64;
    // Beyond the label itself: a stub's clearance at each end plus the label's gap.
    let label_room = 2.0 * config.tokens.stub_clearance() + config.tokens.px(1.0);
    for sweep in 0..SWEEPS {
        // Units: down, then up; the first pass only looks at rows already settled.
        let order: Vec<usize> = if sweep % 2 == 0 { (0..rows.len()).collect() } else { (0..rows.len()).rev().collect() };
        for &ri in &order {
            let settled = |other: usize| {
                if sweep == 0 { row_of[other] < ri } else if sweep == 1 { row_of[other] > ri } else { true }
            };
            let mut desired_center: HashMap<usize, (f64, f64)> = HashMap::new();
            for &(a, b) in &cross {
                for (mine, theirs) in [(a, b), (b, a)] {
                    let (um, ut) = (unit_of[&mine], unit_of[&theirs]);
                    if row_of[um] != ri || !settled(ut) || (row_of[ut] == ri && sweep < 2) {
                        continue;
                    }
                    // Where this unit's left edge would put `mine` right above/below `theirs`.
                    let offset = node_cx(mine, &units, &unit_x) - unit_x[um];
                    let want = node_cx(theirs, &units, &unit_x) - offset;
                    let e = desired_center.entry(um).or_insert((0.0, 0.0));
                    e.0 += want;
                    e.1 += 1.0;
                }
            }
            let mut row = rows[ri].clone();
            let desired: HashMap<usize, (f64, f64)> = row
                .iter()
                .map(|&u| match desired_center.get(&u) {
                    Some(&(s, w)) => (u, (s / w, w)),
                    None => (u, (unit_x[u], 0.05)),
                })
                .collect();
            row.sort_by(|&a, &b| {
                let ca = desired[&a].0 + units[a].width / 2.0;
                let cb = desired[&b].0 + units[b].width / 2.0;
                ca.total_cmp(&cb).then(a.cmp(&b))
            });
            let widths: Vec<f64> = row.iter().map(|&u| units[u].width).collect();
            let want: Vec<f64> = row.iter().map(|&u| desired[&u].0).collect();
            let wts: Vec<f64> = row.iter().map(|&u| desired[&u].1).collect();
            let gaps: Vec<f64> = (1..row.len())
                .map(|i| {
                    let (left, right) = (&row[..i], &row[i..]);
                    labelled
                        .iter()
                        .filter(|&&(a, b, _)| (left.contains(&a) && right.contains(&b)) || (left.contains(&b) && right.contains(&a)))
                        .map(|&(_, _, w)| w + label_room)
                        .fold(gap_x, f64::max)
                        .min(gap_x * MAX_LABEL_GAP_FACTOR)
                })
                .collect();
            let xs = place_in_order(&widths, &want, &wts, &gaps, None);
            for (&u, x) in row.iter().zip(xs) {
                unit_x[u] = x;
            }
            rows[ri] = row;
        }

        // Nodes within each group layer: toward all their neighbours, order by desire.
        for ui in 0..nu {
            if !units[ui].bordered {
                continue;
            }
            let origin = unit_x[ui] + unit_pad(&units[ui], config).0;
            let content_w = units[ui].content_w;
            for li in 0..units[ui].layers.len() {
                let layer = units[ui].layers[li].clone();
                let mut desired: HashMap<NodeIndex, (f64, f64)> = HashMap::new();
                for &n in &layer {
                    let mut sum = 0.0;
                    let mut cnt = 0.0;
                    for m in compiled.graph.neighbors_undirected(n) {
                        if m == n || layer.contains(&m) {
                            continue;
                        }
                        sum += node_cx(m, &units, &unit_x) - origin - sizes[&n].0 / 2.0;
                        cnt += 1.0;
                    }
                    let cur = units[ui].local_pos[&n].0;
                    desired.insert(n, if cnt > 0.0 { (sum / cnt, cnt) } else { (cur, 0.05) });
                }
                let mut sorted = layer.clone();
                sorted.sort_by(|a, b| {
                    let ca = desired[a].0 + sizes[a].0 / 2.0;
                    let cb = desired[b].0 + sizes[b].0 / 2.0;
                    ca.total_cmp(&cb).then(a.cmp(b))
                });
                let widths: Vec<f64> = sorted.iter().map(|n| sizes[n].0).collect();
                let used: f64 = widths.iter().sum::<f64>() + node_gap * (widths.len() - 1) as f64;
                let want: Vec<f64> = sorted.iter().map(|n| desired[n].0).collect();
                let wts: Vec<f64> = sorted.iter().map(|n| desired[n].1).collect();
                let xs = place_in_order(&widths, &want, &wts, &vec![node_gap; widths.len().saturating_sub(1)], Some((0.0, content_w - used)));
                for (n, x) in sorted.iter().zip(xs) {
                    units[ui].local_pos.get_mut(n).unwrap().0 = x;
                }
                units[ui].layers[li] = sorted;
            }
        }
    }

    // --- Vertical placement: rows stacked, gaps sized for the edges crossing them ---
    let row_h: Vec<f64> = rows.iter().map(|r| r.iter().map(|&u| units[u].height).fold(0.0, f64::max)).collect();
    let mut row_y = vec![0.0_f64; rows.len()];
    for ri in 1..rows.len() {
        let crossing = cross
            .iter()
            .filter(|(a, b)| {
                let (ra, rb) = (row_of[unit_of[a]], row_of[unit_of[b]]);
                ra.min(rb) < ri && ra.max(rb) >= ri
            })
            .count();
        let bordered = rows[ri - 1].iter().chain(&rows[ri]).any(|&u| units[u].bordered);
        // Most edges crossing a row gap drop straight through it; only those that turn
        // there need a lane of their own — about half, in practice.
        let gap = crate::gaps::gap_for(crossing.div_ceil(2), config).max(if bordered { config.group_gap_y } else { 0.0 });
        row_y[ri] = row_y[ri - 1] + row_h[ri - 1] + gap;
    }
    // Within its row, a shorter unit hugs the side its edges leave from — or, when it
    // is wired to units in its own row, lines up with those.
    let mut unit_y = vec![0.0_f64; nu];
    for (ri, row) in rows.iter().enumerate() {
        for &u in row {
            let down = cross.iter().filter(|(a, b)| unit_of[a] == u && row_of[unit_of[b]] > ri || unit_of[b] == u && row_of[unit_of[a]] > ri).count();
            let up = cross.iter().filter(|(a, b)| unit_of[a] == u && row_of[unit_of[b]] < ri || unit_of[b] == u && row_of[unit_of[a]] < ri).count();
            let slack = row_h[ri] - units[u].height;
            unit_y[u] = row_y[ri] + if down >= up { slack } else { 0.0 };
        }
    }
    for (ri, row) in rows.iter().enumerate() {
        for &u in row {
            let slack = row_h[ri] - units[u].height;
            if slack <= 0.0 {
                continue;
            }
            let (_, pad_top) = unit_pad(&units[u], config);
            let mut sum = 0.0;
            let mut cnt = 0.0;
            for &(a, b) in &cross {
                for (mine, theirs) in [(a, b), (b, a)] {
                    let ut = unit_of[&theirs];
                    if unit_of[&mine] != u || row_of[ut] != ri || units[ut].height < units[u].height {
                        continue;
                    }
                    let tu = &units[ut];
                    let their_cy = unit_y[ut] + unit_pad(tu, config).1 + tu.local_pos[&theirs].1 + sizes[&theirs].1 / 2.0;
                    let my_off = pad_top + units[u].local_pos[&mine].1 + sizes[&mine].1 / 2.0;
                    sum += their_cy - my_off;
                    cnt += 1.0;
                }
            }
            if cnt > 0.0 {
                unit_y[u] = (sum / cnt).clamp(row_y[ri], row_y[ri] + slack);
            }
        }
    }

    // --- Absolute node positions --------------------------------------------------
    let min_x = unit_x.iter().copied().fold(f64::MAX, f64::min);
    let mut positions: HashMap<NodeIndex, NodeLayout> = HashMap::with_capacity(compiled.graph.node_count());
    for u in &units {
        let (px, py) = unit_pad(u, config);
        for &n in &u.nodes {
            let (lx, ly) = u.local_pos[&n];
            let (w, h) = sizes[&n];
            positions.insert(
                n,
                NodeLayout { x: config.margin_x + unit_x[u.idx] - min_x + px + lx, y: config.margin_y + unit_y[u.idx] + py + ly, width: w, height: h },
            );
        }
    }
    Ok(LayoutResult { positions, sequence_info: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn place_in_order_respects_gaps_and_pulls_toward_targets() {
        // Both want x=0: they share the overlap cost symmetrically.
        let xs = place_in_order(&[10.0, 10.0], &[0.0, 0.0], &[1.0, 1.0], &[5.0], None);
        assert_eq!(xs, vec![-7.5, 7.5]);
        // No conflict: each lands exactly where wanted.
        let xs = place_in_order(&[10.0, 10.0], &[0.0, 100.0], &[1.0, 1.0], &[5.0], None);
        assert_eq!(xs, vec![0.0, 100.0]);
        // Bounds clamp the whole run.
        let xs = place_in_order(&[10.0], &[-50.0], &[1.0], &[], Some((0.0, 20.0)));
        assert_eq!(xs, vec![0.0]);
    }

    #[test]
    fn mutually_calling_units_share_a_rank_and_sources_sit_above_targets() {
        // 0 -> 1 <-> 2 -> 3, and a source 4 that only feeds 3.
        let r = unit_ranks(5, &[(0, 1), (1, 2), (2, 1), (2, 3), (4, 3)]);
        assert_eq!(r[1], r[2]);
        assert_eq!(r[0] + 1, r[1]);
        assert_eq!(r[3], r[1] + 1);
        assert_eq!(r[4] + 1, r[3]);
    }
}
