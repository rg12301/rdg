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
    visit::EdgeRef,
};
use std::collections::HashMap;

use rdg_schema::{DiagramPayload, GroupDef, GroupTree, NodeText};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Data attached to every graph node.
#[derive(Debug, Clone)]
pub struct NodeData {
    /// Original user-supplied identifier.
    pub id: String,
    /// The node's text as one string (see [`rdg_schema::NodeDef::resolved_label`]).
    pub label: String,
    /// Title / subtitle / description as the author set them, when they did; drawn as
    /// given. `None`: `label` is drawn, its lines classified by `rdg_layout::classify_label`.
    pub text: Option<NodeText>,
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
    /// Explicit semantic category (else the theme derives one from the type).
    pub category: Option<String>,
    /// `card` / `icon` (else the theme decides).
    pub display: Option<String>,
    /// Layout hint: the rank the author pinned this node to.
    pub rank: Option<u32>,
    /// Drawn shape, resolved from the theme before layout: `card`, `cylinder`,
    /// `ellipse`, `diamond`, `icon`, or a marker (`start`, `end`, `choice`). `None`
    /// before resolution — consumers then fall back to the node type.
    pub shape: Option<String>,
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
    /// Flow-step badge text (`3`, `3a`, `3.1`) — see [`rdg_schema::resolve_steps`];
    /// `None` means no badge.
    pub step: Option<String>,
    /// Whether the edge takes part in ranking (`constraint: false` in the YAML opts out:
    /// the edge is routed and drawn, but never pushes its target to a later rank).
    pub constraint: bool,
}

/// The fully-validated, cycle-free compiled graph.
pub struct CompiledGraph {
    /// The petgraph stable directed graph.
    pub graph: StableDiGraph<NodeData, EdgeData>,
    /// Maps user-supplied string IDs to petgraph [`NodeIndex`] values.
    pub node_map: HashMap<String, NodeIndex>,
    /// `true` when at least one cycle was detected and broken.
    pub had_cycles: bool,
    /// Visual groups / swimlanes, as declared (`nodes` rewritten to the node ids each
    /// group holds directly — see [`Self::group_tree`]).
    pub groups: Vec<GroupDef>,
    /// How `groups` nest.
    pub group_tree: GroupTree,
    /// Nodes each group holds directly (inner groups' nodes excluded), in listed order.
    pub group_nodes: Vec<Vec<NodeIndex>>,
    /// Optional diagram title banner.
    pub title: Option<String>,
    /// Optional diagram description / subtitle.
    pub description: Option<String>,
    /// Canonical diagram category (e.g. `flowchart`, `sequence`, `er`, `class`, `state`).
    pub diagram_type: String,
    /// Sequential order of edges as declared in input payload.
    pub edge_order: Vec<petgraph::stable_graph::EdgeIndex>,
    /// Draw a legend (`None`: the theme decides).
    pub legend: Option<bool>,
    /// Layout hint: lists of node/group ids that share a rank (`same_rank:` as written;
    /// the layout resolves ids to nodes or tiers).
    pub same_rank: Vec<Vec<String>>,
    /// Sequence diagrams: the script in order (messages refer to graph edges). Built
    /// from `sequence:`, or from `edges` as a plain list of messages.
    pub sequence: Vec<SeqItem>,
}

impl NodeData {
    /// The node's name: its explicit title, else its label (the header of a table or
    /// class card, whose other lines are its fields).
    pub fn heading(&self) -> &str {
        self.text.as_ref().map_or(self.label.as_str(), |t| t.title.as_str())
    }
}

impl CompiledGraph {
    /// The group holding node `n` directly, if any.
    pub fn group_of(&self, n: NodeIndex) -> Option<usize> {
        self.group_nodes.iter().position(|ns| ns.contains(&n))
    }

    /// Every node inside group `g`, nested groups included.
    pub fn nodes_within(&self, g: usize) -> Vec<NodeIndex> {
        (0..self.groups.len())
            .filter(|&d| self.group_tree.contains(g, d))
            .flat_map(|d| self.group_nodes[d].iter().copied())
            .collect()
    }
}

/// One compiled step of a sequence diagram.
#[derive(Debug, Clone)]
pub enum SeqItem {
    Message {
        edge: EdgeIndex,
        /// Explicit `activate` / `deactivate` (None: the default rule decides).
        activate: Option<bool>,
        deactivate: Option<bool>,
        create: bool,
        destroy: bool,
    },
    Note { text: String, placement: NotePlacement, over: Vec<NodeIndex> },
    /// `sections[0]` is the body; later sections are `else` / `and` branches.
    Fragment { kind: String, sections: Vec<(Option<String>, Vec<SeqItem>)> },
    Divider(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotePlacement {
    Over,
    LeftOf,
    RightOf,
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

    // --- 1. Group nesting, then container languages and icon inheritance --------
    let group_tree = payload.group_tree();
    let mut node_to_group_lang: HashMap<String, String> = HashMap::new();
    let mut compiled_groups = payload.groups.clone();
    for (group, direct) in compiled_groups.iter_mut().zip(&group_tree.nodes) {
        group.nodes = direct.clone();
    }

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
            // Only when *every* node in it is written in that language: one Ruby member
            // among untyped ones doesn't make a Ruby group.
            if !member_langs.is_empty() && member_langs.len() == group.nodes.len() && member_langs.iter().all(|l| l == &member_langs[0]) {
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
        // The icon would only repeat the language its group already shows — drop it
        // (an explicit `icon:` or a database/broker logo is a different fact; kept).
        if let Some(grp_lang) = node_to_group_lang.get(&node_def.id) {
            let has_explicit_icon = node_def.icon.as_ref().is_some_and(|i| !i.trim().is_empty());
            if !has_explicit_icon && icon.as_deref() == Some(grp_lang.as_str()) {
                icon = None;
            }
        }

        let data = NodeData {
            id: node_def.id.clone(),
            label: node_def.resolved_label(),
            text: node_def.text(),
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
            category: node_def.category.clone(),
            display: node_def.display.clone(),
            rank: node_def.rank,
            shape: None,
        };
        let idx = graph.add_node(data);
        node_map.insert(node_def.id.clone(), idx);
    }

    // --- 2. Add edges -----------------------------------------------------------
    let numbered = payload.is_numbered();
    let steps = rdg_schema::resolve_steps(&payload.edges, numbered);
    for (edge_def, step) in payload.edges.iter().zip(steps) {
        let edge_def = payload.effective_edge(edge_def);
        let src = node_map.get(&edge_def.from).copied().ok_or_else(|| {
            anyhow::anyhow!("edge references unknown source node id '{}'", edge_def.from)
        })?;
        let dst = node_map.get(&edge_def.to).copied().ok_or_else(|| {
            anyhow::anyhow!("edge references unknown target node id '{}'", edge_def.to)
        })?;
        // UML draws the generalisation arrow from the child up to its parent: the edge
        // is stored parent -> child (so the layout ranks the parent above) and flagged
        // `reversed`, which the renderers undo when they draw it.
        let style = edge_def.resolved_style();
        let points_up = !is_sequence && style.as_deref().is_some_and(|s| matches!(s.to_ascii_lowercase().as_str(), "inheritance" | "realization" | "generalization" | "extends" | "implements"));
        let (src, dst) = if points_up { (dst, src) } else { (src, dst) };
        let e_idx = graph.add_edge(
            src,
            dst,
            EdgeData {
                label: edge_def.label.clone(),
                edge_style: style,
                reversed: points_up,
                color: edge_def.color.clone(),
                width: edge_def.width,
                line_style: edge_def.line_style.clone(),
                head: edge_def.head.clone(),
                tail: edge_def.tail.clone(),
                source_port: edge_def.source_port.clone(),
                target_port: edge_def.target_port.clone(),
                style_extra: edge_def.style_extra.clone(),
                step,
                constraint: edge_def.constraint.unwrap_or(true),
            },
        );
        edge_order.push(e_idx);
    }

    // --- 2b. Sequence script -----------------------------------------------------
    let sequence = if is_sequence {
        if payload.sequence.is_empty() {
            edge_order
                .iter()
                .map(|&edge| SeqItem::Message { edge, activate: None, deactivate: None, create: false, destroy: false })
                .collect()
        } else {
            if !payload.edges.is_empty() {
                anyhow::bail!("a sequence diagram takes its messages from `sequence:` or `edges:`, not both");
            }
            let mut counter = 0u32;
            compile_steps(&payload.sequence, &node_map, &mut graph, &mut edge_order, numbered, &mut counter)?
        }
    } else {
        Vec::new()
    };

    // --- 3. Cycle detection & breaking (skipped for sequence diagrams) -------
    let had_cycles = if !is_sequence && is_cyclic_directed(&graph) {
        break_cycles(&mut graph);
        true
    } else {
        false
    };

    let group_nodes = group_tree
        .nodes
        .iter()
        .map(|ids| ids.iter().filter_map(|id| node_map.get(id).copied()).collect())
        .collect();
    Ok(CompiledGraph {
        graph,
        node_map,
        had_cycles,
        groups: compiled_groups,
        group_tree,
        group_nodes,
        title: payload.title.clone(),
        description: payload.description.clone(),
        diagram_type,
        edge_order,
        legend: payload.legend,
        same_rank: payload.same_rank.clone(),
        sequence,
    })
}

/// Compile sequence steps, adding each message as a graph edge (in order).
fn compile_steps(
    steps: &[rdg_schema::SeqStep],
    node_map: &HashMap<String, NodeIndex>,
    graph: &mut StableDiGraph<NodeData, EdgeData>,
    edge_order: &mut Vec<EdgeIndex>,
    numbered: bool,
    counter: &mut u32,
) -> Result<Vec<SeqItem>> {
    use rdg_schema::SeqStepKind;
    let node = |id: &str| node_map.get(id).copied().ok_or_else(|| anyhow::anyhow!("sequence step references unknown participant '{id}'"));
    let mut out = Vec::with_capacity(steps.len());
    for step in steps {
        match step.kind().map_err(|e| anyhow::anyhow!(e))? {
            SeqStepKind::Message { from, to } => {
                let (src, dst) = (node(from)?, node(to)?);
                *counter += 1;
                let edge = graph.add_edge(
                    src,
                    dst,
                    EdgeData {
                        label: step.label.clone(),
                        edge_style: step.edge_style.as_ref().map(|s| s.to_ascii_lowercase()),
                        reversed: false,
                        color: step.color.clone(),
                        width: None,
                        line_style: None,
                        head: None,
                        tail: None,
                        source_port: None,
                        target_port: None,
                        style_extra: None,
                        step: numbered.then(|| counter.to_string()),
                        constraint: true,
                    },
                );
                edge_order.push(edge);
                out.push(SeqItem::Message {
                    edge,
                    activate: step.activate,
                    deactivate: step.deactivate,
                    create: step.create.unwrap_or(false),
                    destroy: step.destroy.unwrap_or(false),
                });
            }
            SeqStepKind::Note => {
                let (placement, ids): (NotePlacement, Vec<&str>) = if let Some(l) = step.left_of.as_deref() {
                    (NotePlacement::LeftOf, vec![l])
                } else if let Some(r) = step.right_of.as_deref() {
                    (NotePlacement::RightOf, vec![r])
                } else if !step.over.is_empty() {
                    (NotePlacement::Over, step.over.iter().map(String::as_str).collect())
                } else {
                    anyhow::bail!("a note needs `over`, `left_of` or `right_of`");
                };
                let over = ids.into_iter().map(node).collect::<Result<Vec<_>>>()?;
                out.push(SeqItem::Note { text: step.note.clone().unwrap_or_default(), placement, over });
            }
            SeqStepKind::Fragment { kind, label } => {
                let mut sections = vec![(Some(label.to_string()).filter(|l| !l.is_empty()), compile_steps(&step.steps, node_map, graph, edge_order, numbered, counter)?)];
                for b in step.else_.iter().chain(&step.and) {
                    sections.push((b.label.clone(), compile_steps(&b.steps, node_map, graph, edge_order, numbered, counter)?));
                }
                out.push(SeqItem::Fragment { kind: kind.to_string(), sections });
            }
            SeqStepKind::Divider(label) => out.push(SeqItem::Divider(label.to_string())),
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Eades-Lin-Smyth (Greedy FAS) Cycle Breaking
// ---------------------------------------------------------------------------

/// Reverse back-edges using the Eades-Lin-Smyth heuristic (1993) to make the graph a DAG.
///
/// ELS guarantees an O(V + E) runtime and preserves the primary flow of the graph
/// much better than arbitrary DFS back-edge reversal.
fn break_cycles(graph: &mut StableDiGraph<NodeData, EdgeData>) {
    // Depth-first search in declaration order (nodes as listed, each node's edges as
    // listed): an edge back to a node still on the search stack closes a cycle and is
    // reversed. Authors list a flow in its reading order, so this reverses the edge
    // that *returns* ("retry" back to "run tests"), never the forward step — a
    // degree-based feedback-arc heuristic can pick either, and once put a retry
    // decision at the top of the chart it belonged at the bottom of. Deterministic:
    // only declaration order is consulted (no hash-map iteration).
    /// A node on the search stack with its out-edges (declaration order) and how many
    /// of them were taken.
    type Frame = (NodeIndex, Vec<(EdgeIndex, NodeIndex)>, usize);
    let nodes: Vec<NodeIndex> = graph.node_indices().collect();
    let mut state: HashMap<NodeIndex, u8> = nodes.iter().map(|&n| (n, 0u8)).collect(); // 0 new, 1 on stack, 2 done
    let out_edges = |g: &StableDiGraph<NodeData, EdgeData>, u: NodeIndex| -> Vec<(EdgeIndex, NodeIndex)> {
        let mut out: Vec<(EdgeIndex, NodeIndex)> = g.edges_directed(u, Direction::Outgoing).map(|e| (e.id(), e.target())).collect();
        out.sort_by_key(|&(e, _)| e);
        out
    };
    let mut back_edges: Vec<(EdgeIndex, NodeIndex, NodeIndex)> = Vec::new();
    for &root in &nodes {
        match state.get_mut(&root) {
            Some(s) if *s == 0 => *s = 1,
            _ => continue,
        }
        let mut stack: Vec<Frame> = vec![(root, out_edges(graph, root), 0)];
        while let Some((u, edges, next)) = stack.last_mut() {
            if *next < edges.len() {
                let (e, v) = edges[*next];
                *next += 1;
                match state.get(&v).copied().unwrap_or(2) {
                    0 => {
                        state.insert(v, 1);
                        let es = out_edges(graph, v);
                        stack.push((v, es, 0));
                    }
                    1 if v != *u => back_edges.push((e, *u, v)),
                    _ => {}
                }
            } else {
                state.insert(*u, 2);
                stack.pop();
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
                    reversed: !data.reversed,
                    color: data.color,
                    width: data.width,
                    line_style: data.line_style,
                    head: data.head,
                    tail: data.tail,
                    source_port: data.source_port,
                    target_port: data.target_port,
                    style_extra: data.style_extra,
                    step: data.step,
                    constraint: data.constraint,
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
    use petgraph::visit::IntoEdgeReferences;
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
    fn test_cycle_breaking_reverses_the_returning_edge() {
        // A retry loop written in reading order: the edge that *returns* (flaky -> test)
        // is the one reversed, never the forward step.
        let yaml = "nodes: [{id: start}, {id: test}, {id: ok}, {id: flaky}, {id: done}]\nedges: [{from: start, to: test}, {from: test, to: ok}, {from: ok, to: flaky}, {from: flaky, to: test}, {from: ok, to: done}]\n";
        let compiled = build_graph(&DiagramPayload::from_yaml(yaml).unwrap()).unwrap();
        assert!(compiled.had_cycles);
        let reversed: Vec<(String, String)> = compiled
            .graph
            .edge_references()
            .filter(|e| e.weight().reversed)
            .map(|e| (compiled.graph[e.source()].id.clone(), compiled.graph[e.target()].id.clone()))
            .collect();
        // Stored test -> flaky (so the layout ranks flaky below test) and drawn flaky -> test.
        assert_eq!(reversed, vec![("test".to_string(), "flaky".to_string())]);
    }

    #[test]
    fn test_inheritance_is_stored_parent_to_child_and_drawn_child_to_parent() {
        let yaml = "nodes: [{id: child, type: class}, {id: base, type: class}]\nedges: [{from: child, to: base, edge_style: inheritance}]\n";
        let compiled = build_graph(&DiagramPayload::from_yaml(yaml).unwrap()).unwrap();
        assert!(!compiled.had_cycles);
        let e = compiled.graph.edge_references().next().unwrap();
        assert_eq!(compiled.graph[e.source()].id, "base");
        assert_eq!(compiled.graph[e.target()].id, "child");
        assert!(e.weight().reversed, "drawn from the child up to its parent");
    }

    #[test]
    fn test_edge_constraint_and_same_rank_reach_the_compiled_graph() {
        let yaml = "same_rank: [[a, c]]\nnodes: [{id: a}, {id: b}, {id: c}]\nedges: [{from: a, to: b, constraint: false}, {from: b, to: c}]\n";
        let compiled = build_graph(&DiagramPayload::from_yaml(yaml).unwrap()).unwrap();
        assert_eq!(compiled.same_rank, vec![vec!["a".to_string(), "c".to_string()]]);
        let flags: Vec<bool> = compiled.edge_order.iter().map(|&e| compiled.graph[e].constraint).collect();
        assert_eq!(flags, vec![false, true]);
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
        let steps: Vec<Option<&str>> = compiled
            .edge_order
            .iter()
            .map(|&idx| compiled.graph[idx].step.as_deref())
            .collect();
        assert_eq!(steps, vec![Some("1"), Some("2")]);
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
        assert_eq!(compiled.graph[compiled.edge_order[0]].step.as_deref(), Some("42"));
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
