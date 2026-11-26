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
    let mut done = bundle(compiled, layout, plans, tokens, Fan::In);
    done.extend(bundle(compiled, layout, plans, tokens, Fan::Out));
    // A set that found no ladder at its listed nodes ladders at the other end.
    for k in 0..compiled.sets.len() {
        if !done.contains(&k) {
            fan_at_shared(compiled, layout, plans, tokens, k);
        }
    }
}

/// Whether a rung from `a` to `b` would run within a lane of another arrow's parallel
/// leg, or within a border lane of a group border line (where it would read as part
/// of the box).
fn crowded_by(others: &[Vec<Pt>], borders: &[(bool, f64, f64, f64)], lane: f64, a: Pt, b: Pt) -> bool {
    let vertical = (a.0 - b.0).abs() < 0.5;
    let (at, lo, hi) = if vertical { (a.0, a.1.min(b.1), a.1.max(b.1)) } else { (a.1, a.0.min(b.0), a.0.max(b.0)) };
    let by_legs = others.iter().any(|p| {
        p.windows(2).any(|w| {
            let same_way = if vertical { (w[0].0 - w[1].0).abs() < 0.5 } else { (w[0].1 - w[1].1).abs() < 0.5 };
            let (wat, wlo, whi) = if vertical { (w[0].0, w[0].1.min(w[1].1), w[0].1.max(w[1].1)) } else { (w[0].1, w[0].0.min(w[1].0), w[0].0.max(w[1].0)) };
            same_way && (wat - at).abs() < lane && hi.min(whi) - lo.max(wlo) > crate::spread::MIN_PARALLEL
        })
    });
    by_legs || borders.iter().any(|&(v, bat, blo, bhi)| v == vertical && (bat - at).abs() < lane * 1.25 && hi.min(bhi) - lo.max(blo) > crate::spread::MIN_PARALLEL)
}

/// The rung line for a member: of the lines within the ladder's room and the member's
/// own leg, the first not crowded by other arrows or borders that crosses the fewest
/// other arrows with its drop and rung; `None` when no line is left. Returns the
/// rung index and its drop and junction points.
struct Ladder {
    clearance: f64,
    lane: f64,
    /// How far the trunk's leg runs past the face.
    room: f64,
    /// How far the member's own leg runs past the face.
    own_leg: f64,
    side: Side,
    cross: Pt,
    trunk_cross: Pt,
}

fn best_rung(from_rung: usize, l: &Ladder, others: &[Vec<Pt>], crowded: &dyn Fn(Pt, Pt) -> bool) -> Option<(usize, Pt, Pt)> {
    let (clearance, lane, side, cross, trunk_cross) = (l.clearance, l.lane, l.side, l.cross, l.trunk_cross);
    let mut best: Option<(usize, usize, Pt, Pt)> = None;
    for rung in from_rung..from_rung + 4 {
        let depth = clearance + (rung as f64) * lane;
        if depth + lane > l.room || depth > l.own_leg + 0.5 {
            break;
        }
        let (drop, junction) = match side {
            Side::Bottom => ((cross.0, cross.1 + depth), (trunk_cross.0, cross.1 + depth)),
            Side::Top => ((cross.0, cross.1 - depth), (trunk_cross.0, cross.1 - depth)),
            Side::Right => ((cross.0 + depth, cross.1), (cross.0 + depth, trunk_cross.1)),
            Side::Left => ((cross.0 - depth, cross.1), (cross.0 - depth, trunk_cross.1)),
        };
        if crowded(drop, junction) {
            continue;
        }
        let legs = [cross, drop, junction];
        let crossings: usize = others.iter().map(|p| crate::polish::crossing_points(&legs, p).len()).sum();
        if best.is_none_or(|(c, ..)| crossings < c) {
            best = Some((crossings, rung, drop, junction));
        }
        if crossings == 0 {
            break;
        }
    }
    best.map(|(_, r, d, j)| (r, d, j))
}

/// Group border lines as `(vertical?, at, lo, hi)`.
fn border_lines(group_rects: &[Option<(f64, f64, f64, f64)>]) -> Vec<(bool, f64, f64, f64)> {
    group_rects.iter().flatten().flat_map(|&(x, y, w, h)| [(true, x, y, y + h), (true, x + w, y, y + h), (false, y, x, x + w), (false, y + h, x, x + w)]).collect()
}

/// How far a route runs straight on past `cross` (its first point outside, `path[k]`)
/// along the face's normal: the room a ladder has on that route.
fn outward_of(path: &[Pt], k: usize, cross: Pt, side: Side) -> (f64, bool) {
    let end = path[k];
    match side {
        Side::Bottom => (end.1 - cross.1, end.0 == cross.0),
        Side::Top => (cross.1 - end.1, end.0 == cross.0),
        Side::Right => (end.0 - cross.0, end.1 == cross.1),
        Side::Left => (cross.0 - end.0, end.1 == cross.1),
    }
}

/// What a moved leg must keep clear of: node boxes, and other arrows and borders
/// (through `clear`), with the margin the check uses.
struct Room<'a> {
    obstacles: &'a [(NodeIndex, crate::routing::ObstacleRect)],
    clear: &'a dyn Fn(Pt, Pt) -> bool,
    margin: f64,
    /// One lane: the step between the lines tried when the first is taken.
    lane: f64,
}

/// Pushes a trunk's first bend past the face out to `needed`, so the ladder fits: the
/// bend and the leg after it move out together, when that leg stays clear of nodes
/// and other arrows and the leg beyond it still reaches past the new line. Returns
/// whether the path changed.
fn extend_trunk(path: &mut [Pt], k: usize, cross: Pt, side: Side, needed: f64, room: &Room<'_>) -> bool {
    let debug = std::env::var_os("RDG_DEBUG_BUNDLE").is_some();
    if k + 1 >= path.len() {
        return false;
    }
    let margin = room.margin;
    let vertical = matches!(side, Side::Top | Side::Bottom);
    let out = match side {
        Side::Bottom | Side::Right => 1.0,
        Side::Top | Side::Left => -1.0,
    };
    let (bend, next) = (path[k], path[k + 1]);
    // The leg after the bend runs along the face; the one beyond continues outward.
    let along_face = if vertical { (bend.1 - next.1).abs() < 0.5 } else { (bend.0 - next.0).abs() < 0.5 };
    if !along_face {
        return false;
    }
    // The needed line, or the next lanes out when it is taken.
    for step in 0..3 {
        let depth = needed + step as f64 * room.lane;
        let line = if vertical { cross.1 + out * depth } else { cross.0 + out * depth };
        if let Some(beyond) = path.get(k + 2) {
            let reach = if vertical { (beyond.1 - line) * out } else { (beyond.0 - line) * out };
            if reach < margin {
                if debug {
                    eprintln!("extend trunk: the leg beyond ends {reach:.0}px past the line {line:.0}");
                }
                return false;
            }
        }
        let (nb, nn) = if vertical { ((bend.0, line), (next.0, line)) } else { ((line, bend.1), (line, next.1)) };
        if let Some((n, _)) = room.obstacles.iter().find(|(_, o)| o.clips_segment(nb.0, nb.1, nn.0, nn.1, margin)) {
            if debug {
                eprintln!("extend trunk: the moved leg ({:.0},{:.0})-({:.0},{:.0}) clips node {}", nb.0, nb.1, nn.0, nn.1, n.index());
            }
            return false;
        }
        if !(room.clear)(nb, nn) {
            if debug {
                eprintln!("extend trunk: the moved leg ({:.0},{:.0})-({:.0},{:.0}) is crowded", nb.0, nb.1, nn.0, nn.1);
            }
            continue;
        }
        path[k] = nb;
        path[k + 1] = nn;
        return true;
    }
    false
}

/// Ladders an explicit set at its shared node: the arrow leaves (or reaches) that node
/// as one line and branches a clearance out, one rung per listed node — the fallback
/// when the listed nodes stand apart or leave their box by different faces.
fn fan_at_shared(compiled: &CompiledGraph, layout: &LayoutResult, plans: &mut HashMap<EdgeIndex, EdgeRoutingPlan>, tokens: &DesignTokens, k: usize) {
    let info = &compiled.sets[k];
    let Some(nl) = layout.positions.get(&info.shared) else { return };
    let rect = (nl.x - 1.0, nl.y - 1.0, nl.x + nl.width + 1.0, nl.y + nl.height + 1.0);
    let clearance = tokens.stub_clearance();
    let lane = tokens.channel_pitch().max(10.0);
    let debug = std::env::var_os("RDG_DEBUG_BUNDLE").is_some();
    let name = |n: NodeIndex| compiled.graph[n].id.clone();
    let mut ids: Vec<EdgeIndex> = plans.keys().copied().collect();
    ids.sort_by_key(|e| e.index());
    let members: Vec<EdgeIndex> = ids.iter().copied().filter(|e| compiled.graph[*e].set == Some(k) && plans[e].junction.is_none() && plans[e].start.is_none()).collect();
    if members.len() < 2 {
        return;
    }
    let others: Vec<Vec<Pt>> = ids.iter().filter(|e| !members.contains(e)).filter_map(|e| edge_full_path(compiled, layout, *e, &plans[e])).collect();
    let borders = border_lines(&rdg_layout::groups::group_rects(compiled, layout, tokens));
    let obstacles = node_obstacles(layout);
    // Routes read from the shared node outward.
    let mut exits: Vec<(EdgeIndex, Vec<Pt>, usize, Pt, Side)> = Vec::new();
    for &e in &members {
        let Some(mut path) = edge_full_path(compiled, layout, e, &plans[&e]) else { continue };
        if info.fan_in {
            path.reverse();
        }
        if let Some((kk, cross, side)) = exit_of(&path, rect) {
            exits.push((e, path, kk, cross, side));
        }
    }
    let mut by_side: HashMap<u8, usize> = HashMap::new();
    for ex in &exits {
        *by_side.entry(ex.4 as u8).or_default() += 1;
    }
    let Some(face) = by_side.iter().max_by_key(|(s, n)| (**n, std::cmp::Reverse(**s))).map(|(s, _)| *s) else { return };
    let same: Vec<&(EdgeIndex, Vec<Pt>, usize, Pt, Side)> = exits.iter().filter(|ex| ex.4 as u8 == face).collect();
    if same.len() < 2 {
        if debug {
            eprintln!("fan at {}: members leave by different faces ({} on the majority face)", name(info.shared), same.len());
        }
        return;
    }
    let len = |p: &[Pt]| p.windows(2).map(|w| (w[1].0 - w[0].0).abs() + (w[1].1 - w[0].1).abs()).sum::<f64>();
    let trunk = same.iter().min_by(|a, b| len(&a.1).total_cmp(&len(&b.1)).then(a.0.index().cmp(&b.0.index()))).expect("members");
    let (trunk_e, trunk_k, trunk_cross, side) = (trunk.0, trunk.2, trunk.3, trunk.4);
    let mut trunk_path = trunk.1.clone();
    let needed = clearance + (same.len() as f64) * lane;
    let mut outward = outward_of(&trunk_path, trunk_k, trunk_cross, side);
    if outward.1 && outward.0 < needed {
        let clear = |a: Pt, b: Pt| !crowded_by(&others, &borders, lane, a, b);
        if extend_trunk(&mut trunk_path, trunk_k, trunk_cross, side, needed, &Room { obstacles: &obstacles, clear: &clear, margin: tokens.px(0.5), lane }) {
            outward = outward_of(&trunk_path, trunk_k, trunk_cross, side);
            let mut w: Vec<Pt> = trunk_path[1..trunk_path.len() - 1].to_vec();
            if info.fan_in {
                w.reverse();
            }
            plans.get_mut(&trunk_e).expect("trunk has a plan").waypoints = w;
        }
    }
    if !outward.1 || outward.0 < clearance + lane {
        if debug {
            eprintln!("fan at {}: trunk's leg past the face is not straight or too short ({:.0})", name(info.shared), outward.0);
        }
        return;
    }
    let vertical = matches!(side, Side::Top | Side::Bottom);
    let along = |p: Pt| if vertical { p.0 } else { p.1 };
    let trunk_along = along(trunk_cross);
    let mut joiners: Vec<&(EdgeIndex, Vec<Pt>, usize, Pt, Side)> = same.iter().copied().filter(|ex| ex.0 != trunk_e).collect();
    joiners.sort_by(|a, b| (along(a.3) - trunk_along).abs().total_cmp(&(along(b.3) - trunk_along).abs()).then(a.0.index().cmp(&b.0.index())));
    let mut rung = 0usize;
    let room = Room { obstacles: &obstacles, clear: &|a: Pt, b: Pt| !crowded_by(&others, &borders, lane, a, b), margin: tokens.px(0.5), lane };
    for ex in joiners {
        let (e, kk, cross) = (ex.0, ex.2, ex.3);
        let mut path = ex.1.clone();
        let mut own = outward_of(&path, kk, cross, side);
        if !own.1 {
            continue;
        }
        // A member whose leg ends before the next rung has it pushed out that far first.
        // (Pushing it out as far as the trunk allows, so every rung is open, measured
        // four more crossings on the fixtures.)
        let want = (clearance + (rung as f64 + 1.0) * lane).min(outward.0 - lane);
        if own.0 + 0.5 < want && extend_trunk(&mut path, kk, cross, side, want, &room) {
            own = outward_of(&path, kk, cross, side);
        }
        let crowded = |a: Pt, b: Pt| crowded_by(&others, &borders, lane, a, b);
        let ladder = Ladder { clearance, lane, room: outward.0, own_leg: own.0, side, cross, trunk_cross };
        let Some((r, drop, junction)) = best_rung(rung, &ladder, &others, &crowded) else {
            if debug {
                eprintln!("fan at {}: no rung line left from rung {rung} (trunk leg {:.0}, member leg {:.0})", name(info.shared), outward.0, own.0);
            }
            break;
        };
        rung = r;
        let m = tokens.px(0.5);
        if obstacles.iter().filter(|(n, _)| *n != info.shared).any(|(_, o)| o.clips_segment(drop.0, drop.1, junction.0, junction.1, m)) {
            continue;
        }
        // The member's route past the rung: from just outside the face on to its end.
        let rest: Vec<Pt> = path[kk..path.len() - 1].to_vec();
        let _ = &own;
        let plan = plans.get_mut(&e).expect("member has a plan");
        if info.fan_in {
            // The arrow runs member -> shared: turned back, it ends on the trunk.
            let mut w = rest;
            w.reverse();
            w.push(drop);
            plan.waypoints = w;
            plan.junction = Some(junction);
        } else {
            let mut w = vec![drop];
            w.extend(rest);
            plan.waypoints = w;
            plan.start = Some(junction);
        }
        rung += 1;
        if debug {
            let (s, t) = compiled.graph.edge_endpoints(e).expect("edge");
            eprintln!("fan {} -> {}: branches at {}'s face at ({:.0},{:.0}) rung {rung}", name(s), name(t), name(info.shared), junction.0, junction.1);
        }
    }
}

fn bundle(compiled: &CompiledGraph, layout: &LayoutResult, plans: &mut HashMap<EdgeIndex, EdgeRoutingPlan>, tokens: &DesignTokens, fan: Fan) -> std::collections::HashSet<usize> {
    let mut done = std::collections::HashSet::new();
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
    // Collector sets: (the shared node, what bounds the members, meaning) -> member
    // edges. Fan-in: members are sources; fan-out: members are targets. The bound is a
    // group the members share — or, for an arrow written with several nodes at one
    // end, the set itself (`(true, set)`): those bundle whatever their size, style,
    // ports or numbering, from the box round their members.
    let mut sets: HashMap<(NodeIndex, (bool, usize), String), Vec<EdgeIndex>> = HashMap::new();
    let mut edge_ids: Vec<EdgeIndex> = plans.keys().copied().collect();
    edge_ids.sort_by_key(|e| e.index());
    for &e in &edge_ids {
        let Some((s, t, _, _)) = resolve_edge_layout(compiled, layout, e) else { continue };
        let d = &compiled.graph[e];
        if s == t || plans[&e].junction.is_some() || plans[&e].start.is_some() {
            continue;
        }
        if let Some(k) = d.set {
            let info = &compiled.sets[k];
            if (fan == Fan::In) == info.fan_in {
                sets.entry((info.shared, (true, k), String::new())).or_default().push(e);
            }
            continue;
        }
        if d.step.is_some() || d.source_port.is_some() || d.target_port.is_some() {
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
        sets.entry((t, (false, g), meaning)).or_default().push(e);
    }
    let mut keys: Vec<&(NodeIndex, (bool, usize), String)> = sets.keys().collect();
    keys.sort_by(|a, b| a.0.index().cmp(&b.0.index()).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let obstacles = node_obstacles(layout);
    let clearance = tokens.stub_clearance();
    let lane = tokens.channel_pitch().max(10.0);
    for key in keys {
        let members = &sets[key];
        let explicit = key.1.0;
        if members.len() < if explicit { 2 } else { MIN_BUNDLE } {
            continue;
        }
        let rect = if explicit {
            // The innermost group every listed node is in and the other end is not —
            // the ladder then forms a clearance outside its border, as for a group's
            // own bundle; or, with no such group, the box round the listed nodes, a
            // hair past their faces so each member's port lies inside it.
            let info = &compiled.sets[key.1.1];
            let in_shared = chains.get(&info.shared).cloned().unwrap_or_default();
            let common = info.members.first().and_then(|m| chains.get(m)).and_then(|chain| {
                chain.iter().find(|g| !in_shared.contains(g) && info.members.iter().all(|m| chains.get(m).is_some_and(|c| c.contains(g)))).copied()
            });
            match common.and_then(|g| group_rects[g]) {
                Some((gx, gy, gw, gh)) => (gx, gy, gx + gw, gy + gh),
                None => {
                    let mut r: Option<(f64, f64, f64, f64)> = None;
                    for n in &info.members {
                        let Some(nl) = layout.positions.get(n) else { continue };
                        let e = r.get_or_insert((nl.x, nl.y, nl.x + nl.width, nl.y + nl.height));
                        *e = (e.0.min(nl.x), e.1.min(nl.y), e.2.max(nl.x + nl.width), e.3.max(nl.y + nl.height));
                    }
                    let Some((x0, y0, x1, y1)) = r else { continue };
                    (x0 - 1.0, y0 - 1.0, x1 + 1.0, y1 + 1.0)
                }
            }
        } else {
            let Some((gx, gy, gw, gh)) = group_rects[key.1.1] else { continue };
            (gx, gy, gx + gw, gy + gh)
        };
        // Each member's route and where it leaves the group — for a fan-out, where it
        // enters, which is where the reversed route leaves; everything below works on
        // the route in that direction and is turned back at the end.
        // Every other arrow's route, as it stands: a rung keeps a lane from their legs.
        let mut others: Vec<Vec<Pt>> = Vec::new();
        let mut other_ids: Vec<EdgeIndex> = plans.keys().copied().filter(|e| !members.contains(e)).collect();
        other_ids.sort_by_key(|e| e.index());
        for e in other_ids {
            if let Some(p) = edge_full_path(compiled, layout, e, &plans[&e]) {
                others.push(p);
            }
        }
        let borders = border_lines(&group_rects);
        let crowded = |a: Pt, b: Pt| crowded_by(&others, &borders, lane, a, b);
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
        let debug = std::env::var_os("RDG_DEBUG_BUNDLE").is_some();
        let name = |n: NodeIndex| compiled.graph[n].id.clone();
        if exits.len() < if explicit { 2 } else { MIN_BUNDLE } {
            if debug {
                eprintln!("bundle at {}: only {} of {} members leave the box ({:.0},{:.0})-({:.0},{:.0})", name(key.0), exits.len(), members.len(), rect.0, rect.1, rect.2, rect.3);
            }
            continue;
        }
        // The majority face, then the trunk: the shortest route leaving by it.
        let mut by_side: HashMap<u8, usize> = HashMap::new();
        for ex in &exits {
            *by_side.entry(ex.4 as u8).or_default() += 1;
        }
        let face = *by_side.iter().max_by_key(|(s, n)| (**n, std::cmp::Reverse(**s))).map(|(s, _)| s).expect("at least one exit");
        let same: Vec<&(EdgeIndex, Vec<Pt>, usize, Pt, Side)> = exits.iter().filter(|ex| ex.4 as u8 == face).collect();
        if same.len() < if explicit { 2 } else { MIN_BUNDLE } {
            if debug {
                eprintln!("bundle at {}: members leave by different faces ({} on the majority face)", name(key.0), same.len());
            }
            continue;
        }
        let len = |p: &[Pt]| p.windows(2).map(|w| (w[1].0 - w[0].0).abs() + (w[1].1 - w[0].1).abs()).sum::<f64>();
        let trunk = same.iter().min_by(|a, b| len(&a.1).total_cmp(&len(&b.1)).then(a.0.index().cmp(&b.0.index()))).expect("members");
        let (trunk_e, trunk_k, trunk_cross, side) = (trunk.0, trunk.2, trunk.3, trunk.4);
        let mut trunk_path = trunk.1.clone();
        // The trunk's first leg outside must run straight away from the border, long
        // enough for the ladder; its line is where the members join. A trunk that
        // turns too early is pushed out when the room is there.
        let needed = clearance + (same.len() as f64) * lane;
        let mut outward = outward_of(&trunk_path, trunk_k, trunk_cross, side);
        if outward.1 && outward.0 < needed {
            let clear = |a: Pt, b: Pt| !crowded(a, b);
            if extend_trunk(&mut trunk_path, trunk_k, trunk_cross, side, needed, &Room { obstacles: &obstacles, clear: &clear, margin: tokens.px(0.5), lane }) {
                outward = outward_of(&trunk_path, trunk_k, trunk_cross, side);
                let mut w: Vec<Pt> = trunk_path[1..trunk_path.len() - 1].to_vec();
                if fan == Fan::Out {
                    w.reverse();
                }
                plans.get_mut(&trunk_e).expect("trunk has a plan").waypoints = w;
            }
        }
        if !outward.1 || outward.0 < clearance + lane {
            if debug {
                eprintln!("bundle at {}: trunk {}'s leg past the border is not straight or too short ({:.0})", name(key.0), name(compiled.graph.edge_endpoints(trunk_e).expect("edge").0), outward.0);
            }
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
                if debug {
                    eprintln!("bundle at {}: member {:?} does not cross the border straight", name(key.0), compiled.graph.edge_endpoints(e).map(|(a, b)| (name(a), name(b))));
                }
                continue;
            }
            // The first rung line clear of other arrows' legs; the ladder stops where
            // the trunk's leg ends, and a member whose own leg ends before the rung
            // cannot join there.
            let mut path = path.clone();
            let mut own_leg = outward_of(&path, k, cross, side).0;
            let room = Room { obstacles: &obstacles, clear: &|a: Pt, b: Pt| !crowded(a, b), margin: tokens.px(0.5), lane };
            let want = (clearance + (rung as f64 + 1.0) * lane).min(outward.0 - lane);
            if own_leg + 0.5 < want && extend_trunk(&mut path, k, cross, side, want, &room) {
                own_leg = outward_of(&path, k, cross, side).0;
            }
            let ladder = Ladder { clearance, lane, room: outward.0, own_leg, side, cross, trunk_cross };
            let Some((r, drop, junction)) = best_rung(rung, &ladder, &others, &crowded) else {
                if debug {
                    eprintln!("bundle at {}: no rung line left from rung {rung} (trunk leg {:.0}, member leg {own_leg:.0})", name(key.0), outward.0);
                }
                break;
            };
            rung = r;
            // The rung must clip no node — other than the set's own members, which it
            // runs a clearance from by design (their obstacle boxes carry that clearance).
            let m = tokens.px(0.5);
            let own = |n: &NodeIndex| explicit && compiled.sets[key.1.1].members.contains(n);
            if let Some((n, _)) = obstacles.iter().filter(|(n, _)| !own(n)).find(|(_, o)| o.clips_segment(drop.0, drop.1, junction.0, junction.1, m) || o.clips_segment(cross.0, cross.1, drop.0, drop.1, m)) {
                if debug {
                    let nl = &layout.positions[n];
                    eprintln!("bundle at {}: rung for {:?} at ({:.0},{:.0})-({:.0},{:.0}) clips {} at ({:.0},{:.0}) {:.0}x{:.0}; box ({:.0},{:.0})-({:.0},{:.0})", name(key.0), compiled.graph.edge_endpoints(e).map(|(a, b)| (name(a), name(b))), drop.0, drop.1, junction.0, junction.1, name(*n), nl.x, nl.y, nl.width, nl.height, rect.0, rect.1, rect.2, rect.3);
                }
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
            if explicit {
                done.insert(key.1.1);
            }
            if std::env::var_os("RDG_DEBUG_BUNDLE").is_some() {
                let name = |n: NodeIndex| compiled.graph[n].id.clone();
                let (s, t) = compiled.graph.edge_endpoints(e).expect("edge");
                let (ts, _) = compiled.graph.edge_endpoints(trunk_e).expect("edge");
                eprintln!("bundle {} -> {}: joins {}'s trunk at ({:.0},{:.0}) rung {}", name(s), name(t), name(ts), junction.0, junction.1, rung);
            }
        }
    }
    done
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
