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
    Writer,
    events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event},
};
use std::collections::HashMap;
use std::io::Cursor;

use crate::graph::CompiledGraph;
use crate::layout::LayoutResult;

// ---------------------------------------------------------------------------
// Style mapping
// ---------------------------------------------------------------------------

/// Map a semantic node type and theme to a draw.io style string.
///
/// Supports `"dark"` theme as well as semantic node type styling using the
/// white-card paradigm (white fill, drop shadow, absolute 8px arc, colored border).
#[allow(clippy::useless_format)]
pub fn style_for_type(node_type: &str, theme: &str) -> String {
    // Base shared by every node
    let base = "rounded=1;absoluteArcSize=1;arcSize=8;\
                whiteSpace=wrap;html=1;fillColor=#ffffff;shadow=1;\
                strokeWidth=1.5;\
                fontFamily=Inter,Helvetica,sans-serif;\
                fontSize=12;fontStyle=1;fontColor=#0f172a;\
                spacingTop=6;spacingBottom=6;spacingLeft=8;spacingRight=8;";

    if theme == "dark" {
        return format!("{base}fillColor=#1e293b;fontColor=#f1f5f9;strokeColor=#475569;");
    }

    match node_type.to_ascii_lowercase().as_str() {
        "proxy" | "gateway" | "api" => format!("{base}strokeColor=#818cf8;"),
        "server" | "service" | "backend" => format!("{base}strokeColor=#34d399;"),
        "database" | "db" | "storage" => {
            format!(
                "shape=cylinder3;boundedLbl=1;backgroundOutline=1;\
                     whiteSpace=wrap;html=1;fillColor=#ffffff;shadow=1;\
                     strokeWidth=1.5;strokeColor=#38bdf8;\
                     fontFamily=Inter,Helvetica,sans-serif;\
                     fontSize=12;fontStyle=1;fontColor=#0f172a;\
                     spacingTop=6;spacingBottom=6;"
            )
        }
        "queue" | "broker" | "bus" => {
            format!(
                "shape=mxgraph.flowchart.start_2;\
                     perimeter=mxPerimeter.ellipsePerimeter;\
                     whiteSpace=wrap;html=1;fillColor=#ffffff;shadow=1;\
                     strokeWidth=1.5;strokeColor=#fbbf24;\
                     fontFamily=Inter,Helvetica,sans-serif;\
                     fontSize=12;fontStyle=1;fontColor=#0f172a;"
            )
        }
        "cache" | "redis" | "memcache" => format!("{base}strokeColor=#f87171;"),
        "function" | "lambda" | "faas" => {
            format!(
                "shape=mxgraph.aws4.lambda;\
                     whiteSpace=wrap;html=1;fillColor=#ffffff;shadow=1;\
                     strokeWidth=1.5;strokeColor=#fb923c;\
                     fontFamily=Inter,Helvetica,sans-serif;\
                     fontSize=12;fontStyle=1;fontColor=#0f172a;"
            )
        }
        "decision" | "condition" => {
            format!(
                "rhombus;whiteSpace=wrap;html=1;fillColor=#ffffff;shadow=1;\
                     strokeWidth=2;strokeColor=#a78bfa;\
                     fontFamily=Inter,Helvetica,sans-serif;\
                     fontSize=11;fontStyle=2;fontColor=#0f172a;"
            )
        }
        "client" | "user" | "browser" => {
            format!(
                "shape=mxgraph.general.person_2;\
                     whiteSpace=wrap;html=1;fillColor=#ffffff;shadow=1;\
                     strokeWidth=1.5;strokeColor=#94a3b8;\
                     fontFamily=Inter,Helvetica,sans-serif;\
                     fontSize=12;fontStyle=1;fontColor=#0f172a;"
            )
        }
        _ => format!("{base}strokeColor=#cbd5e1;"),
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
    model.push_attribute(("page", "0"));
    model.push_attribute(("pageScale", "1"));
    model.push_attribute(("background", "#f8fafc"));
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

    // --- Group / Swimlane container cells -----------------------------------
    for group in &compiled.groups {
        let mut min_x = f64::MAX;
        let mut min_y = f64::MAX;
        let mut max_x = f64::MIN;
        let mut max_y = f64::MIN;
        let mut found_count = 0;

        for node_id in &group.nodes {
            if let Some(&node_idx) = compiled.node_map.get(node_id) {
                if let Some(nl) = layout.positions.get(&node_idx) {
                    min_x = min_x.min(nl.x);
                    min_y = min_y.min(nl.y);
                    max_x = max_x.max(nl.x + nl.width);
                    max_y = max_y.max(nl.y + nl.height);
                    found_count += 1;
                }
            }
        }

        if found_count == 0 {
            continue;
        }

        let pad_h = 24.0;
        let pad_top = 34.0;
        let pad_bot = 20.0;

        let gx = (min_x - pad_h).max(10.0);
        let gy = (min_y - pad_top).max(10.0);
        let gw = (max_x - min_x) + (pad_h * 2.0);
        let gh = (max_y - min_y) + pad_top + pad_bot;

        let color = group.color.as_deref().unwrap_or("#64748b");
        let group_style = format!(
            "rounded=1;absoluteArcSize=1;arcSize=10;\
             fillColor={color};fillOpacity=15;\
             strokeColor={color};strokeWidth=1.5;\
             dashed=1;dashPattern=6 6;\
             verticalAlign=top;align=left;\
             spacingLeft=16;spacingTop=8;\
             fontFamily=Inter,Helvetica,sans-serif;\
             fontStyle=1;fontSize=12;fontColor={color};"
        );

        let mut g_cell = BytesStart::new("mxCell");
        g_cell.push_attribute(("id", group.id.as_str()));
        g_cell.push_attribute(("value", group.label.as_str()));
        g_cell.push_attribute(("style", group_style.as_str()));
        g_cell.push_attribute(("vertex", "1"));
        g_cell.push_attribute(("parent", "1"));
        w.write_event(Event::Start(g_cell))?;

        let mut g_geo = BytesStart::new("mxGeometry");
        g_geo.push_attribute(("x", gx.round().to_string().as_str()));
        g_geo.push_attribute(("y", gy.round().to_string().as_str()));
        g_geo.push_attribute(("width", gw.round().to_string().as_str()));
        g_geo.push_attribute(("height", gh.round().to_string().as_str()));
        g_geo.push_attribute(("as", "geometry"));
        w.write_event(Event::Empty(g_geo))?;

        w.write_event(Event::End(BytesEnd::new("mxCell")))?;
    }

    // --- Node cells ---------------------------------------------------------
    for node_idx in compiled.graph.node_indices() {
        let node_data = &compiled.graph[node_idx];
        let nl = match layout.positions.get(&node_idx) {
            Some(p) => p,
            None => continue,
        };
        let style = style_for_type(&node_data.node_type, theme);
        let tooltip = node_data.metadata.as_deref().unwrap_or("");

        // Build HTML label: bold title + optional muted sub-label
        let html_value = {
            let mut parts = node_data.label.splitn(2, '\n');
            let title = parts.next().unwrap_or(&node_data.label);
            let subtitle = parts.next();
            match subtitle {
                Some(sub) => format!(
                    "<b>{}</b><br/><font style='font-size:10px;color:#64748b'>{}</font>",
                    title, sub
                ),
                None => format!("<b>{}</b>", title),
            }
        };

        let mut cell = BytesStart::new("mxCell");
        cell.push_attribute(("id", node_data.id.as_str()));
        cell.push_attribute(("value", html_value.as_str()));
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
    // Group outgoing edges by their rendered source node to compute distributed exit ports.
    let mut outgoing_by_src: HashMap<
        petgraph::graph::NodeIndex,
        Vec<(petgraph::graph::EdgeIndex, f64)>,
    > = HashMap::new();

    for edge_idx in compiled.graph.edge_indices() {
        let (src, dst) = compiled.graph.edge_endpoints(edge_idx).unwrap();
        let edge_data = &compiled.graph[edge_idx];
        let (render_src, render_dst) = if edge_data.reversed {
            (dst, src)
        } else {
            (src, dst)
        };
        let target_x = layout
            .positions
            .get(&render_dst)
            .map(|p| p.x)
            .unwrap_or(0.0);
        outgoing_by_src
            .entry(render_src)
            .or_default()
            .push((edge_idx, target_x));
    }

    let mut exit_ports: HashMap<petgraph::graph::EdgeIndex, f64> = HashMap::new();
    for (_, mut edges) in outgoing_by_src {
        edges.sort_by(|a, b| {
            a.1.partial_cmp(&b.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        let count = edges.len();
        for (i, (edge_idx, _)) in edges.into_iter().enumerate() {
            let port = if count <= 1 {
                0.5
            } else {
                0.1 + (0.8 / (count - 1) as f64) * (i as f64)
            };
            exit_ports.insert(edge_idx, port);
        }
    }

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

        let exit_x = exit_ports.get(&edge_idx).copied().unwrap_or(0.5);
        let entry_x = 0.5_f64;

        let custom_style = match edge_data.edge_style.as_deref() {
            Some("async") => {
                "dashed=1;dashPattern=8 4;strokeColor=#d97706;strokeWidth=1.5;endArrow=open;endFill=0;"
            }
            Some("error") | Some("fallback") => {
                "dashed=1;dashPattern=6 3;strokeColor=#ef4444;strokeWidth=1.5;endArrow=blockThin;endFill=0;"
            }
            Some("data") | Some("stream") => {
                "strokeColor=#6366f1;strokeWidth=2;endArrow=blockThin;endFill=1;"
            }
            Some("bi") | Some("bidirectional") => {
                "strokeColor=#64748b;strokeWidth=1.5;startArrow=blockThin;startFill=1;endArrow=blockThin;endFill=1;"
            }
            _ => "strokeColor=#64748b;strokeWidth=1.5;endArrow=blockThin;endFill=1;",
        };

        let edge_style = format!(
            "edgeStyle=orthogonalEdgeStyle;\
             rounded=1;orthogonalLoop=1;jettySize=auto;html=1;\
             exitX={exit_x:.1};exitY=1.0;exitDx=0;exitDy=0;\
             entryX={entry_x:.1};entryY=0.0;entryDx=0;entryDy=0;\
             {custom_style}\
             endSize=6;\
             jumpStyle=arc;jumpSize=6;\
             labelBackgroundColor=#ffffff;labelBorderColor=none;\
             fontFamily=Inter,Helvetica,sans-serif;fontSize=11;fontColor=#475569;"
        );

        let mut cell = BytesStart::new("mxCell");
        cell.push_attribute(("id", edge_id.as_str()));
        cell.push_attribute(("value", label));
        cell.push_attribute(("style", edge_style.as_str()));
        cell.push_attribute(("edge", "1"));
        cell.push_attribute(("source", render_src));
        cell.push_attribute(("target", render_dst));
        cell.push_attribute(("parent", "1"));
        w.write_event(Event::Start(cell))?;

        // <mxGeometry relative="1" as="geometry">
        //   <mxPoint y="-10" as="offset" />
        // </mxGeometry>
        let mut geo = BytesStart::new("mxGeometry");
        geo.push_attribute(("relative", "1"));
        geo.push_attribute(("as", "geometry"));
        if !label.is_empty() {
            w.write_event(Event::Start(geo))?;
            let mut pt = BytesStart::new("mxPoint");
            pt.push_attribute(("y", "-10"));
            pt.push_attribute(("as", "offset"));
            w.write_event(Event::Empty(pt))?;
            w.write_event(Event::End(BytesEnd::new("mxGeometry")))?;
        } else {
            w.write_event(Event::Empty(geo))?;
        }

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
pub fn render_svg(compiled: &CompiledGraph, layout: &LayoutResult, _theme: &str) -> Result<String> {
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
    svg.push_attribute(("viewBox", format!("0 0 {canvas_w} {canvas_h}").as_str()));
    w.write_event(Event::Start(svg))?;

    // Background
    let mut bg = BytesStart::new("rect");
    bg.push_attribute(("width", "100%"));
    bg.push_attribute(("height", "100%"));
    bg.push_attribute(("fill", "#f8fafc"));
    w.write_event(Event::Empty(bg))?;

    // --- Draw group containers ----------------------------------------------
    for group in &compiled.groups {
        let mut min_x = f64::MAX;
        let mut min_y = f64::MAX;
        let mut max_x = f64::MIN;
        let mut max_y = f64::MIN;
        let mut found_count = 0;

        for node_id in &group.nodes {
            if let Some(&node_idx) = compiled.node_map.get(node_id) {
                if let Some(nl) = layout.positions.get(&node_idx) {
                    min_x = min_x.min(nl.x);
                    min_y = min_y.min(nl.y);
                    max_x = max_x.max(nl.x + nl.width);
                    max_y = max_y.max(nl.y + nl.height);
                    found_count += 1;
                }
            }
        }

        if found_count == 0 {
            continue;
        }

        let pad_h = 20.0;
        let pad_top = 28.0;
        let pad_bot = 16.0;

        let gx = (min_x - pad_h).max(10.0);
        let gy = (min_y - pad_top).max(10.0);
        let gw = (max_x - min_x) + (pad_h * 2.0);
        let gh = (max_y - min_y) + pad_top + pad_bot;
        let color = group.color.as_deref().unwrap_or("#64748b");

        let mut rect = BytesStart::new("rect");
        rect.push_attribute(("x", gx.round().to_string().as_str()));
        rect.push_attribute(("y", gy.round().to_string().as_str()));
        rect.push_attribute(("width", gw.round().to_string().as_str()));
        rect.push_attribute(("height", gh.round().to_string().as_str()));
        rect.push_attribute(("rx", "8"));
        rect.push_attribute(("fill", color));
        rect.push_attribute(("fill-opacity", "0.08"));
        rect.push_attribute(("stroke", color));
        rect.push_attribute(("stroke-width", "1.5"));
        rect.push_attribute(("stroke-dasharray", "6 6"));
        w.write_event(Event::Empty(rect))?;

        let mut text = BytesStart::new("text");
        text.push_attribute(("x", (gx + 12.0).round().to_string().as_str()));
        text.push_attribute(("y", (gy + 18.0).round().to_string().as_str()));
        text.push_attribute(("font-family", "sans-serif"));
        text.push_attribute(("font-size", "11"));
        text.push_attribute(("font-weight", "bold"));
        text.push_attribute(("fill", color));
        w.write_event(Event::Start(text))?;
        w.write_event(Event::Text(BytesText::new(&group.label)))?;
        w.write_event(Event::End(BytesEnd::new("text")))?;
    }

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

        let (stroke, stroke_w, dash) = match edge_data.edge_style.as_deref() {
            Some("async") => ("#d97706", "1.5", Some("8 4")),
            Some("error") | Some("fallback") => ("#ef4444", "1.5", Some("6 3")),
            Some("data") | Some("stream") => ("#6366f1", "2.0", None),
            _ => ("#64748b", "1.5", None),
        };

        let mut line = BytesStart::new("line");
        line.push_attribute(("x1", x1.to_string().as_str()));
        line.push_attribute(("y1", y1.to_string().as_str()));
        line.push_attribute(("x2", x2.to_string().as_str()));
        line.push_attribute(("y2", y2.to_string().as_str()));
        line.push_attribute(("stroke", stroke));
        line.push_attribute(("stroke-width", stroke_w));
        if let Some(d) = dash {
            line.push_attribute(("stroke-dasharray", d));
        }
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
    use crate::layout::{LayoutConfig, compute_layout};
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
            groups: vec![],
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
                edge_style: None,
            }],
            groups: vec![],
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
        assert!(
            xml.contains("vertex=\"1\""),
            "nodes must carry vertex=\"1\""
        );
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
        assert_eq!(edge_count, 1, "expected exactly 1 edge cell");
        assert_eq!(
            vertex_count, 2,
            "expected exactly 2 vertex cells (mandatory + node)"
        );
    }

    #[test]
    fn test_style_mapping_database() {
        let style = style_for_type("database", "standard");
        assert!(
            style.contains("cylinder3"),
            "database style must reference 'cylinder3'"
        );
    }

    #[test]
    fn test_style_mapping_default() {
        let style = style_for_type("unknown_widget", "standard");
        assert!(!style.is_empty(), "default style must not be empty");
        assert!(
            style.contains("rounded"),
            "default style should use rounded"
        );
    }

    #[test]
    fn test_svg_output_is_valid_xml() {
        let payload = two_node_payload();
        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let svg = render_svg(&compiled, &layout, "standard").unwrap();
        assert!(
            svg.starts_with("<?xml"),
            "SVG must start with XML declaration"
        );
        assert!(svg.contains("<svg"), "must contain svg element");
        assert!(svg.contains("</svg>"), "must close svg element");
    }

    #[test]
    fn test_style_mapping_dark_mode() {
        let style = style_for_type("proxy", "dark");
        assert!(
            style.contains("fillColor=#1e293b"),
            "dark mode must use slate-800 fill"
        );
        assert!(
            style.contains("fontColor=#f1f5f9"),
            "dark mode must use light font color"
        );
        assert!(
            style.contains("strokeColor=#475569"),
            "dark mode must use slate-600 stroke"
        );
    }

    #[test]
    fn test_drawio_html_two_line_label() {
        let payload = DiagramPayload {
            diagram_type: "flowchart".to_owned(),
            theme: None,
            nodes: vec![NodeDef {
                id: "n1".to_owned(),
                label: "API Gateway\nKong Ingress".to_owned(),
                node_type: "proxy".to_owned(),
                metadata: None,
            }],
            edges: vec![],
            groups: vec![],
        };
        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let xml = render_drawio(&compiled, &layout, "standard").unwrap();
        assert!(
            xml.contains("&lt;b&gt;API Gateway&lt;/b&gt;&lt;br/&gt;&lt;font style=&apos;font-size:10px;color:#64748b&apos;&gt;Kong Ingress&lt;/font&gt;"),
            "node cell must contain two-line formatted HTML label"
        );
    }

    #[test]
    fn test_drawio_edge_style_and_offset() {
        let payload = two_node_payload();
        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let xml = render_drawio(&compiled, &layout, "standard").unwrap();

        assert!(xml.contains("page=\"0\""), "page should be 0");
        assert!(
            xml.contains("background=\"#f8fafc\""),
            "background should be #f8fafc"
        );
        assert!(
            xml.contains("jumpStyle=arc"),
            "edge style must have jumpStyle=arc"
        );
        assert!(
            xml.contains("endArrow=blockThin"),
            "edge style must have endArrow=blockThin"
        );
        assert!(
            xml.contains("labelBackgroundColor=#ffffff"),
            "edge style must have labelBackgroundColor=#ffffff"
        );
        assert!(
            xml.contains("<mxPoint y=\"-10\" as=\"offset\""),
            "labeled edge geometry must include offset mxPoint"
        );
    }

    #[test]
    fn test_drawio_sibling_exit_ports() {
        let payload = DiagramPayload {
            diagram_type: "flowchart".to_owned(),
            theme: None,
            nodes: vec![
                NodeDef {
                    id: "src".to_owned(),
                    label: "Source".to_owned(),
                    node_type: "proxy".to_owned(),
                    metadata: None,
                },
                NodeDef {
                    id: "dst1".to_owned(),
                    label: "Target 1".to_owned(),
                    node_type: "server".to_owned(),
                    metadata: None,
                },
                NodeDef {
                    id: "dst2".to_owned(),
                    label: "Target 2".to_owned(),
                    node_type: "server".to_owned(),
                    metadata: None,
                },
            ],
            edges: vec![
                EdgeDef {
                    from: "src".to_owned(),
                    to: "dst1".to_owned(),
                    label: None,
                    edge_style: None,
                },
                EdgeDef {
                    from: "src".to_owned(),
                    to: "dst2".to_owned(),
                    label: None,
                    edge_style: None,
                },
            ],
            groups: vec![],
        };
        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let xml = render_drawio(&compiled, &layout, "standard").unwrap();

        assert!(
            xml.contains("exitX=0.1;"),
            "first sibling exit port should be 0.1"
        );
        assert!(
            xml.contains("exitX=0.9;"),
            "second sibling exit port should be 0.9"
        );
    }

    #[test]
    fn test_drawio_renders_groups_and_semantic_edge_styles() {
        use crate::schema::GroupDef;

        let payload = DiagramPayload {
            diagram_type: "architecture".to_owned(),
            theme: None,
            groups: vec![GroupDef {
                id: "grp_core".to_owned(),
                label: "Core Services".to_owned(),
                color: Some("#3b82f6".to_owned()),
                nodes: vec!["s1".to_owned(), "s2".to_owned()],
            }],
            nodes: vec![
                NodeDef {
                    id: "s1".to_owned(),
                    label: "Producer".to_owned(),
                    node_type: "server".to_owned(),
                    metadata: None,
                },
                NodeDef {
                    id: "s2".to_owned(),
                    label: "Consumer".to_owned(),
                    node_type: "server".to_owned(),
                    metadata: None,
                },
            ],
            edges: vec![EdgeDef {
                from: "s1".to_owned(),
                to: "s2".to_owned(),
                label: Some("events".to_owned()),
                edge_style: Some("async".to_owned()),
            }],
        };

        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let xml = render_drawio(&compiled, &layout, "standard").unwrap();

        assert!(xml.contains("id=\"grp_core\""), "must render group cell");
        assert!(xml.contains("Core Services"), "must render group label");
        assert!(xml.contains("dashPattern=8 4"), "async edge must be dashed");
        assert!(xml.contains("strokeColor=#d97706"), "async edge must be amber");

        let svg = render_svg(&compiled, &layout, "standard").unwrap();
        assert!(svg.contains("Core Services"), "svg must render group label");
        assert!(svg.contains("stroke-dasharray=\"8 4\""), "svg must render async dasharray");
    }
}
