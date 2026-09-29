//! Style resolution shared by every render backend.
//!
//! This is the single source of truth for "what color/shape does node type X get,
//! in theme Y" and "what stroke/dash pattern does edge style Z get" — each backend
//! (draw.io, SVG) encodes these into its own output format, but the underlying
//! color/shape table lives here so the two backends can't drift apart.

/// Every node `type` the renderers style specially (aliases included). Anything else
/// renders as the neutral default card — valid, but usually a typo worth flagging.
pub const NODE_TYPES: &[&str] = &[
    "proxy", "gateway", "api", "server", "service", "backend", "database", "db", "storage", "table",
    "entity", "record", "queue", "broker", "bus", "function", "lambda", "faas", "decision",
    "condition", "client", "user", "browser", "start", "start_state", "initial", "initial_state",
    "end", "end_state", "final", "final_state", "choice", "branch", "cache", "redis", "memcache",
    "class", "interface", "abstract_class", "struct", "participant", "actor", "default",
];

/// Every `edge_style` the renderers understand (aliases included).
pub const EDGE_STYLES: &[&str] = &[
    "flow", "async", "error", "fallback", "data", "stream", "bidirectional", "bi", "sync", "call",
    "reply", "return", "one_to_many", "many_to_many", "one_to_one", "zero_to_many", "inheritance",
    "realization", "composition", "aggregation", "dependency",
];


/// The drawn outline of a node, as far as where an arrow touches it is concerned.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outline {
    Rect,
    Ellipse,
    Diamond,
    /// A cylinder whose top and bottom are elliptical caps `cap` px deep.
    Cylinder { cap: f64 },
    /// A node drawn as its logo: a `size` px icon centred in an invisible `halo` px
    /// circle (or square) at the top of the box, the label underneath. Arrows start and
    /// end on the halo, not on the logo's own edge.
    Icon { size: f64, halo: f64, circle: bool },
}

/// Default depth of a database cylinder's elliptical cap, px (themes set their own via
/// `node.cylinder_cap`, carried in `DesignTokens::cylinder_cap`).
pub const CYLINDER_CAP: f64 = 8.0;

/// Outline of a node as it is drawn: from its theme-resolved `shape` (see
/// `crate::look::prepare_graph`), else from its type.
pub fn outline_of(nd: &rdg_graph::NodeData, tokens: &rdg_layout::DesignTokens) -> Outline {
    match nd.shape.as_deref() {
        Some("cylinder") => Outline::Cylinder { cap: tokens.cylinder_cap },
        Some("ellipse") | Some("start") | Some("end") => Outline::Ellipse,
        Some("diamond") | Some("choice") => Outline::Diamond,
        Some("icon") => Outline::Icon { size: tokens.icon_node_size, halo: tokens.icon_halo_size(), circle: tokens.icon_halo_circle },
        Some(_) => Outline::Rect,
        None => outline_for(&nd.node_type),
    }
}

/// Outline for a node type (mirrors the shapes both backends draw).
pub fn outline_for(node_type: &str) -> Outline {
    match node_type.to_ascii_lowercase().as_str() {
        "database" | "db" | "storage" | "table" | "entity" | "record" => Outline::Cylinder { cap: CYLINDER_CAP },
        "queue" | "broker" | "bus" | "start" | "start_state" | "initial" | "initial_state" | "end" | "end_state"
        | "final" | "final_state" => Outline::Ellipse,
        "decision" | "condition" | "choice" | "branch" => Outline::Diamond,
        _ => Outline::Rect,
    }
}

/// Top-left of a logo node's logo, relative to the node's own top-left: centred in its
/// halo at the top of the box. (Cards carry their icon inline before the title.)
pub fn logo_offset(width: f64, size: f64, halo: f64) -> (f64, f64) {
    ((width - size) / 2.0, (halo - size) / 2.0)
}

