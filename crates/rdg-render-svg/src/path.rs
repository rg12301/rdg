//! SVG path-string emission for orthogonal connector routes.
//!
//! This is genuinely SVG-specific (emits raw `M`/`L`/`Q` path syntax), unlike the
//! waypoint *geometry* in `rdg_render_core::routing`, which both backends share.

/// Compute an orthogonal SVG path with smooth fillet corners (R = 8px) passing through all waypoints.
/// Returns `(path_d, label_center_x, label_center_y)`.
/// Places label at the midpoint of the longest segment away from bends and crossings.
pub fn build_orthogonal_svg_path(
    p1: (f64, f64),
    p2: (f64, f64),
    waypoints: &[(f64, f64)],
) -> (String, f64, f64) {
    let mut all_points = Vec::with_capacity(waypoints.len() + 2);
    all_points.push(p1);
    all_points.extend_from_slice(waypoints);
    all_points.push(p2);

    let n = all_points.len();
    if n <= 1 {
        return (String::new(), 0.0, 0.0);
    }
    if n == 2 {
        let (a, b) = (all_points[0], all_points[1]);
        return (
            format!("M {:.1} {:.1} L {:.1} {:.1}", a.0, a.1, b.0, b.1),
            (a.0 + b.0) / 2.0,
            (a.1 + b.1) / 2.0,
        );
    }

    // Identify longest segment for label placement away from corners
    let mut max_seg_len = -1.0_f64;
    let mut label_pos = (
        (all_points[0].0 + all_points[1].0) / 2.0,
        (all_points[0].1 + all_points[1].1) / 2.0,
    );

    for i in 0..n - 1 {
        let dx = all_points[i + 1].0 - all_points[i].0;
        let dy = all_points[i + 1].1 - all_points[i].1;
        let seg_len = (dx * dx + dy * dy).sqrt();
        if seg_len > max_seg_len {
            max_seg_len = seg_len;
            label_pos = (
                (all_points[i].0 + all_points[i + 1].0) / 2.0,
                (all_points[i].1 + all_points[i + 1].1) / 2.0,
            );
        }
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
            d.push_str(&format!(" L {:.1} {:.1}", corner.0, corner.1));
            current_pt = corner;
        } else {
            let u1_x = d1_x / len1;
            let u1_y = d1_y / len1;
            let u2_x = d2_x / len2;
            let u2_y = d2_y / len2;

            let in_pt = (corner.0 - u1_x * r, corner.1 - u1_y * r);
            let out_pt = (corner.0 + u2_x * r, corner.1 + u2_y * r);

            d.push_str(&format!(
                " L {:.1} {:.1} Q {:.1} {:.1} {:.1} {:.1}",
                in_pt.0, in_pt.1, corner.0, corner.1, out_pt.0, out_pt.1
            ));
            current_pt = out_pt;
        }
    }

    let last = all_points[n - 1];
    d.push_str(&format!(" L {:.1} {:.1}", last.0, last.1));

    (d, label_pos.0, label_pos.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_two_point_path_is_a_straight_line() {
        let (d, lx, ly) = build_orthogonal_svg_path((0.0, 0.0), (10.0, 0.0), &[]);
        assert_eq!(d, "M 0.0 0.0 L 10.0 0.0");
        assert_eq!((lx, ly), (5.0, 0.0));
    }

    #[test]
    fn test_path_with_waypoints_has_fillet_curves() {
        let (d, _, _) = build_orthogonal_svg_path((0.0, 0.0), (20.0, 20.0), &[(20.0, 0.0)]);
        assert!(
            d.contains('Q'),
            "a corner between two segments should be filleted"
        );
    }
}
