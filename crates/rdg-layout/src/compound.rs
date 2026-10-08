//! Compound layout for diagrams with visual groups (containers/swimlanes), nested to any
//! depth, flowing top-to-bottom or left-to-right.
//!
//! Groups are laid out inside-out. A group's content — its own nodes plus the boxes of
//! the groups nested in it (its *items*; an inner group counts as one item, wired to
//! whatever its nodes are wired to) — is layered along the flow like a small diagram of
//! its own, then padded into a box no narrower than its title.
//!
//! The top level — outermost groups, plus a borderless one-node unit for every
//! ungrouped node — is stacked in tiers by the flow between them, the way a person
//! arranges tiers: a group sits one tier below whatever feeds it, groups that call each
//! other both ways share a tier, and a group that only sends is pulled down to just
//! above what it calls. Within a tier every unit slides across the flow toward the nodes
//! its edges connect to, and inside every group each item slides toward its own
//! neighbours, so arrows between tiers run short and straight.
//!
//! Everything is computed in a *flow frame* — `main` along the flow, `cross` across it —
//! so one implementation serves both directions: left-to-right is the same layout
//! transposed (a group's title row stays on top either way, see [`Frame::pad`]).

use anyhow::Result;
use petgraph::stable_graph::NodeIndex;
use petgraph::visit::{EdgeRef, IntoEdgeReferences};
use std::collections::{HashMap, HashSet};

use rdg_graph::CompiledGraph;

use crate::groups::{Padding, group_min_width, group_padding, title_clear_x};
use crate::sugiyama::order_layers;
use crate::{LayoutConfig, LayoutDirection, LayoutResult, NodeLayout};

/// A tier may grow to this multiple of the widest single unit before the next unit
/// wraps onto a tier of its own (see also [`ROW_AREA_FACTOR`]).
const ROW_WIDTH_SLACK: f64 = 2.0;
/// A tier may also grow to the square root of this multiple of all units' area — a
/// landscape canvas (about 3:2) rather than a tall stack of one group per tier.
const ROW_AREA_FACTOR: f64 = 2.5;
/// Up to this many edges from one group to a sibling group (both fed by the same unit)
/// is a link drawn across the tier they share, not a reason to stack them.
const SIBLING_LINK_MAX: usize = 2;
/// A group whose items have no edges between them lays them out in a grid (rows of
/// about the square root of their count, landscape) from this many items up, instead of
/// one long row that would widen every tier.
const GRID_FROM: usize = 5;
/// Placement sweeps (units, then items within groups) — enough for positions to settle.
const SWEEPS: usize = 4;
/// A gap widened for crossing edge labels grows to at most this many group gaps.
const MAX_LABEL_GAP_FACTOR: f64 = 5.0;

/// The author's layout hints for one set of items (see `DiagramPayload::same_rank`,
/// `NodeDef::rank`, `GroupDef::rank`).
#[derive(Debug, Default)]
struct RankHints {
    /// Pinned rank per item, by index.
    pinned: Vec<Option<usize>>,
    /// Lists of item indices that share a rank.
    same: Vec<Vec<usize>>,
}

/// Something placed inside a group, or at the top level: a node, or an inner group's box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Item {
    Node(NodeIndex),
    Group(usize),
}

/// Maps real `(x, y)` / `(width, height)` pairs to flow-frame `(cross, main)` pairs —
/// and back, since it's the same swap.
#[derive(Debug, Clone, Copy)]
struct Frame {
    lr: bool,
}

impl Frame {
    fn swap(self, (a, b): (f64, f64)) -> (f64, f64) {
        if self.lr { (b, a) } else { (a, b) }
    }

    /// Real padding per frame side: `[cross before, cross after, main before, main after]`.
    /// The title row is always on top: across the flow when it runs left-to-right.
    fn pad(self, p: Padding) -> [f64; 4] {
        if self.lr { [p.top, p.bottom, p.left, p.right] } else { [p.left, p.right, p.top, p.bottom] }
    }
}

/// A group's laid-out content and the box around it, in frame coordinates.
#[derive(Debug, Clone)]
struct Block {
    /// Outer size `(cross, main)`.
    size: (f64, f64),
    /// Where the content starts, relative to the box corner.
    inset: (f64, f64),
    /// Content extent `(cross, main)`.
    content: (f64, f64),
    /// Items per layer along the flow, each ordered across it.
    layers: Vec<Vec<Item>>,
    /// Item corners relative to the content origin.
    pos: HashMap<Item, (f64, f64)>,
    /// Extra left padding (part of `inset.0`) that keeps nodes fed from above clear
    /// of the title — reported to the renderer as `LayoutResult::group_title_shift`.
    title_shift: f64,
}

/// The layout under construction; every coordinate is in the flow frame.
struct Compound<'a> {
    compiled: &'a CompiledGraph,
    config: &'a LayoutConfig,
    frame: Frame,
    node_size: HashMap<NodeIndex, (f64, f64)>,
    /// Laid-out groups; `None` for a group with nothing inside.
    blocks: Vec<Option<Block>>,
    /// Group holding each node directly.
    node_group: HashMap<NodeIndex, usize>,
    /// Corner of each top-level unit.
    top_pos: HashMap<Item, (f64, f64)>,
}

impl Compound<'_> {
    fn block(&self, g: usize) -> &Block {
        self.blocks[g].as_ref().expect("a group holding an item has a block")
    }

    fn size(&self, it: Item) -> (f64, f64) {
        match it {
            Item::Node(n) => self.node_size[&n],
            Item::Group(g) => self.block(g).size,
        }
    }

    fn container(&self, it: Item) -> Option<usize> {
        match it {
            Item::Node(n) => self.node_group.get(&n).copied(),
            Item::Group(g) => self.compiled.group_tree.parent[g],
        }
    }

    /// The item directly inside `scope` (the top level when `None`) that holds node `n`,
    /// or `None` when `n` is outside `scope`.
    fn item_in(&self, scope: Option<usize>, n: NodeIndex) -> Option<Item> {
        let mut it = Item::Node(n);
        loop {
            match self.container(it) {
                c if c == scope => return Some(it),
                Some(p) => it = Item::Group(p),
                None => return None,
            }
        }
    }

    /// The item directly inside `scope` that holds the node or group with this id.
    fn item_for_id(&self, scope: Option<usize>, id: &str) -> Option<Item> {
        if let Some(&n) = self.compiled.node_map.get(id) {
            return self.item_in(scope, n);
        }
        let g = self.compiled.groups.iter().position(|g| g.id == id)?;
        let mut it = Item::Group(g);
        loop {
            match self.container(it) {
                c if c == scope => return Some(it),
                Some(p) => it = Item::Group(p),
                None => return None,
            }
        }
    }

    /// The author's rank hints for `items` (inside `scope`): pinned ranks per item and
    /// the `same_rank` lists resolved to item indices.
    fn hints(&self, scope: Option<usize>, items: &[Item], index: &HashMap<Item, usize>) -> RankHints {
        let pinned = items
            .iter()
            .map(|it| match *it {
                Item::Node(n) => self.compiled.graph[n].rank.map(|r| r as usize),
                Item::Group(g) => self.compiled.groups[g].rank.map(|r| r as usize),
            })
            .collect();
        let same = self
            .compiled
            .same_rank
            .iter()
            .map(|ids| {
                let mut set: Vec<usize> = ids.iter().filter_map(|id| self.item_for_id(scope, id)).filter_map(|it| index.get(&it).copied()).collect();
                set.sort_unstable();
                set.dedup();
                set
            })
            .filter(|s| s.len() > 1)
            .collect();
        RankHints { pinned, same }
    }

    fn corner(&self, it: Item) -> (f64, f64) {
        match self.container(it) {
            None => self.top_pos[&it],
            Some(g) => {
                let (o, b) = (self.corner(Item::Group(g)), self.block(g));
                let p = b.pos[&it];
                (o.0 + b.inset.0 + p.0, o.1 + b.inset.1 + p.1)
            }
        }
    }

    fn center(&self, n: NodeIndex) -> (f64, f64) {
        let (c, s) = (self.corner(Item::Node(n)), self.node_size[&n]);
        (c.0 + s.0 / 2.0, c.1 + s.1 / 2.0)
    }

    /// Where group `g`'s content starts.
    fn origin(&self, g: usize) -> (f64, f64) {
        let (o, b) = (self.corner(Item::Group(g)), self.block(g));
        (o.0 + b.inset.0, o.1 + b.inset.1)
    }

    /// Room a labelled edge needs *across* the flow, between two boxes side by side: the
    /// label's width top-to-bottom (it sits beside a vertical leg), its height left-to-right.
    fn label_across(&self, label: &str) -> f64 {
        let (w, h) = label_box(label, &self.config.tokens);
        if self.frame.lr { h } else { w }
    }

    /// Room a labelled edge needs *along* the flow, between two consecutive ranks:
    /// nothing top-to-bottom (labels sit beside the vertical leg), the label's width
    /// left-to-right (it sits above the horizontal leg crossing the gap).
    fn label_along(&self, label: &str) -> f64 {
        if self.frame.lr { label_box(label, &self.config.tokens).0 } else { 0.0 }
    }

    /// A label's own size plus a stub at each end and a little air.
    fn label_room(&self) -> f64 {
        2.0 * self.config.tokens.stub_clearance() + self.config.tokens.px(1.0)
    }

    /// Lays group `g`'s items out in layers along the flow and wraps them in its box.
    fn layout_group(&mut self, g: usize) {
        let mut items: Vec<Item> = self.compiled.group_nodes[g].iter().map(|&n| Item::Node(n)).collect();
        items.extend(self.compiled.group_tree.children(g).filter(|&c| self.blocks[c].is_some()).map(Item::Group));
        if items.is_empty() {
            return;
        }
        let (layers, pos, content) = self.arrange(g, &items);
        let (w, h) = self.frame.swap(content);
        let pad = self.frame.pad(group_padding(&self.config.tokens, w, h));
        let mut inset = (pad[0], pad[2]);
        let mut size = (content.0 + pad[0] + pad[1], content.1 + pad[2] + pad[3]);
        let group = &self.compiled.groups[g];
        // Top-to-bottom, an arrow dropping in from above onto a node in the top layer
        // must not cross the title: the content is shifted right until every such
        // node's centre is past the title's end (real width is the cross axis
        // top-to-bottom; left-to-right arrows arrive from the side, under the title).
        let mut title_shift = 0.0_f64;
        if !self.frame.lr {
            let clear = title_clear_x(group, &self.config.tokens);
            let fed = layers.first().map_or(Vec::new(), |top| self.fed_items(g, top));
            for it in fed {
                title_shift = title_shift.max(clear - (inset.0 + pos[&it].0 + self.size(it).0 / 2.0));
            }
            title_shift = title_shift.max(0.0).round();
            inset.0 += title_shift;
            size.0 += title_shift;
        }
        // At least as wide as the title, widened evenly so the content stays centred.
        let min_w = group_min_width(group, &self.config.tokens);
        let (width, start) = if self.frame.lr { (&mut size.1, &mut inset.1) } else { (&mut size.0, &mut inset.0) };
        if *width < min_w {
            *start += (min_w - *width) / 2.0;
            *width = min_w;
        }
        self.blocks[g] = Some(Block { size, inset, content, layers, pos, title_shift });
    }

    /// The `top` items of group `g` an edge enters from upstream: from outside the
    /// group, with nothing in the group sending back to where it came from (a group
    /// called both ways shares its tier and is entered from the side).
    fn fed_items(&self, g: usize, top: &[Item]) -> Vec<Item> {
        let within = |n: NodeIndex| self.item_in(Some(g), n).is_some();
        // The smallest scope holding both the group and `n`, and `n`'s item there.
        let source_item = |n: NodeIndex| {
            let mut scope = self.compiled.group_tree.parent[g];
            loop {
                if let Some(it) = self.item_in(scope, n) {
                    return (scope, it);
                }
                scope = scope.and_then(|s| self.compiled.group_tree.parent[s]);
            }
        };
        let mut fed: Vec<Item> = Vec::new();
        for e in self.compiled.graph.edge_references() {
            let (a, b) = (e.source(), e.target());
            if within(a) {
                continue;
            }
            let Some(it) = self.item_in(Some(g), b).filter(|it| top.contains(it)) else { continue };
            let (scope, from) = source_item(a);
            let answered = self.compiled.graph.edge_references().any(|r| within(r.source()) && !within(r.target()) && self.item_in(scope, r.target()) == Some(from));
            if !answered && !fed.contains(&it) {
                fed.push(it);
            }
        }
        fed
    }

    /// Layered arrangement of `items` inside group `g`: ranks by the edges between them,
    /// median ordering, each layer centred across the flow, gaps sized for the edges
    /// (and, left-to-right, their labels) crossing them.
    #[allow(clippy::type_complexity)]
    fn arrange(&self, g: usize, items: &[Item]) -> (Vec<Vec<Item>>, HashMap<Item, (f64, f64)>, (f64, f64)) {
        let index: HashMap<Item, usize> = items.iter().enumerate().map(|(i, &it)| (it, i)).collect();
        let mut edges: Vec<(Item, Item)> = Vec::new();
        let mut rank_edges: Vec<(Item, Item)> = Vec::new();
        let mut labels: Vec<(Item, Item, &str)> = Vec::new();
        for e in self.compiled.graph.edge_references() {
            let (Some(a), Some(b)) = (self.item_in(Some(g), e.source()), self.item_in(Some(g), e.target())) else {
                continue;
            };
            if a != b {
                edges.push((a, b));
                if e.weight().constraint {
                    rank_edges.push((a, b));
                }
                if let Some(l) = e.weight().label.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
                    labels.push((a, b, l));
                }
            }
        }
        let is_group: Vec<bool> = items.iter().map(|it| matches!(it, Item::Group(_))).collect();
        let hints = self.hints(Some(g), items, &index);
        let mut rank = unit_ranks(items.len(), &rank_edges.iter().map(|(a, b)| (index[a], index[b])).collect::<Vec<_>>(), &is_group, &hints);
        let mut n_ranks = rank.iter().copied().max().map_or(0, |m| m + 1);
        // Unwired items (a tier of stores, a box of replicas): a landscape grid.
        if n_ranks == 1 && edges.is_empty() && items.len() >= GRID_FROM {
            let n = items.len();
            let rows = n.div_ceil(((n as f64 * 2.0).sqrt()).ceil() as usize);
            let cols = n.div_ceil(rows);
            rank = (0..n).map(|i| i / cols).collect();
            n_ranks = rows;
        }
        let mut layers: Vec<Vec<Item>> = vec![Vec::new(); n_ranks];
        for (i, &it) in items.iter().enumerate() {
            layers[rank[i]].push(it);
        }
        order_layers(&mut layers, &edges);

        let rank_of = |it: &Item| rank[index[it]];
        let mut gaps = crate::gaps::gaps_for_spans(n_ranks, edges.iter().map(|(a, b)| (rank_of(a), rank_of(b))), self.config);
        for &(a, b, l) in &labels {
            let (ra, rb) = (rank_of(&a), rank_of(&b));
            if ra.abs_diff(rb) == 1 {
                let gap = &mut gaps[ra.min(rb)];
                *gap = gap.max(self.label_along(l) + self.label_room());
            }
        }

        let spacing = self.config.node_spacing as f64;
        let cross_len = |layer: &[Item]| -> f64 {
            layer.iter().map(|&it| self.size(it).0).sum::<f64>() + spacing * layer.len().saturating_sub(1) as f64
        };
        let widest = layers.iter().map(|l| cross_len(l)).fold(0.0, f64::max);
        let mut pos = HashMap::new();
        let mut main = 0.0;
        for (r, layer) in layers.iter().enumerate() {
            let depth = layer.iter().map(|&it| self.size(it).1).fold(0.0, f64::max);
            let mut cross = (widest - cross_len(layer)) / 2.0;
            for &it in layer {
                let (c, m) = self.size(it);
                pos.insert(it, (cross, main + (depth - m) / 2.0));
                cross += c + spacing;
            }
            main += depth + if r + 1 < n_ranks { gaps[r] } else { 0.0 };
        }
        (layers, pos, (widest, main))
    }

    /// Slides the items of every layer of group `g` across the flow toward their
    /// neighbours outside that layer, reordering them by where they want to be.
    fn settle_group(&mut self, g: usize) {
        let origin = self.origin(g).0;
        let (content_cross, layer_count) = (self.block(g).content.0, self.block(g).layers.len());
        let gap = self.config.node_spacing as f64;
        for li in 0..layer_count {
            let layer = self.block(g).layers[li].clone();
            let mut desired: HashMap<Item, (f64, f64)> = HashMap::new();
            let mut order_key: HashMap<Item, f64> = HashMap::new();
            for &it in &layer {
                let (mut sum, mut cnt, mut centre_sum) = (0.0, 0.0, 0.0);
                let item_cross = self.corner(it).0;
                for e in self.compiled.graph.edge_references() {
                    for (mine, theirs) in [(e.source(), e.target()), (e.target(), e.source())] {
                        if mine == theirs || self.item_in(Some(g), mine) != Some(it) {
                            continue;
                        }
                        if self.item_in(Some(g), theirs).is_some_and(|t| layer.contains(&t)) {
                            continue;
                        }
                        // Where this item's corner would put `mine` straight across from `theirs`.
                        let offset = self.center(mine).0 - item_cross;
                        sum += self.center(theirs).0 - offset - origin;
                        centre_sum += self.center(theirs).0;
                        cnt += 1.0;
                    }
                }
                let cur = self.block(g).pos[&it].0;
                desired.insert(it, if cnt > 0.0 { (sum / cnt, cnt) } else { (cur, 0.05) });
                // Ordered by where the neighbours are (ties — siblings fed by one node —
                // keep their declared order); placed by the offset-aware target.
                order_key.insert(it, if cnt > 0.0 { (centre_sum / cnt * 4.0).round() / 4.0 } else { origin + cur + self.size(it).0 / 2.0 });
            }
            let mut sorted = layer;
            sorted.sort_by(|a, b| order_key[a].total_cmp(&order_key[b]).then(a.cmp(b)));
            let widths: Vec<f64> = sorted.iter().map(|&it| self.size(it).0).collect();
            let used = widths.iter().sum::<f64>() + gap * widths.len().saturating_sub(1) as f64;
            let want: Vec<f64> = sorted.iter().map(|it| desired[it].0).collect();
            let wts: Vec<f64> = sorted.iter().map(|it| desired[it].1).collect();
            let xs = place_in_order(&widths, &want, &wts, &vec![gap; widths.len().saturating_sub(1)], Some((0.0, content_cross - used)));
            let block = self.blocks[g].as_mut().expect("settled group has a block");
            for (it, x) in sorted.iter().zip(xs) {
                block.pos.get_mut(it).expect("layer item has a position").0 = x;
            }
            block.layers[li] = sorted;
        }
    }
}

/// Size of an edge label as the annotation pass will draw it: a long one-line label is
/// wrapped onto two lines (see `edge_label_wrap_chars`).
fn label_box(label: &str, tokens: &crate::DesignTokens) -> (f64, f64) {
    let mut lines = label.lines().count().max(1);
    let mut longest = label.lines().map(|l| l.chars().count()).max().unwrap_or(0);
    if lines == 1 && longest > tokens.edge_label_wrap_chars {
        let longest_word = label.split_whitespace().map(|w| w.chars().count()).max().unwrap_or(0);
        longest = longest.div_ceil(2).max(longest_word) + 2;
        lines = 2;
    }
    let font = tokens.edge_label_font_size;
    (longest as f64 * tokens.char_width(font), lines as f64 * tokens.line_height(font) + 2.0)
}

/// Weighted isotonic placement of boxes in a fixed left-to-right order: minimises
/// `Σ weight·(x - desired)²` subject to box `i+1` starting at least `gaps[i]` after
/// box `i` ends (pool-adjacent-violators on gap-shifted coordinates), then clamps
/// into `[lo, hi]` (on the shifted axis, when bounds are given). Returns left edges.
pub(crate) fn place_in_order(widths: &[f64], desired: &[f64], weights: &[f64], gaps: &[f64], bounds: Option<(f64, f64)>) -> Vec<f64> {
    let n = widths.len();
    let mut prefix = vec![0.0; n];
    for i in 1..n {
        prefix[i] = prefix[i - 1] + widths[i - 1] + gaps[i - 1];
    }
    // Blocks of (weight sum, weighted target sum, count).
    let mut blocks: Vec<(f64, f64, usize)> = Vec::with_capacity(n);
    for i in 0..n {
        let w = weights[i].max(1e-6);
        blocks.push((w, w * (desired[i] - prefix[i]), 1));
        while blocks.len() >= 2 {
            let (w1, s1, c1) = blocks[blocks.len() - 1];
            let (w0, s0, c0) = blocks[blocks.len() - 2];
            if s0 / w0 > s1 / w1 {
                blocks.pop();
                *blocks.last_mut().unwrap() = (w0 + w1, s0 + s1, c0 + c1);
            } else {
                break;
            }
        }
    }
    let mut out = Vec::with_capacity(n);
    for (w, s, c) in blocks {
        let mut y = s / w;
        if let Some((lo, hi)) = bounds {
            y = y.clamp(lo, hi.max(lo));
        }
        for _ in 0..c {
            let i = out.len();
            out.push(y + prefix[i]);
        }
    }
    out
}

/// Longest-path rank per unit over the unit graph, with mutually-reachable units
/// (strongly connected components) sharing one rank, and pure sources pulled down to
/// sit directly above the earliest unit they feed.
///
/// Two units wired both ways follow the majority: the direction with more edges ranks
/// them (a pipeline stage that writes four stores and reads one back sits *above* the
/// stores); only a tie keeps them side by side on one tier.
///
/// Two *groups* fed by the same unit (sibling regions under one router, say) stay side
/// by side when only a light link (at most [`SIBLING_LINK_MAX`] edges) runs from one to
/// the other — a replication arrow between regions is drawn across, not as a reason to
/// stack the second region under the first.
fn unit_ranks(n: usize, edges: &[(usize, usize)], is_group: &[bool], hints: &RankHints) -> Vec<usize> {
    use petgraph::algo::tarjan_scc;
    use petgraph::graph::DiGraph;
    let mut count: HashMap<(usize, usize), usize> = HashMap::new();
    for &(a, b) in edges {
        *count.entry((a, b)).or_default() += 1;
    }
    let edges: Vec<(usize, usize)> = edges
        .iter()
        .copied()
        .filter(|&(a, b)| count.get(&(b, a)).is_none_or(|&back| back <= count[&(a, b)]))
        .collect();
    let feeders = |u: usize| edges.iter().filter(move |&&(p, q)| q == u && p != u).map(|&(p, _)| p);
    let siblings = |a: usize, b: usize| feeders(a).any(|p| p != b && feeders(b).any(|q| q == p));
    let edges: Vec<(usize, usize)> = edges
        .iter()
        .copied()
        .filter(|&(a, b)| !(is_group[a] && is_group[b] && count[&(a, b)] <= SIBLING_LINK_MAX && siblings(a, b)))
        .collect();
    let edges = &edges[..];
    let mut g: DiGraph<(), ()> = DiGraph::new();
    let idx: Vec<_> = (0..n).map(|_| g.add_node(())).collect();
    for &(a, b) in edges {
        g.add_edge(idx[a], idx[b], ());
    }
    // tarjan_scc returns components in reverse topological order.
    let sccs = tarjan_scc(&g);
    let mut comp = vec![0usize; n];
    for (ci, c) in sccs.iter().enumerate() {
        for v in c {
            comp[v.index()] = ci;
        }
    }
    let nc = sccs.len();
    let mut preds: Vec<HashSet<usize>> = vec![HashSet::new(); nc];
    let mut succs: Vec<HashSet<usize>> = vec![HashSet::new(); nc];
    for &(a, b) in edges {
        let (ca, cb) = (comp[a], comp[b]);
        if ca != cb {
            preds[cb].insert(ca);
            succs[ca].insert(cb);
        }
    }
    let mut rank = vec![0usize; nc];
    for c in (0..nc).rev() {
        rank[c] = preds[c].iter().map(|&p| rank[p] + 1).max().unwrap_or(0);
    }
    for c in 0..nc {
        if preds[c].is_empty() {
            if let Some(m) = succs[c].iter().map(|&s| rank[s]).min() {
                rank[c] = m.saturating_sub(1);
            }
        }
    }
    let mut rank: Vec<usize> = (0..n).map(|u| rank[comp[u]]).collect();
    // Author's hints: pinned ranks hold, `same_rank` lists level to their deepest
    // member, and whatever either feeds is pushed down again until nothing moves.
    let pinned: Vec<Option<usize>> = (0..n).map(|u| hints.pinned.get(u).copied().flatten()).collect();
    if pinned.iter().any(Option::is_some) || !hints.same.is_empty() {
        for (u, p) in pinned.iter().enumerate() {
            if let Some(r) = p {
                rank[u] = *r;
            }
        }
        for _ in 0..(n + hints.same.len() + 2) {
            let mut changed = false;
            for &(a, b) in edges {
                if comp[a] != comp[b] && pinned[b].is_none() && rank[b] < rank[a] + 1 {
                    rank[b] = rank[a] + 1;
                    changed = true;
                }
            }
            for set in &hints.same {
                let m = set.iter().filter_map(|&u| rank.get(u)).copied().max().unwrap_or(0);
                for &u in set {
                    if u < n && rank[u] != m {
                        rank[u] = m;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
    }
    rank
}

/// Tiered placement for diagrams with visual groups — see the module docs.
pub(crate) fn layout_compound(compiled: &CompiledGraph, config: &LayoutConfig) -> Result<LayoutResult> {
    let frame = Frame { lr: config.direction == LayoutDirection::LeftToRight };
    let node_size = compiled.graph.node_indices().map(|n| (n, frame.swap(crate::layout_node_size(compiled, n, config)))).collect();
    let node_group = (0..compiled.groups.len()).flat_map(|g| compiled.group_nodes[g].iter().map(move |&n| (n, g))).collect();
    let mut c = Compound { compiled, config, frame, node_size, blocks: vec![None; compiled.groups.len()], node_group, top_pos: HashMap::new() };
    let outer_first = compiled.group_tree.outer_first();
    for &g in outer_first.iter().rev() {
        c.layout_group(g);
    }

    // --- Top-level units: outermost groups, then ungrouped nodes --------------------
    let mut units: Vec<Item> = outer_first
        .iter()
        .filter(|&&g| compiled.group_tree.parent[g].is_none() && c.blocks[g].is_some())
        .map(|&g| Item::Group(g))
        .collect();
    let mut ungrouped: Vec<NodeIndex> = compiled.graph.node_indices().filter(|n| !c.node_group.contains_key(n)).collect();
    ungrouped.sort();
    units.extend(ungrouped.into_iter().map(Item::Node));
    let nu = units.len();
    let unit_index: HashMap<Item, usize> = units.iter().enumerate().map(|(i, &u)| (u, i)).collect();
    let unit_of: HashMap<NodeIndex, usize> =
        compiled.graph.node_indices().map(|n| (n, unit_index[&c.item_in(None, n).expect("every node is in a top-level unit")])).collect();
    let unit_size: Vec<(f64, f64)> = units.iter().map(|&u| c.size(u)).collect();
    let usize_of = |u: usize| unit_size[u];
    let bordered = |u: usize| matches!(units[u], Item::Group(_));
    let (gap_cross, gap_main) = frame.swap((config.group_gap_x, config.group_gap_y));

    // Cross-unit edges, node level (for positions) and unit level (for ranks), and the
    // room each labelled one needs across / along the flow.
    let mut cross: Vec<(NodeIndex, NodeIndex)> = Vec::new();
    let mut unit_edges: Vec<(usize, usize)> = Vec::new();
    let mut labelled: Vec<(usize, usize, f64, f64)> = Vec::new();
    for e in compiled.graph.edge_references() {
        let (a, b) = (e.source(), e.target());
        let (ua, ub) = (unit_of[&a], unit_of[&b]);
        if ua == ub {
            continue;
        }
        cross.push((a, b));
        if e.weight().constraint {
            unit_edges.push((ua, ub));
        }
        if let Some(l) = e.weight().label.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
            labelled.push((ua, ub, c.label_across(l), c.label_along(l)));
        }
    }
    let is_group: Vec<bool> = (0..nu).map(bordered).collect();
    let hints = c.hints(None, &units, &unit_index);
    let rank = unit_ranks(nu, &unit_edges, &is_group, &hints);

    // --- Tiers: one per rank, wrapped when too wide ------------------------------
    let max_w = (0..nu).map(|u| usize_of(u).0).fold(0.0, f64::max);
    let area: f64 = (0..nu).map(|u| usize_of(u).0 * usize_of(u).1).sum();
    // Always room for two of the widest units side by side.
    let row_budget = (max_w * ROW_WIDTH_SLACK + gap_cross).max((area * ROW_AREA_FACTOR).sqrt());
    let max_rank = rank.iter().copied().max().unwrap_or(0);
    let mut rows: Vec<Vec<usize>> = Vec::new();
    for r in 0..=max_rank {
        let mut members: Vec<usize> = (0..nu).filter(|&u| rank[u] == r).collect();
        if members.is_empty() {
            continue;
        }
        // Keep the units most tied to the tiers above; wrap the rest below them.
        let from_above = |u: usize| unit_edges.iter().filter(|&&(a, b)| b == u && rank[a] < r).count();
        members.sort_by_key(|&u| (std::cmp::Reverse(from_above(u)), u));
        let mut row: Vec<usize> = Vec::new();
        let mut w = 0.0;
        for u in members {
            let add = if row.is_empty() { usize_of(u).0 } else { gap_cross + usize_of(u).0 };
            if !row.is_empty() && w + add > row_budget {
                row.sort_unstable();
                rows.push(std::mem::take(&mut row));
                w = usize_of(u).0;
            } else {
                w += add;
            }
            row.push(u);
        }
        row.sort_unstable();
        rows.push(row);
    }
    let mut row_of = vec![0usize; nu];
    for (ri, row) in rows.iter().enumerate() {
        for &u in row {
            row_of[u] = ri;
        }
    }

    // --- Across the flow ------------------------------------------------------------
    for row in &rows {
        let mut x = 0.0;
        for &u in row {
            c.top_pos.insert(units[u], (x, 0.0));
            x += usize_of(u).0 + gap_cross;
        }
    }
    for sweep in 0..SWEEPS {
        // Units: forward, then back; the first pass only looks at tiers already settled.
        let order: Vec<usize> = if sweep % 2 == 0 { (0..rows.len()).collect() } else { (0..rows.len()).rev().collect() };
        for &ri in &order {
            let settled = |other: usize| {
                if sweep == 0 { row_of[other] < ri } else if sweep == 1 { row_of[other] > ri } else { true }
            };
            // Two targets per unit: where its *neighbours* are (the order across the
            // tier — so two groups fed by one node tie exactly and keep their declared
            // order), and where its corner would put each wired node straight across from
            // its partner (the position within that order).
            let mut desired_sum: HashMap<usize, (f64, f64)> = HashMap::new();
            let mut order_sum: HashMap<usize, (f64, f64)> = HashMap::new();
            for &(a, b) in &cross {
                for (mine, theirs) in [(a, b), (b, a)] {
                    let (um, ut) = (unit_of[&mine], unit_of[&theirs]);
                    if row_of[um] != ri || !settled(ut) || (row_of[ut] == ri && sweep < 2) {
                        continue;
                    }
                    let offset = c.center(mine).0 - c.top_pos[&units[um]].0;
                    let e = desired_sum.entry(um).or_insert((0.0, 0.0));
                    e.0 += c.center(theirs).0 - offset;
                    e.1 += 1.0;
                    let o = order_sum.entry(um).or_insert((0.0, 0.0));
                    o.0 += c.center(theirs).0;
                    o.1 += 1.0;
                }
            }
            let mut row = rows[ri].clone();
            let desired: HashMap<usize, (f64, f64)> = row
                .iter()
                .map(|&u| match desired_sum.get(&u) {
                    Some(&(s, w)) => (u, (s / w, w)),
                    None => (u, (c.top_pos[&units[u]].0, 0.05)),
                })
                .collect();
            let order_key = |u: usize| match order_sum.get(&u) {
                Some(&(s, w)) => (s / w * 4.0).round() / 4.0,
                None => c.top_pos[&units[u]].0 + usize_of(u).0 / 2.0,
            };
            row.sort_by(|&a, &b| order_key(a).total_cmp(&order_key(b)).then(a.cmp(&b)));
            // Then swap neighbours while that removes crossings with the settled tiers
            // (a wide group's barycentre can put a narrow one on the far side of edges
            // it then has to cross).
            let crossings = |row: &[usize]| -> usize {
                let idx: HashMap<usize, usize> = row.iter().enumerate().map(|(i, &u)| (u, i)).collect();
                let mut ends: Vec<(usize, usize, f64)> = Vec::new();
                for &(a, b) in &cross {
                    for (mine, theirs) in [(a, b), (b, a)] {
                        let (um, ut) = (unit_of[&mine], unit_of[&theirs]);
                        if row_of[um] != ri || !settled(ut) || row_of[ut] == ri {
                            continue;
                        }
                        ends.push((row_of[ut], idx[&um], c.center(theirs).0));
                    }
                }
                let mut n = 0;
                for i in 0..ends.len() {
                    for j in i + 1..ends.len() {
                        let ((ra, ia, xa), (rb, ib, xb)) = (ends[i], ends[j]);
                        if ra == rb && ia != ib && (xa - xb).abs() > 0.5 && ((ia < ib) != (xa < xb)) {
                            n += 1;
                        }
                    }
                }
                n
            };
            for _ in 0..4 {
                let mut improved = false;
                for k in 0..row.len().saturating_sub(1) {
                    let before = crossings(&row);
                    if before == 0 {
                        break;
                    }
                    row.swap(k, k + 1);
                    if crossings(&row) < before {
                        improved = true;
                    } else {
                        row.swap(k, k + 1);
                    }
                }
                if !improved {
                    break;
                }
            }
            let widths: Vec<f64> = row.iter().map(|&u| usize_of(u).0).collect();
            let want: Vec<f64> = row.iter().map(|&u| desired[&u].0).collect();
            let wts: Vec<f64> = row.iter().map(|&u| desired[&u].1).collect();
            // A gap crossed by labelled edges must fit the widest label beside its arrow.
            let gaps: Vec<f64> = (1..row.len())
                .map(|i| {
                    let (left, right) = (&row[..i], &row[i..]);
                    labelled
                        .iter()
                        .filter(|&&(a, b, _, _)| (left.contains(&a) && right.contains(&b)) || (left.contains(&b) && right.contains(&a)))
                        .map(|&(_, _, across, _)| across + c.label_room())
                        .fold(gap_cross, f64::max)
                        .min(gap_cross * MAX_LABEL_GAP_FACTOR)
                })
                .collect();
            let xs = place_in_order(&widths, &want, &wts, &gaps, None);
            for (&u, x) in row.iter().zip(xs) {
                c.top_pos.get_mut(&units[u]).expect("unit placed").0 = x;
            }
            rows[ri] = row;
        }
        for &g in &outer_first {
            if c.blocks[g].is_some() {
                c.settle_group(g);
            }
        }
    }

    // --- Along the flow: tiers stacked, gaps sized for the edges crossing them ------
    let row_h: Vec<f64> = rows.iter().map(|r| r.iter().map(|&u| usize_of(u).1).fold(0.0, f64::max)).collect();
    let mut row_y = vec![0.0_f64; rows.len()];
    for ri in 1..rows.len() {
        let spans = |a: usize, b: usize| {
            let (ra, rb) = (row_of[a], row_of[b]);
            ra.min(rb) < ri && ra.max(rb) >= ri
        };
        let crossing = cross.iter().filter(|(a, b)| spans(unit_of[a], unit_of[b])).count();
        let has_border = rows[ri - 1].iter().chain(&rows[ri]).any(|&u| bordered(u));
        // Most edges crossing a tier gap drop straight through it; only those that turn
        // there need a lane of their own — about half, in practice.
        let mut gap = crate::gaps::gap_for(crossing.div_ceil(2), config).max(if has_border { gap_main } else { 0.0 });
        for &(a, b, _, along) in &labelled {
            if row_of[a].abs_diff(row_of[b]) == 1 && spans(a, b) {
                gap = gap.max((along + c.label_room()).min(gap_main * MAX_LABEL_GAP_FACTOR));
            }
        }
        row_y[ri] = row_y[ri - 1] + row_h[ri - 1] + gap;
    }
    // Within its tier, a shallower unit hugs the side its edges leave from — or, when
    // it is wired to units in its own tier, lines up with those.
    let offset_main = |c: &Compound, n: NodeIndex| c.center(n).1 - c.top_pos[&units[unit_of[&n]]].1;
    for (ri, row) in rows.iter().enumerate() {
        for &u in row {
            let toward = |later: bool| {
                cross
                    .iter()
                    .filter(|(a, b)| {
                        let other = |x: &NodeIndex, y: &NodeIndex| unit_of[x] == u && if later { row_of[unit_of[y]] > ri } else { row_of[unit_of[y]] < ri };
                        other(a, b) || other(b, a)
                    })
                    .count()
            };
            let slack = row_h[ri] - usize_of(u).1;
            c.top_pos.get_mut(&units[u]).expect("unit placed").1 = row_y[ri] + if toward(true) >= toward(false) { slack } else { 0.0 };
        }
    }
    for (ri, row) in rows.iter().enumerate() {
        for &u in row {
            let slack = row_h[ri] - usize_of(u).1;
            if slack <= 0.0 {
                continue;
            }
            let (mut sum, mut cnt) = (0.0, 0.0);
            for &(a, b) in &cross {
                for (mine, theirs) in [(a, b), (b, a)] {
                    let ut = unit_of[&theirs];
                    if unit_of[&mine] != u || row_of[ut] != ri || usize_of(ut).1 < usize_of(u).1 {
                        continue;
                    }
                    sum += c.center(theirs).1 - offset_main(&c, mine);
                    cnt += 1.0;
                }
            }
            if cnt > 0.0 {
                c.top_pos.get_mut(&units[u]).expect("unit placed").1 = (sum / cnt).clamp(row_y[ri], row_y[ri] + slack);
            }
        }
    }

    // --- Real coordinates (`normalize_positions` moves them onto the margin) --------
    let positions = compiled
        .graph
        .node_indices()
        .map(|n| {
            let (x, y) = frame.swap(c.corner(Item::Node(n)));
            let (width, height) = frame.swap(c.node_size[&n]);
            (n, NodeLayout { x, y, width, height })
        })
        .collect();
    let group_widths = c.blocks.iter().enumerate().filter_map(|(g, b)| Some((g, frame.swap(b.as_ref()?.size).0))).collect();
    let group_title_shift = c.blocks.iter().enumerate().filter_map(|(g, b)| b.as_ref().filter(|b| b.title_shift > 0.0).map(|b| (g, b.title_shift))).collect();
    Ok(LayoutResult { positions, group_widths, group_title_shift, ..Default::default() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn place_in_order_respects_gaps_and_pulls_toward_targets() {
        // Both want x=0: they share the overlap cost symmetrically.
        let xs = place_in_order(&[10.0, 10.0], &[0.0, 0.0], &[1.0, 1.0], &[5.0], None);
        assert_eq!(xs, vec![-7.5, 7.5]);
        // No conflict: each lands exactly where wanted.
        let xs = place_in_order(&[10.0, 10.0], &[0.0, 100.0], &[1.0, 1.0], &[5.0], None);
        assert_eq!(xs, vec![0.0, 100.0]);
        // Bounds clamp the whole run.
        let xs = place_in_order(&[10.0], &[-50.0], &[1.0], &[], Some((0.0, 20.0)));
        assert_eq!(xs, vec![0.0]);
    }

    #[test]
    fn mutual_units_follow_the_majority_direction() {
        // 0 -> 1 three times, 1 -> 0 once: 1 sits below 0.
        let r = unit_ranks(2, &[(0, 1), (0, 1), (0, 1), (1, 0)], &[false; 2], &RankHints::default());
        assert_eq!(r, vec![0, 1]);
        // A tie keeps them side by side.
        let r = unit_ranks(2, &[(0, 1), (1, 0)], &[false; 2], &RankHints::default());
        assert_eq!(r[0], r[1]);
    }

    #[test]
    fn sibling_groups_with_a_light_link_share_a_tier_but_nodes_do_not() {
        let edges = [(0, 1), (0, 2), (1, 2)];
        let groups = unit_ranks(3, &edges, &[false, true, true], &RankHints::default());
        assert_eq!(groups[1], groups[2]);
        let nodes = unit_ranks(3, &edges, &[false; 3], &RankHints::default());
        assert_eq!(nodes[2], nodes[1] + 1);
    }

    #[test]
    fn hints_pin_and_level_units() {
        let hints = RankHints { pinned: vec![None, None, Some(0)], same: vec![vec![0, 1]] };
        let r = unit_ranks(3, &[(0, 1), (1, 2)], &[false; 3], &hints);
        assert_eq!(r[0], r[1], "same_rank levels 0 and 1");
        assert_eq!(r[2], 0, "pinned despite being fed");
    }

    #[test]
    fn mutually_calling_units_share_a_rank_and_sources_sit_above_targets() {
        // 0 -> 1 <-> 2 -> 3, and a source 4 that only feeds 3.
        let r = unit_ranks(5, &[(0, 1), (1, 2), (2, 1), (2, 3), (4, 3)], &[false; 5], &RankHints::default());
        assert_eq!(r[1], r[2]);
        assert_eq!(r[0] + 1, r[1]);
        assert_eq!(r[3], r[1] + 1);
        assert_eq!(r[4] + 1, r[3]);
    }
}
