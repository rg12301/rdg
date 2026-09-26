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
        "database" | "db" | "storage" => {
            format!(
                "shape=cylinder3;boundedLbl=1;backgroundOutline=1;\
                     whiteSpace=wrap;html=1;fillColor=#ffffff;shadow=1;\
                     strokeWidth=1.5;strokeColor=#38bdf8;\
                     fontFamily=Inter,Helvetica,sans-serif;\
                     fontSize=12;fontStyle=1;fontColor=#0f172a;\
                     spacingTop=16;spacingBottom=6;"
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

/// Compute an orthogonal SVG path with rounded fillet corners between two points.
/// Returns `(path_d, label_center_x, label_center_y)`.
/// Places label along the initial straight segment away from turns and crossings.
fn orthogonal_path(x1: f64, y1: f64, x2: f64, y2: f64, is_horizontal: bool) -> (String, f64, f64) {
    if is_horizontal {
        // Horizontal flow: exit right (x1, y1), entry left (x2, y2)
        if (y2 - y1).abs() < 1.0 {
            return (
                format!("M {x1:.1} {y1:.1} L {x2:.1} {y2:.1}"),
                (x1 + x2) / 2.0,
                y1,
            );
        }
        let xmid = (x1 + x2) / 2.0;
        let r = 8.0_f64
            .min((xmid - x1).abs() / 2.0)
            .min((y2 - y1).abs() / 2.0)
            .min((x2 - xmid).abs() / 2.0)
            .max(0.0);

        let lx = x1 + 22.0_f64.min((xmid - x1).abs() * 0.45);
        let ly = y1;

        if r < 1.0 {
            return (
                format!("M {x1:.1} {y1:.1} L {xmid:.1} {y1:.1} L {xmid:.1} {y2:.1} L {x2:.1} {y2:.1}"),
                lx,
                ly,
            );
        }

        let (q1_end_y, q2_start_y) = if y2 > y1 {
            (y1 + r, y2 - r)
        } else {
            (y1 - r, y2 + r)
        };

        let path = format!(
            "M {x1:.1} {y1:.1} \
             L {xm_prev:.1} {y1:.1} \
             Q {xmid:.1} {y1:.1} {xmid:.1} {q1_end_y:.1} \
             L {xmid:.1} {q2_start_y:.1} \
             Q {xmid:.1} {y2:.1} {xm_next:.1} {y2:.1} \
             L {x2:.1} {y2:.1}",
            xm_prev = xmid - r,
            xm_next = xmid + r,
        );
        (path, lx, ly)
    } else {
        // Vertical flow: exit bottom (x1, y1), entry top (x2, y2)
        if (x2 - x1).abs() < 1.0 {
            return (
                format!("M {x1:.1} {y1:.1} L {x2:.1} {y2:.1}"),
                x1,
                (y1 + y2) / 2.0,
            );
        }
        let ymid = (y1 + y2) / 2.0;
        let r = 8.0_f64
            .min((ymid - y1).abs() / 2.0)
            .min((x2 - x1).abs() / 2.0)
            .min((y2 - ymid).abs() / 2.0)
            .max(0.0);

        // Place label on the initial vertical drop, away from the turn at ymid
        let lx = x1;
        let ly = y1 + 18.0_f64.min((ymid - y1).abs() * 0.45);

        if r < 1.0 {
            return (
                format!("M {x1:.1} {y1:.1} L {x1:.1} {ymid:.1} L {x2:.1} {ymid:.1} L {x2:.1} {y2:.1}"),
                lx,
                ly,
            );
        }

        let (q1_end_x, q2_start_x) = if x2 > x1 {
            (x1 + r, x2 - r)
        } else {
            (x1 - r, x2 + r)
        };

        let path = format!(
            "M {x1:.1} {y1:.1} \
             L {x1:.1} {ym_prev:.1} \
             Q {x1:.1} {ymid:.1} {q1_end_x:.1} {ymid:.1} \
             L {q2_start_x:.1} {ymid:.1} \
             Q {x2:.1} {ymid:.1} {x2:.1} {ym_next:.1} \
             L {x2:.1} {y2:.1}",
            ym_prev = ymid - r,
            ym_next = ymid + r,
        );
        (path, lx, ly)
    }
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
        let style = style_for_type(&node_data.node_type, theme);
        let tooltip = node_data.metadata.as_deref().unwrap_or("");

        // Build HTML label: formatted with typography, title/subtitle hierarchy, and code spans
        let html_value = format_html_label(&node_data.label, theme, &node_data.node_type);

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
    // Group outgoing edges by their rendered source node to compute distributed exit ports.
    let mut outgoing_by_src: HashMap<
        petgraph::graph::NodeIndex,
        Vec<(petgraph::graph::EdgeIndex, f64)>,
    > = HashMap::new();

    for edge_idx in compiled.graph.edge_indices() {
        let (src, dst) = compiled.graph.edge_endpoints(edge_idx).unwrap();
        let edge_data = &compiled.graph[edge_idx];
        let (render_src, render_dst) = if edge_data.reversed {
            (dst, src)
        } else {
            (src, dst)
        };
        let target_x = layout
            .positions
            .get(&render_dst)
            .map(|p| p.x)
            .unwrap_or(0.0);
        outgoing_by_src
            .entry(render_src)
            .or_default()
            .push((edge_idx, target_x));
    }

    let mut exit_ports: HashMap<petgraph::graph::EdgeIndex, f64> = HashMap::new();
    for (_, mut edges) in outgoing_by_src {
        edges.sort_by(|a, b| {
            a.1.partial_cmp(&b.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
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

    // Group incoming edges by target node to compute distributed entry ports.
    let mut incoming_by_dst: HashMap<
        petgraph::graph::NodeIndex,
        Vec<(petgraph::graph::EdgeIndex, f64)>,
    > = HashMap::new();

    for edge_idx in compiled.graph.edge_indices() {
        let (src, dst) = compiled.graph.edge_endpoints(edge_idx).unwrap();
        let edge_data = &compiled.graph[edge_idx];
        let (render_src, render_dst) = if edge_data.reversed {
            (dst, src)
        } else {
            (src, dst)
        };
        let source_x = layout
            .positions
            .get(&render_src)
            .map(|p| p.x)
            .unwrap_or(0.0);
        incoming_by_dst
            .entry(render_dst)
            .or_default()
            .push((edge_idx, source_x));
    }

    let mut entry_ports: HashMap<petgraph::graph::EdgeIndex, f64> = HashMap::new();
    for (_, mut edges) in incoming_by_dst {
        edges.sort_by(|a, b| {
            a.1.partial_cmp(&b.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
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
        let (render_src, render_dst, s_idx, d_idx) = if edge_data.reversed {
            (dst_id.as_str(), src_id.as_str(), dst, src)
        } else {
            (src_id.as_str(), dst_id.as_str(), src, dst)
        };

        let port_frac = exit_ports.get(&edge_idx).copied().unwrap_or(0.5);
        let entry_port_frac = entry_ports.get(&edge_idx).copied().unwrap_or(0.5);

        let (src_nl, dst_nl) = (layout.positions.get(&s_idx), layout.positions.get(&d_idx));
        let is_horizontal = match (src_nl, dst_nl) {
            (Some(s), Some(d)) => (d.x - s.x) > (d.y - s.y).abs(),
            _ => false,
        };

        let (exit_attr, entry_attr) = if is_horizontal {
            (
                format!("exitX=1.0;exitY={port_frac:.1};exitDx=0;exitDy=0;"),
                format!("entryX=0.0;entryY={entry_port_frac:.1};entryDx=0;entryDy=0;"),
            )
        } else {
            (
                format!("exitX={port_frac:.1};exitY=1.0;exitDx=0;exitDy=0;"),
                format!("entryX={entry_port_frac:.1};entryY=0.0;entryDx=0;entryDy=0;"),
            )
        };

        let bi_style = format!("strokeColor={default_edge_color};strokeWidth=1.5;startArrow=blockThin;startFill=1;endArrow=blockThin;endFill=1;");
        let default_style = format!("strokeColor={default_edge_color};strokeWidth=1.5;endArrow=blockThin;endFill=1;");

        let custom_style = match edge_data.edge_style.as_deref() {
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
            _ => &default_style,
        };

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
        //   <mxPoint y="-10" as="offset" />
        // </mxGeometry>
        let mut geo = BytesStart::new("mxGeometry");
        geo.push_attribute(("relative", "1"));
        geo.push_attribute(("as", "geometry"));
        if !label.is_empty() {
            w.write_event(Event::Start(geo))?;
            let mut pt = BytesStart::new("mxPoint");
            pt.push_attribute(("y", "-10"));
            pt.push_attribute(("as", "offset"));
            w.write_event(Event::Empty(pt))?;
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

    w.write_event(Event::End(BytesEnd::new("defs")))?;

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

    // --- Draw edges (behind nodes) with orthogonal rounded paths ------------
    let mut outgoing_by_src: HashMap<
        petgraph::graph::NodeIndex,
        Vec<(petgraph::graph::EdgeIndex, f64)>,
    > = HashMap::new();

    for edge_idx in compiled.graph.edge_indices() {
        let (src, dst) = compiled.graph.edge_endpoints(edge_idx).unwrap();
        let edge_data = &compiled.graph[edge_idx];
        let (render_src, render_dst) = if edge_data.reversed {
            (dst, src)
        } else {
            (src, dst)
        };
        let target_x = layout
            .positions
            .get(&render_dst)
            .map(|p| p.x)
            .unwrap_or(0.0);
        outgoing_by_src
            .entry(render_src)
            .or_default()
            .push((edge_idx, target_x));
    }

    let mut exit_ports: HashMap<petgraph::graph::EdgeIndex, f64> = HashMap::new();
    for (_, mut edges) in outgoing_by_src {
        edges.sort_by(|a, b| {
            a.1.partial_cmp(&b.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
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

    // Group incoming edges by target node to compute distributed entry ports.
    let mut incoming_by_dst: HashMap<
        petgraph::graph::NodeIndex,
        Vec<(petgraph::graph::EdgeIndex, f64)>,
    > = HashMap::new();

    for edge_idx in compiled.graph.edge_indices() {
        let (src, dst) = compiled.graph.edge_endpoints(edge_idx).unwrap();
        let edge_data = &compiled.graph[edge_idx];
        let (render_src, render_dst) = if edge_data.reversed {
            (dst, src)
        } else {
            (src, dst)
        };
        let source_x = layout
            .positions
            .get(&render_src)
            .map(|p| p.x)
            .unwrap_or(0.0);
        incoming_by_dst
            .entry(render_dst)
            .or_default()
            .push((edge_idx, source_x));
    }

    let mut entry_ports: HashMap<petgraph::graph::EdgeIndex, f64> = HashMap::new();
    for (_, mut edges) in incoming_by_dst {
        edges.sort_by(|a, b| {
            a.1.partial_cmp(&b.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
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

        let port_frac = exit_ports.get(&edge_idx).copied().unwrap_or(0.5);
        let entry_port_frac = entry_ports.get(&edge_idx).copied().unwrap_or(0.5);
        let is_horizontal = (dst_nl.x - src_nl.x) > (dst_nl.y - src_nl.y).abs();

        let (x1, y1, x2, y2) = if is_horizontal {
            (
                src_nl.x + src_nl.width,
                src_nl.y + src_nl.height * port_frac,
                dst_nl.x,
                dst_nl.y + dst_nl.height * entry_port_frac,
            )
        } else {
            (
                src_nl.x + src_nl.width * port_frac,
                src_nl.y + src_nl.height,
                dst_nl.x + dst_nl.width * entry_port_frac,
                dst_nl.y,
            )
        };

        let is_bi = matches!(edge_data.edge_style.as_deref(), Some("bi") | Some("bidirectional"));
        let (stroke, stroke_w, dash, marker_id) = match edge_data.edge_style.as_deref() {
            Some("async") => ("#d97706", "1.5", Some("8 4"), "arrow-amber"),
            Some("error") | Some("fallback") => ("#ef4444", "1.5", Some("6 3"), "arrow-red"),
            Some("data") | Some("stream") => ("#6366f1", "2.0", None, "arrow-indigo"),
            _ => (default_edge, "1.5", None, if is_dark { "arrow-dark" } else { "arrow-slate" }),
        };

        let (path_d, lx, ly) = orthogonal_path(x1, y1, x2, y2, is_horizontal);

        let mut path = BytesStart::new("path");
        path.push_attribute(("d", path_d.as_str()));
        path.push_attribute(("fill", "none"));
        path.push_attribute(("stroke", stroke));
        path.push_attribute(("stroke-width", stroke_w));
        if let Some(d) = dash {
            path.push_attribute(("stroke-dasharray", d));
        }
        if is_bi {
            path.push_attribute(("marker-start", format!("url(#{marker_id})").as_str()));
        }
        path.push_attribute(("marker-end", format!("url(#{marker_id})").as_str()));
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
        let pill_w = (char_count as f64 * 6.2 + 10.0).max(20.0);
        let pill_h = 16.0;
        let pill_x = el.lx - pill_w / 2.0;
        let pill_y = el.ly - pill_h / 2.0;

        let mut pill = BytesStart::new("rect");
        pill.push_attribute(("x", format!("{pill_x:.1}").as_str()));
        pill.push_attribute(("y", format!("{pill_y:.1}").as_str()));
        pill.push_attribute(("width", format!("{pill_w:.1}").as_str()));
        pill.push_attribute(("height", format!("{pill_h:.1}").as_str()));
        pill.push_attribute(("rx", "4"));
        pill.push_attribute(("ry", "4"));
        pill.push_attribute(("fill", bg_color));
        w.write_event(Event::Empty(pill))?;

        let mut text = BytesStart::new("text");
        text.push_attribute(("x", format!("{:.1}", el.lx).as_str()));
        text.push_attribute(("y", format!("{:.1}", el.ly + 3.5).as_str()));
        text.push_attribute(("text-anchor", "middle"));
        text.push_attribute(("font-family", "Inter, Helvetica, sans-serif"));
        text.push_attribute(("font-size", "10"));
        text.push_attribute(("font-weight", "500"));
        text.push_attribute(("fill", if is_dark { "#cbd5e1" } else { "#475569" }));
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

        let is_db = matches!(
            node_data.node_type.to_ascii_lowercase().as_str(),
            "database" | "db" | "storage"
        );
        let is_decision = matches!(
            node_data.node_type.to_ascii_lowercase().as_str(),
            "decision" | "condition"
        );

        if is_db {
            let rh = (nl.height * 0.18).min(12.0);
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
            top_cap.push_attribute(("fill", card_fill));
            top_cap.push_attribute(("stroke", stroke_color));
            top_cap.push_attribute(("stroke-width", "1.5"));
            w.write_event(Event::Empty(top_cap))?;
        } else if is_decision {
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

        // Center text inside cylindrical body below the top ellipse cap for databases
        let start_y = if is_db {
            let rh = (nl.height * 0.18).min(12.0);
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
            theme: None,
            direction: None,
            nodes: vec![NodeDef {
                id: "n1".to_owned(),
                label: "API Gateway".to_owned(),
                node_type: "proxy".to_owned(),
                metadata: Some("Routes all traffic".to_owned()),
            }],
            edges: vec![],
            groups: vec![],
        }
    }

    fn two_node_payload() -> DiagramPayload {
        DiagramPayload {
            diagram_type: "flowchart".to_owned(),
            theme: None,
            direction: None,
            nodes: vec![
                NodeDef {
                    id: "n1".to_owned(),
                    label: "API Gateway".to_owned(),
                    node_type: "proxy".to_owned(),
                    metadata: None,
                },
                NodeDef {
                    id: "n2".to_owned(),
                    label: "User DB".to_owned(),
                    node_type: "database".to_owned(),
                    metadata: None,
                },
            ],
            edges: vec![EdgeDef {
                from: "n1".to_owned(),
                to: "n2".to_owned(),
                label: Some("queries".to_owned()),
                edge_style: None,
            }],
            groups: vec![],
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
            theme: None,
            direction: None,
            nodes: vec![NodeDef {
                id: "n1".to_owned(),
                label: "API Gateway\n(Kong Ingress)".to_owned(),
                node_type: "proxy".to_owned(),
                metadata: None,
            }],
            edges: vec![],
            groups: vec![],
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
            theme: None,
            direction: None,
            nodes: vec![NodeDef {
                id: "n1".to_owned(),
                label: "petgraph::\nStableDiGraph".to_owned(),
                node_type: "database".to_owned(),
                metadata: None,
            }],
            edges: vec![],
            groups: vec![],
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
            theme: None,
            direction: None,
            nodes: vec![
                NodeDef {
                    id: "src".to_owned(),
                    label: "Source".to_owned(),
                    node_type: "proxy".to_owned(),
                    metadata: None,
                },
                NodeDef {
                    id: "dst1".to_owned(),
                    label: "Target 1".to_owned(),
                    node_type: "server".to_owned(),
                    metadata: None,
                },
                NodeDef {
                    id: "dst2".to_owned(),
                    label: "Target 2".to_owned(),
                    node_type: "server".to_owned(),
                    metadata: None,
                },
            ],
            edges: vec![
                EdgeDef {
                    from: "src".to_owned(),
                    to: "dst1".to_owned(),
                    label: None,
                    edge_style: None,
                },
                EdgeDef {
                    from: "src".to_owned(),
                    to: "dst2".to_owned(),
                    label: None,
                    edge_style: None,
                },
            ],
            groups: vec![],
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
            theme: None,
            direction: None,
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
                    metadata: None,
                },
                NodeDef {
                    id: "s2".to_owned(),
                    label: "Consumer".to_owned(),
                    node_type: "server".to_owned(),
                    metadata: None,
                },
            ],
            edges: vec![EdgeDef {
                from: "s1".to_owned(),
                to: "s2".to_owned(),
                label: Some("events".to_owned()),
                edge_style: Some("async".to_owned()),
            }],
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
            theme: None,
            direction: None,
            nodes: vec![NodeDef {
                id: "n1".to_owned(),
                label: "Distributed Architecture Data Pipeline Coordinator".to_owned(),
                node_type: "server".to_owned(),
                metadata: None,
            }],
            edges: vec![],
            groups: vec![],
        };
        let compiled = build_graph(&payload).unwrap();
        let layout = compute_layout(&compiled, &LayoutConfig::default()).unwrap();
        let svg = render_svg(&compiled, &layout, "standard").unwrap();

        assert!(svg.contains("dy=\"14\""), "wrapped lines must have dy offset");
        assert!(svg.contains("Distributed"), "first line should contain Distributed");
    }
}
