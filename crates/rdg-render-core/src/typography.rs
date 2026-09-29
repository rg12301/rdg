//! Format-agnostic inline typography: subscript/superscript glyph mapping, LaTeX-to-Unicode,
//! and Markdown-ish inline span parsing (`` `code` ``, `**bold**`, `*italic*`, …).
//!
//! Both render backends consume [`parse_inline_spans`] and [`wrap_and_classify_label`] and
//! encode the resulting [`StyledSpan`]s into their own markup (draw.io HTML tags vs SVG
//! `<tspan>` attributes).

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

/// Wraps label into lines and classifies each line — see [`rdg_layout::classify_label`],
/// which the layout engine's size estimate uses too, so the box fits what is drawn.
pub fn wrap_and_classify_label(label: &str, max_chars: usize) -> Vec<ProcessedLine> {
    rdg_layout::classify_label(label, max_chars)
        .into_iter()
        .map(|l| ProcessedLine { text: l.text, is_subtitle: l.is_subtitle })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_inline_spans_code_bold_italic() {
        let spans = parse_inline_spans("`code` **bold** *italic*");
        assert!(spans.iter().any(|s| s.style.is_code && s.text == "code"));
        assert!(spans.iter().any(|s| s.style.is_bold && s.text == "bold"));
        assert!(
            spans
                .iter()
                .any(|s| s.style.is_italic && s.text == "italic")
        );
    }

    #[test]
    fn test_latex_to_unicode() {
        assert_eq!(latex_to_unicode(r"\alpha \times \beta"), "α × β");
    }

    #[test]
    fn test_subscript_superscript() {
        assert_eq!(to_subscript("2"), "₂");
        assert_eq!(to_superscript("2"), "²");
    }

    #[test]
    fn test_first_line_is_title_later_lines_are_details() {
        let lines = wrap_and_classify_label("auth-api-ext\nPublic Auth API\nPort 4000", 40);
        assert_eq!(lines.iter().map(|l| l.is_subtitle).collect::<Vec<_>>(), vec![false, true, true]);
        // A title that visibly continues keeps its next line.
        let lines = wrap_and_classify_label("petgraph::\nStableDiGraph\nthe graph store", 40);
        assert_eq!(lines.iter().map(|l| l.is_subtitle).collect::<Vec<_>>(), vec![false, false, true]);
    }

    #[test]
    fn test_wrap_and_classify_label_marks_parenthesized_as_subtitle() {
        let lines = wrap_and_classify_label("Title\n(subtitle)", 40);
        assert_eq!(lines.len(), 2);
        assert!(!lines[0].is_subtitle);
        assert!(lines[1].is_subtitle);
    }
}
