//! SVG sequence diagram rendering: lifelines, message arrows, and participant cards.

use anyhow::Result;
use quick_xml::{
    Writer,
    events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event},
};
use std::io::Cursor;

use rdg_graph::CompiledGraph;
use rdg_layout::{DesignTokens, LayoutResult, SequenceLayoutInfo};
use rdg_render_core::style::node_accent_color;

pub fn render_sequence_svg(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    seq: &SequenceLayoutInfo,
    theme: &str,
    background: Option<&str>,
    tokens: &DesignTokens,
) -> Result<String> {
    let is_dark = theme == "dark";
    let bg_color = background.unwrap_or(if is_dark { "#0f172a" } else { "#f8fafc" });
    let card_fill = if is_dark { "#1e293b" } else { "#ffffff" };
    let lifeline_color = if is_dark { "#475569" } else { "#94a3b8" };
    let text_color = if is_dark { "#f1f5f9" } else { "#0f172a" };
    let sub_color = if is_dark { "#94a3b8" } else { "#64748b" };

    let max_x = layout
        .positions
        .values()
        .map(|nl| nl.x + nl.width)
        .fold(0.0_f64, f64::max);
    let canvas_w = (max_x + tokens.px(7.5)).max(tokens.px(75.0));
    let canvas_h = (seq.lifeline_bottom_y + tokens.px(5.0)).max(tokens.px(37.5));

    let mut buf = Vec::with_capacity(8192);
    let mut w = Writer::new_with_indent(Cursor::new(&mut buf), b' ', 2);

    w.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;

    let mut svg = BytesStart::new("svg");
    svg.push_attribute(("xmlns", "http://www.w3.org/2000/svg"));
    svg.push_attribute(("version", "1.1"));
    svg.push_attribute(("width", canvas_w.round().to_string().as_str()));
    svg.push_attribute(("height", canvas_h.round().to_string().as_str()));
    svg.push_attribute((
        "viewBox",
        format!("0 0 {} {}", canvas_w.round(), canvas_h.round()).as_str(),
    ));
    w.write_event(Event::Start(svg))?;

    // Background rect
    let mut bg = BytesStart::new("rect");
    bg.push_attribute(("width", "100%"));
    bg.push_attribute(("height", "100%"));
    bg.push_attribute(("fill", bg_color));
    w.write_event(Event::Empty(bg))?;

    // Defs: drop shadow and markers
    w.write_event(Event::Start(BytesStart::new("defs")))?;

    let mut filter = BytesStart::new("filter");
    filter.push_attribute(("id", "card-shadow"));
    filter.push_attribute(("x", "-20%"));
    filter.push_attribute(("y", "-20%"));
    filter.push_attribute(("width", "140%"));
    filter.push_attribute(("height", "140%"));
    w.write_event(Event::Start(filter))?;

    let mut shadow = BytesStart::new("feDropShadow");
    shadow.push_attribute(("dx", "0"));
    shadow.push_attribute(("dy", "2"));
    shadow.push_attribute(("stdDeviation", "3"));
    shadow.push_attribute(("flood-color", "#0f172a"));
    shadow.push_attribute(("flood-opacity", if is_dark { "0.35" } else { "0.08" }));
    w.write_event(Event::Empty(shadow))?;
    w.write_event(Event::End(BytesEnd::new("filter")))?;

    // Arrow markers
    let markers = [
        (
            "seq-arrow-sync",
            if is_dark { "#cbd5e1" } else { "#1e293b" },
            true,
        ),
        ("seq-arrow-reply", lifeline_color, false),
        ("seq-arrow-async", "#d97706", false),
    ];
    for (id, col, filled) in markers {
        let mut marker = BytesStart::new("marker");
        marker.push_attribute(("id", id));
        marker.push_attribute(("viewBox", "0 0 10 10"));
        marker.push_attribute(("refX", "8"));
        marker.push_attribute(("refY", "5"));
        marker.push_attribute(("markerWidth", "6"));
        marker.push_attribute(("markerHeight", "6"));
        marker.push_attribute(("orient", "auto"));
        w.write_event(Event::Start(marker))?;

        let mut mpath = BytesStart::new("path");
        mpath.push_attribute(("d", "M 0 1.5 L 8 5 L 0 8.5 z"));
        if filled {
            mpath.push_attribute(("fill", col));
        } else {
            mpath.push_attribute(("fill", "none"));
            mpath.push_attribute(("stroke", col));
            mpath.push_attribute(("stroke-width", "1.5"));
        }
        w.write_event(Event::Empty(mpath))?;
        w.write_event(Event::End(BytesEnd::new("marker")))?;
    }

    w.write_event(Event::End(BytesEnd::new("defs")))?;

    // Title / Description
    if let Some(title) = &compiled.title {
        let mut t_elem = BytesStart::new("text");
        t_elem.push_attribute(("x", "24"));
        t_elem.push_attribute(("y", "28"));
        t_elem.push_attribute(("font-family", "Inter, Helvetica, sans-serif"));
        t_elem.push_attribute(("font-size", "15"));
        t_elem.push_attribute(("font-weight", "bold"));
        t_elem.push_attribute(("fill", text_color));
        w.write_event(Event::Start(t_elem))?;
        w.write_event(Event::Text(BytesText::new(title)))?;
        w.write_event(Event::End(BytesEnd::new("text")))?;

        if let Some(desc) = &compiled.description {
            let mut d_elem = BytesStart::new("text");
            d_elem.push_attribute(("x", "24"));
            d_elem.push_attribute(("y", "44"));
            d_elem.push_attribute(("font-family", "Inter, Helvetica, sans-serif"));
            d_elem.push_attribute(("font-size", "11"));
            d_elem.push_attribute(("fill", sub_color));
            w.write_event(Event::Start(d_elem))?;
            w.write_event(Event::Text(BytesText::new(desc)))?;
            w.write_event(Event::End(BytesEnd::new("text")))?;
        }
    }

    // 1. Draw lifelines (vertical dashed lines)
    for &lx in seq.lifeline_x.values() {
        let mut line = BytesStart::new("line");
        line.push_attribute(("x1", lx.round().to_string().as_str()));
        line.push_attribute(("y1", seq.lifeline_top_y.round().to_string().as_str()));
        line.push_attribute(("x2", lx.round().to_string().as_str()));
        line.push_attribute(("y2", seq.lifeline_bottom_y.round().to_string().as_str()));
        line.push_attribute(("stroke", lifeline_color));
        line.push_attribute(("stroke-width", "1.5"));
        line.push_attribute(("stroke-dasharray", "6 6"));
        w.write_event(Event::Empty(line))?;
    }

    // 2. Draw message arrows
    for msg in &seq.messages {
        let edge_data = &compiled.graph[msg.edge_idx];
        let from_x = seq.lifeline_x.get(&msg.from_node).copied().unwrap_or(0.0);
        let to_x = seq.lifeline_x.get(&msg.to_node).copied().unwrap_or(0.0);
        let y = msg.y;
        let is_reply = msg.is_reply || edge_data.edge_style.as_deref() == Some("reply");
        let is_async = edge_data.edge_style.as_deref() == Some("async");

        let (stroke, marker, dash) = if is_reply {
            (lifeline_color, "seq-arrow-reply", Some("6 3"))
        } else if is_async {
            ("#d97706", "seq-arrow-async", Some("8 4"))
        } else {
            (
                if is_dark { "#cbd5e1" } else { "#1e293b" },
                "seq-arrow-sync",
                None,
            )
        };

        if msg.is_self_call {
            let loop_d = format!(
                "M {from_x:.1} {y_top:.1} H {x_out:.1} V {y_bot:.1} H {from_x:.1}",
                y_top = y - 8.0,
                x_out = from_x + 36.0,
                y_bot = y + 16.0,
            );
            let mut path = BytesStart::new("path");
            path.push_attribute(("d", loop_d.as_str()));
            path.push_attribute(("fill", "none"));
            path.push_attribute(("stroke", stroke));
            path.push_attribute(("stroke-width", "1.5"));
            path.push_attribute(("marker-end", format!("url(#{marker})").as_str()));
            w.write_event(Event::Empty(path))?;

            if let Some(label) = &edge_data.label {
                let mut text = BytesStart::new("text");
                text.push_attribute(("x", (from_x + 42.0).round().to_string().as_str()));
                text.push_attribute(("y", (y + 6.0).round().to_string().as_str()));
                text.push_attribute(("font-family", "Inter, Helvetica, sans-serif"));
                text.push_attribute(("font-size", "10"));
                text.push_attribute(("fill", sub_color));
                w.write_event(Event::Start(text))?;
                w.write_event(Event::Text(BytesText::new(label)))?;
                w.write_event(Event::End(BytesEnd::new("text")))?;
            }
        } else {
            let mut line = BytesStart::new("line");
            line.push_attribute(("x1", from_x.round().to_string().as_str()));
            line.push_attribute(("y1", y.round().to_string().as_str()));
            line.push_attribute(("x2", to_x.round().to_string().as_str()));
            line.push_attribute(("y2", y.round().to_string().as_str()));
            line.push_attribute(("stroke", stroke));
            line.push_attribute(("stroke-width", "1.5"));
            if let Some(d) = dash {
                line.push_attribute(("stroke-dasharray", d));
            }
            line.push_attribute(("marker-end", format!("url(#{marker})").as_str()));
            w.write_event(Event::Empty(line))?;

            if let Some(label) = &edge_data.label {
                let mid_x = (from_x + to_x) / 2.0;
                let char_len = label.chars().count();
                let edge_label_font = tokens.font_size * 0.83;
                let pill_w = (char_len as f64 * tokens.char_width(edge_label_font) + tokens.px(1.25))
                    .max(tokens.px(2.5));

                let mut pill = BytesStart::new("rect");
                pill.push_attribute(("x", (mid_x - pill_w / 2.0).round().to_string().as_str()));
                pill.push_attribute(("y", (y - 16.0).round().to_string().as_str()));
                pill.push_attribute(("width", pill_w.round().to_string().as_str()));
                pill.push_attribute(("height", "14"));
                pill.push_attribute(("rx", "3"));
                pill.push_attribute(("fill", bg_color));
                w.write_event(Event::Empty(pill))?;

                let mut text = BytesStart::new("text");
                text.push_attribute(("x", mid_x.round().to_string().as_str()));
                text.push_attribute(("y", (y - 5.0).round().to_string().as_str()));
                text.push_attribute(("text-anchor", "middle"));
                text.push_attribute(("font-family", "Inter, Helvetica, sans-serif"));
                text.push_attribute(("font-size", "10"));
                text.push_attribute(("font-weight", "500"));
                text.push_attribute(("fill", if is_dark { "#cbd5e1" } else { "#475569" }));
                w.write_event(Event::Start(text))?;
                w.write_event(Event::Text(BytesText::new(label)))?;
                w.write_event(Event::End(BytesEnd::new("text")))?;
            }
        }
    }

    // 3. Draw participant cards (on top of lifelines)
    for node_idx in compiled.graph.node_indices() {
        let node_data = &compiled.graph[node_idx];
        let nl = match layout.positions.get(&node_idx) {
            Some(p) => p,
            None => continue,
        };
        let stroke_color = node_data
            .color
            .as_deref()
            .unwrap_or_else(|| node_accent_color(&node_data.node_type, theme));

        let mut rect = BytesStart::new("rect");
        rect.push_attribute(("x", nl.x.round().to_string().as_str()));
        rect.push_attribute(("y", nl.y.round().to_string().as_str()));
        rect.push_attribute(("width", nl.width.round().to_string().as_str()));
        rect.push_attribute(("height", nl.height.round().to_string().as_str()));
        rect.push_attribute(("rx", "8"));
        rect.push_attribute(("fill", card_fill));
        rect.push_attribute(("stroke", stroke_color));
        rect.push_attribute(("stroke-width", "1.5"));
        rect.push_attribute(("filter", "url(#card-shadow)"));
        w.write_event(Event::Empty(rect))?;

        // Optional icon
        if let Some(icon_key) = node_data.icon.as_deref() {
            if let Some(icon_markup) =
                rdg_icons::render_icon_svg(icon_key, nl.x + 10.0, nl.y + 12.0, 18.0)
            {
                w.write_event(Event::Text(BytesText::from_escaped(icon_markup)))?;
            }
        }

        let cx = if node_data.icon.is_some() {
            nl.x + nl.width / 2.0 + 8.0
        } else {
            nl.x + nl.width / 2.0
        };
        let mut text = BytesStart::new("text");
        text.push_attribute(("x", cx.round().to_string().as_str()));
        text.push_attribute(("y", (nl.y + 26.0).round().to_string().as_str()));
        text.push_attribute(("text-anchor", "middle"));
        text.push_attribute(("font-family", "Inter, Helvetica, sans-serif"));
        text.push_attribute(("font-size", "12"));
        text.push_attribute(("font-weight", "bold"));
        text.push_attribute(("fill", text_color));
        w.write_event(Event::Start(text))?;
        w.write_event(Event::Text(BytesText::new(&node_data.label)))?;
        w.write_event(Event::End(BytesEnd::new("text")))?;
    }

    w.write_event(Event::End(BytesEnd::new("svg")))?;
    String::from_utf8(buf).map_err(|e| anyhow::anyhow!("invalid UTF-8 in svg output: {e}"))
}
