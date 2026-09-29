//! draw.io (`mxfile`) XML rendering backend.
//!
//! ## draw.io (`mxfile`) format rules enforced here
//!
//! * Always **uncompressed** — `<mxfile><diagram><mxGraphModel><root>`.
//! * `<root>` always begins with the two mandatory stub cells:
//!   `<mxCell id="0" />` and `<mxCell id="1" parent="0" />`.
//! * Node cells carry `vertex="1"`, edge cells carry `edge="1"` (mutually exclusive).
//! * Coordinates from the layout engine are injected into `<mxGeometry … as="geometry" />`.
//! * Semantic node types are mapped to pre-coded draw.io `style=` strings.

mod html;
mod sequence;
mod style;

pub use style::{edge_style, group_style, node_style};

use anyhow::Result;
use quick_xml::{
    Writer,
    events::{BytesDecl, BytesEnd, BytesStart, Event},
};
use std::collections::HashMap;
use std::io::Cursor;

use petgraph::stable_graph::EdgeIndex;

use rdg_graph::CompiledGraph;
use rdg_layout::{DesignTokens, LayoutResult};
use rdg_render_core::routing::{EdgeRoutingPlan, Side};
use rdg_render_core::look::{edge_look, group_look, node_look};
use rdg_render_core::theme::Theme;

use html::{format_html_label_with_details, format_html_table_or_class};
use sequence::render_sequence_drawio;

/// Render the compiled, laid-out graph to an uncompressed draw.io XML string.
///
/// `edge_plans` is expected to be the output of
/// [`rdg_render_core::routing::plan_all_edge_routes`] (or, more commonly,
/// [`rdg_render_core::review::compute_reviewed_layout`]) for this exact `layout` — the
/// caller computes and reviews it once, then feeds the same result to whichever backend(s)
/// it renders, rather than each backend silently recomputing (and potentially disagreeing
/// on) routing. Ignored for sequence diagrams, which never route edges.
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
    edge_plans: &HashMap<EdgeIndex, EdgeRoutingPlan>,
    theme: &Theme,
    background: Option<&str>,
    tokens: &DesignTokens,
) -> Result<String> {
    if let Some(seq) = &layout.sequence_info {
        return render_sequence_drawio(compiled, layout, seq, theme, background, tokens);
    }

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

    // --- Optional Diagram Title Header --------------------------------------
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
        // Anchored to the content origin (margin), not a fixed pixel position.
        let (title_x, title_y) =
            rdg_render_core::canvas::content_bounds(compiled, layout, edge_plans, tokens)
                .map_or((24.0, 12.0), |b| (b.min_x, b.min_y));
        t_geo.push_attribute(("x", format!("{title_x:.1}").as_str()));
        t_geo.push_attribute(("y", format!("{title_y:.1}").as_str()));
        let title_w = title.chars().count().max(compiled.description.as_deref().map_or(0, |d| d.chars().count())) as f64
            * tokens.char_width(f.title_size);
        t_geo.push_attribute(("width", format!("{:.0}", title_w).as_str()));
        t_geo.push_attribute(("height", format!("{:.0}", tokens.title_band_for(compiled.description.is_some())).as_str()));
        t_geo.push_attribute(("as", "geometry"));
        w.write_event(Event::Empty(t_geo))?;

        w.write_event(Event::End(BytesEnd::new("mxCell")))?;
    }

    // --- Group / Swimlane container cells -----------------------------------
    let mut node_to_group_id: HashMap<String, String> = HashMap::new();
    let mut group_origins: HashMap<String, (f64, f64)> = HashMap::new();

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

        group_origins.insert(group.id.clone(), (gx, gy));
        for nid in &group.nodes {
            node_to_group_id.insert(nid.clone(), group.id.clone());
        }

        let look = group_look(theme, group);
        let style_owned = group_style(theme, &look);
        let group_style = style_owned.as_str();
        let group_value = match group.resolved_icon().and_then(|k| rdg_icons::icon_as_data_uri(&k, theme.icon_style())) {
            Some(uri) => format!(
                "<img src=\"{uri}\" width=\"{s}\" height=\"{s}\" style=\"vertical-align:middle;margin-right:6px;\"/>{}",
                look.title,
                s = theme.font.group_title_size + 4.0
            ),
            None => look.title.clone(),
        };

        let mut g_cell = BytesStart::new("mxCell");
        g_cell.push_attribute(("id", group.id.as_str()));
        g_cell.push_attribute(("value", group_value.as_str()));
        g_cell.push_attribute(("style", group_style));
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
        let look = node_look(theme, node_data);
        let tooltip = node_data.metadata.as_deref().unwrap_or("");
        let is_icon_node = look.shape == "icon";
        // A box's icon goes inline before its title, the two centred together (tables
        // and class cards draw their own header, icon nodes put the logo on top,
        // markers have no icon).
        let boxed = matches!(look.shape.as_str(), "card" | "cylinder" | "ellipse" | "diamond");
        let inline_icon = node_data.icon.as_deref().filter(|_| node_data.fields.is_empty() && boxed);
        let halo = tokens.icon_halo_size();
        let mut style = node_style(theme, &look, halo + tokens.px(0.25));

        // Build HTML label: title line + detail lines, typography, code spans.
        let html_value = if !node_data.fields.is_empty() {
            style.push_str("spacingTop=0;spacingBottom=0;spacingLeft=0;spacingRight=0;overflow=hidden;");
            format_html_table_or_class(&node_data.label, &node_data.fields, theme, &look, &node_data.node_type)
        } else {
            format_html_label_with_details(&node_data.label, theme, &look, node_data.technology.as_deref(), tokens, inline_icon)
        };

        // `style_extra` is a raw draw.io style fragment appended verbatim, for whatever
        // this schema doesn't expose a typed field for.
        if let Some(extra) = &node_data.style_extra {
            style.push_str(extra);
        }

        let (parent_id, rel_x, rel_y) = if let Some(gid) = node_to_group_id.get(&node_data.id) {
            let (gx, gy) = group_origins[gid];
            (gid.as_str(), nl.x - gx, nl.y - gy)
        } else {
            ("1", nl.x, nl.y)
        };

        // A `link` makes the shape clickable in draw.io, represented by wrapping the
        // (now id/value-less) mxCell in a <UserObject id=".." label=".." link="..">, exactly
        // how draw.io itself represents a clickable shape — there's no `link=` style token.
        let wrapped_in_user_object = node_data.link.is_some();
        if let Some(link) = &node_data.link {
            let mut user_object = BytesStart::new("UserObject");
            user_object.push_attribute(("id", node_data.id.as_str()));
            user_object.push_attribute(("label", html_value.as_str()));
            user_object.push_attribute(("link", link.as_str()));
            if !tooltip.is_empty() {
                user_object.push_attribute(("tooltip", tooltip));
            }
            w.write_event(Event::Start(user_object))?;
        }

        let mut cell = BytesStart::new("mxCell");
        if !wrapped_in_user_object {
            cell.push_attribute(("id", node_data.id.as_str()));
            cell.push_attribute(("value", html_value.as_str()));
            if !tooltip.is_empty() {
                cell.push_attribute(("tooltip", tooltip));
            }
        }
        cell.push_attribute(("style", style.as_str()));
        cell.push_attribute(("vertex", "1"));
        cell.push_attribute(("parent", parent_id));
        w.write_event(Event::Start(cell))?;

        // <mxGeometry x="…" y="…" width="…" height="…" as="geometry" />
        let mut geo = BytesStart::new("mxGeometry");
        geo.push_attribute(("x", rel_x.round().to_string().as_str()));
        geo.push_attribute(("y", rel_y.round().to_string().as_str()));
        geo.push_attribute(("width", nl.width.round().to_string().as_str()));
        geo.push_attribute(("height", nl.height.round().to_string().as_str()));
        geo.push_attribute(("as", "geometry"));
        w.write_event(Event::Empty(geo))?;

        w.write_event(Event::End(BytesEnd::new("mxCell")))?;
        if wrapped_in_user_object {
            w.write_event(Event::End(BytesEnd::new("UserObject")))?;
        }

        // An icon node's logo, centred in its halo at the top of the box (a card's icon
        // is inline in its label, above).
        if is_icon_node {
            if let Some(plate) = theme.icon.backdrop.as_deref() {
                // The halo filled with a light plate, so dark marks stay visible on dark
                // canvases.
                let shape = if tokens.icon_halo_circle { "ellipse;".to_string() } else { format!("rounded=1;absoluteArcSize=1;arcSize={};", 2.0 * (look.corner_radius + 2.0)) };
                let plate_style = format!("{shape}fillColor={plate};strokeColor=none;connectable=0;editable=0;");
                let x = (nl.width - halo) / 2.0;
                write_child(&mut w, &format!("{}_plate", node_data.id), &plate_style, &node_data.id, (x, 0.0, halo, halo))?;
            }
            if let Some(uri) = node_data.icon.as_deref().and_then(|k| rdg_icons::icon_as_data_uri(k, theme.icon_style())) {
                let size = tokens.icon_node_size;
                // draw.io styles are `;`-separated, so its image URIs drop the `;base64`.
                let logo_style = format!(
                    "shape=image;image={};imageAspect=1;aspect=fixed;fillColor=none;strokeColor=none;connectable=0;editable=0;",
                    uri.replacen(";base64,", ",", 1)
                );
                let (bx, by) = rdg_render_core::style::logo_offset(nl.width, size, halo);
                write_child(&mut w, &format!("{}_badge", node_data.id), &logo_style, &node_data.id, (bx, by, size, size))?;
            }
        }
    }

    // --- Edge cells ---------------------------------------------------------

    // Label and flow-badge spots come from the shared scorer so draw.io and SVG agree
    // (see `rdg_render_core::annotate`).
    let annotations = rdg_render_core::annotate::place_edge_annotations(compiled, layout, edge_plans, tokens);

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

        let plan = edge_plans.get(&edge_idx);
        let port_frac = plan.map_or(0.5, |p| p.exit_port);
        let entry_port_frac = plan.map_or(0.5, |p| p.entry_port);
        let src_side = plan.map_or(Side::Bottom, |p| p.src_side);
        let dst_side = plan.map_or(Side::Top, |p| p.dst_side);
        let waypoints = plan.map_or(&[][..], |p| p.waypoints.as_slice());

        // Start/end/choice markers are tiny fixed-size shapes (see
        // `estimate_node_size_inner`) whose label now renders *below* the shape's own
        // geometry (`verticalLabelPosition=bottom` in `style.rs`) rather than inside
        // it. draw.io computes `exitX`/`exitY` purely from the source cell's own box,
        // so a plain `exitDy=0` starts the edge exactly at the box's bottom edge —
        // right where that external label sits — and the line was drawn straight
        // through the label text. `exitDy` is a pixel offset draw.io adds on top of
        // the percentage anchor specifically for cases like this; nudging it down by
        // the label's approximate line height clears the text instead of crossing it.
        // A real-fixture re-render (the flowchart samples' start-node outgoing edges)
        // surfaced this directly once the label-outside-the-shape fix landed.
        let (exit_s_idx, _) = if edge_data.reversed { (dst, src) } else { (src, dst) };
        let exit_is_marker = matches!(
            compiled.graph[exit_s_idx].node_type.to_ascii_lowercase().as_str(),
            "start" | "start_state" | "initial" | "initial_state" |
            "end" | "end_state" | "final" | "final_state" |
            "choice" | "branch"
        );
        let marker_bottom_exit = exit_is_marker && src_side == Side::Bottom;
        let exit_perimeter = if marker_bottom_exit { "exitPerimeter=0;" } else { "" };
        let marker_exit_y = if marker_bottom_exit { tokens.marker_label_clearance_ratio } else { 1.0 };

        // Exact attachment on the drawn outline (ellipse, cylinder cap, diamond), as
        // relative coordinates with perimeter projection off — draw.io would otherwise
        // project a bounding-box point toward the centre, landing it off the face normal.
        let rel = |nl: &rdg_layout::NodeLayout, (px, py): (f64, f64)| ((px - nl.x) / nl.width, (py - nl.y) / nl.height);
        let exit_attr = match (layout.positions.get(&exit_s_idx), marker_bottom_exit) {
            (Some(nl), false) => {
                let (ex, ey) = rel(nl, rdg_render_core::routing::node_attach_point(compiled, exit_s_idx, nl, src_side, port_frac, tokens));
                format!("exitX={ex:.5};exitY={ey:.5};exitDx=0;exitDy=0;exitPerimeter=0;")
            }
            _ => format!("exitX={port_frac:.5};exitY={marker_exit_y:.1};exitDx=0;exitDy=0;{exit_perimeter}"),
        };
        let entry_s_idx = if edge_data.reversed { src } else { dst };
        let entry_attr = match layout.positions.get(&entry_s_idx) {
            Some(nl) => {
                let (ex, ey) = rel(nl, rdg_render_core::routing::node_attach_point(compiled, entry_s_idx, nl, dst_side, entry_port_frac, tokens));
                format!("entryX={ex:.5};entryY={ey:.5};entryDx=0;entryDy=0;entryPerimeter=0;")
            }
            None => format!("entryX={entry_port_frac:.5};entryY=0.0;entryDx=0;entryDy=0;"),
        };

        // Flow-numbering badge: a small floating circle beside the line near its source,
        // when this edge has a resolved `step` (i.e. the diagram opted into `numbered: true`).
        if let (Some(step), Some(&(bx, by))) = (edge_data.step, annotations.badges.get(&edge_idx)) {
            write_step_badge(&mut w, &edge_id, step, bx, by, theme, tokens)?;
        }

        // Line, arrowheads and label type from the theme's look for this edge style,
        // with the edge's own overrides; `style_extra` is appended verbatim.
        let look = edge_look(theme, edge_data);
        let edge_style = format!(
            "{exit_attr}{entry_attr}{}{}",
            style::edge_style(theme, &look),
            edge_data.style_extra.as_deref().unwrap_or("")
        );

        let mut cell = BytesStart::new("mxCell");
        cell.push_attribute(("id", edge_id.as_str()));
        let spot = if label.is_empty() { None } else { annotations.labels.get(&edge_idx) };
        // The placer may have wrapped the label onto two lines to fit a gap (html=1 label).
        let label_value = spot.map_or_else(|| label.to_string(), |s| s.text.replace('\n', "<br>"));
        cell.push_attribute(("value", label_value.as_str()));
        cell.push_attribute(("style", edge_style.as_str()));
        cell.push_attribute(("edge", "1"));
        cell.push_attribute(("source", render_src));
        cell.push_attribute(("target", render_dst));
        cell.push_attribute(("parent", "1"));
        w.write_event(Event::Start(cell))?;

        // <mxGeometry x="…" y="…" relative="1" as="geometry">   (label: x along path, y beside it)
        //   <Array as="points"> <mxPoint x="..." y="..." /> … </Array>
        // </mxGeometry>
        let mut geo = BytesStart::new("mxGeometry");
        if let Some(spot) = spot {
            let (gx, gy) = spot.drawio_geometry();
            geo.push_attribute(("x", format!("{gx:.4}").as_str()));
            geo.push_attribute(("y", format!("{gy:.1}").as_str()));
        }
        geo.push_attribute(("relative", "1"));
        geo.push_attribute(("as", "geometry"));
        if waypoints.is_empty() {
            w.write_event(Event::Empty(geo))?;
        } else {
            w.write_event(Event::Start(geo))?;
            let mut arr = BytesStart::new("Array");
            arr.push_attribute(("as", "points"));
            w.write_event(Event::Start(arr))?;
            for &(wx, wy) in waypoints {
                let mut pt = BytesStart::new("mxPoint");
                pt.push_attribute(("x", format!("{wx:.1}").as_str()));
                pt.push_attribute(("y", format!("{wy:.1}").as_str()));
                w.write_event(Event::Empty(pt))?;
            }
            w.write_event(Event::End(BytesEnd::new("Array")))?;
            w.write_event(Event::End(BytesEnd::new("mxGeometry")))?;
        }

        w.write_event(Event::End(BytesEnd::new("mxCell")))?;
    }

    // --- Legend: categories and edge styles used, below the diagram --------------
    if rdg_render_core::look::legend_enabled(theme, compiled) {
        let items = rdg_render_core::look::legend_items(theme, compiled);
        if let (false, Some(b)) = (items.is_empty(), rdg_render_core::canvas::content_bounds(compiled, layout, edge_plans, tokens)) {
            write_legend(&mut w, theme, tokens, &items, b.min_x, b.max_y + tokens.px(3.0), (b.max_x - b.min_x).max(tokens.px(40.0)))?;
        }
    }

    w.write_event(Event::End(BytesEnd::new("root")))?;
    w.write_event(Event::End(BytesEnd::new("mxGraphModel")))?;
    w.write_event(Event::End(BytesEnd::new("diagram")))?;
    w.write_event(Event::End(BytesEnd::new("mxfile")))?;

    Ok(String::from_utf8(buf)?)
}

/// A plain vertex cell at absolute `(x, y, w, h)`.
/// A decorative child cell of `parent` at `rect` (relative to it).
fn write_child<W: std::io::Write>(w: &mut Writer<W>, id: &str, style: &str, parent: &str, rect: (f64, f64, f64, f64)) -> Result<()> {
    let mut c = BytesStart::new("mxCell");
    c.push_attribute(("id", id));
    c.push_attribute(("value", ""));
    c.push_attribute(("style", style));
    c.push_attribute(("vertex", "1"));
    c.push_attribute(("parent", parent));
    w.write_event(Event::Start(c))?;
    let mut g = BytesStart::new("mxGeometry");
    g.push_attribute(("x", format!("{:.0}", rect.0).as_str()));
    g.push_attribute(("y", format!("{:.0}", rect.1).as_str()));
    g.push_attribute(("width", format!("{:.0}", rect.2).as_str()));
    g.push_attribute(("height", format!("{:.0}", rect.3).as_str()));
    g.push_attribute(("as", "geometry"));
    w.write_event(Event::Empty(g))?;
    w.write_event(Event::End(BytesEnd::new("mxCell")))?;
    Ok(())
}

pub(crate) fn write_vertex<W: std::io::Write>(w: &mut Writer<W>, id: &str, value: &str, style: &str, rect: (f64, f64, f64, f64)) -> Result<()> {
    let mut c = BytesStart::new("mxCell");
    c.push_attribute(("id", id));
    c.push_attribute(("value", value));
    c.push_attribute(("style", style));
    c.push_attribute(("vertex", "1"));
    c.push_attribute(("parent", "1"));
    w.write_event(Event::Start(c))?;
    let mut g = BytesStart::new("mxGeometry");
    for (k, v) in [("x", rect.0), ("y", rect.1), ("width", rect.2), ("height", rect.3)] {
        g.push_attribute((k, format!("{v:.1}").as_str()));
    }
    g.push_attribute(("as", "geometry"));
    w.write_event(Event::Empty(g))?;
    w.write_event(Event::End(BytesEnd::new("mxCell")))?;
    Ok(())
}

/// The legend block: a heading, then rows of category swatches and edge-style samples.
pub(crate) fn write_legend<W: std::io::Write>(
    w: &mut Writer<W>,
    theme: &Theme,
    tokens: &DesignTokens,
    items: &[rdg_render_core::look::LegendItem],
    x0: f64,
    y0: f64,
    max_w: f64,
) -> Result<()> {
    use rdg_render_core::look::LegendItem;
    let f = &theme.font;
    let text_style = |size: f64, color: &str, bold: bool| {
        format!(
            "text;html=1;strokeColor=none;fillColor=none;align=left;verticalAlign=middle;whiteSpace=nowrap;fontFamily={};fontSize={size};fontColor={color};fontStyle={};",
            f.family,
            u8::from(bold)
        )
    };
    let head_h = tokens.line_height(f.group_title_size);
    write_vertex(w, "legend_title", "Legend", &text_style(f.group_title_size, &theme.text.primary, true), (x0, y0, 120.0, head_h))?;
    let row_h = f.edge_label_size * 1.35 + 10.0;
    let mut y = y0 + f.group_title_size * 1.35 + 6.0;
    for (r, row) in rdg_render_core::look::legend_rows(items, theme, max_w).iter().enumerate() {
        let mut x = x0;
        for (k, (it, width)) in row.iter().enumerate() {
            let id = format!("legend_{r}_{k}");
            let mid = y + row_h / 2.0;
            let label = match it {
                LegendItem::Category { label, fill, fill_opacity, stroke, .. } => {
                    let style = format!(
                        "rounded=1;absoluteArcSize=1;arcSize=6;fillColor={fill};fillOpacity={};strokeColor={stroke};strokeWidth=1.25;connectable=0;",
                        (fill_opacity * 100.0).round()
                    );
                    write_vertex(w, &format!("{id}_swatch"), "", &style, (x, mid - 7.0, 22.0, 14.0))?;
                    label
                }
                LegendItem::Edge { label, look } => {
                    let dash = look.dash.as_deref().map_or("dashed=0;".to_string(), |d| format!("dashed=1;dashPattern={d};"));
                    let style = format!("endArrow=none;html=1;strokeColor={};strokeWidth={};{dash}", look.color, look.width);
                    let mut c = BytesStart::new("mxCell");
                    let sid = format!("{id}_line");
                    c.push_attribute(("id", sid.as_str()));
                    c.push_attribute(("value", ""));
                    c.push_attribute(("style", style.as_str()));
                    c.push_attribute(("edge", "1"));
                    c.push_attribute(("parent", "1"));
                    w.write_event(Event::Start(c))?;
                    let mut g = BytesStart::new("mxGeometry");
                    g.push_attribute(("relative", "1"));
                    g.push_attribute(("as", "geometry"));
                    w.write_event(Event::Start(g))?;
                    for (k2, px) in [("sourcePoint", x), ("targetPoint", x + 22.0)] {
                        let mut p = BytesStart::new("mxPoint");
                        p.push_attribute(("x", format!("{px:.1}").as_str()));
                        p.push_attribute(("y", format!("{mid:.1}").as_str()));
                        p.push_attribute(("as", k2));
                        w.write_event(Event::Empty(p))?;
                    }
                    w.write_event(Event::End(BytesEnd::new("mxGeometry")))?;
                    w.write_event(Event::End(BytesEnd::new("mxCell")))?;
                    label
                }
            };
            write_vertex(w, &format!("{id}_label"), label, &text_style(f.edge_label_size, &theme.text.muted, false), (x + 30.0, mid - row_h / 2.0, width - 30.0, row_h))?;
            x += width;
        }
        y += row_h;
    }
    Ok(())
}

/// Writes a small floating filled-circle vertex carrying a flow-sequence number, centered
/// at `(x, y)`. Independent of any edge cell — draw.io has no "detached badge on this edge"
/// primitive, so this is just its own tiny vertex placed where the badge belongs.
fn write_step_badge<W: std::io::Write>(
    w: &mut Writer<W>,
    edge_id: &str,
    step: u32,
    x: f64,
    y: f64,
    theme: &Theme,
    tokens: &DesignTokens,
) -> Result<()> {
    let size = 2.0 * tokens.badge_radius;
    let b = &theme.badge;
    let style = format!(
        "ellipse;whiteSpace=wrap;html=1;fillColor={};strokeColor=none;fontColor={};fontSize={};fontStyle=1;fontFamily={};",
        b.fill, b.text, theme.font.badge_size, theme.font.family
    );

    let mut cell = BytesStart::new("mxCell");
    let id = format!("step_{edge_id}");
    cell.push_attribute(("id", id.as_str()));
    cell.push_attribute(("value", step.to_string().as_str()));
    cell.push_attribute(("style", style.as_str()));
    cell.push_attribute(("vertex", "1"));
    cell.push_attribute(("parent", "1"));
    w.write_event(Event::Start(cell))?;

    let mut geo = BytesStart::new("mxGeometry");
    geo.push_attribute(("x", (x - size / 2.0).round().to_string().as_str()));
    geo.push_attribute(("y", (y - size / 2.0).round().to_string().as_str()));
    geo.push_attribute(("width", size.to_string().as_str()));
    geo.push_attribute(("height", size.to_string().as_str()));
    geo.push_attribute(("as", "geometry"));
    w.write_event(Event::Empty(geo))?;

    w.write_event(Event::End(BytesEnd::new("mxCell")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rdg_graph::build_graph;
    use rdg_layout::{LayoutConfig, compute_layout};
    use rdg_schema::{DiagramPayload, EdgeDef, GroupDef, NodeDef};

    fn one_node_payload() -> DiagramPayload {
        DiagramPayload {
            nodes: vec![NodeDef {
                id: "n1".to_owned(),
                label: "Node 1".to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn two_node_payload() -> DiagramPayload {
        DiagramPayload {
            nodes: vec![
                NodeDef {
                    id: "n1".to_owned(),
                    label: "Source".to_owned(),
                    node_type: "database".to_owned(),
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
        render_drawio(&compiled, &layout, &edge_plans, &theme, None, &tokens).unwrap()
    }

    #[test]
    fn test_drawio_has_mandatory_root_cells() {
        let xml = render(&one_node_payload(), "standard");
        assert!(xml.contains("<mxCell id=\"0\""));
        assert!(xml.contains("<mxCell id=\"1\" parent=\"0\""));
    }

    #[test]
    fn test_drawio_node_has_vertex_attribute() {
        let xml = render(&one_node_payload(), "standard");
        assert!(xml.contains("vertex=\"1\""));
    }

    #[test]
    fn test_drawio_edge_has_edge_attribute() {
        let xml = render(&two_node_payload(), "standard");
        assert!(xml.contains("edge=\"1\""));
    }

    #[test]
    fn test_drawio_no_vertex_on_edges() {
        let xml = render(&two_node_payload(), "standard");
        // Every "edge=\"1\"" cell must not also declare vertex="1" on the same mxCell.
        for cell in xml.split("<mxCell").skip(1) {
            let header_end = cell.find('>').unwrap_or(cell.len());
            let header = &cell[..header_end];
            if header.contains("edge=\"1\"") {
                assert!(!header.contains("vertex=\"1\""));
            }
        }
    }

    #[test]
    fn test_drawio_dark_mode_canvas_and_edges() {
        let xml = render(&two_node_payload(), "dark");
        let dark = rdg_render_core::theme::Theme::builtin("dark").unwrap();
        assert!(xml.contains(&format!("background=\"{}\"", dark.canvas.background)));
    }

    #[test]
    fn test_drawio_renders_groups_and_semantic_edge_styles() {
        let payload = DiagramPayload {
            groups: vec![GroupDef {
                id: "g1".to_owned(),
                label: "Cluster".to_owned(),
                color: Some("#0284c7".to_owned()),
                nodes: vec!["n1".to_owned(), "n2".to_owned()],
                ..Default::default()
            }],
            nodes: vec![
                NodeDef {
                    id: "n1".to_owned(),
                    label: "A".to_owned(),
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
                edge_style: Some("async".to_owned()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let xml = render(&payload, "standard");
        assert!(xml.contains("container=1"));
        let light = rdg_render_core::theme::Theme::builtin("light").unwrap();
        let dash = light.edge_look(Some("async")).dash.unwrap();
        assert!(xml.contains(&format!("dashPattern={dash}")));
        // The group keeps its explicit colour.
        assert!(xml.contains("strokeColor=#0284c7"));
    }

    #[test]
    fn test_composition_edge_is_theme_aware() {
        let payload = DiagramPayload {
            nodes: vec![
                NodeDef {
                    id: "n1".to_owned(),
                    label: "A".to_owned(),
                    node_type: "class".to_owned(),
                    ..Default::default()
                },
                NodeDef {
                    id: "n2".to_owned(),
                    label: "B".to_owned(),
                    node_type: "class".to_owned(),
                    ..Default::default()
                },
            ],
            edges: vec![EdgeDef {
                from: "n1".to_owned(),
                to: "n2".to_owned(),
                edge_style: Some("composition".to_owned()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let light = render(&payload, "standard");
        let dark = render(&payload, "dark");
        assert!(light.contains("startArrow=diamond;startFill=1"));
        let (lt, dt) = (
            rdg_render_core::theme::Theme::builtin("light").unwrap(),
            rdg_render_core::theme::Theme::builtin("dark").unwrap(),
        );
        assert!(light.contains(&format!("strokeColor={}", lt.edge.color)));
        assert!(dark.contains(&format!("strokeColor={}", dt.edge.color)));
        assert_ne!(lt.edge.color, dt.edge.color);
    }

    // -----------------------------------------------------------------------
    // Phase G: edge cases + Phase F feature coverage
    // -----------------------------------------------------------------------

    #[test]
    fn test_unicode_edge_and_node_labels_produce_valid_escaped_xml() {
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
        // quick-xml must have escaped the raw "<script>" and "&" rather than passing them
        // through unescaped (which would produce invalid/unsafe XML).
        assert!(!xml.contains("<script>"));
        assert!(xml.contains("支付服务"));
        assert!(xml.contains("é") && xml.contains("ü"));
    }

    #[test]
    fn test_style_extra_appears_verbatim_in_node_and_edge_style() {
        let payload = DiagramPayload {
            nodes: vec![
                NodeDef {
                    id: "n1".to_owned(),
                    label: "A".to_owned(),
                    style_extra: Some("opacity=42;".to_owned()),
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
                style_extra: Some("curved=1;".to_owned()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let xml = render(&payload, "standard");
        assert!(xml.contains("opacity=42;"));
        assert!(xml.contains("curved=1;"));
    }

    #[test]
    fn test_class_preset_resolution_reflected_in_rendered_style() {
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
        assert!(xml.contains("strokeColor=#ef4444"));
    }

    #[test]
    fn test_link_wraps_node_in_user_object() {
        let payload = DiagramPayload {
            nodes: vec![NodeDef {
                id: "n1".to_owned(),
                label: "A".to_owned(),
                link: Some("https://example.com".to_owned()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let xml = render(&payload, "standard");
        assert!(xml.contains("<UserObject"));
        assert!(xml.contains("link=\"https://example.com\""));
        // The id/value move up to UserObject — the inner mxCell must not duplicate them.
        assert!(!xml.contains("<mxCell id=\"n1\""));
    }

    #[test]
    fn test_numbered_edges_render_step_badges() {
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
        assert!(xml.contains("value=\"1\""));
        assert!(xml.contains("id=\"step_"));
    }

    #[test]
    fn test_unnumbered_diagram_has_no_step_badges() {
        let xml = render(&two_node_payload(), "standard");
        assert!(!xml.contains("id=\"step_"));
    }
}
