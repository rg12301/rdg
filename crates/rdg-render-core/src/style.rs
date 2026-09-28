//! Style resolution shared by every render backend.
//!
//! This is the single source of truth for "what color/shape does node type X get,
//! in theme Y" and "what stroke/dash pattern does edge style Z get" — each backend
//! (draw.io, SVG) encodes these into its own output format, but the underlying
//! color/shape table lives here so the two backends can't drift apart.

/// Accent/stroke color for a semantic node type, consistent across every backend.
///
/// Database-like types (`database`/`db`/`storage`/`table`/`entity`/`record`) share one
/// theme-aware accent color rather than each backend keeping its own copy of this table.
pub fn node_accent_color(node_type: &str, theme: &str) -> &'static str {
    let is_dark = theme == "dark";
    match node_type.to_ascii_lowercase().as_str() {
        "proxy" | "gateway" | "api" => "#818cf8",
        "server" | "service" | "backend" => "#34d399",
        "database" | "db" | "storage" | "table" | "entity" | "record" => {
            if is_dark {
                "#38bdf8"
            } else {
                "#0284c7"
            }
        }
        "queue" | "broker" | "bus" => "#fbbf24",
        "cache" | "redis" | "memcache" => "#f87171",
        "function" | "lambda" | "faas" => "#fb923c",
        "decision" | "condition" => "#a78bfa",
        "client" | "user" | "browser" => "#94a3b8",
        "class" | "interface" | "abstract_class" | "struct" => "#6366f1",
        "participant" | "actor" => "#64748b",
        "start" | "start_state" | "initial" | "initial_state" | "end" | "end_state" | "final"
        | "final_state" => "#0f172a",
        "choice" | "branch" => "#a78bfa",
        _ => {
            if is_dark {
                "#475569"
            } else {
                "#cbd5e1"
            }
        }
    }
}

/// Shared stroke/width/dash for a semantic edge style, consistent across every backend.
///
/// Arrowhead/marker selection stays backend-specific (draw.io encodes them as style-string
/// tokens like `startArrow=ERone`, SVG references `<marker>` element ids) since the two
/// backends have no common representation worth extracting — but the underlying color,
/// width, and dash pattern are identical between backends and belong in one place.
#[derive(Debug, Clone, Copy)]
pub struct EdgeStyleColors {
    pub stroke: &'static str,
    pub width: &'static str,
    pub dash: Option<&'static str>,
}

pub fn edge_style_colors(
    edge_style: Option<&str>,
    theme: &str,
    default_edge: &'static str,
) -> EdgeStyleColors {
    let is_dark = theme == "dark";
    match edge_style {
        Some("async") => EdgeStyleColors {
            stroke: "#d97706",
            width: "1.5",
            dash: Some("8 4"),
        },
        Some("error") | Some("fallback") => EdgeStyleColors {
            stroke: "#ef4444",
            width: "1.5",
            dash: Some("6 3"),
        },
        Some("data") | Some("stream") => EdgeStyleColors {
            stroke: "#6366f1",
            width: "2.0",
            dash: None,
        },
        Some("one_to_many") | Some("many_to_many") | Some("one_to_one") | Some("zero_to_many") => {
            EdgeStyleColors {
                stroke: "#0284c7",
                width: "1.5",
                dash: None,
            }
        }
        Some("inheritance") => EdgeStyleColors {
            stroke: "#6366f1",
            width: "1.5",
            dash: None,
        },
        Some("realization") => EdgeStyleColors {
            stroke: "#6366f1",
            width: "1.5",
            dash: Some("6 3"),
        },
        Some("composition") | Some("aggregation") => EdgeStyleColors {
            stroke: if is_dark { "#cbd5e1" } else { "#0f172a" },
            width: "1.5",
            dash: None,
        },
        Some("dependency") => EdgeStyleColors {
            stroke: "#64748b",
            width: "1.5",
            dash: Some("6 3"),
        },
        _ => EdgeStyleColors {
            stroke: default_edge,
            width: "1.5",
            dash: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_database_and_table_share_theme_aware_accent() {
        assert_eq!(node_accent_color("database", "standard"), "#0284c7");
        assert_eq!(node_accent_color("table", "standard"), "#0284c7");
        assert_eq!(node_accent_color("database", "dark"), "#38bdf8");
        assert_eq!(node_accent_color("table", "dark"), "#38bdf8");
    }

    #[test]
    fn test_default_fallback_is_theme_aware() {
        assert_eq!(node_accent_color("mystery", "standard"), "#cbd5e1");
        assert_eq!(node_accent_color("mystery", "dark"), "#475569");
    }

    #[test]
    fn test_composition_stroke_is_theme_aware() {
        let light = edge_style_colors(Some("composition"), "standard", "#64748b");
        let dark = edge_style_colors(Some("composition"), "dark", "#64748b");
        assert_eq!(light.stroke, "#0f172a");
        assert_eq!(dark.stroke, "#cbd5e1");
    }

    #[test]
    fn test_default_edge_uses_provided_fallback_color() {
        let s = edge_style_colors(None, "standard", "#64748b");
        assert_eq!(s.stroke, "#64748b");
        assert!(s.dash.is_none());
    }
}
