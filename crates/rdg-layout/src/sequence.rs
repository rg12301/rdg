//! Sequence diagram layout.
//!
//! Participants sit left to right (a group's members kept together), each with a
//! lifeline; the script (`CompiledGraph::sequence`) is laid out top to bottom, one row
//! per message, note, fragment boundary or divider.
//!
//! Horizontal spacing is solved from what has to fit between lifelines — message
//! labels, self-call loops, notes, fragment headers — rather than a fixed pitch: each
//! requirement is a minimum distance between two lifelines, applied narrowest span
//! first by widening the gaps it spans evenly (widening never breaks one already met).
//!
//! Activations: a call opens a bar on the receiver when a reply to it comes later (the
//! reply closes it); `activate` / `deactivate` on a message override that. Bars nest,
//! each level offset by half a bar, and arrows meet the bar's edge, not the lifeline.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use petgraph::stable_graph::{EdgeIndex, NodeIndex};

use rdg_graph::{CompiledGraph, NotePlacement, SeqItem};

use crate::{DesignTokens, LayoutConfig, LayoutResult, NodeLayout, estimate_node_box, node_lines, wrap_label};

/// One message arrow.
#[derive(Debug, Clone)]
pub struct SequenceMessageLayout {
    pub edge_idx: EdgeIndex,
    pub from_node: NodeIndex,
    pub to_node: NodeIndex,
    /// The arrow's y (a self-call: the top of its loop).
    pub y: f64,
    /// Where the arrow leaves the sender and meets the receiver — the facing edge of
    /// an activation bar, a created participant's box, or the lifeline.
    pub from_x: f64,
    pub to_x: f64,
    pub is_self_call: bool,
    pub is_reply: bool,
    /// Self-calls: how far the loop reaches right, and how tall it is.
    pub loop_w: f64,
    pub loop_h: f64,
}

/// A participant's lifeline (from under its box to its end or destruction).
#[derive(Debug, Clone)]
pub struct SeqLifeline {
    pub node: NodeIndex,
    pub x: f64,
    pub y0: f64,
    pub y1: f64,
    /// Ends in an ✕ (destroyed by a message).
    pub destroyed: bool,
}

/// An activation bar (`x` is its left edge).
#[derive(Debug, Clone)]
pub struct SeqActivation {
    pub node: NodeIndex,
    pub x: f64,
    pub y0: f64,
    pub y1: f64,
    pub width: f64,
}

#[derive(Debug, Clone)]
pub struct SeqNote {
    /// Wrapped lines.
    pub lines: Vec<String>,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// A combined fragment (`alt`, `loop`, …): a frame with its keyword in a tab at the
/// top left, the guard beside it, and a dashed separator above each further branch.
#[derive(Debug, Clone)]
pub struct SeqFragment {
    pub kind: String,
    pub label: Option<String>,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// Height of the keyword tab.
    pub tab_h: f64,
    /// `(y, guard)` of each `else` / `and` separator.
    pub separators: Vec<(f64, Option<String>)>,
    pub depth: usize,
}

/// A full-width separator.
#[derive(Debug, Clone)]
pub struct SeqDivider {
    pub label: String,
    pub y: f64,
}

/// A participant group's box (`group` indexes `CompiledGraph::groups`).
#[derive(Debug, Clone)]
pub struct SeqGroupBox {
    pub group: usize,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Everything a renderer needs to draw a sequence diagram, besides the participant
/// boxes in `LayoutResult::positions`.
#[derive(Debug, Clone)]
pub struct SequenceLayoutInfo {
    /// Participant → lifeline x.
    pub lifeline_x: HashMap<NodeIndex, f64>,
    /// Bottom of the (top-row) participant boxes.
    pub lifeline_top_y: f64,
    /// Bottom of the lifelines.
    pub lifeline_bottom_y: f64,
    pub lifelines: Vec<SeqLifeline>,
    pub messages: Vec<SequenceMessageLayout>,
    pub activations: Vec<SeqActivation>,
    pub notes: Vec<SeqNote>,
    /// Outer fragments before the ones nested in them (draw in order).
    pub fragments: Vec<SeqFragment>,
    pub dividers: Vec<SeqDivider>,
    pub groups: Vec<SeqGroupBox>,
    /// Horizontal extent of the content (dividers span it).
    pub min_x: f64,
    pub max_x: f64,
}

/// Font metrics used throughout.
struct Metrics {
    lbl_cw: f64,
    lbl_lh: f64,
    note_cw: f64,
    note_lh: f64,
    bar_w: f64,
}

impl Metrics {
    fn new(t: &DesignTokens) -> Self {
        Self {
            lbl_cw: t.char_width(t.edge_label_font_size),
            lbl_lh: t.line_height(t.edge_label_font_size),
            note_cw: t.char_width(t.detail_font_size),
            note_lh: t.line_height(t.detail_font_size),
            bar_w: t.seq_activation_width,
        }
    }
}

/// A message's label lines, with its step number prefixed when numbered.
pub fn message_label_lines(compiled: &CompiledGraph, e: EdgeIndex) -> Vec<String> {
    let ed = &compiled.graph[e];
    let mut lines: Vec<String> = ed.label.as_deref().map(|l| l.lines().map(str::to_string).collect()).unwrap_or_default();
    if let Some(n) = &ed.step {
        match lines.first_mut() {
            Some(first) => *first = format!("{n}. {first}"),
            None => lines.push(format!("{n}.")),
        }
    }
    lines
}

fn text_w(lines: &[String], cw: f64) -> f64 {
    lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as f64 * cw
}

/// A note's wrapped lines and box size.
fn note_box(text: &str, t: &DesignTokens, m: &Metrics) -> (Vec<String>, f64, f64) {
    let lines = wrap_label(text, t.seq_note_wrap_chars);
    let w = text_w(&lines, m.note_cw) + t.px(2.0);
    let h = lines.len() as f64 * m.note_lh + t.px(1.5);
    (lines, w.max(t.px(6.0)), h)
}

/// Width a fragment's header needs: the keyword tab plus the widest guard.
fn fragment_header_w(kind: &str, sections: &[(Option<String>, Vec<SeqItem>)], t: &DesignTokens, m: &Metrics) -> f64 {
    let tab = kind.chars().count() as f64 * m.lbl_cw + t.px(2.5);
    let guard = sections
        .iter()
        .filter_map(|(g, _)| g.as_ref())
        .map(|g| (g.chars().count() + 2) as f64 * m.lbl_cw)
        .fold(0.0, f64::max);
    tab + guard + t.px(2.0)
}

/// Participant box size: a card sized to its label, or an actor figure with the label
/// underneath.
fn participant_size(compiled: &CompiledGraph, n: NodeIndex, t: &DesignTokens) -> (f64, f64) {
    let nd = &compiled.graph[n];
    if nd.shape.as_deref() == Some("actor") {
        let lines = node_lines(nd, t.wrap_chars_normal);
        let w = lines.iter().map(|l| l.text.chars().count()).max().unwrap_or(0) as f64 * t.char_width(t.font_size);
        return ((w + t.px(1.0)).max(t.px(6.0)), actor_figure_h(t) + lines.len() as f64 * t.line_height(t.font_size));
    }
    estimate_node_box(nd, nd.icon.is_some(), t.px(12.0), t.px(5.0), t)
}

/// Height of an actor's stick figure (the label goes under it).
pub fn actor_figure_h(t: &DesignTokens) -> f64 {
    t.px(4.5)
}

/// A message, flattened out of the script tree.
struct Msg {
    edge: EdgeIndex,
    src: NodeIndex,
    dst: NodeIndex,
    activate: Option<bool>,
    deactivate: Option<bool>,
    create: bool,
    destroy: bool,
    is_reply: bool,
    is_async: bool,
}

fn flatten(items: &[SeqItem], compiled: &CompiledGraph, out: &mut Vec<Msg>) {
    for it in items {
        match it {
            SeqItem::Message { edge, activate, deactivate, create, destroy } => {
                let (src, dst) = compiled.graph.edge_endpoints(*edge).expect("message edge");
                let style = compiled.graph[*edge].edge_style.as_deref().unwrap_or("");
                out.push(Msg {
                    edge: *edge,
                    src,
                    dst,
                    activate: *activate,
                    deactivate: *deactivate,
                    create: *create,
                    destroy: *destroy,
                    is_reply: matches!(style, "reply" | "return" | "response"),
                    is_async: style == "async",
                });
            }
            SeqItem::Fragment { sections, .. } => {
                for (_, s) in sections {
                    flatten(s, compiled, out);
                }
            }
            _ => {}
        }
    }
}

/// Activation intervals, by message index.
#[derive(Debug, Clone)]
struct Interval {
    node: NodeIndex,
    start: usize,
    end: usize,
    /// Opened by a self-call: starts where the loop comes back.
    from_self: bool,
    depth: usize,
}

fn activation_intervals(msgs: &[Msg]) -> Vec<Interval> {
    let mut done: Vec<Interval> = Vec::new();
    let mut open: Vec<Interval> = Vec::new();
    for (k, m) in msgs.iter().enumerate() {
        let is_self = m.src == m.dst;
        if m.deactivate.unwrap_or(m.is_reply && !is_self) {
            if let Some(i) = open.iter().rposition(|iv| iv.node == m.src) {
                let mut iv = open.remove(i);
                iv.end = k;
                done.push(iv);
            }
        }
        let has_reply = || msgs[k + 1..].iter().any(|r| r.is_reply && r.src == m.dst && r.dst == m.src);
        let act = m.activate.unwrap_or(!is_self && !m.is_reply && !m.is_async && has_reply());
        if act && is_self {
            // A self-call's nested bar covers the work it returns to: a short bar.
            done.push(Interval { node: m.dst, start: k, end: k, from_self: true, depth: 0 });
        } else if act {
            open.push(Interval { node: m.dst, start: k, end: k, from_self: false, depth: 0 });
        }
    }
    // Still open at the end: close at that participant's last message.
    for mut iv in open {
        iv.end = msgs.iter().rposition(|m| m.src == iv.node || m.dst == iv.node).unwrap_or(iv.start).max(iv.start);
        done.push(iv);
    }
    done.sort_by_key(|iv| (iv.start, iv.node.index()));
    for i in 0..done.len() {
        let d = done[..i]
            .iter()
            .filter(|o| o.node == done[i].node && o.start <= done[i].start && o.end >= done[i].start)
            .count();
        done[i].depth = d + 1;
    }
    done
}

pub fn compute_sequence_layout(compiled: &CompiledGraph, config: &LayoutConfig) -> Result<LayoutResult> {
    let t = &config.tokens;
    let m = Metrics::new(t);
    if compiled.graph.node_count() == 0 {
        return Ok(LayoutResult { positions: HashMap::new(), sequence_info: None });
    }

    // --- Participants, a group's members together --------------------------------
    let mut group_of: HashMap<NodeIndex, usize> = HashMap::new();
    for (gi, g) in compiled.groups.iter().enumerate() {
        for id in &g.nodes {
            if let Some(&n) = compiled.node_map.get(id) {
                group_of.entry(n).or_insert(gi);
            }
        }
    }
    let mut order: Vec<NodeIndex> = Vec::new();
    let mut seen: HashSet<NodeIndex> = HashSet::new();
    for n in compiled.graph.node_indices() {
        if seen.contains(&n) {
            continue;
        }
        match group_of.get(&n) {
            Some(&g) => {
                for mbr in compiled.graph.node_indices().filter(|x| group_of.get(x) == Some(&g)) {
                    if seen.insert(mbr) {
                        order.push(mbr);
                    }
                }
            }
            None => {
                seen.insert(n);
                order.push(n);
            }
        }
    }
    let k = order.len();
    let pos_of: HashMap<NodeIndex, usize> = order.iter().enumerate().map(|(i, &n)| (n, i)).collect();
    let sizes: Vec<(f64, f64)> = order.iter().map(|&n| participant_size(compiled, n, t)).collect();

    let mut msgs = Vec::new();
    flatten(&compiled.sequence, compiled, &mut msgs);
    let intervals = activation_intervals(&msgs);

    // --- Horizontal: minimum lifeline distances ----------------------------------
    let group_pad = t.px(1.5);
    let mut gaps: Vec<f64> = (0..k.saturating_sub(1))
        .map(|i| {
            let boundary = group_of.get(&order[i]) != group_of.get(&order[i + 1])
                && (group_of.contains_key(&order[i]) || group_of.contains_key(&order[i + 1]));
            (sizes[i].0 + sizes[i + 1].0) / 2.0 + t.seq_participant_gap + if boundary { 2.0 * group_pad } else { 0.0 }
        })
        .collect();
    let mut cons: Vec<(usize, usize, f64)> = Vec::new();
    let loop_w = t.px(3.5);
    // A requirement `d` beside participant `i` on one side: a gap to its neighbour, or
    // room at the diagram's edge (handled by the final shift).
    let mut side = |cons: &mut Vec<(usize, usize, f64)>, i: usize, right: bool, d: f64| {
        if right && i + 1 < k {
            cons.push((i, i + 1, d));
        } else if !right && i > 0 {
            cons.push((i - 1, i, d));
        }
    };
    for msg in &msgs {
        let (a, b) = (pos_of[&msg.src], pos_of[&msg.dst]);
        let lw = text_w(&message_label_lines(compiled, msg.edge), m.lbl_cw) + t.px(1.0);
        if a == b {
            side(&mut cons, a, true, loop_w + m.bar_w + lw + t.px(2.0));
        } else {
            let extra = if msg.create { sizes[b].0 / 2.0 } else { 0.0 };
            cons.push((a.min(b), a.max(b), lw + t.px(3.0) + 2.0 * m.bar_w + extra));
        }
    }
    type Cons = Vec<(usize, usize, f64)>;
    #[allow(clippy::too_many_arguments)]
    fn walk_cons(
        items: &[SeqItem],
        pos_of: &HashMap<NodeIndex, usize>,
        compiled: &CompiledGraph,
        t: &DesignTokens,
        m: &Metrics,
        cons: &mut Cons,
        side: &mut dyn FnMut(&mut Cons, usize, bool, f64),
        involved: &mut Vec<usize>,
    ) {
        for it in items {
            match it {
                SeqItem::Message { edge, .. } => {
                    let (s, d) = compiled.graph.edge_endpoints(*edge).expect("message edge");
                    involved.extend([pos_of[&s], pos_of[&d]]);
                }
                SeqItem::Note { text, placement, over } => {
                    let (_, nw, _) = note_box(text, t, m);
                    let idx: Vec<usize> = over.iter().map(|n| pos_of[n]).collect();
                    let (a, b) = (*idx.iter().min().unwrap_or(&0), *idx.iter().max().unwrap_or(&0));
                    involved.extend([a, b]);
                    match placement {
                        NotePlacement::Over if a < b => cons.push((a, b, nw - 2.0 * t.px(2.0))),
                        NotePlacement::Over => {
                            side(cons, a, true, nw / 2.0 + t.px(1.0));
                            side(cons, a, false, nw / 2.0 + t.px(1.0));
                        }
                        NotePlacement::RightOf => side(cons, a, true, nw + m.bar_w + t.px(2.0)),
                        NotePlacement::LeftOf => side(cons, a, false, nw + m.bar_w + t.px(2.0)),
                    }
                }
                SeqItem::Fragment { kind, sections } => {
                    let mut inner = Vec::new();
                    for (_, s) in sections {
                        walk_cons(s, pos_of, compiled, t, m, cons, side, &mut inner);
                    }
                    let need = fragment_header_w(kind, sections, t, m);
                    if let (Some(&a), Some(&b)) = (inner.iter().min(), inner.iter().max()) {
                        if a < b {
                            cons.push((a, b, need - 2.0 * t.px(1.5)));
                        }
                    }
                    involved.extend(inner);
                }
                SeqItem::Divider(_) => {}
            }
        }
    }
    let mut involved = Vec::new();
    walk_cons(&compiled.sequence, &pos_of, compiled, t, &m, &mut cons, &mut side, &mut involved);
    cons.sort_by(|x, y| (x.1 - x.0).cmp(&(y.1 - y.0)).then(x.0.cmp(&y.0)));
    for &(a, b, d) in &cons {
        let have: f64 = gaps[a..b].iter().sum();
        if d > have {
            let add = (d - have) / (b - a) as f64;
            for g in &mut gaps[a..b] {
                *g += add;
            }
        }
    }
    let mut px_of = vec![0.0_f64; k];
    for i in 1..k {
        px_of[i] = px_of[i - 1] + gaps[i - 1];
    }
    let lx = |n: NodeIndex| px_of[pos_of[&n]];

    // --- Vertical --------------------------------------------------------------------
    let title_h = if compiled.title.is_some() { t.title_band_for(compiled.description.is_some()) } else { 0.0 };
    let group_band = if group_of.is_empty() { 0.0 } else { t.px(1.5) + t.line_height(t.group_title_font_size) + t.px(1.0) };
    let top = config.margin_y + title_h + group_band;
    let created: HashSet<NodeIndex> = msgs.iter().filter(|mm| mm.create).map(|mm| mm.dst).collect();
    let header_h = order.iter().zip(&sizes).filter(|(n, _)| !created.contains(n)).map(|(_, s)| s.1).fold(0.0, f64::max);

    let mut positions: HashMap<NodeIndex, NodeLayout> = HashMap::new();
    let mut lifeline_y0: HashMap<NodeIndex, f64> = HashMap::new();
    for (i, &n) in order.iter().enumerate() {
        if !created.contains(&n) {
            let (w, h) = sizes[i];
            positions.insert(n, NodeLayout { x: px_of[i] - w / 2.0, y: top, width: w, height: h });
            lifeline_y0.insert(n, top + h);
        }
    }

    // Activation bar geometry (x is known now; y comes from the walk).
    let bar_left = |iv: &Interval| lx(iv.node) - m.bar_w / 2.0 + (iv.depth - 1) as f64 * m.bar_w / 2.0;
    // Where a message at index `k` meets participant `n`, facing `toward`.
    let attach = |n: NodeIndex, k: usize, toward: f64, receiving: bool| -> f64 {
        let x = lx(n);
        let active = intervals
            .iter()
            .filter(|iv| iv.node == n && iv.end >= k && if receiving { iv.start <= k } else { iv.start < k || (iv.start == k && iv.from_self) })
            .max_by_key(|iv| iv.depth);
        match active {
            Some(iv) => {
                let l = bar_left(iv);
                if toward > x { l + m.bar_w } else { l }
            }
            None => x,
        }
    };

    struct Walk {
        cursor: f64,
        k: usize,
        ys: Vec<f64>,
        loop_hs: Vec<f64>,
        messages: Vec<SequenceMessageLayout>,
        notes: Vec<SeqNote>,
        fragments: Vec<SeqFragment>,
        dividers: Vec<SeqDivider>,
        destroyed: HashMap<NodeIndex, f64>,
    }
    let mut st = Walk {
        cursor: top + header_h + t.px(3.0),
        k: 0,
        ys: vec![0.0; msgs.len()],
        loop_hs: vec![0.0; msgs.len()],
        messages: Vec::new(),
        notes: Vec::new(),
        fragments: Vec::new(),
        dividers: Vec::new(),
        destroyed: HashMap::new(),
    };

    #[allow(clippy::too_many_arguments)]
    fn walk(
        items: &[SeqItem],
        depth: usize,
        st: &mut Walk,
        msgs: &[Msg],
        compiled: &CompiledGraph,
        t: &DesignTokens,
        m: &Metrics,
        order: &[NodeIndex],
        sizes: &[(f64, f64)],
        pos_of: &HashMap<NodeIndex, usize>,
        px_of: &[f64],
        attach: &dyn Fn(NodeIndex, usize, f64, bool) -> f64,
        positions: &mut HashMap<NodeIndex, NodeLayout>,
        lifeline_y0: &mut HashMap<NodeIndex, f64>,
    ) -> Option<(f64, f64)> {
        let mut ext: Option<(f64, f64)> = None;
        let grow = |ext: &mut Option<(f64, f64)>, a: f64, b: f64| {
            *ext = Some(match *ext {
                Some((x0, x1)) => (x0.min(a), x1.max(b)),
                None => (a, b),
            });
        };
        for it in items {
            match it {
                SeqItem::Message { .. } => {
                    let k = st.k;
                    st.k += 1;
                    let msg = &msgs[k];
                    let lines = message_label_lines(compiled, msg.edge);
                    let label_h = lines.len() as f64 * m.lbl_lh;
                    let label_w = text_w(&lines, m.lbl_cw);
                    let (xs, xd) = (px_of[pos_of[&msg.src]], px_of[pos_of[&msg.dst]]);
                    if msg.src == msg.dst {
                        let loop_w = t.px(3.5);
                        let loop_h = t.px(3.0).max(label_h + t.px(1.0));
                        let y = st.cursor + t.px(0.75);
                        let from_x = attach(msg.src, k, xs + 1.0, false);
                        let to_x = attach(msg.dst, k, xs + 1.0, true);
                        st.ys[k] = y;
                        st.loop_hs[k] = loop_h;
                        st.cursor = y + loop_h + t.seq_message_gap;
                        grow(&mut ext, xs - m.bar_w, from_x.max(to_x) + loop_w + t.px(0.75) + label_w);
                        st.messages.push(SequenceMessageLayout {
                            edge_idx: msg.edge,
                            from_node: msg.src,
                            to_node: msg.dst,
                            y,
                            from_x,
                            to_x,
                            is_self_call: true,
                            is_reply: msg.is_reply,
                            loop_w,
                            loop_h,
                        });
                        continue;
                    }
                    let y;
                    let to_x;
                    if msg.create {
                        let i = pos_of[&msg.dst];
                        let (w, h) = sizes[i];
                        y = st.cursor + (label_h + t.px(0.75)).max(h / 2.0 + t.px(0.5));
                        positions.insert(order[i], NodeLayout { x: xd - w / 2.0, y: y - h / 2.0, width: w, height: h });
                        lifeline_y0.insert(order[i], y + h / 2.0);
                        to_x = if xs < xd { xd - w / 2.0 } else { xd + w / 2.0 };
                        st.cursor = y + h / 2.0 + t.seq_message_gap;
                    } else {
                        y = st.cursor + label_h + t.px(0.75);
                        to_x = attach(msg.dst, k, xs, true);
                        st.cursor = y + t.seq_message_gap;
                    }
                    let from_x = attach(msg.src, k, xd, false);
                    if msg.destroy {
                        st.destroyed.insert(msg.dst, y);
                    }
                    st.ys[k] = y;
                    grow(&mut ext, from_x.min(to_x), from_x.max(to_x));
                    st.messages.push(SequenceMessageLayout {
                        edge_idx: msg.edge,
                        from_node: msg.src,
                        to_node: msg.dst,
                        y,
                        from_x,
                        to_x,
                        is_self_call: false,
                        is_reply: msg.is_reply,
                        loop_w: 0.0,
                        loop_h: 0.0,
                    });
                }
                SeqItem::Note { text, placement, over } => {
                    let (lines, nw, nh) = note_box(text, t, m);
                    let xs: Vec<f64> = over.iter().map(|n| px_of[pos_of[n]]).collect();
                    let (a, b) = (xs.iter().copied().fold(f64::MAX, f64::min), xs.iter().copied().fold(f64::MIN, f64::max));
                    let (x0, x1) = match placement {
                        NotePlacement::Over => {
                            let (mut x0, mut x1) = if b > a { (a - t.px(2.0), b + t.px(2.0)) } else { (a - nw / 2.0, a + nw / 2.0) };
                            if x1 - x0 < nw {
                                let c = (x0 + x1) / 2.0;
                                (x0, x1) = (c - nw / 2.0, c + nw / 2.0);
                            }
                            (x0, x1)
                        }
                        NotePlacement::RightOf => (a + m.bar_w + t.px(1.0), a + m.bar_w + t.px(1.0) + nw),
                        NotePlacement::LeftOf => (a - m.bar_w - t.px(1.0) - nw, a - m.bar_w - t.px(1.0)),
                    };
                    let y = st.cursor + t.px(0.5);
                    st.cursor = y + nh + t.seq_message_gap;
                    grow(&mut ext, x0, x1);
                    st.notes.push(SeqNote { lines, x: x0, y, w: x1 - x0, h: nh });
                }
                SeqItem::Fragment { kind, sections } => {
                    let slot = st.fragments.len();
                    let tab_h = m.lbl_lh + t.px(1.0);
                    let top = st.cursor + t.px(0.5);
                    st.fragments.push(SeqFragment { kind: kind.clone(), label: sections[0].0.clone(), x: 0.0, y: top, w: 0.0, h: 0.0, tab_h, separators: Vec::new(), depth });
                    st.cursor = top + tab_h + t.px(1.0);
                    let mut inner: Option<(f64, f64)> = None;
                    let mut seps = Vec::new();
                    for (si, (guard, body)) in sections.iter().enumerate() {
                        if si > 0 {
                            let sy = st.cursor + t.px(0.25);
                            seps.push((sy, guard.clone()));
                            st.cursor = sy + m.lbl_lh + t.px(1.0);
                        }
                        if let Some((a, b)) = walk(body, depth + 1, st, msgs, compiled, t, m, order, sizes, pos_of, px_of, attach, positions, lifeline_y0) {
                            grow(&mut inner, a, b);
                        }
                    }
                    let bottom = st.cursor - t.seq_message_gap + t.px(1.5);
                    st.cursor = bottom + t.seq_message_gap;
                    let pad = t.px(1.5);
                    let (mut x0, mut x1) = inner.map_or((px_of[0] - sizes[0].0 / 2.0, px_of[px_of.len() - 1] + sizes[sizes.len() - 1].0 / 2.0), |(a, b)| (a - pad, b + pad));
                    let need = fragment_header_w(kind, sections, t, m);
                    if x1 - x0 < need {
                        let c = (x0 + x1) / 2.0;
                        (x0, x1) = (c - need / 2.0, c + need / 2.0);
                    }
                    let f = &mut st.fragments[slot];
                    (f.x, f.w, f.h, f.separators) = (x0, x1 - x0, bottom - top, seps);
                    grow(&mut ext, x0, x1);
                }
                SeqItem::Divider(label) => {
                    let y = st.cursor + t.px(0.5);
                    st.cursor = y + m.lbl_lh + t.seq_message_gap;
                    st.dividers.push(SeqDivider { label: label.clone(), y });
                }
            }
        }
        ext
    }
    walk(&compiled.sequence, 0, &mut st, &msgs, compiled, t, &m, &order, &sizes, &pos_of, &px_of, &attach, &mut positions, &mut lifeline_y0);

    let bottom = st.cursor + t.px(1.0);
    let activations: Vec<SeqActivation> = intervals
        .iter()
        .map(|iv| {
            // A participant created by the opening message starts its bar under its box.
            let created_here = msgs[iv.start].create && msgs[iv.start].dst == iv.node;
            let y0 = if created_here {
                lifeline_y0.get(&iv.node).copied().unwrap_or(st.ys[iv.start])
            } else {
                st.ys[iv.start] + if iv.from_self { st.loop_hs[iv.start] } else { 0.0 }
            };
            let y1 = if iv.end > iv.start { st.ys[iv.end] + st.loop_hs[iv.end] } else { y0 + t.px(2.0) };
            SeqActivation { node: iv.node, x: bar_left(iv), y0, y1: y1.max(y0 + t.px(1.0)), width: m.bar_w }
        })
        .collect();
    let lifelines: Vec<SeqLifeline> = order
        .iter()
        .map(|&n| SeqLifeline {
            node: n,
            x: lx(n),
            y0: lifeline_y0.get(&n).copied().unwrap_or(top + header_h),
            y1: st.destroyed.get(&n).copied().unwrap_or(bottom),
            destroyed: st.destroyed.contains_key(&n),
        })
        .collect();
    let mut groups: Vec<SeqGroupBox> = Vec::new();
    for gi in 0..compiled.groups.len() {
        let members: Vec<usize> = (0..k).filter(|&i| group_of.get(&order[i]) == Some(&gi)).collect();
        if members.is_empty() {
            continue;
        }
        let x0 = members.iter().map(|&i| px_of[i] - sizes[i].0 / 2.0).fold(f64::MAX, f64::min) - group_pad;
        let x1 = members.iter().map(|&i| px_of[i] + sizes[i].0 / 2.0).fold(f64::MIN, f64::max) + group_pad;
        let y = top - group_band;
        groups.push(SeqGroupBox { group: gi, x: x0, y, w: x1 - x0, h: bottom + t.px(1.0) - y });
    }

    // --- Shift everything so the content starts at the margin ------------------------
    let mut min_x = f64::MAX;
    let mut max_x = f64::MIN;
    let mut see = |a: f64, b: f64| {
        min_x = min_x.min(a);
        max_x = max_x.max(b);
    };
    positions.values().for_each(|nl| see(nl.x, nl.x + nl.width));
    groups.iter().for_each(|g| see(g.x, g.x + g.w));
    st.notes.iter().for_each(|n| see(n.x, n.x + n.w));
    st.fragments.iter().for_each(|f| see(f.x, f.x + f.w));
    for msg in &st.messages {
        let lw = text_w(&message_label_lines(compiled, msg.edge_idx), m.lbl_cw);
        if msg.is_self_call {
            see(msg.from_x, msg.from_x.max(msg.to_x) + msg.loop_w + t.px(0.75) + lw);
        } else {
            let c = (msg.from_x + msg.to_x) / 2.0;
            see(msg.from_x.min(msg.to_x).min(c - lw / 2.0), msg.from_x.max(msg.to_x).max(c + lw / 2.0));
        }
    }
    let dx = config.margin_x - min_x;
    for nl in positions.values_mut() {
        nl.x += dx;
    }
    for g in &mut groups {
        g.x += dx;
    }
    for n in &mut st.notes {
        n.x += dx;
    }
    for f in &mut st.fragments {
        f.x += dx;
    }
    for msg in &mut st.messages {
        msg.from_x += dx;
        msg.to_x += dx;
    }
    let mut activations = activations;
    for a in &mut activations {
        a.x += dx;
    }
    let mut lifelines = lifelines;
    for l in &mut lifelines {
        l.x += dx;
    }
    let lifeline_x = lifelines.iter().map(|l| (l.node, l.x)).collect();

    Ok(LayoutResult {
        positions,
        sequence_info: Some(SequenceLayoutInfo {
            lifeline_x,
            lifeline_top_y: top + header_h,
            lifeline_bottom_y: bottom,
            lifelines,
            messages: st.messages,
            activations,
            notes: st.notes,
            fragments: st.fragments,
            dividers: st.dividers,
            groups,
            min_x: min_x + dx,
            max_x: max_x + dx,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rdg_graph::build_graph;
    use rdg_schema::DiagramPayload;

    fn layout(yaml: &str) -> (CompiledGraph, LayoutResult) {
        let compiled = build_graph(&DiagramPayload::from_yaml(yaml).unwrap()).unwrap();
        let layout = compute_sequence_layout(&compiled, &LayoutConfig::default()).unwrap();
        (compiled, layout)
    }

    #[test]
    fn calls_with_replies_activate_and_arrows_meet_the_bar() {
        let (c, l) = layout(
            "participants:\n  - {id: a, label: A}\n  - {id: b, label: B}\n  - {id: q, label: Q}\n\
             sequence:\n  - {from: a, to: b, label: call}\n  - {from: b, to: q, label: fire, style: async}\n  - {from: b, to: a, label: done, style: reply}\n",
        );
        let s = l.sequence_info.unwrap();
        // Only the call that gets a reply opens a bar (the async one doesn't).
        assert_eq!(s.activations.len(), 1, "{:?}", s.activations);
        let bar = &s.activations[0];
        assert_eq!(bar.node, c.node_map["b"]);
        assert!((bar.y0 - s.messages[0].y).abs() < 1e-6 && (bar.y1 - s.messages[2].y).abs() < 1e-6);
        // The call ends on the bar's left edge, not the lifeline.
        assert!((s.messages[0].to_x - bar.x).abs() < 1e-6);
    }

    #[test]
    fn fragments_notes_and_created_participants() {
        let (c, l) = layout(
            "participants:\n  - {id: a, label: A}\n  - {id: b, label: B}\n\
             sequence:\n  - {note: \"a long note that has to wrap onto more than one line\", over: [a, b]}\n  - alt: ok\n    steps:\n      - {from: a, to: b, label: make, create: true}\n    else:\n      label: fail\n      steps:\n        - {from: a, to: a, label: retry}\n",
        );
        let s = l.sequence_info.unwrap();
        assert_eq!(s.notes.len(), 1);
        assert!(s.notes[0].lines.len() > 1);
        assert_eq!(s.fragments.len(), 1);
        let f = &s.fragments[0];
        assert_eq!(f.separators.len(), 1);
        // Everything inside the fragment is inside its frame.
        for m in &s.messages {
            assert!(m.y > f.y && m.y < f.y + f.h);
            assert!(m.from_x.min(m.to_x) > f.x && m.from_x.max(m.to_x) < f.x + f.w);
        }
        // `b` is created by the message: its box is down at that message, not in the top row.
        let b = &l.positions[&c.node_map["b"]];
        let a = &l.positions[&c.node_map["a"]];
        assert!(b.y > a.y + a.height);
        assert!((b.y + b.height / 2.0 - s.messages[0].y).abs() < 1e-6);
    }
}
