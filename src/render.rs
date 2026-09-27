//! Output rendering: converts a laid-out graph to draw.io XML or SVG.
//!
//! ## draw.io (`mxfile`) format rules enforced here
//!
//! * Always **uncompressed** — `<mxfile><diagram><mxGraphModel><root>`.
//! * `<root>` always begins with the two mandatory stub cells:
//!   `<mxCell id="0" />` and `<mxCell id="1" parent="0" />`.
//! * Node cells carry `vertex="1"`, edge cells carry `edge="1"` (mutually exclusive).
//! * Coordinates from the layout engine are injected into `<mxGeometry … as="geometry" />`.
//! * Semantic node types are mapped to pre-coded draw.io `style=` strings.

use anyhow::Result;
use quick_xml::{
    Writer,
    events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event},
};
use std::collections::HashMap;
use std::io::Cursor;

use crate::graph::CompiledGraph;
use crate::layout::LayoutResult;

// ---------------------------------------------------------------------------
// Style mapping
// ---------------------------------------------------------------------------

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
        "proxy" | "gateway" | "api" => format!("{base}strokeColor=#818cf8;"),
        "server" | "service" | "backend" => format!("{base}strokeColor=#34d399;"),
        "database" | "db" | "storage" | "table" | "entity" | "record" => {
            let (bg, text, stroke) = if theme == "dark" {
                ("#1e293b", "#f1f5f9", "#38bdf8")
            } else {
                ("#ffffff", "#0f172a", "#0284c7")
            };
            format!(
                "shape=cylinder3;boundedLbl=1;backgroundOutline=1;size=15;\
                 whiteSpace=wrap;html=1;fillColor={bg};shadow=1;\
                 strokeWidth=1.5;strokeColor={stroke};\
                 fontFamily=Inter,Helvetica,sans-serif;\
                 fontSize=12;fontStyle=1;fontColor={text};\
                 spacingTop=16;spacingBottom=6;spacingLeft=8;spacingRight=8;"
            )
        }
        "queue" | "broker" | "bus" => {
            format!(
                "shape=mxgraph.flowchart.start_2;\
                     perimeter=mxPerimeter.ellipsePerimeter;\
                     whiteSpace=wrap;html=1;fillColor=#ffffff;shadow=1;\
                     strokeWidth=1.5;strokeColor=#fbbf24;\
                     fontFamily=Inter,Helvetica,sans-serif;\
                     fontSize=12;fontStyle=1;fontColor=#0f172a;"
            )
        }
        "cache" | "redis" | "memcache" => format!("{base}strokeColor=#f87171;"),
        "function" | "lambda" | "faas" => {
            format!(
                "shape=mxgraph.aws4.lambda;\
                     whiteSpace=wrap;html=1;fillColor=#ffffff;shadow=1;\
                     strokeWidth=1.5;strokeColor=#fb923c;\
                     fontFamily=Inter,Helvetica,sans-serif;\
                     fontSize=12;fontStyle=1;fontColor=#0f172a;"
            )
        }
        "decision" | "condition" => {
            format!(
                "rhombus;whiteSpace=wrap;html=1;fillColor=#ffffff;shadow=1;\
                     strokeWidth=2;strokeColor=#a78bfa;\
                     fontFamily=Inter,Helvetica,sans-serif;\
                     fontSize=11;fontStyle=2;fontColor=#0f172a;"
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
        "class" | "interface" | "abstract_class" | "struct" => format!("{base}strokeColor=#6366f1;strokeWidth=1.5;"),
        "participant" | "actor" => format!("{base}strokeColor=#64748b;strokeWidth=1.5;"),
        "start" | "start_state" | "initial" | "initial_state" => {
            "shape=ellipse;fillColor=#0f172a;strokeColor=#0f172a;strokeWidth=1;".to_string()
        }
        "end" | "end_state" | "final" | "final_state" => {
            "shape=endState;fillColor=#0f172a;strokeColor=#0f172a;strokeWidth=2;".to_string()
        }
        "choice" | "branch" => {
            "rhombus;fillColor=#ffffff;strokeColor=#a78bfa;strokeWidth=2;whiteSpace=wrap;html=1;".to_string()
        }
        _ => format!("{base}strokeColor=#cbd5e1;"),
    }
}

/// Stroke colour used for a given node type in the white-card paradigm.
pub fn stroke_for_type(node_type: &str, theme: &str) -> &'static str {
    match node_type.to_ascii_lowercase().as_str() {
        "proxy" | "gateway" | "api" => "#818cf8",
        "server" | "service" | "backend" => "#34d399",
        "database" | "db" | "storage" => "#38bdf8",
        "queue" | "broker" | "bus" => "#fbbf24",
        "cache" | "redis" | "memcache" => "#f87171",
        "function" | "lambda" | "faas" => "#fb923c",
        "decision" | "condition" => "#a78bfa",
        "client" | "user" | "browser" => "#94a3b8",
        "table" | "entity" | "record" => "#0284c7",
        "class" | "interface" | "abstract_class" | "struct" => "#6366f1",
        "participant" | "actor" => "#64748b",
        "start" | "start_state" | "initial" | "initial_state"
        | "end" | "end_state" | "final" | "final_state" => "#0f172a",
        "choice" | "branch" => "#a78bfa",
        _ => {
            if theme == "dark" {
                "#475569"
            } else {
                "#cbd5e1"
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Typography & Inline Formatting (Markdown, LaTeX, Code & Titles)
// ---------------------------------------------------------------------------

/// Maps ASCII digits and signs to Unicode subscript glyphs.
pub fn to_subscript(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '0' => '₀',
            '1' => '₁',
            '2' => '₂',
            '3' => '₃',
            '4' => '₄',
            '5' => '₅',
            '6' => '₆',
            '7' => '₇',
            '8' => '₈',
            '9' => '₉',
            '+' => '₊',
            '-' => '₋',
            '=' => '₌',
            '(' => '₍',
            ')' => '₎',
            'a' => 'ₐ',
            'e' => 'ₑ',
            'h' => 'ₕ',
            'i' => 'ᵢ',
            'j' => 'ⱼ',
            'k' => 'ₖ',
            'l' => 'ₗ',
            'm' => 'ₘ',
            'n' => 'ₙ',
            'o' => 'ₒ',
            'p' => 'ₚ',
            'r' => 'ᵣ',
            's' => 'ₛ',
            't' => 'ₜ',
            'u' => 'ᵤ',
            'v' => 'ᵥ',
            'x' => 'ₓ',
            _ => c,
        })
        .collect()
}

/// Maps ASCII digits and signs to Unicode superscript glyphs.
pub fn to_superscript(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '0' => '⁰',
            '1' => '¹',
            '2' => '²',
            '3' => '³',
            '4' => '⁴',
            '5' => '⁵',
            '6' => '⁶',
            '7' => '⁷',
            '8' => '⁸',
            '9' => '⁹',
            '+' => '⁺',
            '-' => '⁻',
            '=' => '⁼',
            '(' => '⁽',
            ')' => '⁾',
            'a' => 'ᵃ',
            'b' => 'ᵇ',
            'c' => 'ᶜ',
            'd' => 'ᵈ',
            'e' => 'ᵉ',
            'f' => 'ᶠ',
            'g' => 'ᵍ',
            'h' => 'ʰ',
            'i' => 'ⁱ',
            'j' => 'ʲ',
            'k' => 'ᵏ',
            'l' => 'ˡ',
            'm' => 'ᵐ',
            'n' => 'ⁿ',
            'o' => 'ᵒ',
            'p' => 'ᵖ',
            'r' => 'ʳ',
            's' => 'ˢ',
            't' => 'ᵗ',
            'u' => 'ᵘ',
            'v' => 'ᵛ',
            'w' => 'ʷ',
            'x' => 'ˣ',
            'y' => 'ʸ',
            'z' => 'ᶻ',
            _ => c,
        })
        .collect()
}

/// Converts common LaTeX mathematical operators, relations, and Greek letters
/// to their Unicode equivalents for native SVG rendering.
pub fn latex_to_unicode(latex: &str) -> String {
    let mut s = latex.to_string();
    let replacements = [
        (r"\times", "×"),
        (r"\cdot", "·"),
        (r"\approx", "≈"),
        (r"\le", "≤"),
        (r"\ge", "≥"),
        (r"\neq", "≠"),
        (r"\ne", "≠"),
        (r"\pm", "±"),
        (r"\to", "→"),
        (r"\rightarrow", "→"),
        (r"\leftarrow", "←"),
        (r"\in", "∈"),
        (r"\notin", "∉"),
        (r"\subset", "⊂"),
        (r"\subseteq", "⊆"),
        (r"\cap", "∩"),
        (r"\cup", "∪"),
        (r"\infty", "∞"),
        (r"\partial", "∂"),
        (r"\nabla", "∇"),
        (r"\sum", "∑"),
        (r"\prod", "∏"),
        (r"\int", "∫"),
        (r"\alpha", "α"),
        (r"\beta", "β"),
        (r"\gamma", "γ"),
        (r"\delta", "δ"),
        (r"\epsilon", "ε"),
        (r"\zeta", "ζ"),
        (r"\eta", "η"),
        (r"\theta", "θ"),
        (r"\kappa", "κ"),
        (r"\lambda", "λ"),
        (r"\mu", "μ"),
        (r"\nu", "ν"),
        (r"\xi", "ξ"),
        (r"\pi", "π"),
        (r"\rho", "ρ"),
        (r"\sigma", "σ"),
        (r"\tau", "τ"),
        (r"\phi", "φ"),
        (r"\chi", "χ"),
        (r"\psi", "ψ"),
        (r"\omega", "ω"),
        (r"\Delta", "Δ"),
        (r"\Gamma", "Γ"),
        (r"\Lambda", "Λ"),
        (r"\Sigma", "Σ"),
        (r"\Phi", "Φ"),
        (r"\Psi", "Ψ"),
        (r"\Omega", "Ω"),
    ];
    for (from, to) in replacements {
        s = s.replace(from, to);
    }
    s
}

/// Visual styling attributes for an inline span.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpanStyle {
    pub is_code: bool,
    pub is_bold: bool,
    pub is_italic: bool,
    pub is_underline: bool,
    pub is_strikethrough: bool,
    pub is_subscript: bool,
    pub is_superscript: bool,
    pub is_math: bool,
}

/// A parsed span of text with associated typographic styling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledSpan {
    pub text: String,
    pub style: SpanStyle,
}

/// Parse a single line into styled spans, recognizing:
/// - `` `code` ``
/// - `**bold**`
/// - `~~strikethrough~~`
/// - `__underline__`
/// - `*italic*`
/// - `~subscript~`
/// - `^superscript^`
/// - `$math$` or `\(math\)`
pub fn parse_inline_spans(input: &str) -> Vec<StyledSpan> {
    let mut spans = Vec::new();
    let mut current_text = String::new();
    let s = input.trim();
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut i = 0;

    let flush = |spans: &mut Vec<StyledSpan>, text: &mut String| {
        if !text.is_empty() {
            spans.push(StyledSpan {
                text: std::mem::take(text),
                style: SpanStyle::default(),
            });
        }
    };

    while i < n {
        // Inline code: `...`
        if chars[i] == '`' {
            let start = i + 1;
            let mut j = start;
            while j < n && chars[j] != '`' {
                j += 1;
            }
            if j < n && chars[j] == '`' && j > start {
                flush(&mut spans, &mut current_text);
                let code_content: String = chars[start..j].iter().collect();
                spans.push(StyledSpan {
                    text: code_content,
                    style: SpanStyle {
                        is_code: true,
                        ..Default::default()
                    },
                });
                i = j + 1;
                continue;
            }
        }

        // Bold: **...**
        if i + 1 < n && chars[i] == '*' && chars[i + 1] == '*' {
            let start = i + 2;
            let mut j = start;
            while j + 1 < n && !(chars[j] == '*' && chars[j + 1] == '*') {
                j += 1;
            }
            if j + 1 < n && chars[j] == '*' && chars[j + 1] == '*' && j > start {
                flush(&mut spans, &mut current_text);
                let bold_content: String = chars[start..j].iter().collect();
                spans.push(StyledSpan {
                    text: bold_content,
                    style: SpanStyle {
                        is_bold: true,
                        ..Default::default()
                    },
                });
                i = j + 2;
                continue;
            }
        }

        // Strikethrough: ~~...~~
        if i + 1 < n && chars[i] == '~' && chars[i + 1] == '~' {
            let start = i + 2;
            let mut j = start;
            while j + 1 < n && !(chars[j] == '~' && chars[j + 1] == '~') {
                j += 1;
            }
            if j + 1 < n && chars[j] == '~' && chars[j + 1] == '~' && j > start {
                flush(&mut spans, &mut current_text);
                let strike_content: String = chars[start..j].iter().collect();
                spans.push(StyledSpan {
                    text: strike_content,
                    style: SpanStyle {
                        is_strikethrough: true,
                        ..Default::default()
                    },
                });
                i = j + 2;
                continue;
            }
        }

        // Underline: __...__
        if i + 1 < n && chars[i] == '_' && chars[i + 1] == '_' {
            let start = i + 2;
            let mut j = start;
            while j + 1 < n && !(chars[j] == '_' && chars[j + 1] == '_') {
                j += 1;
            }
            if j + 1 < n && chars[j] == '_' && chars[j + 1] == '_' && j > start {
                flush(&mut spans, &mut current_text);
                let u_content: String = chars[start..j].iter().collect();
                spans.push(StyledSpan {
                    text: u_content,
                    style: SpanStyle {
                        is_underline: true,
                        ..Default::default()
                    },
                });
                i = j + 2;
                continue;
            }
        }

        // Italic: *...*
        if chars[i] == '*' {
            let start = i + 1;
            let mut j = start;
            while j < n && chars[j] != '*' {
                j += 1;
            }
            if j < n && chars[j] == '*' && j > start {
                flush(&mut spans, &mut current_text);
                let it_content: String = chars[start..j].iter().collect();
                spans.push(StyledSpan {
                    text: it_content,
                    style: SpanStyle {
                        is_italic: true,
                        ..Default::default()
                    },
                });
                i = j + 1;
                continue;
            }
        }

        // Subscript: ~...~
        if chars[i] == '~' {
            let start = i + 1;
            let mut j = start;
            while j < n && chars[j] != '~' {
                j += 1;
            }
            if j < n && chars[j] == '~' && j > start {
                flush(&mut spans, &mut current_text);
                let sub_content: String = chars[start..j].iter().collect();
                spans.push(StyledSpan {
                    text: sub_content,
                    style: SpanStyle {
                        is_subscript: true,
                        ..Default::default()
                    },
                });
                i = j + 1;
                continue;
            }
        }

        // Superscript: ^...^
        if chars[i] == '^' {
            let start = i + 1;
            let mut j = start;
            while j < n && chars[j] != '^' {
                j += 1;
            }
            if j < n && chars[j] == '^' && j > start {
                flush(&mut spans, &mut current_text);
                let sup_content: String = chars[start..j].iter().collect();
                spans.push(StyledSpan {
                    text: sup_content,
                    style: SpanStyle {
                        is_superscript: true,
                        ..Default::default()
                    },
                });
                i = j + 1;
                continue;
            }
        }

        // LaTeX math: $...$
        if chars[i] == '$' {
            let start = i + 1;
            let mut j = start;
            while j < n && chars[j] != '$' {
                j += 1;
            }
            if j < n && chars[j] == '$' && j > start {
                flush(&mut spans, &mut current_text);
                let math_content: String = chars[start..j].iter().collect();
                spans.push(StyledSpan {
                    text: math_content,
                    style: SpanStyle {
                        is_math: true,
                        ..Default::default()
                    },
                });
                i = j + 1;
                continue;
            }
        }

        // LaTeX math: \(...\)
        if i + 1 < n && chars[i] == '\\' && chars[i + 1] == '(' {
            let start = i + 2;
            let mut j = start;
            while j + 1 < n && !(chars[j] == '\\' && chars[j + 1] == ')') {
                j += 1;
            }
            if j + 1 < n && chars[j] == '\\' && chars[j + 1] == ')' && j > start {
                flush(&mut spans, &mut current_text);
                let math_content: String = chars[start..j].iter().collect();
                spans.push(StyledSpan {
                    text: math_content,
                    style: SpanStyle {
                        is_math: true,
                        ..Default::default()
                    },
                });
                i = j + 2;
                continue;
            }
        }

        current_text.push(chars[i]);
        i += 1;
    }

    flush(&mut spans, &mut current_text);
    if spans.is_empty() {
        spans.push(StyledSpan {
            text: s.to_string(),
            style: SpanStyle::default(),
        });
    }
    spans
}

/// A line classified as either a title line or a muted subtitle line.
#[derive(Debug, Clone)]
pub struct ProcessedLine {
    pub text: String,
    pub is_subtitle: bool,
}

/// Wraps label into lines and classifies each line.
///
/// Multi-line titles (like `petgraph::\nStableDiGraph`) keep both lines as bold titles.
/// Parenthesized `(subtitle)` or bracketed `[detail]` or `{fields}` blocks are classified
/// as muted subtitles.
pub fn wrap_and_classify_label(label: &str, max_chars: usize) -> Vec<ProcessedLine> {
    let mut out = Vec::new();
    let mut in_multiline_block = false;

    for raw_line in label.split('\n') {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let clean = crate::layout::strip_markdown_tokens(trimmed);
        let c_trim = clean.trim();

        let is_sub = if in_multiline_block {
            if c_trim.ends_with('}') || c_trim.ends_with(')') || c_trim.ends_with(']') {
                in_multiline_block = false;
            }
            true
        } else if (c_trim.starts_with('(') && c_trim.ends_with(')'))
            || (c_trim.starts_with('[') && c_trim.ends_with(']'))
            || (c_trim.starts_with('{') && c_trim.ends_with('}'))
        {
            true
        } else if c_trim.starts_with('{') || c_trim.starts_with('(') || c_trim.starts_with('[') {
            in_multiline_block = true;
            true
        } else {
            false
        };

        let wrapped = crate::layout::wrap_label(trimmed, max_chars);
        for line in wrapped {
            out.push(ProcessedLine {
                text: line,
                is_subtitle: is_sub,
            });
        }
    }

    if out.is_empty() {
        out.push(ProcessedLine {
            text: label.to_string(),
            is_subtitle: false,
        });
    }

    out
}

/// Format a label for draw.io HTML rendering, applying monospace code tags,
/// MathJax delimiters, text formatting tags, and title/subtitle hierarchy.
pub fn format_html_label(label: &str, theme: &str, node_type: &str) -> String {
    let is_diamond = matches!(
        node_type.to_ascii_lowercase().as_str(),
        "decision" | "condition" | "cache" | "redis" | "memcache"
    );
    let max_chars = if is_diamond { 16 } else { 20 };
    let lines = wrap_and_classify_label(label, max_chars);
    if lines.is_empty() {
        return String::new();
    }

    let sub_color = if theme == "dark" { "#94a3b8" } else { "#64748b" };

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
    icon_key: Option<&str>,
) -> String {
    let header_bg = if theme == "dark" { "#334155" } else { "#f1f5f9" };
    let border_color = if theme == "dark" { "#475569" } else { "#e2e8f0" };
    let text_color = if theme == "dark" { "#f1f5f9" } else { "#0f172a" };
    let muted_color = if theme == "dark" { "#94a3b8" } else { "#64748b" };

    let title_prefix = if node_type.eq_ignore_ascii_case("interface") {
        "&lt;&lt;interface&gt;&gt;<br/>"
    } else if node_type.eq_ignore_ascii_case("abstract_class") {
        "&lt;&lt;abstract&gt;&gt;<br/>"
    } else {
        ""
    };

    let icon_html = icon_key
        .and_then(crate::icons::icon_as_data_uri)
        .map(|uri| format!("<img src=\"{uri}\" width=\"14\" height=\"14\" style=\"vertical-align:middle;margin-right:4px;display:inline-block;\"/>"))
        .unwrap_or_default();

    let mut html = format!(
        "<table style=\"width:100%;height:100%;border-collapse:collapse;font-family:Inter,Helvetica,sans-serif;\">\
         <tr style=\"background:{header_bg};\">\
           <td colspan=\"2\" style=\"padding:6px 8px;border-bottom:1px solid {border_color};text-align:center;color:{text_color};font-size:12px;\">\
             <b>{title_prefix}{icon_html}{label}</b>\
           </td>\
         </tr>"
    );

    for field in fields {
        let field_clean = crate::layout::strip_markdown_tokens(field);
        let (left_part, right_part) = if let Some(idx) = field_clean.find(':') {
            (&field_clean[..idx], &field_clean[idx + 1..])
        } else {
            (field_clean.as_str(), "")
        };

        let is_pk = field_clean.to_ascii_uppercase().contains("[PK]")
            || field_clean.to_ascii_uppercase().contains("PRIMARY KEY");
        let name_style = if is_pk {
            "font-family:JetBrains Mono,monospace;font-size:10px;font-weight:bold;color:#d97706;".to_string()
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

/// Format an HTML label for a node with optional icon and technology badge.
pub fn format_html_label_with_details(
    label: &str,
    theme: &str,
    node_type: &str,
    icon_key: Option<&str>,
    tech: Option<&str>,
) -> String {
    let base_html = format_html_label(label, theme, node_type);
    let icon_html = icon_key
        .and_then(crate::icons::icon_as_data_uri)
        .map(|uri| format!("<img src=\"{uri}\" width=\"16\" height=\"16\" style=\"vertical-align:middle;margin-right:5px;display:inline-block;\"/>"))
        .unwrap_or_default();

    let mut result = if !icon_html.is_empty() {
        format!("{icon_html}{base_html}")
    } else {
        base_html
    };

    if let Some(t) = tech {
        if !label.contains(t) {
            let sub_color = if theme == "dark" { "#94a3b8" } else { "#64748b" };
            result.push_str(&format!(
                "<br/><font style=\"font-size:10px;color:{sub_color};font-weight:normal;\">[{t}]</font>"
            ));
        }
    }

    result
}

/// Bounding face for node connector port attachment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    Top,
    Bottom,
    Left,
    Right,
}

impl Side {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "top" | "north" | "n" => Some(Side::Top),
            "bottom" | "south" | "s" => Some(Side::Bottom),
            "left" | "west" | "w" => Some(Side::Left),
            "right" | "east" | "e" => Some(Side::Right),
            _ => None,
        }
    }
}

impl std::str::FromStr for Side {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Side::parse(s).ok_or(())
    }
}

/// Metadata bounding container title zones to prevent edge cutting through group titles.
#[derive(Debug, Clone)]
pub struct GroupTitleZone {
    pub min_x: f64,
    pub max_x: f64,
    pub min_y: f64,
    pub max_y: f64,
    pub node_ids: std::collections::HashSet<String>,
}

/// Computes the group title avoidance zones from compiled groups and layout positions.
pub fn compute_group_title_zones(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
) -> Vec<GroupTitleZone> {
    let mut zones = Vec::new();
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

        let pad_h = crate::layout::GROUP_PAD_H;
        let pad_top = crate::layout::GROUP_PAD_TOP;
        let gx = (min_x - pad_h).max(10.0);
        let gy = (min_y - pad_top).max(10.0);
        let title_w = (group.label.chars().count() as f64 * 8.5 + 40.0).max(160.0);

        zones.push(GroupTitleZone {
            min_x: gx,
            max_x: gx + title_w,
            min_y: gy,
            max_y: gy + 34.0,
            node_ids: group.nodes.iter().cloned().collect(),
        });
    }
    zones
}

/// Determines the exit side and entry side for an edge, evaluating Euclidean face distance
/// as the starting candidate, while adjusting for geometric flow, obstacle avoidance,
/// and container title banners.
pub fn choose_edge_sides(
    src_nl: &crate::layout::NodeLayout,
    src_id: &str,
    dst_nl: &crate::layout::NodeLayout,
    dst_id: &str,
    edge_data: &crate::graph::EdgeData,
    title_zones: &[GroupTitleZone],
) -> (Side, Side) {
    let explicit_src = edge_data.source_port.as_deref().and_then(Side::parse);
    let explicit_dst = edge_data.target_port.as_deref().and_then(Side::parse);

    if let (Some(s), Some(d)) = (explicit_src, explicit_dst) {
        return (s, d);
    }

    let src_cx = src_nl.x + src_nl.width * 0.5;
    let src_cy = src_nl.y + src_nl.height * 0.5;
    let dst_cx = dst_nl.x + dst_nl.width * 0.5;
    let dst_cy = dst_nl.y + dst_nl.height * 0.5;

    let src_faces = [
        (Side::Bottom, (src_cx, src_nl.y + src_nl.height)),
        (Side::Right, (src_nl.x + src_nl.width, src_cy)),
        (Side::Top, (src_cx, src_nl.y)),
        (Side::Left, (src_nl.x, src_cy)),
    ];

    let dst_faces = [
        (Side::Top, (dst_cx, dst_nl.y)),
        (Side::Left, (dst_nl.x, dst_cy)),
        (Side::Bottom, (dst_cx, dst_nl.y + dst_nl.height)),
        (Side::Right, (dst_nl.x + dst_nl.width, dst_cy)),
    ];

    let mut best_pair = (Side::Bottom, Side::Top);
    let mut best_score = f64::MAX;

    for &(s_side, s_pt) in &src_faces {
        if let Some(es) = explicit_src {
            if es != s_side {
                continue;
            }
        }
        for &(d_side, d_pt) in &dst_faces {
            if let Some(ed) = explicit_dst {
                if ed != d_side {
                    continue;
                }
            }

            let dx = d_pt.0 - s_pt.0;
            let dy = d_pt.1 - s_pt.1;
            let dist = (dx * dx + dy * dy).sqrt();

            let mut penalty = 0.0_f64;

            // 1. Natural launch direction penalties: avoid sharp 180° backward launch
            match s_side {
                Side::Bottom => {
                    if dst_nl.y + dst_nl.height < src_nl.y {
                        penalty += 800.0;
                    }
                }
                Side::Top => {
                    if dst_nl.y > src_nl.y + src_nl.height {
                        penalty += 800.0;
                    }
                }
                Side::Right => {
                    if dst_nl.x + dst_nl.width < src_nl.x {
                        penalty += 800.0;
                    }
                }
                Side::Left => {
                    if dst_nl.x > src_nl.x + src_nl.width {
                        penalty += 800.0;
                    }
                }
            }

            // 2. Natural arrival direction penalties
            match d_side {
                Side::Top => {
                    if src_nl.y + src_nl.height > dst_nl.y + 10.0 {
                        penalty += 800.0;
                    }
                }
                Side::Bottom => {
                    if src_nl.y < dst_nl.y + dst_nl.height - 10.0 {
                        penalty += 800.0;
                    }
                }
                Side::Left => {
                    if src_nl.x + src_nl.width > dst_nl.x + 10.0 {
                        penalty += 800.0;
                    }
                }
                Side::Right => {
                    if src_nl.x < dst_nl.x + dst_nl.width - 10.0 {
                        penalty += 800.0;
                    }
                }
            }

            // 3. Container title banner collision avoidance.
            // Only penalise Top entry into a group if:
            //   (a) the source is already inside the same group (internal edges must not exit-top), OR
            //   (b) there is insufficient whitespace above the group title for a clean clearance stub
            //       (i.e. the vertical gap between source bottom and group top is < 40px).
            // We do NOT penalise when the source is well above the group with plenty of clearance,
            // because Top entry is the cleanest route in that case (one bend, uses whitespace above).
            for tz in title_zones {
                if tz.node_ids.contains(dst_id) && d_side == Side::Top {
                    let src_inside_group = tz.node_ids.contains(src_id);
                    let gap_above_group = tz.min_y - (src_nl.y + src_nl.height);
                    let insufficient_clearance = gap_above_group < 40.0;
                    if src_inside_group || insufficient_clearance {
                        penalty += 15000.0;
                    }
                }
            }

            // 4. Primary orientation flow bonuses
            let mut bonus = 0.0_f64;
            let is_horizontal_flow = (dst_nl.x - src_nl.x) > (dst_nl.y - src_nl.y).abs() * 1.2;
            let is_vertical_flow = (dst_nl.y - src_nl.y) > (dst_nl.x - src_nl.x).abs() * 1.2;

            if (is_vertical_flow && s_side == Side::Bottom && d_side == Side::Top)
                || (is_horizontal_flow && s_side == Side::Right && d_side == Side::Left)
            {
                bonus += 60.0;
            }

            let score = dist + penalty - bonus;
            if score < best_score {
                best_score = score;
                best_pair = (s_side, d_side);
            }
        }
    }

    best_pair
}

/// Routing plan containing resolved attachment faces, distributed ports, channels, and collision-free waypoints.
#[derive(Debug, Clone)]
pub struct EdgeRoutingPlan {
    pub src_side: Side,
    pub dst_side: Side,
    pub exit_port: f64,
    pub entry_port: f64,
    pub channel_y: f64,
    pub corridor_x: f64,
    pub waypoints: Vec<(f64, f64)>,
}

/// Computes intelligent, obstacle-aware routing plans for all edges in the graph.
///
/// Features:
/// 1. Group title collision avoidance (enters Side::Left or Side::Right instead of cutting title banners).
/// 2. Port sorting monotonically by target entry coordinates (zero self-crossings among siblings).
/// 3. Multi-channel corridor staggering (parallel horizontal segments have dedicated channels, zero overlapping lines).
/// 4. Obstacle-aware vertical corridor allocation (prevents lines from routing through intermediate components).
pub fn plan_all_edge_routes(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
) -> HashMap<petgraph::stable_graph::EdgeIndex, EdgeRoutingPlan> {
    let title_zones = compute_group_title_zones(compiled, layout);
    let mut initial_sides: HashMap<petgraph::stable_graph::EdgeIndex, (Side, Side)> = HashMap::new();

    for edge_idx in compiled.graph.edge_indices() {
        let (src, dst) = compiled.graph.edge_endpoints(edge_idx).unwrap();
        let edge_data = &compiled.graph[edge_idx];
        let (s_idx, d_idx) = if edge_data.reversed { (dst, src) } else { (src, dst) };
        let (Some(src_nl), Some(dst_nl)) = (layout.positions.get(&s_idx), layout.positions.get(&d_idx)) else {
            continue;
        };
        let src_id = &compiled.graph[s_idx].id;
        let dst_id = &compiled.graph[d_idx].id;
        let sides = choose_edge_sides(src_nl, src_id, dst_nl, dst_id, edge_data, &title_zones);
        initial_sides.insert(edge_idx, sides);
    }

    // Step 2: Distribute exit ports without crossing
    // Group outgoing edges by (source_node, src_side)
    let mut outgoing_groups: HashMap<(petgraph::stable_graph::NodeIndex, Side), Vec<(petgraph::stable_graph::EdgeIndex, f64)>> = HashMap::new();

    for (&edge_idx, &(src_side, dst_side)) in &initial_sides {
        let (src, dst) = compiled.graph.edge_endpoints(edge_idx).unwrap();
        let edge_data = &compiled.graph[edge_idx];
        let (s_idx, d_idx) = if edge_data.reversed { (dst, src) } else { (src, dst) };
        let dst_nl = layout.positions.get(&d_idx).unwrap();

        // Sort coordinate: actual target entry coordinate
        let target_coord = match dst_side {
            Side::Left => dst_nl.x,
            Side::Right => dst_nl.x + dst_nl.width,
            Side::Top | Side::Bottom => dst_nl.x + dst_nl.width * 0.5,
        };
        outgoing_groups.entry((s_idx, src_side)).or_default().push((edge_idx, target_coord));
    }

    let mut exit_ports: HashMap<petgraph::stable_graph::EdgeIndex, f64> = HashMap::new();
    for (_, mut edges) in outgoing_groups {
        edges.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(&b.0)));
        let count = edges.len();
        for (i, (edge_idx, _)) in edges.into_iter().enumerate() {
            let port = if count <= 1 {
                0.5
            } else {
                0.1 + (0.8 / (count - 1) as f64) * (i as f64)
            };
            exit_ports.insert(edge_idx, port);
        }
    }

    // Step 3: Distribute entry ports without crossing
    let mut incoming_groups: HashMap<(petgraph::stable_graph::NodeIndex, Side), Vec<(petgraph::stable_graph::EdgeIndex, f64)>> = HashMap::new();

    for (&edge_idx, &(src_side, dst_side)) in &initial_sides {
        let (src, dst) = compiled.graph.edge_endpoints(edge_idx).unwrap();
        let edge_data = &compiled.graph[edge_idx];
        let (s_idx, d_idx) = if edge_data.reversed { (dst, src) } else { (src, dst) };
        let src_nl = layout.positions.get(&s_idx).unwrap();

        let src_coord = match src_side {
            Side::Left => src_nl.x,
            Side::Right => src_nl.x + src_nl.width,
            Side::Top | Side::Bottom => src_nl.x + src_nl.width * 0.5,
        };
        incoming_groups.entry((d_idx, dst_side)).or_default().push((edge_idx, src_coord));
    }

    let mut entry_ports: HashMap<petgraph::stable_graph::EdgeIndex, f64> = HashMap::new();
    for (_, mut edges) in incoming_groups {
        edges.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(&b.0)));
        let count = edges.len();
        for (i, (edge_idx, _)) in edges.into_iter().enumerate() {
            let port = if count <= 1 {
                0.5
            } else {
                0.2 + (0.6 / (count - 1) as f64) * (i as f64)
            };
            entry_ports.insert(edge_idx, port);
        }
    }

    // Step 4: Multi-channel corridor allocation for parallel horizontal segments
    let mut corridor_buckets: HashMap<(i32, i32), Vec<(petgraph::stable_graph::EdgeIndex, f64)>> = HashMap::new();
    let mut channel_y_map: HashMap<petgraph::stable_graph::EdgeIndex, f64> = HashMap::new();

    for (&edge_idx, &(src_side, dst_side)) in &initial_sides {
        let (src, dst) = compiled.graph.edge_endpoints(edge_idx).unwrap();
        let edge_data = &compiled.graph[edge_idx];
        let (s_idx, d_idx) = if edge_data.reversed { (dst, src) } else { (src, dst) };
        let src_nl = layout.positions.get(&s_idx).unwrap();
        let dst_nl = layout.positions.get(&d_idx).unwrap();

        if src_side == Side::Bottom && dst_side == Side::Top {
            let y1 = src_nl.y + src_nl.height;
            let y2 = dst_nl.y;
            if y2 > y1 + 10.0 {
                let key = ((y1 / 45.0).round() as i32, (y2 / 45.0).round() as i32);
                let mid_x = (src_nl.x + dst_nl.x) / 2.0;
                corridor_buckets.entry(key).or_default().push((edge_idx, mid_x));
                continue;
            }
        }

        // Default channel height
        let y1 = src_nl.y + src_nl.height;
        let y2 = dst_nl.y;
        channel_y_map.insert(edge_idx, (y1 + y2) / 2.0);
    }

    for (_, mut edges) in corridor_buckets {
        edges.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(&b.0)));
        let m = edges.len();
        let first_edge = edges[0].0;
        let (src, dst) = compiled.graph.edge_endpoints(first_edge).unwrap();
        let edge_data = &compiled.graph[first_edge];
        let (s_idx, d_idx) = if edge_data.reversed { (dst, src) } else { (src, dst) };
        let src_nl = layout.positions.get(&s_idx).unwrap();
        let dst_nl = layout.positions.get(&d_idx).unwrap();
        let y1 = src_nl.y + src_nl.height;
        let y2 = dst_nl.y;
        let base_ymid = (y1 + y2) / 2.0;
        let max_spread = (y2 - y1 - 20.0).max(0.0);
        let gap = 16.0_f64.min(max_spread / (m as f64).max(1.0));

        for (k, (edge_idx, _)) in edges.into_iter().enumerate() {
            let ch_y = base_ymid + (k as f64 - (m - 1) as f64 * 0.5) * gap;
            channel_y_map.insert(edge_idx, ch_y);
        }
    }

    // Protect all group title banners: if any horizontal segment intersects a container title banner,
    // shift channel_y above the container to preserve header legibility.
    for (edge_idx, ch_y) in channel_y_map.iter_mut() {
        let (src, dst) = compiled.graph.edge_endpoints(*edge_idx).unwrap();
        let edge_data = &compiled.graph[*edge_idx];
        let (s_idx, d_idx) = if edge_data.reversed { (dst, src) } else { (src, dst) };
        let src_nl = layout.positions.get(&s_idx).unwrap();
        let dst_nl = layout.positions.get(&d_idx).unwrap();

        let x_span_min = src_nl.x.min(dst_nl.x);
        let x_span_max = (src_nl.x + src_nl.width).max(dst_nl.x + dst_nl.width);

        for tz in &title_zones {
            if x_span_max >= tz.min_x
                && x_span_min <= tz.max_x + 60.0
                && *ch_y >= tz.min_y - 8.0
                && *ch_y <= tz.max_y + 12.0
            {
                *ch_y = tz.min_y - 16.0;
            }
        }
    }

    // Step 5: Obstacle-aware vertical corridor allocation for horizontal transitions
    let mut corridor_x_map: HashMap<petgraph::stable_graph::EdgeIndex, f64> = HashMap::new();
    for (&edge_idx, &(src_side, dst_side)) in &initial_sides {
        let (src, dst) = compiled.graph.edge_endpoints(edge_idx).unwrap();
        let edge_data = &compiled.graph[edge_idx];
        let (s_idx, d_idx) = if edge_data.reversed { (dst, src) } else { (src, dst) };
        let src_nl = layout.positions.get(&s_idx).unwrap();
        let dst_nl = layout.positions.get(&d_idx).unwrap();

        if (src_side == Side::Right && dst_side == Side::Left)
            || (src_side == Side::Left && dst_side == Side::Right)
        {
            let x1 = if src_side == Side::Right { src_nl.x + src_nl.width } else { src_nl.x };
            let x2 = if dst_side == Side::Left { dst_nl.x } else { dst_nl.x + dst_nl.width };
            let y1 = src_nl.y + src_nl.height * 0.5;
            let y2 = dst_nl.y + dst_nl.height * 0.5;
            let y_min = y1.min(y2);
            let y_max = y1.max(y2);

            let mut best_x = (x1 + x2) / 2.0;

            // Check if best_x collides with any intermediate node in the vertical span
            for (other_idx, other_nl) in &layout.positions {
                if *other_idx == s_idx || *other_idx == d_idx {
                    continue;
                }
                if (other_nl.x - 12.0..=other_nl.x + other_nl.width + 12.0).contains(&best_x)
                    && other_nl.y + other_nl.height >= y_min
                    && other_nl.y <= y_max
                {
                    // Obstacle detected! Shift into the open white space to the right of the obstacle
                    let right_candidate = other_nl.x + other_nl.width + 24.0;
                    if x2 > x1 && right_candidate < x2 - 12.0 {
                        best_x = right_candidate;
                    } else if x2 > x1 {
                        best_x = (other_nl.x - 24.0).max(x1 + 12.0);
                    }
                }
            }
            corridor_x_map.insert(edge_idx, best_x);
        }
    }

    // Build a complete obstacle map from all node positions — used to avoid routing through them.
    // We collect these once and filter per-edge inside the loop.
    let all_obstacles: Vec<(petgraph::stable_graph::NodeIndex, ObstacleRect)> = layout
        .positions
        .iter()
        .map(|(&ni, nl)| {
            (ni, ObstacleRect { x: nl.x, y: nl.y, w: nl.width, h: nl.height })
        })
        .collect();

    let mut plans = HashMap::new();
    for (edge_idx, (src_side, dst_side)) in initial_sides {
        let (src, dst) = compiled.graph.edge_endpoints(edge_idx).unwrap();
        let edge_data = &compiled.graph[edge_idx];
        let (s_idx, d_idx) = if edge_data.reversed { (dst, src) } else { (src, dst) };
        let src_nl = layout.positions.get(&s_idx).unwrap();
        let dst_nl = layout.positions.get(&d_idx).unwrap();

        let exit_port = exit_ports.get(&edge_idx).copied().unwrap_or(0.5);
        let entry_port = entry_ports.get(&edge_idx).copied().unwrap_or(0.5);
        let channel_y = channel_y_map.get(&edge_idx).copied().unwrap_or((src_nl.y + dst_nl.y) / 2.0);
        let corridor_x = corridor_x_map.get(&edge_idx).copied().unwrap_or(0.0);

        let (x1, y1) = match src_side {
            Side::Bottom => (src_nl.x + src_nl.width * exit_port, src_nl.y + src_nl.height),
            Side::Top => (src_nl.x + src_nl.width * exit_port, src_nl.y),
            Side::Left => (src_nl.x, src_nl.y + src_nl.height * exit_port),
            Side::Right => (src_nl.x + src_nl.width, src_nl.y + src_nl.height * exit_port),
        };

        let (x2, y2) = match dst_side {
            Side::Top => (dst_nl.x + dst_nl.width * entry_port, dst_nl.y),
            Side::Bottom => (dst_nl.x + dst_nl.width * entry_port, dst_nl.y + dst_nl.height),
            Side::Left => (dst_nl.x, dst_nl.y + dst_nl.height * entry_port),
            Side::Right => (dst_nl.x + dst_nl.width, dst_nl.y + dst_nl.height * entry_port),
        };

        // Build per-edge obstacle list: exclude source and destination nodes so their
        // face-stubs are not treated as blocked.
        let edge_obstacles: Vec<ObstacleRect> = all_obstacles
            .iter()
            .filter(|(ni, _)| *ni != s_idx && *ni != d_idx)
            .map(|(_, obs)| obs.clone())
            .collect();

        let waypoints = compute_edge_waypoints_with_obstacles(
            (x1, y1), src_side,
            (x2, y2), dst_side,
            channel_y, corridor_x,
            &edge_obstacles,
        );

        plans.insert(edge_idx, EdgeRoutingPlan {
            src_side,
            dst_side,
            exit_port,
            entry_port,
            channel_y,
            corridor_x,
            waypoints,
        });
    }

    plans
}

/// Minimum straight clearance stub extending perpendicularly from any component face before any bend.
pub const STUB_CLEARANCE: f64 = 24.0;

/// A simple AABB obstacle used for arrow routing.
#[derive(Debug, Clone)]
pub struct ObstacleRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl ObstacleRect {
    /// Returns true if axis-aligned segment from (ax,ay)→(bx,by) clips through this rect.
    /// Segment must be purely horizontal or purely vertical.
    fn clips_segment(&self, ax: f64, ay: f64, bx: f64, by: f64, m: f64) -> bool {
        let (rx0, rx1) = (self.x - m, self.x + self.w + m);
        let (ry0, ry1) = (self.y - m, self.y + self.h + m);

        if (ay - by).abs() < 0.5 {
            // Horizontal segment
            let y = ay;
            if y <= ry0 || y >= ry1 { return false; }
            let (sx0, sx1) = (ax.min(bx), ax.max(bx));
            sx1 > rx0 && sx0 < rx1
        } else {
            // Vertical segment
            let x = ax;
            if x <= rx0 || x >= rx1 { return false; }
            let (sy0, sy1) = (ay.min(by), ay.max(by));
            sy1 > ry0 && sy0 < ry1
        }
    }
}


/// Generate the ideal orthogonal waypoints for a given face-pair, with guaranteed
/// STUB_CLEARANCE perpendicular stubs at source and destination.
///
/// The `channel_y` and `corridor_x` hints are used for the shared-channel slot so
/// parallel sibling edges don't overlap.  The result is a polyline of intermediate
/// waypoints (not including p1/p2 themselves).
fn ideal_waypoints_for_faces(
    p1: (f64, f64),
    src_side: Side,
    p2: (f64, f64),
    dst_side: Side,
    channel_y: f64,
    corridor_x: f64,
) -> Vec<(f64, f64)> {
    let (x1, y1) = p1;
    let (x2, y2) = p2;
    let sc = STUB_CLEARANCE;

    match (src_side, dst_side) {
        // ── Straight down → straight up  (most common flow-diagram case) ──────
        (Side::Bottom, Side::Top) => {
            if (x2 - x1).abs() < 1.5 {
                // Same column — no waypoints needed (straight vertical line)
                vec![]
            } else if y2 >= y1 + sc * 2.0 {
                // There is room for a single horizontal jog
                let min_ch = y1 + sc;
                let max_ch = y2 - sc;
                let ch_y = channel_y.clamp(min_ch, max_ch);
                vec![(x1, ch_y), (x2, ch_y)]
            } else {
                // Nodes too close vertically — go around the side
                let y_down = y1 + sc;
                let y_up = y2 - sc;
                let side_x = if corridor_x > 0.0 {
                    corridor_x
                } else if x2 > x1 {
                    x1.min(x2) - 32.0
                } else {
                    x1.max(x2) + 32.0
                };
                vec![(x1, y_down), (side_x, y_down), (side_x, y_up), (x2, y_up)]
            }
        }

        // ── Right → Left  (horizontal same-level flow) ───────────────────────
        (Side::Right, Side::Left) => {
            if (y2 - y1).abs() < 1.5 {
                vec![]
            } else if x2 >= x1 + sc * 2.0 {
                let min_cr = x1 + sc;
                let max_cr = x2 - sc;
                let cr_x = if corridor_x > 0.0 { corridor_x } else { (x1 + x2) / 2.0 };
                let cr_x = cr_x.clamp(min_cr, max_cr);
                vec![(cr_x, y1), (cr_x, y2)]
            } else {
                // Nodes overlap/too close horizontally — route around vertically
                let cr_x = if corridor_x > 0.0 { corridor_x } else { x1.max(x2) + 32.0 };
                vec![(cr_x, y1), (cr_x, y2)]
            }
        }

        // ── Left → Right  (reverse horizontal) ──────────────────────────────
        (Side::Left, Side::Right) => {
            if (y2 - y1).abs() < 1.5 {
                vec![]
            } else {
                let cr_x = if corridor_x > 0.0 {
                    corridor_x
                } else {
                    x1.min(x2) - 32.0
                };
                let cr_x = cr_x.min(x1 - sc).min(x2 - sc);
                vec![(cr_x, y1), (cr_x, y2)]
            }
        }

        // ── Up → Down  (back-edge) ────────────────────────────────────────────
        (Side::Top, Side::Bottom) => {
            if (x2 - x1).abs() < 1.5 {
                vec![]
            } else {
                let ch_y = if channel_y > 0.0 { channel_y } else { (y1 + y2) / 2.0 };
                let ch_y = ch_y.min(y1 - sc).max(y2 + sc);
                vec![(x1, ch_y), (x2, ch_y)]
            }
        }

        // ── Bottom → Left  (turn right-down-left) ────────────────────────────
        (Side::Bottom, Side::Left) => {
            if x2 >= x1 + sc && y2 >= y1 + sc {
                // Clean L-bend: one corner
                vec![(x1, y2)]
            } else {
                let y_stub = y1 + sc;
                let x_stub = (x2 - sc).min(x1 - 24.0);
                vec![(x1, y_stub), (x_stub, y_stub), (x_stub, y2)]
            }
        }

        // ── Bottom → Right  (turn left-down-right) ───────────────────────────
        (Side::Bottom, Side::Right) => {
            if x1 >= x2 + sc && y2 >= y1 + sc {
                vec![(x1, y2)]
            } else {
                let y_stub = y1 + sc;
                let x_stub = (x2 + sc).max(x1 + 24.0);
                vec![(x1, y_stub), (x_stub, y_stub), (x_stub, y2)]
            }
        }

        // ── Top → Left ────────────────────────────────────────────────────────
        (Side::Top, Side::Left) => {
            let y_stub = y1 - sc;
            if x2 >= x1 + sc && y2 <= y_stub {
                vec![(x1, y2)]
            } else {
                let x_stub = (x2 - sc).min(x1 - 24.0);
                vec![(x1, y_stub), (x_stub, y_stub), (x_stub, y2)]
            }
        }

        // ── Top → Right ───────────────────────────────────────────────────────
        (Side::Top, Side::Right) => {
            let y_stub = y1 - sc;
            if x1 >= x2 + sc && y2 <= y_stub {
                vec![(x1, y2)]
            } else {
                let x_stub = (x2 + sc).max(x1 + 24.0);
                vec![(x1, y_stub), (x_stub, y_stub), (x_stub, y2)]
            }
        }

        // ── Right → Top ───────────────────────────────────────────────────────
        (Side::Right, Side::Top) => {
            if x2 >= x1 + sc && y2 >= y1 + sc {
                vec![(x2, y1)]
            } else {
                let x_stub = x1 + sc;
                let y_stub = (y2 - sc).min(y1 - 24.0);
                vec![(x_stub, y1), (x_stub, y_stub), (x2, y_stub)]
            }
        }

        // ── Left → Top ────────────────────────────────────────────────────────
        (Side::Left, Side::Top) => {
            let x_stub = x1 - sc;
            if x1 >= x2 + sc && y2 >= y1 + sc {
                vec![(x2, y1)]
            } else {
                let y_stub = (y2 - sc).min(y1 - 24.0);
                vec![(x_stub, y1), (x_stub, y_stub), (x2, y_stub)]
            }
        }

        // ── Left → Bottom ─────────────────────────────────────────────────────
        (Side::Left, Side::Bottom) => {
            let x_stub = x1 - sc;
            if x1 >= x2 + sc && y1 >= y2 + sc {
                vec![(x2, y1)]
            } else {
                let y_stub = (y2 + sc).max(y1 + 24.0);
                vec![(x_stub, y1), (x_stub, y_stub), (x2, y_stub)]
            }
        }

        // ── Right → Bottom ────────────────────────────────────────────────────
        (Side::Right, Side::Bottom) => {
            if x1 >= x2 + sc && y1 >= y2 + sc {
                vec![(x2, y1)]
            } else {
                let x_stub = x1 + sc;
                let y_stub = (y2 + sc).max(y1 + 24.0);
                vec![(x_stub, y1), (x_stub, y_stub), (x2, y_stub)]
            }
        }

        // ── Same-side exit/entry  (e.g. Right→Right, Bottom→Bottom) ──────────
        _ => {
            let ym = (y1 + y2) / 2.0;
            vec![(x1, ym), (x2, ym)]
        }
    }
}

/// Check whether a polyline p1 → waypoints → p2 passes through any obstacle.
/// Returns the index of the first clipping waypoint-segment, or None.
fn first_clipping_segment(
    p1: (f64, f64),
    waypoints: &[(f64, f64)],
    p2: (f64, f64),
    obstacles: &[ObstacleRect],
) -> Option<usize> {
    let mut all_pts = Vec::with_capacity(waypoints.len() + 2);
    all_pts.push(p1);
    all_pts.extend_from_slice(waypoints);
    all_pts.push(p2);

    for (i, w) in all_pts.windows(2).enumerate() {
        let (ax, ay) = w[0];
        let (bx, by) = w[1];
        for obs in obstacles {
            if obs.clips_segment(ax, ay, bx, by, 6.0) {
                return Some(i);
            }
        }
    }
    None
}

/// Compute collision-free, isolated orthogonal waypoints between start point and end point.
///
/// Algorithm:
/// 1. Generate ideal waypoints for the face-pair (with guaranteed STUB_CLEARANCE stubs).
/// 2. Check each segment of the ideal path against the obstacle list.
/// 3. If any segment clips an obstacle, add bypass legs that route the offending segment
///    around the obstacle via open whitespace (choosing the less-blocked side).
/// 4. Repeat up to 3 times to handle cascading obstacles.
pub fn compute_edge_waypoints(
    p1: (f64, f64),
    src_side: Side,
    p2: (f64, f64),
    dst_side: Side,
    channel_y: f64,
    corridor_x: f64,
) -> Vec<(f64, f64)> {
    compute_edge_waypoints_with_obstacles(p1, src_side, p2, dst_side, channel_y, corridor_x, &[])
}

/// Full obstacle-aware variant of `compute_edge_waypoints`.
pub fn compute_edge_waypoints_with_obstacles(
    p1: (f64, f64),
    src_side: Side,
    p2: (f64, f64),
    dst_side: Side,
    channel_y: f64,
    corridor_x: f64,
    obstacles: &[ObstacleRect],
) -> Vec<(f64, f64)> {
    // Phase 1: ideal analytical waypoints
    let mut waypoints = ideal_waypoints_for_faces(p1, src_side, p2, dst_side, channel_y, corridor_x);

    if obstacles.is_empty() {
        return waypoints;
    }

    // Phase 2: iterative obstacle bypass (max 3 passes to avoid infinite loops)
    for _pass in 0..3 {
        let Some(clip_seg) = first_clipping_segment(p1, &waypoints, p2, obstacles) else {
            break; // No more clipping segments — done
        };

        // Reconstruct the full point list so we can address the clipping segment
        let mut all_pts = Vec::with_capacity(waypoints.len() + 2);
        all_pts.push(p1);
        all_pts.extend_from_slice(&waypoints);
        all_pts.push(p2);

        let (ax, ay) = all_pts[clip_seg];
        let (bx, by) = all_pts[clip_seg + 1];

        // Find the clipping obstacle (first one that clips this segment)
        let clipping_obs = obstacles.iter().find(|obs| obs.clips_segment(ax, ay, bx, by, 6.0));
        let Some(obs) = clipping_obs else { break; };

        // Compute two bypass options: go above/left of obstacle or below/right
        let bypass = if (ay - by).abs() < 0.5 {
            // Horizontal segment — route above or below
            let above_y = obs.y - STUB_CLEARANCE;
            let below_y = obs.y + obs.h + STUB_CLEARANCE;
            let mid_x = (ax + bx) / 2.0;

            // Choose side closer to channel_y hint, or less occupied
            let use_above = if channel_y > 0.0 {
                (above_y - channel_y).abs() < (below_y - channel_y).abs()
            } else {
                above_y.abs() < below_y.abs()
            };
            let det_y = if use_above { above_y } else { below_y };
            vec![(ax, det_y), (mid_x, det_y), (bx, det_y)]
        } else {
            // Vertical segment — route left or right
            let left_x = obs.x - STUB_CLEARANCE;
            let right_x = obs.x + obs.w + STUB_CLEARANCE;
            let mid_y = (ay + by) / 2.0;

            let use_left = if corridor_x > 0.0 {
                (left_x - corridor_x).abs() < (right_x - corridor_x).abs()
            } else {
                // Prefer whichever side is further from the centre of the diagram
                left_x < right_x
            };
            let det_x = if use_left { left_x } else { right_x };
            vec![(det_x, ay), (det_x, mid_y), (det_x, by)]
        };

        // Rebuild waypoints: keep segments before clip_seg, inject bypass, keep segments after
        // clip_seg+1 in all_pts corresponds to waypoint index clip_seg-1 (since all_pts[0]=p1)
        let mut new_wps: Vec<(f64, f64)> = Vec::new();
        // Points between p1 (index 0) and clip_seg (exclusive) that are waypoints:
        if clip_seg > 1 {
            new_wps.extend(all_pts[1..clip_seg].iter().copied());
        }

        // Inject bypass legs (skip first and last as they coincide with segment endpoints)
        new_wps.extend_from_slice(&bypass);
        // Points after clip_seg+1 up to but excluding p2 (last index):
        let tail_start = clip_seg + 2;
        let tail_end = all_pts.len() - 1;
        if tail_start < tail_end {
            new_wps.extend(all_pts[tail_start..tail_end].iter().copied());
        }
        waypoints = new_wps;
    }

    waypoints
}



/// Compute an orthogonal SVG path with smooth fillet corners (R = 8px) passing through all waypoints.
/// Returns `(path_d, label_center_x, label_center_y)`.
/// Places label at the midpoint of the longest segment away from bends and crossings.
fn build_orthogonal_svg_path(
    p1: (f64, f64),
    p2: (f64, f64),
    waypoints: &[(f64, f64)],
) -> (String, f64, f64) {
    let mut all_points = Vec::with_capacity(waypoints.len() + 2);
    all_points.push(p1);
    all_points.extend_from_slice(waypoints);
    all_points.push(p2);

    let n = all_points.len();
    if n <= 1 {
        return (String::new(), 0.0, 0.0);
    }
    if n == 2 {
        let (a, b) = (all_points[0], all_points[1]);
        return (
            format!("M {:.1} {:.1} L {:.1} {:.1}", a.0, a.1, b.0, b.1),
            (a.0 + b.0) / 2.0,
            (a.1 + b.1) / 2.0,
        );
    }

    // Identify longest segment for label placement away from corners
    let mut max_seg_len = -1.0_f64;
    let mut label_pos = ((all_points[0].0 + all_points[1].0) / 2.0, (all_points[0].1 + all_points[1].1) / 2.0);

    for i in 0..n - 1 {
        let dx = all_points[i + 1].0 - all_points[i].0;
        let dy = all_points[i + 1].1 - all_points[i].1;
        let seg_len = (dx * dx + dy * dy).sqrt();
        if seg_len > max_seg_len {
            max_seg_len = seg_len;
            label_pos = (
                (all_points[i].0 + all_points[i + 1].0) / 2.0,
                (all_points[i].1 + all_points[i + 1].1) / 2.0,
            );
        }
    }

    let mut d = format!("M {:.1} {:.1}", all_points[0].0, all_points[0].1);
    let mut current_pt = all_points[0];

    for i in 1..n - 1 {
        let prev = current_pt;
        let corner = all_points[i];
        let next = all_points[i + 1];

        let d1_x = corner.0 - prev.0;
        let d1_y = corner.1 - prev.1;
        let len1 = (d1_x * d1_x + d1_y * d1_y).sqrt();

        let d2_x = next.0 - corner.0;
        let d2_y = next.1 - corner.1;
        let len2 = (d2_x * d2_x + d2_y * d2_y).sqrt();

        let r = 8.0_f64.min(len1 / 2.0).min(len2 / 2.0);

        if r < 1.0 || len1 < 1.0 || len2 < 1.0 {
            d.push_str(&format!(" L {:.1} {:.1}", corner.0, corner.1));
            current_pt = corner;
        } else {
            let u1_x = d1_x / len1;
            let u1_y = d1_y / len1;
            let u2_x = d2_x / len2;
            let u2_y = d2_y / len2;

            let in_pt = (corner.0 - u1_x * r, corner.1 - u1_y * r);
            let out_pt = (corner.0 + u2_x * r, corner.1 + u2_y * r);

            d.push_str(&format!(
                " L {:.1} {:.1} Q {:.1} {:.1} {:.1} {:.1}",
                in_pt.0, in_pt.1, corner.0, corner.1, out_pt.0, out_pt.1
            ));
            current_pt = out_pt;
        }
    }

    let last = all_points[n - 1];
    d.push_str(&format!(" L {:.1} {:.1}", last.0, last.1));

    (d, label_pos.0, label_pos.1)
}

/// Compute an orthogonal SVG path with rounded fillet corners between two points.
/// Returns `(path_d, label_center_x, label_center_y)`.
/// Retained for backwards compatibility with tests and callers.
#[allow(dead_code)]
fn orthogonal_path(x1: f64, y1: f64, x2: f64, y2: f64, is_horizontal: bool) -> (String, f64, f64) {
    if is_horizontal {
        let waypoints = compute_edge_waypoints((x1, y1), Side::Right, (x2, y2), Side::Left, (y1 + y2) / 2.0, (x1 + x2) / 2.0);
        build_orthogonal_svg_path((x1, y1), (x2, y2), &waypoints)
    } else {
        let waypoints = compute_edge_waypoints((x1, y1), Side::Bottom, (x2, y2), Side::Top, (y1 + y2) / 2.0, (x1 + x2) / 2.0);
        build_orthogonal_svg_path((x1, y1), (x2, y2), &waypoints)
    }
}

// ---------------------------------------------------------------------------
// Sequence Diagram Renderers
// ---------------------------------------------------------------------------

fn render_sequence_drawio(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    seq: &crate::layout::SequenceLayoutInfo,
    theme: &str,
) -> Result<String> {
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
    let bg_color = if theme == "dark" { "#0f172a" } else { "#f8fafc" };
    model.push_attribute(("background", bg_color));
    model.push_attribute(("math", "1"));
    model.push_attribute(("shadow", "0"));
    w.write_event(Event::Start(model))?;

    // <root>
    w.write_event(Event::Start(BytesStart::new("root")))?;

    // Mandatory stub cells
    let mut cell0 = BytesStart::new("mxCell");
    cell0.push_attribute(("id", "0"));
    w.write_event(Event::Empty(cell0))?;

    let mut cell1 = BytesStart::new("mxCell");
    cell1.push_attribute(("id", "1"));
    cell1.push_attribute(("parent", "0"));
    w.write_event(Event::Empty(cell1))?;

    // Title / Description
    if let Some(title) = &compiled.title {
        let title_color = if theme == "dark" { "#f1f5f9" } else { "#0f172a" };
        let sub_color = if theme == "dark" { "#94a3b8" } else { "#64748b" };
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
        t_geo.push_attribute(("x", "24"));
        t_geo.push_attribute(("y", "12"));
        t_geo.push_attribute(("width", "500"));
        t_geo.push_attribute(("height", "40"));
        t_geo.push_attribute(("as", "geometry"));
        w.write_event(Event::Empty(t_geo))?;
        w.write_event(Event::End(BytesEnd::new("mxCell")))?;
    }

    let is_dark = theme == "dark";
    let lifeline_color = if is_dark { "#475569" } else { "#94a3b8" };
    let label_bg_color = if is_dark { "#1e293b" } else { "#ffffff" };
    let label_font_color = if is_dark { "#cbd5e1" } else { "#475569" };

    // 1. Participant header boxes
    for node_idx in compiled.graph.node_indices() {
        let node_data = &compiled.graph[node_idx];
        let nl = match layout.positions.get(&node_idx) {
            Some(p) => p,
            None => continue,
        };
        let style = style_for_type(&node_data.node_type, theme);
        let icon_key = node_data.icon.as_deref();
        let icon_html = icon_key
            .and_then(crate::icons::icon_as_data_uri)
            .map(|uri| format!("<img src=\"{uri}\" width=\"16\" height=\"16\" style=\"vertical-align:middle;margin-right:5px;display:inline-block;\"/>"))
            .unwrap_or_default();
        let html_value = format!("{icon_html}<b>{}</b>", node_data.label);

        let mut cell = BytesStart::new("mxCell");
        cell.push_attribute(("id", node_data.id.as_str()));
        cell.push_attribute(("value", html_value.as_str()));
        cell.push_attribute(("style", style.as_str()));
        cell.push_attribute(("vertex", "1"));
        cell.push_attribute(("parent", "1"));
        w.write_event(Event::Start(cell))?;

        let mut geo = BytesStart::new("mxGeometry");
        geo.push_attribute(("x", nl.x.round().to_string().as_str()));
        geo.push_attribute(("y", nl.y.round().to_string().as_str()));
        geo.push_attribute(("width", nl.width.round().to_string().as_str()));
        geo.push_attribute(("height", nl.height.round().to_string().as_str()));
        geo.push_attribute(("as", "geometry"));
        w.write_event(Event::Empty(geo))?;

        w.write_event(Event::End(BytesEnd::new("mxCell")))?;

        // 2. Vertical lifeline for this participant
        let lx = seq.lifeline_x.get(&node_idx).copied().unwrap_or(nl.x + nl.width / 2.0);
        let lifeline_style = format!(
            "edgeStyle=none;rounded=0;orthogonalLoop=1;jettySize=auto;html=1;dashed=1;dashPattern=6 6;strokeColor={lifeline_color};strokeWidth=1.5;endArrow=none;"
        );
        let mut line_cell = BytesStart::new("mxCell");
        let line_id = format!("lifeline_{}", node_data.id);
        line_cell.push_attribute(("id", line_id.as_str()));
        line_cell.push_attribute(("value", ""));
        line_cell.push_attribute(("style", lifeline_style.as_str()));
        line_cell.push_attribute(("edge", "1"));
        line_cell.push_attribute(("parent", "1"));
        w.write_event(Event::Start(line_cell))?;

        let mut line_geo = BytesStart::new("mxGeometry");
        line_geo.push_attribute(("relative", "1"));
        line_geo.push_attribute(("as", "geometry"));
        w.write_event(Event::Start(line_geo))?;

        let mut pt_src = BytesStart::new("mxPoint");
        pt_src.push_attribute(("x", lx.round().to_string().as_str()));
        pt_src.push_attribute(("y", seq.lifeline_top_y.round().to_string().as_str()));
        pt_src.push_attribute(("as", "sourcePoint"));
        w.write_event(Event::Empty(pt_src))?;

        let mut pt_dst = BytesStart::new("mxPoint");
        pt_dst.push_attribute(("x", lx.round().to_string().as_str()));
        pt_dst.push_attribute(("y", seq.lifeline_bottom_y.round().to_string().as_str()));
        pt_dst.push_attribute(("as", "targetPoint"));
        w.write_event(Event::Empty(pt_dst))?;

        w.write_event(Event::End(BytesEnd::new("mxGeometry")))?;
        w.write_event(Event::End(BytesEnd::new("mxCell")))?;
    }

    // 3. Chronological message arrows
    for (i, msg) in seq.messages.iter().enumerate() {
        let edge_data = &compiled.graph[msg.edge_idx];
        let src_id = &compiled.graph[msg.from_node].id;
        let dst_id = &compiled.graph[msg.to_node].id;
        let edge_id = format!("seq_msg_{i}_{src_id}_{dst_id}");
        let label = edge_data.label.as_deref().unwrap_or("");
        let from_x = seq.lifeline_x.get(&msg.from_node).copied().unwrap_or(0.0);
        let to_x = seq.lifeline_x.get(&msg.to_node).copied().unwrap_or(0.0);
        let y = msg.y;

        if msg.is_self_call {
            let self_style = format!(
                "edgeStyle=orthogonalEdgeStyle;rounded=1;orthogonalLoop=1;jettySize=auto;html=1;\
                 strokeColor=#475569;strokeWidth=1.5;endArrow=blockThin;endFill=1;\
                 labelBackgroundColor={label_bg_color};labelBorderColor=none;\
                 fontFamily=Inter,Helvetica,sans-serif;fontSize=11;fontColor={label_font_color};"
            );
            let mut cell = BytesStart::new("mxCell");
            cell.push_attribute(("id", edge_id.as_str()));
            cell.push_attribute(("value", label));
            cell.push_attribute(("style", self_style.as_str()));
            cell.push_attribute(("edge", "1"));
            cell.push_attribute(("parent", "1"));
            w.write_event(Event::Start(cell))?;

            let mut geo = BytesStart::new("mxGeometry");
            geo.push_attribute(("relative", "1"));
            geo.push_attribute(("as", "geometry"));
            w.write_event(Event::Start(geo))?;

            let mut pt_src = BytesStart::new("mxPoint");
            pt_src.push_attribute(("x", from_x.round().to_string().as_str()));
            pt_src.push_attribute(("y", (y - 8.0).round().to_string().as_str()));
            pt_src.push_attribute(("as", "sourcePoint"));
            w.write_event(Event::Empty(pt_src))?;

            let mut pt_dst = BytesStart::new("mxPoint");
            pt_dst.push_attribute(("x", from_x.round().to_string().as_str()));
            pt_dst.push_attribute(("y", (y + 16.0).round().to_string().as_str()));
            pt_dst.push_attribute(("as", "targetPoint"));
            w.write_event(Event::Empty(pt_dst))?;

            let mut pts_array = BytesStart::new("Array");
            pts_array.push_attribute(("as", "points"));
            w.write_event(Event::Start(pts_array))?;

            let mut p1 = BytesStart::new("mxPoint");
            p1.push_attribute(("x", (from_x + 36.0).round().to_string().as_str()));
            p1.push_attribute(("y", (y - 8.0).round().to_string().as_str()));
            w.write_event(Event::Empty(p1))?;

            let mut p2 = BytesStart::new("mxPoint");
            p2.push_attribute(("x", (from_x + 36.0).round().to_string().as_str()));
            p2.push_attribute(("y", (y + 16.0).round().to_string().as_str()));
            w.write_event(Event::Empty(p2))?;

            w.write_event(Event::End(BytesEnd::new("Array")))?;

            if !label.is_empty() {
                let mut pt_offset = BytesStart::new("mxPoint");
                pt_offset.push_attribute(("x", "20"));
                pt_offset.push_attribute(("y", "-4"));
                pt_offset.push_attribute(("as", "offset"));
                w.write_event(Event::Empty(pt_offset))?;
            }

            w.write_event(Event::End(BytesEnd::new("mxGeometry")))?;
            w.write_event(Event::End(BytesEnd::new("mxCell")))?;
        } else {
            let is_reply = msg.is_reply || edge_data.edge_style.as_deref() == Some("reply");
            let is_async = edge_data.edge_style.as_deref() == Some("async");

            let edge_style = if is_reply {
                format!(
                    "edgeStyle=none;rounded=0;orthogonalLoop=1;jettySize=auto;html=1;\
                     dashed=1;dashPattern=6 3;strokeColor={lifeline_color};strokeWidth=1.5;\
                     endArrow=open;endFill=0;endSize=7;\
                     labelBackgroundColor={label_bg_color};labelBorderColor=none;\
                     fontFamily=Inter,Helvetica,sans-serif;fontSize=11;fontColor={label_font_color};"
                )
            } else if is_async {
                format!(
                    "edgeStyle=none;rounded=0;orthogonalLoop=1;jettySize=auto;html=1;\
                     dashed=1;dashPattern=8 4;strokeColor=#d97706;strokeWidth=1.5;\
                     endArrow=open;endFill=0;endSize=7;\
                     labelBackgroundColor={label_bg_color};labelBorderColor=none;\
                     fontFamily=Inter,Helvetica,sans-serif;fontSize=11;fontColor={label_font_color};"
                )
            } else {
                let stroke_col = if is_dark { "#cbd5e1" } else { "#1e293b" };
                format!(
                    "edgeStyle=none;rounded=0;orthogonalLoop=1;jettySize=auto;html=1;\
                     strokeColor={stroke_col};strokeWidth=1.5;\
                     endArrow=blockThin;endFill=1;endSize=6;\
                     labelBackgroundColor={label_bg_color};labelBorderColor=none;\
                     fontFamily=Inter,Helvetica,sans-serif;fontSize=11;fontColor={label_font_color};"
                )
            };

            let mut cell = BytesStart::new("mxCell");
            cell.push_attribute(("id", edge_id.as_str()));
            cell.push_attribute(("value", label));
            cell.push_attribute(("style", edge_style.as_str()));
            cell.push_attribute(("edge", "1"));
            cell.push_attribute(("parent", "1"));
            w.write_event(Event::Start(cell))?;

            let mut geo = BytesStart::new("mxGeometry");
            geo.push_attribute(("relative", "1"));
            geo.push_attribute(("as", "geometry"));
            w.write_event(Event::Start(geo))?;

            let mut pt_src = BytesStart::new("mxPoint");
            pt_src.push_attribute(("x", from_x.round().to_string().as_str()));
            pt_src.push_attribute(("y", y.round().to_string().as_str()));
            pt_src.push_attribute(("as", "sourcePoint"));
            w.write_event(Event::Empty(pt_src))?;

            let mut pt_dst = BytesStart::new("mxPoint");
            pt_dst.push_attribute(("x", to_x.round().to_string().as_str()));
            pt_dst.push_attribute(("y", y.round().to_string().as_str()));
            pt_dst.push_attribute(("as", "targetPoint"));
            w.write_event(Event::Empty(pt_dst))?;

            if !label.is_empty() {
                let mut pt_offset = BytesStart::new("mxPoint");
                pt_offset.push_attribute(("y", "-10"));
                pt_offset.push_attribute(("as", "offset"));
                w.write_event(Event::Empty(pt_offset))?;
            }

            w.write_event(Event::End(BytesEnd::new("mxGeometry")))?;
            w.write_event(Event::End(BytesEnd::new("mxCell")))?;
        }
    }

    // </root></mxGraphModel></diagram></mxfile>
    w.write_event(Event::End(BytesEnd::new("root")))?;
    w.write_event(Event::End(BytesEnd::new("mxGraphModel")))?;
    w.write_event(Event::End(BytesEnd::new("diagram")))?;
    w.write_event(Event::End(BytesEnd::new("mxfile")))?;

    String::from_utf8(buf).map_err(|e| anyhow::anyhow!("invalid UTF-8 in drawio output: {e}"))
}

fn render_sequence_svg(
    compiled: &CompiledGraph,
    layout: &LayoutResult,
    seq: &crate::layout::SequenceLayoutInfo,
    theme: &str,
) -> Result<String> {
    let is_dark = theme == "dark";
    let bg_color = if is_dark { "#0f172a" } else { "#f8fafc" };
    let card_fill = if is_dark { "#1e293b" } else { "#ffffff" };
    let lifeline_color = if is_dark { "#475569" } else { "#94a3b8" };
    let text_color = if is_dark { "#f1f5f9" } else { "#0f172a" };
    let sub_color = if is_dark { "#94a3b8" } else { "#64748b" };

    let max_x = layout
        .positions
        .values()
        .map(|nl| nl.x + nl.width)
        .fold(0.0_f64, f64::max);
    let canvas_w = (max_x + 60.0).max(600.0);
    let canvas_h = (seq.lifeline_bottom_y + 40.0).max(300.0);

    let mut buf = Vec::with_capacity(8192);
    let mut w = Writer::new_with_indent(Cursor::new(&mut buf), b' ', 2);

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

    // Background rect
    let mut bg = BytesStart::new("rect");
    bg.push_attribute(("width", "100%"));
    bg.push_attribute(("height", "100%"));
    bg.push_attribute(("fill", bg_color));
    w.write_event(Event::Empty(bg))?;

    // Defs: drop shadow and markers
    w.write_event(Event::Start(BytesStart::new("defs")))?;

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
        ("seq-arrow-sync", if is_dark { "#cbd5e1" } else { "#1e293b" }, true),
        ("seq-arrow-reply", lifeline_color, false),
        ("seq-arrow-async", "#d97706", false),
    ];
    for (id, col, filled) in markers {
        let mut marker = BytesStart::new("marker");
        marker.push_attribute(("id", id));
        marker.push_attribute(("viewBox", "0 0 10 10"));
        marker.push_attribute(("refX", "8"));
        marker.push_attribute(("refY", "5"));
        marker.push_attribute(("markerWidth", "6"));
        marker.push_attribute(("markerHeight", "6"));
        marker.push_attribute(("orient", "auto"));
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

    w.write_event(Event::End(BytesEnd::new("defs")))?;

    // Title / Description
    if let Some(title) = &compiled.title {
        let mut t_elem = BytesStart::new("text");
        t_elem.push_attribute(("x", "24"));
        t_elem.push_attribute(("y", "28"));
        t_elem.push_attribute(("font-family", "Inter, Helvetica, sans-serif"));
        t_elem.push_attribute(("font-size", "15"));
        t_elem.push_attribute(("font-weight", "bold"));
        t_elem.push_attribute(("fill", text_color));
        w.write_event(Event::Start(t_elem))?;
        w.write_event(Event::Text(BytesText::new(title)))?;
        w.write_event(Event::End(BytesEnd::new("text")))?;

        if let Some(desc) = &compiled.description {
            let mut d_elem = BytesStart::new("text");
            d_elem.push_attribute(("x", "24"));
            d_elem.push_attribute(("y", "44"));
            d_elem.push_attribute(("font-family", "Inter, Helvetica, sans-serif"));
            d_elem.push_attribute(("font-size", "11"));
            d_elem.push_attribute(("fill", sub_color));
            w.write_event(Event::Start(d_elem))?;
            w.write_event(Event::Text(BytesText::new(desc)))?;
            w.write_event(Event::End(BytesEnd::new("text")))?;
        }
    }

    // 1. Draw lifelines (vertical dashed lines)
    for &lx in seq.lifeline_x.values() {
        let mut line = BytesStart::new("line");
        line.push_attribute(("x1", lx.round().to_string().as_str()));
        line.push_attribute(("y1", seq.lifeline_top_y.round().to_string().as_str()));
        line.push_attribute(("x2", lx.round().to_string().as_str()));
        line.push_attribute(("y2", seq.lifeline_bottom_y.round().to_string().as_str()));
        line.push_attribute(("stroke", lifeline_color));
        line.push_attribute(("stroke-width", "1.5"));
        line.push_attribute(("stroke-dasharray", "6 6"));
        w.write_event(Event::Empty(line))?;
    }

    // 2. Draw message arrows
    for msg in &seq.messages {
        let edge_data = &compiled.graph[msg.edge_idx];
        let from_x = seq.lifeline_x.get(&msg.from_node).copied().unwrap_or(0.0);
        let to_x = seq.lifeline_x.get(&msg.to_node).copied().unwrap_or(0.0);
        let y = msg.y;
        let is_reply = msg.is_reply || edge_data.edge_style.as_deref() == Some("reply");
        let is_async = edge_data.edge_style.as_deref() == Some("async");

        let (stroke, marker, dash) = if is_reply {
            (lifeline_color, "seq-arrow-reply", Some("6 3"))
        } else if is_async {
            ("#d97706", "seq-arrow-async", Some("8 4"))
        } else {
            (if is_dark { "#cbd5e1" } else { "#1e293b" }, "seq-arrow-sync", None)
        };

        if msg.is_self_call {
            let loop_d = format!(
                "M {from_x:.1} {y_top:.1} H {x_out:.1} V {y_bot:.1} H {from_x:.1}",
                y_top = y - 8.0,
                x_out = from_x + 36.0,
                y_bot = y + 16.0,
            );
            let mut path = BytesStart::new("path");
            path.push_attribute(("d", loop_d.as_str()));
            path.push_attribute(("fill", "none"));
            path.push_attribute(("stroke", stroke));
            path.push_attribute(("stroke-width", "1.5"));
            path.push_attribute(("marker-end", format!("url(#{marker})").as_str()));
            w.write_event(Event::Empty(path))?;

            if let Some(label) = &edge_data.label {
                let mut text = BytesStart::new("text");
                text.push_attribute(("x", (from_x + 42.0).round().to_string().as_str()));
                text.push_attribute(("y", (y + 6.0).round().to_string().as_str()));
                text.push_attribute(("font-family", "Inter, Helvetica, sans-serif"));
                text.push_attribute(("font-size", "10"));
                text.push_attribute(("fill", sub_color));
                w.write_event(Event::Start(text))?;
                w.write_event(Event::Text(BytesText::new(label)))?;
                w.write_event(Event::End(BytesEnd::new("text")))?;
            }
        } else {
            let mut line = BytesStart::new("line");
            line.push_attribute(("x1", from_x.round().to_string().as_str()));
            line.push_attribute(("y1", y.round().to_string().as_str()));
            line.push_attribute(("x2", to_x.round().to_string().as_str()));
            line.push_attribute(("y2", y.round().to_string().as_str()));
            line.push_attribute(("stroke", stroke));
            line.push_attribute(("stroke-width", "1.5"));
            if let Some(d) = dash {
                line.push_attribute(("stroke-dasharray", d));
            }
            line.push_attribute(("marker-end", format!("url(#{marker})").as_str()));
            w.write_event(Event::Empty(line))?;

            if let Some(label) = &edge_data.label {
                let mid_x = (from_x + to_x) / 2.0;
                let char_len = label.chars().count();
                let pill_w = (char_len as f64 * 6.2 + 10.0).max(20.0);

                let mut pill = BytesStart::new("rect");
                pill.push_attribute(("x", (mid_x - pill_w / 2.0).round().to_string().as_str()));
                pill.push_attribute(("y", (y - 16.0).round().to_string().as_str()));
                pill.push_attribute(("width", pill_w.round().to_string().as_str()));
                pill.push_attribute(("height", "14"));
                pill.push_attribute(("rx", "3"));
                pill.push_attribute(("fill", bg_color));
                w.write_event(Event::Empty(pill))?;

                let mut text = BytesStart::new("text");
                text.push_attribute(("x", mid_x.round().to_string().as_str()));
                text.push_attribute(("y", (y - 5.0).round().to_string().as_str()));
                text.push_attribute(("text-anchor", "middle"));
                text.push_attribute(("font-family", "Inter, Helvetica, sans-serif"));
                text.push_attribute(("font-size", "10"));
                text.push_attribute(("font-weight", "500"));
                text.push_attribute(("fill", if is_dark { "#cbd5e1" } else { "#475569" }));
                w.write_event(Event::Start(text))?;
                w.write_event(Event::Text(BytesText::new(label)))?;
                w.write_event(Event::End(BytesEnd::new("text")))?;
            }
        }
    }

    // 3. Draw participant cards (on top of lifelines)
    for node_idx in compiled.graph.node_indices() {
        let node_data = &compiled.graph[node_idx];
        let nl = match layout.positions.get(&node_idx) {
            Some(p) => p,
            None => continue,
        };
        let stroke_color = stroke_for_type(&node_data.node_type, theme);

        let mut rect = BytesStart::new("rect");
        rect.push_attribute(("x", nl.x.round().to_string().as_str()));
        rect.push_attribute(("y", nl.y.round().to_string().as_str()));
        rect.push_attribute(("width", nl.width.round().to_string().as_str()));
        rect.push_attribute(("height", nl.height.round().to_string().as_str()));
        rect.push_attribute(("rx", "8"));
        rect.push_attribute(("fill", card_fill));
        rect.push_attribute(("stroke", stroke_color));
        rect.push_attribute(("stroke-width", "1.5"));
        rect.push_attribute(("filter", "url(#card-shadow)"));
        w.write_event(Event::Empty(rect))?;

        // Optional icon
        if let Some(icon_key) = node_data.icon.as_deref() {
            if let Some(icon_markup) = crate::icons::render_icon_svg(icon_key, nl.x + 10.0, nl.y + 12.0, 18.0) {
                w.write_event(Event::Text(BytesText::from_escaped(icon_markup)))?;
            }
        }

        let cx = if node_data.icon.is_some() { nl.x + nl.width / 2.0 + 8.0 } else { nl.x + nl.width / 2.0 };
        let mut text = BytesStart::new("text");
        text.push_attribute(("x", cx.round().to_string().as_str()));
        text.push_attribute(("y", (nl.y + 26.0).round().to_string().as_str()));
        text.push_attribute(("text-anchor", "middle"));
        text.push_attribute(("font-family", "Inter, Helvetica, sans-serif"));
        text.push_attribute(("font-size", "12"));
        text.push_attribute(("font-weight", "bold"));
        text.push_attribute(("fill", text_color));
        w.write_event(Event::Start(text))?;
        w.write_event(Event::Text(BytesText::new(&node_data.label)))?;
        w.write_event(Event::End(BytesEnd::new("text")))?;
    }

    w.write_event(Event::End(BytesEnd::new("svg")))?;
    String::from_utf8(buf).map_err(|e| anyhow::anyhow!("invalid UTF-8 in svg output: {e}"))
}

// ---------------------------------------------------------------------------
// draw.io XML renderer
// ---------------------------------------------------------------------------

/// Render the compiled, laid-out graph to an uncompressed draw.io XML string.
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
    theme: &str,
) -> Result<String> {
    if let Some(seq) = &layout.sequence_info {
        return render_sequence_drawio(compiled, layout, seq, theme);
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
    let bg_color = if theme == "dark" { "#0f172a" } else { "#f8fafc" };
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
        let title_color = if theme == "dark" { "#f1f5f9" } else { "#0f172a" };
        let sub_color = if theme == "dark" { "#94a3b8" } else { "#64748b" };
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
        t_geo.push_attribute(("x", "24"));
        t_geo.push_attribute(("y", "12"));
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

        let pad_h = crate::layout::GROUP_PAD_H;
        let pad_top = crate::layout::GROUP_PAD_TOP;
        let pad_bot = crate::layout::GROUP_PAD_BOT;

        let gx = (min_x - pad_h).max(10.0);
        let gy = (min_y - pad_top).max(10.0);
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
             fontStyle=1;fontSize=12;fontColor={color};"
        );

        let mut g_cell = BytesStart::new("mxCell");
        g_cell.push_attribute(("id", group.id.as_str()));
        g_cell.push_attribute(("value", group.label.as_str()));
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
            style.push_str(";spacingTop=0;spacingBottom=0;spacingLeft=0;spacingRight=0;overflow=hidden;");
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
            )
        };

        let (parent_id, rel_x, rel_y) = if let Some(gid) = node_to_group_id.get(&node_data.id) {
            let (gx, gy) = group_origins[gid];
            (gid.as_str(), nl.x - gx, nl.y - gy)
        } else {
            ("1", nl.x, nl.y)
        };

        let mut cell = BytesStart::new("mxCell");
        cell.push_attribute(("id", node_data.id.as_str()));
        cell.push_attribute(("value", html_value.as_str()));
        cell.push_attribute(("style", style.as_str()));
        cell.push_attribute(("vertex", "1"));
        cell.push_attribute(("parent", parent_id));
        if !tooltip.is_empty() {
            cell.push_attribute(("tooltip", tooltip));
        }
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
    }

    // --- Edge cells ---------------------------------------------------------
    let edge_plans = plan_all_edge_routes(compiled, layout);

    let is_dark = theme == "dark";
    let default_edge_color = if is_dark { "#94a3b8" } else { "#64748b" };
    let label_bg_color = if is_dark { "#1e293b" } else { "#ffffff" };
    let label_font_color = if is_dark { "#cbd5e1" } else { "#475569" };

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
        let port_frac = plan.map(|p| p.exit_port).unwrap_or(0.5);
        let entry_port_frac = plan.map(|p| p.entry_port).unwrap_or(0.5);
        let src_side = plan.map(|p| p.src_side).unwrap_or(Side::Bottom);
        let dst_side = plan.map(|p| p.dst_side).unwrap_or(Side::Top);
        let waypoints = plan.map(|p| p.waypoints.as_slice()).unwrap_or(&[]);

        let exit_attr = match src_side {
            Side::Bottom => format!("exitX={port_frac:.1};exitY=1.0;exitDx=0;exitDy=0;"),
            Side::Top => format!("exitX={port_frac:.1};exitY=0.0;exitDx=0;exitDy=0;"),
            Side::Left => format!("exitX=0.0;exitY={port_frac:.1};exitDx=0;exitDy=0;"),
            Side::Right => format!("exitX=1.0;exitY={port_frac:.1};exitDx=0;exitDy=0;"),
        };

        let entry_attr = match dst_side {
            Side::Top => format!("entryX={entry_port_frac:.1};entryY=0.0;entryDx=0;entryDy=0;"),
            Side::Bottom => format!("entryX={entry_port_frac:.1};entryY=1.0;entryDx=0;entryDy=0;"),
            Side::Left => format!("entryX=0.0;entryY={entry_port_frac:.1};entryDx=0;entryDy=0;"),
            Side::Right => format!("entryX=1.0;entryY={entry_port_frac:.1};entryDx=0;entryDy=0;"),
        };

        let bi_style = format!("strokeColor={default_edge_color};strokeWidth=1.5;startArrow=blockThin;startFill=1;endArrow=blockThin;endFill=1;");
        let default_style = format!("strokeColor={default_edge_color};strokeWidth=1.5;endArrow=blockThin;endFill=1;");

        let base_custom_style = match edge_data.edge_style.as_deref() {
            Some("async") => {
                "dashed=1;dashPattern=8 4;strokeColor=#d97706;strokeWidth=1.5;endArrow=open;endFill=0;"
            }
            Some("error") | Some("fallback") => {
                "dashed=1;dashPattern=6 3;strokeColor=#ef4444;strokeWidth=1.5;endArrow=blockThin;endFill=0;"
            }
            Some("data") | Some("stream") => {
                "strokeColor=#6366f1;strokeWidth=2;endArrow=blockThin;endFill=1;"
            }
            Some("bi") | Some("bidirectional") => &bi_style,
            // ER Diagram Relationships
            Some("one_to_many") => {
                "strokeColor=#0284c7;strokeWidth=1.5;startArrow=ERone;startFill=0;endArrow=ERmany;endFill=0;"
            }
            Some("many_to_many") => {
                "strokeColor=#0284c7;strokeWidth=1.5;startArrow=ERmany;startFill=0;endArrow=ERmany;endFill=0;"
            }
            Some("one_to_one") => {
                "strokeColor=#0284c7;strokeWidth=1.5;startArrow=ERone;startFill=0;endArrow=ERone;endFill=0;"
            }
            Some("zero_to_many") => {
                "strokeColor=#0284c7;strokeWidth=1.5;startArrow=ERzeroToOne;startFill=0;endArrow=ERmany;endFill=0;"
            }
            // UML Class Diagram Relationships
            Some("inheritance") => {
                "strokeColor=#6366f1;strokeWidth=1.5;endArrow=block;endFill=0;endSize=10;"
            }
            Some("realization") => {
                "dashed=1;dashPattern=6 3;strokeColor=#6366f1;strokeWidth=1.5;endArrow=block;endFill=0;endSize=10;"
            }
            Some("composition") => {
                "strokeColor=#0f172a;strokeWidth=1.5;startArrow=diamond;startFill=1;startSize=12;endArrow=none;"
            }
            Some("aggregation") => {
                "strokeColor=#0f172a;strokeWidth=1.5;startArrow=diamond;startFill=0;startSize=12;endArrow=none;"
            }
            Some("dependency") => {
                "dashed=1;dashPattern=6 3;strokeColor=#64748b;strokeWidth=1.5;endArrow=open;endFill=0;"
            }
            _ => &default_style,
        };

        let mut custom_style = base_custom_style.to_string();
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

        let edge_style = format!(
            "edgeStyle=orthogonalEdgeStyle;\
             rounded=1;orthogonalLoop=1;jettySize=auto;html=1;\
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
                let mut pt = BytesStart::new("mxPoint");
                pt.push_attribute(("y", "-10"));
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

// ---------------------------------------------------------------------------
// SVG renderer
// ---------------------------------------------------------------------------

/// Render the compiled, laid-out graph to an SVG string.
///
/// Uses hand-rolled SVG primitives to avoid bringing in a heavy dependency
/// while still producing well-formed, human-readable output.
///
/// # Errors
///
/// Returns an error if the underlying XML writer fails.
pub fn render_svg(compiled: &CompiledGraph, layout: &LayoutResult, theme: &str) -> Result<String> {
    if let Some(seq) = &layout.sequence_info {
        return render_sequence_svg(compiled, layout, seq, theme);
    }

    // Compute canvas bounds including nodes and group containers.
    let mut max_x = 0.0_f64;
    let mut max_y = 0.0_f64;

    for nl in layout.positions.values() {
        max_x = max_x.max(nl.x + nl.width);
        max_y = max_y.max(nl.y + nl.height);
    }

    for group in &compiled.groups {
        for node_id in &group.nodes {
            if let Some(&node_idx) = compiled.node_map.get(node_id) {
                if let Some(nl) = layout.positions.get(&node_idx) {
                    max_x = max_x.max(nl.x + nl.width + crate::layout::GROUP_PAD_H);
                    max_y = max_y.max(nl.y + nl.height + crate::layout::GROUP_PAD_BOT);
                }
            }
        }
    }

    let margin = 36.0_f64;
    let canvas_w = (max_x + margin).max(120.0);
    let canvas_h = (max_y + margin).max(100.0);

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
    let bg_color = if is_dark { "#0f172a" } else { "#f8fafc" };

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
        t_elem.push_attribute(("x", "24"));
        t_elem.push_attribute(("y", "28"));
        t_elem.push_attribute(("font-family", "Inter, Helvetica, sans-serif"));
        t_elem.push_attribute(("font-size", "15"));
        t_elem.push_attribute(("font-weight", "bold"));
        t_elem.push_attribute(("fill", title_color));
        w.write_event(Event::Start(t_elem))?;
        w.write_event(Event::Text(BytesText::new(title)))?;
        w.write_event(Event::End(BytesEnd::new("text")))?;

        if let Some(desc) = &compiled.description {
            let mut d_elem = BytesStart::new("text");
            d_elem.push_attribute(("x", "24"));
            d_elem.push_attribute(("y", "44"));
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

        let pad_h = crate::layout::GROUP_PAD_H;
        let pad_top = crate::layout::GROUP_PAD_TOP;
        let pad_bot = crate::layout::GROUP_PAD_BOT;

        let gx = (min_x - pad_h).max(10.0);
        let gy = (min_y - pad_top).max(10.0);
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

        let mut text = BytesStart::new("text");
        text.push_attribute(("x", (gx + 14.0).round().to_string().as_str()));
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
    let edge_plans = plan_all_edge_routes(compiled, layout);
    let default_edge = if is_dark { "#94a3b8" } else { "#64748b" };

    struct SvgEdgeLabel {
        lx: f64,
        ly: f64,
        text: String,
    }
    let mut pending_labels: Vec<SvgEdgeLabel> = Vec::new();

    // Pass 1: Render all edge paths
    for edge_idx in compiled.graph.edge_indices() {
        let (src_idx, dst_idx) = compiled.graph.edge_endpoints(edge_idx).unwrap();
        let edge_data = &compiled.graph[edge_idx];

        let (s_idx, d_idx) = if edge_data.reversed {
            (dst_idx, src_idx)
        } else {
            (src_idx, dst_idx)
        };

        let (Some(src_nl), Some(dst_nl)) = (
            layout.positions.get(&s_idx),
            layout.positions.get(&d_idx),
        ) else {
            continue;
        };

        let plan = edge_plans.get(&edge_idx);
        let src_side = plan.map(|p| p.src_side).unwrap_or(Side::Bottom);
        let dst_side = plan.map(|p| p.dst_side).unwrap_or(Side::Top);
        let exit_port = plan.map(|p| p.exit_port).unwrap_or(0.5);
        let entry_port = plan.map(|p| p.entry_port).unwrap_or(0.5);
        let channel_y = plan.map(|p| p.channel_y).unwrap_or((src_nl.y + dst_nl.y) / 2.0);

        let (x1, y1) = match src_side {
            Side::Bottom => (src_nl.x + src_nl.width * exit_port, src_nl.y + src_nl.height),
            Side::Top => (src_nl.x + src_nl.width * exit_port, src_nl.y),
            Side::Left => (src_nl.x, src_nl.y + src_nl.height * exit_port),
            Side::Right => (src_nl.x + src_nl.width, src_nl.y + src_nl.height * exit_port),
        };

        let (x2, y2) = match dst_side {
            Side::Top => (dst_nl.x + dst_nl.width * entry_port, dst_nl.y),
            Side::Bottom => (dst_nl.x + dst_nl.width * entry_port, dst_nl.y + dst_nl.height),
            Side::Left => (dst_nl.x, dst_nl.y + dst_nl.height * entry_port),
            Side::Right => (dst_nl.x + dst_nl.width, dst_nl.y + dst_nl.height * entry_port),
        };

        let is_bi = matches!(edge_data.edge_style.as_deref(), Some("bi") | Some("bidirectional"));
        let mut marker_start: Option<&str> = None;
        let mut marker_end: Option<&str> = Some(if is_dark { "arrow-dark" } else { "arrow-slate" });

        let (base_stroke, base_w, base_dash) = match edge_data.edge_style.as_deref() {
            Some("async") => {
                marker_end = Some("arrow-amber");
                ("#d97706", "1.5", Some("8 4"))
            }
            Some("error") | Some("fallback") => {
                marker_end = Some("arrow-red");
                ("#ef4444", "1.5", Some("6 3"))
            }
            Some("data") | Some("stream") => {
                marker_end = Some("arrow-indigo");
                ("#6366f1", "2.0", None)
            }
            Some("one_to_many") => {
                marker_start = Some("marker-er-one");
                marker_end = Some("marker-er-many");
                ("#0284c7", "1.5", None)
            }
            Some("many_to_many") => {
                marker_start = Some("marker-er-many");
                marker_end = Some("marker-er-many");
                ("#0284c7", "1.5", None)
            }
            Some("one_to_one") => {
                marker_start = Some("marker-er-one");
                marker_end = Some("marker-er-one");
                ("#0284c7", "1.5", None)
            }
            Some("zero_to_many") => {
                marker_start = Some("marker-er-one");
                marker_end = Some("marker-er-many");
                ("#0284c7", "1.5", None)
            }
            Some("inheritance") => {
                marker_end = Some("marker-uml-triangle");
                ("#6366f1", "1.5", None)
            }
            Some("realization") => {
                marker_end = Some("marker-uml-triangle");
                ("#6366f1", "1.5", Some("6 3"))
            }
            Some("composition") => {
                marker_start = Some("marker-uml-diamond-fill");
                marker_end = None;
                (if is_dark { "#cbd5e1" } else { "#0f172a" }, "1.5", None)
            }
            Some("aggregation") => {
                marker_start = Some("marker-uml-diamond-hollow");
                marker_end = None;
                (if is_dark { "#cbd5e1" } else { "#0f172a" }, "1.5", None)
            }
            Some("dependency") => {
                marker_end = Some(if is_dark { "arrow-dark" } else { "arrow-slate" });
                ("#64748b", "1.5", Some("6 3"))
            }
            _ => {
                if is_bi {
                    marker_start = Some(if is_dark { "arrow-dark" } else { "arrow-slate" });
                }
                (default_edge, "1.5", None)
            }
        };

        // Custom formatting overrides
        let stroke = edge_data.color.as_deref().unwrap_or(base_stroke);
        let stroke_w_buf = edge_data.width.map(|w| format!("{w:.1}")).unwrap_or_else(|| base_w.to_string());
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

        let corridor_x = plan.map(|p| p.corridor_x).unwrap_or(0.0);
        let default_wps = compute_edge_waypoints((x1, y1), src_side, (x2, y2), dst_side, channel_y, corridor_x);
        let waypoints = plan.map(|p| p.waypoints.as_slice()).unwrap_or(&default_wps);
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

    // Pass 2: Render all edge labels on top of all paths to guarantee zero line collisions
    for el in pending_labels {
        let char_count = el.text.chars().count();
        let pill_w = (char_count as f64 * 6.5 + 8.0).max(20.0);
        let pill_h = 14.0;
        let pill_x = el.lx - pill_w / 2.0;
        let pill_y = el.ly - pill_h / 2.0;

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
        text.push_attribute(("y", format!("{:.1}", el.ly + 3.5).as_str()));
        text.push_attribute(("text-anchor", "middle"));
        text.push_attribute(("font-family", "Inter, Helvetica, sans-serif"));
        text.push_attribute(("font-size", "10"));
        text.push_attribute(("font-weight", "500"));
        text.push_attribute(("fill", if is_dark { "#94a3b8" } else { "#475569" }));
        w.write_event(Event::Start(text))?;
        w.write_event(Event::Text(BytesText::new(&el.text)))?;
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
        let stroke_color = stroke_for_type(&node_data.node_type, theme);

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
        let is_table_cylinder = !is_class && (matches!(
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
                let field_clean = crate::layout::strip_markdown_tokens(field);
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
                    right_text.push_attribute(("x", (nl.x + nl.width - 12.0).round().to_string().as_str()));
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
            let rh = (nl.height * 0.14).clamp(8.0, 14.0);
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
                if let Some(badge_markup) = crate::icons::render_icon_badge_svg(icon_key, nl.x, nl.y, is_dark) {
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
                    let field_clean = crate::layout::strip_markdown_tokens(field);
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
                        right_text.push_attribute(("x", (nl.x + nl.width - 12.0).round().to_string().as_str()));
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
                if let Some(badge_markup) = crate::icons::render_icon_badge_svg(icon_key, nl.x, nl.y, is_dark) {
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
            let rh = (nl.height * 0.14).clamp(8.0, 14.0);
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
                    tspan.push_attribute(("fill", if pl.is_subtitle { sub_color } else { title_color }));
                } else {
                    tspan.push_attribute(("font-size", if pl.is_subtitle { "10" } else { "12" }));
                    tspan.push_attribute(("fill", if pl.is_subtitle { sub_color } else { title_color }));
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::build_graph;
    use crate::layout::{LayoutConfig, compute_layout};
    use crate::schema::{DiagramPayload, EdgeDef, NodeDef};

    fn one_node_payload() -> DiagramPayload {
        DiagramPayload {
            diagram_type: "flowchart".to_owned(),
            nodes: vec![NodeDef {
                id: "n1".to_owned(),
                label: "API Gateway".to_owned(),
                node_type: "proxy".to_owned(),
                metadata: Some("Routes all traffic".to_owned()),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn two_node_payload() -> DiagramPayload {
        DiagramPayload {
            diagram_type: "flowchart".to_owned(),
            nodes: vec![
                NodeDef {
                    id: "n1".to_owned(),
                    label: "API Gateway".to_owned(),
                    node_type: "proxy".to_owned(),
                    ..Default::default()
                },
                NodeDef {
                    id: "n2".to_owned(),
                    label: "User DB".to_owned(),
                    node_type: "database".to_owned(),
                    ..Default::default()
                },
            ],
            edges: vec![EdgeDef {
                from: "n1".to_owned(),
                to: "n2".to_owned(),
                label: Some("queries".to_owned()),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn test_drawio_has_mandatory_root_cells() {
        let payload = one_node_payload();
        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let xml = render_drawio(&compiled, &layout, "standard").unwrap();

        assert!(xml.contains("id=\"0\""), "must contain cell with id=\"0\"");
        assert!(
            xml.contains("id=\"1\" parent=\"0\""),
            "must contain cell with id=\"1\" parent=\"0\""
        );
    }

    #[test]
    fn test_drawio_node_has_vertex_attribute() {
        let payload = one_node_payload();
        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let xml = render_drawio(&compiled, &layout, "standard").unwrap();
        assert!(
            xml.contains("vertex=\"1\""),
            "nodes must carry vertex=\"1\""
        );
    }

    #[test]
    fn test_drawio_edge_has_edge_attribute() {
        let payload = two_node_payload();
        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let xml = render_drawio(&compiled, &layout, "standard").unwrap();
        assert!(xml.contains("edge=\"1\""), "edges must carry edge=\"1\"");
    }

    #[test]
    fn test_drawio_no_vertex_on_edges() {
        let payload = two_node_payload();
        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let xml = render_drawio(&compiled, &layout, "standard").unwrap();
        // edge cells must not have vertex="1" on the same cell — verify by
        // checking that no single mxCell element has both attributes.
        // A simple heuristic: count occurrences.
        let edge_count = xml.matches("edge=\"1\"").count();
        let vertex_count = xml.matches("vertex=\"1\"").count();
        assert_eq!(edge_count, 1, "expected exactly 1 edge cell");
        assert_eq!(
            vertex_count, 2,
            "expected exactly 2 vertex cells (mandatory + node)"
        );
    }

    #[test]
    fn test_style_mapping_database() {
        let style = style_for_type("database", "standard");
        assert!(
            style.contains("cylinder3"),
            "database style must reference 'cylinder3'"
        );
    }

    #[test]
    fn test_style_mapping_default() {
        let style = style_for_type("unknown_widget", "standard");
        assert!(!style.is_empty(), "default style must not be empty");
        assert!(
            style.contains("rounded"),
            "default style should use rounded"
        );
    }

    #[test]
    fn test_svg_output_is_valid_xml() {
        let payload = two_node_payload();
        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let svg = render_svg(&compiled, &layout, "standard").unwrap();
        assert!(
            svg.starts_with("<?xml"),
            "SVG must start with XML declaration"
        );
        assert!(svg.contains("<svg"), "must contain svg element");
        assert!(svg.contains("</svg>"), "must close svg element");
    }

    #[test]
    fn test_style_mapping_dark_mode() {
        let style = style_for_type("proxy", "dark");
        assert!(
            style.contains("fillColor=#1e293b"),
            "dark mode must use slate-800 fill"
        );
        assert!(
            style.contains("fontColor=#f1f5f9"),
            "dark mode must use light font color"
        );
        assert!(
            style.contains("strokeColor=#475569"),
            "dark mode must use slate-600 stroke"
        );
    }

    #[test]
    fn test_drawio_html_two_line_label() {
        let payload = DiagramPayload {
            diagram_type: "flowchart".to_owned(),
            nodes: vec![NodeDef {
                id: "n1".to_owned(),
                label: "API Gateway\n(Kong Ingress)".to_owned(),
                node_type: "proxy".to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let xml = render_drawio(&compiled, &layout, "standard").unwrap();
        assert!(
            xml.contains("&lt;b&gt;API Gateway&lt;/b&gt;&lt;br/&gt;&lt;font style=&quot;font-size:10px;color:#64748b&quot;&gt;(Kong Ingress)&lt;/font&gt;"),
            "node cell must contain two-line formatted HTML label"
        );
    }

    #[test]
    fn test_drawio_html_multiline_title() {
        let payload = DiagramPayload {
            diagram_type: "flowchart".to_owned(),
            nodes: vec![NodeDef {
                id: "n1".to_owned(),
                label: "petgraph::\nStableDiGraph".to_owned(),
                node_type: "database".to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let xml = render_drawio(&compiled, &layout, "standard").unwrap();
        assert!(
            xml.contains("&lt;b&gt;petgraph::&lt;/b&gt;&lt;br/&gt;&lt;b&gt;StableDiGraph&lt;/b&gt;"),
            "both non-parenthesized lines must be formatted as bold titles"
        );
    }

    #[test]
    fn test_typography_spans_and_latex() {
        let spans = parse_inline_spans("`clap::Cli` and $R \\times C$");
        assert_eq!(spans.len(), 3);
        assert!(spans[0].style.is_code);
        assert_eq!(spans[0].text, "clap::Cli");
        assert_eq!(spans[1].text, " and ");
        assert!(spans[2].style.is_math);
        assert_eq!(spans[2].text, r"R \times C");

        let uni = latex_to_unicode(r"R \times C \le \alpha");
        assert_eq!(uni, "R × C ≤ α");
    }

    #[test]
    fn test_drawio_edge_style_and_offset() {
        let payload = two_node_payload();
        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let xml = render_drawio(&compiled, &layout, "standard").unwrap();

        assert!(xml.contains("page=\"0\""), "page should be 0");
        assert!(
            xml.contains("background=\"#f8fafc\""),
            "background should be #f8fafc"
        );
        assert!(
            xml.contains("jumpStyle=arc"),
            "edge style must have jumpStyle=arc"
        );
        assert!(
            xml.contains("endArrow=blockThin"),
            "edge style must have endArrow=blockThin"
        );
        assert!(
            xml.contains("labelBackgroundColor=#ffffff"),
            "edge style must have labelBackgroundColor=#ffffff"
        );
        assert!(
            xml.contains("<mxPoint y=\"-10\" as=\"offset\""),
            "labeled edge geometry must include offset mxPoint"
        );
    }

    #[test]
    fn test_drawio_sibling_exit_ports() {
        let payload = DiagramPayload {
            diagram_type: "flowchart".to_owned(),
            nodes: vec![
                NodeDef {
                    id: "src".to_owned(),
                    label: "Source".to_owned(),
                    node_type: "proxy".to_owned(),
                    ..Default::default()
                },
                NodeDef {
                    id: "dst1".to_owned(),
                    label: "Target 1".to_owned(),
                    node_type: "server".to_owned(),
                    ..Default::default()
                },
                NodeDef {
                    id: "dst2".to_owned(),
                    label: "Target 2".to_owned(),
                    node_type: "server".to_owned(),
                    ..Default::default()
                },
            ],
            edges: vec![
                EdgeDef {
                    from: "src".to_owned(),
                    to: "dst1".to_owned(),
                    ..Default::default()
                },
                EdgeDef {
                    from: "src".to_owned(),
                    to: "dst2".to_owned(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let xml = render_drawio(&compiled, &layout, "standard").unwrap();

        assert!(
            xml.contains("exitX=0.1;"),
            "first sibling exit port should be 0.1"
        );
        assert!(
            xml.contains("exitX=0.9;"),
            "second sibling exit port should be 0.9"
        );
    }

    #[test]
    fn test_drawio_renders_groups_and_semantic_edge_styles() {
        use crate::schema::GroupDef;

        let payload = DiagramPayload {
            diagram_type: "architecture".to_owned(),
            groups: vec![GroupDef {
                id: "grp_core".to_owned(),
                label: "Core Services".to_owned(),
                color: Some("#3b82f6".to_owned()),
                nodes: vec!["s1".to_owned(), "s2".to_owned()],
            }],
            nodes: vec![
                NodeDef {
                    id: "s1".to_owned(),
                    label: "Producer".to_owned(),
                    node_type: "server".to_owned(),
                    ..Default::default()
                },
                NodeDef {
                    id: "s2".to_owned(),
                    label: "Consumer".to_owned(),
                    node_type: "server".to_owned(),
                    ..Default::default()
                },
            ],
            edges: vec![EdgeDef {
                from: "s1".to_owned(),
                to: "s2".to_owned(),
                label: Some("events".to_owned()),
                edge_style: Some("async".to_owned()),
                ..Default::default()
            }],
            ..Default::default()
        };

        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let xml = render_drawio(&compiled, &layout, "standard").unwrap();

        assert!(xml.contains("id=\"grp_core\""), "must render group cell");
        assert!(xml.contains("Core Services"), "must render group label");
        assert!(xml.contains("container=1"), "group cell style must include container=1");
        assert!(xml.contains("parent=\"grp_core\""), "child nodes must specify group container as parent");
        assert!(xml.contains("dashPattern=8 4"), "async edge must be dashed");
        assert!(xml.contains("strokeColor=#d97706"), "async edge must be amber");

        let svg = render_svg(&compiled, &layout, "standard").unwrap();
        assert!(svg.contains("Core Services"), "svg must render group label");
        assert!(svg.contains("class=\"diagram-group\""), "svg must render group container with diagram-group class");
        assert!(svg.contains("stroke-dasharray=\"8 4\""), "svg must render async dasharray");
    }

    #[test]
    fn test_drawio_dark_mode_canvas_and_edges() {
        let payload = two_node_payload();
        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let xml = render_drawio(&compiled, &layout, "dark").unwrap();

        assert!(xml.contains("background=\"#0f172a\""), "dark theme must use slate-900 canvas");
        assert!(xml.contains("labelBackgroundColor=#1e293b"), "dark theme must use dark edge label bg");

        let svg = render_svg(&compiled, &layout, "dark").unwrap();
        assert!(svg.contains("fill=\"#0f172a\""), "dark theme svg must have dark canvas");
    }

    #[test]
    fn test_drawio_horizontal_ports() {
        let payload = two_node_payload();
        let compiled = build_graph(&payload).unwrap();
        let config = LayoutConfig {
            direction: crate::layout::LayoutDirection::LeftToRight,
            ..LayoutConfig::default()
        };
        let layout = compute_layout(&compiled, &config).unwrap();
        let xml = render_drawio(&compiled, &layout, "standard").unwrap();
        assert!(xml.contains("exitX=1.0;"), "horizontal layout edge must exit from right");
        assert!(xml.contains("entryX=0.0;"), "horizontal layout edge must enter on left");
    }

    #[test]
    fn test_svg_card_shadow_and_orthogonal_paths() {
        let payload = two_node_payload();
        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let svg = render_svg(&compiled, &layout, "standard").unwrap();

        assert!(svg.contains("id=\"card-shadow\""), "svg must define card-shadow filter");
        assert!(svg.contains("feDropShadow"), "svg must include drop shadow primitive");
        assert!(svg.contains("<path d=\"M "), "edges should be rendered as orthogonal paths");
        assert!(svg.contains("<tspan "), "node labels must use tspan elements");
        assert!(svg.contains("marker-end="), "edges must have arrow markers");
        assert!(svg.contains("ellipse"), "database nodes must render 3D cylinder top cap");
    }

    #[test]
    fn test_svg_wrapped_multiline_labels() {
        let payload = DiagramPayload {
            diagram_type: "flowchart".to_owned(),
            nodes: vec![NodeDef {
                id: "n1".to_owned(),
                label: "Distributed Architecture Data Pipeline Coordinator".to_owned(),
                node_type: "server".to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let svg = render_svg(&compiled, &layout, "standard").unwrap();

        assert!(svg.contains("dy=\"14\""), "wrapped lines must have dy offset");
        assert!(svg.contains("Distributed"), "first line should contain Distributed");
    }

    #[test]
    fn test_language_and_database_icons_rendered() {
        let payload = DiagramPayload {
            diagram_type: "architecture".to_owned(),
            nodes: vec![
                NodeDef {
                    id: "svc".to_owned(),
                    label: "Order Service".to_owned(),
                    node_type: "service".to_owned(),
                    language: Some("rust".to_owned()),
                    technology: Some("Axum".to_owned()),
                    ..Default::default()
                },
                NodeDef {
                    id: "db".to_owned(),
                    label: "Order DB".to_owned(),
                    node_type: "database".to_owned(),
                    db_type: Some("postgres".to_owned()),
                    ..Default::default()
                },
            ],
            edges: vec![
                crate::schema::EdgeDef {
                    from: "svc".to_owned(),
                    to: "db".to_owned(),
                    label: Some("queries".to_owned()),
                    ..Default::default()
                }
            ],
            ..Default::default()
        };

        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();

        let xml = render_drawio(&compiled, &layout, "standard").unwrap();
        assert!(xml.contains("data:image/svg+xml;base64,"), "drawio must embed base64 icon data URI");
        assert!(xml.contains("Axum"), "drawio must include tech badge");
        assert!(xml.contains("shape=cylinder3"), "drawio database must use cylinder3 shape");

        let svg = render_svg(&compiled, &layout, "standard").unwrap();
        assert!(svg.contains("class=\"node-tech-icon\""), "svg must embed tech icon element");
        assert!(svg.contains("ellipse"), "svg database must render 3D cylinder top cap");
        assert!(svg.contains("Order Service"), "svg must render service title");
    }

    #[test]
    fn test_erd_markers_and_table_cylinder() {
        let payload = DiagramPayload {
            diagram_type: "erd".to_owned(),
            nodes: vec![
                NodeDef {
                    id: "orders".to_owned(),
                    label: "orders".to_owned(),
                    node_type: "table".to_owned(),
                    fields: vec![
                        "id: UUID [PK]".to_owned(),
                        "user_id: UUID [FK]".to_owned(),
                        "amount: DECIMAL".to_owned(),
                    ],
                    ..Default::default()
                },
                NodeDef {
                    id: "users".to_owned(),
                    label: "users".to_owned(),
                    node_type: "table".to_owned(),
                    fields: vec![
                        "id: UUID [PK]".to_owned(),
                        "email: VARCHAR".to_owned(),
                    ],
                    ..Default::default()
                },
            ],
            edges: vec![
                crate::schema::EdgeDef {
                    from: "users".to_owned(),
                    to: "orders".to_owned(),
                    edge_style: Some("one_to_many".to_owned()),
                    label: Some("places".to_owned()),
                    ..Default::default()
                }
            ],
            ..Default::default()
        };

        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();

        let xml = render_drawio(&compiled, &layout, "standard").unwrap();
        assert!(xml.contains("startArrow=ERone"), "drawio ER edge must have ERone start arrow");
        assert!(xml.contains("endArrow=ERmany"), "drawio ER edge must have ERmany end arrow");
        assert!(xml.contains("[PK]"), "drawio table must render PK field");

        let svg = render_svg(&compiled, &layout, "standard").unwrap();
        assert!(svg.contains("marker-er-many"), "svg must define and use marker-er-many");
        assert!(svg.contains("marker-er-one"), "svg must define and use marker-er-one");
        assert!(svg.contains("UUID [PK]"), "svg must render primary key column");
    }

    #[test]
    fn test_uml_markers_and_class_render() {
        let payload = DiagramPayload {
            diagram_type: "class".to_owned(),
            nodes: vec![
                NodeDef {
                    id: "animal".to_owned(),
                    label: "Animal".to_owned(),
                    node_type: "abstract_class".to_owned(),
                    fields: vec![
                        "+name: String".to_owned(),
                        "+make_sound(): void".to_owned(),
                    ],
                    ..Default::default()
                },
                NodeDef {
                    id: "dog".to_owned(),
                    label: "Dog".to_owned(),
                    node_type: "class".to_owned(),
                    fields: vec![
                        "+bark(): void".to_owned(),
                    ],
                    ..Default::default()
                },
            ],
            edges: vec![
                crate::schema::EdgeDef {
                    from: "dog".to_owned(),
                    to: "animal".to_owned(),
                    edge_style: Some("inheritance".to_owned()),
                    ..Default::default()
                }
            ],
            ..Default::default()
        };

        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();

        let xml = render_drawio(&compiled, &layout, "standard").unwrap();
        assert!(xml.contains("endArrow=block;endFill=0"), "drawio UML inheritance must be hollow triangle");
        assert!(
            xml.contains("&amp;lt;&amp;lt;abstract&amp;gt;&amp;gt;") || xml.contains("&lt;&lt;abstract&gt;&gt;"),
            "drawio abstract class must have stereotype prefix"
        );

        let svg = render_svg(&compiled, &layout, "standard").unwrap();
        assert!(svg.contains("marker-uml-triangle"), "svg must define and use marker-uml-triangle");
        assert!(svg.contains("&lt;&lt;abstract&gt;&gt;"), "svg must render abstract stereotype");
    }

    #[test]
    fn test_sequence_diagram_render() {
        let payload = DiagramPayload {
            diagram_type: "sequence".to_owned(),
            nodes: vec![
                NodeDef {
                    id: "client".to_owned(),
                    label: "Client".to_owned(),
                    node_type: "client".to_owned(),
                    ..Default::default()
                },
                NodeDef {
                    id: "server".to_owned(),
                    label: "API Server".to_owned(),
                    node_type: "server".to_owned(),
                    ..Default::default()
                },
            ],
            edges: vec![
                crate::schema::EdgeDef {
                    from: "client".to_owned(),
                    to: "server".to_owned(),
                    label: Some("POST /login".to_owned()),
                    ..Default::default()
                },
                crate::schema::EdgeDef {
                    from: "server".to_owned(),
                    to: "client".to_owned(),
                    label: Some("200 OK (JWT)".to_owned()),
                    edge_style: Some("dashed".to_owned()),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();

        let xml = render_drawio(&compiled, &layout, "standard").unwrap();
        assert!(xml.contains("dashed=1"), "drawio sequence lifeline must be dashed");
        assert!(xml.contains("dashPattern=6 6"), "drawio sequence lifeline must have dash pattern");
        assert!(xml.contains("POST /login"), "drawio sequence must include message label");

        let svg = render_svg(&compiled, &layout, "standard").unwrap();
        assert!(svg.contains("stroke-dasharray=\"6 6\""), "svg sequence lifeline must be dashed");
        assert!(svg.contains("POST /login"), "svg sequence must include message text");
    }

    #[test]
    fn test_group_title_collision_avoidance() {
        use crate::schema::GroupDef;

        let payload = DiagramPayload {
            nodes: vec![
                NodeDef {
                    id: "source_node".to_owned(),
                    label: "Source Node".to_owned(),
                    ..Default::default()
                },
                NodeDef {
                    id: "target_node".to_owned(),
                    label: "Target Node".to_owned(),
                    ..Default::default()
                },
            ],
            groups: vec![GroupDef {
                id: "grp1".to_owned(),
                label: "Rendering Layer Container".to_owned(),
                color: Some("#7c3aed".to_owned()),
                nodes: vec!["target_node".to_owned()],
            }],
            edges: vec![crate::schema::EdgeDef {
                from: "source_node".to_owned(),
                to: "target_node".to_owned(),
                label: Some("connects".to_owned()),
                ..Default::default()
            }],
            ..Default::default()
        };

        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let plans = plan_all_edge_routes(&compiled, &layout);

        let plan = plans.values().next().unwrap();
        // Since target_node is inside a group whose title sits directly at the top,
        // the router must divert the entry face to Left or Right to avoid cutting the container title!
        assert!(
            plan.dst_side == Side::Left || plan.dst_side == Side::Right,
            "target face must be Left or Right to avoid cutting container title banner"
        );

        let svg = render_svg(&compiled, &layout, "standard").unwrap();
        assert!(svg.contains("connects"), "edge label must be rendered");
    }

    #[test]
    fn test_monotonic_port_sorting_no_crossing() {
        // Source node connects to two target nodes placed left and right
        let payload = DiagramPayload {
            nodes: vec![
                NodeDef {
                    id: "src".to_owned(),
                    label: "Source".to_owned(),
                    ..Default::default()
                },
                NodeDef {
                    id: "dst_left".to_owned(),
                    label: "Left Target".to_owned(),
                    ..Default::default()
                },
                NodeDef {
                    id: "dst_right".to_owned(),
                    label: "Right Target".to_owned(),
                    ..Default::default()
                },
            ],
            edges: vec![
                // Declare right first, left second to verify sorting fixes crossing
                crate::schema::EdgeDef {
                    from: "src".to_owned(),
                    to: "dst_right".to_owned(),
                    ..Default::default()
                },
                crate::schema::EdgeDef {
                    from: "src".to_owned(),
                    to: "dst_left".to_owned(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let plans = plan_all_edge_routes(&compiled, &layout);

        // Find the plans for dst_left and dst_right
        let left_edge_idx = compiled.graph.edge_indices().find(|&e| {
            let (_, d) = compiled.graph.edge_endpoints(e).unwrap();
            compiled.graph[d].id == "dst_left"
        }).unwrap();
        let right_edge_idx = compiled.graph.edge_indices().find(|&e| {
            let (_, d) = compiled.graph.edge_endpoints(e).unwrap();
            compiled.graph[d].id == "dst_right"
        }).unwrap();

        let plan_left = &plans[&left_edge_idx];
        let plan_right = &plans[&right_edge_idx];

        let left_target_nl = &layout.positions[&compiled.node_map["dst_left"]];
        let right_target_nl = &layout.positions[&compiled.node_map["dst_right"]];

        if left_target_nl.x < right_target_nl.x {
            assert!(
                plan_left.exit_port < plan_right.exit_port,
                "exit ports must be monotonic with target coordinates to prevent crossings (left: {}, right: {})",
                plan_left.exit_port,
                plan_right.exit_port
            );
        }
    }

    #[test]
    fn test_custom_edge_formatting_render() {
        let payload = DiagramPayload {
            nodes: vec![
                NodeDef {
                    id: "n1".to_owned(),
                    label: "Service A".to_owned(),
                    ..Default::default()
                },
                NodeDef {
                    id: "n2".to_owned(),
                    label: "Service B".to_owned(),
                    ..Default::default()
                },
            ],
            edges: vec![crate::schema::EdgeDef {
                from: "n1".to_owned(),
                to: "n2".to_owned(),
                label: Some("custom edge".to_owned()),
                color: Some("#ec4899".to_owned()),
                width: Some(2.5),
                line_style: Some("dashed".to_owned()),
                head: Some("open".to_owned()),
                tail: Some("circle".to_owned()),
                ..Default::default()
            }],
            ..Default::default()
        };

        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();

        let xml = render_drawio(&compiled, &layout, "standard").unwrap();
        assert!(xml.contains("strokeColor=#ec4899"), "drawio must include custom edge color");
        assert!(xml.contains("strokeWidth=2.5"), "drawio must include custom stroke width");
        assert!(xml.contains("dashed=1"), "drawio must include dashed line style");
        assert!(xml.contains("endArrow=open"), "drawio must include open endArrow");
        assert!(xml.contains("startArrow=circle"), "drawio must include circle startArrow");

        let svg = render_svg(&compiled, &layout, "standard").unwrap();
        assert!(svg.contains("stroke=\"#ec4899\""), "svg must include custom edge color");
        assert!(svg.contains("stroke-width=\"2.5\""), "svg must include custom stroke width");
        assert!(svg.contains("stroke-dasharray=\"8 4\""), "svg must include dashed dasharray");
        assert!(svg.contains("marker-end=\"url(#marker-open-slate)\""), "svg must use open marker");
        assert!(svg.contains("marker-start=\"url(#marker-circle-fill)\""), "svg must use circle marker");
    }

    #[test]
    fn test_arrow_shortest_distance_and_clearance_stubs() {
        let p1 = (100.0, 100.0);
        let p2 = (300.0, 250.0);
        let channel_y = 175.0;
        let corridor_x = 200.0;

        // Downward vertical flow (Side::Bottom, Side::Top)
        let waypoints = compute_edge_waypoints(p1, Side::Bottom, p2, Side::Top, channel_y, corridor_x);
        assert_eq!(waypoints.len(), 2, "must produce 2 intermediate waypoints for S-bend");
        let w1 = waypoints[0];
        let w2 = waypoints[1];

        // Verify clearance stub: turn must not happen right at start point
        assert!(
            (w1.1 - p1.1).abs() >= 18.0,
            "clearance stub at start must be at least 18px from start face (got {})",
            (w1.1 - p1.1).abs()
        );
        // Verify clearance stub: turn must not happen right at end point
        assert!(
            (p2.1 - w2.1).abs() >= 18.0,
            "clearance stub at end must be at least 18px from destination face (got {})",
            (p2.1 - w2.1).abs()
        );

        // Path generation with smooth fillet corners
        let (svg_d, lx, ly) = build_orthogonal_svg_path(p1, p2, &waypoints);
        assert!(svg_d.starts_with("M 100.0 100.0"), "must start at p1");
        assert!(svg_d.contains("Q "), "must use rounded fillet corners");
        assert!(svg_d.ends_with("300.0 250.0"), "must end at p2");
        assert!((lx - 200.0).abs() < 2.0, "label should be centered in the horizontal channel");
        assert!((ly - 175.0).abs() < 2.0, "label should be at channel height");
    }

    #[test]
    fn test_arrow_waypoints_emitted_in_drawio_xml() {
        // Multi-level hierarchy that requires orthogonal bends
        let payload = DiagramPayload {
            nodes: vec![
                NodeDef {
                    id: "root".to_owned(),
                    label: "Root Node".to_owned(),
                    ..Default::default()
                },
                NodeDef {
                    id: "child".to_owned(),
                    label: "Child Node Offset".to_owned(),
                    ..Default::default()
                },
            ],
            edges: vec![crate::schema::EdgeDef {
                from: "root".to_owned(),
                to: "child".to_owned(),
                label: Some("routes through".to_owned()),
                ..Default::default()
            }],
            ..Default::default()
        };

        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let xml = render_drawio(&compiled, &layout, "standard").unwrap();

        // Check that mxGeometry contains waypoints Array if bend is needed
        assert!(xml.contains("edgeStyle=orthogonalEdgeStyle"), "must use orthogonal edge style");
        assert!(xml.contains("labelBackgroundColor=#ffffff"), "must have clean borderless label");
    }
}
