//! SVG rendering backend.
//!
//! Hand-rolled SVG (via `quick-xml`). Every colour, font, stroke and size comes from the
//! [`Theme`], through the looks resolved in `rdg_render_core::look` — the same ones the
//! draw.io backend uses, so both outputs agree.

mod path;
mod sequence;

use anyhow::Result;
use quick_xml::{
    Writer,
    events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event},
};
use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;

use petgraph::stable_graph::EdgeIndex;

use rdg_graph::{CompiledGraph, NodeData};
use rdg_layout::{DesignTokens, LayoutResult, NodeLayout};
use rdg_render_core::frame::Align;
use rdg_render_core::look::{LegendItem, NodeLook, edge_look, group_look, legend_heading_width, legend_row_width, legend_rows, node_look};
use rdg_render_core::routing::EdgeRoutingPlan;
use rdg_render_core::theme::{ResolvedEdge, Theme};
use rdg_render_core::typography::{latex_to_unicode, parse_inline_spans, to_subscript, to_superscript, wrap_and_classify_label};

use path::build_orthogonal_svg_path;
use sequence::render_sequence_svg;

pub(crate) type W<'a> = Writer<Cursor<&'a mut Vec<u8>>>;

pub(crate) fn f1(v: f64) -> String {
    format!("{v:.1}")
}

pub(crate) fn el(w: &mut W, name: &str, attrs: &[(&str, String)]) -> Result<()> {
    let mut e = BytesStart::new(name);
    for (k, v) in attrs {
        e.push_attribute((*k, v.as_str()));
    }
    w.write_event(Event::Empty(e))?;
    Ok(())
}

pub(crate) fn open(w: &mut W, name: &str, attrs: &[(&str, String)]) -> Result<()> {
    let mut e = BytesStart::new(name);
    for (k, v) in attrs {
        e.push_attribute((*k, v.as_str()));
    }
    w.write_event(Event::Start(e))?;
    Ok(())
}

pub(crate) fn close(w: &mut W, name: &str) -> Result<()> {
    w.write_event(Event::End(BytesEnd::new(name)))?;
    Ok(())
}

pub(crate) fn text_el(w: &mut W, attrs: &[(&str, String)], content: &str) -> Result<()> {
    open(w, "text", attrs)?;
    w.write_event(Event::Text(BytesText::new(content)))?;
    close(w, "text")
}

/// Stroke dash attribute for a dash pattern.
pub(crate) fn dash_attr(dash: Option<&str>) -> Option<(&'static str, String)> {
    dash.map(|d| ("stroke-dasharray", d.to_string()))
}

// ---------------------------------------------------------------------------
// Arrowhead markers
// ---------------------------------------------------------------------------

/// Arrowheads used, keyed by `(draw.io marker name, colour, filled)` → marker id. Ids are
/// stable per kind (`marker-er-many-…`, `marker-uml-triangle-…`) so they're greppable.
#[derive(Default)]
pub(crate) struct Markers(BTreeMap<(String, String, bool), String>);

impl Markers {
    pub(crate) fn id(&mut self, kind: &str, color: &str, filled: bool) -> Option<String> {
        let k = kind.to_ascii_lowercase();
        if k == "none" || k.is_empty() {
            return None;
        }
        let key = (k.clone(), color.to_string(), filled);
        let n = self.0.len();
        let family = match k.as_str() {
            "ermany" => "er-many",
            "erone" | "ermandone" => "er-one",
            "erzerotoone" => "er-zero-one",
            "block" | "blockthin" | "classic" | "classicthin" if !filled => "uml-triangle",
            "diamond" | "diamondthin" => "diamond",
            "oval" | "circle" => "circle",
            "open" | "openthin" => "open",
            _ => "arrow",
        };
        Some(self.0.entry(key).or_insert_with(|| format!("marker-{family}-{n}")).clone())
    }

    pub(crate) fn write_defs(&self, w: &mut W, size: f64) -> Result<()> {
        for ((kind, color, filled), id) in &self.0 {
            let s = size;
            let fill = if *filled { color.as_str() } else { "none" };
            // Drawn pointing right, tip at (s, s/2); `auto-start-reverse` flips tails.
            let (vw, vh, body) = match kind.as_str() {
                "ermany" => (s * 1.6, s * 1.6, format!(
                    r#"<path d="M0 0 L{a} {h} L0 {b} M{a} 0 V{b}" fill="none" stroke="{color}" stroke-width="1.2"/>"#,
                    a = s * 1.4, h = s * 0.8, b = s * 1.6
                )),
                "erone" | "ermandone" => (s * 1.6, s * 1.6, format!(
                    r#"<path d="M{a} 0 V{b} M{c} 0 V{b}" fill="none" stroke="{color}" stroke-width="1.2"/>"#,
                    a = s * 0.7, b = s * 1.6, c = s * 1.1
                )),
                "erzerotoone" => (s * 2.0, s * 1.6, format!(
                    r#"<circle cx="{cx}" cy="{cy}" r="{r}" fill="none" stroke="{color}" stroke-width="1.2"/><path d="M{x} 0 V{b}" stroke="{color}" stroke-width="1.2"/>"#,
                    cx = s * 0.6, cy = s * 0.8, r = s * 0.45, x = s * 1.5, b = s * 1.6
                )),
                "diamond" | "diamondthin" => (s * 2.0, s, format!(
                    r#"<path d="M0 {h} L{m} 0 L{e} {h} L{m} {s} Z" fill="{fill}" stroke="{color}" stroke-width="1"/>"#,
                    h = s / 2.0, m = s, e = s * 2.0
                )),
                "oval" | "circle" => (s, s, format!(r#"<circle cx="{h}" cy="{h}" r="{r}" fill="{fill}" stroke="{color}"/>"#, h = s / 2.0, r = s / 2.0 - 0.5)),
                "open" | "openthin" => (s, s, format!(
                    r#"<path d="M0 0 L{s} {h} L0 {s}" fill="none" stroke="{color}" stroke-width="1.3" stroke-linejoin="round"/>"#,
                    h = s / 2.0
                )),
                _ => (s, s, format!(
                    r#"<path d="M0 0 L{s} {h} L0 {s} Z" fill="{fill}" stroke="{color}" stroke-width="1" stroke-linejoin="round"/>"#,
                    h = s / 2.0
                )),
            };
            let markup = format!(
                r#"<marker id="{id}" viewBox="-1 -1 {a} {b}" refX="{vw}" refY="{ry}" markerWidth="{a}" markerHeight="{b}" markerUnits="userSpaceOnUse" orient="auto-start-reverse">{body}</marker>"#,
                a = vw + 2.0,
                b = vh + 2.0,
                ry = vh / 2.0,
            );
            w.write_event(Event::Text(BytesText::from_escaped(markup)))?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Render the compiled, laid-out graph to an SVG string.
///
/// `edge_plans` is the output of [`rdg_render_core::review::compute_reviewed_layout`] for
/// this exact `layout` (see `rdg_render_drawio::render_drawio`). Ignored for sequence
/// diagrams, which never route edges.
///
/// # Errors
///
/// Returns an error if XML serialization fails (practically infallible).
pub fn render_svg(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    edge_plans: &HashMap<EdgeIndex, EdgeRoutingPlan>,
    theme: &Theme,
    background: Option<&str>,
    tokens: &DesignTokens,
) -> Result<String> {
    if let Some(seq) = &layout.sequence_info {
        return render_sequence_svg(compiled, layout, seq, theme, background, tokens);
    }
    let f = &theme.font;

    // Canvas = everything drawn — title and legend blocks included, each placed in the
    // diagram's largest white patch — plus a margin mirroring the left/top one.
    let frame = rdg_render_core::frame::compute_frame(compiled, layout, edge_plans, theme, tokens);
    let (canvas_w, canvas_h) = frame.as_ref().map_or((0.0, 0.0), |f| f.canvas_size());
    let canvas_w = canvas_w.max(tokens.px(15.0));
    let canvas_h = canvas_h.max(tokens.px(12.5));

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

    // --- Resolve edges first: their looks decide which markers exist -------------
    let mut markers = Markers::default();
    struct EdgeDraw {
        pts: Vec<(f64, f64)>,
        look: ResolvedEdge,
        head: Option<String>,
        tail: Option<String>,
    }
    let mut edges: Vec<(EdgeIndex, EdgeDraw)> = Vec::new();
    for edge_idx in compiled.graph.edge_indices() {
        let edge_data = &compiled.graph[edge_idx];
        let Some(plan) = edge_plans.get(&edge_idx) else { continue };
        let Some(pts) = rdg_render_core::annotate::edge_polyline(compiled, layout, edge_idx, Some(plan), tokens) else {
            continue;
        };
        let look = edge_look(theme, edge_data);
        let head = markers.id(&look.head, &look.color, look.head_fill);
        let tail = look.tail.as_deref().and_then(|t| markers.id(t, &look.color, look.tail_fill));
        edges.push((edge_idx, EdgeDraw { pts, look, head, tail }));
    }

    // --- Background, grid, defs ------------------------------------------------------
    let bg = background.unwrap_or(&theme.canvas.background).to_string();
    open(&mut w, "defs", &[])?;
    if theme.node.shadow {
        let shadow = if theme.mode == rdg_render_core::theme::Mode::Dark { "0.35" } else { "0.10" };
        w.write_event(Event::Text(BytesText::from_escaped(format!(
            r##"<filter id="card-shadow" x="-10%" y="-10%" width="120%" height="140%"><feDropShadow dx="0" dy="2" stdDeviation="2.5" flood-color="#000000" flood-opacity="{shadow}"/></filter>"##
        ))))?;
    }
    if let Some(grid) = &theme.canvas.grid {
        let g = theme.canvas.grid_spacing;
        w.write_event(Event::Text(BytesText::from_escaped(format!(
            r#"<pattern id="canvas-grid" width="{g}" height="{g}" patternUnits="userSpaceOnUse"><circle cx="1" cy="1" r="1" fill="{grid}"/></pattern>"#
        ))))?;
    }
    markers.write_defs(&mut w, theme.edge.head_size)?;
    close(&mut w, "defs")?;
    el(&mut w, "rect", &[("width", "100%".into()), ("height", "100%".into()), ("fill", bg.clone())])?;
    if theme.canvas.grid.is_some() {
        el(&mut w, "rect", &[("width", "100%".into()), ("height", "100%".into()), ("fill", "url(#canvas-grid)".into())])?;
    }
    let shadow_attr = || theme.node.shadow.then(|| ("filter", "url(#card-shadow)".to_string()));

    // --- Title -----------------------------------------------------------------------
    if let Some(t) = frame.as_ref().and_then(|f| f.title.as_ref()) {
        // Lines anchored to the side the block is aligned to.
        let (ax, anchor) = match t.align {
            Align::Left => (t.x, "start"),
            Align::Center => (t.x + t.w / 2.0, "middle"),
            Align::Right => (t.x + t.w, "end"),
        };
        let mut y = t.y;
        for line in &t.title_lines {
            text_el(
                &mut w,
                &[("x", f1(ax)), ("y", f1(y + t.title_lh * 0.78)), ("text-anchor", anchor.into()), ("font-size", format!("{}", f.title_size)), ("font-weight", "bold".into()), ("fill", theme.title.color.clone())],
                line,
            )?;
            y += t.title_lh;
        }
        if !t.description_lines.is_empty() {
            y += t.gap;
        }
        for line in &t.description_lines {
            text_el(
                &mut w,
                &[("x", f1(ax)), ("y", f1(y + t.description_lh * 0.78)), ("text-anchor", anchor.into()), ("font-size", format!("{}", f.description_size)), ("fill", theme.title.description_color.clone())],
                line,
            )?;
            y += t.description_lh;
        }
    }

    // --- Groups ----------------------------------------------------------------------
    let rects = rdg_render_core::canvas::group_rects(compiled, layout, tokens);
    let mut gi = 0;
    for group in &compiled.groups {
        if !group.nodes.iter().any(|id| compiled.node_map.get(id).is_some_and(|i| layout.positions.contains_key(i))) {
            continue;
        }
        let Some(&(gx, gy, gw, gh)) = rects.get(gi) else { break };
        gi += 1;
        let look = group_look(theme, group);
        let mut attrs = vec![
            ("x", f1(gx)),
            ("y", f1(gy)),
            ("width", f1(gw)),
            ("height", f1(gh)),
            ("rx", f1(look.corner_radius)),
            ("fill", look.fill.clone()),
            ("fill-opacity", format!("{:.3}", look.fill_opacity)),
            ("stroke", look.stroke.clone()),
            ("stroke-width", f1(look.stroke_width)),
        ];
        attrs.extend(dash_attr(look.dash.as_deref()));
        el(&mut w, "rect", &attrs)?;
        let mut tx = gx + 12.0;
        let ty = gy + 8.0 + tokens.line_height(f.group_title_size) * 0.8;
        if let Some(icon) = group.resolved_icon() {
            let s = f.group_title_size + 4.0;
            if let Some(m) = rdg_icons::render_icon_svg(&icon, theme.icon_style(), tx, ty - s * 0.8, s, &format!("g{gi}-")) {
                w.write_event(Event::Text(BytesText::from_escaped(m)))?;
                tx += s + 6.0;
            }
        }
        text_el(
            &mut w,
            &[
                ("x", f1(tx)),
                ("y", f1(ty)),
                ("font-size", format!("{}", f.group_title_size)),
                ("font-weight", if look.title_bold { "bold" } else { "normal" }.into()),
                ("fill", look.title_color.clone()),
            ],
            &look.title,
        )?;
    }

    // --- Edges (under nodes) -----------------------------------------------------------
    for (_, e) in &edges {
        let d = build_orthogonal_svg_path(e.pts[0], e.pts[e.pts.len() - 1], &e.pts[1..e.pts.len() - 1]);
        let mut attrs = vec![
            ("d", d),
            ("fill", "none".into()),
            ("stroke", e.look.color.clone()),
            ("stroke-width", f1(e.look.width)),
        ];
        attrs.extend(dash_attr(e.look.dash.as_deref()));
        if let Some(m) = &e.head {
            attrs.push(("marker-end", format!("url(#{m})")));
        }
        if let Some(m) = &e.tail {
            attrs.push(("marker-start", format!("url(#{m})")));
        }
        el(&mut w, "path", &attrs)?;
    }

    // --- Nodes ---------------------------------------------------------------------------
    let mut icon_n = 0usize;
    for node_idx in compiled.graph.node_indices() {
        let nd = &compiled.graph[node_idx];
        let Some(nl) = layout.positions.get(&node_idx) else { continue };
        let look = node_look(theme, nd);
        icon_n += 1;
        draw_node(&mut w, theme, tokens, nd, nl, &look, icon_n, &shadow_attr)?;
    }

    // --- Edge labels and flow badges, on top ---------------------------------------------
    let annotations = rdg_render_core::annotate::place_edge_annotations(compiled, layout, edge_plans, tokens);
    for (edge_idx, e) in &edges {
        let Some(spot) = annotations.labels.get(edge_idx) else { continue };
        let (x, y, pw, ph) = spot.rect();
        el(
            &mut w,
            "rect",
            &[("x", f1(x)), ("y", f1(y)), ("width", f1(pw)), ("height", f1(ph)), ("rx", "2".into()), ("fill", theme.edge_label.background.clone())],
        )?;
        let color = if theme.edge_label.use_edge_color { e.look.color.clone() } else { theme.edge_label.color.clone() };
        let lines: Vec<&str> = spot.text.lines().collect();
        let line_h = ph / lines.len().max(1) as f64;
        for (i, line) in lines.iter().enumerate() {
            let baseline = y + line_h * (i as f64 + 0.5) + f.edge_label_size * 0.35;
            text_el(
                &mut w,
                &[
                    ("x", f1(spot.cx)),
                    ("y", f1(baseline)),
                    ("text-anchor", "middle".into()),
                    ("font-size", format!("{}", f.edge_label_size)),
                    ("fill", color.clone()),
                ],
                line,
            )?;
        }
    }
    for edge_idx in compiled.graph.edge_indices() {
        let (Some(step), Some(&(bx, by))) = (compiled.graph[edge_idx].step, annotations.badges.get(&edge_idx)) else {
            continue;
        };
        el(&mut w, "circle", &[("cx", f1(bx)), ("cy", f1(by)), ("r", f1(tokens.badge_radius)), ("fill", theme.badge.fill.clone())])?;
        text_el(
            &mut w,
            &[
                ("x", f1(bx)),
                ("y", f1(by + f.badge_size * 0.35)),
                ("text-anchor", "middle".into()),
                ("font-size", format!("{}", f.badge_size)),
                ("font-weight", "bold".into()),
                ("fill", theme.badge.text.clone()),
            ],
            &step.to_string(),
        )?;
    }

    // --- Legend ------------------------------------------------------------------------
    if let Some(l) = frame.as_ref().and_then(|f| f.legend.as_ref()) {
        draw_legend(&mut w, theme, tokens, &l.items, l.x, l.y, l.max_w, l.w, l.align)?;
    }

    close(&mut w, "svg")?;
    Ok(String::from_utf8(buf)?)
}

// ---------------------------------------------------------------------------
// Nodes
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_node(
    w: &mut W,
    theme: &Theme,
    tokens: &DesignTokens,
    nd: &NodeData,
    nl: &NodeLayout,
    look: &NodeLook,
    n: usize,
    shadow_attr: &dyn Fn() -> Option<(&'static str, String)>,
) -> Result<()> {
    let f = &theme.font;
    let (cx, cy) = (nl.x + nl.width / 2.0, nl.y + nl.height / 2.0);
    let paint = |extra: &[(&'static str, String)]| {
        let mut a = vec![
            ("fill", look.fill.clone()),
            ("fill-opacity", format!("{:.3}", look.fill_opacity)),
            ("stroke", look.stroke.clone()),
            ("stroke-width", f1(look.stroke_width)),
        ];
        a.extend_from_slice(extra);
        a.extend(shadow_attr());
        a
    };

    match look.shape.as_str() {
        "start" => {
            el(w, "circle", &[("cx", f1(cx)), ("cy", f1(cy)), ("r", f1(nl.width / 2.0 - 2.0)), ("fill", look.fill.clone())])?;
            return label_below(w, theme, tokens, &nd.label, cx, nl.y + nl.height, &look.title_color);
        }
        "end" => {
            el(w, "circle", &[("cx", f1(cx)), ("cy", f1(cy)), ("r", f1(nl.width / 2.0 - 1.0)), ("fill", "none".into()), ("stroke", look.stroke.clone()), ("stroke-width", "2".into())])?;
            el(w, "circle", &[("cx", f1(cx)), ("cy", f1(cy)), ("r", f1(nl.width / 2.0 - 6.0)), ("fill", look.fill.clone())])?;
            return label_below(w, theme, tokens, &nd.label, cx, nl.y + nl.height, &look.title_color);
        }
        "choice" | "diamond" => {
            let d = format!(
                "M {cx:.1} {t:.1} L {r:.1} {cy:.1} L {cx:.1} {b:.1} L {l:.1} {cy:.1} Z",
                t = nl.y,
                r = nl.x + nl.width,
                b = nl.y + nl.height,
                l = nl.x
            );
            el(w, "path", &paint(&[("d", d)]))?;
            if look.shape == "choice" {
                return label_below(w, theme, tokens, &nd.label, cx, nl.y + nl.height, &look.title_color);
            }
        }
        "ellipse" => {
            el(w, "ellipse", &paint(&[("cx", f1(cx)), ("cy", f1(cy)), ("rx", f1(nl.width / 2.0)), ("ry", f1(nl.height / 2.0))]))?;
        }
        "cylinder" => {
            let (rx, rh) = (nl.width / 2.0, theme.node.cylinder_cap);
            let body = format!(
                "M {x:.1} {t:.1} L {x:.1} {b:.1} A {rx:.1} {rh:.1} 0 0 0 {r:.1} {b:.1} L {r:.1} {t:.1} Z",
                x = nl.x,
                t = nl.y + rh,
                b = nl.y + nl.height - rh,
                r = nl.x + nl.width
            );
            el(w, "path", &paint(&[("d", body)]))?;
            let mut cap = vec![("cx", f1(cx)), ("cy", f1(nl.y + rh)), ("rx", f1(rx)), ("ry", f1(rh))];
            cap.extend([("fill", look.fill.clone()), ("fill-opacity", format!("{:.3}", look.fill_opacity)), ("stroke", look.stroke.clone()), ("stroke-width", f1(look.stroke_width))]);
            el(w, "ellipse", &cap)?;
        }
        "icon" => {
            let halo = tokens.icon_halo_size();
            if let Some(plate) = &theme.icon.backdrop {
                // The halo filled with a light plate, so dark marks stay visible.
                if tokens.icon_halo_circle {
                    el(w, "circle", &[("cx", f1(cx)), ("cy", f1(nl.y + halo / 2.0)), ("r", f1(halo / 2.0)), ("fill", plate.clone())])?;
                } else {
                    el(
                        w,
                        "rect",
                        &[("x", f1(cx - halo / 2.0)), ("y", f1(nl.y)), ("width", f1(halo)), ("height", f1(halo)), ("rx", f1(look.corner_radius + 2.0)), ("fill", plate.clone())],
                    )?;
                }
            }
            if let Some(key) = nd.icon.as_deref() {
                let size = tokens.icon_node_size;
                let (dx, dy) = rdg_render_core::style::logo_offset(nl.width, size, halo);
                if let Some(m) = rdg_icons::render_icon_svg(key, theme.icon_style(), nl.x + dx, nl.y + dy, size, &format!("n{n}-")) {
                    w.write_event(Event::Text(BytesText::from_escaped(m)))?;
                }
            }
            let top = nl.y + halo + tokens.px(0.25);
            return node_text(w, theme, tokens, nd, look, cx, top, None, None);
        }
        _ => {
            el(
                w,
                "rect",
                &paint(&[("x", f1(nl.x)), ("y", f1(nl.y)), ("width", f1(nl.width)), ("height", f1(nl.height)), ("rx", f1(look.corner_radius))]),
            )?;
        }
    }

    if !nd.fields.is_empty() {
        return field_card(w, theme, nd, nl, look);
    }
    // Vertically centred in the box (a cylinder's body, below its cap); the icon sits
    // inline before the first line, the pair centred together.
    let lines = wrap_and_classify_label(&nd.label, if look.shape == "diamond" { tokens.wrap_chars_diamond } else { tokens.wrap_chars_normal });
    let tech = nd.technology.as_deref().filter(|t| !nd.label.contains(*t));
    let inline_icon = nd.icon.as_deref().filter(|k| rdg_icons::icon_document(k, theme.icon_style()).is_some());
    let text_h: f64 = lines
        .iter()
        .enumerate()
        .map(|(i, l)| if i == 0 { first_line_height(tokens, f, l.is_subtitle, inline_icon.is_some()) } else { tokens.line_height(if l.is_subtitle { f.node_detail_size } else { f.node_title_size }) })
        .sum::<f64>()
        + tech.map_or(0.0, |_| tokens.line_height(f.node_detail_size));
    let (top, bottom) = if look.shape == "cylinder" { (nl.y + 2.0 * theme.node.cylinder_cap, nl.y + nl.height - theme.node.cylinder_cap) } else { (nl.y, nl.y + nl.height) };
    let start = top + ((bottom - top) - text_h) / 2.0;
    node_text(w, theme, tokens, nd, look, cx, start, Some(lines), inline_icon.map(|k| (k, n)))
}

/// Height of a label's first line: its text, or the inline icon beside it if taller.
fn first_line_height(tokens: &DesignTokens, f: &rdg_render_core::theme::Fonts, is_subtitle: bool, has_icon: bool) -> f64 {
    let lh = tokens.line_height(if is_subtitle { f.node_detail_size } else { f.node_title_size });
    if has_icon { lh.max(tokens.icon_size + 4.0) } else { lh }
}

/// A node's label lines starting at `top`, centred on `cx`.
#[allow(clippy::too_many_arguments)]
fn node_text(
    w: &mut W,
    theme: &Theme,
    tokens: &DesignTokens,
    nd: &NodeData,
    look: &NodeLook,
    cx: f64,
    top: f64,
    lines: Option<Vec<rdg_render_core::typography::ProcessedLine>>,
    inline_icon: Option<(&str, usize)>,
) -> Result<()> {
    let f = &theme.font;
    let lines = lines.unwrap_or_else(|| wrap_and_classify_label(&nd.label, tokens.wrap_chars_normal));
    let mut y = top;
    for (i, pl) in lines.iter().enumerate() {
        let size = if pl.is_subtitle { f.node_detail_size } else { f.node_title_size };
        let lh = if i == 0 { first_line_height(tokens, f, pl.is_subtitle, inline_icon.is_some()) } else { tokens.line_height(size) };
        // Text sits centred in its line box (the line box is taller beside an icon).
        let baseline = y + lh / 2.0 + size * 0.35;
        let mut cx = cx;
        if let (0, Some((key, n))) = (i, inline_icon) {
            let plain: String = parse_inline_spans(&pl.text).into_iter().map(|s| s.text).collect();
            let text_w = plain.chars().count() as f64 * tokens.char_width(size);
            let (s, gap) = (tokens.icon_size, tokens.icon_reserve - tokens.icon_size);
            let x0 = cx - (s + gap + text_w) / 2.0;
            if let Some(plate) = theme.icon.backdrop.as_ref().filter(|_| !rdg_icons::is_glyph(key)) {
                el(w, "rect", &[("x", f1(x0 - 1.0)), ("y", f1(y + (lh - s) / 2.0 - 1.0)), ("width", f1(s + 2.0)), ("height", f1(s + 2.0)), ("rx", "3".into()), ("fill", plate.clone())])?;
            }
            if let Some(m) = rdg_icons::render_icon_svg(key, theme.icon_style(), x0, y + (lh - s) / 2.0, s, &format!("n{n}-")) {
                w.write_event(Event::Text(BytesText::from_escaped(m)))?;
            }
            cx = x0 + s + gap + text_w / 2.0;
        }
        y += lh;
        let color = if pl.is_subtitle { &look.detail_color } else { &look.title_color };
        let weight = if !pl.is_subtitle && theme.node.title_bold { "bold" } else { "normal" };
        open(
            w,
            "text",
            &[("x", f1(cx)), ("y", f1(baseline)), ("text-anchor", "middle".into()), ("font-size", format!("{size}")), ("font-weight", weight.into()), ("fill", color.clone())],
        )?;
        for span in parse_inline_spans(&pl.text) {
            let t = if span.style.is_math {
                latex_to_unicode(&span.text)
            } else if span.style.is_subscript {
                to_subscript(&span.text)
            } else if span.style.is_superscript {
                to_superscript(&span.text)
            } else {
                span.text
            };
            let mut attrs: Vec<(&str, String)> = Vec::new();
            if span.style.is_code {
                attrs.push(("font-family", f.code_family.clone()));
            }
            if span.style.is_bold {
                attrs.push(("font-weight", "bold".into()));
            }
            if span.style.is_italic || span.style.is_math {
                attrs.push(("font-style", "italic".into()));
            }
            if span.style.is_underline {
                attrs.push(("text-decoration", "underline".into()));
            }
            if span.style.is_strikethrough {
                attrs.push(("text-decoration", "line-through".into()));
            }
            open(w, "tspan", &attrs)?;
            w.write_event(Event::Text(BytesText::new(&t)))?;
            close(w, "tspan")?;
        }
        close(w, "text")?;
    }
    if let Some(t) = nd.technology.as_deref().filter(|t| !nd.label.contains(*t)) {
        let lh = tokens.line_height(f.node_detail_size);
        text_el(
            w,
            &[("x", f1(cx)), ("y", f1(y + lh * 0.75)), ("text-anchor", "middle".into()), ("font-size", format!("{}", f.node_detail_size)), ("fill", look.detail_color.clone())],
            &format!("[{t}]"),
        )?;
    }
    Ok(())
}

/// A marker's label, underneath it.
fn label_below(w: &mut W, theme: &Theme, tokens: &DesignTokens, label: &str, cx: f64, top: f64, color: &str) -> Result<()> {
    let size = theme.font.node_title_size;
    let lh = tokens.line_height(size);
    for (i, pl) in wrap_and_classify_label(label, tokens.wrap_chars_normal).iter().enumerate() {
        let plain: String = parse_inline_spans(&pl.text).into_iter().map(|s| s.text).collect();
        text_el(
            w,
            &[
                ("x", f1(cx)),
                ("y", f1(top + lh * (i as f64 + 0.93))),
                ("text-anchor", "middle".into()),
                ("font-size", format!("{size}")),
                ("font-weight", "bold".into()),
                ("fill", color.to_string()),
            ],
            &plain,
        )?;
    }
    Ok(())
}

/// Table / class card body: name header, divider, one row per field.
fn field_card(w: &mut W, theme: &Theme, nd: &NodeData, nl: &NodeLayout, look: &NodeLook) -> Result<()> {
    let f = &theme.font;
    let lower = nd.node_type.to_ascii_lowercase();
    let cx = nl.x + nl.width / 2.0;
    let mut y = nl.y + if look.shape == "cylinder" { 2.0 * theme.node.cylinder_cap } else { 0.0 } + 16.0;
    if let Some(st) = match lower.as_str() {
        "interface" => Some("«interface»"),
        "abstract_class" => Some("«abstract»"),
        _ => None,
    } {
        text_el(w, &[("x", f1(cx)), ("y", f1(y)), ("text-anchor", "middle".into()), ("font-size", format!("{}", f.node_detail_size)), ("font-style", "italic".into()), ("fill", look.detail_color.clone())], st)?;
        y += 14.0;
    }
    let mut title = vec![("x", f1(cx)), ("y", f1(y)), ("text-anchor", "middle".into()), ("font-size", format!("{}", f.node_title_size)), ("font-weight", "bold".into()), ("fill", look.title_color.clone())];
    if lower == "abstract_class" {
        title.push(("font-style", "italic".into()));
    }
    text_el(w, &title, &nd.label)?;
    y += 8.0;
    el(w, "line", &[("x1", f1(nl.x)), ("y1", f1(y)), ("x2", f1(nl.x + nl.width)), ("y2", f1(y)), ("stroke", look.stroke.clone()), ("stroke-opacity", "0.4".into())])?;
    for (i, field) in nd.fields.iter().enumerate() {
        let fy = y + 16.0 + i as f64 * 18.0;
        let clean = rdg_layout::strip_markdown_tokens(field);
        let (left, right) = clean.split_once(':').unwrap_or((clean.as_str(), ""));
        let is_pk = clean.to_ascii_uppercase().contains("[PK]");
        let mut la = vec![("x", f1(nl.x + 12.0)), ("y", f1(fy)), ("font-family", f.code_family.clone()), ("font-size", format!("{}", f.node_detail_size))];
        if is_pk {
            la.push(("font-weight", "bold".into()));
            la.push(("fill", look.stroke.clone()));
        } else {
            la.push(("fill", look.title_color.clone()));
        }
        text_el(w, &la, left.trim())?;
        if !right.trim().is_empty() {
            text_el(
                w,
                &[("x", f1(nl.x + nl.width - 12.0)), ("y", f1(fy)), ("text-anchor", "end".into()), ("font-family", f.code_family.clone()), ("font-size", format!("{}", f.node_detail_size)), ("fill", look.detail_color.clone())],
                right.trim(),
            )?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Legend
// ---------------------------------------------------------------------------

/// The legend at `(x0, y0)`: rows wrapped at `max_w`, each row (and the heading) lined
/// up inside the `block_w`-wide block per `align`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_legend(w: &mut W, theme: &Theme, tokens: &DesignTokens, items: &[LegendItem], x0: f64, y0: f64, max_w: f64, block_w: f64, align: Align) -> Result<()> {
    let f = &theme.font;
    text_el(
        w,
        &[("x", f1(align.line_x(x0, block_w, legend_heading_width(theme)))), ("y", f1(y0 + tokens.line_height(f.group_title_size) * 0.8)), ("font-size", format!("{}", f.group_title_size)), ("font-weight", "bold".into()), ("fill", theme.text.primary.clone())],
        "Legend",
    )?;
    let row_h = f.edge_label_size * 1.35 + 10.0;
    let mut y = y0 + f.group_title_size * 1.35 + 6.0;
    for row in legend_rows(items, theme, max_w).iter() {
        let mut x = align.line_x(x0, block_w, legend_row_width(row));
        for (it, width) in row {
            let mid = y + row_h / 2.0;
            let label = match it {
                LegendItem::Category { label, fill, fill_opacity, stroke, .. } => {
                    el(
                        w,
                        "rect",
                        &[("x", f1(x)), ("y", f1(mid - 7.0)), ("width", "22".into()), ("height", "14".into()), ("rx", "3".into()), ("fill", fill.clone()), ("fill-opacity", format!("{fill_opacity:.3}")), ("stroke", stroke.clone()), ("stroke-width", "1.25".into())],
                    )?;
                    label
                }
                LegendItem::Edge { label, look } => {
                    let mut a = vec![("x1", f1(x)), ("y1", f1(mid)), ("x2", f1(x + 22.0)), ("y2", f1(mid)), ("stroke", look.color.clone()), ("stroke-width", f1(look.width))];
                    a.extend(dash_attr(look.dash.as_deref()));
                    el(w, "line", &a)?;
                    label
                }
            };
            text_el(
                w,
                &[("x", f1(x + 30.0)), ("y", f1(mid + f.edge_label_size * 0.35)), ("font-size", format!("{}", f.edge_label_size)), ("fill", theme.text.muted.clone())],
                label,
            )?;
            x += width;
        }
        y += row_h;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rdg_graph::build_graph;
    use rdg_layout::{LayoutConfig, compute_layout};
    use rdg_schema::{DiagramPayload, EdgeDef, NodeDef};

    fn payload_with_types(types: &[(&str, &str)]) -> DiagramPayload {
        DiagramPayload {
            nodes: types
                .iter()
                .map(|(id, ty)| NodeDef {
                    id: id.to_string(),
                    label: id.to_string(),
                    node_type: ty.to_string(),
                    ..Default::default()
                })
                .collect(),
            edges: (0..types.len().saturating_sub(1))
                .map(|i| EdgeDef {
                    from: types[i].0.to_string(),
                    to: types[i + 1].0.to_string(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    fn render(payload: &DiagramPayload, theme: &str) -> String {
        let theme = rdg_render_core::theme::Theme::builtin(theme).unwrap();
        let mut tokens = DesignTokens::default();
        theme.apply_to_tokens(&mut tokens);
        let mut compiled = build_graph(payload).unwrap();
        rdg_render_core::look::prepare_graph(&theme, &mut compiled);
        let config = LayoutConfig { tokens, ..LayoutConfig::default() };
        let layout = compute_layout(&compiled, &config).unwrap();
        let edge_plans = rdg_render_core::routing::plan_all_edge_routes(
            &compiled,
            &layout,
            rdg_render_core::routing::RoutingAlgorithm::CornerHeuristic,
            &tokens,
        );
        render_svg(&compiled, &layout, &edge_plans, &theme, None, &tokens).unwrap()
    }

    #[test]
    fn test_svg_output_is_valid_xml() {
        let xml = render(&payload_with_types(&[("n1", "default")]), "standard");
        assert!(xml.starts_with("<?xml"));
        assert!(xml.contains("<svg"));
        assert!(xml.contains("</svg>"));
    }

    #[test]
    fn test_logos_using_xlink_are_namespaced() {
        // Some devicon logos (go, grpc, …) use `xlink:href`; strict parsers reject the file
        // unless the root declares the prefix.
        let mut p = payload_with_types(&[("n1", "service")]);
        p.nodes[0].language = Some("go".into());
        let xml = render(&p, "light");
        if xml.contains("xlink:") {
            assert!(xml.contains("xmlns:xlink=\"http://www.w3.org/1999/xlink\""));
        }
    }

    #[test]
    fn test_database_and_table_share_accent_color_in_svg() {
        let db = render(&payload_with_types(&[("n1", "database")]), "standard");
        let table = render(&payload_with_types(&[("n1", "table")]), "standard");
        let t = rdg_render_core::theme::Theme::builtin("light").unwrap();
        let want = format!("stroke=\"{}\"", t.category("database").stroke);
        assert!(db.contains(&want) && table.contains(&want));
    }

    #[test]
    fn test_card_shadow_and_cylinder_paths_present() {
        // Shadows are a theme choice: on in `classic`, off in the flat themes.
        let xml = render(&payload_with_types(&[("n1", "database")]), "classic");
        assert!(xml.contains("url(#card-shadow)"));
        assert!(!render(&payload_with_types(&[("n1", "database")]), "light").contains("url(#card-shadow)"));
        assert!(xml.contains("<ellipse"));
    }

    #[test]
    fn test_erd_markers_and_table_cylinder() {
        let payload = DiagramPayload {
            nodes: vec![
                NodeDef {
                    id: "users".to_owned(),
                    label: "users".to_owned(),
                    node_type: "table".to_owned(),
                    fields: vec!["id: uuid [PK]".to_owned()],
                    ..Default::default()
                },
                NodeDef {
                    id: "orders".to_owned(),
                    label: "orders".to_owned(),
                    node_type: "table".to_owned(),
                    ..Default::default()
                },
            ],
            edges: vec![EdgeDef {
                from: "users".to_owned(),
                to: "orders".to_owned(),
                edge_style: Some("one_to_many".to_owned()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let xml = render(&payload, "standard");
        assert!(xml.contains("marker-er-one"));
        assert!(xml.contains("marker-er-many"));
        assert!(xml.contains("[PK]") || xml.contains("id"));
    }

    #[test]
    fn test_uml_markers_and_class_render() {
        let payload = DiagramPayload {
            nodes: vec![
                NodeDef {
                    id: "a".to_owned(),
                    label: "Base".to_owned(),
                    node_type: "class".to_owned(),
                    ..Default::default()
                },
                NodeDef {
                    id: "b".to_owned(),
                    label: "Derived".to_owned(),
                    node_type: "class".to_owned(),
                    ..Default::default()
                },
            ],
            edges: vec![EdgeDef {
                from: "b".to_owned(),
                to: "a".to_owned(),
                edge_style: Some("inheritance".to_owned()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let xml = render(&payload, "standard");
        assert!(xml.contains("marker-uml-triangle"));
    }

    #[test]
    fn test_start_and_end_state_render_as_circles() {
        let xml = render(
            &payload_with_types(&[("s", "start"), ("e", "end")]),
            "standard",
        );
        assert!(xml.contains("<circle"));
    }

    // -----------------------------------------------------------------------
    // Phase G: edge cases + Phase F feature coverage
    // -----------------------------------------------------------------------

    #[test]
    fn test_unicode_labels_produce_valid_escaped_xml() {
        let payload = DiagramPayload {
            nodes: vec![
                NodeDef {
                    id: "n1".to_owned(),
                    label: "支付服务 <script>".to_owned(),
                    ..Default::default()
                },
                NodeDef {
                    id: "n2".to_owned(),
                    label: "B".to_owned(),
                    ..Default::default()
                },
            ],
            edges: vec![EdgeDef {
                from: "n1".to_owned(),
                to: "n2".to_owned(),
                label: Some("é & ü".to_owned()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let xml = render(&payload, "standard");
        assert!(!xml.contains("<script>"));
        assert!(xml.contains("支付服务"));
    }

    #[test]
    fn test_explicit_node_color_overrides_semantic_type_color() {
        let payload = DiagramPayload {
            nodes: vec![NodeDef {
                id: "n1".to_owned(),
                label: "A".to_owned(),
                node_type: "server".to_owned(), // would normally be emerald (#34d399)
                color: Some("#ef4444".to_owned()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let xml = render(&payload, "standard");
        assert!(xml.contains("stroke=\"#ef4444\""));
        assert!(!xml.contains("stroke=\"#34d399\""));
    }

    #[test]
    fn test_class_preset_resolution_reflected_in_svg_stroke() {
        let payload = DiagramPayload::from_yaml(
            r##"
node_styles:
  critical:
    color: "#ef4444"
nodes:
  - id: n1
    label: "A"
    class: critical
edges: []
"##,
        )
        .unwrap();
        let xml = render(&payload, "standard");
        assert!(xml.contains("stroke=\"#ef4444\""));
    }

    #[test]
    fn test_numbered_edges_render_step_badge_circles_and_text() {
        let payload = DiagramPayload::from_yaml(
            r#"
numbered: true
nodes:
  - id: n1
    label: "A"
  - id: n2
    label: "B"
edges:
  - from: n1
    to: n2
"#,
        )
        .unwrap();
        let xml = render(&payload, "standard");
        assert!(xml.contains(">1</text>"));
    }

    #[test]
    fn test_unnumbered_diagram_has_no_step_text() {
        let xml = render(
            &payload_with_types(&[("n1", "default"), ("n2", "default")]),
            "standard",
        );
        assert!(!xml.contains(">1</text>"));
    }
}
