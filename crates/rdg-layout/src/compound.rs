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
use crate::fold::{FOLD_ASPECT, FOLD_MIN_TIERS};
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
    /// Tier of each top-level unit, once the tiers are known.
    unit_row: HashMap<Item, usize>,
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

    /// Whether node `n` can leave group `g`'s box straight along the flow — toward later
    /// layers when `onward`, earlier ones otherwise — i.e. sits on the box's exit layer
    /// at every level of nesting between it and `g`. Otherwise the arrow has to leave
    /// the box sideways and run down a gutter beside it.
    fn exits_straight(&self, g: usize, n: NodeIndex, onward: bool) -> bool {
        let mut it = Item::Node(n);
        loop {
            let Some(h) = self.container(it) else { return true };
            let layers = &self.block(h).layers;
            let edge = if onward { layers.last() } else { layers.first() };
            if !edge.is_some_and(|l| l.contains(&it)) {
                return false;
            }
            if h == g {
                return true;
            }
            it = Item::Group(h);
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
        let group = &self.compiled.groups[g];
        let pad = self.frame.pad(group_padding(&self.config.tokens, group, w, h));
        let mut inset = (pad[0], pad[2]);
        let mut size = (content.0 + pad[0] + pad[1], content.1 + pad[2] + pad[3]);
        // Top-to-bottom, an arrow dropping in from above onto a node in the top layer
        // must not cross the title: the content is shifted right until every such
        // node's centre is past the title's end (real width is the cross axis
        // top-to-bottom; left-to-right arrows arrive from the side, under the title).
        let mut title_shift = 0.0_f64;
        if !self.frame.lr {
            let clear = title_clear_x(group, &self.config.tokens);
            let fed = layers.first().map_or(Vec::new(), |top| self.fed_items(g, top));
            for it in fed {
                // The whole face an arrow may enter by must be past the title: a tile's
                // or logo's mark, the ports' span of a card, an inner group's middle.
                let entry_half = match it {
                    Item::Node(n) => match crate::caption_mark(&self.compiled.graph[n], &self.config.tokens) {
                        Some(mark) => mark / 2.0 + self.config.tokens.px(1.0),
                        None => self.size(it).0 * 0.38,
                    },
                    Item::Group(_) => 0.0,
                };
                title_shift = title_shift.max(clear - (inset.0 + pos[&it].0 + self.size(it).0 / 2.0 - entry_half));
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
        // Top-to-bottom the slot under the group's title is best taken by a top-layer
        // item nothing enters from above: a fed item there forces the whole content
        // right of the title (see `layout_group`), leaving an empty band under it.
        if !self.frame.lr {
            if let Some(top) = layers.first_mut() {
                let fed = self.fed_items(g, top);
                if top.first().is_some_and(|it| fed.contains(it)) {
                    if let Some(i) = top.iter().position(|it| !fed.contains(it)) {
                        let it = top.remove(i);
                        top.insert(0, it);
                    }
                }
            }
        }

        let rank_of = |it: &Item| rank[index[it]];
        // Where each item's centre falls across the flow with every layer packed from
        // the left — what `settle_group` then slides, keeping the order.
        let spacing = self.config.node_spacing as f64;
        let centre = |it: &Item| -> f64 {
            let li = &layers[rank_of(it)];
            let before: f64 = li.iter().take_while(|x| *x != it).map(|&x| self.size(x).0 + spacing).sum();
            before + self.size(*it).0 / 2.0
        };
        let mut gaps = crate::gaps::gaps_for_tracks(n_ranks, edges.iter().map(|(a, b)| ((rank_of(a), rank_of(b)), (centre(a), centre(b)))), self.config);
        for &(a, b, l) in &labels {
            let (ra, rb) = (rank_of(&a), rank_of(&b));
            if ra.abs_diff(rb) == 1 {
                let gap = &mut gaps[ra.min(rb)];
                *gap = gap.max(self.label_along(l) + self.label_room());
            }
        }

        // Gutters within each layer: the configured spacing, widened where edges from
        // a layer above to one below drop through.
        let cross_gaps: Vec<Vec<f64>> = layers
            .iter()
            .enumerate()
            .map(|(r, layer)| {
                let base = vec![spacing; layer.len().saturating_sub(1)];
                let through: Vec<f64> = edges
                    .iter()
                    .filter(|(a, b)| rank_of(a).min(rank_of(b)) < r && rank_of(a).max(rank_of(b)) > r)
                    .map(|(a, b)| (centre(a) + centre(b)) / 2.0)
                    .collect();
                let mut spans = Vec::with_capacity(layer.len());
                let mut x = 0.0;
                for &it in layer {
                    spans.push((x, x + self.size(it).0));
                    x += self.size(it).0 + spacing;
                }
                let bordered: Vec<bool> = layer.iter().map(|it| matches!(it, Item::Group(_))).collect();
                gutters_for_through(&spans, &bordered, &through, &base, &self.config.tokens)
            })
            .collect();
        if std::env::var_os("RDG_DEBUG_GUTTERS").is_some() {
            let depths: Vec<f64> = layers.iter().map(|l| l.iter().map(|&it| self.size(it).1).fold(0.0, f64::max)).collect();
            eprintln!("group {} layers={} depths={depths:?} gaps={gaps:?}", self.compiled.groups[g].id, layers.len());
        }
        let cross_len = |r: usize| -> f64 { layers[r].iter().map(|&it| self.size(it).0).sum::<f64>() + cross_gaps[r].iter().sum::<f64>() };
        let widest = (0..layers.len()).map(cross_len).fold(0.0, f64::max);
        let mut pos = HashMap::new();
        let mut main = 0.0;
        for (r, layer) in layers.iter().enumerate() {
            let offsets = self.layer_offsets(layer);
            let depth = layer.iter().zip(&offsets).map(|(&it, o)| o + self.size(it).1).fold(0.0, f64::max);
            let mut cross = (widest - cross_len(r)) / 2.0;
            for (i, &it) in layer.iter().enumerate() {
                let c = self.size(it).0;
                pos.insert(it, (cross, main + offsets[i]));
                cross += c + cross_gaps[r].get(i).copied().unwrap_or(0.0);
            }
            main += depth + if r + 1 < n_ranks { gaps[r] } else { 0.0 };
        }
        (layers, pos, (widest, main))
    }

    /// How far along the flow each item of a layer starts. Top-to-bottom the marks of
    /// captioned nodes (tiles, logos, flowchart markers) line up — their centres on one
    /// line, the way a row of icons reads — and a box starts level with the marks'
    /// tops; a node with a longer caption just hangs lower. Left-to-right every item is
    /// centred across its column, which centres its mark too.
    fn layer_offsets(&self, layer: &[Item]) -> Vec<f64> {
        let mark = |it: Item| match it {
            Item::Node(n) => crate::caption_mark(&self.compiled.graph[n], &self.config.tokens),
            Item::Group(_) => None,
        };
        if self.frame.lr || layer.iter().all(|&it| mark(it).is_none()) {
            let depth = layer.iter().map(|&it| self.size(it).1).fold(0.0, f64::max);
            return layer.iter().map(|&it| (depth - self.size(it).1) / 2.0).collect();
        }
        let line = layer.iter().filter_map(|&it| mark(it)).fold(0.0, f64::max) / 2.0;
        layer.iter().map(|&it| mark(it).map_or(0.0, |m| line - m / 2.0)).collect()
    }

    /// Refits group `g`'s box across the flow to where its items ended up: settling
    /// slides items toward their neighbours inside a box sized before the slide, which
    /// left an empty band on one side. The box keeps its place on the canvas (its
    /// corner moves by what was trimmed), the padding and the title rule are re-applied
    /// to the settled positions, and nothing inside moves.
    fn tighten_group(&mut self, g: usize) {
        let frame = self.frame;
        let tokens = &self.config.tokens;
        let group = &self.compiled.groups[g];
        let block = self.block(g);
        let items: Vec<Item> = block.pos.keys().copied().collect();
        let (lo, hi) = items.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &it| {
            let x = block.pos[&it].0;
            (lo.min(x), hi.max(x + self.size(it).0))
        });
        if !lo.is_finite() {
            return;
        }
        let content = (hi - lo, block.content.1);
        let (w, h) = frame.swap(content);
        let pad = frame.pad(group_padding(tokens, group, w, h));
        let mut inset0 = pad[0];
        let mut size0 = content.0 + pad[0] + pad[1];
        let mut title_shift = 0.0_f64;
        if !frame.lr {
            let clear = title_clear_x(group, tokens);
            let fed = block.layers.first().map_or(Vec::new(), |top| self.fed_items(g, top));
            for it in fed {
                let entry_half = match it {
                    Item::Node(n) => match crate::caption_mark(&self.compiled.graph[n], tokens) {
                        Some(mark) => mark / 2.0 + tokens.px(1.0),
                        None => self.size(it).0 * 0.38,
                    },
                    Item::Group(_) => 0.0,
                };
                title_shift = title_shift.max(clear - (inset0 + block.pos[&it].0 - lo + self.size(it).0 / 2.0 - entry_half));
            }
            title_shift = title_shift.max(0.0).round();
            inset0 += title_shift;
            size0 += title_shift;
        }
        if !frame.lr {
            let min_w = group_min_width(group, tokens);
            if size0 < min_w {
                inset0 += (min_w - size0) / 2.0;
                size0 = min_w;
            }
        }
        if std::env::var_os("RDG_DEBUG_GUTTERS").is_some() {
            eprintln!("tighten {}: items {lo:.0}..{hi:.0} content {:.0} -> {:.0} inset {:.0} -> {inset0:.0} shift {title_shift:.0} size {:.0} -> {size0:.0}", group.id, block.content.0, content.0, block.inset.0, block.size.0);
        }
        // Items stay put on the canvas: the corner absorbs the trim and the new inset.
        let old_inset = block.inset.0;
        let delta = old_inset + lo - inset0;
        let block = self.blocks[g].as_mut().expect("tightened group has a block");
        for p in block.pos.values_mut() {
            p.0 -= lo;
        }
        block.content.0 = content.0;
        block.inset.0 = inset0;
        block.size.0 = size0;
        block.title_shift = title_shift;
        match self.compiled.group_tree.parent[g] {
            None => self.top_pos.get_mut(&Item::Group(g)).expect("top-level group placed").0 += delta,
            Some(parent) => self.blocks[parent].as_mut().expect("parent block").pos.get_mut(&Item::Group(g)).expect("child placed").0 += delta,
        }
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
            let want: Vec<f64> = sorted.iter().map(|it| desired[it].0).collect();
            let wts: Vec<f64> = sorted.iter().map(|it| desired[it].1).collect();
            // Edges dropping through this layer — from a layer above to one below, or
            // from outside the group past it — keep a lane each in the gutter they use.
            let layer_of = |it: &Item| self.block(g).layers.iter().position(|l| l.contains(it));
            let my_row = self.unit_row.get(&self.item_in(None, match self.compiled.group_nodes[g].first() {
                Some(&n) => n,
                None => return,
            }).expect("group node has a unit")).copied();
            let mut through: Vec<f64> = Vec::new();
            for e in self.compiled.graph.edge_references() {
                let (a, b) = (e.source(), e.target());
                let (ia, ib) = (self.item_in(Some(g), a), self.item_in(Some(g), b));
                let passes = match (ia.and_then(|it| layer_of(&it)), ib.and_then(|it| layer_of(&it))) {
                    (Some(la), Some(lb)) => la.min(lb) < li && la.max(lb) > li,
                    (Some(l), None) | (None, Some(l)) => {
                        let outside = if ia.is_some() { b } else { a };
                        let their_row = self.item_in(None, outside).and_then(|it| self.unit_row.get(&it).copied());
                        match (my_row, their_row) {
                            (Some(mine), Some(theirs)) if theirs < mine => l > li,
                            (Some(mine), Some(theirs)) if theirs > mine => l < li,
                            _ => false,
                        }
                    }
                    _ => false,
                };
                if passes {
                    through.push((self.center(a).0 + self.center(b).0) / 2.0 - origin);
                }
            }
            let mut spans = Vec::with_capacity(sorted.len());
            let mut x = 0.0;
            for &w in &widths {
                spans.push((x, x + w));
                x += w + gap;
            }
            let start = sorted.first().map_or(0.0, |it| self.block(g).pos[it].0);
            let spans: Vec<(f64, f64)> = spans.iter().map(|&(a, b)| (a + start, b + start)).collect();
            let bordered: Vec<bool> = sorted.iter().map(|it| matches!(it, Item::Group(_))).collect();
            let gaps = gutters_for_through(&spans, &bordered, &through, &vec![gap; widths.len().saturating_sub(1)], &self.config.tokens);
            let used = widths.iter().sum::<f64>() + gaps.iter().sum::<f64>();
            let xs = place_in_order(&widths, &want, &wts, &gaps, Some((0.0, (content_cross - used).max(0.0))));
            let block = self.blocks[g].as_mut().expect("settled group has a block");
            for (it, x) in sorted.iter().zip(xs) {
                block.pos.get_mut(it).expect("layer item has a position").0 = x;
            }
            block.layers[li] = sorted;
        }
    }
}

/// Widths of the gutters between boxes placed side by side (`spans` = each box's
/// `(start, end)` across the flow, in order) once the edges dropping *through* this row
/// get a lane each: `through` holds where each such edge would like to cross (the
/// middle of its two ends), and every one is charged to the nearest gutter — the one it
/// falls in, or the closest when it falls on a box. The gutters outside the row's ends
/// are free. A gutter beside a bordered box also keeps a lane's width off that border
/// (where a line would read as tracing it); beside a plain node a stub's clearance.
/// `base[i]` is the gutter's width without any of that (labels, configured spacing).
fn gutters_for_through(spans: &[(f64, f64)], bordered: &[bool], through: &[f64], base: &[f64], tokens: &crate::DesignTokens) -> Vec<f64> {
    let n = spans.len();
    if n < 2 {
        return base.to_vec();
    }
    let mut demand = vec![0usize; n - 1];
    for &x in through {
        if x <= spans[0].0 || x >= spans[n - 1].1 {
            continue;
        }
        let nearest = (1..n)
            .map(|i| {
                let (lo, hi) = (spans[i - 1].1, spans[i].0);
                let d = if x < lo { lo - x } else if x > hi { x - hi } else { 0.0 };
                (i - 1, d)
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i);
        if let Some(i) = nearest {
            demand[i] += 1;
        }
    }
    let lane = tokens.channel_pitch();
    let side = |b: bool| if b { lane * 1.25 } else { tokens.stub_clearance() };
    if std::env::var_os("RDG_DEBUG_GUTTERS").is_some() {
        eprintln!("gutters: spans={spans:?} through={through:?} demand={demand:?}");
    }
    (0..n - 1)
        .map(|i| {
            if demand[i] == 0 {
                return base[i];
            }
            let lanes = (demand[i].min(crate::gaps::MAX_CHANNELS) as f64 + 1.0) * lane;
            base[i].max(lanes + side(bordered[i]) + side(bordered[i + 1]))
        })
        .collect()
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
/// Lays the diagram out with the tier width that makes it squarest: the default
/// budget first, then wider tiers when the result is tall and narrower when it is
/// wide, keeping whichever aspect lies closest to 1 (ties keep the default). A
/// reader sees a square more easily than a strip, and so does a page.
pub(crate) fn layout_compound(compiled: &CompiledGraph, config: &LayoutConfig) -> Result<LayoutResult> {
    let aspect = |r: &LayoutResult| -> f64 {
        let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for nl in r.positions.values() {
            x0 = x0.min(nl.x);
            y0 = y0.min(nl.y);
            x1 = x1.max(nl.x + nl.width);
            y1 = y1.max(nl.y + nl.height);
        }
        if x1 > x0 && y1 > y0 { ((x1 - x0) / (y1 - y0)).ln().abs() } else { 0.0 }
    };
    let base = layout_compound_with(compiled, config, 1.0)?;
    let mut best = (aspect(&base), base);
    if best.0 < 0.4_f64.ln().abs() {
        return Ok(best.1);
    }
    // Tall (more height than width along the page) wants wider tiers; wide wants
    // narrower ones. The frame decides which way "wider tiers" goes.
    let tall = {
        let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for nl in best.1.positions.values() {
            x0 = x0.min(nl.x);
            y0 = y0.min(nl.y);
            x1 = x1.max(nl.x + nl.width);
            y1 = y1.max(nl.y + nl.height);
        }
        (y1 - y0) > (x1 - x0)
    };
    let lr = config.direction == LayoutDirection::LeftToRight;
    // Top-to-bottom a tall page needs wider tiers; left-to-right the tiers run down
    // the page, so a tall page needs narrower ones.
    let scales: &[f64] = if tall != lr { &[1.5, 2.2, 3.2] } else { &[0.7, 0.5, 0.35] };
    for &k in scales {
        let r = layout_compound_with(compiled, config, k)?;
        let a = aspect(&r);
        if a + 1e-9 < best.0 {
            best = (a, r);
        } else {
            break;
        }
    }
    Ok(best.1)
}

fn layout_compound_with(compiled: &CompiledGraph, config: &LayoutConfig, budget_scale: f64) -> Result<LayoutResult> {
    let frame = Frame { lr: config.direction == LayoutDirection::LeftToRight };
    let node_size = compiled.graph.node_indices().map(|n| (n, frame.swap(crate::layout_node_size(compiled, n, config)))).collect();
    let node_group = (0..compiled.groups.len()).flat_map(|g| compiled.group_nodes[g].iter().map(move |&n| (n, g))).collect();
    let mut c = Compound { compiled, config, frame, node_size, blocks: vec![None; compiled.groups.len()], node_group, top_pos: HashMap::new(), unit_row: HashMap::new() };
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
    let row_budget = (max_w * ROW_WIDTH_SLACK + gap_cross).max((area * ROW_AREA_FACTOR).sqrt()) * budget_scale;
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
            c.unit_row.insert(units[u], ri);
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
            // Edges with an end above this tier and one below drop through it: a lane
            // each in the gutter nearest the line between their ends.
            // So do edges that start or end in a group of this tier at a node the box
            // won't let out straight along the flow: they leave it sideways and run down
            // a gutter beside it.
            let mut through: Vec<f64> = Vec::new();
            for &(a, b) in &cross {
                let (ra, rb) = (row_of[unit_of[&a]], row_of[unit_of[&b]]);
                if ra.min(rb) < ri && ra.max(rb) > ri {
                    through.push((c.center(a).0 + c.center(b).0) / 2.0);
                    continue;
                }
                for (mine, theirs, onward) in [(a, b, rb > ra), (b, a, ra > rb)] {
                    let um = unit_of[&mine];
                    if row_of[um] != ri || row_of[unit_of[&theirs]] == ri {
                        continue;
                    }
                    if let Item::Group(g) = units[um] {
                        if !c.exits_straight(g, mine, onward) {
                            through.push(c.center(mine).0);
                        }
                    }
                }
            }
            if std::env::var_os("RDG_DEBUG_GUTTERS").is_some() {
                let names: Vec<String> = row.iter().map(|&u| match units[u] { Item::Node(n) => compiled.graph[n].id.clone(), Item::Group(g) => format!("[{}]", compiled.groups[g].id) }).collect();
                eprintln!("row {ri} sweep {sweep}: {names:?}");
            }
            let mut spans = Vec::with_capacity(row.len());
            let mut x = 0.0;
            for (i, &w) in widths.iter().enumerate() {
                spans.push((x, x + w));
                x += w + gaps.get(i).copied().unwrap_or(0.0);
            }
            // Spans in the row's current frame, so the through points compare to them.
            let shift = c.top_pos[&units[row[0]]].0;
            let spans: Vec<(f64, f64)> = spans.iter().map(|&(a, b)| (a + shift, b + shift)).collect();
            let row_bordered: Vec<bool> = row.iter().map(|&u| bordered(u)).collect();
            let gaps = gutters_for_through(&spans, &row_bordered, &through, &gaps, &config.tokens);
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
        // Inner groups first: a tightened child shrinks its parent's content.
        for &g in outer_first.iter().rev() {
            if c.blocks[g].is_some() {
                c.tighten_group(g);
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
        let has_border = rows[ri - 1].iter().chain(&rows[ri]).any(|&u| bordered(u));
        // Edges crossing a tier gap drop straight through it unless their ends are
        // offset; those turn in the first gap they cross, and the deepest pile of
        // overlapping turns sets how many tracks the gap must hold.
        let straight = config.tokens.min_port_pitch();
        let turns: Vec<(f64, f64)> = cross
            .iter()
            .filter(|(a, b)| spans(unit_of[a], unit_of[b]) && row_of[unit_of[a]].min(row_of[unit_of[b]]) == ri - 1)
            .map(|(a, b)| (c.center(*a).0, c.center(*b).0))
            .filter(|(xa, xb)| (xa - xb).abs() > straight)
            .map(|(xa, xb)| (xa.min(xb), xa.max(xb)))
            .collect();
        let mut gap = crate::gaps::gap_for(crate::gaps::max_overlap(&turns), config).max(if has_border { gap_main } else { 0.0 });
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

    // --- Folding: a long strip of narrow tiers reads as columns on a page ----------
    // A chain of steps (a flowchart, a state machine, a stack of small groups) is
    // many times taller than wide; like a long procedure in print it continues in
    // the next column, the arrow from the bottom of one column to the top of the
    // next looping round. Only strips fold: anything already near square is left.
    let main_len = row_y.last().copied().unwrap_or(0.0) + row_h.last().copied().unwrap_or(0.0);
    let row_span = |c: &Compound, ri: usize| -> (f64, f64) {
        rows[ri].iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &u| {
            let x = c.top_pos[&units[u]].0;
            (lo.min(x), hi.max(x + usize_of(u).0))
        })
    };
    let cross_w = (0..rows.len()).map(|ri| row_span(&c, ri)).fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), (l, h)| (a.min(l), b.max(h)));
    let cross_w = cross_w.1 - cross_w.0;
    if !frame.lr && rows.len() >= FOLD_MIN_TIERS && main_len > FOLD_ASPECT * cross_w {
        let cols = ((main_len / cross_w).sqrt().round() as usize).clamp(2, 3);
        let per = rows.len().div_ceil(cols);
        let fold_gap = gap_cross * 2.0 + 4.0 * config.tokens.channel_pitch();
        let mut x_start = 0.0;
        for col in 0..cols {
            let span = col * per..((col + 1) * per).min(rows.len());
            if span.is_empty() {
                break;
            }
            let (lo, hi) = span.clone().map(|ri| row_span(&c, ri)).fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), (l, h)| (a.min(l), b.max(h)));
            let y0 = row_y[span.start];
            // Left to right the bands snake back and forth, each starting under the
            // end of the one before (short hops between bands); top to bottom the
            // columns read like text.
            let mirror = frame.lr && col % 2 == 1;
            for ri in span.clone() {
                for &u in &rows[ri] {
                    let main = usize_of(u).1;
                    let p = c.top_pos.get_mut(&units[u]).expect("unit placed");
                    p.0 += x_start - lo;
                    p.1 = if mirror { main_len - (p.1 - y0) - main } else { p.1 - y0 };
                }
            }
            x_start += (hi - lo) + fold_gap;
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
    Ok(LayoutResult { positions, group_widths, group_title_shift, layered: Some(config.direction), ..Default::default() })
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
