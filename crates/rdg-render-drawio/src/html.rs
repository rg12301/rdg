//! draw.io HTML label builders (draw.io renders a node `value=` as HTML when `html=1`).
//! Colours, fonts and sizes come from the theme via the node's resolved look.

use rdg_layout::DesignTokens;
use rdg_render_core::look::NodeLook;
use rdg_render_core::theme::Theme;
use rdg_render_core::typography::{parse_inline_spans, wrap_and_classify_label};

/// `#rrggbb` + opacity → CSS `rgba(...)`.
fn rgba(hex: &str, opacity: f64) -> String {
    let h = hex.trim_start_matches('#');
    let c = |i: usize| u8::from_str_radix(h.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0);
    format!("rgba({},{},{},{:.3})", c(0), c(2), c(4), opacity)
}

/// A node's label: the first line as the title, later lines as muted detail lines,
/// inline typography (`code`, **bold**, math, …) applied.
///
/// `inline_icon`: an icon key drawn before the first line, the pair centred together.
pub fn format_html_label(label: &str, theme: &Theme, look: &NodeLook, tokens: &DesignTokens, inline_icon: Option<&str>) -> String {
    let max_chars = if look.shape == "diamond" { tokens.wrap_chars_diamond } else { tokens.wrap_chars_normal };
    let lines = wrap_and_classify_label(label, max_chars);
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
    format!(
        "<font style=\"font-size:{}px;color:{};font-weight:normal;\">{html}</font>",
        theme.font.node_detail_size, look.detail_color
    )
}

/// A node's label plus its `[technology]` detail line.
pub fn format_html_label_with_details(
    label: &str,
    theme: &Theme,
    look: &NodeLook,
    tech: Option<&str>,
    tokens: &DesignTokens,
    inline_icon: Option<&str>,
) -> String {
    let mut result = format_html_label(label, theme, look, tokens, inline_icon);
    if let Some(t) = tech.filter(|t| !label.contains(*t)) {
        result.push_str("<br/>");
        result.push_str(&detail_line(theme, look, &format!("[{t}]")));
    }
    result
}

/// A table (ER entity) or class card: a tinted header with the name, then one row per
/// field. Primary-key fields are set in the node's category hue.
pub fn format_html_table_or_class(label: &str, fields: &[String], theme: &Theme, look: &NodeLook, node_type: &str) -> String {
    let header_bg = rgba(&look.stroke, 0.12);
    let border = &theme.text.dim;
    let title_prefix = if node_type.eq_ignore_ascii_case("interface") {
        "&lt;&lt;interface&gt;&gt;<br/>"
    } else if node_type.eq_ignore_ascii_case("abstract_class") {
        "&lt;&lt;abstract&gt;&gt;<br/>"
    } else {
        ""
    };
    let f = &theme.font;
    let mut html = format!(
        "<table style=\"width:100%;height:100%;border-collapse:collapse;font-family:{};\">\
         <tr style=\"background:{header_bg};\">\
           <td colspan=\"2\" style=\"padding:6px 8px;border-bottom:1px solid {border};text-align:center;color:{};font-size:{}px;\">\
             <b>{title_prefix}{label}</b>\
           </td>\
         </tr>",
        f.family, look.title_color, f.node_title_size
    );
    for field in fields {
        let clean = rdg_layout::strip_markdown_tokens(field);
        let (left, right) = clean.split_once(':').unwrap_or((clean.as_str(), ""));
        let upper = clean.to_ascii_uppercase();
        let is_pk = upper.contains("[PK]") || upper.contains("PRIMARY KEY");
        let name_style = if is_pk {
            format!("font-family:{};font-size:{}px;font-weight:bold;color:{};", f.code_family, f.node_detail_size, look.stroke)
        } else {
            format!("font-family:{};font-size:{}px;color:{};", f.code_family, f.node_detail_size, look.title_color)
        };
        let right_html = if right.trim().is_empty() {
            String::new()
        } else {
            format!(
                "<td style=\"padding:2px 8px;text-align:right;color:{};font-size:{}px;font-family:{};\">{}</td>",
                look.detail_color,
                f.node_detail_size,
                f.code_family,
                right.trim()
            )
        };
        let colspan = if right_html.is_empty() { "colspan=\"2\"" } else { "" };
        html.push_str(&format!(
            "<tr style=\"border-bottom:1px solid {border};\">\
               <td {colspan} style=\"padding:2px 8px;text-align:left;{name_style}\">{}</td>{right_html}\
             </tr>",
            left.trim()
        ));
    }
    html.push_str("</table>");
    html
}
