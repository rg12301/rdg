//! Group container geometry — the one definition of how big a group's box is, where
//! its title sits and how far its border stands off its content. Layout sizes each
//! group's block with it, routing treats the titles as obstacles, and both renderers
//! draw the boxes from [`group_rects`], so none of them can disagree about where a
//! container is.

use std::collections::HashMap;

use petgraph::stable_graph::NodeIndex;

use rdg_graph::CompiledGraph;
use rdg_schema::GroupDef;

use crate::{DesignTokens, NodeLayout};

/// `(x, y, width, height)`.
pub type Rect = (f64, f64, f64, f64);

/// Space between a group's border and its content, per side.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Padding {
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
}

/// Padding for a group whose content (nodes and inner groups) spans `content_w` ×
/// `content_h`: content-aware sides and bottom, and the title row on top.
pub fn group_padding(tokens: &DesignTokens, content_w: f64, content_h: f64) -> Padding {
    let side = tokens.group_pad_for(content_w, content_h);
    Padding { left: side, right: side, top: tokens.group_pad_top_for(content_w, content_h), bottom: side }
}

/// Offset of the title text (its icon first, when it has one) from the box's top-left.
pub fn group_title_inset(tokens: &DesignTokens) -> (f64, f64) {
    (tokens.px(1.5), tokens.px(1.0))
}

/// Size of a group's header icon and the gap after it.
pub fn group_icon_size(tokens: &DesignTokens) -> (f64, f64) {
    (tokens.group_title_font_size + 4.0, 6.0)
}

/// Width of the title text plus its icon.
fn title_text_width(group: &GroupDef, tokens: &DesignTokens) -> f64 {
    let font = tokens.group_title_font_size;
    let (icon, gap) = group_icon_size(tokens);
    let icon_w = if group.resolved_icon().is_some() { icon + gap } else { 0.0 };
    group.label.chars().count() as f64 * tokens.char_width(font) + icon_w
}

/// The narrowest a group's box may be, for content `content_w` wide with `pad` around
/// it: its title fits, and the centre of the content clears the title's end (by a stub),
/// so an arrow dropping into the middle of the group — onto a lone node, say — never
/// crosses the title. When the box must grow for this, the extra width goes on the
/// left, under the title (see [`widen_for_title`]).
pub fn group_min_width(group: &GroupDef, tokens: &DesignTokens, content_w: f64, pad: Padding) -> f64 {
    let inset = group_title_inset(tokens).0;
    let text = title_text_width(group, tokens);
    let title_end = inset + text + tokens.stub_clearance();
    (text + 2.0 * inset).max(title_end + content_w / 2.0 + pad.right).ceil()
}

/// How much wider than its content-plus-padding width `w` a group must be drawn —
/// added on the left, so the content sits right of the title.
pub fn widen_for_title(group: &GroupDef, tokens: &DesignTokens, content_w: f64, pad: Padding, w: f64) -> f64 {
    (group_min_width(group, tokens, content_w, pad) - w).max(0.0)
}

/// The title's own box inside a group drawn at `rect` — what arrows and edge labels
/// must stay off. Just the text plus a little air: anything bigger would wall off the
/// top faces of the nodes under it.
pub fn group_title_rect(group: &GroupDef, rect: Rect, tokens: &DesignTokens) -> Rect {
    let font = tokens.group_title_font_size;
    let w = title_text_width(group, tokens) + tokens.px(2.5);
    let h = tokens.px(1.0) + tokens.line_height(font) + tokens.px(1.25);
    (rect.0, rect.1, w.min(rect.2), h)
}

/// Every group's drawn rectangle, indexed like `compiled.groups`; `None` for a group
/// with no laid-out node inside it at any depth. Inner groups are measured first: a
/// group's content is its own nodes plus its inner groups' boxes, padded by
/// [`group_padding`] and widened on the left to [`group_min_width`].
pub fn group_rects(compiled: &CompiledGraph, positions: &HashMap<NodeIndex, NodeLayout>, tokens: &DesignTokens) -> Vec<Option<Rect>> {
    let mut rects: Vec<Option<Rect>> = vec![None; compiled.groups.len()];
    for g in compiled.group_tree.outer_first().into_iter().rev() {
        let mut ext: Option<(f64, f64, f64, f64)> = None;
        let mut add = |(x, y, w, h): Rect| {
            let e = ext.get_or_insert((x, y, x + w, y + h));
            *e = (e.0.min(x), e.1.min(y), e.2.max(x + w), e.3.max(y + h));
        };
        for n in &compiled.group_nodes[g] {
            if let Some(nl) = positions.get(n) {
                add((nl.x, nl.y, nl.width, nl.height));
            }
        }
        for c in compiled.group_tree.children(g) {
            if let Some(r) = rects[c] {
                add(r);
            }
        }
        let Some((x0, y0, x1, y1)) = ext else { continue };
        let (cw, ch) = (x1 - x0, y1 - y0);
        let pad = group_padding(tokens, cw, ch);
        let w = cw + pad.left + pad.right;
        let extra = widen_for_title(&compiled.groups[g], tokens, cw, pad, w);
        rects[g] = Some((x0 - pad.left - extra, y0 - pad.top, w + extra, ch + pad.top + pad.bottom));
    }
    rects
}
