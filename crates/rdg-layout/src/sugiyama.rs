//! Pure-Rust layered layout: topological sort + longest-path ranking + barycentric
//! crossing reduction + chain straightening.

use anyhow::Result;
use petgraph::stable_graph::NodeIndex;
use petgraph::visit::EdgeRef;
use std::collections::HashMap;

use rdg_graph::CompiledGraph;

use crate::{LayoutConfig, LayoutDirection, LayoutResult, NodeLayout};

/// Pure-Rust layered layout via topological sort + longest-path ranking + barycentric crossing reduction.
pub(crate) fn layout_topological(
    compiled: &CompiledGraph,
    config: &LayoutConfig,
) -> Result<LayoutResult> {
    use petgraph::algo::toposort;

    let topo_order = toposort(&compiled.graph, None).map_err(|_| {
        anyhow::anyhow!("topological sort failed — unexpected cycle after FAS pass")
    })?;

    // Longest path over the ranking edges (`constraint: false` edges are drawn but
    // never push their target down a rank).
    let ranked_preds = |node: NodeIndex| {
        compiled.graph.edges_directed(node, petgraph::Direction::Incoming).filter(|e| e.weight().constraint).map(|e| e.source())
    };
    // A dense web (more than `density_threshold` edges per node) laid out by longest
    // path becomes one long chain with most arrows looping back up it — nearly every
    // node has a predecessor one rank up. Such a graph has no real hierarchy to show,
    // so its nodes fill a compact block instead: about √n per rank, in topological
    // (that is, declaration) order, and the arrows go where they go.
    let n = compiled.graph.node_count();
    let density = compiled.graph.edge_count() as f64 / n.max(1) as f64;
    let per_rank = if density > config.tokens.density_threshold { Some(((n as f64 * 1.3).sqrt().ceil() as usize).max(2)) } else { None };
    let mut layer: HashMap<NodeIndex, usize> = HashMap::with_capacity(topo_order.len());
    for (i, &node) in topo_order.iter().enumerate() {
        let node_layer = match per_rank {
            Some(w) => i / w,
            None => ranked_preds(node).filter_map(|p| layer.get(&p).copied()).max().map_or(0, |m| m + 1),
        };
        layer.insert(node, node_layer);
    }

    // A pure source sits directly above the first node it feeds (not at the very top,
    // which strands a lookup table several ranks away from the one table that uses it).
    if per_rank.is_none() {
        let sources: Vec<NodeIndex> = topo_order.iter().copied().filter(|&n| ranked_preds(n).next().is_none()).collect();
        for n in sources {
            if let Some(first) = compiled.graph.neighbors_directed(n, petgraph::Direction::Outgoing).filter_map(|s| layer.get(&s).copied()).min() {
                layer.insert(n, first.saturating_sub(1));
            }
        }
    }
    apply_rank_hints(compiled, &mut layer, &topo_order);

    let max_layer = layer.values().copied().max().unwrap_or(0);
    let mut layer_buckets: Vec<Vec<NodeIndex>> = vec![Vec::new(); max_layer + 1];
    // Iterate `topo_order` (a `Vec`, insertion-order-stable), not `&layer` — `layer` is a
    // `HashMap`, whose iteration order is randomized per process (Rust's default hasher
    // seeds itself from the OS on each run). Two nodes landing in the same layer with no
    // barycentric neighbors on one side tie in `minimise_crossings`' sort, so their
    // relative order there falls back to whatever order they were pushed into the bucket
    // here — meaning the same YAML could silently lay out differently between runs, purely
    // from hash-seed randomness, never from anything in the diagram itself. Confirmed
    // directly: repeated runs of the same command on the same class-diagram sample produced
    // different edge routing (and different resulting draw.io corner-rounding artifacts).
    for &node in &topo_order {
        let l = layer[&node];
        layer_buckets[l].push(node);
    }

    minimise_crossings(&mut layer_buckets, compiled);

    let mut node_sizes: HashMap<NodeIndex, (f64, f64)> = HashMap::new();
    for &node in &topo_order {
        node_sizes.insert(node, crate::layout_node_size(compiled, node, config));
    }

    let mut positions: HashMap<NodeIndex, NodeLayout> =
        HashMap::with_capacity(compiled.graph.node_count());

    let gaps = crate::gaps::rank_gaps(compiled, &layer, config);
    let gap_after = |rank: usize| gaps.get(rank).copied().unwrap_or(config.rank_spacing as f64);

    let axis = match config.direction {
        LayoutDirection::TopToBottom => Axis::X,
        LayoutDirection::LeftToRight => Axis::Y,
    };

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
                let max_h_in_layer = bucket
                    .iter()
                    .map(|n| node_sizes[n].1)
                    .fold(0.0_f64, f64::max);

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

                current_y += max_h_in_layer + gap_after(row);
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
                let max_w_in_col = bucket
                    .iter()
                    .map(|n| node_sizes[n].0)
                    .fold(0.0_f64, f64::max);

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

                current_x += max_w_in_col + gap_after(col);
            }
        }
    }

    straighten_layers(
        &mut positions,
        &layer_buckets,
        compiled,
        config.node_spacing as f64,
        axis,
    );

    Ok(LayoutResult { positions, layered: Some(config.direction), ..Default::default() })
}

/// 3-pass median crossing minimisation over the compiled graph's nodes.
pub(crate) fn minimise_crossings(layer_buckets: &mut [Vec<NodeIndex>], compiled: &CompiledGraph) {
    let edges: Vec<(NodeIndex, NodeIndex)> = compiled.graph.edge_indices().filter_map(|e| compiled.graph.edge_endpoints(e)).collect();
    order_layers(layer_buckets, &edges);
}

/// 3-pass median crossing minimisation: alternately sorts each layer by the median
/// position of its neighbours in the layer above (forward) and below (backward).
/// Items with no neighbour there keep their relative order (stable sort) at the end.
pub(crate) fn order_layers<T: Copy + Eq + std::hash::Hash>(layer_buckets: &mut [Vec<T>], edges: &[(T, T)]) {
    let mut preds: HashMap<T, Vec<T>> = HashMap::new();
    let mut succs: HashMap<T, Vec<T>> = HashMap::new();
    for &(a, b) in edges {
        preds.entry(b).or_default().push(a);
        succs.entry(a).or_default().push(b);
    }
    let median = |item: T, positions: &HashMap<T, f64>, incoming: bool| -> f64 {
        let neighbours = if incoming { preds.get(&item) } else { succs.get(&item) };
        let mut ps: Vec<f64> = neighbours.into_iter().flatten().filter_map(|n| positions.get(n).copied()).collect();
        if ps.is_empty() {
            return f64::MAX / 2.0;
        }
        // `total_cmp` keeps this panic-free even on a NaN/Inf coordinate.
        ps.sort_by(f64::total_cmp);
        ps[ps.len() / 2]
    };
    let index_of = |layer: &[T]| -> HashMap<T, f64> { layer.iter().enumerate().map(|(i, &n)| (n, i as f64)).collect() };
    for _pass in 0..3 {
        for i in 1..layer_buckets.len() {
            let prev = index_of(&layer_buckets[i - 1]);
            layer_buckets[i].sort_by(|&a, &b| median(a, &prev, true).total_cmp(&median(b, &prev, true)));
        }
        for i in (0..layer_buckets.len().saturating_sub(1)).rev() {
            let next = index_of(&layer_buckets[i + 1]);
            layer_buckets[i].sort_by(|&a, &b| median(a, &next, false).total_cmp(&median(b, &next, false)));
        }
    }
    transpose(layer_buckets, &preds, &succs);
}

/// Crossings between two adjacent layers, given each layer's order and the edges
/// between them (as neighbour lists on the lower layer's items).
fn crossings_between<T: Copy + Eq + std::hash::Hash>(upper: &[T], lower: &[T], preds: &HashMap<T, Vec<T>>) -> usize {
    let pos: HashMap<T, usize> = upper.iter().enumerate().map(|(i, &n)| (n, i)).collect();
    let mut ends: Vec<(usize, usize)> = Vec::new();
    for (j, item) in lower.iter().enumerate() {
        for p in preds.get(item).into_iter().flatten() {
            if let Some(&i) = pos.get(p) {
                ends.push((j, i));
            }
        }
    }
    let mut count = 0;
    for a in 0..ends.len() {
        for b in a + 1..ends.len() {
            let ((j1, i1), (j2, i2)) = (ends[a], ends[b]);
            if (j1 < j2 && i1 > i2) || (j1 > j2 && i1 < i2) {
                count += 1;
            }
        }
    }
    count
}

/// Sugiyama's *transpose* step: swap adjacent items of a layer whenever that removes
/// crossings with the layers on either side, until nothing improves. The median sort
/// above is blind to ties and to the two neighbouring layers at once; this catches the
/// crossings it leaves (two fan-outs from side-by-side sources crossing each other).
fn transpose<T: Copy + Eq + std::hash::Hash>(layers: &mut [Vec<T>], preds: &HashMap<T, Vec<T>>, succs: &HashMap<T, Vec<T>>) {
    let n = layers.len();
    let around = |layers: &[Vec<T>], i: usize| -> usize {
        let mut c = 0;
        if i > 0 {
            c += crossings_between(&layers[i - 1], &layers[i], preds);
        }
        if i + 1 < n {
            c += crossings_between(&layers[i + 1], &layers[i], succs);
        }
        c
    };
    for _round in 0..8 {
        let mut improved = false;
        for i in 0..n {
            for k in 0..layers[i].len().saturating_sub(1) {
                let before = around(layers, i);
                if before == 0 {
                    break;
                }
                layers[i].swap(k, k + 1);
                if around(layers, i) < before {
                    improved = true;
                } else {
                    layers[i].swap(k, k + 1);
                }
            }
        }
        if !improved {
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// Chain straightening (lightweight stand-in for Brandes–Köpf coordinate assignment)
// ---------------------------------------------------------------------------

/// Applies the author's layout hints to longest-path ranks: a node's `rank:` pins it
/// (nothing pushes it; what it feeds still moves below it), and every `same_rank`
/// list is levelled to its deepest member — then successors are pushed down again
/// until nothing moves.
fn apply_rank_hints(compiled: &CompiledGraph, layer: &mut HashMap<NodeIndex, usize>, order: &[NodeIndex]) {
    let sets: Vec<Vec<NodeIndex>> = compiled
        .same_rank
        .iter()
        .map(|ids| ids.iter().filter_map(|id| compiled.node_map.get(id).copied()).collect::<Vec<_>>())
        .filter(|s| s.len() > 1)
        .collect();
    let pinned: HashMap<NodeIndex, usize> = order.iter().filter_map(|&n| compiled.graph[n].rank.map(|r| (n, r as usize))).collect();
    if sets.is_empty() && pinned.is_empty() {
        return;
    }
    for (&n, &r) in &pinned {
        layer.insert(n, r);
    }
    for _ in 0..(order.len() + sets.len() + 2) {
        let mut changed = false;
        for &n in order {
            if pinned.contains_key(&n) {
                continue;
            }
            let need = compiled
                .graph
                .edges_directed(n, petgraph::Direction::Incoming)
                .filter(|e| e.weight().constraint)
                .filter_map(|e| layer.get(&e.source()).map(|r| r + 1))
                .max()
                .unwrap_or(0);
            if layer.get(&n).is_some_and(|&r| r < need) {
                layer.insert(n, need);
                changed = true;
            }
        }
        for s in &sets {
            let m = s.iter().filter_map(|n| layer.get(n)).copied().max().unwrap_or(0);
            for n in s {
                if layer.get(n) != Some(&m) {
                    layer.insert(*n, m);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

/// Which coordinate is the "cross" axis being straightened — X for a top-to-bottom
/// layout (ranks stack in Y, siblings spread in X), Y for left-to-right (ranks stack in
/// X, siblings spread in Y).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    X,
    Y,
}

fn cross_pos(nl: &NodeLayout, axis: Axis) -> f64 {
    match axis {
        Axis::X => nl.x,
        Axis::Y => nl.y,
    }
}

fn cross_size(nl: &NodeLayout, axis: Axis) -> f64 {
    match axis {
        Axis::X => nl.width,
        Axis::Y => nl.height,
    }
}

fn set_cross_pos(nl: &mut NodeLayout, axis: Axis, value: f64) {
    match axis {
        Axis::X => nl.x = value,
        Axis::Y => nl.y = value,
    }
}

/// Nudges each node toward the mean cross-axis center of its already-placed
/// predecessors (down sweep) or successors (up sweep), within the fixed rank order
/// `layer_buckets` already established by [`minimise_crossings`] — never reordering
/// siblings, only shifting them, and never letting two siblings in the same rank
/// overlap or violate `cross_spacing`.
///
/// This is what makes a simple `A -> B -> C` chain draw as a straight line instead of
/// zigzagging whenever a sibling rank has a different width, and what centers a node
/// with several children over the span those children ended up occupying. It is a
/// deliberately lightweight heuristic, not full Brandes–Köpf alignment: it can't always
/// find the single best assignment, but a couple of down/up passes converge on
/// noticeably straighter, more human-drawn-looking output at a fraction of the
/// implementation cost.
fn straighten_layers(
    positions: &mut HashMap<NodeIndex, NodeLayout>,
    layer_buckets: &[Vec<NodeIndex>],
    compiled: &CompiledGraph,
    cross_spacing: f64,
    axis: Axis,
) {
    use petgraph::Direction;

    if layer_buckets.len() < 2 {
        return;
    }

    for _pass in 0..2 {
        for layer in layer_buckets.iter().skip(1) {
            sweep_layer(positions, layer, compiled, Some(Direction::Incoming), cross_spacing, axis);
        }
        for layer in layer_buckets[..layer_buckets.len() - 1].iter().rev() {
            sweep_layer(positions, layer, compiled, Some(Direction::Outgoing), cross_spacing, axis);
        }
    }
    // Then settle every rank between its neighbours on both sides: a one-way sweep
    // leaves whichever direction ran last winning (a node pulled under its children
    // but off its parent), and a leaf rank aligned only to its parents.
    for _pass in 0..4 {
        for layer in layer_buckets.iter() {
            sweep_layer(positions, layer, compiled, None, cross_spacing, axis);
        }
    }
    if std::env::var_os("RDG_DEBUG_STRAIGHTEN").is_some() {
        for (i, layer) in layer_buckets.iter().enumerate() {
            let row: Vec<String> = layer.iter().map(|n| format!("{}@{:.0}", compiled.graph[*n].id, cross_pos(&positions[n], axis) + cross_size(&positions[n], axis) / 2.0)).collect();
            eprintln!("straighten rank {i}: {}", row.join("  "));
        }
    }
}

/// `direction`: which neighbours pull — predecessors, successors, or (`None`) both.
fn sweep_layer(
    positions: &mut HashMap<NodeIndex, NodeLayout>,
    layer: &[NodeIndex],
    compiled: &CompiledGraph,
    direction: Option<petgraph::Direction>,
    cross_spacing: f64,
    axis: Axis,
) {
    // Desired center is computed from a read-only snapshot first — mutating `positions`
    // mid-sweep would make later same-layer lookups see already-shifted neighbors instead
    // of the rank being aligned against.
    let snapshot: &HashMap<NodeIndex, NodeLayout> = positions;
    let desired: Vec<(NodeIndex, f64, f64, bool)> = layer
        .iter()
        .map(|&node| {
            let nl = &snapshot[&node];
            let size = cross_size(nl, axis);
            let current_center = cross_pos(nl, axis) + size / 2.0;
            // A back edge (reversed to break a cycle — a retry loop) barely pulls: the
            // forward chain stays straight and the loop goes round it.
            let dirs: &[petgraph::Direction] = match direction {
                Some(petgraph::Direction::Incoming) => &[petgraph::Direction::Incoming],
                Some(petgraph::Direction::Outgoing) => &[petgraph::Direction::Outgoing],
                None => &[petgraph::Direction::Incoming, petgraph::Direction::Outgoing],
            };
            let neighbor_centers: Vec<(f64, f64)> = dirs
                .iter()
                .flat_map(|&dir| {
                    compiled.graph.edges_directed(node, dir).filter_map(move |e| {
                        let neighbor = match dir {
                            petgraph::Direction::Incoming => e.source(),
                            petgraph::Direction::Outgoing => e.target(),
                        };
                        let weight = if e.weight().reversed { 0.15 } else { 1.0 };
                        snapshot.get(&neighbor).map(|n| (cross_pos(n, axis) + cross_size(n, axis) / 2.0, weight))
                    })
                })
                .filter(|&(_, w)| w > 0.0)
                .collect();
            let total_weight: f64 = neighbor_centers.iter().map(|&(_, w)| w).sum();
            let target_center = if neighbor_centers.is_empty() {
                current_center
            } else {
                neighbor_centers.iter().map(|&(c, w)| c * w).sum::<f64>() / total_weight
            };
            (node, target_center, size, !neighbor_centers.is_empty())
        })
        .collect();

    // The rank keeps its crossing-minimised order; within it every node lands as close
    // to its target as the spacing allows, a crowded run sharing the shortfall both
    // ways (weighted isotonic regression — pushing only rightward from the first node
    // let every rank creep right each pass, and the parents follow, without end).
    let widths: Vec<f64> = desired.iter().map(|d| d.2).collect();
    let wants: Vec<f64> = desired.iter().map(|d| d.1 - d.2 / 2.0).collect();
    let weights: Vec<f64> = desired.iter().map(|d| if d.3 { 1.0 } else { 0.05 }).collect();
    let gaps = vec![cross_spacing; widths.len().saturating_sub(1)];
    let xs = crate::compound::place_in_order(&widths, &wants, &weights, &gaps, None);
    for ((node, _, _, _), x) in desired.into_iter().zip(xs) {
        set_cross_pos(positions.get_mut(&node).unwrap(), axis, x);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LayoutConfig, compute_layout};
    use rdg_graph::build_graph;
    use rdg_schema::{DiagramPayload, EdgeDef, NodeDef};

    fn chain_payload(ids: &[&str]) -> DiagramPayload {
        DiagramPayload {
            nodes: ids
                .iter()
                .map(|id| NodeDef {
                    id: id.to_string(),
                    label: id.to_string(),
                    ..Default::default()
                })
                .collect(),
            edges: ids
                .windows(2)
                .map(|w| EdgeDef {
                    from: w[0].to_string(),
                    to: w[1].to_string(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn test_straight_chain_stays_vertically_aligned() {
        // A -> B -> C with no siblings should end up with all three sharing the same
        // x-center once straightened, instead of whatever the raw per-layer centering
        // happened to produce.
        let payload = chain_payload(&["a", "b", "c"]);
        let compiled = build_graph(&payload).unwrap();
        let result = compute_layout(&compiled, &LayoutConfig::default()).unwrap();

        let center = |id: &str| {
            let nl = &result.positions[&compiled.node_map[id]];
            nl.x + nl.width / 2.0
        };
        let (ca, cb, cc) = (center("a"), center("b"), center("c"));
        assert!(
            (ca - cb).abs() < 0.5,
            "a and b should share a center: {ca} vs {cb}"
        );
        assert!(
            (cb - cc).abs() < 0.5,
            "b and c should share a center: {cb} vs {cc}"
        );
    }

    #[test]
    fn test_straightening_does_not_overlap_siblings() {
        // n1 -> n2, n1 -> n3, n1 -> n4: three children in one rank. Straightening should
        // pull n1 toward their combined center without making any two children overlap.
        let payload = DiagramPayload {
            nodes: ["n1", "n2", "n3", "n4"]
                .iter()
                .map(|id| NodeDef {
                    id: id.to_string(),
                    label: id.to_string(),
                    ..Default::default()
                })
                .collect(),
            edges: ["n2", "n3", "n4"]
                .iter()
                .map(|to| EdgeDef {
                    from: "n1".to_string(),
                    to: to.to_string(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let compiled = build_graph(&payload).unwrap();
        let result = compute_layout(&compiled, &LayoutConfig::default()).unwrap();

        let mut children: Vec<&NodeLayout> = ["n2", "n3", "n4"]
            .iter()
            .map(|id| &result.positions[&compiled.node_map[*id]])
            .collect();
        children.sort_by(|a, b| a.x.total_cmp(&b.x));
        for pair in children.windows(2) {
            assert!(
                pair[0].x + pair[0].width <= pair[1].x + 0.01,
                "siblings must not overlap after straightening"
            );
        }
    }

    #[test]
    fn test_explicit_width_height_override_skips_estimation() {
        let mut payload = chain_payload(&["a", "b"]);
        payload.nodes[0].width = Some(300.0);
        payload.nodes[0].height = Some(120.0);
        let compiled = build_graph(&payload).unwrap();
        let result = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let nl = &result.positions[&compiled.node_map["a"]];
        assert_eq!(nl.width, 300.0);
        assert_eq!(nl.height, 120.0);
    }
}
