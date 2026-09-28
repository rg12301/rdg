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

pub use style::style_for_type;

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
use rdg_render_core::routing::{EdgeRoutingPlan, Side, badge_point_near_exit};
use rdg_render_core::style::edge_style_colors;

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
    theme: &str,
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
        // Anchored to the content origin (margin), not a fixed pixel position.
        let (title_x, title_y) =
            rdg_render_core::canvas::content_bounds(compiled, layout, edge_plans, tokens)
                .map_or((24.0, 12.0), |b| (b.min_x, b.min_y));
        t_geo.push_attribute(("x", format!("{title_x:.1}").as_str()));
        t_geo.push_attribute(("y", format!("{title_y:.1}").as_str()));
        t_geo.push_attribute(("width", "500"));
        t_geo.push_attribute(("height", "40"));
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

        let color = group.color.as_deref().unwrap_or("#64748b");
        let group_style = format!(
            "rounded=1;absoluteArcSize=1;arcSize=10;\
             fillColor={color};fillOpacity=10;\
             strokeColor={color};strokeWidth=1.5;\
             dashed=1;dashPattern=6 6;\
             verticalAlign=top;align=left;\
             spacingLeft=16;spacingTop=10;\
             container=1;collapsible=0;recursiveResize=0;connectable=0;\
             fontFamily=Inter,Helvetica,sans-serif;\
             fontStyle=1;fontSize=12;fontColor={color};html=1;"
        );

        let group_value = if let Some(icon_key) = group.resolved_icon() {
            if let Some(uri) = rdg_icons::icon_as_data_uri(&icon_key) {
                format!(
                    "<img src=\"{uri}\" width=\"16\" height=\"16\" style=\"vertical-align:middle;margin-right:6px;\"/><b>{}</b>",
                    group.label
                )
            } else {
                group.label.clone()
            }
        } else {
            group.label.clone()
        };

        let mut g_cell = BytesStart::new("mxCell");
        g_cell.push_attribute(("id", group.id.as_str()));
        g_cell.push_attribute(("value", group_value.as_str()));
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
    let is_dark = theme == "dark";
    for node_idx in compiled.graph.node_indices() {
        let node_data = &compiled.graph[node_idx];
        let nl = match layout.positions.get(&node_idx) {
            Some(p) => p,
            None => continue,
        };
        let mut style = style_for_type(&node_data.node_type, theme);
        let tooltip = node_data.metadata.as_deref().unwrap_or("");

        // Build HTML label: formatted with typography, title/subtitle hierarchy, and code spans
        let html_value = if !node_data.fields.is_empty() {
            style.push_str(
                ";spacingTop=0;spacingBottom=0;spacingLeft=0;spacingRight=0;overflow=hidden;",
            );
            format_html_table_or_class(
                &node_data.label,
                &node_data.fields,
                theme,
                &node_data.node_type,
                node_data.icon.as_deref(),
            )
        } else {
            format_html_label_with_details(
                &node_data.label,
                theme,
                &node_data.node_type,
                node_data.icon.as_deref(),
                node_data.technology.as_deref(),
                tokens,
            )
        };

        // A per-node `color` override wins over the semantic-type accent color; `style_extra`
        // is a raw draw.io style fragment appended verbatim, for whatever this schema doesn't
        // expose a typed field for.
        if let Some(color) = &node_data.color {
            style.push_str(&format!("strokeColor={color};"));
        }
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

        // Render elevated brand / tech icon badge at vertex without wrapper tile
        if let Some(icon_key) = node_data.icon.as_deref() {
            if let Some(uri) = rdg_icons::icon_as_data_uri(icon_key) {
                let badge_id = format!("{}_badge", node_data.id);
                let badge_value = format!("<img src=\"{uri}\" width=\"22\" height=\"22\"/>");
                let badge_style = "fillColor=none;strokeColor=none;rounded=0;shadow=0;html=1;align=center;verticalAlign=middle;connectable=0;";
                let mut badge_cell = BytesStart::new("mxCell");
                badge_cell.push_attribute(("id", badge_id.as_str()));
                badge_cell.push_attribute(("value", badge_value.as_str()));
                badge_cell.push_attribute(("style", badge_style));
                badge_cell.push_attribute(("vertex", "1"));
                badge_cell.push_attribute(("parent", node_data.id.as_str()));
                w.write_event(Event::Start(badge_cell))?;

                let is_diamond = matches!(
                    node_data.node_type.to_ascii_lowercase().as_str(),
                    "decision" | "condition" | "choice" | "branch" | "cache" | "redis" | "memcache"
                );
                let (bx, by) = if is_diamond {
                    (-11, (nl.height / 2.0 - 11.0).round() as i32)
                } else {
                    (-11, -11)
                };

                let mut badge_geo = BytesStart::new("mxGeometry");
                badge_geo.push_attribute(("x", bx.to_string().as_str()));
                badge_geo.push_attribute(("y", by.to_string().as_str()));
                badge_geo.push_attribute(("width", "22"));
                badge_geo.push_attribute(("height", "22"));
                badge_geo.push_attribute(("as", "geometry"));
                w.write_event(Event::Empty(badge_geo))?;

                w.write_event(Event::End(BytesEnd::new("mxCell")))?;
            }
        }
    }

    // --- Edge cells ---------------------------------------------------------
    let default_edge_color: &'static str = if is_dark { "#94a3b8" } else { "#64748b" };
    let label_bg_color = if is_dark { "#1e293b" } else { "#ffffff" };
    let label_font_color = if is_dark { "#cbd5e1" } else { "#475569" };

    // Two different edges' paths can legitimately pass close to each other (common in
    // any real, moderately dense diagram), which lands their labels — each drawn at
    // its own edge's path midpoint, oblivious to every other edge — on the same spot
    // often enough in practice to be worth fixing: confirmed directly while visually
    // reviewing rendered sample diagrams (e.g. a "SQL" label and an "invoke" label
    // overlapping into illegible text reading as "SQLoke"). This pre-pass estimates
    // each labeled edge's default midpoint the same way draw.io would place it (this
    // crate has no literal pixel coordinates elsewhere — every edge's actual
    // attachment point is the percentage-based `exitX`/`entryX` written below — so
    // `port_point` is used here purely to *predict* draw.io's placement for
    // decluttering purposes, in `compiled.graph.edge_indices()` order for
    // determinism), then nudges any that collide with an already-placed label.
    let label_dy: HashMap<EdgeIndex, f64> = {
        let mut anchors = Vec::new();
        let mut ids = Vec::new();
        for edge_idx in compiled.graph.edge_indices() {
            let edge_data = &compiled.graph[edge_idx];
            let Some(label) = edge_data.label.as_deref() else {
                continue;
            };
            if label.is_empty() {
                continue;
            }
            let Some((_, _, src_nl, dst_nl)) =
                rdg_render_core::routing::resolve_edge_layout(compiled, layout, edge_idx)
            else {
                continue;
            };
            let plan = edge_plans.get(&edge_idx);
            let src_side = plan.map_or(Side::Bottom, |p| p.src_side);
            let dst_side = plan.map_or(Side::Top, |p| p.dst_side);
            let exit_port = plan.map_or(0.5, |p| p.exit_port);
            let entry_port = plan.map_or(0.5, |p| p.entry_port);
            let waypoints = plan.map_or(&[][..], |p| p.waypoints.as_slice());
            let p1 = rdg_render_core::routing::port_point(src_nl, src_side, exit_port);
            let p2 = rdg_render_core::routing::port_point(dst_nl, dst_side, entry_port);
            let (mx, my) = rdg_render_core::routing::polyline_midpoint(p1, waypoints, p2);

            let char_count = label.chars().count();
            let edge_label_font = tokens.font_size * 0.83;
            let box_w = (char_count as f64 * tokens.char_width(edge_label_font) + tokens.px(1.5))
                .max(tokens.px(2.5));
            anchors.push((mx, my - tokens.px(1.25), box_w, tokens.line_height(edge_label_font) * 1.2));
            ids.push(edge_idx);
        }
        // Labels must clear every node box too, not just each other — see
        // `declutter_label_positions_avoiding`'s doc comment for the real diagram
        // that surfaced this (a label pushed clear of another label landed squarely
        // on an unrelated node's card instead). Group title banners get the same
        // treatment — a label on top of a group's title text is just as unreadable.
        let mut node_obstacles: Vec<(f64, f64, f64, f64)> =
            layout.positions.values().map(|nl| (nl.x, nl.y, nl.width, nl.height)).collect();
        node_obstacles.extend(
            rdg_render_core::routing::compute_group_title_zones(compiled, layout, tokens)
                .iter()
                .map(|tz| (tz.min_x, tz.min_y, tz.max_x - tz.min_x, tz.max_y - tz.min_y)),
        );
        let dys =
            rdg_render_core::routing::declutter_label_positions_avoiding(&anchors, &node_obstacles, tokens);
        ids.into_iter().zip(dys).collect()
    };

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

        let exit_attr = match src_side {
            Side::Bottom => format!("exitX={port_frac:.3};exitY={marker_exit_y:.1};exitDx=0;exitDy=0;{exit_perimeter}"),
            Side::Top => format!("exitX={port_frac:.3};exitY=0.0;exitDx=0;exitDy=0;"),
            Side::Left => format!("exitX=0.0;exitY={port_frac:.3};exitDx=0;exitDy=0;"),
            Side::Right => format!("exitX=1.0;exitY={port_frac:.3};exitDx=0;exitDy=0;"),
        };

        let entry_attr = match dst_side {
            Side::Top => format!("entryX={entry_port_frac:.3};entryY=0.0;entryDx=0;entryDy=0;"),
            Side::Bottom => format!("entryX={entry_port_frac:.3};entryY=1.0;entryDx=0;entryDy=0;"),
            Side::Left => format!("entryX=0.0;entryY={entry_port_frac:.3};entryDx=0;entryDy=0;"),
            Side::Right => format!("entryX=1.0;entryY={entry_port_frac:.3};entryDx=0;entryDy=0;"),
        };

        // Flow-numbering badge: a small floating circle near the source exit point, when
        // this edge has a resolved `step` (i.e. the diagram opted into `numbered: true`).
        if let Some(step) = edge_data.step {
            let (s_idx, _) = if edge_data.reversed {
                (dst, src)
            } else {
                (src, dst)
            };
            if let Some(src_nl) = layout.positions.get(&s_idx) {
                // Start/end/choice markers are tiny fixed-size shapes whose label now
                // renders *below* them rather than inside (see
                // `rdg-render-svg`'s `write_label_below_marker` and this crate's
                // `style.rs` `verticalLabelPosition=bottom`) — the default 14px badge
                // offset lands the badge right on top of that label instead of near
                // the shape itself. Push it out past the label for those node types;
                // every other node type keeps its label inside the card, so 14px next
                // to the card edge stays correct there.
                let is_marker = matches!(
                    compiled.graph[s_idx].node_type.to_ascii_lowercase().as_str(),
                    "start" | "start_state" | "initial" | "initial_state" |
                    "end" | "end_state" | "final" | "final_state" |
                    "choice" | "branch"
                );
                // `port_point` (used by `badge_point_near_exit`) computes from the
                // node's own true geometry, unaware of the `exitY=2.0` trick used
                // below for these same marker types' actual edge exit point — so the
                // badge needs a larger distance to end up past both the label and
                // that pushed-out exit point, not just past the label. Not so large
                // that it reaches into a short edge's destination box, though — the
                // node-to-node gap on a start/end marker's own outgoing edge is often
                // small (e.g. an immediately-following decision or terminal node).
                let distance = if is_marker { src_nl.height * 1.4 } else { tokens.px(1.75) };
                let (bx, by) = badge_point_near_exit(src_nl, src_side, port_frac, distance);
                write_step_badge(&mut w, &edge_id, step, bx, by, is_dark)?;
            }
        }

        let edge_style_key = edge_data.edge_style.as_deref();

        // Stroke/width/dash come from the shared `rdg_render_core::style` table (also used by
        // the SVG backend); the arrow-marker tokens are draw.io-specific style-string syntax
        // and stay here.
        let mut custom_style = if matches!(edge_style_key, Some("bi") | Some("bidirectional")) {
            format!(
                "strokeColor={default_edge_color};strokeWidth=1.5;\
                 startArrow=blockThin;startFill=1;endArrow=blockThin;endFill=1;"
            )
        } else {
            let colors = edge_style_colors(edge_style_key, theme, default_edge_color);
            let dash_prefix = colors
                .dash
                .map(|d| format!("dashed=1;dashPattern={d};"))
                .unwrap_or_default();
            let arrow_tokens = match edge_style_key {
                Some("async") => "endArrow=open;endFill=0;",
                Some("error") | Some("fallback") => "endArrow=blockThin;endFill=0;",
                Some("one_to_many") => "startArrow=ERone;startFill=0;endArrow=ERmany;endFill=0;",
                Some("many_to_many") => "startArrow=ERmany;startFill=0;endArrow=ERmany;endFill=0;",
                Some("one_to_one") => "startArrow=ERone;startFill=0;endArrow=ERone;endFill=0;",
                Some("zero_to_many") => {
                    "startArrow=ERzeroToOne;startFill=0;endArrow=ERmany;endFill=0;"
                }
                Some("inheritance") | Some("realization") => "endArrow=block;endFill=0;endSize=10;",
                Some("composition") => "startArrow=diamond;startFill=1;startSize=12;endArrow=none;",
                Some("aggregation") => "startArrow=diamond;startFill=0;startSize=12;endArrow=none;",
                Some("dependency") => "endArrow=open;endFill=0;",
                _ => "endArrow=blockThin;endFill=1;",
            };
            format!(
                "{dash_prefix}strokeColor={};strokeWidth={};{arrow_tokens}",
                colors.stroke, colors.width
            )
        };

        if let Some(c) = &edge_data.color {
            custom_style.push_str(&format!("strokeColor={c};"));
        }
        if let Some(w) = edge_data.width {
            custom_style.push_str(&format!("strokeWidth={w:.1};"));
        }
        if let Some(ls) = &edge_data.line_style {
            match ls.to_ascii_lowercase().as_str() {
                "dashed" => custom_style.push_str("dashed=1;dashPattern=8 4;"),
                "dotted" => custom_style.push_str("dashed=1;dashPattern=2 3;"),
                "solid" => custom_style.push_str("dashed=0;"),
                _ => {}
            }
        }
        if let Some(h) = &edge_data.head {
            custom_style.push_str(&format!("endArrow={h};"));
        }
        if let Some(t) = &edge_data.tail {
            custom_style.push_str(&format!("startArrow={t};"));
        }
        if let Some(extra) = &edge_data.style_extra {
            custom_style.push_str(extra);
        }

        let edge_style = format!(
            "edgeStyle=none;\
             rounded=1;html=1;\
             {exit_attr}\
             {entry_attr}\
             {custom_style}\
             endSize=6;\
             jumpStyle=arc;jumpSize=6;\
             labelBackgroundColor={label_bg_color};labelBorderColor=none;\
             fontFamily=Inter,Helvetica,sans-serif;fontSize=11;fontColor={label_font_color};"
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
        //   <Array as="points">
        //     <mxPoint x="..." y="..." />
        //   </Array>
        //   <mxPoint y="-10" as="offset" />
        // </mxGeometry>
        let mut geo = BytesStart::new("mxGeometry");
        geo.push_attribute(("relative", "1"));
        geo.push_attribute(("as", "geometry"));
        if !waypoints.is_empty() || !label.is_empty() {
            w.write_event(Event::Start(geo))?;

            if !waypoints.is_empty() {
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
            }

            if !label.is_empty() {
                let dy = label_dy.get(&edge_idx).copied().unwrap_or(0.0);
                let offset_y = -tokens.px(1.25) + dy;
                let mut pt = BytesStart::new("mxPoint");
                pt.push_attribute(("y", format!("{offset_y:.1}").as_str()));
                pt.push_attribute(("as", "offset"));
                w.write_event(Event::Empty(pt))?;
            }

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

/// Writes a small floating filled-circle vertex carrying a flow-sequence number, centered
/// at `(x, y)`. Independent of any edge cell — draw.io has no "detached badge on this edge"
/// primitive, so this is just its own tiny vertex placed where the badge belongs.
fn write_step_badge<W: std::io::Write>(
    w: &mut Writer<W>,
    edge_id: &str,
    step: u32,
    x: f64,
    y: f64,
    is_dark: bool,
) -> Result<()> {
    let size = 18.0;
    let (fill, font_color) = if is_dark {
        ("#f1f5f9", "#0f172a")
    } else {
        ("#0f172a", "#ffffff")
    };
    let style = format!(
        "ellipse;whiteSpace=wrap;html=1;fillColor={fill};strokeColor=none;\
         fontColor={font_color};fontSize=10;fontStyle=1;fontFamily=Inter,Helvetica,sans-serif;"
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
        let compiled = build_graph(payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let edge_plans = rdg_render_core::routing::plan_all_edge_routes(
            &compiled,
            &layout,
            rdg_render_core::routing::RoutingAlgorithm::CornerHeuristic,
            &DesignTokens::default(),
        );
        render_drawio(&compiled, &layout, &edge_plans, theme, None, &DesignTokens::default()).unwrap()
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
        assert!(xml.contains("background=\"#0f172a\""));
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
        assert!(xml.contains("dashPattern=8 4"));
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
        assert!(light.contains("startArrow=diamond;startFill=1;startSize=12"));
        assert!(light.contains("strokeColor=#0f172a;strokeWidth=1.5;startArrow=diamond"));
        assert!(dark.contains("strokeColor=#cbd5e1;strokeWidth=1.5;startArrow=diamond"));
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
