//! Output rendering: converts a laid-out graph to draw.io XML or SVG.
//!
//! ## draw.io (`mxfile`) format rules enforced here
//!
//! * Always **uncompressed** — `<mxfile><diagram><mxGraphModel><root>`.
//! * `<root>` always begins with the two mandatory stub cells:
//!   `<mxCell id="0" />` and `<mxCell id="1" parent="0" />`.
//! * Node cells carry `vertex="1"`, edge cells carry `edge="1"` (mutually exclusive).
//! * Coordinates from the layout engine are injected into `<mxGeometry … as="geometry" />`.
//! * Semantic node types are mapped to pre-coded draw.io `style=` strings.

use anyhow::Result;
use quick_xml::{
    events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event},
    Writer,
};
use std::io::Cursor;

use crate::graph::CompiledGraph;
use crate::layout::LayoutResult;

// ---------------------------------------------------------------------------
// Style mapping
// ---------------------------------------------------------------------------

/// Map a semantic node type and theme to a draw.io style string.
///
/// The `theme` parameter is reserved for future expansion (e.g. dark mode).
pub fn style_for_type(node_type: &str, _theme: &str) -> String {
    match node_type {
        "proxy" | "gateway" | "api" => {
            "rounded=1;whiteSpace=wrap;html=1;fillColor=#DAE8FC;strokeColor=#6C8EBF;".to_owned()
        }
        "database" | "db" | "storage" => {
            "shape=mxgraph.flowchart.database;whiteSpace=wrap;html=1;\
             fillColor=#dae8fc;strokeColor=#6c8ebf;"
                .to_owned()
        }
        "server" | "service" | "backend" => {
            "rounded=0;whiteSpace=wrap;html=1;fillColor=#d5e8d4;strokeColor=#82b366;".to_owned()
        }
        "queue" | "broker" | "bus" => {
            "shape=mxgraph.flowchart.queue;whiteSpace=wrap;html=1;\
             fillColor=#fff2cc;strokeColor=#d6b656;"
                .to_owned()
        }
        "cache" | "redis" | "memcache" => {
            "rhombus;whiteSpace=wrap;html=1;fillColor=#f8cecc;strokeColor=#b85450;".to_owned()
        }
        "function" | "lambda" | "faas" => {
            "shape=mxgraph.aws4.lambda;whiteSpace=wrap;html=1;\
             fillColor=#FF9900;strokeColor=#d6b656;"
                .to_owned()
        }
        "client" | "user" | "browser" => {
            "shape=mxgraph.general.person_2;whiteSpace=wrap;html=1;\
             fillColor=#f5f5f5;strokeColor=#666666;fontColor=#333333;"
                .to_owned()
        }
        "decision" | "condition" => {
            "rhombus;whiteSpace=wrap;html=1;fillColor=#fff2cc;strokeColor=#d6b656;".to_owned()
        }
        _ => "rounded=1;whiteSpace=wrap;html=1;".to_owned(),
    }
}

/// Colour used in SVG for a given node type.
fn svg_fill_for_type(node_type: &str) -> &'static str {
    match node_type {
        "proxy" | "gateway" | "api" => "#DAE8FC",
        "database" | "db" | "storage" => "#dae8fc",
        "server" | "service" | "backend" => "#d5e8d4",
        "queue" | "broker" | "bus" => "#fff2cc",
        "cache" | "redis" | "memcache" => "#f8cecc",
        "function" | "lambda" | "faas" => "#FFE6CC",
        "client" | "user" | "browser" => "#f5f5f5",
        _ => "#ffffff",
    }
}

// ---------------------------------------------------------------------------
// draw.io XML renderer
// ---------------------------------------------------------------------------

/// Render the compiled, laid-out graph to an uncompressed draw.io XML string.
///
/// The output can be saved with a `.drawio` extension and opened directly in
/// the draw.io desktop app or <https://app.diagrams.net>.
///
/// # Errors
///
/// Returns an error if XML serialization fails (practically infallible for
/// well-formed inputs).
pub fn render_drawio(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    theme: &str,
) -> Result<String> {
    let mut buf = Vec::with_capacity(4096);
    let mut w = Writer::new_with_indent(Cursor::new(&mut buf), b' ', 2);

    // <?xml version="1.0" encoding="UTF-8"?>
    w.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;

    // <mxfile host="aigraph-compiler" version="1.0">
    let mut mxfile = BytesStart::new("mxfile");
    mxfile.push_attribute(("host", "aigraph-compiler"));
    mxfile.push_attribute(("version", "1.0"));
    w.write_event(Event::Start(mxfile))?;

    // <diagram id="diagram-1" name="Page-1">
    let mut diagram = BytesStart::new("diagram");
    diagram.push_attribute(("id", "diagram-1"));
    diagram.push_attribute(("name", "Page-1"));
    w.write_event(Event::Start(diagram))?;

    // <mxGraphModel …>
    let mut model = BytesStart::new("mxGraphModel");
    model.push_attribute(("dx", "1422"));
    model.push_attribute(("dy", "762"));
    model.push_attribute(("grid", "1"));
    model.push_attribute(("gridSize", "10"));
    model.push_attribute(("guides", "1"));
    model.push_attribute(("tooltips", "1"));
    model.push_attribute(("connect", "1"));
    model.push_attribute(("arrows", "1"));
    model.push_attribute(("fold", "1"));
    model.push_attribute(("page", "1"));
    model.push_attribute(("pageScale", "1"));
    model.push_attribute(("pageWidth", "1169"));
    model.push_attribute(("pageHeight", "827"));
    model.push_attribute(("math", "0"));
    model.push_attribute(("shadow", "0"));
    w.write_event(Event::Start(model))?;

    // <root>
    w.write_event(Event::Start(BytesStart::new("root")))?;

    // Mandatory stub cells — MUST always be present.
    // <mxCell id="0" />
    let mut cell0 = BytesStart::new("mxCell");
    cell0.push_attribute(("id", "0"));
    w.write_event(Event::Empty(cell0))?;

    // <mxCell id="1" parent="0" />
    let mut cell1 = BytesStart::new("mxCell");
    cell1.push_attribute(("id", "1"));
    cell1.push_attribute(("parent", "0"));
    w.write_event(Event::Empty(cell1))?;

    // --- Node cells ---------------------------------------------------------
    for node_idx in compiled.graph.node_indices() {
        let node_data = &compiled.graph[node_idx];
        let nl = match layout.positions.get(&node_idx) {
            Some(p) => p,
            None => continue,
        };
        let style = style_for_type(&node_data.node_type, theme);
        let tooltip = node_data.metadata.as_deref().unwrap_or("");

        let mut cell = BytesStart::new("mxCell");
        cell.push_attribute(("id", node_data.id.as_str()));
        cell.push_attribute(("value", node_data.label.as_str()));
        cell.push_attribute(("style", style.as_str()));
        cell.push_attribute(("vertex", "1"));
        cell.push_attribute(("parent", "1"));
        if !tooltip.is_empty() {
            cell.push_attribute(("tooltip", tooltip));
        }
        w.write_event(Event::Start(cell))?;

        // <mxGeometry x="…" y="…" width="…" height="…" as="geometry" />
        let mut geo = BytesStart::new("mxGeometry");
        geo.push_attribute(("x", nl.x.round().to_string().as_str()));
        geo.push_attribute(("y", nl.y.round().to_string().as_str()));
        geo.push_attribute(("width", nl.width.round().to_string().as_str()));
        geo.push_attribute(("height", nl.height.round().to_string().as_str()));
        geo.push_attribute(("as", "geometry"));
        w.write_event(Event::Empty(geo))?;

        w.write_event(Event::End(BytesEnd::new("mxCell")))?;
    }

    // --- Edge cells ---------------------------------------------------------
    for edge_idx in compiled.graph.edge_indices() {
        let (src, dst) = compiled.graph.edge_endpoints(edge_idx).unwrap();
        let edge_data = &compiled.graph[edge_idx];
        let src_id = &compiled.graph[src].id;
        let dst_id = &compiled.graph[dst].id;
        let edge_id = format!("e_{src_id}_{dst_id}");
        let label = edge_data.label.as_deref().unwrap_or("");

        // If the edge was reversed for cycle breaking, flip source/target back.
        let (render_src, render_dst) = if edge_data.reversed {
            (dst_id.as_str(), src_id.as_str())
        } else {
            (src_id.as_str(), dst_id.as_str())
        };

        let mut cell = BytesStart::new("mxCell");
        cell.push_attribute(("id", edge_id.as_str()));
        cell.push_attribute(("value", label));
        cell.push_attribute(("style", "edgeStyle=orthogonalEdgeStyle;html=1;"));
        cell.push_attribute(("edge", "1"));
        cell.push_attribute(("source", render_src));
        cell.push_attribute(("target", render_dst));
        cell.push_attribute(("parent", "1"));
        w.write_event(Event::Start(cell))?;

        // <mxGeometry relative="1" as="geometry" />
        let mut geo = BytesStart::new("mxGeometry");
        geo.push_attribute(("relative", "1"));
        geo.push_attribute(("as", "geometry"));
        w.write_event(Event::Empty(geo))?;

        w.write_event(Event::End(BytesEnd::new("mxCell")))?;
    }

    w.write_event(Event::End(BytesEnd::new("root")))?;
    w.write_event(Event::End(BytesEnd::new("mxGraphModel")))?;
    w.write_event(Event::End(BytesEnd::new("diagram")))?;
    w.write_event(Event::End(BytesEnd::new("mxfile")))?;

    Ok(String::from_utf8(buf)?)
}

// ---------------------------------------------------------------------------
// SVG renderer
// ---------------------------------------------------------------------------

/// Render the compiled, laid-out graph to an SVG string.
///
/// Uses hand-rolled SVG primitives to avoid bringing in a heavy dependency
/// while still producing well-formed, human-readable output.
///
/// # Errors
///
/// Returns an error if the underlying XML writer fails.
pub fn render_svg(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    _theme: &str,
) -> Result<String> {
    // Compute canvas bounds.
    let margin = 40.0_f64;
    let (max_x, max_y) = layout
        .positions
        .values()
        .fold((0.0_f64, 0.0_f64), |(mx, my), nl| {
            (mx.max(nl.x + nl.width), my.max(nl.y + nl.height))
        });
    let canvas_w = max_x + margin;
    let canvas_h = max_y + margin;

    let mut buf = Vec::with_capacity(4096);
    let mut w = Writer::new_with_indent(Cursor::new(&mut buf), b' ', 2);

    // <?xml version="1.0" encoding="UTF-8"?>
    w.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;

    let mut svg = BytesStart::new("svg");
    svg.push_attribute(("xmlns", "http://www.w3.org/2000/svg"));
    svg.push_attribute(("version", "1.1"));
    svg.push_attribute(("width", canvas_w.to_string().as_str()));
    svg.push_attribute(("height", canvas_h.to_string().as_str()));
    svg.push_attribute((
        "viewBox",
        format!("0 0 {canvas_w} {canvas_h}").as_str(),
    ));
    w.write_event(Event::Start(svg))?;

    // Background
    let mut bg = BytesStart::new("rect");
    bg.push_attribute(("width", "100%"));
    bg.push_attribute(("height", "100%"));
    bg.push_attribute(("fill", "#f8f8f8"));
    w.write_event(Event::Empty(bg))?;

    // --- Draw edges (behind nodes) ------------------------------------------
    for edge_idx in compiled.graph.edge_indices() {
        let (src_idx, dst_idx) = compiled.graph.edge_endpoints(edge_idx).unwrap();
        let edge_data = &compiled.graph[edge_idx];

        let (src_idx, dst_idx) = if edge_data.reversed {
            (dst_idx, src_idx)
        } else {
            (src_idx, dst_idx)
        };

        let (Some(src_nl), Some(dst_nl)) = (
            layout.positions.get(&src_idx),
            layout.positions.get(&dst_idx),
        ) else {
            continue;
        };

        // Centre-bottom to centre-top connector.
        let x1 = src_nl.x + src_nl.width / 2.0;
        let y1 = src_nl.y + src_nl.height;
        let x2 = dst_nl.x + dst_nl.width / 2.0;
        let y2 = dst_nl.y;

        let mut line = BytesStart::new("line");
        line.push_attribute(("x1", x1.to_string().as_str()));
        line.push_attribute(("y1", y1.to_string().as_str()));
        line.push_attribute(("x2", x2.to_string().as_str()));
        line.push_attribute(("y2", y2.to_string().as_str()));
        line.push_attribute(("stroke", "#555555"));
        line.push_attribute(("stroke-width", "1.5"));
        line.push_attribute(("marker-end", "url(#arrow)"));
        w.write_event(Event::Empty(line))?;

        // Edge label
        if let Some(label) = &edge_data.label {
            let lx = (x1 + x2) / 2.0 + 4.0;
            let ly = (y1 + y2) / 2.0;
            let mut text = BytesStart::new("text");
            text.push_attribute(("x", lx.to_string().as_str()));
            text.push_attribute(("y", ly.to_string().as_str()));
            text.push_attribute(("font-family", "sans-serif"));
            text.push_attribute(("font-size", "11"));
            text.push_attribute(("fill", "#333333"));
            w.write_event(Event::Start(text))?;
            w.write_event(Event::Text(BytesText::new(label)))?;
            w.write_event(Event::End(BytesEnd::new("text")))?;
        }
    }

    // --- Draw nodes ---------------------------------------------------------
    for node_idx in compiled.graph.node_indices() {
        let node_data = &compiled.graph[node_idx];
        let Some(nl) = layout.positions.get(&node_idx) else {
            continue;
        };
        let fill = svg_fill_for_type(&node_data.node_type);

        let mut rect = BytesStart::new("rect");
        rect.push_attribute(("x", nl.x.to_string().as_str()));
        rect.push_attribute(("y", nl.y.to_string().as_str()));
        rect.push_attribute(("width", nl.width.to_string().as_str()));
        rect.push_attribute(("height", nl.height.to_string().as_str()));
        rect.push_attribute(("rx", "6"));
        rect.push_attribute(("ry", "6"));
        rect.push_attribute(("fill", fill));
        rect.push_attribute(("stroke", "#555555"));
        rect.push_attribute(("stroke-width", "1.5"));
        w.write_event(Event::Empty(rect))?;

        // Node label (centred)
        let cx = nl.x + nl.width / 2.0;
        let cy = nl.y + nl.height / 2.0 + 4.0; // +4 for font baseline offset
        let mut text = BytesStart::new("text");
        text.push_attribute(("x", cx.to_string().as_str()));
        text.push_attribute(("y", cy.to_string().as_str()));
        text.push_attribute(("text-anchor", "middle"));
        text.push_attribute(("font-family", "sans-serif"));
        text.push_attribute(("font-size", "13"));
        text.push_attribute(("fill", "#1a1a1a"));
        w.write_event(Event::Start(text))?;
        w.write_event(Event::Text(BytesText::new(&node_data.label)))?;
        w.write_event(Event::End(BytesEnd::new("text")))?;
    }

    w.write_event(Event::End(BytesEnd::new("svg")))?;
    Ok(String::from_utf8(buf)?)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::build_graph;
    use crate::layout::{compute_layout, LayoutConfig};
    use crate::schema::{DiagramPayload, EdgeDef, NodeDef};

    fn one_node_payload() -> DiagramPayload {
        DiagramPayload {
            diagram_type: "flowchart".to_owned(),
            theme: None,
            nodes: vec![NodeDef {
                id: "n1".to_owned(),
                label: "API Gateway".to_owned(),
                node_type: "proxy".to_owned(),
                metadata: Some("Routes all traffic".to_owned()),
            }],
            edges: vec![],
        }
    }

    fn two_node_payload() -> DiagramPayload {
        DiagramPayload {
            diagram_type: "flowchart".to_owned(),
            theme: None,
            nodes: vec![
                NodeDef {
                    id: "n1".to_owned(),
                    label: "API Gateway".to_owned(),
                    node_type: "proxy".to_owned(),
                    metadata: None,
                },
                NodeDef {
                    id: "n2".to_owned(),
                    label: "User DB".to_owned(),
                    node_type: "database".to_owned(),
                    metadata: None,
                },
            ],
            edges: vec![EdgeDef {
                from: "n1".to_owned(),
                to: "n2".to_owned(),
                label: Some("queries".to_owned()),
            }],
        }
    }

    #[test]
    fn test_drawio_has_mandatory_root_cells() {
        let payload = one_node_payload();
        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let xml = render_drawio(&compiled, &layout, "standard").unwrap();

        assert!(xml.contains("id=\"0\""), "must contain cell with id=\"0\"");
        assert!(
            xml.contains("id=\"1\" parent=\"0\""),
            "must contain cell with id=\"1\" parent=\"0\""
        );
    }

    #[test]
    fn test_drawio_node_has_vertex_attribute() {
        let payload = one_node_payload();
        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let xml = render_drawio(&compiled, &layout, "standard").unwrap();
        assert!(xml.contains("vertex=\"1\""), "nodes must carry vertex=\"1\"");
    }

    #[test]
    fn test_drawio_edge_has_edge_attribute() {
        let payload = two_node_payload();
        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let xml = render_drawio(&compiled, &layout, "standard").unwrap();
        assert!(xml.contains("edge=\"1\""), "edges must carry edge=\"1\"");
    }

    #[test]
    fn test_drawio_no_vertex_on_edges() {
        let payload = two_node_payload();
        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let xml = render_drawio(&compiled, &layout, "standard").unwrap();
        // edge cells must not have vertex="1" on the same cell — verify by
        // checking that no single mxCell element has both attributes.
        // A simple heuristic: count occurrences.
        let edge_count = xml.matches("edge=\"1\"").count();
        let vertex_count = xml.matches("vertex=\"1\"").count();
        assert_eq!(
            edge_count, 1,
            "expected exactly 1 edge cell"
        );
        assert_eq!(
            vertex_count, 2,
            "expected exactly 2 vertex cells (mandatory + node)"
        );
    }

    #[test]
    fn test_style_mapping_database() {
        let style = style_for_type("database", "standard");
        assert!(
            style.contains("database"),
            "database style must reference 'database'"
        );
    }

    #[test]
    fn test_style_mapping_default() {
        let style = style_for_type("unknown_widget", "standard");
        assert!(!style.is_empty(), "default style must not be empty");
        assert!(style.contains("rounded"), "default style should use rounded");
    }

    #[test]
    fn test_svg_output_is_valid_xml() {
        let payload = two_node_payload();
        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let svg = render_svg(&compiled, &layout, "standard").unwrap();
        assert!(svg.starts_with("<?xml"), "SVG must start with XML declaration");
        assert!(svg.contains("<svg"), "must contain svg element");
        assert!(svg.contains("</svg>"), "must close svg element");
    }
}
