//! draw.io `style=` string construction, built on the shared node/edge color table in
//! `rdg_render_core::style`.

use rdg_render_core::style::node_accent_color;

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
        "proxy" | "gateway" | "api" => {
            format!("{base}strokeColor={};", node_accent_color(node_type, theme))
        }
        "server" | "service" | "backend" => {
            format!("{base}strokeColor={};", node_accent_color(node_type, theme))
        }
        "database" | "db" | "storage" | "table" | "entity" | "record" => {
            let (bg, text) = ("#ffffff", "#0f172a");
            let stroke = node_accent_color(node_type, theme);
            format!(
                "shape=cylinder3;boundedLbl=1;backgroundOutline=1;size=8;\
                 whiteSpace=wrap;html=1;fillColor={bg};shadow=1;\
                 strokeWidth=1.5;strokeColor={stroke};\
                 fontFamily=Inter,Helvetica,sans-serif;\
                 fontSize=12;fontStyle=1;fontColor={text};\
                 spacingTop=12;spacingBottom=6;spacingLeft=8;spacingRight=8;"
            )
        }
        "queue" | "broker" | "bus" => {
            format!(
                "ellipse;whiteSpace=wrap;html=1;fillColor=#ffffff;shadow=1;\
                 strokeWidth=1.5;strokeColor=#fbbf24;\
                 fontFamily=Inter,Helvetica,sans-serif;\
                 fontSize=12;fontStyle=1;fontColor=#0f172a;\
                 align=center;verticalAlign=middle;"
            )
        }
        "function" | "lambda" | "faas" => {
            // `mxgraph.aws4.lambda` used to be the shape here directly, but that AWS4
            // stencil is an icon-only glyph — unlike `mxgraph.general.person_2` (used
            // for `client`/`user`/`browser` below), it does not paint a `fillColor`/
            // `strokeColor` background behind itself at all, so the node rendered as
            // bare text over a pale watermark with no visible card, unlike every other
            // semantic type. The shared white-card `base` (same family as
            // server/service/cache) gives it the bordered "Orange Lambda card" the CLI
            // help text promises; a real-fixture re-render (`docs/*.yaml` plus varied
            // sample diagrams) confirmed the icon-only shape regression directly.
            format!("{base}strokeColor=#fb923c;")
        }
        "decision" | "condition" => {
            format!(
                "rhombus;whiteSpace=wrap;html=1;fillColor=#ffffff;shadow=1;\
                 strokeWidth=2;strokeColor=#a78bfa;\
                 fontFamily=Inter,Helvetica,sans-serif;\
                 fontSize=11;fontStyle=2;fontColor=#0f172a;\
                 align=center;verticalAlign=middle;\
                 spacingLeft=12;spacingRight=12;"
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
        // These two are deliberately tiny fixed-size markers (28x28 / 32x32, see
        // `estimate_node_size_inner` in `rdg-layout`), far smaller than their label
        // text. Without `html=1` the cell's HTML `value` (built by
        // `format_html_label`, e.g. `<b>Order Placed</b>`) rendered as literal
        // on-canvas text — the `<b>` tags themselves were visible — instead of being
        // interpreted as markup, and without `verticalLabelPosition=bottom` the label
        // centered itself on top of the shape instead of sitting below it, so the text
        // and the small circle overlapped badly. `whiteSpace=nowrap` keeps the label
        // from wrapping to the shape's own tiny width once it's freed to sit outside
        // it. A real-fixture re-render (varied sample diagrams' flowchart start/end
        // nodes) surfaced both defects directly.
        "start" | "start_state" | "initial" | "initial_state" => {
            "shape=ellipse;fillColor=#0f172a;strokeColor=#0f172a;strokeWidth=1;\
             html=1;whiteSpace=nowrap;verticalLabelPosition=bottom;verticalAlign=top;"
                .to_string()
        }
        "end" | "end_state" | "final" | "final_state" => {
            "shape=endState;fillColor=#0f172a;strokeColor=#0f172a;strokeWidth=2;\
             html=1;whiteSpace=nowrap;verticalLabelPosition=bottom;verticalAlign=top;"
                .to_string()
        }
        "choice" | "branch" => {
            // Same fixed tiny-marker sizing (36x36, see `estimate_node_size_inner`) as
            // start/end above, so it needs the same label-outside-the-shape treatment.
            "rhombus;fillColor=#ffffff;strokeColor=#a78bfa;strokeWidth=2;html=1;\
             whiteSpace=nowrap;verticalLabelPosition=bottom;verticalAlign=top;"
                .to_string()
        }
        "cache" | "redis" | "memcache" => {
            format!("{base}strokeColor={};", node_accent_color(node_type, theme))
        }
        "class" | "interface" | "abstract_class" | "struct" => {
            format!(
                "{base}strokeColor={};strokeWidth=1.5;",
                node_accent_color(node_type, theme)
            )
        }
        "participant" | "actor" => {
            format!(
                "{base}strokeColor={};strokeWidth=1.5;",
                node_accent_color(node_type, theme)
            )
        }
        other => format!("{base}strokeColor={};", node_accent_color(other, theme)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_style_mapping_database() {
        let style = style_for_type("database", "standard");
        assert!(style.contains("shape=cylinder3"));
        assert!(style.contains("strokeColor=#0284c7"));
    }

    #[test]
    fn test_style_mapping_default() {
        let style = style_for_type("unknown_type", "standard");
        assert!(style.contains("strokeColor=#cbd5e1"));
    }

    #[test]
    fn test_style_mapping_dark_mode() {
        let style = style_for_type("proxy", "dark");
        assert!(style.contains("fillColor=#1e293b"));
        assert!(style.contains("strokeColor=#475569"));
    }

    #[test]
    fn test_table_and_database_share_style_family() {
        let db = style_for_type("database", "standard");
        let table = style_for_type("table", "standard");
        assert!(db.contains("shape=cylinder3"));
        assert!(table.contains("shape=cylinder3"));
        assert_eq!(db, table);
    }
}
