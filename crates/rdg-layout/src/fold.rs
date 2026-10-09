//! Folds a long strip of tiers into columns, like a long procedure continued in the
//! next column of a page: a chain of fifteen steps is otherwise fifteen times taller
//! than wide, and a reader (or a page) sees a square far more easily than a strip.

use std::collections::HashMap;

use petgraph::stable_graph::NodeIndex;

use crate::{LayoutConfig, LayoutDirection, NodeLayout};

/// A strip folds only from this many tiers, and only when it is this many times
/// longer along the flow than across it.
pub(crate) const FOLD_MIN_TIERS: usize = 4;
pub(crate) const FOLD_ASPECT: f64 = 2.2;

/// Folds `tiers` (each a rank's nodes, in flow order) into two or three columns when
/// the layout is a strip. Columns stand a gap apart that leaves room for the arrow
/// looping from the foot of one to the head of the next.
pub(crate) fn fold_strip(
    positions: &mut HashMap<NodeIndex, NodeLayout>,
    tiers: &[Vec<NodeIndex>],
    compiled: &rdg_graph::CompiledGraph,
    config: &LayoutConfig,
) -> HashMap<petgraph::stable_graph::EdgeIndex, String> {
    use petgraph::visit::{EdgeRef, IntoEdgeReferences};
    let mut hints = HashMap::new();
    let lr = config.direction == LayoutDirection::LeftToRight;
    // Only top-to-bottom strips fold: columns of a downward flow read like text,
    // while a left-to-right strip folded into bands either drags a ribbon of arrows
    // back across the page or loops its hops round the outside.
    if tiers.len() < FOLD_MIN_TIERS || lr {
        return hints;
    }
    // (main, cross) extents of a tier: along the flow and across it.
    let extent = |nodes: &[NodeIndex], positions: &HashMap<NodeIndex, NodeLayout>| {
        nodes.iter().filter_map(|n| positions.get(n)).fold((f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY), |(m0, m1, c0, c1), nl| {
            let (ma, mb, ca, cb) = if lr { (nl.x, nl.x + nl.width, nl.y, nl.y + nl.height) } else { (nl.y, nl.y + nl.height, nl.x, nl.x + nl.width) };
            (m0.min(ma), m1.max(mb), c0.min(ca), c1.max(cb))
        })
    };
    let all: Vec<NodeIndex> = tiers.iter().flatten().copied().collect();
    let (m0, m1, c0, c1) = extent(&all, positions);
    if !(m0.is_finite() && c0.is_finite()) || (m1 - m0) <= FOLD_ASPECT * (c1 - c0) {
        return hints;
    }
    let cols = (((m1 - m0) / (c1 - c0)).sqrt().round() as usize).clamp(2, 3);
    let per = tiers.len().div_ceil(cols);
    // The arrows that continue the strip in the next column enter its first node
    // from above.
    let tier_of: HashMap<NodeIndex, usize> = tiers.iter().enumerate().flat_map(|(t, ns)| ns.iter().map(move |&n| (n, t))).collect();
    for e in compiled.graph.edge_references() {
        if let (Some(&ta), Some(&tb)) = (tier_of.get(&e.source()), tier_of.get(&e.target())) {
            if tb > ta && tb % per == 0 && tb / per == ta / per + 1 {
                hints.insert(e.id(), "top".to_string());
            }
        }
    }
    let gap = (config.node_spacing as f64) * 2.0 + 4.0 * config.tokens.channel_pitch();
    let mut cross_start = c0;
    for col in 0..cols {
        let span = col * per..((col + 1) * per).min(tiers.len());
        if span.is_empty() {
            break;
        }
        let nodes: Vec<NodeIndex> = tiers[span].iter().flatten().copied().collect();
        let (cm0, _, cc0, cc1) = extent(&nodes, positions);
        // Left to right the bands snake: every other band runs back the other way,
        // starting under the end of the band above, so the arrows from one band to
        // the next are short drops instead of a ribbon across the whole width. Top
        // to bottom the columns read like text.
        let mirror = lr && col % 2 == 1;
        for n in &nodes {
            if let Some(nl) = positions.get_mut(n) {
                if lr {
                    if mirror {
                        // Stacked from the right: the band's first tier ends where the
                        // band above ends, the next tier left of it, and so on.
                        nl.x = m1 - (nl.x - cm0) - nl.width;
                    } else {
                        nl.x -= cm0 - m0;
                    }
                    nl.y += cross_start - cc0;
                } else {
                    nl.y -= cm0 - m0;
                    nl.x += cross_start - cc0;
                }
            }
        }
        cross_start += (cc1 - cc0) + gap;
    }
    hints
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_graph() -> rdg_graph::CompiledGraph {
        rdg_graph::build_graph(&rdg_schema::DiagramPayload::default()).expect("an empty diagram compiles")
    }

    #[test]
    fn a_long_chain_folds_into_columns_and_a_square_does_not() {
        let config = LayoutConfig::default();
        let mut positions = HashMap::new();
        let tiers: Vec<Vec<NodeIndex>> = (0..12)
            .map(|i| {
                let n = NodeIndex::new(i);
                positions.insert(n, NodeLayout { x: 0.0, y: i as f64 * 100.0, width: 80.0, height: 40.0 });
                vec![n]
            })
            .collect();
        fold_strip(&mut positions, &tiers, &empty_graph(), &config);
        let max_y = positions.values().map(|p| p.y + p.height).fold(0.0, f64::max);
        let max_x = positions.values().map(|p| p.x + p.width).fold(0.0, f64::max);
        assert!(max_y < 700.0, "folded: {max_y}");
        assert!(max_x > 200.0, "spread into columns: {max_x}");
        // Every column starts at the top again.
        assert!(positions.values().filter(|p| p.y == 0.0).count() >= 2);

        let mut square = HashMap::new();
        let tiers: Vec<Vec<NodeIndex>> = (0..6).map(|i| { let n = NodeIndex::new(i); square.insert(n, NodeLayout { x: 0.0, y: i as f64 * 60.0, width: 300.0, height: 40.0 }); vec![n] }).collect();
        let before = square.clone();
        fold_strip(&mut square, &tiers, &empty_graph(), &config);
        assert_eq!(square[&NodeIndex::new(5)].y, before[&NodeIndex::new(5)].y);
    }
}
