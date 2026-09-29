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
const MAX_CHANNELS: usize = 12;

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
    fn test_gap_floors_at_two_stubs() {
        let mut c = LayoutConfig::default();
        c.rank_spacing = 10;
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
        let mut c = LayoutConfig::default();
        c.rank_spacing = 300;
        assert_eq!(gap_for(3, &c), 300.0);
    }
}
