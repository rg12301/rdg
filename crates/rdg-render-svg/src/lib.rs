//! SVG rendering backend.
//!
//! Uses hand-rolled SVG primitives (via `quick-xml`) to avoid bringing in a heavy
//! dependency while still producing well-formed, human-readable output.

mod path;
mod sequence;

use anyhow::Result;
use quick_xml::{
    Writer,
    events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event},
};
use std::collections::HashMap;
use std::io::Cursor;

use petgraph::stable_graph::EdgeIndex;

use rdg_graph::CompiledGraph;
use rdg_layout::{DesignTokens, LayoutResult};
use rdg_render_core::routing::{EdgeRoutingPlan, Side, compute_edge_waypoints, port_point};
use rdg_render_core::style::{edge_style_colors, node_accent_color};
use rdg_render_core::typography::{
    latex_to_unicode, parse_inline_spans, to_subscript, to_superscript, wrap_and_classify_label,
};

use path::build_orthogonal_svg_path;
use sequence::render_sequence_svg;

/// Writes a centered, bold, word-wrapped label starting `top_y` below a small fixed-size
/// marker shape (start/end/choice state circles — see `estimate_node_size_inner` in
/// `rdg-layout`, which sizes those to ~28-36px regardless of label length). Centering a
/// real label on top of a marker that small would bury the text inside the shape, so
/// every marker-shaped node places its label below itself instead, the same fix applied
/// to the draw.io backend's `verticalLabelPosition=bottom`.
fn write_label_below_marker<W: std::io::Write>(
    w: &mut Writer<W>,
    cx: f64,
    top_y: f64,
    label: &str,
    title_color: &str,
    tokens: &DesignTokens,
) -> Result<()> {
    let lines = wrap_and_classify_label(label, tokens.wrap_chars_normal);
    if lines.is_empty() {
        return Ok(());
    }

    let line_height = tokens.line_height(tokens.font_size);
    let mut text = BytesStart::new("text");
    let first_baseline = top_y + line_height * 0.93;
    let font_size = tokens.font_size.round().to_string();
    text.push_attribute(("x", format!("{cx:.1}").as_str()));
    text.push_attribute(("y", format!("{first_baseline:.1}").as_str()));
    text.push_attribute(("text-anchor", "middle"));
    text.push_attribute(("font-family", "Inter, Helvetica, sans-serif"));
    text.push_attribute(("font-size", font_size.as_str()));
    text.push_attribute(("font-weight", "bold"));
    text.push_attribute(("fill", title_color));
    w.write_event(Event::Start(text))?;

    for (line_idx, pl) in lines.iter().enumerate() {
        let plain: String = parse_inline_spans(&pl.text).into_iter().map(|s| s.text).collect();
        let mut tspan = BytesStart::new("tspan");
        tspan.push_attribute(("x", format!("{cx:.1}").as_str()));
        if line_idx > 0 {
            tspan.push_attribute(("dy", format!("{line_height:.1}").as_str()));
        }
        w.write_event(Event::Start(tspan))?;
        w.write_event(Event::Text(BytesText::new(&plain)))?;
        w.write_event(Event::End(BytesEnd::new("tspan")))?;
    }
    w.write_event(Event::End(BytesEnd::new("text")))?;
    Ok(())
}

/// Render the compiled, laid-out graph to an SVG string.
///
/// `edge_plans` is expected to be the output of
/// [`rdg_render_core::routing::plan_all_edge_routes`] (or, more commonly,
/// [`rdg_render_core::review::compute_reviewed_layout`]) for this exact `layout` — see
/// `rdg_render_drawio::render_drawio`'s doc comment for why this isn't computed internally.
/// Ignored for sequence diagrams, which never route edges.
///
/// # Errors
///
/// Returns an error if XML serialization fails (practically infallible for well-formed inputs).
pub fn render_svg(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    edge_plans: &HashMap<EdgeIndex, EdgeRoutingPlan>,
    theme: &str,
    background: Option<&str>,
    tokens: &DesignTokens,
) -> Result<String> {
    if let Some(seq) = &layout.sequence_info {
        return render_sequence_svg(compiled, layout, seq, theme, background, tokens);
    }

    // Canvas = everything actually drawn (nodes, group containers, routed waypoints,
    // title) plus a right/bottom margin mirroring the left/top one.
    let bounds = rdg_render_core::canvas::content_bounds(compiled, layout, edge_plans, tokens);
    let (canvas_w, canvas_h) = bounds.as_ref().map_or((0.0, 0.0), rdg_render_core::canvas::canvas_size);
    let canvas_w = canvas_w.max(tokens.px(15.0));
    let canvas_h = canvas_h.max(tokens.px(12.5));
    let (origin_x, origin_y) = bounds.as_ref().map_or((0.0, 0.0), |b| (b.min_x.max(0.0), b.min_y.max(0.0)));

    let mut buf = Vec::with_capacity(8192);
    let mut w = Writer::new_with_indent(Cursor::new(&mut buf), b' ', 2);

    // <?xml version="1.0" encoding="UTF-8"?>
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

    let is_dark = theme == "dark";
    let bg_color = background.unwrap_or(if is_dark { "#0f172a" } else { "#f8fafc" });

    // Canvas background
    let mut bg = BytesStart::new("rect");
    bg.push_attribute(("width", "100%"));
    bg.push_attribute(("height", "100%"));
    bg.push_attribute(("fill", bg_color));
    w.write_event(Event::Empty(bg))?;

    // --- Defs: drop shadows and markers -------------------------------------
    w.write_event(Event::Start(BytesStart::new("defs")))?;

    // Drop shadow filter for elevated cards
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
        ("arrow-slate", "#64748b", true),
        ("arrow-dark", "#94a3b8", true),
        ("arrow-amber", "#d97706", false),
        ("arrow-red", "#ef4444", false),
        ("arrow-indigo", "#6366f1", true),
    ];
    for (id, col, filled) in markers {
        let mut marker = BytesStart::new("marker");
        marker.push_attribute(("id", id));
        marker.push_attribute(("viewBox", "0 0 10 10"));
        marker.push_attribute(("refX", "8"));
        marker.push_attribute(("refY", "5"));
        marker.push_attribute(("markerWidth", "6"));
        marker.push_attribute(("markerHeight", "6"));
        marker.push_attribute(("orient", "auto-start-reverse"));
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

    // ER & UML Markers
    let er_color = "#0284c7";
    let uml_color = "#6366f1";
    let uml_dark = if is_dark { "#cbd5e1" } else { "#0f172a" };

    // Crow's foot (many)
    let mut m_er_many = BytesStart::new("marker");
    m_er_many.push_attribute(("id", "marker-er-many"));
    m_er_many.push_attribute(("viewBox", "0 0 12 12"));
    m_er_many.push_attribute(("refX", "10"));
    m_er_many.push_attribute(("refY", "6"));
    m_er_many.push_attribute(("markerWidth", "8"));
    m_er_many.push_attribute(("markerHeight", "8"));
    m_er_many.push_attribute(("orient", "auto-start-reverse"));
    w.write_event(Event::Start(m_er_many))?;
    let mut p_er_many = BytesStart::new("path");
    p_er_many.push_attribute(("d", "M 0 1 L 10 6 L 0 11 M 10 0 L 10 12"));
    p_er_many.push_attribute(("fill", "none"));
    p_er_many.push_attribute(("stroke", er_color));
    p_er_many.push_attribute(("stroke-width", "1.5"));
    w.write_event(Event::Empty(p_er_many))?;
    w.write_event(Event::End(BytesEnd::new("marker")))?;

    // Single line (one)
    let mut m_er_one = BytesStart::new("marker");
    m_er_one.push_attribute(("id", "marker-er-one"));
    m_er_one.push_attribute(("viewBox", "0 0 12 12"));
    m_er_one.push_attribute(("refX", "8"));
    m_er_one.push_attribute(("refY", "6"));
    m_er_one.push_attribute(("markerWidth", "8"));
    m_er_one.push_attribute(("markerHeight", "8"));
    m_er_one.push_attribute(("orient", "auto-start-reverse"));
    w.write_event(Event::Start(m_er_one))?;
    let mut p_er_one = BytesStart::new("path");
    p_er_one.push_attribute(("d", "M 4 1 L 4 11 M 8 1 L 8 11"));
    p_er_one.push_attribute(("fill", "none"));
    p_er_one.push_attribute(("stroke", er_color));
    p_er_one.push_attribute(("stroke-width", "1.5"));
    w.write_event(Event::Empty(p_er_one))?;
    w.write_event(Event::End(BytesEnd::new("marker")))?;

    // UML Inheritance Triangle (hollow closed triangle)
    let mut m_uml_tri = BytesStart::new("marker");
    m_uml_tri.push_attribute(("id", "marker-uml-triangle"));
    m_uml_tri.push_attribute(("viewBox", "0 0 12 12"));
    m_uml_tri.push_attribute(("refX", "10"));
    m_uml_tri.push_attribute(("refY", "6"));
    m_uml_tri.push_attribute(("markerWidth", "8"));
    m_uml_tri.push_attribute(("markerHeight", "8"));
    m_uml_tri.push_attribute(("orient", "auto-start-reverse"));
    w.write_event(Event::Start(m_uml_tri))?;
    let mut p_uml_tri = BytesStart::new("path");
    p_uml_tri.push_attribute(("d", "M 1 1 L 11 6 L 1 11 z"));
    p_uml_tri.push_attribute(("fill", bg_color));
    p_uml_tri.push_attribute(("stroke", uml_color));
    p_uml_tri.push_attribute(("stroke-width", "1.5"));
    w.write_event(Event::Empty(p_uml_tri))?;
    w.write_event(Event::End(BytesEnd::new("marker")))?;

    // UML Composition Diamond (filled)
    let mut m_uml_df = BytesStart::new("marker");
    m_uml_df.push_attribute(("id", "marker-uml-diamond-fill"));
    m_uml_df.push_attribute(("viewBox", "0 0 16 12"));
    m_uml_df.push_attribute(("refX", "2"));
    m_uml_df.push_attribute(("refY", "6"));
    m_uml_df.push_attribute(("markerWidth", "10"));
    m_uml_df.push_attribute(("markerHeight", "8"));
    m_uml_df.push_attribute(("orient", "auto-start-reverse"));
    w.write_event(Event::Start(m_uml_df))?;
    let mut p_uml_df = BytesStart::new("path");
    p_uml_df.push_attribute(("d", "M 1 6 L 8 1 L 15 6 L 8 11 z"));
    p_uml_df.push_attribute(("fill", uml_dark));
    p_uml_df.push_attribute(("stroke", uml_dark));
    p_uml_df.push_attribute(("stroke-width", "1.5"));
    w.write_event(Event::Empty(p_uml_df))?;
    w.write_event(Event::End(BytesEnd::new("marker")))?;

    // UML Aggregation Diamond (hollow)
    let mut m_uml_dh = BytesStart::new("marker");
    m_uml_dh.push_attribute(("id", "marker-uml-diamond-hollow"));
    m_uml_dh.push_attribute(("viewBox", "0 0 16 12"));
    m_uml_dh.push_attribute(("refX", "2"));
    m_uml_dh.push_attribute(("refY", "6"));
    m_uml_dh.push_attribute(("markerWidth", "10"));
    m_uml_dh.push_attribute(("markerHeight", "8"));
    m_uml_dh.push_attribute(("orient", "auto-start-reverse"));
    w.write_event(Event::Start(m_uml_dh))?;
    let mut p_uml_dh = BytesStart::new("path");
    p_uml_dh.push_attribute(("d", "M 1 6 L 8 1 L 15 6 L 8 11 z"));
    p_uml_dh.push_attribute(("fill", bg_color));
    p_uml_dh.push_attribute(("stroke", uml_dark));
    p_uml_dh.push_attribute(("stroke-width", "1.5"));
    w.write_event(Event::Empty(p_uml_dh))?;
    w.write_event(Event::End(BytesEnd::new("marker")))?;

    // Open Chevron Marker
    let mut m_open = BytesStart::new("marker");
    m_open.push_attribute(("id", "marker-open-slate"));
    m_open.push_attribute(("viewBox", "0 0 10 10"));
    m_open.push_attribute(("refX", "7"));
    m_open.push_attribute(("refY", "5"));
    m_open.push_attribute(("markerWidth", "6"));
    m_open.push_attribute(("markerHeight", "6"));
    m_open.push_attribute(("orient", "auto-start-reverse"));
    w.write_event(Event::Start(m_open))?;
    let mut p_open = BytesStart::new("path");
    p_open.push_attribute(("d", "M 1 2 L 7 5 L 1 8"));
    p_open.push_attribute(("fill", "none"));
    p_open.push_attribute(("stroke", if is_dark { "#94a3b8" } else { "#64748b" }));
    p_open.push_attribute(("stroke-width", "1.5"));
    w.write_event(Event::Empty(p_open))?;
    w.write_event(Event::End(BytesEnd::new("marker")))?;

    // Circle / Dot Marker
    let mut m_circle = BytesStart::new("marker");
    m_circle.push_attribute(("id", "marker-circle-fill"));
    m_circle.push_attribute(("viewBox", "0 0 10 10"));
    m_circle.push_attribute(("refX", "5"));
    m_circle.push_attribute(("refY", "5"));
    m_circle.push_attribute(("markerWidth", "6"));
    m_circle.push_attribute(("markerHeight", "6"));
    m_circle.push_attribute(("orient", "auto-start-reverse"));
    w.write_event(Event::Start(m_circle))?;
    let mut c_elem = BytesStart::new("circle");
    c_elem.push_attribute(("cx", "5"));
    c_elem.push_attribute(("cy", "5"));
    c_elem.push_attribute(("r", "3.5"));
    c_elem.push_attribute(("fill", if is_dark { "#94a3b8" } else { "#64748b" }));
    w.write_event(Event::Empty(c_elem))?;
    w.write_event(Event::End(BytesEnd::new("marker")))?;

    w.write_event(Event::End(BytesEnd::new("defs")))?;

    // --- Optional Diagram Title Header --------------------------------------
    if let Some(title) = &compiled.title {
        let title_color = if is_dark { "#f1f5f9" } else { "#0f172a" };
        let sub_color = if is_dark { "#94a3b8" } else { "#64748b" };

        let mut t_elem = BytesStart::new("text");
        t_elem.push_attribute(("x", format!("{origin_x:.1}").as_str()));
        t_elem.push_attribute(("y", format!("{:.1}", origin_y + 15.0).as_str()));
        t_elem.push_attribute(("font-family", "Inter, Helvetica, sans-serif"));
        t_elem.push_attribute(("font-size", "15"));
        t_elem.push_attribute(("font-weight", "bold"));
        t_elem.push_attribute(("fill", title_color));
        w.write_event(Event::Start(t_elem))?;
        w.write_event(Event::Text(BytesText::new(title)))?;
        w.write_event(Event::End(BytesEnd::new("text")))?;

        if let Some(desc) = &compiled.description {
            let mut d_elem = BytesStart::new("text");
            d_elem.push_attribute(("x", format!("{origin_x:.1}").as_str()));
            d_elem.push_attribute(("y", format!("{:.1}", origin_y + 31.0).as_str()));
            d_elem.push_attribute(("font-family", "Inter, Helvetica, sans-serif"));
            d_elem.push_attribute(("font-size", "11"));
            d_elem.push_attribute(("fill", sub_color));
            w.write_event(Event::Start(d_elem))?;
            w.write_event(Event::Text(BytesText::new(desc)))?;
            w.write_event(Event::End(BytesEnd::new("text")))?;
        }
    }

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

        let pad_h = tokens.group_pad_for(max_x - min_x, max_y - min_y);
        let pad_top = tokens.group_pad_top_for(max_x - min_x, max_y - min_y);
        let pad_bot = pad_h;
        let min_edge = tokens.px(1.25);

        let gx = (min_x - pad_h).max(min_edge);
        let gy = (min_y - pad_top).max(min_edge);
        let gw = (max_x - min_x) + (pad_h * 2.0);
        let gh = (max_y - min_y) + pad_top + pad_bot;
        let color = group.color.as_deref().unwrap_or("#64748b");

        let mut g = BytesStart::new("g");
        g.push_attribute(("id", group.id.as_str()));
        g.push_attribute(("class", "diagram-group"));
        w.write_event(Event::Start(g))?;

        let mut rect = BytesStart::new("rect");
        rect.push_attribute(("x", gx.round().to_string().as_str()));
        rect.push_attribute(("y", gy.round().to_string().as_str()));
        rect.push_attribute(("width", gw.round().to_string().as_str()));
        rect.push_attribute(("height", gh.round().to_string().as_str()));
        rect.push_attribute(("rx", "8"));
        rect.push_attribute(("fill", color));
        rect.push_attribute(("fill-opacity", if is_dark { "0.12" } else { "0.08" }));
        rect.push_attribute(("stroke", color));
        rect.push_attribute(("stroke-width", "1.5"));
        rect.push_attribute(("stroke-dasharray", "6 6"));
        w.write_event(Event::Empty(rect))?;

        let group_icon = group
            .resolved_icon()
            .and_then(|k| rdg_icons::render_icon_svg(&k, gx + 14.0, gy + 8.0, 16.0));
        let text_x = if group_icon.is_some() {
            gx + 36.0
        } else {
            gx + 14.0
        };

        if let Some(icon_svg) = group_icon {
            w.write_event(Event::Text(BytesText::from_escaped(icon_svg)))?;
        }

        let mut text = BytesStart::new("text");
        text.push_attribute(("x", text_x.round().to_string().as_str()));
        text.push_attribute(("y", (gy + 20.0).round().to_string().as_str()));
        text.push_attribute(("font-family", "Inter, Helvetica, sans-serif"));
        text.push_attribute(("font-size", "11"));
        text.push_attribute(("font-weight", "bold"));
        text.push_attribute(("fill", color));
        w.write_event(Event::Start(text))?;
        w.write_event(Event::Text(BytesText::new(&group.label)))?;
        w.write_event(Event::End(BytesEnd::new("text")))?;

        w.write_event(Event::End(BytesEnd::new("g")))?;
    }

    // --- Draw edges (behind nodes) with intelligent orthogonal rounded paths -
    let default_edge = if is_dark { "#94a3b8" } else { "#64748b" };

    struct SvgEdgeLabel {
        lx: f64,
        ly: f64,
        text: String,
    }
    let mut pending_labels: Vec<SvgEdgeLabel> = Vec::new();
    let mut pending_badges: Vec<(f64, f64, u32)> = Vec::new();

    // Pass 1: Render all edge paths
    for edge_idx in compiled.graph.edge_indices() {
        let (src_idx, dst_idx) = compiled.graph.edge_endpoints(edge_idx).unwrap();
        let edge_data = &compiled.graph[edge_idx];

        let (s_idx, d_idx) = if edge_data.reversed {
            (dst_idx, src_idx)
        } else {
            (src_idx, dst_idx)
        };

        let (Some(src_nl), Some(dst_nl)) =
            (layout.positions.get(&s_idx), layout.positions.get(&d_idx))
        else {
            continue;
        };

        let plan = edge_plans.get(&edge_idx);
        let src_side = plan.map_or(Side::Bottom, |p| p.src_side);
        let dst_side = plan.map_or(Side::Top, |p| p.dst_side);
        let exit_port = plan.map_or(0.5, |p| p.exit_port);
        let entry_port = plan.map_or(0.5, |p| p.entry_port);
        let channel_y = plan.map_or((src_nl.y + dst_nl.y) / 2.0, |p| p.channel_y);

        // Start/end/choice markers are tiny fixed-size shapes whose label renders
        // below the shape itself (`write_label_below_marker`, called further down for
        // these same types) rather than inside it — an edge leaving from the exact
        // bottom of that small box would be drawn straight through the label text
        // sitting right below it. Both the exit point and the step badge need to
        // clear the label, same fix as the draw.io backend's `exitDy`.
        let src_is_marker = matches!(
            compiled.graph[s_idx].node_type.to_ascii_lowercase().as_str(),
            "start" | "start_state" | "initial" | "initial_state" |
            "end" | "end_state" | "final" | "final_state" |
            "choice" | "branch"
        );
        // Expressed as a multiple of the marker's *own* height, not a flat pixel
        // value, so it stays correct if `DesignTokens::{start,end,choice}_marker_*`
        // ever changes the marker's actual size — the same
        // `marker_label_clearance_ratio` the draw.io backend applies via its `exitY`
        // extrapolation (see that backend's `style.rs`), just computed directly here
        // since native SVG draws literal coordinates rather than percentage anchors.
        let marker_clearance = if src_is_marker && src_side == Side::Bottom {
            src_nl.height * (tokens.marker_label_clearance_ratio - 1.0)
        } else {
            0.0
        };

        let (x1, y1_raw) = port_point(src_nl, src_side, exit_port);
        let y1 = y1_raw + marker_clearance;
        let (x2, y2) = port_point(dst_nl, dst_side, entry_port);

        if let Some(step) = edge_data.step {
            let badge_distance =
                if src_is_marker { src_nl.height * 1.4 } else { tokens.px(1.75) };
            let (bx, by) = rdg_render_core::routing::badge_point_near_exit(
                src_nl, src_side, exit_port, badge_distance,
            );
            pending_badges.push((bx, by, step));
        }

        let is_bi = matches!(
            edge_data.edge_style.as_deref(),
            Some("bi") | Some("bidirectional")
        );
        let mut marker_start: Option<&str> = None;
        let mut marker_end: Option<&str> = Some(if is_dark { "arrow-dark" } else { "arrow-slate" });

        match edge_data.edge_style.as_deref() {
            Some("async") => marker_end = Some("arrow-amber"),
            Some("error") | Some("fallback") => marker_end = Some("arrow-red"),
            Some("data") | Some("stream") => marker_end = Some("arrow-indigo"),
            Some("one_to_many") => {
                marker_start = Some("marker-er-one");
                marker_end = Some("marker-er-many");
            }
            Some("many_to_many") => {
                marker_start = Some("marker-er-many");
                marker_end = Some("marker-er-many");
            }
            Some("one_to_one") => {
                marker_start = Some("marker-er-one");
                marker_end = Some("marker-er-one");
            }
            Some("zero_to_many") => {
                marker_start = Some("marker-er-one");
                marker_end = Some("marker-er-many");
            }
            Some("inheritance") => marker_end = Some("marker-uml-triangle"),
            Some("realization") => marker_end = Some("marker-uml-triangle"),
            Some("composition") => {
                marker_start = Some("marker-uml-diamond-fill");
                marker_end = None;
            }
            Some("aggregation") => {
                marker_start = Some("marker-uml-diamond-hollow");
                marker_end = None;
            }
            Some("dependency") => {
                marker_end = Some(if is_dark { "arrow-dark" } else { "arrow-slate" })
            }
            _ => {
                if is_bi {
                    marker_start = Some(if is_dark { "arrow-dark" } else { "arrow-slate" });
                }
            }
        };

        // Stroke/width/dash come from the shared `rdg_render_core::style` table (also used by
        // the draw.io backend); marker selection above stays SVG-specific.
        let colors = edge_style_colors(edge_data.edge_style.as_deref(), theme, default_edge);
        let (base_stroke, base_w, base_dash) = (colors.stroke, colors.width, colors.dash);

        // Custom formatting overrides
        let stroke = edge_data.color.as_deref().unwrap_or(base_stroke);
        let stroke_w_buf = edge_data
            .width
            .map_or_else(|| base_w.to_string(), |w| format!("{w:.1}"));
        let dash = if let Some(ls) = &edge_data.line_style {
            match ls.to_ascii_lowercase().as_str() {
                "dashed" => Some("8 4"),
                "dotted" => Some("3 3"),
                "solid" => None,
                _ => base_dash,
            }
        } else {
            base_dash
        };

        if let Some(h) = &edge_data.head {
            marker_end = match h.to_ascii_lowercase().as_str() {
                "none" => None,
                "open" => Some("marker-open-slate"),
                "diamond" => Some("marker-uml-diamond-fill"),
                "circle" | "oval" => Some("marker-circle-fill"),
                "ermany" => Some("marker-er-many"),
                "erone" => Some("marker-er-one"),
                _ => Some(if is_dark { "arrow-dark" } else { "arrow-slate" }),
            };
        }
        if let Some(t) = &edge_data.tail {
            marker_start = match t.to_ascii_lowercase().as_str() {
                "none" => None,
                "open" => Some("marker-open-slate"),
                "diamond" => Some("marker-uml-diamond-fill"),
                "circle" | "oval" => Some("marker-circle-fill"),
                "ermany" => Some("marker-er-many"),
                "erone" => Some("marker-er-one"),
                _ => Some(if is_dark { "arrow-dark" } else { "arrow-slate" }),
            };
        }

        let corridor_x = plan.map_or(0.0, |p| p.corridor_x);
        let default_wps = compute_edge_waypoints(
            (x1, y1),
            src_side,
            (x2, y2),
            dst_side,
            channel_y,
            corridor_x,
            tokens,
        );
        let waypoints = plan.map_or(default_wps.as_slice(), |p| p.waypoints.as_slice());
        let (path_d, lx, ly) = build_orthogonal_svg_path((x1, y1), (x2, y2), waypoints);

        let mut path = BytesStart::new("path");
        path.push_attribute(("d", path_d.as_str()));
        path.push_attribute(("fill", "none"));
        path.push_attribute(("stroke", stroke));
        path.push_attribute(("stroke-width", stroke_w_buf.as_str()));
        if let Some(d) = dash {
            path.push_attribute(("stroke-dasharray", d));
        }
        if let Some(ms) = marker_start {
            path.push_attribute(("marker-start", format!("url(#{ms})").as_str()));
        }
        if let Some(me) = marker_end {
            path.push_attribute(("marker-end", format!("url(#{me})").as_str()));
        }
        w.write_event(Event::Empty(path))?;

        if let Some(label) = &edge_data.label {
            let label_trimmed = label.trim();
            if !label_trimmed.is_empty() {
                pending_labels.push(SvgEdgeLabel {
                    lx,
                    ly,
                    text: label_trimmed.to_string(),
                });
            }
        }
    }

    // Two different edges' paths can legitimately pass close to each other (common in
    // any real, moderately dense diagram), which put their labels at the same spot
    // often enough in practice to be worth fixing rather than living with — confirmed
    // directly while visually reviewing rendered sample diagrams (e.g. a "SQL" label
    // and an "invoke" label landing on top of each other, reading as "SQLoke"). Pass 1
    // already computed every label's un-adjusted anchor in a fixed, deterministic
    // order (edge declaration order); decluttering them as a batch here — rather than
    // each label only knowing about its own edge — is the only way to detect that kind
    // of cross-edge collision at all.
    // Edge labels render a size step below the base body font.
    let edge_label_font = tokens.font_size * 0.83;
    let pill_width = |text: &str| {
        (text.chars().count() as f64 * tokens.char_width(edge_label_font) + tokens.px(1.0)).max(tokens.px(2.5))
    };
    let pill_height = tokens.line_height(edge_label_font);

    let declutter_items: Vec<(f64, f64, f64, f64)> = pending_labels
        .iter()
        .map(|el| (el.lx, el.ly, pill_width(&el.text), pill_height))
        .collect();
    // Labels must clear every node box too, not just each other — otherwise
    // decluttering one label away from another can just as easily land it on top of
    // an unrelated node's card instead (see `declutter_label_positions_avoiding`'s
    // own doc comment for the real diagram that surfaced this). Group title banners
    // get the same treatment: a label landing on a group's title text is just as
    // unreadable as one landing on a node.
    let mut node_obstacles: Vec<(f64, f64, f64, f64)> =
        layout.positions.values().map(|nl| (nl.x, nl.y, nl.width, nl.height)).collect();
    node_obstacles.extend(
        rdg_render_core::routing::compute_group_title_zones(compiled, layout, tokens)
            .iter()
            .map(|tz| (tz.min_x, tz.min_y, tz.max_x - tz.min_x, tz.max_y - tz.min_y)),
    );
    let label_dys = rdg_render_core::routing::declutter_label_positions_avoiding(
        &declutter_items,
        &node_obstacles,
        tokens,
    );

    // Pass 2: Render all edge labels on top of all paths to guarantee zero line collisions
    for (el, dy) in pending_labels.into_iter().zip(label_dys) {
        let pill_w = pill_width(&el.text);
        let pill_h = pill_height;
        let ly = el.ly + dy;
        let pill_x = el.lx - pill_w / 2.0;
        let pill_y = ly - pill_h / 2.0;

        let mut pill = BytesStart::new("rect");
        pill.push_attribute(("x", format!("{pill_x:.1}").as_str()));
        pill.push_attribute(("y", format!("{pill_y:.1}").as_str()));
        pill.push_attribute(("width", format!("{pill_w:.1}").as_str()));
        pill.push_attribute(("height", format!("{pill_h:.1}").as_str()));
        // Clean text cutout background with NO border box!
        pill.push_attribute(("fill", if is_dark { "#0f172a" } else { "#f8fafc" }));
        w.write_event(Event::Empty(pill))?;

        let mut text = BytesStart::new("text");
        text.push_attribute(("x", format!("{:.1}", el.lx).as_str()));
        text.push_attribute(("y", format!("{:.1}", ly + 3.5).as_str()));
        text.push_attribute(("text-anchor", "middle"));
        text.push_attribute(("font-family", "Inter, Helvetica, sans-serif"));
        text.push_attribute(("font-size", "10"));
        text.push_attribute(("font-weight", "500"));
        text.push_attribute(("fill", if is_dark { "#94a3b8" } else { "#475569" }));
        w.write_event(Event::Start(text))?;
        w.write_event(Event::Text(BytesText::new(&el.text)))?;
        w.write_event(Event::End(BytesEnd::new("text")))?;
    }

    // Pass 3: Flow-numbering badges, on top of both paths and labels.
    let (badge_fill, badge_text) = if is_dark {
        ("#f1f5f9", "#0f172a")
    } else {
        ("#0f172a", "#ffffff")
    };
    for (bx, by, step) in pending_badges {
        let mut circle = BytesStart::new("circle");
        circle.push_attribute(("cx", format!("{bx:.1}").as_str()));
        circle.push_attribute(("cy", format!("{by:.1}").as_str()));
        circle.push_attribute(("r", "9"));
        circle.push_attribute(("fill", badge_fill));
        w.write_event(Event::Empty(circle))?;

        let mut text = BytesStart::new("text");
        text.push_attribute(("x", format!("{bx:.1}").as_str()));
        text.push_attribute(("y", format!("{:.1}", by + 3.2).as_str()));
        text.push_attribute(("text-anchor", "middle"));
        text.push_attribute(("font-family", "Inter, Helvetica, sans-serif"));
        text.push_attribute(("font-size", "10"));
        text.push_attribute(("font-weight", "700"));
        text.push_attribute(("fill", badge_text));
        w.write_event(Event::Start(text))?;
        w.write_event(Event::Text(BytesText::new(&step.to_string())))?;
        w.write_event(Event::End(BytesEnd::new("text")))?;
    }

    // --- Draw nodes with white-card elevation & semantic accents ------------
    let card_fill = if is_dark { "#1e293b" } else { "#ffffff" };
    let title_color = if is_dark { "#f1f5f9" } else { "#0f172a" };
    let sub_color = if is_dark { "#94a3b8" } else { "#64748b" };

    for node_idx in compiled.graph.node_indices() {
        let node_data = &compiled.graph[node_idx];
        let Some(nl) = layout.positions.get(&node_idx) else {
            continue;
        };
        let stroke_color = node_data
            .color
            .as_deref()
            .unwrap_or_else(|| node_accent_color(&node_data.node_type, theme));

        let lower_type = node_data.node_type.to_ascii_lowercase();
        let is_start = matches!(
            lower_type.as_str(),
            "start" | "start_state" | "initial" | "initial_state"
        );
        let is_end = matches!(
            lower_type.as_str(),
            "end" | "end_state" | "final" | "final_state"
        );
        let is_choice = matches!(lower_type.as_str(), "choice" | "branch");
        let is_decision = matches!(lower_type.as_str(), "decision" | "condition");
        let is_class = matches!(
            lower_type.as_str(),
            "class" | "interface" | "abstract_class" | "struct"
        );
        let is_table_cylinder = !is_class
            && (matches!(
                lower_type.as_str(),
                "table" | "entity" | "record" | "database" | "db" | "storage"
            ) || !node_data.fields.is_empty());

        if is_start {
            let cx = nl.x + nl.width / 2.0;
            let cy = nl.y + nl.height / 2.0;
            let mut circle = BytesStart::new("circle");
            circle.push_attribute(("cx", format!("{cx:.1}").as_str()));
            circle.push_attribute(("cy", format!("{cy:.1}").as_str()));
            circle.push_attribute(("r", "12"));
            circle.push_attribute(("fill", stroke_color));
            w.write_event(Event::Empty(circle))?;
            // `nl` is a fixed 28x28 marker (see `estimate_node_size_inner`), far
            // smaller than most labels — centering the label on top of it the way the
            // generic card path does would bury the text inside the circle. Drawing it
            // below instead (same fix applied to the draw.io backend's
            // `verticalLabelPosition=bottom`) keeps it legible without needing the box
            // itself to grow.
            write_label_below_marker(&mut w, cx, nl.y + nl.height, &node_data.label, title_color, tokens)?;
            continue;
        } else if is_end {
            let cx = nl.x + nl.width / 2.0;
            let cy = nl.y + nl.height / 2.0;
            let mut outer = BytesStart::new("circle");
            outer.push_attribute(("cx", format!("{cx:.1}").as_str()));
            outer.push_attribute(("cy", format!("{cy:.1}").as_str()));
            outer.push_attribute(("r", "14"));
            outer.push_attribute(("fill", "none"));
            outer.push_attribute(("stroke", stroke_color));
            outer.push_attribute(("stroke-width", "2.0"));
            w.write_event(Event::Empty(outer))?;

            let mut inner = BytesStart::new("circle");
            inner.push_attribute(("cx", format!("{cx:.1}").as_str()));
            inner.push_attribute(("cy", format!("{cy:.1}").as_str()));
            inner.push_attribute(("r", "8"));
            inner.push_attribute(("fill", stroke_color));
            w.write_event(Event::Empty(inner))?;
            write_label_below_marker(&mut w, cx, nl.y + nl.height, &node_data.label, title_color, tokens)?;
            continue;
        } else if is_choice || is_decision {
            let poly_d = format!(
                "M {cx:.1} {top:.1} L {right:.1} {cy:.1} L {cx:.1} {bot:.1} L {left:.1} {cy:.1} Z",
                cx = nl.x + nl.width / 2.0,
                top = nl.y,
                right = nl.x + nl.width,
                cy = nl.y + nl.height / 2.0,
                bot = nl.y + nl.height,
                left = nl.x,
            );
            let mut poly = BytesStart::new("path");
            poly.push_attribute(("d", poly_d.as_str()));
            poly.push_attribute(("fill", card_fill));
            poly.push_attribute(("stroke", stroke_color));
            poly.push_attribute(("stroke-width", "2.0"));
            poly.push_attribute(("filter", "url(#card-shadow)"));
            w.write_event(Event::Empty(poly))?;

            // Render authentic icon badge pinned at left vertex if present
            if let Some(icon_key) = node_data.icon.as_deref() {
                if let Some(badge_markup) = rdg_icons::render_icon_badge_svg(
                    icon_key,
                    nl.x,
                    nl.y + nl.height / 2.0,
                    is_dark,
                ) {
                    w.write_event(Event::Text(BytesText::from_escaped(badge_markup)))?;
                }
            }
        } else if is_class {
            let mut rect = BytesStart::new("rect");
            rect.push_attribute(("x", format!("{:.1}", nl.x).as_str()));
            rect.push_attribute(("y", format!("{:.1}", nl.y).as_str()));
            rect.push_attribute(("width", format!("{:.1}", nl.width).as_str()));
            rect.push_attribute(("height", format!("{:.1}", nl.height).as_str()));
            rect.push_attribute(("rx", "6"));
            rect.push_attribute(("ry", "6"));
            rect.push_attribute(("fill", card_fill));
            rect.push_attribute(("stroke", stroke_color));
            rect.push_attribute(("stroke-width", "1.5"));
            rect.push_attribute(("filter", "url(#card-shadow)"));
            w.write_event(Event::Empty(rect))?;

            let cx = nl.x + nl.width / 2.0;
            let mut cur_y = nl.y + 14.0;

            let stereotype = if lower_type == "interface" {
                Some("&lt;&lt;interface&gt;&gt;")
            } else if lower_type == "abstract_class" {
                Some("&lt;&lt;abstract&gt;&gt;")
            } else {
                None
            };

            if let Some(st) = stereotype {
                let mut st_text = BytesStart::new("text");
                st_text.push_attribute(("x", format!("{cx:.1}").as_str()));
                st_text.push_attribute(("y", format!("{cur_y:.1}").as_str()));
                st_text.push_attribute(("text-anchor", "middle"));
                st_text.push_attribute(("font-family", "Inter, Helvetica, sans-serif"));
                st_text.push_attribute(("font-size", "10"));
                st_text.push_attribute(("font-style", "italic"));
                st_text.push_attribute(("fill", sub_color));
                w.write_event(Event::Start(st_text))?;
                w.write_event(Event::Text(BytesText::from_escaped(st)))?;
                w.write_event(Event::End(BytesEnd::new("text")))?;
                cur_y += 14.0;
            }

            let mut title_text = BytesStart::new("text");
            title_text.push_attribute(("x", format!("{cx:.1}").as_str()));
            title_text.push_attribute(("y", format!("{cur_y:.1}").as_str()));
            title_text.push_attribute(("text-anchor", "middle"));
            title_text.push_attribute(("font-family", "Inter, Helvetica, sans-serif"));
            title_text.push_attribute(("font-size", "12"));
            title_text.push_attribute(("font-weight", "bold"));
            if lower_type == "abstract_class" {
                title_text.push_attribute(("font-style", "italic"));
            }
            title_text.push_attribute(("fill", title_color));
            w.write_event(Event::Start(title_text))?;
            w.write_event(Event::Text(BytesText::new(&node_data.label)))?;
            w.write_event(Event::End(BytesEnd::new("text")))?;
            cur_y += 8.0;

            let mut h_line = BytesStart::new("line");
            h_line.push_attribute(("x1", (nl.x).round().to_string().as_str()));
            h_line.push_attribute(("y1", cur_y.round().to_string().as_str()));
            h_line.push_attribute(("x2", (nl.x + nl.width).round().to_string().as_str()));
            h_line.push_attribute(("y2", cur_y.round().to_string().as_str()));
            h_line.push_attribute(("stroke", if is_dark { "#475569" } else { "#e2e8f0" }));
            h_line.push_attribute(("stroke-width", "1.0"));
            w.write_event(Event::Empty(h_line))?;

            for (f_idx, field) in node_data.fields.iter().enumerate() {
                let field_y = cur_y + 16.0 + (f_idx as f64 * 18.0);
                let field_clean = rdg_layout::strip_markdown_tokens(field);
                let (left_part, right_part) = if let Some(idx) = field_clean.find(':') {
                    (&field_clean[..idx], &field_clean[idx + 1..])
                } else {
                    (field_clean.as_str(), "")
                };

                let mut left_text = BytesStart::new("text");
                left_text.push_attribute(("x", (nl.x + 12.0).round().to_string().as_str()));
                left_text.push_attribute(("y", field_y.round().to_string().as_str()));
                left_text.push_attribute(("font-family", "JetBrains Mono, monospace"));
                left_text.push_attribute(("font-size", "10"));
                left_text.push_attribute(("fill", title_color));
                w.write_event(Event::Start(left_text))?;
                w.write_event(Event::Text(BytesText::new(left_part.trim())))?;
                w.write_event(Event::End(BytesEnd::new("text")))?;

                if !right_part.trim().is_empty() {
                    let mut right_text = BytesStart::new("text");
                    right_text.push_attribute((
                        "x",
                        (nl.x + nl.width - 12.0).round().to_string().as_str(),
                    ));
                    right_text.push_attribute(("y", field_y.round().to_string().as_str()));
                    right_text.push_attribute(("text-anchor", "end"));
                    right_text.push_attribute(("font-family", "JetBrains Mono, monospace"));
                    right_text.push_attribute(("font-size", "10"));
                    right_text.push_attribute(("fill", sub_color));
                    w.write_event(Event::Start(right_text))?;
                    w.write_event(Event::Text(BytesText::new(right_part.trim())))?;
                    w.write_event(Event::End(BytesEnd::new("text")))?;
                }
            }
            continue;
        } else if is_table_cylinder {
            let rh = (nl.height * 0.08).clamp(6.0, 9.0);
            let rx = nl.width / 2.0;
            let cx = nl.x + rx;

            let body_d = format!(
                "M {x:.1} {y_top:.1} \
                 L {x:.1} {y_bot:.1} \
                 A {rx:.1} {rh:.1} 0 0 0 {x_right:.1} {y_bot:.1} \
                 L {x_right:.1} {y_top:.1} Z",
                x = nl.x,
                y_top = nl.y + rh,
                y_bot = nl.y + nl.height - rh,
                x_right = nl.x + nl.width,
            );
            let mut body = BytesStart::new("path");
            body.push_attribute(("d", body_d.as_str()));
            body.push_attribute(("fill", card_fill));
            body.push_attribute(("stroke", stroke_color));
            body.push_attribute(("stroke-width", "1.5"));
            body.push_attribute(("filter", "url(#card-shadow)"));
            w.write_event(Event::Empty(body))?;

            let mut top_cap = BytesStart::new("ellipse");
            top_cap.push_attribute(("cx", format!("{cx:.1}").as_str()));
            top_cap.push_attribute(("cy", format!("{:.1}", nl.y + rh).as_str()));
            top_cap.push_attribute(("rx", format!("{rx:.1}").as_str()));
            top_cap.push_attribute(("ry", format!("{rh:.1}").as_str()));
            let cap_fill = if is_dark { "#334155" } else { "#f1f5f9" };
            top_cap.push_attribute(("fill", cap_fill));
            top_cap.push_attribute(("stroke", stroke_color));
            top_cap.push_attribute(("stroke-width", "1.5"));
            w.write_event(Event::Empty(top_cap))?;

            // Render authentic database/engine icon badge pinned at top-left boundary!
            if let Some(icon_key) = node_data.icon.as_deref() {
                if let Some(badge_markup) =
                    rdg_icons::render_icon_badge_svg(icon_key, nl.x, nl.y, is_dark)
                {
                    w.write_event(Event::Text(BytesText::from_escaped(badge_markup)))?;
                }
            }

            // If table has fields, render structured table rows inside the cylinder!
            if !node_data.fields.is_empty() {
                // Table header label
                let header_y = nl.y + rh * 2.0 + 8.0;
                let mut text = BytesStart::new("text");
                text.push_attribute(("x", format!("{cx:.1}").as_str()));
                text.push_attribute(("y", format!("{header_y:.1}").as_str()));
                text.push_attribute(("text-anchor", "middle"));
                text.push_attribute(("font-family", "Inter, Helvetica, sans-serif"));
                text.push_attribute(("font-size", "11"));
                text.push_attribute(("font-weight", "bold"));
                text.push_attribute(("fill", title_color));
                w.write_event(Event::Start(text))?;
                w.write_event(Event::Text(BytesText::new(&node_data.label)))?;
                w.write_event(Event::End(BytesEnd::new("text")))?;

                // Divider line below header
                let mut h_line = BytesStart::new("line");
                h_line.push_attribute(("x1", (nl.x + 8.0).round().to_string().as_str()));
                h_line.push_attribute(("y1", (header_y + 6.0).round().to_string().as_str()));
                h_line.push_attribute(("x2", (nl.x + nl.width - 8.0).round().to_string().as_str()));
                h_line.push_attribute(("y2", (header_y + 6.0).round().to_string().as_str()));
                h_line.push_attribute(("stroke", if is_dark { "#475569" } else { "#e2e8f0" }));
                h_line.push_attribute(("stroke-width", "1.0"));
                w.write_event(Event::Empty(h_line))?;

                // Fields rows
                for (f_idx, field) in node_data.fields.iter().enumerate() {
                    let field_y = header_y + 18.0 + (f_idx as f64 * 20.0);
                    let field_clean = rdg_layout::strip_markdown_tokens(field);
                    let (left_part, right_part) = if let Some(idx) = field_clean.find(':') {
                        (&field_clean[..idx], &field_clean[idx + 1..])
                    } else {
                        (field_clean.as_str(), "")
                    };
                    let is_pk = field_clean.to_ascii_uppercase().contains("[PK]")
                        || field_clean.to_ascii_uppercase().contains("PRIMARY KEY");

                    let mut left_text = BytesStart::new("text");
                    left_text.push_attribute(("x", (nl.x + 12.0).round().to_string().as_str()));
                    left_text.push_attribute(("y", field_y.round().to_string().as_str()));
                    left_text.push_attribute(("font-family", "JetBrains Mono, monospace"));
                    left_text.push_attribute(("font-size", "10"));
                    if is_pk {
                        left_text.push_attribute(("font-weight", "bold"));
                        left_text.push_attribute(("fill", "#d97706"));
                    } else {
                        left_text.push_attribute(("fill", title_color));
                    }
                    w.write_event(Event::Start(left_text))?;
                    w.write_event(Event::Text(BytesText::new(left_part.trim())))?;
                    w.write_event(Event::End(BytesEnd::new("text")))?;

                    if !right_part.trim().is_empty() {
                        let mut right_text = BytesStart::new("text");
                        right_text.push_attribute((
                            "x",
                            (nl.x + nl.width - 12.0).round().to_string().as_str(),
                        ));
                        right_text.push_attribute(("y", field_y.round().to_string().as_str()));
                        right_text.push_attribute(("text-anchor", "end"));
                        right_text.push_attribute(("font-family", "JetBrains Mono, monospace"));
                        right_text.push_attribute(("font-size", "9"));
                        right_text.push_attribute(("fill", sub_color));
                        w.write_event(Event::Start(right_text))?;
                        w.write_event(Event::Text(BytesText::new(right_part.trim())))?;
                        w.write_event(Event::End(BytesEnd::new("text")))?;
                    }
                }
                continue;
            }
        } else {
            let mut rect = BytesStart::new("rect");
            rect.push_attribute(("x", format!("{:.1}", nl.x).as_str()));
            rect.push_attribute(("y", format!("{:.1}", nl.y).as_str()));
            rect.push_attribute(("width", format!("{:.1}", nl.width).as_str()));
            rect.push_attribute(("height", format!("{:.1}", nl.height).as_str()));
            rect.push_attribute(("rx", "8"));
            rect.push_attribute(("ry", "8"));
            rect.push_attribute(("fill", card_fill));
            rect.push_attribute(("stroke", stroke_color));
            rect.push_attribute(("stroke-width", "1.5"));
            rect.push_attribute(("filter", "url(#card-shadow)"));
            w.write_event(Event::Empty(rect))?;

            // Render authentic language / database / user icon badge pinned at top-left boundary!
            if let Some(icon_key) = node_data.icon.as_deref() {
                if let Some(badge_markup) =
                    rdg_icons::render_icon_badge_svg(icon_key, nl.x, nl.y, is_dark)
                {
                    w.write_event(Event::Text(BytesText::from_escaped(badge_markup)))?;
                }
            }
        }

        // Multi-line text wrapping with centered tspans and typography support
        let max_line_chars = if is_decision { 16 } else { 20 };
        let lines = wrap_and_classify_label(&node_data.label, max_line_chars);
        let cx = nl.x + nl.width / 2.0;

        let total_text_h = match lines.len() {
            0 => 0.0,
            1 => 14.0,
            n => 14.0 + (n - 1) as f64 * 14.0,
        };

        // Center text inside cylindrical body below the top ellipse cap for databases/tables
        let start_y = if is_table_cylinder {
            let rh = (nl.height * 0.08).clamp(6.0, 9.0);
            let body_top = nl.y + 2.0 * rh + 2.0;
            let body_bot = nl.y + nl.height - rh - 2.0;
            let body_h = (body_bot - body_top).max(total_text_h);
            body_top + (body_h - total_text_h) / 2.0 + 11.0
        } else {
            nl.y + (nl.height - total_text_h) / 2.0 + 11.0
        };

        let mut text = BytesStart::new("text");
        text.push_attribute(("x", format!("{cx:.1}").as_str()));
        text.push_attribute(("y", format!("{start_y:.1}").as_str()));
        text.push_attribute(("text-anchor", "middle"));
        text.push_attribute(("font-family", "Inter, Helvetica, sans-serif"));
        w.write_event(Event::Start(text))?;

        for (line_idx, pl) in lines.iter().enumerate() {
            let spans = parse_inline_spans(&pl.text);

            for (span_idx, span) in spans.into_iter().enumerate() {
                let mut tspan = BytesStart::new("tspan");
                if span_idx == 0 {
                    tspan.push_attribute(("x", format!("{cx:.1}").as_str()));
                    if line_idx > 0 {
                        tspan.push_attribute(("dy", "14"));
                    }
                }

                let span_text = if span.style.is_math {
                    latex_to_unicode(&span.text)
                } else if span.style.is_subscript {
                    to_subscript(&span.text)
                } else if span.style.is_superscript {
                    to_superscript(&span.text)
                } else {
                    span.text
                };

                if span.style.is_code {
                    tspan.push_attribute((
                        "font-family",
                        "JetBrains Mono, Menlo, Courier New, monospace",
                    ));
                    tspan.push_attribute(("font-size", "11"));
                    if pl.is_subtitle {
                        tspan.push_attribute(("font-weight", "normal"));
                        tspan.push_attribute(("fill", sub_color));
                    } else {
                        tspan.push_attribute(("font-weight", "bold"));
                        tspan.push_attribute(("fill", title_color));
                    }
                } else if span.style.is_math {
                    tspan.push_attribute((
                        "font-family",
                        "Cambria Math, Latin Modern Math, Times New Roman, serif",
                    ));
                    tspan.push_attribute(("font-style", "italic"));
                    tspan.push_attribute(("font-size", if pl.is_subtitle { "10" } else { "12" }));
                    tspan.push_attribute((
                        "fill",
                        if pl.is_subtitle {
                            sub_color
                        } else {
                            title_color
                        },
                    ));
                } else {
                    tspan.push_attribute(("font-size", if pl.is_subtitle { "10" } else { "12" }));
                    tspan.push_attribute((
                        "fill",
                        if pl.is_subtitle {
                            sub_color
                        } else {
                            title_color
                        },
                    ));
                    let is_bold = span.style.is_bold || (!pl.is_subtitle && !span.style.is_italic);
                    tspan.push_attribute(("font-weight", if is_bold { "bold" } else { "normal" }));
                    if span.style.is_italic {
                        tspan.push_attribute(("font-style", "italic"));
                    }
                    if span.style.is_underline {
                        tspan.push_attribute(("text-decoration", "underline"));
                    }
                    if span.style.is_strikethrough {
                        tspan.push_attribute(("text-decoration", "line-through"));
                    }
                }

                w.write_event(Event::Start(tspan))?;
                w.write_event(Event::Text(BytesText::new(&span_text)))?;
                w.write_event(Event::End(BytesEnd::new("tspan")))?;
            }
        }

        if let Some(t) = &node_data.technology {
            if !node_data.label.contains(t) {
                let mut tech_span = BytesStart::new("tspan");
                tech_span.push_attribute(("x", format!("{cx:.1}").as_str()));
                tech_span.push_attribute(("dy", "14"));
                tech_span.push_attribute(("font-family", "JetBrains Mono, monospace"));
                tech_span.push_attribute(("font-size", "9"));
                tech_span.push_attribute(("fill", sub_color));
                w.write_event(Event::Start(tech_span))?;
                w.write_event(Event::Text(BytesText::new(&format!("[{t}]"))))?;
                w.write_event(Event::End(BytesEnd::new("tspan")))?;
            }
        }

        w.write_event(Event::End(BytesEnd::new("text")))?;
    }

    w.write_event(Event::End(BytesEnd::new("svg")))?;
    Ok(String::from_utf8(buf)?)
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
        let compiled = build_graph(payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let edge_plans = rdg_render_core::routing::plan_all_edge_routes(
            &compiled,
            &layout,
            rdg_render_core::routing::RoutingAlgorithm::CornerHeuristic,
            &DesignTokens::default(),
        );
        render_svg(&compiled, &layout, &edge_plans, theme, None, &DesignTokens::default()).unwrap()
    }

    #[test]
    fn test_svg_output_is_valid_xml() {
        let xml = render(&payload_with_types(&[("n1", "default")]), "standard");
        assert!(xml.starts_with("<?xml"));
        assert!(xml.contains("<svg"));
        assert!(xml.contains("</svg>"));
    }

    #[test]
    fn test_database_and_table_share_accent_color_in_svg() {
        let db = render(&payload_with_types(&[("n1", "database")]), "standard");
        let table = render(&payload_with_types(&[("n1", "table")]), "standard");
        assert!(db.contains("stroke=\"#0284c7\""));
        assert!(table.contains("stroke=\"#0284c7\""));
    }

    #[test]
    fn test_card_shadow_and_cylinder_paths_present() {
        let xml = render(&payload_with_types(&[("n1", "database")]), "standard");
        assert!(xml.contains("url(#card-shadow)"));
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
