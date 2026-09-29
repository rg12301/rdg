//! draw.io `style=` strings, built from the theme-resolved looks in
//! `rdg_render_core::look` — no colour, font or size is decided here.

use rdg_render_core::look::{GroupLook, NodeLook};
use rdg_render_core::theme::{ResolvedEdge, Theme};

fn pct(opacity: f64) -> u32 {
    (opacity.clamp(0.0, 1.0) * 100.0).round() as u32
}

/// A node's style. `label_top` is where an icon node's label starts (under its halo).
pub fn node_style(theme: &Theme, look: &NodeLook, label_top: f64) -> String {
    let f = &theme.font;
    let common = |fill: &str, opacity: f64, stroke: &str| {
        format!(
            "html=1;whiteSpace=wrap;fillColor={fill};fillOpacity={};strokeColor={stroke};strokeWidth={};shadow={};\
             fontFamily={};fontSize={};fontColor={};fontStyle=0;",
            pct(opacity),
            look.stroke_width,
            u8::from(look.shadow),
            f.family,
            f.node_title_size,
            look.title_color,
        )
    };
    let pad = "spacingTop=4;spacingBottom=4;spacingLeft=8;spacingRight=8;";
    // Markers draw their label underneath the (tiny) shape.
    let below = "verticalLabelPosition=bottom;verticalAlign=top;whiteSpace=nowrap;";
    match look.shape.as_str() {
        "cylinder" => format!(
            "shape=cylinder3;boundedLbl=1;backgroundOutline=1;size={cap};{}align=center;verticalAlign=middle;{pad}spacingTop={};",
            common(&look.fill, look.fill_opacity, &look.stroke),
            2.0 * theme.node.cylinder_cap,
            cap = theme.node.cylinder_cap
        ),
        "ellipse" => format!("ellipse;{}align=center;verticalAlign=middle;{pad}", common(&look.fill, look.fill_opacity, &look.stroke)),
        "diamond" => format!(
            "rhombus;{}align=center;verticalAlign=middle;spacingLeft=12;spacingRight=12;",
            common(&look.fill, look.fill_opacity, &look.stroke),
        ),
        "start" => format!("ellipse;{}{below}", common(&look.fill, 1.0, &look.stroke)),
        "end" => format!("shape=endState;{}{below}", common(&look.fill, 1.0, &look.stroke)),
        "choice" => format!("rhombus;{}{below}", common(&look.fill, look.fill_opacity, &look.stroke)),
        // Drawn as its logo: an invisible box; the logo is a child image cell, the
        // label starts under the logo's halo.
        "icon" => format!(
            "rounded=0;{}align=center;verticalAlign=top;spacingTop={label_top:.0};spacingBottom=0;spacingLeft=0;spacingRight=0;",
            common("none", 1.0, "none")
        ),
        _ => format!(
            "rounded=1;absoluteArcSize=1;arcSize={};{}align=center;verticalAlign=middle;{pad}",
            2.0 * look.corner_radius,
            common(&look.fill, look.fill_opacity, &look.stroke)
        ),
    }
}

/// A group container's style.
pub fn group_style(theme: &Theme, g: &GroupLook) -> String {
    let dash = g.dash.as_deref().map_or("dashed=0;".to_string(), |d| format!("dashed=1;dashPattern={d};"));
    format!(
        "rounded=1;absoluteArcSize=1;arcSize={};fillColor={};fillOpacity={};strokeColor={};strokeWidth={};{dash}\
         verticalAlign=top;align=left;spacingLeft=12;spacingTop=8;\
         container=1;collapsible=0;recursiveResize=0;connectable=0;html=1;\
         fontFamily={};fontSize={};fontStyle={};fontColor={};",
        2.0 * g.corner_radius,
        g.fill,
        pct(g.fill_opacity),
        g.stroke,
        g.stroke_width,
        theme.font.family,
        theme.font.group_title_size,
        u8::from(g.title_bold),
        g.title_color,
    )
}

/// An edge's line and arrowhead style (without its geometry anchors).
pub fn edge_style(theme: &Theme, e: &ResolvedEdge) -> String {
    let dash = e.dash.as_deref().map_or("dashed=0;".to_string(), |d| format!("dashed=1;dashPattern={d};"));
    let tail = match &e.tail {
        Some(t) => format!("startArrow={t};startFill={};startSize={};", u8::from(e.tail_fill), theme.edge.head_size),
        None => "startArrow=none;".to_string(),
    };
    let label_color = if theme.edge_label.use_edge_color { e.color.as_str() } else { theme.edge_label.color.as_str() };
    format!(
        "edgeStyle=none;html=1;rounded={};jumpStyle={};jumpSize=6;\
         strokeColor={};strokeWidth={};{dash}endArrow={};endFill={};endSize={};{tail}\
         labelBackgroundColor={};labelBorderColor=none;\
         fontFamily={};fontSize={};fontColor={label_color};",
        u8::from(theme.edge.rounded),
        if theme.edge.line_jumps { "arc" } else { "none" },
        e.color,
        e.width,
        e.head,
        u8::from(e.head_fill),
        theme.edge.head_size,
        theme.edge_label.background,
        theme.font.family,
        theme.font.edge_label_size,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rdg_render_core::look::node_look;

    fn look_for(theme: &Theme, ty: &str, shape: &str) -> NodeLook {
        let mut compiled = rdg_graph::build_graph(&rdg_schema::DiagramPayload {
            nodes: vec![rdg_schema::NodeDef { id: "a".into(), label: "A".into(), node_type: ty.into(), ..Default::default() }],
            ..Default::default()
        })
        .unwrap();
        rdg_render_core::look::prepare_graph(theme, &mut compiled);
        let idx = compiled.node_map["a"];
        let mut nd = compiled.graph[idx].clone();
        nd.shape = Some(shape.into());
        node_look(theme, &nd)
    }

    #[test]
    fn node_colours_come_from_the_category() {
        let t = Theme::builtin("light").unwrap();
        let s = node_style(&t, &look_for(&t, "service", "card"), 0.0);
        assert!(s.contains(&format!("strokeColor={}", t.category("backend").stroke)), "{s}");
        assert!(s.contains("fillOpacity="));
        assert!(s.contains(&format!("fontFamily={}", t.font.family)));
    }

    #[test]
    fn database_is_a_cylinder_in_light_and_an_ellipse_is_available() {
        let t = Theme::builtin("light").unwrap();
        assert!(node_style(&t, &look_for(&t, "database", "cylinder"), 0.0).contains("shape=cylinder3"));
        assert!(node_style(&t, &look_for(&t, "queue", "ellipse"), 0.0).starts_with("ellipse;"));
    }

    #[test]
    fn dark_theme_changes_colours_not_meaning() {
        let (l, d) = (Theme::builtin("light").unwrap(), Theme::builtin("dark").unwrap());
        let (sl, sd) = (node_style(&l, &look_for(&l, "queue", "card"), 0.0), node_style(&d, &look_for(&d, "queue", "card"), 0.0));
        assert!(sl.contains(&l.category("messagebus").stroke) && sd.contains(&d.category("messagebus").stroke));
        assert_ne!(sl, sd);
    }

    #[test]
    fn async_edges_are_dashed_in_the_messaging_hue() {
        let t = Theme::builtin("light").unwrap();
        let s = edge_style(&t, &t.edge_look(Some("async")));
        assert!(s.contains("dashed=1") && s.contains(&t.category("messagebus").stroke));
    }
}
