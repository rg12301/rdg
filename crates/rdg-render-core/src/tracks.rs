//! Tracks for the search router's routes (layout rules section 13, step 2).
//!
//! The search finds each route's topology — which gaps between rows and which gutters
//! between columns it runs through — on a grid whose lines fall wherever ports and
//! lanes happen to lie, so the legs of different arrows in one gap end up a few px
//! apart or on top of each other, and a spreading pass has to pull them apart after.
//! This pass gives every interior leg in a gap a *track* instead, the way the channel
//! router does: the legs of one gap are ordered so that the connectors rising and
//! falling from them cross as little as possible (a leg goes above another when that
//! crosses less), packed into as few tracks as the overlaps allow, and the tracks are
//! spaced a pitch apart about the gap's middle. Column gutters get the same treatment
//! for vertical legs. Runs before spreading, which then has only the leftovers.

use std::collections::HashMap;

use petgraph::stable_graph::EdgeIndex;
use rdg_graph::CompiledGraph;
use rdg_layout::{DesignTokens, LayoutResult};

use crate::routing::EdgeRoutingPlan;

type Pt = (f64, f64);

/// Two legs within this of each other along the gap share a track only when their
/// spans are at least this far apart.
const SHARE_GAP: f64 = 24.0;

/// One interior leg lying in a gap, on the axis being tracked.
#[derive(Debug, Clone)]
struct Leg {
    path: usize,
    k: usize,
    /// Extent along the gap.
    lo: f64,
    hi: f64,
    /// Place across the gap now.
    at: f64,
    /// Where connectors leave the leg toward the row before the gap (`rise`) and the
    /// row after it (`fall`), as positions along the gap.
    rise: Vec<f64>,
    fall: Vec<f64>,
    gap: usize,
}

/// Right-angle crossings between every pair of paths.
fn total_crossings(paths: &[Vec<Pt>]) -> usize {
    let mut n = 0;
    for i in 0..paths.len() {
        for j in i + 1..paths.len() {
            n += crate::polish::crossing_points(&paths[i], &paths[j]).len();
        }
    }
    n
}

/// Maximal bands the node boxes cover across the axis, merged, sorted.
fn bands(mut spans: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: Vec<(f64, f64)> = Vec::new();
    for (a, b) in spans {
        match out.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// Moves every interior leg lying in a gap between rows (horizontal legs) or between
/// columns (vertical legs) onto a track. Returns how many legs moved.
pub fn assign_tracks(compiled: &CompiledGraph, layout: &LayoutResult, plans: &mut HashMap<EdgeIndex, EdgeRoutingPlan>, tokens: &DesignTokens) -> usize {
    if layout.sequence_info.is_some() {
        return 0;
    }
    let (mut paths, path_edge, _ends) = crate::spread::collect_paths(compiled, layout, plans, tokens);
    let pitch = tokens.channel_pitch().max(10.0);
    let clearance = tokens.stub_clearance();
    let debug = std::env::var_os("RDG_DEBUG_TRACKS").is_some();
    let mut moved = 0;
    for vertical in [false, true] {
        // Rows (for horizontal legs) or columns (for vertical ones), and the gaps between.
        let node_spans: Vec<(f64, f64)> = layout.positions.values().map(|nl| if vertical { (nl.x, nl.x + nl.width) } else { (nl.y, nl.y + nl.height) }).collect();
        let rows = bands(node_spans);
        if rows.len() < 2 {
            continue;
        }
        let gaps: Vec<(f64, f64)> = rows.windows(2).map(|w| (w[0].1, w[1].0)).collect();
        let across = |p: Pt| if vertical { p.0 } else { p.1 };
        let along = |p: Pt| if vertical { p.1 } else { p.0 };
        // Interior legs in a gap, with their connectors.
        let mut legs: Vec<Leg> = Vec::new();
        for (pi, pts) in paths.iter().enumerate() {
            let n = pts.len();
            for k in 1..n.saturating_sub(2) {
                let (a, b) = (pts[k], pts[k + 1]);
                let on_axis = (across(a) - across(b)).abs() < 0.5 && (along(a) - along(b)).abs() >= 0.5;
                if !on_axis {
                    continue;
                }
                let at = across(a);
                let Some(gap) = gaps.iter().position(|&(g0, g1)| at > g0 + 1.0 && at < g1 - 1.0) else { continue };
                let (lo, hi) = (along(a).min(along(b)), along(a).max(along(b)));
                let mut rise = Vec::new();
                let mut fall = Vec::new();
                for (end, nb) in [(a, pts[k - 1]), (b, pts[k + 2])] {
                    if across(nb) < at - 0.5 {
                        rise.push(along(end));
                    } else if across(nb) > at + 0.5 {
                        fall.push(along(end));
                    }
                }
                legs.push(Leg { path: pi, k, lo, hi, at, rise, fall, gap });
            }
        }
        let mut gap_ids: Vec<usize> = legs.iter().map(|l| l.gap).collect();
        gap_ids.sort_unstable();
        gap_ids.dedup();
        for g in gap_ids {
            let idx: Vec<usize> = (0..legs.len()).filter(|&i| legs[i].gap == g).collect();
            let n = idx.len();
            if n < 2 {
                continue;
            }
            let spans = |x: f64, l: &Leg| x > l.lo + 0.5 && x < l.hi - 0.5;
            let overlap = |p: &Leg, q: &Leg| p.hi.min(q.hi) - p.lo.max(q.lo) > -SHARE_GAP;
            // p above q (nearer the row before the gap) crosses: q's rising connectors
            // inside p's span, p's falling connectors inside q's span.
            let cross = |p: &Leg, q: &Leg| -> usize { q.rise.iter().filter(|&&x| spans(x, p)).count() + p.fall.iter().filter(|&&x| spans(x, q)).count() };
            let mut above = vec![vec![false; n]; n];
            for a in 0..n {
                for b in 0..n {
                    if a == b {
                        continue;
                    }
                    let (p, q) = (&legs[idx[a]], &legs[idx[b]]);
                    if !overlap(p, q) {
                        continue;
                    }
                    let (c_ab, c_ba) = (cross(p, q), cross(q, p));
                    // Legs of one path joined by a connector keep their order.
                    let joined = p.path == q.path && (p.k + 2 == q.k || q.k + 2 == p.k);
                    if c_ab < c_ba || (c_ab == c_ba && (joined || p.at < q.at)) {
                        above[a][b] = true;
                    }
                }
            }
            // Topological order, cycles broken by the leg nearest the row before.
            let mut order: Vec<usize> = Vec::with_capacity(n);
            let mut placed = vec![false; n];
            while order.len() < n {
                let next = (0..n).filter(|&b| !placed[b] && (0..n).all(|a| placed[a] || !above[a][b])).min_by(|&a, &b| legs[idx[a]].at.total_cmp(&legs[idx[b]].at));
                let b = next.unwrap_or_else(|| (0..n).filter(|&b| !placed[b]).min_by(|&a, &b| legs[idx[a]].at.total_cmp(&legs[idx[b]].at)).expect("unplaced leg"));
                placed[b] = true;
                order.push(b);
            }
            // Left-edge packing: the first track past every leg that must be above, free
            // of overlapping legs.
            let mut track = vec![0usize; n];
            let mut tracks: Vec<Vec<usize>> = Vec::new();
            for &b in &order {
                let floor = (0..n).filter(|&a| above[a][b] && placed[a]).map(|a| track[a] + 1).max().unwrap_or(0);
                let mut t = floor;
                while tracks.get(t).is_some_and(|members| members.iter().any(|&a| overlap(&legs[idx[a]], &legs[idx[b]]))) {
                    t += 1;
                }
                if t >= tracks.len() {
                    tracks.resize_with(t + 1, Vec::new);
                }
                tracks[t].push(b);
                track[b] = t;
            }
            // Track positions: a pitch apart about the gap's middle, a clearance off the
            // rows; closer when the gap is short of room.
            let (g0, g1) = gaps[g];
            let (free0, free1) = (g0 + clearance, g1 - clearance);
            let n_tracks = tracks.len() as f64;
            let step = if n_tracks > 1.0 { pitch.min((free1 - free0).max(0.0) / (n_tracks - 1.0)) } else { 0.0 };
            let mid = (g0 + g1) / 2.0;
            let first = mid - step * (n_tracks - 1.0) / 2.0;
            // The gap's moves stand only when they add no crossing over the whole
            // drawing: the order rule sees the connectors of the gap's own legs, not
            // every line that passes through it.
            let before_pts = paths.clone();
            let before_crossings = total_crossings(&paths);
            let mut gap_moved = 0;
            for b in 0..n {
                let want = (first + step * track[b] as f64).round();
                let l = &legs[idx[b]];
                if (want - l.at).abs() < 0.5 {
                    continue;
                }
                let pts = &mut paths[l.path];
                for p in &mut pts[l.k..=l.k + 1] {
                    if vertical {
                        p.0 = want;
                    } else {
                        p.1 = want;
                    }
                }
                gap_moved += 1;
                if debug {
                    let name = compiled.graph.edge_endpoints(path_edge[l.path]).map_or(String::new(), |(s, d)| format!("{} -> {}", compiled.graph[s].id, compiled.graph[d].id));
                    eprintln!("track: {name} leg {} ({}) {:.0} -> {want:.0} (gap {g}, track {} of {})", l.k, if vertical { "x" } else { "y" }, l.at, track[b], tracks.len());
                }
            }
            if gap_moved > 0 && total_crossings(&paths) > before_crossings {
                if debug {
                    eprintln!("track: gap {g} ({}) kept as it was: the tracks would cross more", if vertical { "columns" } else { "rows" });
                }
                paths = before_pts;
            } else {
                moved += gap_moved;
            }
        }
    }
    if moved > 0 {
        for (pi, p) in paths.iter().enumerate() {
            let mut q = p.clone();
            crate::routing::simplify_orthogonal_polyline(&mut q);
            let plan = plans.get_mut(&path_edge[pi]).expect("path came from a plan");
            plan.waypoints = q[1..q.len() - 1].to_vec();
        }
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_merge_overlapping_spans_in_order() {
        assert_eq!(bands(vec![(100.0, 150.0), (0.0, 50.0), (40.0, 60.0)]), vec![(0.0, 60.0), (100.0, 150.0)]);
    }
}
