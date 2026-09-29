//! Layout engine: maps a compiled petgraph to 2D Cartesian coordinates.
//!
//! A deterministic, pure-Rust layered layout: topological ranking, barycentric
//! crossing minimisation, and a compound 2D grid placement for visual groups.

mod compound;
mod fcose;
mod force;
mod gaps;
pub mod groups;
mod hillclimb;
mod physics;
mod sequence;
mod sugiyama;
mod tokens;
mod wrap;

pub use fcose::compute_fcose_layout;
pub use force::compute_force_layout;
pub use hillclimb::{refine as refine_positions, request_extra_clearance};
pub use sequence::{
    SeqActivation, SeqDivider, SeqFragment, SeqGroupBox, SeqLifeline, SeqNote, SequenceLayoutInfo, SequenceMessageLayout,
    actor_figure_h, compute_sequence_layout, message_label_lines,
};
pub use tokens::DesignTokens;
pub use wrap::{
    estimate_node_box, estimate_node_size, estimate_node_size_with_details, estimate_node_size_with_fields,
    explicit_lines, marker_shape, node_lines, wrap_chars_for, LabelLine, SUBTITLE_WRAP_FACTOR, classify_label,
    strip_markdown_tokens, wrap_label,
};

use anyhow::Result;
use petgraph::stable_graph::NodeIndex;
use petgraph::visit::EdgeRef;
use std::collections::HashMap;

use rdg_graph::CompiledGraph;

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
///
/// `rank_spacing`/`node_spacing`/`node_width`/`node_height`/`margin_x`/`margin_y`/
/// `group_gap_x`/`group_gap_y` are the values a diagram author routinely wants to
/// override per-diagram (already exposed via CLI flags and the YAML `spacing:`
/// block) and so keep their own plain fields, defaulted from [`DesignTokens`] but
/// independently settable. Every other design/threshold value used anywhere in the
/// layout or render pipeline — marker sizes, overlap clearance, retry budgets,
/// dispatch thresholds, and so on — lives on [`Self::tokens`] instead of getting its
/// own field here, so adding a new tunable value never means widening this struct
/// (and every call site that constructs one) again.
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
    /// Outer canvas margin, X axis, in pixels.
    pub margin_x: f64,
    /// Outer canvas margin, Y axis, in pixels.
    pub margin_y: f64,
    /// Horizontal gap between adjacent group containers, in pixels.
    pub group_gap_x: f64,
    /// Vertical gap between adjacent group containers, in pixels.
    pub group_gap_y: f64,
    /// The base design tokens every other spacing/sizing/threshold decision in the
    /// pipeline derives from. See [`DesignTokens`]'s own docs for what's in here and
    /// why it isn't just more fields on this struct.
    pub tokens: DesignTokens,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        let tokens = DesignTokens::default();
        Self {
            rank_spacing: 52,
            node_spacing: 36,
            node_width: 120.0,
            node_height: 44.0,
            direction: LayoutDirection::TopToBottom,
            margin_x: tokens.margin_x(),
            margin_y: tokens.margin_y(),
            group_gap_x: tokens.group_gap(),
            group_gap_y: tokens.group_gap(),
            tokens,
        }
    }
}

/// Computed spatial positions for every node in the graph.
pub struct LayoutResult {
    /// Maps petgraph [`NodeIndex`] to its computed 2D layout box.
    pub positions: HashMap<NodeIndex, NodeLayout>,
    /// Optional sequence diagram layout metadata (when diagram_type == "sequence").
    pub sequence_info: Option<SequenceLayoutInfo>,
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
            sequence_info: None,
        });
    }

    if compiled.diagram_type == "sequence" {
        return compute_sequence_layout(compiled, config);
    }

    let mut result = if compiled.groups.is_empty() {
        sugiyama::layout_topological(compiled, config)?
    } else {
        compound::layout_compound(compiled, config)?
    };

    normalize_positions(&mut result, config, !compiled.groups.is_empty());

    Ok(result)
}

/// Translate every position so the layout's top-left corner sits at the configured
/// canvas margin (plus group-container padding and title banner space, when
/// applicable), instead of wherever the engine's own coordinate origin happened to
/// land. Shared by every layout engine so none of them need to reason about margins
/// themselves — they can place nodes anywhere (including negative coordinates, as the
/// force-directed and fCoSE engines naturally do) and let this pass clean it up.
pub(crate) fn normalize_positions(result: &mut LayoutResult, config: &LayoutConfig, has_groups: bool) {
    if result.positions.is_empty() {
        return;
    }

    let min_x = result.positions.values().map(|nl| nl.x).fold(f64::MAX, f64::min);
    let min_y = result.positions.values().map(|nl| nl.y).fold(f64::MAX, f64::min);
    let max_x = result.positions.values().map(|nl| nl.x + nl.width).fold(f64::MIN, f64::max);
    let max_y = result.positions.values().map(|nl| nl.y + nl.height).fold(f64::MIN, f64::max);

    // (The title is placed after routing, into the diagram's white space — render-core's
    // `frame` — so nothing is reserved for it here.)
    // A group's drawn padding is content-aware (`group_pad_for`) and never exceeds the
    // padding the *whole* diagram's bounding box would earn, so reserving that bound
    // guarantees no group container is drawn past the canvas margin. `finalize_canvas`
    // (render-core) later trims the reservation to the exact drawn extent.
    let (pad_x, pad_top) = if has_groups {
        let (w, h) = (max_x - min_x, max_y - min_y);
        (config.tokens.group_pad_for(w, h), config.tokens.group_pad_top_for(w, h))
    } else {
        (0.0, 0.0)
    };
    let target_min_x = config.margin_x + pad_x;
    let target_min_y = config.margin_y + pad_top;

    let dx = target_min_x - min_x;
    let dy = target_min_y - min_y;

    for nl in result.positions.values_mut() {
        nl.x += dx;
        nl.y += dy;
    }
}

/// Box size for one node as the layered layouts place it: the content-based estimate
/// (label, type, fields, technology, icon), grown — never shrunk — so that its faces can
/// hold the node's connections at [`DesignTokens::min_port_pitch`]. Explicit
/// `width`/`height` overrides are respected exactly.
///
/// A hub with a dozen edges on a 44px-tall box gets ports ~10px apart whichever face the
/// router picks, which no amount of routing cleverness turns back into readable arrows.
/// Connections are assumed to spread over the faces roughly evenly, so the box must fit
/// a third of them along each side (height) and half along the flow axis (width).
pub(crate) fn layout_node_size(
    compiled: &CompiledGraph,
    idx: NodeIndex,
    config: &LayoutConfig,
) -> (f64, f64) {
    let data = &compiled.graph[idx];
    // A mark with its caption underneath: a logo node (the logo in its halo) or a
    // flowchart marker. The box holds both, so groups, routing and labels all make room
    // for the caption; arrows attach to the mark (see `rdg_render_core::style::Outline`).
    if let Some(mark) = caption_mark(data, &config.tokens) {
        let (w, h) = captioned_size(data, mark, &config.tokens);
        return grow_for_degree(compiled, idx, (w, h), config);
    }
    // Only `icon` is drawn (a node's language can be shown on its group instead).
    let (w, h) = estimate_node_box(data, data.icon.is_some(), config.node_width, config.node_height, &config.tokens);
    grow_for_degree(compiled, idx, (w, h), config)
}

/// Size of the mark a node draws above its caption — a logo node's halo, or a
/// flowchart marker's shape — or `None` for a node whose text sits inside its shape.
pub fn caption_mark(nd: &rdg_graph::NodeData, tokens: &DesignTokens) -> Option<f64> {
    match nd.shape.as_deref().or_else(|| marker_shape(&nd.node_type))? {
        "icon" => Some(tokens.icon_halo_size()),
        "start" => Some(tokens.start_marker_size()),
        "end" => Some(tokens.end_marker_size()),
        "choice" => Some(tokens.choice_marker_size()),
        _ => None,
    }
}

/// Gap between a mark and the caption under it.
pub fn caption_gap(tokens: &DesignTokens) -> f64 {
    tokens.px(0.25)
}

/// Box for a `mark`-sized shape with the node's caption lines (and technology line)
/// underneath, rounded up to 4px.
fn captioned_size(data: &rdg_graph::NodeData, mark: f64, t: &DesignTokens) -> (f64, f64) {
    let lines = node_lines(data, t.wrap_chars_normal);
    let font = |l: &LabelLine| if l.is_subtitle { t.detail_font_size } else { t.font_size };
    let mut text_w = lines.iter().map(|l| strip_markdown_tokens(&l.text).chars().count() as f64 * t.char_width(font(l))).fold(0.0, f64::max);
    let mut text_h: f64 = lines.iter().map(|l| t.line_height(font(l))).sum();
    if let Some(tech) = data.technology.as_deref().filter(|tech| !data.label.contains(tech)) {
        text_w = text_w.max((tech.chars().count() + 2) as f64 * t.char_width(t.detail_font_size));
        text_h += t.line_height(t.detail_font_size);
    }
    let w = (text_w + t.px(1.0)).max(mark + t.px(1.0));
    let h = mark + caption_gap(t) + text_h;
    ((w / 4.0).ceil() * 4.0, (h / 4.0).ceil() * 4.0)
}

/// See [`layout_node_size`]. Rounded up to a multiple of 4 like the estimator's output.
pub(crate) fn grow_for_degree(
    compiled: &CompiledGraph,
    idx: NodeIndex,
    (w, h): (f64, f64),
    config: &LayoutConfig,
) -> (f64, f64) {
    let data = &compiled.graph[idx];
    let degree = compiled
        .graph
        .edges_directed(idx, petgraph::Direction::Incoming)
        .chain(compiled.graph.edges_directed(idx, petgraph::Direction::Outgoing))
        .filter(|e| e.source() != e.target())
        .count();
    // n ports over 70% of a face sit `0.7 * len / (n + 1)` apart.
    let len_for = |n: usize| ((n + 1) as f64 * config.tokens.min_port_pitch() / 0.7 / 4.0).ceil() * 4.0;
    let new_h = if data.height.is_some() { h } else { h.max(len_for(degree.div_ceil(3))) };
    let new_w = if data.width.is_some() { w } else { w.max(len_for(degree.div_ceil(2))) };
    (new_w, new_h)
}

/// The ideal edge length the force-directed and fCoSE engines relax toward. Node
/// spacing/rank spacing alone (as used by `sugiyama`, which places boxes edge-to-edge
/// plus a gap) badly understates the actual center-to-center distance connected nodes
/// need once a box's own size is real — a diagram whose boxes are ~120x50px still ends
/// up with next to no clearance between adjacent boxes at an "ideal length" of ~80px,
/// leaving no room for edges to route through at all. Anchoring to the average node
/// diagonal keeps equilibrium spacing proportional to how big the boxes actually are.
pub(crate) fn ideal_edge_length(sizes: &HashMap<NodeIndex, (f64, f64)>, config: &LayoutConfig) -> f64 {
    let avg_diag = if sizes.is_empty() {
        0.0
    } else {
        // `sizes` is a `HashMap`, whose iteration order is randomized per *process* —
        // summing in that order is not associative for floats, so this average (which
        // feeds `ideal_len`, and through it the spectral MDS draft, spiral seed, and
        // every physics constant derived from it) could come out a rounding hair
        // different across runs of the exact same binary on the exact same YAML.
        // Downstream, that's enough to flip which side of a near-tied eigenvalue the
        // MDS draft lands on, cascading into a visibly different final layout —
        // confirmed directly: `rdg`'s own "same input always produces the same
        // output" guarantee broke on a real sample, several runs each producing a
        // distinct result. Sorting by `NodeIndex` first fixes the summation order
        // regardless of hash seed.
        let mut keys: Vec<&NodeIndex> = sizes.keys().collect();
        keys.sort_by_key(|n| n.index());
        keys.iter().map(|&&k| {
            let (w, h) = sizes[&k];
            (w * w + h * h).sqrt()
        }).sum::<f64>() / sizes.len() as f64
    };
    (avg_diag + config.node_spacing as f64 + config.rank_spacing as f64).max(config.tokens.px(12.5))
}

/// Every node's box size up front ([`layout_node_size`]: captions, icons, explicit
/// `width`/`height`, degree). Shared by the force-directed and fCoSE engines, which (unlike
/// `sugiyama`/`compound`) need every node's final size before simulation starts
/// rather than incrementally per-rank.
pub(crate) fn compute_node_sizes(
    compiled: &CompiledGraph,
    config: &LayoutConfig,
) -> HashMap<NodeIndex, (f64, f64)> {
    compiled
        .graph
        .node_indices()
        .map(|idx| (idx, layout_node_size(compiled, idx, config)))
        .collect()
}

/// This diagram's own average node box size, using the same sizing estimator the
/// layout engines use internally. Lets a caller (the CLI, computing a proportional
/// default for `rank_spacing`/`node_spacing` before layout has produced real
/// per-rank sizes) reason about "how big are this diagram's boxes, roughly" without
/// duplicating the estimation logic itself.
pub fn estimate_average_node_size(
    compiled: &CompiledGraph,
    node_width: f64,
    node_height: f64,
    tokens: &DesignTokens,
) -> (f64, f64) {
    let mut total_w = 0.0;
    let mut total_h = 0.0;
    let mut count = 0usize;
    for idx in compiled.graph.node_indices() {
        let (w, h) = estimate_node_box(&compiled.graph[idx], false, node_width, node_height, tokens);
        total_w += w;
        total_h += h;
        count += 1;
    }
    if count == 0 {
        (node_width, node_height)
    } else {
        (total_w / count as f64, total_h / count as f64)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rdg_graph::build_graph;
    use rdg_schema::{DiagramPayload, EdgeDef, NodeDef};

    fn two_node_payload() -> DiagramPayload {
        DiagramPayload {
            diagram_type: "flowchart".to_owned(),
            nodes: vec![
                NodeDef {
                    id: "n1".to_owned(),
                    label: "Source".to_owned(),
                    ..Default::default()
                },
                NodeDef {
                    id: "n2".to_owned(),
                    label: "Sink".to_owned(),
                    ..Default::default()
                },
            ],
            edges: vec![EdgeDef {
                from: "n1".to_owned(),
                to: "n2".to_owned(),
                label: Some("connects".to_owned()),
                ..Default::default()
            }],
            ..Default::default()
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
        let payload = DiagramPayload::default();
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
        let tokens = DesignTokens::default();
        let (short_w, short_h) = estimate_node_size("API", "default", 120.0, 50.0, &tokens);
        let (long_w, long_h) = estimate_node_size(
            "Extremely Long Microservice Component Name Across Architecture",
            "default",
            120.0,
            50.0,
            &tokens,
        );
        assert!(long_w > short_w, "longer text must produce wider node");
        assert!(
            long_h > short_h,
            "multiline wrapped text must produce taller node"
        );
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
        assert!(
            n2_pos.x > n1_pos.x,
            "target node should be placed to the right of source"
        );
    }

    #[test]
    fn test_compound_layout_no_overlap_and_aspect_ratio() {
        use rdg_schema::GroupDef;

        let payload = DiagramPayload {
            diagram_type: "flowchart".to_owned(),
            groups: vec![
                GroupDef {
                    id: "g1".to_owned(),
                    label: "Group 1".to_owned(),
                    color: None,
                    nodes: vec!["n1".to_owned(), "n2".to_owned()],
                    ..Default::default()
                },
                GroupDef {
                    id: "g2".to_owned(),
                    label: "Group 2".to_owned(),
                    color: None,
                    nodes: vec!["n3".to_owned()],
                    ..Default::default()
                },
                GroupDef {
                    id: "g3".to_owned(),
                    label: "Group 3".to_owned(),
                    color: None,
                    nodes: vec!["n4".to_owned()],
                    ..Default::default()
                },
            ],
            nodes: vec![
                NodeDef {
                    id: "n1".to_owned(),
                    label: "Node 1".to_owned(),
                    node_type: "server".to_owned(),
                    ..Default::default()
                },
                NodeDef {
                    id: "n2".to_owned(),
                    label: "Node 2".to_owned(),
                    node_type: "server".to_owned(),
                    ..Default::default()
                },
                NodeDef {
                    id: "n3".to_owned(),
                    label: "Node 3".to_owned(),
                    node_type: "server".to_owned(),
                    ..Default::default()
                },
                NodeDef {
                    id: "n4".to_owned(),
                    label: "Node 4".to_owned(),
                    node_type: "server".to_owned(),
                    ..Default::default()
                },
            ],
            edges: vec![
                EdgeDef {
                    from: "n1".to_owned(),
                    to: "n2".to_owned(),
                    ..Default::default()
                },
                EdgeDef {
                    from: "n2".to_owned(),
                    to: "n3".to_owned(),
                    ..Default::default()
                },
                EdgeDef {
                    from: "n3".to_owned(),
                    to: "n4".to_owned(),
                    ..Default::default()
                },
            ],
            ..Default::default()
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
            let gx = min_x - config.tokens.group_pad();
            let gy = min_y - config.tokens.group_pad_top();
            let gw = (max_x - min_x) + 2.0 * config.tokens.group_pad();
            let gh = (max_y - min_y) + config.tokens.group_pad_top() + config.tokens.group_pad();
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

    #[test]
    fn test_sequence_layout_computes_lifelines_and_messages() {
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
    label: "GET /"
  - from: server
    to: client
    label: "200 OK"
    style: reply
"#;
        let payload = DiagramPayload::from_yaml(seq_yaml).unwrap();
        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        assert!(layout.sequence_info.is_some());
        let seq = layout.sequence_info.unwrap();
        assert_eq!(seq.lifeline_x.len(), 2);
        assert_eq!(seq.messages.len(), 2);
        assert!(
            seq.messages[0].y < seq.messages[1].y,
            "first message must be placed above second message"
        );
        assert!(seq.messages[1].is_reply);
        assert!(seq.lifeline_bottom_y > seq.messages[1].y);
    }

    #[test]
    fn test_table_sizing_with_fields() {
        let fields = vec![
            "id: uuid [PK]".to_string(),
            "email: varchar(255) [UQ]".to_string(),
            "created_at: timestamp".to_string(),
        ];
        let tokens = DesignTokens::default();
        let (w, h) = estimate_node_size_with_fields(
            "users", "table", &fields, 120.0, 50.0, None, None, &tokens,
        );
        assert!(w >= tokens.px(20.0));
        assert!(h >= tokens.line_height(tokens.font_size) * 2.0 + 3.0 * tokens.line_height(tokens.font_size));
    }

    #[test]
    fn test_explicit_size_override_skips_estimation() {
        let (w, h) = estimate_node_size_with_fields(
            "x",
            "default",
            &[],
            120.0,
            50.0,
            Some(500.0),
            Some(9.0),
            &DesignTokens::default(),
        );
        assert_eq!(w, 500.0);
        // Explicit height below the 10px floor is still clamped, not silently accepted.
        assert_eq!(h, 10.0);
    }

    // -----------------------------------------------------------------------
    // Phase G: edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_single_node_no_edges_gets_a_position() {
        let payload = DiagramPayload {
            nodes: vec![NodeDef {
                id: "solo".into(),
                label: "Solo".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let compiled = build_graph(&payload).unwrap();
        let result = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        assert_eq!(result.positions.len(), 1);
    }

    #[test]
    fn test_unbroken_long_word_sizing_does_not_panic() {
        let unbroken = "a".repeat(2000);
        let (w, h) = estimate_node_size(&unbroken, "default", 120.0, 50.0, &DesignTokens::default());
        assert!(w > 0.0 && w.is_finite());
        assert!(h > 0.0 && h.is_finite());
    }

    #[test]
    fn test_custom_margin_and_group_gap_are_respected() {
        let payload = two_node_payload();
        let compiled = build_graph(&payload).unwrap();
        let config = LayoutConfig {
            margin_x: 200.0,
            margin_y: 300.0,
            ..LayoutConfig::default()
        };
        let result = compute_layout(&compiled, &config).unwrap();
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
        assert!(
            (min_x - 200.0).abs() < 0.5,
            "min_x should sit at the configured margin: {min_x}"
        );
        assert!(
            (min_y - 300.0).abs() < 0.5,
            "min_y should sit at the configured margin: {min_y}"
        );
    }

    /// Three levels of nesting plus a sibling group and an ungrouped node, wired
    /// across the levels.
    const NESTED: &str = r#"
nodes:
  - {id: web, label: Web app}
  - {id: admin, label: Admin}
  - {id: rails, label: Rails monolith}
  - {id: legacy, label: Legacy handlers}
  - {id: bff, label: payments-bff}
  - {id: core, title: payments-core-service}
  - {id: bank, title: payments-bank-connector-service}
  - {id: partners, label: Bank partners}
  - {id: done, type: end, title: Settled, subtitle: ledger updated}
groups:
  - {id: clients, label: Clients & ops, nodes: [web, admin]}
  - {id: payments, label: Payments services, nodes: [monolith, bff, core, inner]}
  - {id: monolith, label: Rails monolith, nodes: [rails, legacy]}
  - {id: inner, label: Money movement, parent: payments, nodes: [bank, done]}
edges:
  - {from: web, to: rails}
  - {from: rails, to: legacy}
  - {from: rails, to: bff}
  - {from: bff, to: core}
  - {from: core, to: bank}
  - {from: bank, to: partners}
  - {from: partners, to: bank}
  - {from: bank, to: done}
  - {from: admin, to: core}
"#;

    fn assert_groups_contain_their_members(direction: LayoutDirection) {
        let compiled = build_graph(&DiagramPayload::from_yaml(NESTED).unwrap()).unwrap();
        let config = LayoutConfig { direction, ..LayoutConfig::default() };
        let layout = compute_layout(&compiled, &config).unwrap();
        let rects = groups::group_rects(&compiled, &layout.positions, &config.tokens);
        let inside = |(x, y, w, h): groups::Rect, (ox, oy, ow, oh): groups::Rect| x >= ox - 0.5 && y >= oy - 0.5 && x + w <= ox + ow + 0.5 && y + h <= oy + oh + 0.5;
        let overlap = |(x, y, w, h): groups::Rect, (ox, oy, ow, oh): groups::Rect| x < ox + ow && ox < x + w && y < oy + oh && oy < y + h;
        let node_rect = |n: &NodeIndex| {
            let nl = &layout.positions[n];
            (nl.x, nl.y, nl.width, nl.height)
        };
        let tree = &compiled.group_tree;
        for (g, r) in rects.iter().enumerate() {
            let r = r.expect("every group holds something");
            for n in compiled.nodes_within(g) {
                assert!(inside(node_rect(&n), r), "{direction:?}: node {} outside group {}", compiled.graph[n].id, compiled.groups[g].id);
            }
            for c in tree.children(g) {
                assert!(inside(rects[c].unwrap(), r), "{direction:?}: group {} outside {}", compiled.groups[c].id, compiled.groups[g].id);
            }
            // Nothing else may sit on or inside this box.
            for n in compiled.graph.node_indices().filter(|n| !compiled.nodes_within(g).contains(n)) {
                assert!(!overlap(node_rect(&n), r), "{direction:?}: node {} overlaps group {}", compiled.graph[n].id, compiled.groups[g].id);
            }
            for o in (0..rects.len()).filter(|&o| o != g && !tree.contains(g, o) && !tree.contains(o, g)) {
                assert!(!overlap(rects[o].unwrap(), r), "{direction:?}: groups {} and {} overlap", compiled.groups[o].id, compiled.groups[g].id);
            }
        }
    }

    #[test]
    fn test_nested_groups_contain_members_and_never_overlap_top_to_bottom() {
        assert_groups_contain_their_members(LayoutDirection::TopToBottom);
    }

    #[test]
    fn test_nested_groups_contain_members_and_never_overlap_left_to_right() {
        assert_groups_contain_their_members(LayoutDirection::LeftToRight);
    }

    #[test]
    fn test_marker_caption_is_part_of_its_box() {
        let compiled = build_graph(&DiagramPayload::from_yaml(NESTED).unwrap()).unwrap();
        let config = LayoutConfig::default();
        let layout = compute_layout(&compiled, &config).unwrap();
        let done = &layout.positions[&compiled.node_map["done"]];
        let mark = config.tokens.end_marker_size();
        assert!(done.height > mark + 2.0 * config.tokens.line_height(config.tokens.detail_font_size), "caption below the mark: {done:?}");
        assert!(done.width > mark, "as wide as the caption: {done:?}");
    }

    #[test]
    fn test_group_is_never_narrower_than_its_title() {
        let yaml = "nodes: [{id: a, label: A}]\ngroups: [{id: g, label: A very long group title indeed, nodes: [a]}]\n";
        let compiled = build_graph(&DiagramPayload::from_yaml(yaml).unwrap()).unwrap();
        let config = LayoutConfig::default();
        let layout = compute_layout(&compiled, &config).unwrap();
        let (x, _, w, _) = groups::group_rects(&compiled, &layout.positions, &config.tokens)[0].unwrap();
        let t = &config.tokens;
        let title_end = x + groups::group_title_inset(t).0 + 31.0 * t.char_width(t.group_title_font_size);
        let a = &layout.positions[&compiled.node_map["a"]];
        assert!(x + w >= title_end, "box ends before its title");
        assert!(a.x + a.width / 2.0 > title_end, "the lone node's centre clears the title, so an arrow can drop onto it");
    }
}
