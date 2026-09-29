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

    Ok(LayoutResult {
        positions,
        sequence_info: None,
    })
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
}

// ---------------------------------------------------------------------------
// Chain straightening (lightweight stand-in for Brandes–Köpf coordinate assignment)
// ---------------------------------------------------------------------------

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
            sweep_layer(
                positions,
                layer,
                compiled,
                Direction::Incoming,
                cross_spacing,
                axis,
            );
        }
        for layer in layer_buckets[..layer_buckets.len() - 1].iter().rev() {
            sweep_layer(
                positions,
                layer,
                compiled,
                Direction::Outgoing,
                cross_spacing,
                axis,
            );
        }
    }
}

fn sweep_layer(
    positions: &mut HashMap<NodeIndex, NodeLayout>,
    layer: &[NodeIndex],
    compiled: &CompiledGraph,
    direction: petgraph::Direction,
    cross_spacing: f64,
    axis: Axis,
) {
    // Desired center is computed from a read-only snapshot first — mutating `positions`
    // mid-sweep would make later same-layer lookups see already-shifted neighbors instead
    // of the rank being aligned against.
    let desired: Vec<(NodeIndex, f64, f64)> = layer
        .iter()
        .map(|&node| {
            let nl = &positions[&node];
            let size = cross_size(nl, axis);
            let current_center = cross_pos(nl, axis) + size / 2.0;
            let neighbor_centers: Vec<f64> = compiled
                .graph
                .edges_directed(node, direction)
                .filter_map(|e| {
                    let neighbor = match direction {
                        petgraph::Direction::Incoming => e.source(),
                        petgraph::Direction::Outgoing => e.target(),
                    };
                    positions
                        .get(&neighbor)
                        .map(|n| cross_pos(n, axis) + cross_size(n, axis) / 2.0)
                })
                .collect();
            let target_center = if neighbor_centers.is_empty() {
                current_center
            } else {
                neighbor_centers.iter().sum::<f64>() / neighbor_centers.len() as f64
            };
            (node, target_center, size)
        })
        .collect();

    // Left-to-right (or top-to-bottom) sweep in the rank's existing, crossing-minimized
    // order: each node takes its desired position unless that would overlap the sibling
    // already placed before it, in which case it's pushed just past that sibling.
    let mut trailing_edge = f64::MIN;
    for (node, target_center, size) in desired {
        let mut new_pos = (target_center - size / 2.0).max(0.0);
        if trailing_edge > f64::MIN {
            new_pos = new_pos.max(trailing_edge + cross_spacing);
        }
        set_cross_pos(positions.get_mut(&node).unwrap(), axis, new_pos);
        trailing_edge = new_pos + size;
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
