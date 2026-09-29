//! draw.io sequence diagram rendering: participant groups, fragments (`umlFrame`),
//! dividers, lifelines, activation bars, messages, notes and participant boxes (or
//! `umlActor` figures), from the geometry `rdg_layout`'s sequence layout computed.

use anyhow::Result;
use quick_xml::{
    Writer,
    events::{BytesDecl, BytesEnd, BytesStart, Event},
};
use std::io::Cursor;

use rdg_graph::CompiledGraph;
use rdg_layout::{DesignTokens, LayoutResult, SequenceLayoutInfo};

use crate::html::format_html_label_with_details;
use crate::style::{edge_style, node_style};
use crate::{write_legend, write_vertex};
use rdg_layout::{actor_figure_h, message_label_lines};
use rdg_render_core::look::{edge_look, group_look, node_look};
use rdg_render_core::theme::Theme;

pub fn render_sequence_drawio(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    seq: &SequenceLayoutInfo,
    theme: &Theme,
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
    let bg_color = background.unwrap_or(&theme.canvas.background);
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
        let f = &theme.font;
        let title_html = format!(
            "<b><font style=\"font-size:{}px;color:{};\">{title}</font></b>{}",
            f.title_size,
            theme.title.color,
            compiled.description.as_deref().map_or(String::new(), |desc| format!(
                "<br/><font style=\"font-size:{}px;color:{};\">{desc}</font>",
                f.description_size, theme.title.description_color
            ))
        );
        let title_style_owned = format!(
            "text;html=1;strokeColor=none;fillColor=none;align=left;verticalAlign=top;rounded=0;whiteSpace=nowrap;fontFamily={};",
            f.family
        );
        let title_style = title_style_owned.as_str();
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
        t_geo.push_attribute(("width", format!("{:.0}", title.chars().count() as f64 * tokens.char_width(f.title_size)).as_str()));
        t_geo.push_attribute(("height", format!("{:.0}", tokens.title_band_for(compiled.description.is_some())).as_str()));
        t_geo.push_attribute(("as", "geometry"));
        w.write_event(Event::Empty(t_geo))?;
        w.write_event(Event::End(BytesEnd::new("mxCell")))?;
    }

    let st = &theme.sequence;
    let f = &theme.font;
    // The canvas, sized like the SVG renderer's, so exports keep the margin.
    {
        let title_w = compiled.title.as_deref().map_or(0.0, |t| 24.0 + t.chars().count() as f64 * tokens.char_width(f.title_size));
        let desc_w = compiled.description.as_deref().map_or(0.0, |d| 24.0 + d.chars().count() as f64 * tokens.char_width(f.description_size));
        let mut cw = (seq.max_x + tokens.px(3.0)).max(title_w.max(desc_w) + tokens.px(3.0));
        let mut ch = seq.lifeline_bottom_y + tokens.px(3.0);
        if rdg_render_core::look::legend_enabled(theme, compiled) {
            let items = rdg_render_core::look::legend_items(theme, compiled);
            if !items.is_empty() {
                let (lw, lh) = rdg_render_core::look::legend_size(&items, theme, (seq.max_x - seq.min_x).max(tokens.px(40.0)));
                ch += lh + tokens.px(1.0);
                cw = cw.max(seq.min_x + lw + tokens.px(3.0));
            }
        }
        crate::write_canvas_cell(&mut w, cw, ch)?;
    }
    let lbl = f.edge_label_size;
    let cw = tokens.char_width(lbl);
    let lh = tokens.line_height(lbl);
    let bg = bg_color.to_string();
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let dash = |d: Option<&str>| d.map_or("dashed=0;".to_string(), |d| format!("dashed=1;dashPattern={d};"));
    let text_style = |color: &str, size: f64, bold: bool, align: &str| {
        format!(
            "text;html=1;strokeColor=none;fillColor=none;align={align};verticalAlign=middle;whiteSpace=nowrap;fontFamily={};fontSize={size};fontColor={color};fontStyle={};",
            f.family,
            u8::from(bold)
        )
    };

    // Participant groups.
    for g in &seq.groups {
        let gl = group_look(theme, &compiled.groups[g.group]);
        let style = format!(
            "rounded=1;absoluteArcSize=1;arcSize={};html=1;whiteSpace=wrap;fillColor={};fillOpacity={};strokeColor={};strokeWidth={};{}verticalAlign=top;align=left;spacingLeft={};spacingTop={};fontFamily={};fontSize={};fontColor={};fontStyle={};connectable=0;",
            2.0 * gl.corner_radius,
            gl.fill,
            (gl.fill_opacity * 100.0).round(),
            gl.stroke,
            gl.stroke_width,
            dash(gl.dash.as_deref()),
            tokens.px(1.0),
            tokens.px(0.5),
            f.family,
            f.group_title_size,
            gl.title_color,
            u8::from(gl.title_bold)
        );
        write_vertex(&mut w, &format!("seq_group_{}", g.group), &esc(&gl.title), &style, (g.x, g.y, g.w, g.h))?;
    }

    // Fragments: a frame with the keyword in its tab, the guard beside it.
    let fs = &st.fragment;
    for (i, fr) in seq.fragments.iter().enumerate() {
        let tab_w = fr.kind.chars().count() as f64 * cw + tokens.px(2.0);
        let style = format!(
            "shape=umlFrame;whiteSpace=wrap;html=1;pointerEvents=0;width={tab_w:.0};height={:.0};fillColor={};strokeColor={};strokeWidth={};fontFamily={};fontSize={lbl};fontColor={};fontStyle=1;align=left;verticalAlign=top;spacingLeft={};spacingTop=-1;connectable=0;",
            fr.tab_h, fs.tab_fill, fs.stroke, fs.width, f.family, fs.tab_text, tokens.px(0.5)
        );
        write_vertex(&mut w, &format!("seq_frag_{i}"), &esc(&fr.kind), &style, (fr.x, fr.y, fr.w, fr.h))?;
        if let Some(g) = &fr.label {
            let text = format!("[{g}]");
            let tw = text.chars().count() as f64 * cw + 4.0;
            write_vertex(&mut w, &format!("seq_frag_{i}_guard"), &esc(&text), &text_style(&fs.guard_color, lbl, false, "left"), (fr.x + tab_w + tokens.px(0.75), fr.y, tw, fr.tab_h))?;
        }
        for (j, (sy, guard)) in fr.separators.iter().enumerate() {
            let style = format!("endArrow=none;html=1;strokeColor={};strokeWidth={};{}", fs.stroke, fs.width, dash(Some(&fs.separator_dash)));
            write_line(&mut w, &format!("seq_frag_{i}_sep_{j}"), "", &style, &[(fr.x, *sy), (fr.x + fr.w, *sy)], None)?;
            if let Some(g) = guard {
                let text = format!("[{g}]");
                let tw = text.chars().count() as f64 * cw + 4.0;
                write_vertex(&mut w, &format!("seq_frag_{i}_sep_{j}_guard"), &esc(&text), &text_style(&fs.guard_color, lbl, false, "left"), (fr.x + tokens.px(1.0), *sy + 1.0, tw, lh))?;
            }
        }
    }

    // Dividers.
    for (i, d) in seq.dividers.iter().enumerate() {
        let ds = &st.divider;
        let cy = d.y + lh / 2.0;
        let style = format!("endArrow=none;html=1;strokeColor={};strokeWidth={};{}", ds.color, ds.width, dash(ds.dash.as_deref()));
        write_line(&mut w, &format!("seq_divider_{i}"), "", &style, &[(seq.min_x, cy), (seq.max_x, cy)], None)?;
        if !d.label.is_empty() {
            let tw = d.label.chars().count() as f64 * cw + tokens.px(2.0);
            let style = format!(
                "rounded=1;arcSize=50;html=1;whiteSpace=nowrap;fillColor={bg};strokeColor={};strokeWidth={};fontFamily={};fontSize={lbl};fontColor={};align=center;verticalAlign=middle;",
                ds.color, ds.width, f.family, theme.text.muted
            );
            write_vertex(&mut w, &format!("seq_divider_{i}_label"), &esc(&d.label), &style, ((seq.min_x + seq.max_x - tw) / 2.0, d.y, tw, lh))?;
        }
    }

    // Lifelines (a destroyed one ends in an ✕).
    let ls = &st.lifeline;
    for l in &seq.lifelines {
        let id = &compiled.graph[l.node].id;
        let style = format!("endArrow=none;html=1;strokeColor={};strokeWidth={};{}", ls.color, ls.width, dash(ls.dash.as_deref()));
        write_line(&mut w, &format!("{id}_lifeline"), "", &style, &[(l.x, l.y0), (l.x, l.y1)], None)?;
        if l.destroyed {
            let s = tokens.px(2.0);
            let style = format!("shape=umlDestroy;html=1;strokeWidth=2;strokeColor={};", theme.text.primary);
            write_vertex(&mut w, &format!("{id}_destroy"), "", &style, (l.x - s / 2.0, l.y1 - s / 2.0, s, s))?;
        }
    }

    // Activation bars.
    for (i, a) in seq.activations.iter().enumerate() {
        let stroke = if st.activation.category_stroke { node_look(theme, &compiled.graph[a.node]).stroke } else { st.activation.stroke.clone() };
        let style = format!("rounded=0;html=1;fillColor={};strokeColor={stroke};strokeWidth=1.25;connectable=0;", st.activation.fill);
        write_vertex(&mut w, &format!("seq_activation_{i}"), "", &style, (a.x, a.y0, a.width, a.y1 - a.y0))?;
    }

    // Messages, their labels above the arrow (right of a self-call's loop).
    for msg in &seq.messages {
        let ed = &compiled.graph[msg.edge_idx];
        let look = edge_look(theme, ed);
        let lines = message_label_lines(compiled, msg.edge_idx);
        let value = lines.iter().map(|l| esc(l)).collect::<Vec<_>>().join("<br/>");
        let label_w = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as f64 * cw;
        let label_h = lines.len() as f64 * lh;
        let style = format!("{}rounded=0;edgeStyle=none;jumpStyle=none;", edge_style(theme, &look));
        let id = format!("msg_{}", msg.edge_idx.index());
        if msg.is_self_call {
            let right = msg.from_x.max(msg.to_x) + msg.loop_w;
            let pts = [(msg.from_x, msg.y), (right, msg.y), (right, msg.y + msg.loop_h), (msg.to_x, msg.y + msg.loop_h)];
            // draw.io anchors an edge label at the path's middle (here, on the loop's
            // right side), so shift it right by half its width.
            let (l1, l2, l3) = (right - msg.from_x, msg.loop_h, right - msg.to_x);
            let half = (l1 + l2 + l3) / 2.0;
            let mid_y = if half <= l1 { msg.y } else if half <= l1 + l2 { msg.y + (half - l1) } else { msg.y + msg.loop_h };
            let mid_x = if half <= l1 { msg.from_x + half } else if half <= l1 + l2 { right } else { right - (half - l1 - l2) };
            let want = (right + tokens.px(0.75) + label_w / 2.0, msg.y + msg.loop_h / 2.0);
            write_line(&mut w, &id, &value, &format!("{style}align=center;"), &pts, Some((want.0 - mid_x, want.1 - mid_y)))?;
        } else {
            let offset = (!lines.is_empty()).then_some((0.0, -(label_h / 2.0 + tokens.px(0.5))));
            write_line(&mut w, &id, &value, &style, &[(msg.from_x, msg.y), (msg.to_x, msg.y)], offset)?;
        }
    }

    // Notes.
    let ns = &st.note;
    for (i, n) in seq.notes.iter().enumerate() {
        let style = format!(
            "shape=note;size=8;html=1;whiteSpace=wrap;fillColor={};strokeColor={};strokeWidth=1;fontFamily={};fontSize={};fontColor={};align=left;verticalAlign=middle;spacingLeft={};spacingRight=4;",
            ns.fill, ns.stroke, f.family, f.node_detail_size, ns.text, tokens.px(1.0) - 2.0
        );
        let value = n.lines.iter().map(|l| esc(l)).collect::<Vec<_>>().join("<br/>");
        write_vertex(&mut w, &format!("seq_note_{i}"), &value, &style, (n.x, n.y, n.w, n.h))?;
    }

    // Participants, on top.
    for node_idx in compiled.graph.node_indices() {
        let nd = &compiled.graph[node_idx];
        let Some(nl) = layout.positions.get(&node_idx) else { continue };
        let look = node_look(theme, nd);
        if look.shape == "actor" {
            let fig = actor_figure_h(tokens) - tokens.px(0.5);
            let fw = fig * 0.62;
            let style = format!(
                "shape=umlActor;verticalLabelPosition=bottom;verticalAlign=top;html=1;outlineConnect=0;fillColor=none;strokeColor={};strokeWidth=1.75;fontFamily={};fontSize={};fontColor={};fontStyle=1;spacingTop={};",
                look.stroke, f.family, f.node_title_size, look.title_color, tokens.px(0.5) - 2.0
            );
            write_vertex(&mut w, &nd.id, &esc(&nd.label).replace('\n', "<br/>"), &style, (nl.x + (nl.width - fw) / 2.0, nl.y, fw, fig))?;
            continue;
        }
        let style = node_style(theme, &look, 0.0);
        let value = format_html_label_with_details(&nd.label, theme, &look, nd.technology.as_deref(), tokens, nd.icon.as_deref());
        write_vertex(&mut w, &nd.id, &value, &style, (nl.x, nl.y, nl.width, nl.height))?;
    }

    // Legend (same rules as every other diagram type), under the lifelines.
    if rdg_render_core::look::legend_enabled(theme, compiled) {
        let items = rdg_render_core::look::legend_items(theme, compiled);
        if !items.is_empty() {
            let max_w = (seq.max_x - seq.min_x).max(tokens.px(40.0));
            let lw = rdg_render_core::look::legend_size(&items, theme, max_w).0;
            write_legend(&mut w, theme, tokens, &items, seq.min_x, seq.lifeline_bottom_y + tokens.px(2.0), max_w, lw, rdg_render_core::frame::Align::Left)?;
        }
    }

    // </root></mxGraphModel></diagram></mxfile>
    w.write_event(Event::End(BytesEnd::new("root")))?;
    w.write_event(Event::End(BytesEnd::new("mxGraphModel")))?;
    w.write_event(Event::End(BytesEnd::new("diagram")))?;
    w.write_event(Event::End(BytesEnd::new("mxfile")))?;

    String::from_utf8(buf).map_err(|e| anyhow::anyhow!("invalid UTF-8 in drawio output: {e}"))
}

/// A free-standing polyline edge through `pts` (no source/target cells), with an
/// optional absolute label offset from the path's midpoint.
fn write_line<W: std::io::Write>(w: &mut Writer<W>, id: &str, value: &str, style: &str, pts: &[(f64, f64)], offset: Option<(f64, f64)>) -> Result<()> {
    let mut cell = BytesStart::new("mxCell");
    cell.push_attribute(("id", id));
    cell.push_attribute(("value", value));
    cell.push_attribute(("style", style));
    cell.push_attribute(("edge", "1"));
    cell.push_attribute(("parent", "1"));
    w.write_event(Event::Start(cell))?;
    let mut geo = BytesStart::new("mxGeometry");
    geo.push_attribute(("relative", "1"));
    geo.push_attribute(("as", "geometry"));
    w.write_event(Event::Start(geo))?;
    let point = |w: &mut Writer<W>, (x, y): (f64, f64), role: Option<&str>| -> Result<()> {
        let mut p = BytesStart::new("mxPoint");
        p.push_attribute(("x", format!("{x:.1}").as_str()));
        p.push_attribute(("y", format!("{y:.1}").as_str()));
        if let Some(r) = role {
            p.push_attribute(("as", r));
        }
        w.write_event(Event::Empty(p))?;
        Ok(())
    };
    point(w, pts[0], Some("sourcePoint"))?;
    point(w, pts[pts.len() - 1], Some("targetPoint"))?;
    if pts.len() > 2 {
        let mut arr = BytesStart::new("Array");
        arr.push_attribute(("as", "points"));
        w.write_event(Event::Start(arr))?;
        for &p in &pts[1..pts.len() - 1] {
            point(w, p, None)?;
        }
        w.write_event(Event::End(BytesEnd::new("Array")))?;
    }
    if let Some(o) = offset {
        point(w, o, Some("offset"))?;
    }
    w.write_event(Event::End(BytesEnd::new("mxGeometry")))?;
    w.write_event(Event::End(BytesEnd::new("mxCell")))?;
    Ok(())
}
