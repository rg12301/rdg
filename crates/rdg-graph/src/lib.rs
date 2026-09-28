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
    Direction,
    algo::is_cyclic_directed,
    stable_graph::{EdgeIndex, NodeIndex, StableDiGraph},
    visit::{EdgeRef, IntoEdgeReferences},
};
use std::collections::{HashMap, HashSet};

use rdg_schema::{DiagramPayload, GroupDef};

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
    /// Optional list of table columns or class members.
    pub fields: Vec<String>,
    /// Optional programming language used to code this component (e.g. `rust`, `go`, `python`).
    pub language: Option<String>,
    /// Optional technology stack or framework subtitle (e.g. `Axum + Tokio`, `FastAPI`).
    pub technology: Option<String>,
    /// Optional database engine (e.g. `postgres`, `mysql`, `redis`, `mongodb`).
    pub db_type: Option<String>,
    /// Resolved icon key for visual rendering.
    pub icon: Option<String>,
    /// Optional custom accent/border color, overriding the semantic-type color table.
    pub color: Option<String>,
    /// Optional explicit width in pixels (skips automatic content-based sizing).
    pub width: Option<f64>,
    /// Optional explicit height in pixels (skips automatic content-based sizing).
    pub height: Option<f64>,
    /// Optional cloud provider hint (`aws`, `gcp`, `azure`) for icon/shape selection.
    pub provider: Option<String>,
    /// Optional raw draw.io style fragment appended verbatim (draw.io backend only).
    pub style_extra: Option<String>,
    /// Optional URL making the rendered shape clickable (draw.io backend only).
    pub link: Option<String>,
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
    /// Optional custom stroke color (e.g. `#ef4444`, `#0284c7`).
    pub color: Option<String>,
    /// Optional stroke width/thickness in pixels.
    pub width: Option<f64>,
    /// Optional line pattern (`solid`, `dashed`, `dotted`).
    pub line_style: Option<String>,
    /// Optional arrow head marker type.
    pub head: Option<String>,
    /// Optional arrow tail marker type.
    pub tail: Option<String>,
    /// Optional explicit source port face (`top`, `bottom`, `left`, `right`).
    pub source_port: Option<String>,
    /// Optional explicit target port face (`top`, `bottom`, `left`, `right`).
    pub target_port: Option<String>,
    /// Optional raw draw.io style fragment appended verbatim (draw.io backend only).
    pub style_extra: Option<String>,
    /// Resolved flow sequence number (1-indexed), set only when the diagram opted into
    /// `numbered: true`; `None` means "don't draw a sequence badge for this edge."
    pub step: Option<u32>,
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
    /// Optional diagram title banner.
    pub title: Option<String>,
    /// Optional diagram description / subtitle.
    pub description: Option<String>,
    /// Canonical diagram category (e.g. `flowchart`, `sequence`, `er`, `class`, `state`).
    pub diagram_type: String,
    /// Sequential order of edges as declared in input payload.
    pub edge_order: Vec<petgraph::stable_graph::EdgeIndex>,
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
    let mut edge_order: Vec<EdgeIndex> = Vec::with_capacity(payload.edges.len());

    let diagram_type = payload.resolved_diagram_type().to_string();
    let is_sequence = diagram_type == "sequence";

    // --- 1. Resolve container / group languages and icon inheritance ----------
    let mut node_to_group_lang: HashMap<String, String> = HashMap::new();
    let mut compiled_groups = payload.groups.clone();

    for group in &mut compiled_groups {
        let explicit_or_label_lang = group.resolved_language();
        let common_lang = if let Some(l) = explicit_or_label_lang {
            Some(l)
        } else {
            // Check if member nodes in this group share a common language
            let member_langs: Vec<String> = group
                .nodes
                .iter()
                .filter_map(|nid| {
                    payload
                        .nodes
                        .iter()
                        .find(|n| &n.id == nid)
                        .and_then(|n| n.resolved_language())
                })
                .collect();
            if !member_langs.is_empty() && member_langs.iter().all(|l| l == &member_langs[0]) {
                Some(member_langs[0].clone())
            } else {
                None
            }
        };

        if let Some(lang) = common_lang {
            if group.language.is_none() {
                group.language = Some(lang.clone());
            }
            if group.icon.is_none() {
                group.icon = Some(lang.clone());
            }
            for nid in &group.nodes {
                node_to_group_lang.insert(nid.clone(), lang.clone());
            }
        }
    }

    // --- 2. Add nodes ---------------------------------------------------------
    // `effective_node` resolves any `class:` style preset before we read fields off of it, so
    // presets and per-node explicit fields both flow through the same `resolved_*()` accessors.
    for node_def in &payload.nodes {
        let node_def = payload.effective_node(node_def);
        let node_lang = node_def.resolved_language();
        let mut icon = node_def.resolved_icon();

        // If the node belongs to a group with the same language, and the node did NOT explicitly
        // set a custom icon or specific database engine, suppress the redundant language icon on the shape!
        if let Some(grp_lang) = node_to_group_lang.get(&node_def.id) {
            let has_explicit_icon = node_def.icon.as_ref().is_some_and(|i| !i.trim().is_empty());
            let has_db_engine = node_def.resolved_db_type().is_some();
            if !has_explicit_icon && !has_db_engine {
                if let Some(ref nl) = node_lang {
                    if nl == grp_lang {
                        // Same stack as container! Highlight on container instead of cluttering each shape.
                        icon = None;
                    }
                }
            }
        }

        let data = NodeData {
            id: node_def.id.clone(),
            label: node_def.resolved_label(),
            node_type: node_def.node_type.clone(),
            metadata: node_def.metadata.clone(),
            fields: node_def.resolved_fields(),
            language: node_lang,
            technology: node_def.resolved_technology(),
            db_type: node_def.resolved_db_type(),
            icon,
            color: node_def.color.clone(),
            width: node_def.width,
            height: node_def.height,
            provider: node_def.provider.clone(),
            style_extra: node_def.style_extra.clone(),
            link: node_def.link.clone(),
        };
        let idx = graph.add_node(data);
        node_map.insert(node_def.id.clone(), idx);
    }

    // --- 2. Add edges -----------------------------------------------------------
    let numbered = payload.is_numbered();
    for (decl_index, edge_def) in payload.edges.iter().enumerate() {
        let edge_def = payload.effective_edge(edge_def);
        let src = node_map.get(&edge_def.from).copied().ok_or_else(|| {
            anyhow::anyhow!("edge references unknown source node id '{}'", edge_def.from)
        })?;
        let dst = node_map.get(&edge_def.to).copied().ok_or_else(|| {
            anyhow::anyhow!("edge references unknown target node id '{}'", edge_def.to)
        })?;
        // Only resolve a sequence number when the diagram opted in — an explicit `step`
        // always wins, otherwise it's the 1-indexed declaration order.
        let step = if numbered {
            Some(edge_def.step.unwrap_or(decl_index as u32 + 1))
        } else {
            edge_def.step
        };
        let e_idx = graph.add_edge(
            src,
            dst,
            EdgeData {
                label: edge_def.label.clone(),
                edge_style: edge_def.resolved_style(),
                reversed: false,
                color: edge_def.color.clone(),
                width: edge_def.width,
                line_style: edge_def.line_style.clone(),
                head: edge_def.head.clone(),
                tail: edge_def.tail.clone(),
                source_port: edge_def.source_port.clone(),
                target_port: edge_def.target_port.clone(),
                style_extra: edge_def.style_extra.clone(),
                step,
            },
        );
        edge_order.push(e_idx);
    }

    // --- 3. Cycle detection & breaking (skipped for sequence diagrams) -------
    let had_cycles = if !is_sequence && is_cyclic_directed(&graph) {
        break_cycles(&mut graph);
        true
    } else {
        false
    };

    Ok(CompiledGraph {
        graph,
        node_map,
        had_cycles,
        groups: compiled_groups,
        title: payload.title.clone(),
        description: payload.description.clone(),
        diagram_type,
        edge_order,
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

    let mut remaining: HashSet<NodeIndex> = all_nodes.iter().copied().collect();

    // Sequences s1 (left, sources) and s2 (right, sinks)
    let mut s1: Vec<NodeIndex> = Vec::new();
    let mut s2: Vec<NodeIndex> = Vec::new();

    // Every scan below iterates `all_nodes` (a `Vec`, fixed insertion order) filtered by
    // `remaining.contains(..)`, not `remaining.iter()` directly — `remaining` is a
    // `HashSet`, whose iteration order is randomized per process (Rust's default hasher
    // seeds itself from the OS on each run). A dense graph with many same-degree nodes
    // (ties are the common case, not the exception) hits that order at three points here:
    // which simultaneous sinks/sources get appended to s1/s2 in what order, and — worse —
    // `max_by_key`'s tie-break, which returns the *last* maximal element seen, so a tied
    // "net-source" pick silently changed between runs too. Any of those reorders `s1`,
    // which decides which edges get flagged as back-edges and reversed — changing the
    // DAG orientation, and downstream layout, for a diagram whose YAML never changed.
    while !remaining.is_empty() {
        // 1. Sink elimination: nodes with out-degree 0 among remaining
        let mut sink_found = true;
        while sink_found {
            sink_found = false;
            let sinks: Vec<NodeIndex> = all_nodes
                .iter()
                .copied()
                .filter(|u| remaining.contains(u))
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
            let sources: Vec<NodeIndex> = all_nodes
                .iter()
                .copied()
                .filter(|u| remaining.contains(u))
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
        if let Some(best) = all_nodes
            .iter()
            .copied()
            .filter(|u| remaining.contains(u))
            .max_by_key(|&u| {
                let out_deg = graph
                    .edges_directed(u, Direction::Outgoing)
                    .filter(|e| remaining.contains(&e.target()))
                    .count() as i64;
                let in_deg = graph
                    .edges_directed(u, Direction::Incoming)
                    .filter(|e| remaining.contains(&e.source()))
                    .count() as i64;
                out_deg - in_deg
            })
        {
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
                    color: data.color,
                    width: data.width,
                    line_style: data.line_style,
                    head: data.head,
                    tail: data.tail,
                    source_port: data.source_port,
                    target_port: data.target_port,
                    style_extra: data.style_extra,
                    step: data.step,
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
    use rdg_schema::{DiagramPayload, EdgeDef, NodeDef};

    fn make_payload(nodes: &[(&str, &str)], edges: &[(&str, &str)]) -> DiagramPayload {
        DiagramPayload {
            diagram_type: "flowchart".to_owned(),
            nodes: nodes
                .iter()
                .map(|(id, label)| NodeDef {
                    id: id.to_string(),
                    label: label.to_string(),
                    ..Default::default()
                })
                .collect(),
            edges: edges
                .iter()
                .map(|(from, to)| EdgeDef {
                    from: from.to_string(),
                    to: to.to_string(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
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

    #[test]
    fn test_sequence_diagram_preserves_ping_pong_edges() {
        let seq_yaml = r#"
diagram_type: sequence
nodes:
  - id: client
    label: "Browser"
  - id: server
    label: "Server"
edges:
  - from: client
    to: server
    label: "GET /data"
  - from: server
    to: client
    label: "200 OK"
"#;
        let payload = DiagramPayload::from_yaml(seq_yaml).unwrap();
        let compiled = build_graph(&payload).unwrap();
        assert_eq!(compiled.graph.node_count(), 2);
        assert_eq!(compiled.graph.edge_count(), 2);
        assert!(
            !compiled.had_cycles,
            "sequence diagrams must not treat ping-pong calls as cycles"
        );
        assert_eq!(compiled.edge_order.len(), 2);
    }

    #[test]
    fn test_numbered_edges_get_declaration_order_step() {
        let yaml = r#"
numbered: true
nodes:
  - id: n1
    label: "A"
  - id: n2
    label: "B"
  - id: n3
    label: "C"
edges:
  - from: n1
    to: n2
  - from: n2
    to: n3
"#;
        let payload = DiagramPayload::from_yaml(yaml).unwrap();
        let compiled = build_graph(&payload).unwrap();
        let steps: Vec<Option<u32>> = compiled
            .edge_order
            .iter()
            .map(|&idx| compiled.graph[idx].step)
            .collect();
        assert_eq!(steps, vec![Some(1), Some(2)]);
    }

    #[test]
    fn test_unnumbered_edges_have_no_step() {
        let payload = make_payload(&[("n1", "A"), ("n2", "B")], &[("n1", "n2")]);
        let compiled = build_graph(&payload).unwrap();
        assert_eq!(compiled.graph[compiled.edge_order[0]].step, None);
    }

    #[test]
    fn test_explicit_step_overrides_declaration_order() {
        let yaml = r#"
numbered: true
nodes:
  - id: n1
    label: "A"
  - id: n2
    label: "B"
edges:
  - from: n1
    to: n2
    step: 42
"#;
        let payload = DiagramPayload::from_yaml(yaml).unwrap();
        let compiled = build_graph(&payload).unwrap();
        assert_eq!(compiled.graph[compiled.edge_order[0]].step, Some(42));
    }

    #[test]
    fn test_node_style_preset_applied_in_graph() {
        let yaml = r##"
node_styles:
  critical:
    color: "#ef4444"
nodes:
  - id: n1
    label: "A"
    class: critical
edges: []
"##;
        let payload = DiagramPayload::from_yaml(yaml).unwrap();
        let compiled = build_graph(&payload).unwrap();
        let idx = compiled.node_map["n1"];
        assert_eq!(compiled.graph[idx].color.as_deref(), Some("#ef4444"));
    }

    #[test]
    fn test_duplicate_node_ids_last_one_wins_in_node_map() {
        // Not an error today — documents current behavior so a future change is deliberate,
        // not an accidental silent regression.
        let payload = make_payload(&[("dup", "First"), ("dup", "Second")], &[]);
        let compiled = build_graph(&payload).expect("build should succeed");
        assert_eq!(compiled.graph.node_count(), 2);
        assert_eq!(compiled.node_map.len(), 1);
        let idx = compiled.node_map["dup"];
        assert_eq!(compiled.graph[idx].label, "Second");
    }

    #[test]
    fn test_self_loop_edge_builds_without_error() {
        let payload = make_payload(&[("n1", "A")], &[("n1", "n1")]);
        let compiled = build_graph(&payload).expect("self-loop should build");
        assert_eq!(compiled.graph.edge_count(), 1);
    }

    #[test]
    fn test_fully_connected_small_graph_breaks_cycles() {
        let nodes = [("a", "A"), ("b", "B"), ("c", "C"), ("d", "D")];
        let mut edges = Vec::new();
        for (from, _) in &nodes {
            for (to, _) in &nodes {
                if from != to {
                    edges.push((*from, *to));
                }
            }
        }
        let payload = make_payload(&nodes, &edges);
        let compiled = build_graph(&payload).expect("fully-connected graph should build");
        assert!(compiled.had_cycles);
        assert!(!is_cyclic_directed(&compiled.graph));
    }

    #[test]
    fn test_single_node_no_edges() {
        let payload = make_payload(&[("solo", "Solo")], &[]);
        let compiled = build_graph(&payload).expect("single node should build");
        assert_eq!(compiled.graph.node_count(), 1);
        assert_eq!(compiled.graph.edge_count(), 0);
        assert!(!compiled.had_cycles);
    }

    #[test]
    fn test_large_graph_builds_without_excessive_cost() {
        // A 500-node chain plus a long-range back edge, to exercise FAS on a graph much
        // larger than the hand-written fixtures above without taking real wall-clock time.
        const N: usize = 500;
        let ids: Vec<String> = (0..N).map(|i| format!("n{i}")).collect();
        let id_refs: Vec<(&str, &str)> = ids.iter().map(|s| (s.as_str(), s.as_str())).collect();
        let mut edges: Vec<(&str, &str)> = id_refs.windows(2).map(|w| (w[0].0, w[1].0)).collect();
        edges.push((ids[N - 1].as_str(), ids[0].as_str())); // one back-edge to force FAS to run
        let payload = make_payload(&id_refs, &edges);
        let compiled = build_graph(&payload).expect("large graph should build");
        assert_eq!(compiled.graph.node_count(), N);
        assert_eq!(compiled.graph.edge_count(), N); // (N-1) chain edges + 1 back-edge
        assert!(compiled.had_cycles);
        assert!(!is_cyclic_directed(&compiled.graph));
    }
}
