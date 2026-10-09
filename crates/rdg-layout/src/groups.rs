//! Group container geometry — the one definition of how big a group's box is, where
//! its title sits and how far its border stands off its content. Layout sizes each
//! group's block with it, routing treats the titles as obstacles, and both renderers
//! draw the boxes from [`group_rects`], so none of them can disagree about where a
//! container is.

use rdg_graph::CompiledGraph;
use rdg_schema::GroupDef;

use crate::{DesignTokens, LayoutResult};

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

/// Padding for `group` when its content (nodes and inner groups) spans `content_w` ×
/// `content_h`: content-aware sides and bottom, and the title's lines on top.
pub fn group_padding(tokens: &DesignTokens, group: &GroupDef, content_w: f64, content_h: f64) -> Padding {
    let side = tokens.group_pad_for(content_w, content_h);
    let extra_lines = group_title_lines(group, tokens).len().saturating_sub(1) as f64;
    let top = tokens.group_pad_top_for(content_w, content_h) + extra_lines * tokens.line_height(tokens.group_title_font_size);
    Padding { left: side, right: side, top: top.round(), bottom: side }
}

/// The title as drawn: one line, or two balanced lines when it is longer than
/// [`DesignTokens::group_title_wrap_chars`] and has a space to break at.
pub fn group_title_lines(group: &GroupDef, tokens: &DesignTokens) -> Vec<String> {
    let label = group.label.trim();
    let n = label.chars().count();
    if n <= tokens.group_title_wrap_chars || !label.contains(' ') {
        return vec![label.to_string()];
    }
    // The space nearest the middle gives the most even split.
    let mid = n / 2;
    let Some((i, _)) = label.char_indices().filter(|&(_, c)| c == ' ').min_by_key(|&(i, _)| (label[..i].chars().count() as i64 - mid as i64).abs()) else {
        return vec![label.to_string()];
    };
    vec![label[..i].to_string(), label[i + 1..].to_string()]
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
    let longest = group_title_lines(group, tokens).iter().map(|l| l.chars().count()).max().unwrap_or(0);
    longest as f64 * tokens.char_width(font) + icon_w
}

/// The narrowest a group's box may be: its title, inset on both sides.
pub fn group_min_width(group: &GroupDef, tokens: &DesignTokens) -> f64 {
    (title_text_width(group, tokens) + 2.0 * group_title_inset(tokens).0).ceil()
}

/// Distance from a group's left edge to just past the end of its title (by a stub): a
/// node whose centre lies right of this can be entered from above without the arrow
/// crossing the title text. The layout shifts a group's content right to put every node
/// fed from above past it (see `LayoutResult::group_title_shift`).
pub fn title_clear_x(group: &GroupDef, tokens: &DesignTokens) -> f64 {
    (group_title_inset(tokens).0 + title_text_width(group, tokens) + tokens.stub_clearance()).ceil()
}

/// The title's own box inside a group drawn at `rect` — what arrows and edge labels
/// must stay off. Just the text plus a little air: anything bigger would wall off the
/// top faces of the nodes under it.
pub fn group_title_rect(group: &GroupDef, rect: Rect, tokens: &DesignTokens) -> Rect {
    let font = tokens.group_title_font_size;
    let w = title_text_width(group, tokens) + tokens.px(2.5);
    let h = tokens.px(1.0) + group_title_lines(group, tokens).len() as f64 * tokens.line_height(font) + tokens.px(1.25);
    (rect.0, rect.1, w.min(rect.2), h)
}

/// Every group's drawn rectangle, indexed like `compiled.groups`; `None` for a group
/// with no laid-out node inside it at any depth. Inner groups are measured first: a
/// group's content is its own nodes plus its inner groups' boxes, padded by
/// [`group_padding`] and widened evenly — content stays centred — to the larger of
/// [`group_min_width`] and the width the layout gave it (`layout.group_widths`).
pub fn group_rects(compiled: &CompiledGraph, layout: &LayoutResult, tokens: &DesignTokens) -> Vec<Option<Rect>> {
    let mut rects: Vec<Option<Rect>> = vec![None; compiled.groups.len()];
    for g in compiled.group_tree.outer_first().into_iter().rev() {
        let mut ext: Option<(f64, f64, f64, f64)> = None;
        let mut add = |(x, y, w, h): Rect| {
            let e = ext.get_or_insert((x, y, x + w, y + h));
            *e = (e.0.min(x), e.1.min(y), e.2.max(x + w), e.3.max(y + h));
        };
        for n in &compiled.group_nodes[g] {
            if let Some(nl) = layout.positions.get(n) {
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
        let pad = group_padding(tokens, &compiled.groups[g], cw, ch);
        // Extra room on the left so nodes fed from above sit past the title (the layout's
        // call); any further widening to the minimum width is shared evenly.
        let shift = layout.group_title_shift.get(&g).copied().unwrap_or(0.0);
        let w = cw + pad.left + pad.right + shift;
        let min_w = group_min_width(&compiled.groups[g], tokens).max(layout.group_widths.get(&g).copied().unwrap_or(0.0));
        let extra = (min_w - w).max(0.0);
        rects[g] = Some((x0 - pad.left - shift - extra / 2.0, y0 - pad.top, w + extra, ch + pad.top + pad.bottom));
    }
    rects
}
