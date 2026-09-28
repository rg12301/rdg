//! The design token system: the small set of base values every spacing, sizing, and
//! threshold decision in the layout/render pipeline derives from at runtime.
//!
//! Before this module existed, "how much clearance should a resolved node overlap
//! leave", "how far past a marker's external label should its badge sit", "how wide
//! is this table column" and dozens of others like them were each their own
//! independent hardcoded pixel literal, sprinkled across eight-odd files, with no
//! relationship to each other — so retuning "make things a bit more spacious" meant
//! hunting down and editing a dozen unrelated numbers, and the numbers routinely drifted
//! out of sync with each other (the codebase before this module had five different
//! "pixels per character" constants for what is conceptually the same measurement).
//!
//! The fix isn't to turn all ~150 of those literals into ~150 independent config
//! fields — that just relocates the hardcoding into a config file nobody can reason
//! about either. Instead, [`DesignTokens`] holds a handful of genuinely independent
//! *base* values (a spacing unit, a font size, a couple of ratios, a couple of
//! algorithm-quality knobs, a couple of topology thresholds) and every other value
//! this crate and its render backends need is computed *from* those tokens and from
//! the diagram's actual content (label length, field count, node type) at the point
//! it's needed — proportional and content-aware, the way a frontend design system's
//! spacing/type scale works, rather than a wall of independent pixel constants.
//!
//! Deliberately out of scope here: pure SVG/draw.io presentation — stroke widths,
//! corner radii, dash patterns, arrowhead marker glyph geometry, drop-shadow filter
//! parameters, XML protocol boilerplate (draw.io's viewport `dx`/`dy` hints, grid
//! size). Those are visual "skin", closer to a color palette than a layout threshold —
//! nobody retunes a corner radius the way they retune spacing to fix a cramped
//! diagram — and folding them in here would balloon this module for little real
//! benefit. Also out of scope: floating-point epsilon/stability guards (`1e-6` and
//! similar) — those aren't design values, they're numerical safety margins.

use serde::{Deserialize, Serialize};

/// The base design tokens. Every field has a sensible default (see [`Default`]); most
/// diagrams never need to override any of them, but every one of them *can* be
/// overridden — via `--design-config <file>` on the CLI or a `design:` block in the
/// diagram YAML — without recompiling `rdg`.
///
/// Grouped into typography, spacing, layout-quality, and topology-threshold tokens.
/// Helper methods (e.g. [`DesignTokens::char_width`]) compute the proportional,
/// content-aware values everything downstream actually consumes — callers should
/// reach for those instead of hand-deriving `unit * N` themselves where a helper
/// already exists.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DesignTokens {
    // -- Typography -----------------------------------------------------------------
    /// Base body font size, in pixels. Every other font size in a rendered diagram
    /// (subtitle, group title, table field, badge) is a ratio of this one, not an
    /// independent literal — see [`DesignTokens::font_size_for`].
    pub font_size: f64,
    /// Average glyph advance width as a fraction of font size, for a proportionally
    /// spaced UI sans font at normal weight (Inter/Helvetica). Used to *estimate*
    /// box width from character count before any text is actually shaped — this is
    /// necessarily an approximation (real glyph widths vary), tuned against this
    /// font family rather than derived from first principles.
    pub char_width_ratio: f64,
    /// Line height as a multiple of font size.
    pub line_height_ratio: f64,
    /// Max characters per wrapped line for diamond/rhombus shapes (narrower than
    /// other shapes because a diamond's inscribed rectangle is much smaller than its
    /// bounding box). Shared by the layout engine's own sizing estimate and both
    /// render backends' text wrapping — previously three independent, silently
    /// drifting copies of "roughly how wide is a diamond's text area".
    pub wrap_chars_diamond: usize,
    /// Max characters per wrapped line for every other node shape.
    pub wrap_chars_normal: usize,

    // -- Spacing scale ----------------------------------------------------------------
    /// The base spacing unit, in pixels. Nearly every padding, margin, gap, and
    /// clearance value in the layout engine is a small multiple of this one unit —
    /// the same role a `4px`/`8px` base grid plays in a frontend spacing scale.
    pub unit: f64,
    /// Outer canvas margin, X axis, as a multiple of [`Self::unit`].
    pub margin_x_units: f64,
    /// Outer canvas margin, Y axis, as a multiple of [`Self::unit`].
    pub margin_y_units: f64,
    /// Gap between adjacent group containers (both axes), as a multiple of
    /// [`Self::unit`].
    pub group_gap_units: f64,
    /// Group container internal padding — left/right, and bottom — as a multiple of
    /// [`Self::unit`].
    pub group_pad_units: f64,
    /// Minimum straight clearance stub extending perpendicularly from a component
    /// face before an edge's first bend, as a multiple of [`Self::unit`].
    pub stub_clearance_units: f64,
    /// Floor on the clear distance between two adjacent ranks, as a multiple of
    /// [`Self::unit`]. An edge needs a stub off each face before it can bend, so a gap
    /// under two stubs leaves the final (arrowhead) segment nearly nothing — see
    /// [`DesignTokens::min_rank_gap`], which also enforces the two-stub floor itself so
    /// overriding `stub_clearance_units` can never leave this behind it.
    pub min_rank_gap_units: f64,
    /// Vertical pitch between parallel edge channels sharing one rank gap, as a multiple
    /// of [`Self::unit`]. The layout reserves `channel_pitch * (edges + 1)` of gap so the
    /// router's staggered channels never have to crush together.
    pub channel_pitch_units: f64,
    /// Smallest distance between two connection points on one node face, as a multiple
    /// of [`Self::unit`]. Roughly an arrowhead's width plus air: closer than this and
    /// neighbouring arrows read as one thick line. Face selection treats a face that
    /// would drop below it as full and spills further edges onto another face.
    pub min_port_pitch_units: f64,
    /// Master switch for the final polish pass (`rdg-render-core::polish`): small,
    /// guarded, deterministic adjustments to edge ports and waypoints that remove micro-jogs
    /// and avoidable crossings without changing the layout.
    pub polish_enabled: bool,
    /// Whether the final stage snaps node boxes, ports and waypoints to whole pixels (only
    /// effective when [`Self::polish_enabled`] is on). draw.io rounds node geometry to
    /// integers but writes ports as fractions and waypoints unrounded, which leaves up to
    /// ~0.6px of skew between a port and the line leaving it.
    pub polish_snap: bool,
    /// Shortest interior segment that still reads as intentional, as a multiple of
    /// [`Self::unit`]. Shorter than this is a "micro-jog" the polish pass tries to remove.
    pub polish_min_jog_units: f64,
    /// Upper bound on polish sweeps over the diagram (it also stops early when a sweep
    /// changes nothing).
    pub polish_max_passes: u32,
    /// Longest an edge may grow when polish re-routes it, as a multiple of its current
    /// length (plus a fixed slack of a few units). Removing a crossing is worth a small
    /// detour, not sending an edge around the whole diagram.
    pub polish_max_detour_ratio: f64,
    /// Budget of edge re-routes polish may spend in one run. A hard, deterministic cap (a
    /// count, not a clock) so a very large or very dense diagram can't make the final pass
    /// dominate render time; it stops trying fixes once the budget is spent.
    pub polish_max_reroutes: u32,
    /// Farthest polish may slide an edge's port along its face, as a multiple of
    /// [`Self::unit`].
    pub polish_max_port_shift_units: f64,
    /// Extra A* route cost charged per direction change — higher favors fewer,
    /// straighter bends over a shorter but more zig-zag path.
    pub astar_bend_penalty: f64,
    /// Extra A* route cost charged per obstacle-boundary line a candidate segment
    /// runs directly along — discourages a route that visually hugs a box's edge.
    pub astar_hug_penalty: f64,
    /// Caps how many nearby obstacles feed the A* visibility grid, so a very large
    /// diagram can't blow up the grid's point count.
    pub astar_max_obstacles: usize,
    /// Extra slack (beyond the exact penetration depth) `hillclimb`'s overlap
    /// resolution leaves between two boxes it just separated, as a multiple of
    /// [`Self::unit`] — the difference between two boxes technically not overlapping
    /// and two boxes with genuine visual breathing room between them. Acts as a floor
    /// under [`Self::node_clearance_fraction`]'s proportional clearance, not the only
    /// source of it — see that field.
    pub overlap_clearance_units: f64,
    /// Minimum gap `hillclimb` maintains between *any* two node boxes (not just ones
    /// connected by an edge), as a fraction of their own average size — e.g. `0.18`
    /// means two ~150px boxes keep at least ~27px between them, while two ~400px
    /// boxes keep ~72px, so bigger components read as proportionally as breathable as
    /// smaller ones instead of all sharing the same flat pixel gap regardless of how
    /// large the boxes actually are. This is a genuinely proportional value, unlike
    /// the `_units` tokens above (which are multiples of the flat base `unit`) — the
    /// single knob to turn for "make every force-directed diagram more/less
    /// breathable" without touching anything else. Floored by
    /// [`Self::overlap_clearance_units`] so small nodes still get a sane minimum.
    pub node_clearance_fraction: f64,
    /// Gap between sibling nodes on the same Sugiyama rank, as a fraction of the
    /// node's own size along that axis — the proportional counterpart to an explicit
    /// `--node-spacing`/`node_spacing:` override, used only when neither is given.
    /// Same reasoning as [`Self::node_clearance_fraction`], for the layered (not
    /// force-directed) layout path.
    pub node_spacing_fraction: f64,
    /// Gap between successive Sugiyama ranks, as a fraction of the node's own size
    /// along that axis — the proportional counterpart to an explicit
    /// `--rank-spacing`/`rank_spacing:` override, used only when neither is given.
    pub rank_spacing_fraction: f64,
    /// Extra group container padding, on top of [`Self::group_pad_units`]'s flat
    /// floor, as a fraction of the group's own content bounding box — a group
    /// wrapping a handful of large components gets proportionally more breathing room
    /// around them than one wrapping a single small one, rather than every group
    /// sharing the same fixed padding regardless of how much it actually contains.
    pub group_pad_fraction: f64,

    // -- Node/marker geometry ---------------------------------------------------------
    /// Diameter of a start/initial-state marker, as a multiple of [`Self::unit`].
    pub start_marker_units: f64,
    /// Diameter of an end/final-state marker, as a multiple of [`Self::unit`].
    pub end_marker_units: f64,
    /// Diameter of a choice/branch marker, as a multiple of [`Self::unit`].
    pub choice_marker_units: f64,
    /// How far past its own edge a tiny marker shape's `exitY` connection point is
    /// pushed, as a multiple of the marker's *own* height (not [`Self::unit`] — this
    /// is a proportion of the shape it belongs to, exactly the "relative, not
    /// pixel" case: a bigger marker token should still clear its own label without
    /// this ratio needing to change). `2.0` means "one full marker-height past the
    /// far edge", which is what actually clears the external label rendered below a
    /// start/end/choice marker — see `rdg-render-drawio`'s `style.rs` doc comment for
    /// why draw.io needs this expressed as an extrapolated `exitY` fraction rather
    /// than a literal pixel offset.
    pub marker_label_clearance_ratio: f64,

    // -- Layout quality / algorithm budget ---------------------------------------------
    /// Self-review retry budget: how many times [`compute_reviewed_layout`] (in
    /// `rdg-render-core`) will retry a layout with wider spacing before giving up and
    /// returning its best attempt.
    ///
    /// [`compute_reviewed_layout`]: ../rdg_render_core/review/fn.compute_reviewed_layout.html
    pub max_review_passes: u32,
    /// Multiplier applied to `rank_spacing`/`node_spacing` before each self-review
    /// retry.
    pub retry_spacing_factor: f64,
    /// `hillclimb::refine`'s local-search pass budget.
    pub max_hillclimb_passes: u32,
    /// Force-directed/fCoSE physics relaxation iteration count.
    pub physics_iterations: u32,
    /// Barnes-Hut quadtree opening angle (theta) — smaller is more accurate and
    /// slower, larger is faster and coarser. `0.8` is the standard textbook default.
    pub barnes_hut_theta: f64,
    /// Edge-spring stiffness in the force-directed/fCoSE physics simulation.
    pub attraction_k: f64,
    /// Extra pull strength toward a node's cluster centroid (grouped/compound
    /// layouts), relative to [`Self::attraction_k`].
    pub cohesion_k: f64,
    /// Repulsion strength scales with `ideal_edge_length` squared (dimensionally,
    /// repulsion ~ k / d^2, so k ~ d^2 holds the force at d = ideal_len roughly
    /// constant regardless of scale) — this is that proportionality factor.
    pub repulsion_strength_factor: f64,
    /// Per-iteration cooling multiplier applied to the simulation's max step size —
    /// closer to 1.0 cools more slowly (more exploration, slower convergence).
    pub cooling_rate: f64,

    // -- Topology thresholds (`rdg-dispatch`) ------------------------------------------
    /// Edges-per-node density above which a graph is dispatched to force-directed
    /// layout instead of Sugiyama.
    pub density_threshold: f64,
    /// Node count above which a graph is treated as "massive" and force-directed
    /// layout is used regardless of its other properties.
    pub massive_node_threshold: usize,
    /// Node count above which the edge router switches from the cheap corridor
    /// heuristic to the more expensive but globally obstacle-aware A* search.
    pub obstacle_dense_node_threshold: usize,

    // -- Canvas sanity bounds (`rdg-render-core::review`) ------------------------------
    /// Canvas aspect ratio (width / height) below which the layout is flagged as too
    /// lopsided to read as a diagram.
    pub min_aspect_ratio: f64,
    /// Canvas aspect ratio (width / height) above which the layout is flagged as too
    /// lopsided to read as a diagram.
    pub max_aspect_ratio: f64,

    // -- Edge routing quality (`rdg-render-core::routing`/`review`) --------------------
    /// A shared routing corridor (see `plan_all_edge_routes` Step 4's bucketing) with
    /// more parallel edges than this is flagged as an `AnomalyKind::CongestedCorridor`
    /// bottleneck.
    pub congested_corridor_threshold: usize,
    /// An edge's waypoint count beyond the minimum its face-pair geometrically needs,
    /// past which it's flagged as an `AnomalyKind::ExcessiveBend` bottleneck.
    pub excessive_bend_slack: usize,
}

impl Default for DesignTokens {
    fn default() -> Self {
        Self {
            font_size: 12.0,
            char_width_ratio: 0.6,
            line_height_ratio: 1.35,
            wrap_chars_diamond: 14,
            wrap_chars_normal: 22,

            unit: 8.0,
            margin_x_units: 3.0,
            margin_y_units: 3.5,
            group_gap_units: 8.0,
            group_pad_units: 4.0,
            stub_clearance_units: 3.0,
            min_rank_gap_units: 6.0,
            channel_pitch_units: 1.75,
            min_port_pitch_units: 1.5,
            polish_enabled: true,
            polish_snap: true,
            polish_min_jog_units: 1.5,
            polish_max_passes: 6,
            polish_max_reroutes: 1500,
            polish_max_detour_ratio: 1.3,
            polish_max_port_shift_units: 6.0,
            astar_bend_penalty: 40.0,
            astar_hug_penalty: 15.0,
            astar_max_obstacles: 15,
            overlap_clearance_units: 2.5,
            node_clearance_fraction: 0.18,
            node_spacing_fraction: 0.4,
            rank_spacing_fraction: 0.7,
            group_pad_fraction: 0.07,

            start_marker_units: 3.5,
            end_marker_units: 4.0,
            choice_marker_units: 4.5,
            marker_label_clearance_ratio: 2.0,

            max_review_passes: 3,
            retry_spacing_factor: 1.25,
            max_hillclimb_passes: 25,
            physics_iterations: 300,
            barnes_hut_theta: 0.8,
            attraction_k: 0.06,
            cohesion_k: 1.4,
            repulsion_strength_factor: 1.55,
            cooling_rate: 0.97,

            density_threshold: 2.0,
            massive_node_threshold: 10_000,
            obstacle_dense_node_threshold: 20,

            min_aspect_ratio: 0.15,
            max_aspect_ratio: 6.0,

            congested_corridor_threshold: 4,
            excessive_bend_slack: 2,
        }
    }
}

impl DesignTokens {
    /// Estimated advance width of one character at `font_size`, in pixels — the
    /// single proportional replacement for what used to be five unrelated
    /// "pixels-per-character" literals (6.2, 6.5, 7.0, 7.5, 8.5) scattered across the
    /// sizing and rendering code, one per call site that happened to need it.
    pub fn char_width(&self, font_size: f64) -> f64 {
        font_size * self.char_width_ratio
    }

    /// Line height at `font_size`, in pixels.
    pub fn line_height(&self, font_size: f64) -> f64 {
        font_size * self.line_height_ratio
    }

    /// `self.unit * units` — the standard way every spacing/clearance token above
    /// turns into a pixel value. A tiny helper, but it means every call site reads as
    /// "N units" rather than repeating the multiplication (and rounding mode) itself.
    pub fn px(&self, units: f64) -> f64 {
        self.unit * units
    }

    pub fn margin_x(&self) -> f64 {
        self.px(self.margin_x_units)
    }
    pub fn margin_y(&self) -> f64 {
        self.px(self.margin_y_units)
    }
    pub fn group_gap(&self) -> f64 {
        self.px(self.group_gap_units)
    }
    pub fn group_pad(&self) -> f64 {
        self.px(self.group_pad_units)
    }
    /// Group container top padding — room for the title header row, so this is
    /// derived from the title's own line height rather than being an independent
    /// spacing multiple like the other three sides.
    pub fn group_pad_top(&self) -> f64 {
        self.line_height(self.font_size) + self.px(self.group_pad_units)
    }
    /// Content-aware group side/bottom padding: [`Self::group_pad`]'s flat floor,
    /// widened by [`Self::group_pad_fraction`] of the group's own content bounding
    /// box — see that field's docs. `content_w`/`content_h` are the group's member
    /// nodes' combined bounding box, *before* padding.
    pub fn group_pad_for(&self, content_w: f64, content_h: f64) -> f64 {
        let scale = (content_w + content_h) * 0.5;
        // Whole pixels: a group box is `node bbox - pad`, so an integer pad keeps its
        // corner on the same pixel grid as the (snapped) nodes inside it — draw.io rounds
        // every cell and group-relative coordinate independently, and a fractional pad is
        // how a child ended up half a pixel off its container.
        (scale * self.group_pad_fraction).max(self.group_pad()).round()
    }
    /// Content-aware counterpart to [`Self::group_pad_top`] — see
    /// [`Self::group_pad_for`].
    pub fn group_pad_top_for(&self, content_w: f64, content_h: f64) -> f64 {
        (self.line_height(self.font_size) + self.group_pad_for(content_w, content_h)).round()
    }
    /// Vertical band reserved above the content for the diagram title.
    pub fn title_band(&self) -> f64 {
        (self.line_height(self.font_size) * 2.2).round()
    }
    /// Smallest allowed gap between adjacent ranks: [`Self::min_rank_gap_units`], but
    /// never less than two clearance stubs (one off each face).
    pub fn min_rank_gap(&self) -> f64 {
        self.px(self.min_rank_gap_units).max(2.0 * self.stub_clearance())
    }
    pub fn channel_pitch(&self) -> f64 {
        self.px(self.channel_pitch_units)
    }
    pub fn polish_min_jog(&self) -> f64 {
        self.px(self.polish_min_jog_units)
    }
    pub fn polish_max_port_shift(&self) -> f64 {
        self.px(self.polish_max_port_shift_units)
    }
    pub fn min_port_pitch(&self) -> f64 {
        self.px(self.min_port_pitch_units)
    }
    pub fn stub_clearance(&self) -> f64 {
        self.px(self.stub_clearance_units)
    }
    pub fn overlap_clearance(&self) -> f64 {
        self.px(self.overlap_clearance_units)
    }
    pub fn start_marker_size(&self) -> f64 {
        self.px(self.start_marker_units)
    }
    pub fn end_marker_size(&self) -> f64 {
        self.px(self.end_marker_units)
    }
    pub fn choice_marker_size(&self) -> f64 {
        self.px(self.choice_marker_units)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_tokens_reproduce_legacy_pixel_values() {
        // Not a correctness requirement (any sane token set is valid) — a guard that
        // the *defaults* land close to the values this module replaced, so switching
        // to token-derived formulas didn't silently reflow every existing diagram.
        let t = DesignTokens::default();
        assert!((t.char_width(12.0) - 7.0).abs() < 0.5, "char width was {}", t.char_width(12.0));
        assert!((t.margin_x() - 24.0).abs() < 1.0);
        assert!((t.group_gap() - 64.0).abs() < 1.0);
        assert!((t.start_marker_size() - 28.0).abs() < 1.0);
        assert!((t.overlap_clearance() - 20.0).abs() < 1.0);
    }

    #[test]
    fn test_px_helper_scales_with_unit() {
        let mut t = DesignTokens::default();
        t.unit = 10.0;
        assert_eq!(t.px(2.0), 20.0);
    }
}
