//! draw.io HTML label builders (draw.io renders a node `value=` as HTML when `html=1`).
//! Colours, fonts and sizes come from the theme via the node's resolved look.

use rdg_graph::NodeData;
use rdg_layout::DesignTokens;
use rdg_render_core::look::NodeLook;
use rdg_render_core::theme::Theme;
use rdg_render_core::typography::parse_inline_spans;

/// `#rrggbb` + opacity → CSS `rgba(...)`.
fn rgba(hex: &str, opacity: f64) -> String {
    let h = hex.trim_start_matches('#');
    let c = |i: usize| u8::from_str_radix(h.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0);
    format!("rgba({},{},{},{:.3})", c(0), c(2), c(4), opacity)
}

/// A node's text (see [`rdg_layout::node_lines`]): title lines bold, detail lines muted,
/// inline typography (`code`, **bold**, math, …) applied.
///
/// `inline_icon`: an icon key drawn before the first line, the pair centred together.
pub fn format_html_label(nd: &NodeData, theme: &Theme, look: &NodeLook, tokens: &DesignTokens, inline_icon: Option<&str>) -> String {
    let lines = rdg_layout::node_lines(nd, rdg_layout::wrap_chars_for(nd, tokens));
    let mut html_lines = Vec::new();
    for pl in lines {
        let mut line_html = String::new();
        for span in parse_inline_spans(&pl.text) {
            let mut chunk = span.text;
            if span.style.is_code {
                chunk = format!(
                    "<font face=\"{}\"><code>{chunk}</code></font>",
                    theme.font.code_family
                );
            } else if span.style.is_math {
                chunk = format!("\\({chunk}\\)");
            } else {
                if span.style.is_subscript {
                    chunk = format!("<sub>{chunk}</sub>");
                }
                if span.style.is_superscript {
                    chunk = format!("<sup>{chunk}</sup>");
                }
                if span.style.is_underline {
                    chunk = format!("<u>{chunk}</u>");
                }
                if span.style.is_strikethrough {
                    chunk = format!("<s>{chunk}</s>");
                }
                if span.style.is_italic {
                    chunk = format!("<i>{chunk}</i>");
                }
                if span.style.is_bold {
                    chunk = format!("<b>{chunk}</b>");
                }
            }
            line_html.push_str(&chunk);
        }
        if pl.is_subtitle {
            html_lines.push(detail_line(theme, look, &line_html));
        } else if theme.node.title_bold {
            html_lines.push(format!("<b>{line_html}</b>"));
        } else {
            html_lines.push(line_html);
        }
    }
    let icon = inline_icon.and_then(|k| rdg_icons::icon_as_data_uri(k, theme.icon_style()).map(|uri| (k, uri)));
    if let (Some((key, uri)), Some(first)) = (icon, html_lines.first_mut()) {
        let s = theme.icon.size;
        // On a dark canvas a brand logo sits on the theme's light plate, so dark marks
        // show (category glyphs are drawn in the category's own colour).
        let plate = theme
            .icon
            .backdrop
            .as_deref()
            .filter(|_| !rdg_icons::is_glyph(key))
            .map_or(String::new(), |c| format!("background-color:{c};border-radius:3px;padding:1px;"));
        *first = format!(
            "<img src=\"{uri}\" width=\"{s}\" height=\"{s}\" style=\"vertical-align:middle;margin-right:{}px;{plate}\">{first}",
            theme.icon.gap
        );
    }
    html_lines.join("<br/>")
}

fn detail_line(theme: &Theme, look: &NodeLook, html: &str) -> String {
    let _ = theme;
    format!("<font style=\"font-size:{}px;color:{};font-weight:normal;\">{html}</font>", look.detail_size, look.detail_color)
}

/// A node's text plus its `[technology]` detail line.
pub fn format_html_label_with_details(nd: &NodeData, theme: &Theme, look: &NodeLook, tokens: &DesignTokens, inline_icon: Option<&str>) -> String {
    let mut result = format_html_label(nd, theme, look, tokens, inline_icon);
    if let Some(t) = nd.technology.as_deref().filter(|t| !nd.label.contains(*t)) {
        result.push_str("<br/>");
        result.push_str(&detail_line(theme, look, &format!("[{t}]")));
    }
    result
}

/// A table (ER entity) or class card: a tinted header with the name, then one row per
/// field. Primary-key fields are set in the node's category hue.
pub fn format_html_table_or_class(label: &str, fields: &[String], theme: &Theme, look: &NodeLook, tokens: &DesignTokens, node_type: &str) -> String {
    let header_bg = rgba(&look.stroke, 0.12);
    let border = &theme.text.dim;
    let is_class = matches!(node_type.to_ascii_lowercase().as_str(), "class" | "interface" | "abstract_class" | "struct");
    let title_prefix = if node_type.eq_ignore_ascii_case("interface") {
        "&lt;&lt;interface&gt;&gt;<br/>"
    } else if node_type.eq_ignore_ascii_case("abstract_class") {
        "&lt;&lt;abstract&gt;&gt;<br/>"
    } else {
        ""
    };
    let f = &theme.font;
    // Row and header heights match the `field_*` tokens the card was sized with, so the
    // table fills its box with no blank band under the last row.
    let side = tokens.field_side_pad();
    let head_pad = ((tokens.field_header_height() - tokens.line_height(f.node_title_size)) / 2.0).max(0.0);
    let row_pad = ((tokens.field_row_height() - tokens.line_height(f.node_detail_size)) / 2.0).max(0.0);
    let mut html = format!(
        "<table style=\"width:100%;border-collapse:collapse;font-family:{};line-height:{};\">\
         <tr style=\"background:{header_bg};\">\
           <td colspan=\"2\" style=\"padding:{head_pad:.1}px {side:.1}px;border-bottom:1px solid {border};text-align:center;color:{};font-size:{}px;\">\
             <b>{title_prefix}{label}</b>\
           </td>\
         </tr>",
        f.family, tokens.line_height_ratio, look.title_color, f.node_title_size
    );
    let cleaned: Vec<String> = fields.iter().map(|f| rdg_layout::strip_markdown_tokens(f)).collect();
    let rows: Vec<rdg_layout::FieldRow> = cleaned.iter().map(|c| rdg_layout::field_row(c)).collect();
    let split = if is_class { rdg_layout::field_compartment_split(&rows) } else { None };
    for (i, row) in rows.iter().enumerate() {
        let upper = cleaned[i].to_ascii_uppercase();
        let is_pk = upper.contains("[PK]") || upper.contains("PRIMARY KEY");
        let name_style = if is_pk {
            format!("font-family:{};font-size:{}px;font-weight:bold;color:{};", f.code_family, f.node_detail_size, look.stroke)
        } else {
            format!("font-family:{};font-size:{}px;color:{};", f.code_family, f.node_detail_size, look.title_color)
        };
        // The attribute / operation compartment line sits on the first operation row.
        let row_border = if split == Some(i) { format!("border-top:1.5px solid {border};") } else { String::new() };
        let (left, right_html) = match row {
            rdg_layout::FieldRow::Attribute { name, ty } if !ty.is_empty() => (
                *name,
                format!(
                    "<td style=\"padding:{row_pad:.1}px {side:.1}px;text-align:right;color:{};font-size:{}px;font-family:{};\">{ty}</td>",
                    look.detail_color, f.node_detail_size, f.code_family
                ),
            ),
            rdg_layout::FieldRow::Attribute { name, .. } => (*name, String::new()),
            rdg_layout::FieldRow::Operation(sig) => (*sig, String::new()),
        };
        let colspan = if right_html.is_empty() { "colspan=\"2\"" } else { "" };
        html.push_str(&format!(
            "<tr style=\"{row_border}\">\
               <td {colspan} style=\"padding:{row_pad:.1}px {side:.1}px;text-align:left;{name_style}\">{left}</td>{right_html}\
             </tr>"
        ));
    }
    html.push_str("</table>");
    html
}
