//! Graph construction layer.
//!
//! Converts a [`DiagramPayload`] into a [`petgraph::stable_graph::StableDiGraph`].
//! A stable graph is required because hierarchical layout algorithms temporarily
//! insert and delete dummy nodes to route long edges across layers — node indices
//! must not shift during this process.
//!
//! This module also implements a greedy **Feedback Arc Set (FAS)** cycle-breaker
//! so that DAG-only layout engines (e.g. Sugiyama) can process any input safely.

use anyhow::Result;
use petgraph::{
    algo::is_cyclic_directed,
    stable_graph::{EdgeIndex, NodeIndex, StableDiGraph},
    visit::{EdgeRef, IntoEdgeReferences},
    Direction,
};
use std::collections::{HashMap, HashSet};

use crate::schema::{DiagramPayload, GroupDef};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Data attached to every graph node.
#[derive(Debug, Clone)]
pub struct NodeData {
    /// Original user-supplied identifier.
    pub id: String,
    /// Human-readable label.
    pub label: String,
    /// Semantic type string (maps to a draw.io style).
    pub node_type: String,
    /// Optional free-text annotation.
    pub metadata: Option<String>,
}

/// Data attached to every graph edge.
#[derive(Debug, Clone)]
pub struct EdgeData {
    /// Optional edge label.
    pub label: Option<String>,
    /// Optional semantic edge style (e.g. `async`, `error`, `data`, `bidirectional`).
    pub edge_style: Option<String>,
    /// `true` when this edge was reversed to break a cycle.
    /// The renderer uses this flag to flip arrow direction.
    pub reversed: bool,
}

/// The fully-validated, cycle-free compiled graph.
pub struct CompiledGraph {
    /// The petgraph stable directed graph.
    pub graph: StableDiGraph<NodeData, EdgeData>,
    /// Maps user-supplied string IDs to petgraph [`NodeIndex`] values.
    pub node_map: HashMap<String, NodeIndex>,
    /// `true` when at least one cycle was detected and broken.
    pub had_cycles: bool,
    /// Optional visual groups / swimlanes.
    pub groups: Vec<GroupDef>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Build a [`CompiledGraph`] from a [`DiagramPayload`].
///
/// Steps:
/// 1. Add all nodes, populating `node_map`.
/// 2. Add all edges, validating that source and target IDs exist.
/// 3. Detect cycles; if any exist, run Eades-Lin-Smyth FAS to reverse
///    back-edges until the graph is acyclic.
///
/// # Errors
///
/// Returns an error if any edge references an unknown node ID.
pub fn build_graph(payload: &DiagramPayload) -> Result<CompiledGraph> {
    let mut graph: StableDiGraph<NodeData, EdgeData> = StableDiGraph::new();
    let mut node_map: HashMap<String, NodeIndex> = HashMap::with_capacity(payload.nodes.len());

    // --- 1. Add nodes -------------------------------------------------------
    for node_def in &payload.nodes {
        let data = NodeData {
            id: node_def.id.clone(),
            label: node_def.label.clone(),
            node_type: node_def.node_type.clone(),
            metadata: node_def.metadata.clone(),
        };
        let idx = graph.add_node(data);
        node_map.insert(node_def.id.clone(), idx);
    }

    // --- 2. Add edges -------------------------------------------------------
    for edge_def in &payload.edges {
        let src = node_map.get(&edge_def.from).copied().ok_or_else(|| {
            anyhow::anyhow!(
                "edge references unknown source node id '{}'",
                edge_def.from
            )
        })?;
        let dst = node_map.get(&edge_def.to).copied().ok_or_else(|| {
            anyhow::anyhow!(
                "edge references unknown target node id '{}'",
                edge_def.to
            )
        })?;
        graph.add_edge(
            src,
            dst,
            EdgeData {
                label: edge_def.label.clone(),
                edge_style: edge_def.edge_style.clone(),
                reversed: false,
            },
        );
    }

    // --- 3. Cycle detection & breaking --------------------------------------
    let had_cycles = if is_cyclic_directed(&graph) {
        break_cycles(&mut graph);
        true
    } else {
        false
    };

    Ok(CompiledGraph {
        graph,
        node_map,
        had_cycles,
        groups: payload.groups.clone(),
    })
}

// ---------------------------------------------------------------------------
// Eades-Lin-Smyth (Greedy FAS) Cycle Breaking
// ---------------------------------------------------------------------------

/// Reverse back-edges using the Eades-Lin-Smyth heuristic (1993) to make the graph a DAG.
///
/// ELS guarantees an O(V + E) runtime and preserves the primary flow of the graph
/// much better than arbitrary DFS back-edge reversal.
fn break_cycles(graph: &mut StableDiGraph<NodeData, EdgeData>) {
    let all_nodes: Vec<NodeIndex> = graph.node_indices().collect();
    if all_nodes.is_empty() {
        return;
    }

    let mut remaining: HashSet<NodeIndex> = all_nodes.into_iter().collect();

    // Sequences s1 (left, sources) and s2 (right, sinks)
    let mut s1: Vec<NodeIndex> = Vec::new();
    let mut s2: Vec<NodeIndex> = Vec::new();

    while !remaining.is_empty() {
        // 1. Sink elimination: nodes with out-degree 0 among remaining
        let mut sink_found = true;
        while sink_found {
            sink_found = false;
            let sinks: Vec<NodeIndex> = remaining
                .iter()
                .copied()
                .filter(|&u| {
                    graph
                        .edges_directed(u, Direction::Outgoing)
                        .filter(|e| remaining.contains(&e.target()))
                        .count()
                        == 0
                })
                .collect();

            for u in sinks {
                remaining.remove(&u);
                s2.push(u);
                sink_found = true;
            }
        }

        if remaining.is_empty() {
            break;
        }

        // 2. Source elimination: nodes with in-degree 0 among remaining
        let mut source_found = true;
        while source_found {
            source_found = false;
            let sources: Vec<NodeIndex> = remaining
                .iter()
                .copied()
                .filter(|&u| {
                    graph
                        .edges_directed(u, Direction::Incoming)
                        .filter(|e| remaining.contains(&e.source()))
                        .count()
                        == 0
                })
                .collect();

            for u in sources {
                remaining.remove(&u);
                s1.push(u);
                source_found = true;
            }
        }

        if remaining.is_empty() {
            break;
        }

        // 3. Net-source selection: pick node maximizing out_deg - in_deg
        if let Some(&best) = remaining.iter().max_by_key(|&&u| {
            let out_deg = graph
                .edges_directed(u, Direction::Outgoing)
                .filter(|e| remaining.contains(&e.target()))
                .count() as i64;
            let in_deg = graph
                .edges_directed(u, Direction::Incoming)
                .filter(|e| remaining.contains(&e.source()))
                .count() as i64;
            out_deg - in_deg
        }) {
            remaining.remove(&best);
            s1.push(best);
        }
    }

    // Linear sequence: s1 ++ reverse(s2)
    s2.reverse();
    s1.extend(s2);

    let pos: HashMap<NodeIndex, usize> = s1.iter().enumerate().map(|(i, &n)| (n, i)).collect();

    // Identify edges pointing backwards in the sequence
    let mut back_edges: Vec<(EdgeIndex, NodeIndex, NodeIndex)> = Vec::new();
    for edge in graph.edge_references() {
        let src = edge.source();
        let dst = edge.target();
        if let (Some(&src_pos), Some(&dst_pos)) = (pos.get(&src), pos.get(&dst)) {
            if src_pos > dst_pos {
                back_edges.push((edge.id(), src, dst));
            }
        }
    }

    // Reverse all back-edges in-place
    for (eid, src, dst) in back_edges {
        if let Some(data) = graph.remove_edge(eid) {
            graph.add_edge(
                dst,
                src,
                EdgeData {
                    label: data.label,
                    edge_style: data.edge_style,
                    reversed: true,
                },
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{DiagramPayload, EdgeDef, NodeDef};

    fn make_payload(nodes: &[(&str, &str)], edges: &[(&str, &str)]) -> DiagramPayload {
        DiagramPayload {
            diagram_type: "flowchart".to_owned(),
            theme: None,
            direction: None,
            nodes: nodes
                .iter()
                .map(|(id, label)| NodeDef {
                    id: id.to_string(),
                    label: label.to_string(),
                    node_type: "default".to_owned(),
                    metadata: None,
                })
                .collect(),
            edges: edges
                .iter()
                .map(|(from, to)| EdgeDef {
                    from: from.to_string(),
                    to: to.to_string(),
                    label: None,
                    edge_style: None,
                })
                .collect(),
            groups: vec![],
        }
    }

    #[test]
    fn test_build_simple_graph() {
        let payload = make_payload(&[("n1", "Node 1"), ("n2", "Node 2")], &[("n1", "n2")]);
        let compiled = build_graph(&payload).expect("build should succeed");
        assert_eq!(compiled.graph.node_count(), 2);
        assert_eq!(compiled.graph.edge_count(), 1);
        assert!(!compiled.had_cycles);
    }

    #[test]
    fn test_unknown_source_node_returns_error() {
        let payload = make_payload(&[("n1", "Node 1")], &[("ghost", "n1")]);
        assert!(build_graph(&payload).is_err());
    }

    #[test]
    fn test_unknown_target_node_returns_error() {
        let payload = make_payload(&[("n1", "Node 1")], &[("n1", "ghost")]);
        assert!(build_graph(&payload).is_err());
    }

    #[test]
    fn test_cycle_detection_and_breaking() {
        // n1 → n2 → n3 → n1  (cycle)
        let payload = make_payload(
            &[("n1", "A"), ("n2", "B"), ("n3", "C")],
            &[("n1", "n2"), ("n2", "n3"), ("n3", "n1")],
        );
        let compiled = build_graph(&payload).expect("build should succeed");
        assert!(compiled.had_cycles, "cycle should have been detected");
        assert!(
            !is_cyclic_directed(&compiled.graph),
            "result graph must be acyclic after FAS"
        );
    }

    #[test]
    fn test_empty_graph() {
        let payload = make_payload(&[], &[]);
        let compiled = build_graph(&payload).expect("empty graph should build");
        assert_eq!(compiled.graph.node_count(), 0);
        assert_eq!(compiled.graph.edge_count(), 0);
        assert!(!compiled.had_cycles);
    }
}
