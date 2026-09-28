//! Node sizing & text wrapping.

use crate::DesignTokens;

/// Wrap label into lines, respecting existing newlines and breaking on word boundaries.
/// Prevents orphan closing delimiters/brackets (like single `}`) from landing alone on a line.
pub fn wrap_label(label: &str, max_chars_per_line: usize) -> Vec<String> {
    let mut result = Vec::new();
    for raw_line in label.split('\n') {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.chars().count() <= max_chars_per_line {
            result.push(trimmed.to_string());
            continue;
        }

        let words: Vec<&str> = trimmed.split_whitespace().collect();
        if words.is_empty() {
            continue;
        }

        let mut current_line = String::new();
        for word in words {
            let is_closing = word
                .chars()
                .all(|c| matches!(c, '}' | ')' | ']' | '>' | ';' | ',' | '.' | ':'));

            if current_line.is_empty() {
                current_line.push_str(word);
            } else if is_closing
                || current_line.chars().count() + 1 + word.chars().count()
                    <= max_chars_per_line + if is_closing { 3 } else { 0 }
            {
                current_line.push(' ');
                current_line.push_str(word);
            } else {
                result.push(current_line);
                current_line = word.to_string();
            }
        }
        if !current_line.is_empty() {
            // Fold orphan single bracket/punctuation back into the preceding line
            let is_orphan = current_line
                .trim()
                .chars()
                .all(|c| matches!(c, '}' | ')' | ']' | '>' | ';' | ',' | '.' | ':'));
            if is_orphan && !result.is_empty() {
                let last = result.last_mut().unwrap();
                last.push(' ');
                last.push_str(current_line.trim());
            } else {
                result.push(current_line);
            }
        }
    }
    if result.is_empty() {
        vec![label.to_string()]
    } else {
        result
    }
}

/// Strips inline markdown tokens (` ``, `**`, `*`, `__`, `~~`, `~`, `^`, `$`, `\(` etc.)
/// and maps common LaTeX symbols to short glyph equivalents so that text width measurement
/// accurately reflects visible rendered glyphs.
pub fn strip_markdown_tokens(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut i = 0;

    while i < n {
        let c = chars[i];
        match c {
            '`' | '*' | '~' | '^' | '$' => {
                i += 1;
            }
            '_' => {
                // If double underscore `__`, skip both (markdown underline tag)
                if i + 1 < n && chars[i + 1] == '_' {
                    i += 2;
                    continue;
                }
                // Intraword underscore: preserve if preceded and followed by alphanumeric
                let prev_is_alnum = i > 0 && chars[i - 1].is_alphanumeric();
                let next_is_alnum = i + 1 < n && chars[i + 1].is_alphanumeric();
                if prev_is_alnum && next_is_alnum {
                    out.push('_');
                }
                i += 1;
            }
            '\\' => {
                // Check if \( or \)
                if i + 1 < n && (chars[i + 1] == '(' || chars[i + 1] == ')') {
                    i += 2;
                    continue;
                }
                // Check LaTeX command e.g. \times, \alpha
                let mut cmd = String::new();
                let mut j = i + 1;
                while j < n && chars[j].is_alphabetic() {
                    cmd.push(chars[j]);
                    j += 1;
                }
                if !cmd.is_empty() {
                    match cmd.as_str() {
                        "times" | "cdot" | "approx" | "le" | "ge" | "ne" | "neq" | "pm" | "to"
                        | "in" => out.push('x'),
                        "alpha" | "beta" | "gamma" | "delta" | "theta" | "lambda" | "pi"
                        | "sigma" | "phi" | "omega" => out.push('w'),
                        "infty" | "sum" | "prod" | "int" => out.push('M'),
                        _ => out.push_str(&cmd),
                    }
                    i = j;
                } else {
                    out.push('\\');
                    i += 1;
                }
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Dynamically estimate the width and height of a node based on its label and shape.
pub fn estimate_node_size(
    label: &str,
    node_type: &str,
    min_width: f64,
    min_height: f64,
    tokens: &DesignTokens,
) -> (f64, f64) {
    estimate_node_size_inner(label, node_type, &[], None, false, min_width, min_height, tokens)
}

/// Dynamically estimate the width and height of a node based on its label, shape, and fields.
///
/// `explicit_width`/`explicit_height` (from the node's YAML `width`/`height` fields) skip
/// estimation for whichever axis is set — a node can pin just its width and still let its
/// height auto-size to its field count, or pin both.
#[allow(clippy::too_many_arguments)]
pub fn estimate_node_size_with_fields(
    label: &str,
    node_type: &str,
    fields: &[String],
    min_width: f64,
    min_height: f64,
    explicit_width: Option<f64>,
    explicit_height: Option<f64>,
    tokens: &DesignTokens,
) -> (f64, f64) {
    estimate_node_size_with_details(
        label,
        node_type,
        fields,
        None,
        false,
        min_width,
        min_height,
        explicit_width,
        explicit_height,
        tokens,
    )
}

/// Dynamically estimate the width and height of a node based on its label, shape, fields,
/// technology subtitle, and brand icon badge.
#[allow(clippy::too_many_arguments)]
pub fn estimate_node_size_with_details(
    label: &str,
    node_type: &str,
    fields: &[String],
    technology: Option<&str>,
    has_icon: bool,
    min_width: f64,
    min_height: f64,
    explicit_width: Option<f64>,
    explicit_height: Option<f64>,
    tokens: &DesignTokens,
) -> (f64, f64) {
    if let (Some(w), Some(h)) = (explicit_width, explicit_height) {
        return (w.max(10.0), h.max(10.0));
    }

    let (est_w, est_h) = estimate_node_size_inner(
        label, node_type, fields, technology, has_icon, min_width, min_height, tokens,
    );
    (
        explicit_width.map_or(est_w, |w| w.max(10.0)),
        explicit_height.map_or(est_h, |h| h.max(10.0)),
    )
}

#[allow(clippy::too_many_arguments)]
fn estimate_node_size_inner(
    label: &str,
    node_type: &str,
    fields: &[String],
    technology: Option<&str>,
    _has_icon: bool,
    min_width: f64,
    min_height: f64,
    tokens: &DesignTokens,
) -> (f64, f64) {
    let lower_type = node_type.to_ascii_lowercase();
    match lower_type.as_str() {
        "start" | "start_state" | "initial" | "initial_state" => {
            let d = tokens.start_marker_size();
            return (d, d);
        }
        "end" | "end_state" | "final" | "final_state" => {
            let d = tokens.end_marker_size();
            return (d, d);
        }
        "choice" | "branch" => {
            let d = tokens.choice_marker_size();
            return (d, d);
        }
        _ => {}
    }

    let is_table = matches!(
        lower_type.as_str(),
        "table" | "entity" | "record" | "schema"
    );
    let is_class = matches!(
        lower_type.as_str(),
        "class" | "interface" | "abstract_class" | "struct"
    );

    // A field row needs to fit one line of field text plus a little breathing room;
    // the table variant's header additionally needs headroom for the cylinder's top
    // ellipse cap and curved bottom, which the plain class card doesn't have.
    let field_row_h = tokens.line_height(tokens.font_size) + tokens.px(0.75);
    let table_header_h = tokens.line_height(tokens.font_size) * 2.0 + tokens.px(2.0);
    let table_bottom_pad = tokens.px(2.25);
    let class_header_h = tokens.line_height(tokens.font_size) * 2.0 + tokens.px(0.5);
    let class_bottom_pad = tokens.px(1.5);

    if (is_table || is_class) && !fields.is_empty() {
        let max_field_chars = fields.iter().map(|f| f.chars().count()).max().unwrap_or(0);
        let title_chars = strip_markdown_tokens(label).chars().count();
        let max_chars = title_chars.max(max_field_chars);
        let width = (max_chars as f64 * tokens.char_width(tokens.font_size) + tokens.px(5.0))
            .max(min_width.max(tokens.px(20.0)))
            .min(tokens.px(45.0));
        let height = if is_table {
            table_header_h + (fields.len() as f64 * field_row_h) + table_bottom_pad
        } else {
            class_header_h + (fields.len() as f64 * field_row_h) + class_bottom_pad
        };
        return (snap(width, tokens), snap(height, tokens));
    }

    let is_diamond = matches!(
        lower_type.as_str(),
        "decision" | "condition" | "cache" | "redis" | "memcache"
    );
    let is_db = matches!(lower_type.as_str(), "database" | "db" | "storage");
    let is_ellipse = matches!(lower_type.as_str(), "queue" | "broker" | "bus" | "topic");

    let max_line_chars = if is_diamond { tokens.wrap_chars_diamond } else { tokens.wrap_chars_normal };
    let clean_label = strip_markdown_tokens(label);
    let lines = wrap_label(&clean_label, max_line_chars);
    let mut max_chars = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);

    let mut line_count = lines.len();
    if let Some(tech) = technology {
        if !label.contains(tech) {
            line_count += 1;
            max_chars = max_chars.max(tech.chars().count() + 2);
        }
    }

    let mut width = (max_chars as f64 * tokens.char_width(tokens.font_size) + tokens.px(2.5))
        .max(min_width)
        .min(tokens.px(32.5));

    // First line at full line height, subsequent lines (subtitles) a touch smaller,
    // plus vertical padding.
    let subtitle_line_height = tokens.line_height(tokens.font_size * 0.83);
    let total_h = if line_count <= 1 {
        tokens.line_height(tokens.font_size) + tokens.px(2.0)
    } else {
        tokens.line_height(tokens.font_size)
            + (line_count - 1) as f64 * subtitle_line_height
            + tokens.px(2.25)
    };
    let mut height = total_h.max(min_height);

    if is_diamond {
        // Diamond (rhombus) shapes require inscribed rectangle clearance:
        // (w_text / W) + (h_text / H) <= 1.0. A factor of 2.3x ensures
        // w_text/W + h_text/H <= 0.87, keeping text comfortably within the diagonal
        // boundaries. This is an intrinsic property of the rhombus's own geometry
        // (not a "pixel" tuning value), so it stays a plain ratio rather than a token.
        width = (width * 2.3).max(tokens.px(22.5));
        height = (height * 2.2).max(tokens.px(12.5));
    } else if is_ellipse {
        // Elliptical shapes (queues) have curved boundaries:
        // (w_text / W)^2 + (h_text / H)^2 <= 1.0 — likewise intrinsic to the ellipse.
        width = (width * 1.35).max(tokens.px(16.25));
        height = (height * 1.30).max(tokens.px(8.125));
    } else if is_db {
        // Database cylinders have a top elliptical disc; add headroom so text is
        // centered safely within the cylindrical body below the rim.
        height = (height + tokens.px(2.0)).max(tokens.px(8.125));
        width = width.max(tokens.px(16.25));
    }

    (snap(width, tokens), snap(height, tokens))
}

/// Snaps a dimension up to the nearest `unit / 0.8` pixel grid (10px at the default
/// 8px unit) — keeps node dimensions landing on tidy round numbers instead of odd
/// fractional pixel counts, purely cosmetic but cheap to keep.
fn snap(value: f64, tokens: &DesignTokens) -> f64 {
    let grid = tokens.unit / 0.8;
    (value / grid).ceil() * grid
}
