//! Connectivity-aware gaps between consecutive ranks.
//!
//! A flat `rank_spacing` treats a rank pair joined by one edge the same as one joined
//! by a dozen, and lets the gap shrink below what the router needs for the edge's own
//! stubs — leaving arrows that are barely longer than their arrowhead, and parallel
//! channels crushed onto one line. [`rank_gaps`] turns `rank_spacing` into a *request*
//! and floors it, per gap, at what the edges crossing it actually need.

use std::collections::HashMap;

use petgraph::stable_graph::NodeIndex;
use petgraph::visit::{EdgeRef, IntoEdgeReferences};

use rdg_graph::CompiledGraph;

use crate::LayoutConfig;

/// Parallel channels beyond this many stop widening the gap: a gap that scales
/// linearly with a huge fan-out would blow the canvas up for little readability gain.
pub(crate) const MAX_CHANNELS: usize = 12;

/// Required gap after each rank (`result[r]` separates rank `r` from `r + 1`), given
/// each node's rank in `layer`. Edges with an endpoint missing from `layer` (e.g. the
/// other side of a cross-group edge, when laying out one group) are ignored.
pub(crate) fn rank_gaps(
    compiled: &CompiledGraph,
    layer: &HashMap<NodeIndex, usize>,
    config: &LayoutConfig,
) -> Vec<f64> {
    let n_ranks = layer.values().copied().max().map_or(0, |m| m + 1);
    let spans = compiled
        .graph
        .edge_references()
        .filter_map(|e| Some((*layer.get(&e.source())?, *layer.get(&e.target())?)));
    gaps_for_spans(n_ranks, spans, config)
}

/// Required gap after each of `n_ranks` ranks, given the `(rank, rank)` span of every
/// edge between them.
pub(crate) fn gaps_for_spans(n_ranks: usize, spans: impl IntoIterator<Item = (usize, usize)>, config: &LayoutConfig) -> Vec<f64> {
    let mut crossing = vec![0usize; n_ranks.saturating_sub(1)];
    for (a, b) in spans {
        let (lo, hi) = (a.min(b), a.max(b));
        for c in &mut crossing[lo..hi] {
            *c += 1;
        }
    }
    crossing.into_iter().map(|m| gap_for(m, config)).collect()
}

/// Required gap after each of `n_ranks` ranks, given every edge's `(rank, rank)` span
/// and the `(from, to)` positions of its ends across the flow: an edge whose ends line
/// up drops straight through a gap and needs no track there; the others turn in it,
/// and those whose turns overlap across the flow need a track each — so a gap holds
/// as many tracks as the deepest pile of overlapping turns, not one per edge.
pub(crate) fn gaps_for_tracks(n_ranks: usize, edges: impl IntoIterator<Item = ((usize, usize), (f64, f64))>, config: &LayoutConfig) -> Vec<f64> {
    let straight = config.tokens.min_port_pitch();
    let mut turns: Vec<Vec<(f64, f64)>> = vec![Vec::new(); n_ranks.saturating_sub(1)];
    for ((a, b), (xa, xb)) in edges {
        let (lo, hi) = (a.min(b), a.max(b));
        if lo == hi || (xa - xb).abs() <= straight {
            continue;
        }
        // The turn happens once, in the first gap it crosses (the routers turn as early
        // as they can); the other gaps it drops through.
        if let Some(t) = turns.get_mut(lo) {
            t.push((xa.min(xb), xa.max(xb)));
        }
    }
    turns.into_iter().map(|t| gap_for(max_overlap(&t), config)).collect()
}

/// The deepest pile of mutually overlapping intervals.
pub(crate) fn max_overlap(spans: &[(f64, f64)]) -> usize {
    let mut events: Vec<(f64, i32)> = spans.iter().flat_map(|&(a, b)| [(a, 1), (b, -1)]).collect();
    // Ends before starts at the same point: touching intervals don't stack.
    events.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
    let (mut depth, mut best) = (0i32, 0i32);
    for (_, d) in events {
        depth += d;
        best = best.max(depth);
    }
    best.max(0) as usize
}

/// Gap needed to carry `m` parallel edges: the configured spacing, the two-stub floor,
/// and room for `m` staggered channels (`m + 1` slots plus end margin, mirroring how the
/// router spreads a bucket — see `plan_all_edge_routes`).
pub(crate) fn gap_for(m: usize, config: &LayoutConfig) -> f64 {
    let t = &config.tokens;
    let channels = if m > 1 {
        t.channel_pitch() * (m.min(MAX_CHANNELS) as f64 + 1.0) + t.px(2.5)
    } else {
        0.0
    };
    (config.rank_spacing as f64).max(t.min_rank_gap()).max(channels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_count_overlapping_turns_not_edges() {
        let c = LayoutConfig::default();
        // A fan of four from one source: two turn left, two right — two tracks deep
        // on each side, so the gap holds two tracks, not four.
        let fan = [((0, 1), (100.0, 0.0)), ((0, 1), (100.0, 40.0)), ((0, 1), (100.0, 160.0)), ((0, 1), (100.0, 200.0))];
        assert_eq!(gaps_for_tracks(2, fan, &c), vec![gap_for(2, &c)]);
        // Straight drops need no track at all.
        assert_eq!(gaps_for_tracks(2, [((0, 1), (50.0, 50.0)); 6], &c), vec![gap_for(0, &c)]);
        assert_eq!(max_overlap(&[(0.0, 10.0), (10.0, 20.0)]), 1);
        assert_eq!(max_overlap(&[(0.0, 10.0), (5.0, 20.0), (6.0, 7.0)]), 3);
    }

    #[test]
    fn test_gap_floors_at_two_stubs() {
        let c = LayoutConfig { rank_spacing: 10, ..LayoutConfig::default() };
        assert_eq!(gap_for(1, &c), 2.0 * c.tokens.stub_clearance());
    }

    #[test]
    fn test_gap_grows_with_parallel_edges_and_caps() {
        let c = LayoutConfig::default();
        assert!(gap_for(6, &c) > gap_for(2, &c));
        assert_eq!(gap_for(40, &c), gap_for(MAX_CHANNELS, &c));
    }

    #[test]
    fn test_explicit_large_spacing_is_respected() {
        let c = LayoutConfig { rank_spacing: 300, ..LayoutConfig::default() };
        assert_eq!(gap_for(3, &c), 300.0);
    }
}
