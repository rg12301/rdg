//! Barnes-Hut force-directed layout — Condition B of the topology dispatcher (dense
//! or massive graphs that a layered/hierarchical layout can't represent well).
//!
//! Initial placement is a deterministic spiral seeded purely from each node's index
//! (never RNG — see [`crate::hillclimb`] for why determinism matters here), then
//! [`crate::physics::relax`] runs the actual Barnes-Hut simulation, and
//! [`crate::hillclimb::refine`] does a final local cleanup pass.

use std::collections::HashMap;

use anyhow::Result;

use rdg_graph::CompiledGraph;

use crate::{compute_node_sizes, hillclimb, ideal_edge_length, normalize_positions, physics, LayoutConfig, LayoutResult, NodeLayout};

/// Compute a Barnes-Hut force-directed layout for `compiled`.
///
/// # Errors
///
/// Returns an error only if internal graph operations fail unexpectedly (kept for
/// signature parity with [`crate::compute_layout`]; the simulation itself is
/// infallible).
pub fn compute_force_layout(compiled: &CompiledGraph, config: &LayoutConfig) -> Result<LayoutResult> {
    if compiled.graph.node_count() == 0 {
        return Ok(LayoutResult { positions: HashMap::new(), sequence_info: None });
    }

    let sizes = compute_node_sizes(compiled, config);
    let ideal_len = ideal_edge_length(&sizes, config);

    let initial = physics::spiral_seed(compiled, ideal_len);
    let relaxed = physics::relax(initial, compiled, ideal_len, config.tokens.physics_iterations, None, &config.tokens);

    let mut positions = HashMap::new();
    for (idx, (cx, cy)) in relaxed {
        let (w, h) = sizes[&idx];
        positions.insert(idx, NodeLayout { x: cx - w / 2.0, y: cy - h / 2.0, width: w, height: h });
    }

    hillclimb::refine(&mut positions, compiled, &config.tokens, &HashMap::new());

    let mut result = LayoutResult { positions, sequence_info: None };
    normalize_positions(&mut result, config, !compiled.groups.is_empty());
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rdg_graph::build_graph;
    use rdg_schema::DiagramPayload;

    fn dense_mesh_yaml(n: usize) -> String {
        let mut yaml = String::from("nodes:\n");
        for i in 0..n {
            yaml.push_str(&format!("  - id: n{i}\n    label: N{i}\n"));
        }
        yaml.push_str("edges:\n");
        for i in 0..n {
            for j in 0..n {
                if i != j {
                    yaml.push_str(&format!("  - {{from: n{i}, to: n{j}}}\n"));
                }
            }
        }
        yaml
    }

    #[test]
    fn test_empty_graph_returns_empty_positions() {
        let payload = DiagramPayload::default();
        let compiled = build_graph(&payload).unwrap();
        let result = compute_force_layout(&compiled, &LayoutConfig::default()).unwrap();
        assert!(result.positions.is_empty());
    }

    #[test]
    fn test_single_node_gets_a_position() {
        let payload = DiagramPayload::from_yaml("nodes:\n  - id: solo\n    label: Solo\n").unwrap();
        let compiled = build_graph(&payload).unwrap();
        let result = compute_force_layout(&compiled, &LayoutConfig::default()).unwrap();
        assert_eq!(result.positions.len(), 1);
        let nl = result.positions.values().next().unwrap();
        assert!(nl.x.is_finite() && nl.y.is_finite());
    }

    #[test]
    fn test_dense_mesh_produces_no_overlap() {
        let compiled = build_graph(&DiagramPayload::from_yaml(&dense_mesh_yaml(8)).unwrap()).unwrap();
        let result = compute_force_layout(&compiled, &LayoutConfig::default()).unwrap();
        let boxes: Vec<_> = result.positions.values().collect();
        for i in 0..boxes.len() {
            for j in (i + 1)..boxes.len() {
                let (a, b) = (boxes[i], boxes[j]);
                let overlap = a.x < b.x + b.width && a.x + a.width > b.x && a.y < b.y + b.height && a.y + a.height > b.y;
                assert!(!overlap, "nodes {i} and {j} overlap after force layout + hill climbing");
            }
        }
    }

    #[test]
    fn test_layout_is_deterministic_across_runs() {
        let payload = DiagramPayload::from_yaml(&dense_mesh_yaml(6)).unwrap();
        let compiled = build_graph(&payload).unwrap();
        let config = LayoutConfig::default();
        let r1 = compute_force_layout(&compiled, &config).unwrap();
        let r2 = compute_force_layout(&compiled, &config).unwrap();
        for (idx, nl1) in &r1.positions {
            let nl2 = &r2.positions[idx];
            assert_eq!(nl1.x, nl2.x);
            assert_eq!(nl1.y, nl2.y);
        }
    }

    #[test]
    fn test_all_positions_are_finite() {
        let compiled = build_graph(&DiagramPayload::from_yaml(&dense_mesh_yaml(12)).unwrap()).unwrap();
        let result = compute_force_layout(&compiled, &LayoutConfig::default()).unwrap();
        for nl in result.positions.values() {
            assert!(nl.x.is_finite() && nl.y.is_finite() && nl.width.is_finite() && nl.height.is_finite());
        }
    }

    #[test]
    fn test_explicit_size_override_is_respected() {
        let payload = DiagramPayload::from_yaml(
            "nodes:\n  - id: a\n    label: A\n    width: 300\n    height: 200\n  - id: b\n    label: B\nedges:\n  - from: a\n    to: b\n",
        )
        .unwrap();
        let compiled = build_graph(&payload).unwrap();
        let result = compute_force_layout(&compiled, &LayoutConfig::default()).unwrap();
        let a = &result.positions[&compiled.node_map["a"]];
        assert_eq!(a.width, 300.0);
        assert_eq!(a.height, 200.0);
    }
}
