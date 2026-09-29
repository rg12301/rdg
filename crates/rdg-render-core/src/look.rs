//! Applies a [`Theme`] to a diagram: what shape, colours and icon each node gets, how
//! each group and edge is drawn, and what the legend lists. Both render backends read
//! these, so draw.io and SVG can't drift apart on what anything looks like.

use rdg_graph::{CompiledGraph, EdgeData, NodeData};
use rdg_schema::GroupDef;

use crate::theme::{ResolvedEdge, Theme, split_alpha};

/// Flowchart marker types (small fixed shapes with their label drawn underneath).
pub fn marker_shape(node_type: &str) -> Option<&'static str> {
    match node_type.to_ascii_lowercase().as_str() {
        "start" | "start_state" | "initial" | "initial_state" => Some("start"),
        "end" | "end_state" | "final" | "final_state" => Some("end"),
        "choice" | "branch" => Some("choice"),
        _ => None,
    }
}

/// Resolves, once and before layout, everything about each node that depends on the
/// theme and changes its geometry: its category, its shape (including whether it is
/// drawn as its logo — an *icon node*), and the icon inside it (its brand mark, else
/// its category glyph). Layout then sizes each box for exactly what will be drawn.
pub fn prepare_graph(theme: &Theme, compiled: &mut CompiledGraph) {
    let mono = theme.icon.style == crate::theme::IconStyleName::Mono;
    let is_sequence = compiled.diagram_type == "sequence";
    let idxs: Vec<_> = compiled.graph.node_indices().collect();
    for idx in idxs {
        let nd = &mut compiled.graph[idx];
        let ty = nd.node_type.to_ascii_lowercase();
        let category = theme.category_of(&ty, nd.category.as_deref()).to_string();
        let brand = nd
            .icon
            .clone()
            .filter(|k| !rdg_icons::is_glyph(k) && rdg_icons::icon_document(k, rdg_icons::IconStyle::Color).is_some());
        let display = nd.display.as_deref().map(str::to_ascii_lowercase);
        let shape = if is_sequence && theme.sequence.actor_types.iter().any(|t| t.eq_ignore_ascii_case(&ty)) {
            "actor".to_string()
        } else if let Some(m) = marker_shape(&ty) {
            m.to_string()
        } else {
            let auto_icon = display.is_none()
                && theme.icon.node_types.iter().any(|t| t.eq_ignore_ascii_case(&ty))
                && brand.as_deref().is_some_and(|k| mono || rdg_icons::has_color_logo(k));
            if display.as_deref() == Some("icon") || auto_icon {
                "icon".to_string()
            } else if !nd.fields.is_empty() {
                "card".to_string()
            } else {
                theme.shapes.get(&ty).cloned().unwrap_or_else(|| "card".to_string())
            }
        };
        // In a sequence diagram every participant is a card (or an actor figure).
        let shape = if is_sequence && shape != "actor" { "card".to_string() } else { shape };
        let is_box = matches!(shape.as_str(), "card" | "cylinder" | "ellipse" | "diamond");
        if nd.icon.is_none() && ((is_box && theme.icon.category_glyphs && nd.fields.is_empty()) || shape == "icon") {
            nd.icon = Some(theme.category(&category).glyph.clone());
        }
        nd.category = Some(category);
        nd.shape = Some(shape);
    }
}

/// A node's resolved look.
#[derive(Debug, Clone)]
pub struct NodeLook {
    /// `card`, `cylinder`, `ellipse`, `diamond`, `icon`, `start`, `end`, `choice`.
    pub shape: String,
    pub fill: String,
    /// 0–1.
    pub fill_opacity: f64,
    pub stroke: String,
    pub stroke_width: f64,
    pub corner_radius: f64,
    pub shadow: bool,
    pub title_color: String,
    pub detail_color: String,
    pub category: String,
}

pub fn node_look(theme: &Theme, nd: &NodeData) -> NodeLook {
    let category = nd.category.clone().unwrap_or_else(|| theme.category_of(&nd.node_type, None).to_string());
    let cat = theme.category(&category);
    let (mut fill, fill_opacity) = split_alpha(&cat.fill);
    // An explicit colour recolours the whole node: its stroke, and its tint at the
    // category's tint strength (so a blue-stroked node isn't filled green).
    let custom = nd.color.clone().filter(|_| theme.custom_colors);
    let stroke = custom.clone().unwrap_or_else(|| cat.stroke.clone());
    if custom.is_some() && fill_opacity < 1.0 {
        fill = split_alpha(&stroke).0;
    }
    let shape = nd.shape.clone().unwrap_or_else(|| "card".into());
    let ink = theme.text.primary.clone();
    let (fill, fill_opacity, stroke) = match shape.as_str() {
        "start" | "end" => (ink.clone(), 1.0, ink.clone()),
        _ => (fill, fill_opacity, stroke),
    };
    NodeLook {
        shape,
        fill,
        fill_opacity,
        stroke,
        stroke_width: theme.node.stroke_width,
        corner_radius: theme.node.corner_radius,
        shadow: theme.node.shadow,
        title_color: cat.text.clone().unwrap_or(ink),
        detail_color: theme.node.detail_color.clone(),
        category,
    }
}

/// A group container's resolved look.
#[derive(Debug, Clone)]
pub struct GroupLook {
    pub stroke: String,
    pub fill: String,
    pub fill_opacity: f64,
    pub stroke_width: f64,
    pub dash: Option<String>,
    pub corner_radius: f64,
    pub title_color: String,
    pub title_bold: bool,
    pub title: String,
}

pub fn group_look(theme: &Theme, g: &GroupDef) -> GroupLook {
    let cat = theme.category(g.category.as_deref().unwrap_or(&theme.group.default_category));
    let stroke = g.color.clone().filter(|_| theme.custom_colors).unwrap_or_else(|| cat.stroke.clone());
    let title = if theme.group.title_uppercase { g.label.to_uppercase() } else { g.label.clone() };
    GroupLook {
        fill: split_alpha(&stroke).0,
        title_color: cat.text.clone().unwrap_or_else(|| stroke.clone()),
        stroke,
        fill_opacity: theme.group.fill_opacity,
        stroke_width: theme.group.stroke_width,
        dash: theme.group.dash.clone(),
        corner_radius: theme.group.corner_radius,
        title_bold: theme.group.title_bold,
        title,
    }
}

/// An edge's final look: its style from the theme, then its own overrides.
pub fn edge_look(theme: &Theme, ed: &EdgeData) -> ResolvedEdge {
    let mut e = theme.edge_look(ed.edge_style.as_deref());
    if let Some(c) = ed.color.as_ref().filter(|_| theme.custom_colors) {
        e.color = theme.color_ref(c);
    }
    if let Some(w) = ed.width {
        e.width = w;
    }
    match ed.line_style.as_deref().map(str::to_ascii_lowercase).as_deref() {
        Some("dashed") => e.dash = Some("6 4".into()),
        Some("dotted") => e.dash = Some("2 3".into()),
        Some("solid") => e.dash = None,
        _ => {}
    }
    if let Some(h) = &ed.head {
        e.head = h.clone();
    }
    if let Some(t) = &ed.tail {
        e.tail = Some(t.clone());
    }
    e
}

/// One legend entry.
#[derive(Debug, Clone)]
pub enum LegendItem {
    Category { label: String, fill: String, fill_opacity: f64, stroke: String, glyph: String },
    Edge { label: String, look: ResolvedEdge },
}

/// Whether to draw a legend: the diagram's `legend:` wins over the theme's default.
pub fn legend_enabled(theme: &Theme, compiled: &CompiledGraph) -> bool {
    // Explicit `legend:` wins; otherwise the theme's default, when there are at least two
    // meanings to explain (a single colour needs no key).
    compiled.legend.unwrap_or_else(|| theme.legend.enabled && legend_items(theme, compiled).len() >= 2)
}

/// Legend entries: every category used by a node, then every labelled edge style used,
/// in order of first appearance.
pub fn legend_items(theme: &Theme, compiled: &CompiledGraph) -> Vec<LegendItem> {
    let mut out = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for idx in compiled.graph.node_indices() {
        let nd = &compiled.graph[idx];
        // Only colours actually on the page: boxes (not logos or markers) that aren't
        // wearing a custom `color:`.
        let coloured = matches!(nd.shape.as_deref(), Some("card" | "cylinder" | "ellipse" | "diamond" | "actor") | None);
        if marker_shape(&nd.node_type).is_some() || !coloured || (theme.custom_colors && nd.color.is_some()) {
            continue;
        }
        let c = nd.category.clone().unwrap_or_else(|| theme.category_of(&nd.node_type, None).to_string());
        if seen.contains(&c) {
            continue;
        }
        seen.push(c.clone());
        let cat = theme.category(&c);
        let (fill, fill_opacity) = split_alpha(&cat.fill);
        out.push(LegendItem::Category {
            label: cat.label.clone(),
            fill,
            fill_opacity,
            stroke: cat.stroke.clone(),
            glyph: cat.glyph.clone(),
        });
    }
    let mut seen_edges: Vec<String> = Vec::new();
    for &e in &compiled.edge_order {
        let Some(ed) = compiled.graph.edge_weight(e) else { continue };
        let Some(style) = ed.edge_style.as_deref().map(str::to_ascii_lowercase) else { continue };
        let Some(label) = theme.edge_styles.get(&style).and_then(|l| l.label.clone()) else { continue };
        if seen_edges.contains(&label) {
            continue;
        }
        seen_edges.push(label.clone());
        out.push(LegendItem::Edge { label, look: theme.edge_look(Some(&style)) });
    }
    out
}

/// Legend block size `(width, height)` for `items`, laid out as one row that wraps at
/// `max_width`.
pub fn legend_size(items: &[LegendItem], theme: &Theme, max_width: f64) -> (f64, f64) {
    let rows = legend_rows(items, theme, max_width);
    let row_h = theme.font.edge_label_size * 1.35 + 10.0;
    let w = rows.iter().map(|r| legend_row_width(r)).fold(legend_heading_width(theme), f64::max);
    let heading = theme.font.group_title_size * 1.35 + 6.0;
    (w, heading + rows.len() as f64 * row_h)
}

/// Space after each legend item, before the next one on its row.
const LEGEND_ITEM_GAP: f64 = 20.0;

/// Drawn width of a legend row (its items, without the gap after the last one).
pub fn legend_row_width(row: &[(&LegendItem, f64)]) -> f64 {
    (row.iter().map(|(_, w)| *w).sum::<f64>() - LEGEND_ITEM_GAP).max(0.0)
}

/// Drawn width of the legend's "Legend" heading.
pub fn legend_heading_width(theme: &Theme) -> f64 {
    "Legend".len() as f64 * theme.font.group_title_size * theme.font.char_width_ratio
}

/// Legend items grouped into rows, each with its drawn width.
pub fn legend_rows<'a>(items: &'a [LegendItem], theme: &Theme, max_width: f64) -> Vec<Vec<(&'a LegendItem, f64)>> {
    let cw = theme.font.edge_label_size * theme.font.char_width_ratio;
    let mut rows: Vec<Vec<(&LegendItem, f64)>> = vec![Vec::new()];
    let mut x = 0.0;
    for it in items {
        let label = match it {
            LegendItem::Category { label, .. } | LegendItem::Edge { label, .. } => label,
        };
        let w = 30.0 + label.chars().count() as f64 * cw + LEGEND_ITEM_GAP;
        if x + w > max_width && !rows.last().unwrap().is_empty() {
            rows.push(Vec::new());
            x = 0.0;
        }
        rows.last_mut().unwrap().push((it, w));
        x += w;
    }
    rows
}
