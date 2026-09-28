//! draw.io HTML label builders (drawio renders node `value=` as HTML when `html=1`).

use rdg_layout::DesignTokens;
use rdg_render_core::typography::{parse_inline_spans, wrap_and_classify_label};

/// Format a label for draw.io HTML rendering, applying monospace code tags,
/// MathJax delimiters, text formatting tags, and title/subtitle hierarchy.
pub fn format_html_label(label: &str, theme: &str, node_type: &str, tokens: &DesignTokens) -> String {
    let is_diamond = matches!(
        node_type.to_ascii_lowercase().as_str(),
        "decision" | "condition" | "cache" | "redis" | "memcache"
    );
    // Same wrap width the layout engine's own sizing estimate uses (`wrap.rs`), so
    // the box `rdg-layout` computed is actually wide enough for how this backend
    // wraps the label into it — previously drifted independently (16/20 here vs
    // 14/22 there).
    let max_chars = if is_diamond { tokens.wrap_chars_diamond } else { tokens.wrap_chars_normal };
    let lines = wrap_and_classify_label(label, max_chars);
    if lines.is_empty() {
        return String::new();
    }

    let sub_color = if theme == "dark" {
        "#94a3b8"
    } else {
        "#64748b"
    };

    let mut html_lines = Vec::new();
    for pl in lines {
        let spans = parse_inline_spans(&pl.text);
        let mut line_html = String::new();

        for span in spans {
            let mut chunk = span.text;
            if span.style.is_code {
                chunk = format!(
                    "<font face=\"JetBrains Mono, Menlo, monospace\" style=\"font-size:11px;\"><code>{chunk}</code></font>"
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
            html_lines.push(format!(
                "<font style=\"font-size:10px;color:{sub_color}\">{line_html}</font>"
            ));
        } else {
            html_lines.push(format!("<b>{line_html}</b>"));
        }
    }

    html_lines.join("<br/>")
}

/// Format a table or class diagram card as an HTML table for draw.io rendering.
pub fn format_html_table_or_class(
    label: &str,
    fields: &[String],
    theme: &str,
    node_type: &str,
    _icon_key: Option<&str>,
) -> String {
    let header_bg = if theme == "dark" {
        "#334155"
    } else {
        "#f1f5f9"
    };
    let border_color = if theme == "dark" {
        "#475569"
    } else {
        "#e2e8f0"
    };
    let text_color = if theme == "dark" {
        "#f1f5f9"
    } else {
        "#0f172a"
    };
    let muted_color = if theme == "dark" {
        "#94a3b8"
    } else {
        "#64748b"
    };

    let title_prefix = if node_type.eq_ignore_ascii_case("interface") {
        "&lt;&lt;interface&gt;&gt;<br/>"
    } else if node_type.eq_ignore_ascii_case("abstract_class") {
        "&lt;&lt;abstract&gt;&gt;<br/>"
    } else {
        ""
    };

    let mut html = format!(
        "<table style=\"width:100%;height:100%;border-collapse:collapse;font-family:Inter,Helvetica,sans-serif;\">\
         <tr style=\"background:{header_bg};\">\
           <td colspan=\"2\" style=\"padding:6px 8px;border-bottom:1px solid {border_color};text-align:center;color:{text_color};font-size:12px;\">\
             <b>{title_prefix}{label}</b>\
           </td>\
         </tr>"
    );

    for field in fields {
        let field_clean = rdg_layout::strip_markdown_tokens(field);
        let (left_part, right_part) = if let Some(idx) = field_clean.find(':') {
            (&field_clean[..idx], &field_clean[idx + 1..])
        } else {
            (field_clean.as_str(), "")
        };

        let is_pk = field_clean.to_ascii_uppercase().contains("[PK]")
            || field_clean.to_ascii_uppercase().contains("PRIMARY KEY");
        let name_style = if is_pk {
            "font-family:JetBrains Mono,monospace;font-size:10px;font-weight:bold;color:#d97706;"
                .to_string()
        } else {
            format!("font-family:JetBrains Mono,monospace;font-size:10px;color:{text_color};")
        };

        let right_html = if !right_part.trim().is_empty() {
            format!(
                "<td style=\"padding:2px 8px;text-align:right;color:{muted_color};font-size:10px;font-family:monospace;\">{}</td>",
                right_part.trim()
            )
        } else {
            String::new()
        };

        let colspan = if right_html.is_empty() {
            "colspan=\"2\""
        } else {
            ""
        };

        html.push_str(&format!(
            "<tr style=\"border-bottom:1px solid {border_color};\">\
               <td {colspan} style=\"padding:2px 8px;text-align:left;{name_style}\">{}</td>\
               {right_html}\
             </tr>",
            left_part.trim()
        ));
    }

    html.push_str("</table>");
    html
}

/// Format an HTML label for a node with optional technology badge.
pub fn format_html_label_with_details(
    label: &str,
    theme: &str,
    node_type: &str,
    _icon_key: Option<&str>,
    tech: Option<&str>,
    tokens: &DesignTokens,
) -> String {
    let mut result = format_html_label(label, theme, node_type, tokens);

    if let Some(t) = tech {
        if !label.contains(t) {
            let sub_color = if theme == "dark" {
                "#94a3b8"
            } else {
                "#64748b"
            };
            result.push_str(&format!(
                "<br/><font style=\"font-size:10px;color:{sub_color};font-weight:normal;\">[{t}]</font>"
            ));
        }
    }

    result
}
