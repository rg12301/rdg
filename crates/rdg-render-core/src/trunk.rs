//! Bundles the arrows of a *collector*: three or more edges of one meaning (same label
//! and look) from members of one group to one node outside it — every service of a
//! domain writing one store, every tier tracing to one collector. Drawn one by one
//! they are a ribbon of parallel lines a reader cannot tell apart; drawn as a bundle
//! they are one trunk with the arrowhead and the label, and the other arrows join it
//! just outside the group's border, each at its own junction dot on a short ladder.
//!
//! Runs on the finished routes. The trunk is the member with the shortest route among
//! those leaving the group by the majority face; a member joins when its own route
//! leaves by that face too, its path is cut at the border, dropped a lane or more past
//! the border and run across to the trunk's line — as long as that run clips no node.
//! Members nearer the trunk join higher, so no ladder rung crosses another's drop.
//! Numbered edges are never bundled: each step needs its own arrow.
//!
//! The mirror image, a *fan-out* (one gateway calling every service of a domain with
//! the same protocol), bundles the same way with the roles swapped: one trunk runs
//! from the source to the group's border, and the other arrows branch off it just
//! outside the border, each starting at its own junction dot.

use std::collections::HashMap;

use petgraph::stable_graph::{EdgeIndex, NodeIndex};
use rdg_graph::CompiledGraph;
use rdg_layout::{DesignTokens, LayoutResult};

use crate::routing::{EdgeRoutingPlan, Side, edge_full_path, node_obstacles, resolve_edge_layout};

type Pt = (f64, f64);

/// Fewest same-meaning arrows from one group to one node that make a bundle.
const MIN_BUNDLE: usize = 3;

/// Where segment `a→b` of an axis-aligned path leaves rect `r` (first point strictly
/// outside), as the index of the first outside point and the crossing point.
fn exit_of(path: &[Pt], r: (f64, f64, f64, f64)) -> Option<(usize, Pt, Side)> {
    let (x0, y0, x1, y1) = r;
    let inside = |p: Pt| p.0 > x0 && p.0 < x1 && p.1 > y0 && p.1 < y1;
    if !inside(path[0]) {
        return None;
    }
    for (k, w) in path.windows(2).enumerate() {
        let (a, b) = (w[0], w[1]);
        if inside(b) {
            continue;
        }
        // Crossing of a→b with the rect border (axis-aligned legs).
        let (cross, side) = if (a.0 - b.0).abs() < 0.5 {
            if b.1 >= y1 { ((a.0, y1), Side::Bottom) } else { ((a.0, y0), Side::Top) }
        } else if b.0 >= x1 {
            ((x1, a.1), Side::Right)
        } else {
            ((x0, a.1), Side::Left)
        };
        return Some((k + 1, cross, side));
    }
    None
}

/// Which way a bundle runs: many members of a group into one node, or one node into
/// many members of a group.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Fan {
    In,
    Out,
}

/// Joins each collector's member arrows to its trunk, fan-ins and fan-outs alike (see
/// the module docs).
pub fn bundle_collectors(compiled: &CompiledGraph, layout: &LayoutResult, plans: &mut HashMap<EdgeIndex, EdgeRoutingPlan>, tokens: &DesignTokens) {
    if layout.sequence_info.is_some() {
        return;
    }
    bundle(compiled, layout, plans, tokens, Fan::In);
    bundle(compiled, layout, plans, tokens, Fan::Out);
}

fn bundle(compiled: &CompiledGraph, layout: &LayoutResult, plans: &mut HashMap<EdgeIndex, EdgeRoutingPlan>, tokens: &DesignTokens, fan: Fan) {
    let group_rects = rdg_layout::groups::group_rects(compiled, layout, tokens);
    // Every group a node is in, innermost first.
    let mut chains: HashMap<NodeIndex, Vec<usize>> = HashMap::new();
    for (g, members) in compiled.group_nodes.iter().enumerate() {
        let chain: Vec<usize> = std::iter::successors(Some(g), |&p| compiled.group_tree.parent[p]).collect();
        for &n in members {
            chains.insert(n, chain.clone());
        }
    }
    // The outermost group of `s` that does not hold `t`: the border the bundle leaves by.
    let boundary = |s: NodeIndex, t: NodeIndex| -> Option<usize> {
        let in_t = chains.get(&t).cloned().unwrap_or_default();
        chains.get(&s)?.iter().rev().find(|g| !in_t.contains(g)).copied()
    };
    // Collector sets: (the shared node, boundary group of the members, meaning) ->
    // member edges. Fan-in: members are sources; fan-out: members are targets.
    let mut sets: HashMap<(NodeIndex, usize, String), Vec<EdgeIndex>> = HashMap::new();
    let mut edge_ids: Vec<EdgeIndex> = plans.keys().copied().collect();
    edge_ids.sort_by_key(|e| e.index());
    for &e in &edge_ids {
        let Some((s, t, _, _)) = resolve_edge_layout(compiled, layout, e) else { continue };
        let d = &compiled.graph[e];
        if s == t || d.step.is_some() || d.source_port.is_some() || d.target_port.is_some() || plans[&e].junction.is_some() || plans[&e].start.is_some() {
            continue;
        }
        let (member, shared) = match fan {
            Fan::In => (s, t),
            Fan::Out => (t, s),
        };
        let Some(g) = boundary(member, shared) else { continue };
        let t = shared;
        let meaning = format!(
            "{}|{}|{}|{}|{}",
            d.label.as_deref().map(str::trim).unwrap_or(""),
            d.edge_style.as_deref().unwrap_or(""),
            d.line_style.as_deref().unwrap_or(""),
            d.color.as_deref().unwrap_or(""),
            d.head.as_deref().unwrap_or("")
        );
        sets.entry((t, g, meaning)).or_default().push(e);
    }
    let mut keys: Vec<&(NodeIndex, usize, String)> = sets.keys().collect();
    keys.sort_by(|a, b| a.0.index().cmp(&b.0.index()).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let obstacles = node_obstacles(layout);
    let clearance = tokens.stub_clearance();
    let lane = tokens.channel_pitch().max(10.0);
    for key in keys {
        let members = &sets[key];
        if members.len() < MIN_BUNDLE {
            continue;
        }
        let Some((gx, gy, gw, gh)) = group_rects[key.1] else { continue };
        let rect = (gx, gy, gx + gw, gy + gh);
        // Each member's route and where it leaves the group — for a fan-out, where it
        // enters, which is where the reversed route leaves; everything below works on
        // the route in that direction and is turned back at the end.
        let mut exits: Vec<(EdgeIndex, Vec<Pt>, usize, Pt, Side)> = Vec::new();
        for &e in members {
            let Some(mut path) = edge_full_path(compiled, layout, e, &plans[&e]) else { continue };
            if fan == Fan::Out {
                path.reverse();
            }
            if let Some((k, cross, side)) = exit_of(&path, rect) {
                exits.push((e, path, k, cross, side));
            }
        }
        if exits.len() < MIN_BUNDLE {
            continue;
        }
        // The majority face, then the trunk: the shortest route leaving by it.
        let mut by_side: HashMap<u8, usize> = HashMap::new();
        for ex in &exits {
            *by_side.entry(ex.4 as u8).or_default() += 1;
        }
        let face = *by_side.iter().max_by_key(|(s, n)| (**n, std::cmp::Reverse(**s))).map(|(s, _)| s).expect("at least one exit");
        let same: Vec<&(EdgeIndex, Vec<Pt>, usize, Pt, Side)> = exits.iter().filter(|ex| ex.4 as u8 == face).collect();
        if same.len() < MIN_BUNDLE {
            continue;
        }
        let len = |p: &[Pt]| p.windows(2).map(|w| (w[1].0 - w[0].0).abs() + (w[1].1 - w[0].1).abs()).sum::<f64>();
        let trunk = same.iter().min_by(|a, b| len(&a.1).total_cmp(&len(&b.1)).then(a.0.index().cmp(&b.0.index()))).expect("members");
        let (trunk_e, trunk_path, trunk_k, trunk_cross, side) = (trunk.0, &trunk.1, trunk.2, trunk.3, trunk.4);
        // The trunk's first leg outside must run straight away from the border, long
        // enough for the ladder; its line is where the members join.
        let out_leg_end = trunk_path[trunk_k];
        let outward = match side {
            Side::Bottom => (out_leg_end.1 - trunk_cross.1, out_leg_end.0 == trunk_cross.0),
            Side::Top => (trunk_cross.1 - out_leg_end.1, out_leg_end.0 == trunk_cross.0),
            Side::Right => (out_leg_end.0 - trunk_cross.0, out_leg_end.1 == trunk_cross.1),
            Side::Left => (trunk_cross.0 - out_leg_end.0, out_leg_end.1 == trunk_cross.1),
        };
        if !outward.1 || outward.0 < clearance + lane {
            continue;
        }
        // Along the border: the coordinate that varies between members; across it: the
        // outward one.
        let vertical = matches!(side, Side::Top | Side::Bottom);
        let along = |p: Pt| if vertical { p.0 } else { p.1 };
        let trunk_along = along(trunk_cross);
        // Members nearer the trunk join first (closest to the border).
        let mut joiners: Vec<&(EdgeIndex, Vec<Pt>, usize, Pt, Side)> = same.iter().copied().filter(|ex| ex.0 != trunk_e).collect();
        joiners.sort_by(|a, b| (along(a.3) - trunk_along).abs().total_cmp(&(along(b.3) - trunk_along).abs()).then(a.0.index().cmp(&b.0.index())));
        let mut rung = 0usize;
        for ex in joiners {
            let (e, path, k, cross, _) = (ex.0, &ex.1, ex.2, ex.3, ex.4);
            // The member must leave straight across the border too.
            let prev = path[k - 1];
            if (vertical && (prev.0 - cross.0).abs() > 0.5) || (!vertical && (prev.1 - cross.1).abs() > 0.5) {
                continue;
            }
            let depth = clearance + (rung as f64) * lane;
            if depth + lane > outward.0 {
                break;
            }
            let (drop, junction) = match side {
                Side::Bottom => ((cross.0, cross.1 + depth), (trunk_cross.0, cross.1 + depth)),
                Side::Top => ((cross.0, cross.1 - depth), (trunk_cross.0, cross.1 - depth)),
                Side::Right => ((cross.0 + depth, cross.1), (cross.0 + depth, trunk_cross.1)),
                Side::Left => ((cross.0 - depth, cross.1), (cross.0 - depth, trunk_cross.1)),
            };
            // The rung must clip no node.
            let m = tokens.px(0.5);
            if obstacles.iter().any(|(_, o)| o.clips_segment(drop.0, drop.1, junction.0, junction.1, m) || o.clips_segment(cross.0, cross.1, drop.0, drop.1, m)) {
                continue;
            }
            let plan = plans.get_mut(&e).expect("member has a plan");
            let mut waypoints: Vec<Pt> = path[1..k].to_vec();
            waypoints.push(drop);
            match fan {
                Fan::In => {
                    plan.waypoints = waypoints;
                    plan.junction = Some(junction);
                }
                Fan::Out => {
                    // Back in the arrow's own direction: from the junction, along the
                    // rung, through the border, on to the member.
                    waypoints.reverse();
                    plan.waypoints = waypoints;
                    plan.start = Some(junction);
                }
            }
            rung += 1;
            if std::env::var_os("RDG_DEBUG_BUNDLE").is_some() {
                let name = |n: NodeIndex| compiled.graph[n].id.clone();
                let (s, t) = compiled.graph.edge_endpoints(e).expect("edge");
                let (ts, _) = compiled.graph.edge_endpoints(trunk_e).expect("edge");
                eprintln!("bundle {} -> {}: joins {}'s trunk at ({:.0},{:.0}) rung {}", name(s), name(t), name(ts), junction.0, junction.1, rung);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_of_finds_the_border_crossing_of_a_vertical_leg() {
        let r = (0.0, 0.0, 100.0, 100.0);
        let path = vec![(50.0, 50.0), (50.0, 150.0), (80.0, 150.0)];
        let (k, cross, side) = exit_of(&path, r).unwrap();
        assert_eq!((k, cross, side), (1, (50.0, 100.0), Side::Bottom));
        assert!(exit_of(&[(150.0, 50.0), (150.0, 160.0)], r).is_none());
    }
}
