//! SVG path-string emission for orthogonal connector routes.
//!
//! This is genuinely SVG-specific (emits raw `M`/`L`/`Q` path syntax), unlike the
//! waypoint *geometry* in `rdg_render_core::routing`, which both backends share.

use std::fmt::Write as _;

/// Compute an orthogonal SVG path with smooth fillet corners passing through all waypoints.
///
/// Returns only the path string: where an edge's label goes is decided in one place,
/// `rdg_render_core::annotate`, which the draw.io backend and the canvas
/// bounds also use. This function used to pick its own anchor (the longest segment's
/// midpoint), so the same edge's label sat on a different leg in SVG than in draw.io.
#[cfg(test)]
pub fn build_orthogonal_svg_path(
    p1: (f64, f64),
    p2: (f64, f64),
    waypoints: &[(f64, f64)],
) -> String {
    build_orthogonal_svg_path_with_jumps(p1, p2, waypoints, &[])
}

/// Radius of a line jump: the horizontal leg hops over a crossing vertical one in a
/// half circle this big (width twice this).
pub const JUMP_RADIUS: f64 = 5.0;

/// A straight run from `from` to `to`, with a hop over every point of `jumps` that lies
/// on it (horizontal runs only: the horizontal line always jumps the vertical one, so a
/// reader never has to work out which line carries on).
fn run_with_jumps(d: &mut String, from: (f64, f64), to: (f64, f64), jumps: &[(f64, f64)]) {
    let r = JUMP_RADIUS;
    if (from.1 - to.1).abs() < 0.5 && (to.0 - from.0).abs() > 2.0 * r + 2.0 {
        let dir = if to.0 > from.0 { 1.0 } else { -1.0 };
        let (lo, hi) = (from.0.min(to.0) + r + 1.0, from.0.max(to.0) - r - 1.0);
        let mut xs: Vec<f64> = jumps.iter().filter(|j| (j.1 - from.1).abs() < 0.75 && j.0 > lo && j.0 < hi).map(|j| j.0).collect();
        xs.sort_by(f64::total_cmp);
        if dir < 0.0 {
            xs.reverse();
        }
        xs.dedup_by(|a, b| (*a - *b).abs() < 2.0 * r);
        let sweep = if dir > 0.0 { 1 } else { 0 };
        for x in xs {
            let _ = write!(d, " L {:.1} {:.1} A {r:.1} {r:.1} 0 0 {sweep} {:.1} {:.1}", x - dir * r, from.1, x + dir * r, from.1);
        }
    }
    let _ = write!(d, " L {:.1} {:.1}", to.0, to.1);
}

/// [`build_orthogonal_svg_path`] with line jumps at `jumps` (points on this path's
/// horizontal legs where a vertical leg of another path crosses it).
pub fn build_orthogonal_svg_path_with_jumps(
    p1: (f64, f64),
    p2: (f64, f64),
    waypoints: &[(f64, f64)],
    jumps: &[(f64, f64)],
) -> String {
    let mut all_points = Vec::with_capacity(waypoints.len() + 2);
    all_points.push(p1);
    all_points.extend_from_slice(waypoints);
    all_points.push(p2);

    let n = all_points.len();
    if n <= 1 {
        return String::new();
    }
    if n == 2 {
        let (a, b) = (all_points[0], all_points[1]);
        let mut d = format!("M {:.1} {:.1}", a.0, a.1);
        run_with_jumps(&mut d, a, b, jumps);
        return d;
    }

    let mut d = format!("M {:.1} {:.1}", all_points[0].0, all_points[0].1);
    let mut current_pt = all_points[0];

    for i in 1..n - 1 {
        let prev = current_pt;
        let corner = all_points[i];
        let next = all_points[i + 1];

        let d1_x = corner.0 - prev.0;
        let d1_y = corner.1 - prev.1;
        let len1 = (d1_x * d1_x + d1_y * d1_y).sqrt();

        let d2_x = next.0 - corner.0;
        let d2_y = next.1 - corner.1;
        let len2 = (d2_x * d2_x + d2_y * d2_y).sqrt();

        // Scale the fillet with the shorter of the two adjacent segments — a flat 8px
        // reads as "wiry" on a large, sweeping diagram, so let long runs curve with up to
        // a 14px radius, while short segments still get a proportionally tighter (down to
        // 4px) fillet rather than one that visually swallows the whole segment.
        let r = (len1.min(len2) * 0.28)
            .clamp(4.0, 14.0)
            .min(len1 / 2.0)
            .min(len2 / 2.0);

        if r < 1.0 || len1 < 1.0 || len2 < 1.0 {
            run_with_jumps(&mut d, current_pt, corner, jumps);
            current_pt = corner;
        } else {
            let u1_x = d1_x / len1;
            let u1_y = d1_y / len1;
            let u2_x = d2_x / len2;
            let u2_y = d2_y / len2;

            let in_pt = (corner.0 - u1_x * r, corner.1 - u1_y * r);
            let out_pt = (corner.0 + u2_x * r, corner.1 + u2_y * r);

            run_with_jumps(&mut d, current_pt, in_pt, jumps);
            d.push_str(&format!(" Q {:.1} {:.1} {:.1} {:.1}", corner.0, corner.1, out_pt.0, out_pt.1));
            current_pt = out_pt;
        }
    }

    let last = all_points[n - 1];
    run_with_jumps(&mut d, current_pt, last, jumps);

    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_horizontal_leg_hops_over_a_crossing() {
        let d = build_orthogonal_svg_path_with_jumps((0.0, 10.0), (100.0, 10.0), &[], &[(50.0, 10.0)]);
        assert_eq!(d, "M 0.0 10.0 L 45.0 10.0 A 5.0 5.0 0 0 1 55.0 10.0 L 100.0 10.0");
        // Right to left sweeps the other way; a vertical leg never hops.
        let back = build_orthogonal_svg_path_with_jumps((100.0, 10.0), (0.0, 10.0), &[], &[(50.0, 10.0)]);
        assert!(back.contains("A 5.0 5.0 0 0 0 45.0 10.0"), "{back}");
        let vertical = build_orthogonal_svg_path_with_jumps((10.0, 0.0), (10.0, 100.0), &[], &[(10.0, 50.0)]);
        assert!(!vertical.contains('A'), "{vertical}");
    }

    #[test]
    fn test_two_point_path_is_a_straight_line() {
        let d = build_orthogonal_svg_path((0.0, 0.0), (10.0, 0.0), &[]);
        assert_eq!(d, "M 0.0 0.0 L 10.0 0.0");
    }

    #[test]
    fn test_path_with_waypoints_has_fillet_curves() {
        let d = build_orthogonal_svg_path((0.0, 0.0), (20.0, 20.0), &[(20.0, 0.0)]);
        assert!(
            d.contains('Q'),
            "a corner between two segments should be filleted"
        );
    }
}
