//! SVG sequence diagram rendering: participant groups, fragments, dividers, lifelines,
//! activation bars, messages, notes and participant boxes (or actor figures), all
//! drawn from the theme and the geometry `rdg_layout`'s sequence layout computed.

use anyhow::Result;
use quick_xml::{
    Writer,
    events::{BytesDecl, BytesText, Event},
};
use std::io::Cursor;

use rdg_graph::CompiledGraph;
use rdg_layout::{DesignTokens, LayoutResult, SequenceLayoutInfo, actor_figure_h, message_label_lines};
use rdg_render_core::look::{edge_look, group_look, legend_enabled, legend_items, legend_size, node_look};
use rdg_render_core::theme::Theme;

use crate::{Markers, W, close, dash_attr, draw_legend, draw_node, el, f1, open, text_el};

pub fn render_sequence_svg(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    seq: &SequenceLayoutInfo,
    theme: &Theme,
    background: Option<&str>,
    tokens: &DesignTokens,
) -> Result<String> {
    let f = &theme.font;
    let st = &theme.sequence;
    let bg = background.unwrap_or(&theme.canvas.background).to_string();
    let title_w = compiled.title.as_deref().map_or(0.0, |t| 24.0 + t.chars().count() as f64 * tokens.char_width(f.title_size));
    let desc_w = compiled.description.as_deref().map_or(0.0, |d| 24.0 + d.chars().count() as f64 * tokens.char_width(f.description_size));
    let mut canvas_w = (seq.max_x + tokens.px(3.0)).max(title_w.max(desc_w) + tokens.px(3.0));
    let mut canvas_h = seq.lifeline_bottom_y + tokens.px(3.0);
    // The legend (same rules as every other diagram type), under the lifelines.
    let legend = legend_enabled(theme, compiled).then(|| legend_items(theme, compiled)).filter(|v| !v.is_empty());
    let legend_w = (seq.max_x - seq.min_x).max(tokens.px(40.0));
    if let Some(items) = &legend {
        let (lw, lh) = legend_size(items, theme, legend_w);
        canvas_h += lh + tokens.px(1.0);
        canvas_w = canvas_w.max(seq.min_x + lw + tokens.px(3.0));
    }

    // Message looks (a reply keeps its own style; the theme dashes it).
    let mut markers = Markers::default();
    let looks: Vec<_> = seq
        .messages
        .iter()
        .map(|msg| {
            let look = edge_look(theme, &compiled.graph[msg.edge_idx]);
            let head = markers.id(&look.head, &look.color, look.head_fill);
            (look, head)
        })
        .collect();

    let mut buf = Vec::with_capacity(16384);
    let mut w = Writer::new_with_indent(Cursor::new(&mut buf), b' ', 2);
    w.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;
    open(
        &mut w,
        "svg",
        &[
            ("xmlns", "http://www.w3.org/2000/svg".into()),
            ("xmlns:xlink", "http://www.w3.org/1999/xlink".into()),
            ("version", "1.1".into()),
            ("width", format!("{}", canvas_w.round())),
            ("height", format!("{}", canvas_h.round())),
            ("viewBox", format!("0 0 {} {}", canvas_w.round(), canvas_h.round())),
            ("font-family", f.family.clone()),
        ],
    )?;
    open(&mut w, "defs", &[])?;
    if theme.node.shadow {
        w.write_event(Event::Text(BytesText::from_escaped(
            r##"<filter id="card-shadow" x="-10%" y="-10%" width="120%" height="140%"><feDropShadow dx="0" dy="2" stdDeviation="2.5" flood-color="#000000" flood-opacity="0.1"/></filter>"##,
        )))?;
    }
    markers.write_defs(&mut w, theme.edge.head_size)?;
    close(&mut w, "defs")?;
    el(&mut w, "rect", &[("width", "100%".into()), ("height", "100%".into()), ("fill", bg.clone())])?;

    if let Some(title) = &compiled.title {
        let lh = tokens.line_height(f.title_size);
        text_el(&mut w, &[("x", "24".into()), ("y", f1(12.0 + lh * 0.8)), ("font-size", format!("{}", f.title_size)), ("font-weight", "bold".into()), ("fill", theme.title.color.clone())], title)?;
        if let Some(desc) = &compiled.description {
            text_el(
                &mut w,
                &[("x", "24".into()), ("y", f1(12.0 + lh + tokens.line_height(f.description_size) * 0.85)), ("font-size", format!("{}", f.description_size)), ("fill", theme.title.description_color.clone())],
                desc,
            )?;
        }
    }

    // Participant groups, behind everything.
    for g in &seq.groups {
        let gl = group_look(theme, &compiled.groups[g.group]);
        let mut a = vec![
            ("x", f1(g.x)),
            ("y", f1(g.y)),
            ("width", f1(g.w)),
            ("height", f1(g.h)),
            ("rx", f1(gl.corner_radius)),
            ("fill", gl.fill.clone()),
            ("fill-opacity", format!("{:.3}", gl.fill_opacity)),
            ("stroke", gl.stroke.clone()),
            ("stroke-width", f1(gl.stroke_width)),
        ];
        a.extend(dash_attr(gl.dash.as_deref()));
        el(&mut w, "rect", &a)?;
        text_el(
            &mut w,
            &[
                ("x", f1(g.x + tokens.px(1.0))),
                ("y", f1(g.y + tokens.px(1.0) + f.group_title_size * 0.8)),
                ("font-size", format!("{}", f.group_title_size)),
                ("font-weight", if gl.title_bold { "bold" } else { "normal" }.into()),
                ("fill", gl.title_color.clone()),
            ],
            &gl.title,
        )?;
    }

    // Fragments (outer first).
    let lbl = f.edge_label_size;
    let cw = tokens.char_width(lbl);
    for fr in &seq.fragments {
        let fs = &st.fragment;
        el(&mut w, "rect", &[("x", f1(fr.x)), ("y", f1(fr.y)), ("width", f1(fr.w)), ("height", f1(fr.h)), ("fill", "none".into()), ("stroke", fs.stroke.clone()), ("stroke-width", f1(fs.width))])?;
        let tab_w = fr.kind.chars().count() as f64 * cw + tokens.px(2.0);
        let notch = (fr.tab_h * 0.35).min(6.0);
        el(
            &mut w,
            "path",
            &[
                ("d", format!("M {x:.1} {y:.1} H {r:.1} V {b1:.1} L {r2:.1} {b:.1} H {x:.1} Z", x = fr.x, y = fr.y, r = fr.x + tab_w, b1 = fr.y + fr.tab_h - notch, r2 = fr.x + tab_w - notch, b = fr.y + fr.tab_h)),
                ("fill", fs.tab_fill.clone()),
                ("stroke", fs.stroke.clone()),
                ("stroke-width", f1(fs.width)),
            ],
        )?;
        let baseline = fr.y + fr.tab_h / 2.0 + lbl * 0.35;
        text_el(&mut w, &[("x", f1(fr.x + tokens.px(0.75))), ("y", f1(baseline)), ("font-size", format!("{lbl}")), ("font-weight", "bold".into()), ("fill", fs.tab_text.clone())], &fr.kind)?;
        if let Some(g) = &fr.label {
            text_el(&mut w, &[("x", f1(fr.x + tab_w + tokens.px(1.0))), ("y", f1(baseline)), ("font-size", format!("{lbl}")), ("fill", fs.guard_color.clone())], &format!("[{g}]"))?;
        }
        for (sy, guard) in &fr.separators {
            let mut a = vec![("x1", f1(fr.x)), ("y1", f1(*sy)), ("x2", f1(fr.x + fr.w)), ("y2", f1(*sy)), ("stroke", fs.stroke.clone()), ("stroke-width", f1(fs.width))];
            a.extend(dash_attr(Some(&fs.separator_dash)));
            el(&mut w, "line", &a)?;
            if let Some(g) = guard {
                text_el(&mut w, &[("x", f1(fr.x + tokens.px(1.0))), ("y", f1(sy + tokens.line_height(lbl) * 0.85)), ("font-size", format!("{lbl}")), ("fill", fs.guard_color.clone())], &format!("[{g}]"))?;
            }
        }
    }

    // Dividers.
    for d in &seq.dividers {
        let ds = &st.divider;
        let lh = tokens.line_height(lbl);
        let cy = d.y + lh / 2.0;
        let mut a = vec![("x1", f1(seq.min_x)), ("y1", f1(cy)), ("x2", f1(seq.max_x)), ("y2", f1(cy)), ("stroke", ds.color.clone()), ("stroke-width", f1(ds.width))];
        a.extend(dash_attr(ds.dash.as_deref()));
        el(&mut w, "line", &a)?;
        if !d.label.is_empty() {
            let tw = d.label.chars().count() as f64 * cw + tokens.px(2.0);
            let cx = (seq.min_x + seq.max_x) / 2.0;
            el(&mut w, "rect", &[("x", f1(cx - tw / 2.0)), ("y", f1(d.y)), ("width", f1(tw)), ("height", f1(lh)), ("rx", f1(lh / 2.0)), ("fill", bg.clone()), ("stroke", ds.color.clone()), ("stroke-width", f1(ds.width))])?;
            text_el(&mut w, &[("x", f1(cx)), ("y", f1(cy + lbl * 0.35)), ("text-anchor", "middle".into()), ("font-size", format!("{lbl}")), ("fill", theme.text.muted.clone())], &d.label)?;
        }
    }

    // Lifelines.
    for l in &seq.lifelines {
        let ls = &st.lifeline;
        let mut a = vec![("x1", f1(l.x)), ("y1", f1(l.y0)), ("x2", f1(l.x)), ("y2", f1(l.y1)), ("stroke", ls.color.clone()), ("stroke-width", f1(ls.width))];
        a.extend(dash_attr(ls.dash.as_deref()));
        el(&mut w, "line", &a)?;
        if l.destroyed {
            let s = tokens.px(1.0);
            let ink = theme.text.primary.clone();
            el(&mut w, "path", &[("d", format!("M {:.1} {:.1} L {:.1} {:.1} M {:.1} {:.1} L {:.1} {:.1}", l.x - s, l.y1 - s, l.x + s, l.y1 + s, l.x + s, l.y1 - s, l.x - s, l.y1 + s)), ("stroke", ink), ("stroke-width", "2".into())])?;
        }
    }

    // Activation bars.
    for a in &seq.activations {
        let stroke = if st.activation.category_stroke { node_look(theme, &compiled.graph[a.node]).stroke } else { st.activation.stroke.clone() };
        el(&mut w, "rect", &[("x", f1(a.x)), ("y", f1(a.y0)), ("width", f1(a.width)), ("height", f1(a.y1 - a.y0)), ("fill", st.activation.fill.clone()), ("stroke", stroke), ("stroke-width", "1.25".into())])?;
    }

    // Messages.
    let lh = tokens.line_height(lbl);
    for (msg, (look, head)) in seq.messages.iter().zip(&looks) {
        let lines = message_label_lines(compiled, msg.edge_idx);
        let color = if theme.edge_label.use_edge_color { look.color.clone() } else { theme.edge_label.color.clone() };
        let mut attrs = vec![("fill", "none".to_string()), ("stroke", look.color.clone()), ("stroke-width", f1(look.width))];
        attrs.extend(dash_attr(look.dash.as_deref()));
        if let Some(m) = head {
            attrs.push(("marker-end", format!("url(#{m})")));
        }
        if msg.is_self_call {
            let right = msg.from_x.max(msg.to_x) + msg.loop_w;
            attrs.push(("d", format!("M {:.1} {:.1} H {right:.1} V {:.1} H {:.1}", msg.from_x, msg.y, msg.y + msg.loop_h, msg.to_x)));
            el(&mut w, "path", &attrs)?;
            let top = msg.y + (msg.loop_h - lines.len() as f64 * lh) / 2.0;
            for (i, line) in lines.iter().enumerate() {
                text_el(&mut w, &[("x", f1(right + tokens.px(0.75))), ("y", f1(top + lh * (i as f64 + 0.75))), ("font-size", format!("{lbl}")), ("fill", color.clone())], line)?;
            }
        } else {
            attrs.push(("d", format!("M {:.1} {:.1} H {:.1}", msg.from_x, msg.y, msg.to_x)));
            el(&mut w, "path", &attrs)?;
            let cx = (msg.from_x + msg.to_x) / 2.0;
            let n = lines.len() as f64;
            // A backdrop so lifelines the label crosses don't strike through it.
            let tw = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as f64 * cw + 4.0;
            if tw > 4.0 {
                el(&mut w, "rect", &[("x", f1(cx - tw / 2.0)), ("y", f1(msg.y - tokens.px(0.5) - n * lh)), ("width", f1(tw)), ("height", f1(n * lh)), ("fill", theme.edge_label.background.clone())])?;
            }
            for (i, line) in lines.iter().enumerate() {
                let baseline = msg.y - tokens.px(0.5) - (n - 1.0 - i as f64) * lh - lh * 0.2;
                text_el(&mut w, &[("x", f1(cx)), ("y", f1(baseline)), ("text-anchor", "middle".into()), ("font-size", format!("{lbl}")), ("fill", color.clone())], line)?;
            }
        }
    }

    // Notes.
    let nfs = f.node_detail_size;
    let nlh = tokens.line_height(nfs);
    for n in &seq.notes {
        let ns = &st.note;
        let fold = 8.0_f64.min(n.h / 3.0);
        el(
            &mut w,
            "path",
            &[
                ("d", format!("M {x:.1} {y:.1} H {r1:.1} L {r:.1} {y1:.1} V {b:.1} H {x:.1} Z", x = n.x, y = n.y, r1 = n.x + n.w - fold, r = n.x + n.w, y1 = n.y + fold, b = n.y + n.h)),
                ("fill", ns.fill.clone()),
                ("stroke", ns.stroke.clone()),
                ("stroke-width", "1".into()),
            ],
        )?;
        el(&mut w, "path", &[("d", format!("M {:.1} {:.1} V {:.1} H {:.1}", n.x + n.w - fold, n.y, n.y + fold, n.x + n.w)), ("fill", "none".into()), ("stroke", ns.stroke.clone()), ("stroke-width", "1".into())])?;
        let top = n.y + (n.h - n.lines.len() as f64 * nlh) / 2.0;
        for (i, line) in n.lines.iter().enumerate() {
            text_el(&mut w, &[("x", f1(n.x + tokens.px(1.0))), ("y", f1(top + nlh * (i as f64 + 0.75))), ("font-size", format!("{nfs}")), ("fill", ns.text.clone())], line)?;
        }
    }

    // Participants, on top.
    let shadow_attr = || theme.node.shadow.then(|| ("filter", "url(#card-shadow)".to_string()));
    for idx in compiled.graph.node_indices() {
        let nd = &compiled.graph[idx];
        let Some(nl) = layout.positions.get(&idx) else { continue };
        let look = node_look(theme, nd);
        if look.shape == "actor" {
            draw_actor(&mut w, theme, tokens, nd, nl.x + nl.width / 2.0, nl.y, &look.stroke, &look.title_color)?;
        } else {
            draw_node(&mut w, theme, tokens, nd, nl, &look, idx.index(), &shadow_attr)?;
        }
    }

    if let Some(items) = &legend {
        let lw = legend_size(items, theme, legend_w).0;
        draw_legend(&mut w, theme, tokens, items, seq.min_x, seq.lifeline_bottom_y + tokens.px(2.0), legend_w, lw, rdg_render_core::frame::Align::Left)?;
    }

    close(&mut w, "svg")?;
    String::from_utf8(buf).map_err(|e| anyhow::anyhow!("invalid UTF-8 in svg output: {e}"))
}

/// A stick figure in `ink`, its caption (title bold, details muted) underneath.
#[allow(clippy::too_many_arguments)]
fn draw_actor(w: &mut W, theme: &Theme, tokens: &DesignTokens, nd: &rdg_graph::NodeData, cx: f64, top: f64, ink: &str, text: &str) -> Result<()> {
    let h = actor_figure_h(tokens) - tokens.px(0.5);
    let (r, neck, hip) = (h * 0.16, top + h * 0.32, top + h * 0.68);
    let d = format!(
        "M {cx:.1} {neck:.1} V {hip:.1} M {l:.1} {arm:.1} H {rr:.1} M {cx:.1} {hip:.1} L {l2:.1} {b:.1} M {cx:.1} {hip:.1} L {r2:.1} {b:.1}",
        l = cx - h * 0.3,
        rr = cx + h * 0.3,
        arm = top + h * 0.45,
        l2 = cx - h * 0.26,
        r2 = cx + h * 0.26,
        b = top + h
    );
    el(w, "circle", &[("cx", f1(cx)), ("cy", f1(top + r)), ("r", f1(r)), ("fill", "none".into()), ("stroke", ink.to_string()), ("stroke-width", "1.75".into())])?;
    el(w, "path", &[("d", d), ("fill", "none".into()), ("stroke", ink.to_string()), ("stroke-width", "1.75".into()), ("stroke-linecap", "round".into())])?;
    let fs = theme.font.node_title_size;
    let lh = tokens.line_height(fs);
    // The same lines the layout sized the caption for.
    for (i, line) in rdg_layout::node_lines(nd, tokens.wrap_chars_normal).iter().enumerate() {
        text_el(
            w,
            &[
                ("x", f1(cx)),
                ("y", f1(top + actor_figure_h(tokens) + lh * (i as f64 + 0.75))),
                ("text-anchor", "middle".into()),
                ("font-size", format!("{fs}")),
                ("font-weight", if line.is_subtitle { "normal" } else { "bold" }.into()),
                ("fill", text.to_string()),
            ],
            &line.text,
        )?;
    }
    Ok(())
}
