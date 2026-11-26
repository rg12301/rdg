//! Aligns the turns of sibling arrows: two arrows leaving one face of a node (or
//! entering one) that stand off the face along its normal and then turn *opposite*
//! ways turn on one line — the way a person draws a fork, one bar across with the
//! branches off it — instead of each turning wherever its own search happened to put
//! the bend. Arrows turning the *same* way keep their own lanes (they would otherwise
//! run on top of each other); the spread pass keeps those a lane apart.
//!
//! Runs on the finished routes, after spreading. Only siblings whose turns lie within
//! [`SAME_CHANNEL`] lanes of each other count (further apart they turn in different
//! channels for a reason); a turn moves only outward (away from the face), never
//! closer; never past the next bend; never onto a node, never within a lane of another
//! arrow's parallel leg, and never into more crossings than it had.

/// Sibling turns this many lanes apart or less are meant to be one line.
const SAME_CHANNEL: f64 = 2.0;

use std::collections::HashMap;

use petgraph::stable_graph::{EdgeIndex, NodeIndex};
use rdg_graph::CompiledGraph;
use rdg_layout::{DesignTokens, LayoutResult};

use crate::routing::{EdgeRoutingPlan, Side, edge_full_path, node_obstacles, resolve_edge_layout, simplify_orthogonal_polyline};

type Pt = (f64, f64);

/// One end of a route that leaves (or arrives at) a face along its normal and turns.
#[derive(Debug, Clone, Copy)]
struct Turn {
    path: usize,
    /// The face's node and side.
    node: NodeIndex,
    side: Side,
    /// Index of the bend in the path, and whether the path is read from its end.
    bend: usize,
    from_end: bool,
    /// How far from the face the turn is, and which way (sign) the second leg runs
    /// along the face.
    depth: f64,
    dir: f64,
}

fn turn_of(path: &[Pt], side: Side, from_end: bool, pi: usize, node: NodeIndex) -> Option<Turn> {
    let n = path.len();
    if n < 3 {
        return None;
    }
    let (p0, p1, p2) = if from_end { (path[n - 1], path[n - 2], path[n - 3]) } else { (path[0], path[1], path[2]) };
    let (normal, depth, along, dir) = match side {
        Side::Top | Side::Bottom => ((p0.0 - p1.0).abs() < 0.5, (p1.1 - p0.1).abs(), (p1.1 - p2.1).abs() < 0.5, (p2.0 - p1.0).signum()),
        Side::Left | Side::Right => ((p0.1 - p1.1).abs() < 0.5, (p1.0 - p0.0).abs(), (p1.0 - p2.0).abs() < 0.5, (p2.1 - p1.1).signum()),
    };
    if !normal || !along || depth < 1.0 || dir == 0.0 {
        return None;
    }
    Some(Turn { path: pi, node, side, bend: if from_end { n - 2 } else { 1 }, from_end, depth, dir })
}

/// Pairs of sibling turns on one face turning opposite ways: how many there are and
/// how many turn at different depths.
pub fn sibling_turns(compiled: &CompiledGraph, layout: &LayoutResult, plans: &HashMap<EdgeIndex, EdgeRoutingPlan>) -> (usize, usize) {
    let (paths, turns) = collect(compiled, layout, plans);
    let _ = paths;
    let mut by_face: HashMap<(NodeIndex, u8), Vec<Turn>> = HashMap::new();
    for t in turns {
        by_face.entry((t.node, t.side as u8)).or_default().push(t);
    }
    let lane = DesignTokens::default().channel_pitch().max(10.0);
    let (mut pairs, mut unaligned) = (0, 0);
    for group in by_face.values() {
        for (i, a) in group.iter().enumerate() {
            for b in &group[i + 1..] {
                if a.dir != b.dir && (a.depth - b.depth).abs() <= SAME_CHANNEL * lane {
                    pairs += 1;
                    if (a.depth - b.depth).abs() > 0.5 {
                        unaligned += 1;
                    }
                }
            }
        }
    }
    (pairs, unaligned)
}

fn collect(compiled: &CompiledGraph, layout: &LayoutResult, plans: &HashMap<EdgeIndex, EdgeRoutingPlan>) -> (Vec<(EdgeIndex, Vec<Pt>)>, Vec<Turn>) {
    let mut ids: Vec<EdgeIndex> = plans.keys().copied().collect();
    ids.sort_by_key(|e| e.index());
    let mut paths = Vec::new();
    let mut turns = Vec::new();
    for e in ids {
        let plan = &plans[&e];
        if plan.start.is_some() || plan.junction.is_some() {
            continue;
        }
        let (Some(path), Some((s, d, _, _))) = (edge_full_path(compiled, layout, e, plan), resolve_edge_layout(compiled, layout, e)) else { continue };
        let pi = paths.len();
        if let Some(t) = turn_of(&path, plan.src_side, false, pi, s) {
            turns.push(t);
        }
        if let Some(t) = turn_of(&path, plan.dst_side, true, pi, d) {
            turns.push(t);
        }
        paths.push((e, path));
    }
    (paths, turns)
}

/// Moves the nearer turn of each opposite-way sibling pair out to the farther one's
/// line where that is clear (see the module docs). Returns how many turns moved.
pub fn align_sibling_turns(compiled: &CompiledGraph, layout: &LayoutResult, plans: &mut HashMap<EdgeIndex, EdgeRoutingPlan>, tokens: &DesignTokens) -> usize {
    let (mut paths, turns) = collect(compiled, layout, plans);
    let lane = tokens.channel_pitch().max(10.0);
    let clearance = tokens.stub_clearance();
    let obstacles = node_obstacles(layout);
    let debug = std::env::var_os("RDG_DEBUG_SPREAD").is_some();
    let mut by_face: HashMap<(NodeIndex, u8), Vec<Turn>> = HashMap::new();
    for t in &turns {
        by_face.entry((t.node, t.side as u8)).or_default().push(*t);
    }
    let mut faces: Vec<_> = by_face.into_iter().collect();
    faces.sort_by_key(|((n, s), _)| (n.index(), *s));
    let mut moved = 0;
    for (_, group) in faces {
        // The farthest opposite-way partner sets the line; each nearer sibling with a
        // partner across the face moves out to it.
        for t in &group {
            let target = group
                .iter()
                .filter(|o| o.dir != t.dir && o.depth > t.depth + 0.5 && o.depth - t.depth <= SAME_CHANNEL * lane)
                .map(|o| o.depth)
                .fold(f64::NAN, f64::max);
            if target.is_nan() {
                continue;
            }
            let path = &paths[t.path].1;
            let n = path.len();
            let (b, next) = if t.from_end { (t.bend, t.bend - 1) } else { (t.bend, t.bend + 1) };
            let (face_pt, bend, after) = if t.from_end { (path[n - 1], path[b], path[next]) } else { (path[0], path[b], path[next]) };
            let beyond = if t.from_end { next.checked_sub(1).map(|k| path[k]) } else { path.get(next + 1).copied() };
            // The new turn line, outward from the face.
            let vertical = matches!(t.side, Side::Top | Side::Bottom);
            let out = if vertical { (bend.1 - face_pt.1).signum() } else { (bend.0 - face_pt.0).signum() };
            let line = if vertical { face_pt.1 + out * target } else { face_pt.0 + out * target };
            // Never past the leg after the second one (it continues along the normal):
            // keep a clearance before where that leg ends.
            if let Some(q) = beyond {
                let end = if vertical { q.1 } else { q.0 };
                if (end - line) * out < clearance {
                    continue;
                }
            }
            let new_bend = if vertical { (bend.0, line) } else { (line, bend.1) };
            let new_after = if vertical { (after.0, line) } else { (line, after.1) };
            // The moved legs clip no node and run within a lane of no other arrow's leg.
            let m = tokens.px(0.5);
            if obstacles.iter().any(|(_, o)| o.clips_segment(bend.0, bend.1, new_bend.0, new_bend.1, m) || o.clips_segment(new_bend.0, new_bend.1, new_after.0, new_after.1, m)) {
                continue;
            }
            let (lo, hi) = if vertical { (new_bend.0.min(new_after.0), new_bend.0.max(new_after.0)) } else { (new_bend.1.min(new_after.1), new_bend.1.max(new_after.1)) };
            let crowded = paths.iter().enumerate().filter(|(k, _)| *k != t.path).any(|(_, (_, other))| {
                other.windows(2).any(|w| {
                    let parallel = if vertical { (w[0].1 - w[1].1).abs() < 0.5 && (w[0].1 - line).abs() < lane } else { (w[0].0 - w[1].0).abs() < 0.5 && (w[0].0 - line).abs() < lane };
                    let (a, c) = if vertical { (w[0].0.min(w[1].0), w[0].0.max(w[1].0)) } else { (w[0].1.min(w[1].1), w[0].1.max(w[1].1)) };
                    parallel && c > lo && a < hi
                })
            });
            if crowded {
                continue;
            }
            // No more crossings than the legs had where they were.
            let crossings = |legs: &[Pt]| -> usize {
                paths.iter().enumerate().filter(|(k, _)| *k != t.path).map(|(_, (_, other))| crate::polish::crossing_points(legs, other).len()).sum()
            };
            let before = crossings(&[face_pt, bend, after]);
            let now = crossings(&[face_pt, new_bend, new_after]);
            if now > before {
                continue;
            }
            let path = &mut paths[t.path].1;
            path[b] = new_bend;
            path[next] = new_after;
            moved += 1;
            if debug {
                let (s, d) = compiled.graph.edge_endpoints(paths[t.path].0).expect("edge");
                eprintln!("siblings: {} -> {} turn at {:.0} -> {line:.0} on {:?}'s {:?} face", compiled.graph[s].id, compiled.graph[d].id, if vertical { bend.1 } else { bend.0 }, compiled.graph[t.node].id, t.side);
            }
        }
    }
    if moved > 0 {
        for (e, mut p) in paths {
            simplify_orthogonal_polyline(&mut p);
            let plan = plans.get_mut(&e).expect("path came from a plan");
            plan.waypoints = p[1..p.len() - 1].to_vec();
        }
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_is_read_off_a_route_leaving_a_face_along_its_normal() {
        let n = NodeIndex::new(0);
        // Leaves the bottom face, drops 30, turns right.
        let t = turn_of(&[(100.0, 50.0), (100.0, 80.0), (160.0, 80.0), (160.0, 120.0)], Side::Bottom, false, 0, n).unwrap();
        assert_eq!((t.depth, t.dir, t.bend), (30.0, 1.0, 1));
        // Arrives at a top face from the left: read from the end.
        let t = turn_of(&[(20.0, 10.0), (20.0, 40.0), (100.0, 40.0), (100.0, 70.0)], Side::Top, true, 0, n).unwrap();
        assert_eq!((t.depth, t.dir, t.from_end), (30.0, -1.0, true));
        // A route that runs along the face has no normal stub.
        assert!(turn_of(&[(100.0, 50.0), (160.0, 50.0), (160.0, 120.0)], Side::Bottom, false, 0, n).is_none());
    }
}
