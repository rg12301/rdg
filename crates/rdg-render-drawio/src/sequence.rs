//! draw.io sequence diagram rendering: participant header boxes, vertical lifelines,
//! and chronological message arrows.

use anyhow::Result;
use quick_xml::{
    Writer,
    events::{BytesDecl, BytesEnd, BytesStart, Event},
};
use std::io::Cursor;

use rdg_graph::CompiledGraph;
use rdg_layout::{DesignTokens, LayoutResult, SequenceLayoutInfo};

use crate::style::style_for_type;

pub fn render_sequence_drawio(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    seq: &SequenceLayoutInfo,
    theme: &str,
    background: Option<&str>,
    tokens: &DesignTokens,
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
    let bg_color = background.unwrap_or(if theme == "dark" {
        "#0f172a"
    } else {
        "#f8fafc"
    });
    model.push_attribute(("background", bg_color));
    model.push_attribute(("math", "1"));
    model.push_attribute(("shadow", "0"));
    w.write_event(Event::Start(model))?;

    // <root>
    w.write_event(Event::Start(BytesStart::new("root")))?;

    // Mandatory stub cells
    let mut cell0 = BytesStart::new("mxCell");
    cell0.push_attribute(("id", "0"));
    w.write_event(Event::Empty(cell0))?;

    let mut cell1 = BytesStart::new("mxCell");
    cell1.push_attribute(("id", "1"));
    cell1.push_attribute(("parent", "0"));
    w.write_event(Event::Empty(cell1))?;

    // Title / Description
    if let Some(title) = &compiled.title {
        let title_color = if theme == "dark" {
            "#f1f5f9"
        } else {
            "#0f172a"
        };
        let sub_color = if theme == "dark" {
            "#94a3b8"
        } else {
            "#64748b"
        };
        let title_html = if let Some(desc) = &compiled.description {
            format!(
                "<b><font style=\"font-size:16px;color:{title_color};\">{title}</font></b><br/><font style=\"font-size:11px;color:{sub_color};\">{desc}</font>"
            )
        } else {
            format!("<b><font style=\"font-size:16px;color:{title_color};\">{title}</font></b>")
        };
        let title_style = "text;html=1;strokeColor=none;fillColor=none;align=left;verticalAlign=top;rounded=0;fontFamily=Inter,Helvetica,sans-serif;";
        let mut t_cell = BytesStart::new("mxCell");
        t_cell.push_attribute(("id", "diagram_title_header"));
        t_cell.push_attribute(("value", title_html.as_str()));
        t_cell.push_attribute(("style", title_style));
        t_cell.push_attribute(("vertex", "1"));
        t_cell.push_attribute(("parent", "1"));
        w.write_event(Event::Start(t_cell))?;

        let mut t_geo = BytesStart::new("mxGeometry");
        t_geo.push_attribute(("x", "24"));
        t_geo.push_attribute(("y", "12"));
        t_geo.push_attribute(("width", "500"));
        t_geo.push_attribute(("height", "40"));
        t_geo.push_attribute(("as", "geometry"));
        w.write_event(Event::Empty(t_geo))?;
        w.write_event(Event::End(BytesEnd::new("mxCell")))?;
    }

    let is_dark = theme == "dark";
    let lifeline_color = if is_dark { "#475569" } else { "#94a3b8" };
    let label_bg_color = if is_dark { "#1e293b" } else { "#ffffff" };
    let label_font_color = if is_dark { "#cbd5e1" } else { "#475569" };

    // 1. Participant header boxes
    for node_idx in compiled.graph.node_indices() {
        let node_data = &compiled.graph[node_idx];
        let nl = match layout.positions.get(&node_idx) {
            Some(p) => p,
            None => continue,
        };
        let style = style_for_type(&node_data.node_type, theme);
        let icon_key = node_data.icon.as_deref();
        let icon_html = icon_key
            .and_then(rdg_icons::icon_as_data_uri)
            .map(|uri| format!("<img src=\"{uri}\" width=\"16\" height=\"16\" style=\"vertical-align:middle;margin-right:5px;display:inline-block;\"/>"))
            .unwrap_or_default();
        let html_value = format!("{icon_html}<b>{}</b>", node_data.label);

        let mut cell = BytesStart::new("mxCell");
        cell.push_attribute(("id", node_data.id.as_str()));
        cell.push_attribute(("value", html_value.as_str()));
        cell.push_attribute(("style", style.as_str()));
        cell.push_attribute(("vertex", "1"));
        cell.push_attribute(("parent", "1"));
        w.write_event(Event::Start(cell))?;

        let mut geo = BytesStart::new("mxGeometry");
        geo.push_attribute(("x", nl.x.round().to_string().as_str()));
        geo.push_attribute(("y", nl.y.round().to_string().as_str()));
        geo.push_attribute(("width", nl.width.round().to_string().as_str()));
        geo.push_attribute(("height", nl.height.round().to_string().as_str()));
        geo.push_attribute(("as", "geometry"));
        w.write_event(Event::Empty(geo))?;

        w.write_event(Event::End(BytesEnd::new("mxCell")))?;

        // 2. Vertical lifeline for this participant
        let lx = seq
            .lifeline_x
            .get(&node_idx)
            .copied()
            .unwrap_or(nl.x + nl.width / 2.0);
        let lifeline_style = format!(
            "edgeStyle=none;rounded=0;orthogonalLoop=1;jettySize=auto;html=1;dashed=1;dashPattern=6 6;strokeColor={lifeline_color};strokeWidth=1.5;endArrow=none;"
        );
        let mut line_cell = BytesStart::new("mxCell");
        let line_id = format!("lifeline_{}", node_data.id);
        line_cell.push_attribute(("id", line_id.as_str()));
        line_cell.push_attribute(("value", ""));
        line_cell.push_attribute(("style", lifeline_style.as_str()));
        line_cell.push_attribute(("edge", "1"));
        line_cell.push_attribute(("parent", "1"));
        w.write_event(Event::Start(line_cell))?;

        let mut line_geo = BytesStart::new("mxGeometry");
        line_geo.push_attribute(("relative", "1"));
        line_geo.push_attribute(("as", "geometry"));
        w.write_event(Event::Start(line_geo))?;

        let mut pt_src = BytesStart::new("mxPoint");
        pt_src.push_attribute(("x", lx.round().to_string().as_str()));
        pt_src.push_attribute(("y", seq.lifeline_top_y.round().to_string().as_str()));
        pt_src.push_attribute(("as", "sourcePoint"));
        w.write_event(Event::Empty(pt_src))?;

        let mut pt_dst = BytesStart::new("mxPoint");
        pt_dst.push_attribute(("x", lx.round().to_string().as_str()));
        pt_dst.push_attribute(("y", seq.lifeline_bottom_y.round().to_string().as_str()));
        pt_dst.push_attribute(("as", "targetPoint"));
        w.write_event(Event::Empty(pt_dst))?;

        w.write_event(Event::End(BytesEnd::new("mxGeometry")))?;
        w.write_event(Event::End(BytesEnd::new("mxCell")))?;
    }

    // 3. Chronological message arrows
    for (i, msg) in seq.messages.iter().enumerate() {
        let edge_data = &compiled.graph[msg.edge_idx];
        let src_id = &compiled.graph[msg.from_node].id;
        let dst_id = &compiled.graph[msg.to_node].id;
        let edge_id = format!("seq_msg_{i}_{src_id}_{dst_id}");
        let label = edge_data.label.as_deref().unwrap_or("");
        let from_x = seq.lifeline_x.get(&msg.from_node).copied().unwrap_or(0.0);
        let to_x = seq.lifeline_x.get(&msg.to_node).copied().unwrap_or(0.0);
        let y = msg.y;

        if msg.is_self_call {
            let self_style = format!(
                "edgeStyle=orthogonalEdgeStyle;rounded=1;orthogonalLoop=1;jettySize=auto;html=1;\
                 strokeColor=#475569;strokeWidth=1.5;endArrow=blockThin;endFill=1;\
                 labelBackgroundColor={label_bg_color};labelBorderColor=none;\
                 fontFamily=Inter,Helvetica,sans-serif;fontSize=11;fontColor={label_font_color};"
            );
            let mut cell = BytesStart::new("mxCell");
            cell.push_attribute(("id", edge_id.as_str()));
            cell.push_attribute(("value", label));
            cell.push_attribute(("style", self_style.as_str()));
            cell.push_attribute(("edge", "1"));
            cell.push_attribute(("parent", "1"));
            w.write_event(Event::Start(cell))?;

            let mut geo = BytesStart::new("mxGeometry");
            geo.push_attribute(("relative", "1"));
            geo.push_attribute(("as", "geometry"));
            w.write_event(Event::Start(geo))?;

            let mut pt_src = BytesStart::new("mxPoint");
            pt_src.push_attribute(("x", from_x.round().to_string().as_str()));
            pt_src.push_attribute(("y", (y - tokens.px(1.0)).round().to_string().as_str()));
            pt_src.push_attribute(("as", "sourcePoint"));
            w.write_event(Event::Empty(pt_src))?;

            let mut pt_dst = BytesStart::new("mxPoint");
            pt_dst.push_attribute(("x", from_x.round().to_string().as_str()));
            pt_dst.push_attribute(("y", (y + tokens.px(2.0)).round().to_string().as_str()));
            pt_dst.push_attribute(("as", "targetPoint"));
            w.write_event(Event::Empty(pt_dst))?;

            let mut pts_array = BytesStart::new("Array");
            pts_array.push_attribute(("as", "points"));
            w.write_event(Event::Start(pts_array))?;

            let mut p1 = BytesStart::new("mxPoint");
            p1.push_attribute(("x", (from_x + tokens.px(4.5)).round().to_string().as_str()));
            p1.push_attribute(("y", (y - tokens.px(1.0)).round().to_string().as_str()));
            w.write_event(Event::Empty(p1))?;

            let mut p2 = BytesStart::new("mxPoint");
            p2.push_attribute(("x", (from_x + tokens.px(4.5)).round().to_string().as_str()));
            p2.push_attribute(("y", (y + tokens.px(2.0)).round().to_string().as_str()));
            w.write_event(Event::Empty(p2))?;

            w.write_event(Event::End(BytesEnd::new("Array")))?;

            if !label.is_empty() {
                let mut pt_offset = BytesStart::new("mxPoint");
                pt_offset.push_attribute(("x", "20"));
                pt_offset.push_attribute(("y", "-4"));
                pt_offset.push_attribute(("as", "offset"));
                w.write_event(Event::Empty(pt_offset))?;
            }

            w.write_event(Event::End(BytesEnd::new("mxGeometry")))?;
            w.write_event(Event::End(BytesEnd::new("mxCell")))?;
        } else {
            let is_reply = msg.is_reply || edge_data.edge_style.as_deref() == Some("reply");
            let is_async = edge_data.edge_style.as_deref() == Some("async");

            let edge_style = if is_reply {
                format!(
                    "edgeStyle=none;rounded=0;orthogonalLoop=1;jettySize=auto;html=1;\
                     dashed=1;dashPattern=6 3;strokeColor={lifeline_color};strokeWidth=1.5;\
                     endArrow=open;endFill=0;endSize=7;\
                     labelBackgroundColor={label_bg_color};labelBorderColor=none;\
                     fontFamily=Inter,Helvetica,sans-serif;fontSize=11;fontColor={label_font_color};"
                )
            } else if is_async {
                format!(
                    "edgeStyle=none;rounded=0;orthogonalLoop=1;jettySize=auto;html=1;\
                     dashed=1;dashPattern=8 4;strokeColor=#d97706;strokeWidth=1.5;\
                     endArrow=open;endFill=0;endSize=7;\
                     labelBackgroundColor={label_bg_color};labelBorderColor=none;\
                     fontFamily=Inter,Helvetica,sans-serif;fontSize=11;fontColor={label_font_color};"
                )
            } else {
                let stroke_col = if is_dark { "#cbd5e1" } else { "#1e293b" };
                format!(
                    "edgeStyle=none;rounded=0;orthogonalLoop=1;jettySize=auto;html=1;\
                     strokeColor={stroke_col};strokeWidth=1.5;\
                     endArrow=blockThin;endFill=1;endSize=6;\
                     labelBackgroundColor={label_bg_color};labelBorderColor=none;\
                     fontFamily=Inter,Helvetica,sans-serif;fontSize=11;fontColor={label_font_color};"
                )
            };

            let mut cell = BytesStart::new("mxCell");
            cell.push_attribute(("id", edge_id.as_str()));
            cell.push_attribute(("value", label));
            cell.push_attribute(("style", edge_style.as_str()));
            cell.push_attribute(("edge", "1"));
            cell.push_attribute(("parent", "1"));
            w.write_event(Event::Start(cell))?;

            let mut geo = BytesStart::new("mxGeometry");
            geo.push_attribute(("relative", "1"));
            geo.push_attribute(("as", "geometry"));
            w.write_event(Event::Start(geo))?;

            let mut pt_src = BytesStart::new("mxPoint");
            pt_src.push_attribute(("x", from_x.round().to_string().as_str()));
            pt_src.push_attribute(("y", y.round().to_string().as_str()));
            pt_src.push_attribute(("as", "sourcePoint"));
            w.write_event(Event::Empty(pt_src))?;

            let mut pt_dst = BytesStart::new("mxPoint");
            pt_dst.push_attribute(("x", to_x.round().to_string().as_str()));
            pt_dst.push_attribute(("y", y.round().to_string().as_str()));
            pt_dst.push_attribute(("as", "targetPoint"));
            w.write_event(Event::Empty(pt_dst))?;

            if !label.is_empty() {
                let mut pt_offset = BytesStart::new("mxPoint");
                pt_offset.push_attribute(("y", "-10"));
                pt_offset.push_attribute(("as", "offset"));
                w.write_event(Event::Empty(pt_offset))?;
            }

            w.write_event(Event::End(BytesEnd::new("mxGeometry")))?;
            w.write_event(Event::End(BytesEnd::new("mxCell")))?;
        }
    }

    // </root></mxGraphModel></diagram></mxfile>
    w.write_event(Event::End(BytesEnd::new("root")))?;
    w.write_event(Event::End(BytesEnd::new("mxGraphModel")))?;
    w.write_event(Event::End(BytesEnd::new("diagram")))?;
    w.write_event(Event::End(BytesEnd::new("mxfile")))?;

    String::from_utf8(buf).map_err(|e| anyhow::anyhow!("invalid UTF-8 in drawio output: {e}"))
}
