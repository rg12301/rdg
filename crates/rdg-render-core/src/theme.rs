//! Themes: every visual design value — colours, fonts, stroke widths, corner radii,
//! icon sizes, what each node type and edge style looks like — as data, not code.
//!
//! A theme is a YAML document (see `themes/*.yaml` for the built-ins). A diagram picks
//! one by name (`theme: dark`), or extends one and overrides any subset of its values:
//!
//! ```yaml
//! theme:
//!   extends: dark
//!   font: {family: "Inter, sans-serif"}
//!   categories: {security: {stroke: "#f43f5e"}}
//! ```
//!
//! Overrides are merged key by key into the base, and the result is checked strictly —
//! an unknown key is an error naming it, like everywhere else in rdg.
//!
//! **Colour carries meaning.** Nodes belong to a *category* (frontend, backend,
//! database, …); a category owns one hue, used for the node's tint and stroke, its
//! group, and the edges whose style points at it (`async` → the message-bus hue). Black
//! and white themes keep the same categories and tell them apart with glyphs instead.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};

/// Built-in themes, by name.
pub const BUILTIN: &[(&str, &str)] = &[
    ("light", include_str!("../themes/light.yaml")),
    ("dark", include_str!("../themes/dark.yaml")),
    ("mono-light", include_str!("../themes/mono-light.yaml")),
    ("mono-dark", include_str!("../themes/mono-dark.yaml")),
    ("classic", include_str!("../themes/classic.yaml")),
];

/// The theme used when a diagram names none.
pub const DEFAULT_THEME: &str = "light";

/// Older theme names that still resolve.
fn alias(name: &str) -> &str {
    match name {
        "standard" => "light",
        "bw" | "mono" | "black-white" => "mono-light",
        other => other,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IconStyleName {
    /// Full-colour logos.
    Color,
    /// Single-colour glyphs in the theme's ink.
    Mono,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    /// Theme name (informational).
    pub name: String,
    /// Light or dark surface.
    pub mode: Mode,
    /// Honour colours a diagram sets on nodes, groups and edges (`color:`). Black and
    /// white themes turn this off so their output stays monochrome.
    pub custom_colors: bool,
    pub font: Fonts,
    pub canvas: Canvas,
    pub text: TextColors,
    pub title: Title,
    pub node: NodeStyle,
    pub group: GroupStyle,
    pub edge: EdgeBase,
    pub edge_label: EdgeLabel,
    pub badge: Badge,
    pub icon: IconTheme,
    pub legend: Legend,
    /// Semantic categories, by name.
    pub categories: BTreeMap<String, Category>,
    /// Node `type` → category name. Types not listed use `default_category`.
    pub type_categories: BTreeMap<String, String>,
    pub default_category: String,
    /// `edge_style` → look. Styles not listed use `edge` as is.
    pub edge_styles: BTreeMap<String, EdgeLook>,
    /// Node `type` → shape: `card`, `cylinder`, `ellipse`, `diamond`. Types not listed
    /// are cards. (Flowchart start/end/choice markers keep their own small shapes.)
    pub shapes: BTreeMap<String, String>,
    /// Sequence diagrams: lifelines, activations, notes, fragments, dividers.
    pub sequence: SequenceTheme,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SequenceTheme {
    /// Minimum gap between neighbouring participant boxes, px.
    pub participant_gap: f64,
    /// Vertical room between consecutive messages, beyond their labels, px.
    pub message_gap: f64,
    /// Participant types drawn as a stick figure.
    pub actor_types: Vec<String>,
    pub lifeline: SeqLine,
    pub activation: SeqActivationStyle,
    pub note: SeqNoteStyle,
    pub fragment: SeqFragmentStyle,
    pub divider: SeqLine,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeqLine {
    pub color: String,
    pub width: f64,
    /// Dash pattern, e.g. `"6 6"`; null for solid.
    pub dash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeqActivationStyle {
    pub width: f64,
    pub fill: String,
    /// Outline in the participant's category colour (else `stroke`).
    pub category_stroke: bool,
    pub stroke: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeqNoteStyle {
    pub fill: String,
    pub stroke: String,
    pub text: String,
    /// Note text wraps at this many characters.
    pub wrap_chars: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeqFragmentStyle {
    pub stroke: String,
    pub width: f64,
    /// Fill of the keyword tab (`alt`, `loop`, …).
    pub tab_fill: String,
    pub tab_text: String,
    /// Guard text (`[valid token]`).
    pub guard_color: String,
    /// Dash of the `else` / `and` separators.
    pub separator_dash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fonts {
    /// CSS font stack for everything.
    pub family: String,
    /// Font stack for `code` spans.
    pub code_family: String,
    pub title_size: f64,
    pub description_size: f64,
    pub node_title_size: f64,
    pub node_detail_size: f64,
    pub edge_label_size: f64,
    pub group_title_size: f64,
    pub badge_size: f64,
    /// Proportional glyph width as a fraction of the font size (0.6 for monospace) — how
    /// wide rdg assumes text is when sizing boxes and placing labels.
    pub char_width_ratio: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Canvas {
    pub background: String,
    /// Dot grid colour, or null for none (SVG output; draw.io uses its own editor grid).
    pub grid: Option<String>,
    pub grid_spacing: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextColors {
    pub primary: String,
    pub muted: String,
    pub dim: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Title {
    pub color: String,
    pub description_color: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeStyle {
    pub corner_radius: f64,
    /// Depth of a database cylinder's elliptical top/bottom cap, px.
    pub cylinder_cap: f64,
    pub stroke_width: f64,
    pub shadow: bool,
    /// Detail lines under the title.
    pub detail_color: String,
    /// Title weight: true = bold.
    pub title_bold: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupStyle {
    pub stroke_width: f64,
    /// Dash pattern (e.g. "6 4"), or null for a solid border.
    pub dash: Option<String>,
    /// Opacity of the group's tint (its category's stroke colour), 0–1.
    pub fill_opacity: f64,
    pub corner_radius: f64,
    pub title_uppercase: bool,
    pub title_bold: bool,
    /// Category for groups that don't name one.
    pub default_category: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EdgeBase {
    pub color: String,
    pub width: f64,
    /// draw.io marker name for the arrowhead (SVG draws the matching shape).
    pub head: String,
    pub head_size: f64,
    /// Round the corners of routed edges.
    pub rounded: bool,
    /// Draw a hop where edges cross.
    pub line_jumps: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EdgeLabel {
    pub color: String,
    /// Background behind label text (usually the canvas colour).
    pub background: String,
    /// Draw labels in the colour of their edge instead of `color`.
    pub use_edge_color: bool,
    /// Labels longer than this many characters are wrapped onto two lines (an
    /// explicit `\n` in the label always wins).
    pub wrap_chars: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Badge {
    pub fill: String,
    pub text: String,
    pub radius: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IconTheme {
    /// `color` (full-colour logos) or `mono` (glyphs in `mono_color`).
    pub style: IconStyleName,
    pub mono_color: String,
    /// Icon drawn inside a card, inline before its title, px.
    pub size: f64,
    /// Space between that icon and the title text, px.
    pub gap: f64,
    /// A node drawn *as* its logo (no box), px.
    pub node_size: f64,
    /// The invisible boundary around a logo node's logo that arrows start and end on.
    pub halo: Halo,
    /// Node types drawn as their logo when they have one (e.g. `database` with
    /// `db_type: postgres`). A node's own `display:` wins.
    pub node_types: Vec<String>,
    /// Put the category glyph in cards that have no brand icon.
    pub category_glyphs: bool,
    /// Plate drawn behind an icon node's logo, or null — dark themes use a light one
    /// so dark brand marks (Kafka, GitHub, …) stay visible.
    pub backdrop: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Halo {
    /// `circle` or `square`.
    pub shape: HaloShape,
    /// Gap between the logo and the halo, px.
    pub padding: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HaloShape {
    Circle,
    Square,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Legend {
    /// Draw a legend of the categories and edge styles used (a diagram's `legend:` wins).
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Category {
    /// Legend label.
    pub label: String,
    /// Tint fill; `#rrggbbaa` for transparency.
    pub fill: String,
    pub stroke: String,
    /// Title text colour inside the node (default: theme text).
    #[serde(default)]
    pub text: Option<String>,
    /// Glyph key (see `rdg --list-icons`) marking this category.
    pub glyph: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EdgeLook {
    /// Hex colour, or `@<category>` for that category's stroke.
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub width: Option<f64>,
    #[serde(default)]
    pub dash: Option<String>,
    /// draw.io marker name (`blockThin`, `open`, `block`, `ERmany`, `diamond`, `none`, …).
    #[serde(default)]
    pub head: Option<String>,
    #[serde(default)]
    pub head_fill: Option<bool>,
    #[serde(default)]
    pub tail: Option<String>,
    #[serde(default)]
    pub tail_fill: Option<bool>,
    /// Legend label (styles without one stay out of the legend).
    #[serde(default)]
    pub label: Option<String>,
}

/// A colour split for renderers: `#rrggbb` plus opacity 0–1 (from `#rrggbbaa`).
pub fn split_alpha(c: &str) -> (String, f64) {
    let h = c.trim();
    if h.len() == 9 && h.starts_with('#') {
        let a = u8::from_str_radix(&h[7..9], 16).map_or(1.0, |v| v as f64 / 255.0);
        (h[..7].to_string(), a)
    } else {
        (h.to_string(), 1.0)
    }
}

/// Recursively merges `over` into `base` (maps merge key by key; anything else replaces).
fn merge(base: &mut Value, over: Value) {
    match (base, over) {
        (Value::Mapping(b), Value::Mapping(o)) => {
            for (k, v) in o {
                match b.get_mut(&k) {
                    Some(slot) => merge(slot, v),
                    None => {
                        b.insert(k, v);
                    }
                }
            }
        }
        (slot, v) => *slot = v,
    }
}

fn builtin_value(name: &str) -> Result<Value> {
    let key = alias(name);
    let Some((_, src)) = BUILTIN.iter().find(|(n, _)| *n == key) else {
        let names: Vec<&str> = BUILTIN.iter().map(|(n, _)| *n).collect();
        bail!("unknown theme `{name}` (built-in: {})", names.join(", "));
    };
    Ok(serde_yaml::from_str(src).expect("built-in theme parses"))
}

/// Resolves a theme value: a mapping with an optional `extends` (default: the default
/// theme, or `fallback` when given) plus overrides; a base theme may itself extend
/// another.
fn resolve_value(spec: Value, fallback: &str, depth: usize) -> Result<Value> {
    if depth > 8 {
        bail!("theme `extends` chain is too deep (a cycle?)");
    }
    let Value::Mapping(mut m) = spec else { bail!("a theme must be a mapping") };
    let parent = m.remove(Value::from("extends"));
    let mut base = match parent {
        Some(Value::String(p)) => resolve_value(builtin_value(&p)?, fallback, depth + 1)?,
        Some(_) => bail!("theme `extends` must be a theme name"),
        None if m.contains_key(Value::from("categories")) && m.contains_key(Value::from("font")) => {
            Value::Mapping(Mapping::new()) // a complete theme on its own
        }
        None => resolve_value(builtin_value(fallback)?, fallback, depth + 1)?,
    };
    merge(&mut base, Value::Mapping(m));
    Ok(base)
}

impl Theme {
    /// A built-in theme by name.
    pub fn builtin(name: &str) -> Result<Theme> {
        Self::from_value(builtin_value(name)?, alias(name))
    }

    /// A theme from YAML text: a full theme, or `extends: <name>` plus overrides.
    pub fn from_yaml(src: &str) -> Result<Theme> {
        let v: Value = serde_yaml::from_str(src).context("theme is not valid YAML")?;
        Self::from_value(v, DEFAULT_THEME)
    }

    /// A theme from a parsed YAML value — a theme name string, or a mapping as in
    /// [`Theme::from_yaml`]; `fallback` is what a mapping without `extends` builds on.
    pub fn from_value(v: Value, fallback: &str) -> Result<Theme> {
        let v = match v {
            Value::String(name) => builtin_value(&name)?,
            other => other,
        };
        let resolved = resolve_value(v, fallback, 0)?;
        let theme: Theme = serde_yaml::from_value(resolved).context("invalid theme")?;
        theme.check()?;
        Ok(theme)
    }

    /// Cross-references: every category a type, group default or edge style names exists.
    fn check(&self) -> Result<()> {
        let known = |c: &str| self.categories.contains_key(c);
        for (t, c) in &self.type_categories {
            if !known(c) {
                bail!("type_categories.{t}: unknown category `{c}`");
            }
        }
        for c in [&self.default_category, &self.group.default_category] {
            if !known(c) {
                bail!("unknown default category `{c}`");
            }
        }
        for (t, sh) in &self.shapes {
            if !["card", "cylinder", "ellipse", "diamond"].contains(&sh.as_str()) {
                bail!("shapes.{t}: unknown shape `{sh}` (card, cylinder, ellipse, diamond)");
            }
        }
        for (s, l) in &self.edge_styles {
            if let Some(cat) = l.color.as_deref().and_then(|c| c.strip_prefix('@')) {
                if !known(cat) {
                    bail!("edge_styles.{s}.color: unknown category `@{cat}`");
                }
            }
        }
        Ok(())
    }

    /// This theme serialised back to YAML — a starting point for a custom theme.
    pub fn to_yaml(&self) -> String {
        serde_yaml::to_string(self).unwrap_or_default()
    }

    /// Category name for a node: its explicit `category`, else its type's, else the default.
    pub fn category_of<'a>(&'a self, node_type: &str, explicit: Option<&'a str>) -> &'a str {
        if let Some(c) = explicit.filter(|c| self.categories.contains_key(*c)) {
            return c;
        }
        self.type_categories
            .get(&node_type.to_ascii_lowercase())
            .map_or(self.default_category.as_str(), String::as_str)
    }

    pub fn category(&self, name: &str) -> &Category {
        self.categories.get(name).unwrap_or_else(|| &self.categories[&self.default_category])
    }

    /// A colour reference: `@category` → that category's stroke, else as written.
    pub fn color_ref(&self, c: &str) -> String {
        match c.strip_prefix('@') {
            Some(cat) => self.category(cat).stroke.clone(),
            None => c.to_string(),
        }
    }

    /// The fully resolved look of an edge style (theme base + that style's overrides).
    pub fn edge_look(&self, style: Option<&str>) -> ResolvedEdge {
        let key = style.map(str::to_ascii_lowercase);
        let look = key.as_deref().and_then(|k| self.edge_styles.get(k)).cloned().unwrap_or_default();
        ResolvedEdge {
            color: look.color.as_deref().map_or_else(|| self.edge.color.clone(), |c| self.color_ref(c)),
            width: look.width.unwrap_or(self.edge.width),
            dash: look.dash.clone(),
            head: look.head.clone().unwrap_or_else(|| self.edge.head.clone()),
            head_fill: look.head_fill.unwrap_or(true),
            tail: look.tail.clone(),
            tail_fill: look.tail_fill.unwrap_or(true),
        }
    }

    /// The icon style renderers pass to `rdg_icons`.
    pub fn icon_style(&self) -> rdg_icons::IconStyle<'_> {
        match self.icon.style {
            IconStyleName::Color => rdg_icons::IconStyle::Color,
            IconStyleName::Mono => rdg_icons::IconStyle::Mono(&self.icon.mono_color),
        }
    }

    /// Copies the theme values that change geometry (text sizes, icon sizes, badge
    /// size) into the layout's design tokens, so boxes are sized for what's drawn.
    pub fn apply_to_tokens(&self, t: &mut rdg_layout::DesignTokens) {
        t.font_size = self.font.node_title_size;
        t.char_width_ratio = self.font.char_width_ratio;
        t.detail_font_size = self.font.node_detail_size;
        t.edge_label_font_size = self.font.edge_label_size;
        t.title_font_size = self.font.title_size;
        t.group_title_font_size = self.font.group_title_size;
        t.icon_size = self.icon.size;
        t.icon_reserve = self.icon.size + self.icon.gap;
        t.icon_node_size = self.icon.node_size;
        t.icon_halo_padding = self.icon.halo.padding;
        t.icon_halo_circle = self.icon.halo.shape == HaloShape::Circle;
        t.edge_label_wrap_chars = self.edge_label.wrap_chars;
        t.seq_participant_gap = self.sequence.participant_gap;
        t.seq_message_gap = self.sequence.message_gap;
        t.seq_activation_width = self.sequence.activation.width;
        t.seq_note_wrap_chars = self.sequence.note.wrap_chars;
        t.badge_radius = self.badge.radius;
        t.badge_font_size = self.font.badge_size;
        t.cylinder_cap = self.node.cylinder_cap;
    }
}

impl Default for Theme {
    fn default() -> Self {
        Theme::builtin(DEFAULT_THEME).expect("default theme")
    }
}

/// An edge's final look.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedEdge {
    pub color: String,
    pub width: f64,
    pub dash: Option<String>,
    pub head: String,
    pub head_fill: bool,
    pub tail: Option<String>,
    pub tail_fill: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_theme_loads() {
        for (name, _) in BUILTIN {
            let t = Theme::builtin(name).unwrap_or_else(|e| panic!("{name}: {e:#}"));
            assert_eq!(&t.name, name);
        }
    }

    #[test]
    fn overrides_merge_into_the_extended_theme() {
        let t = Theme::from_yaml("extends: dark\nfont: {family: Inter}\ncategories: {backend: {stroke: \"#ff0000\"}}\n").unwrap();
        assert_eq!(t.mode, Mode::Dark);
        assert_eq!(t.font.family, "Inter");
        assert_eq!(t.category("backend").stroke, "#ff0000");
        // Untouched values come from `dark`.
        assert_eq!(t.category("backend").fill, Theme::builtin("dark").unwrap().category("backend").fill);
    }

    #[test]
    fn unknown_keys_and_categories_are_errors() {
        assert!(format!("{:#}", Theme::from_yaml("extends: light\nfont: {famliy: x}\n").unwrap_err()).contains("famliy"));
        assert!(Theme::from_yaml("extends: light\ntype_categories: {service: nope}\n").is_err());
        assert!(Theme::builtin("nope").is_err());
    }

    #[test]
    fn edge_styles_resolve_category_colours() {
        let t = Theme::builtin("light").unwrap();
        let a = t.edge_look(Some("async"));
        assert_eq!(a.color, t.category("messagebus").stroke);
        assert!(a.dash.is_some());
        assert_eq!(t.edge_look(None).color, t.edge.color);
    }

    #[test]
    fn alpha_colours_split() {
        assert_eq!(split_alpha("#ff000080"), ("#ff0000".into(), 128.0 / 255.0));
        assert_eq!(split_alpha("#123456"), ("#123456".into(), 1.0));
    }
}
