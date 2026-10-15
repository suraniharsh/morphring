//! Pure geometry for the ring (donut) menu: slice angles, hit testing and
//! slice outlines as SVG path data.
//!
//! Angles are in radians, measured clockwise from 12 o'clock, in screen
//! coordinates (y grows downwards). Slice `i` of `n` is centred on
//! `i * span(n)`, so slice 0 sits straight up and the rest follow clockwise.
//!
//! Ported from Filee's `src/Filee.App/Controls/DonutGeometry.cs` and the
//! wedge builder in `DonutMenu.cs` (`CreateWedge`, `CreateRing`).

use std::f64::consts::{PI, TAU};

/// Width in pixels of the visual gap between two neighbouring slices.
pub const GAP_PX: f64 = 3.0;

/// How far in pixels a hovered slice pops outwards. Hit testing extends the
/// outer radius by this much so a popped-out slice stays hit.
pub const POP_OUT_PX: f64 = 7.0;

// Anything within a hair of a full turn is drawn as a ring. An SVG arc whose
// start and end points coincide draws nothing at all, which is a very minimal
// look but not the one I was going for.
const FULL_TURN_EPSILON: f64 = 0.001;

// The smallest half-angle a wedge may shrink to. With lots of slices on a tiny
// ring the gap eats the whole slice; Filee keeps a sliver rather than letting
// the arc flip inside out, and so do I.
const MIN_HALF_ANGLE: f64 = 0.001;

/// A point in screen coordinates (logical pixels, y grows downwards).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    /// Creates a point.
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    fn distance_to(self, other: Point) -> f64 {
        (self.x - other.x).hypot(self.y - other.y)
    }
}

/// What lies under a point of the ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// Outside the ring.
    Outside,
    /// The hole in the middle (cancel or close).
    Center,
    /// The slice with this index.
    Segment(usize),
    /// The ring of a donut that has no slices.
    EmptyRing,
}

/// Angle covered by one of `count` slices; a full turn when there are none.
pub fn span(count: usize) -> f64 {
    if count == 0 { TAU } else { TAU / count as f64 }
}

/// Centre angle of slice `index` out of `count`.
pub fn mid_angle(index: usize, count: usize) -> f64 {
    index as f64 * span(count)
}

/// The point at `radius` from `center` in the direction `angle`.
pub fn point_at(center: Point, radius: f64, angle: f64) -> Point {
    Point::new(
        center.x + radius * angle.sin(),
        center.y - radius * angle.cos(),
    )
}

/// The angle of `p` around `center`, in `0..2π`.
pub fn angle_of(center: Point, p: Point) -> f64 {
    // atan2(x, -y) instead of the textbook atan2(y, x): that single swap turns
    // "counter-clockwise from 3 o'clock" into "clockwise from noon".
    let a = (p.x - center.x).atan2(center.y - p.y);
    if a < 0.0 { a + TAU } else { a }
}

/// Index of the slice containing `angle`, or `None` when there are no slices.
pub fn index_at(angle: f64, count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let span = span(count);
    // Shift by half a slice because slice 0 straddles noon. rem_euclid instead
    // of %, so a slightly negative angle wraps to the last slice rather than
    // becoming a negative index (C# would happily hand you -0 or -1 here).
    let index = ((angle + span / 2.0) / span)
        .floor()
        .rem_euclid(count as f64);
    Some((index as usize).min(count - 1))
}

/// Classifies `p` against a ring centred on `center`, using the standard
/// pop-out slack ([`POP_OUT_PX`]) on the outer edge.
pub fn hit_test(
    center: Point,
    p: Point,
    inner_radius: f64,
    outer_radius: f64,
    count: usize,
) -> Hit {
    hit_test_with_slack(center, p, inner_radius, outer_radius, count, POP_OUT_PX)
}

/// Classifies `p` against a ring centred on `center`. `slack` extends the
/// outer radius, so slices that pop out on hover remain hittable.
///
/// The gaps between slices are not special: a point in a gap belongs to the
/// nearest slice by angle.
pub fn hit_test_with_slack(
    center: Point,
    p: Point,
    inner_radius: f64,
    outer_radius: f64,
    count: usize,
    slack: f64,
) -> Hit {
    let distance = center.distance_to(p);
    if distance < inner_radius {
        return Hit::Center;
    }
    if distance > outer_radius + slack {
        return Hit::Outside;
    }
    match index_at(angle_of(center, p), count) {
        Some(index) => Hit::Segment(index),
        None => Hit::EmptyRing,
    }
}

/// SVG path data for slice `index` of `count` between the two radii.
///
/// A single slice, or no slice at all, is drawn as a full ring, which must be
/// filled with `fill-rule="evenodd"` (see [`ring_path`]).
pub fn slice_path(
    center: Point,
    inner_radius: f64,
    outer_radius: f64,
    index: usize,
    count: usize,
) -> String {
    wedge_path(
        center,
        inner_radius,
        outer_radius,
        mid_angle(index, count),
        span(count),
    )
}

/// SVG path data for a wedge of `span` radians centred on `mid`, with a gap of
/// [`GAP_PX`] pixels to each neighbour. A span of a full turn yields a full
/// ring (see [`ring_path`]).
pub fn wedge_path(
    center: Point,
    inner_radius: f64,
    outer_radius: f64,
    mid: f64,
    span: f64,
) -> String {
    if span >= TAU - FULL_TURN_EPSILON {
        return ring_path(center, inner_radius, outer_radius);
    }

    // The gap is constant in pixels, not in angle, so each radius trims its
    // own half-angle. Trim both by the same angle instead and the gap turns
    // into a little pizza slice of nothing, wide outside and thin inside.
    let outer_half = (span / 2.0 - GAP_PX / 2.0 / outer_radius).max(MIN_HALF_ANGLE);
    let inner_half = (span / 2.0 - GAP_PX / 2.0 / inner_radius).max(MIN_HALF_ANGLE);

    let o1 = point_at(center, outer_radius, mid - outer_half);
    let o2 = point_at(center, outer_radius, mid + outer_half);
    let i1 = point_at(center, inner_radius, mid + inner_half);
    let i2 = point_at(center, inner_radius, mid - inner_half);
    let outer_large = u8::from(outer_half * 2.0 > PI);
    let inner_large = u8::from(inner_half * 2.0 > PI);

    // In SVG's y-down world sweep-flag 1 is clockwise on screen: out along the
    // outer edge clockwise, back along the inner edge counter-clockwise.
    format!(
        "M {} {} A {r2} {r2} 0 {outer_large} 1 {} {} L {} {} A {r1} {r1} 0 {inner_large} 0 {} {} Z",
        num(o1.x),
        num(o1.y),
        num(o2.x),
        num(o2.y),
        num(i1.x),
        num(i1.y),
        num(i2.x),
        num(i2.y),
        r1 = num(inner_radius),
        r2 = num(outer_radius),
    )
}

/// SVG path data for a full ring: the outer and the inner circle, each made of
/// two half-circle arcs. Fill it with `fill-rule="evenodd"` so the hole stays
/// empty.
pub fn ring_path(center: Point, inner_radius: f64, outer_radius: f64) -> String {
    // One arc cannot draw a whole circle (same start and end point), so each
    // circle is two half-circles from noon to six and back.
    let circle = |r: f64| {
        let (x, top, bottom, r) = (num(center.x), num(center.y - r), num(center.y + r), num(r));
        format!("M {x} {top} A {r} {r} 0 0 1 {x} {bottom} A {r} {r} 0 0 1 {x} {top} Z")
    };
    format!("{} {}", circle(outer_radius), circle(inner_radius))
}

// Three decimals is a thousandth of a pixel: plenty for a renderer, short
// enough that a path stays readable in a test failure. Trailing zeros go, and
// so does the "-0" that rounding a tiny negative would otherwise leave behind.
fn num(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".to_owned()
    } else {
        s.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: Point = Point::new(200.0, 200.0);

    fn hit(x: f64, y: f64, count: usize) -> Hit {
        hit_test_with_slack(C, Point::new(x, y), 60.0, 150.0, count, 0.0)
    }

    fn assert_close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-6, "{a} != {b}");
    }

    // Port of DonutGeometryTests.Slices_cover_the_full_circle.
    #[test]
    fn slices_cover_the_full_circle() {
        for count in [1, 2, 5, 8, 12] {
            assert_close(span(count) * count as f64, TAU);
            // Each slice's centre maps back to itself...
            for i in 0..count {
                assert_eq!(
                    index_at(mid_angle(i, count), count),
                    Some(i),
                    "count {count}"
                );
            }
            // ...and every direction maps to exactly one slice.
            for deg in 0..360 {
                let index = index_at(f64::from(deg).to_radians(), count);
                assert!(
                    matches!(index, Some(i) if i < count),
                    "count {count} deg {deg}"
                );
            }
        }
    }

    // Port of DonutGeometryTests.First_slice_is_centred_at_twelve_o_clock.
    #[test]
    fn first_slice_is_centred_at_twelve_o_clock() {
        // Straight up is slice 0, and so is slightly left of up: it straddles noon.
        assert_eq!(hit(200.0, 80.0, 6), Hit::Segment(0));
        assert_eq!(hit(190.0, 80.0, 6), Hit::Segment(0));
        // With 4 slices: 3 o'clock is 1, 6 o'clock is 2, 9 o'clock is 3.
        assert_eq!(hit(320.0, 200.0, 4), Hit::Segment(1));
        assert_eq!(hit(200.0, 320.0, 4), Hit::Segment(2));
        assert_eq!(hit(80.0, 200.0, 4), Hit::Segment(3));
    }

    // Port of DonutGeometryTests.Hole_ring_and_outside_are_classified.
    #[test]
    fn hole_ring_and_outside_are_classified() {
        assert_eq!(hit(210.0, 205.0, 6), Hit::Center);
        assert_eq!(hit(200.0, 100.0, 6), Hit::Segment(0));
        assert_eq!(hit(200.0, 10.0, 6), Hit::Outside);
        let slack = hit_test_with_slack(C, Point::new(200.0, 45.0), 60.0, 150.0, 6, 8.0);
        assert_eq!(slack, Hit::Segment(0));
        assert_eq!(hit(200.0, 100.0, 0), Hit::EmptyRing);
    }

    #[test]
    fn hit_test_uses_the_pop_out_slack() {
        // 156 px out: inside 150 + 7, outside 150 + 0.
        let p = Point::new(200.0, 44.0);
        assert_eq!(hit_test(C, p, 60.0, 150.0, 6), Hit::Segment(0));
        assert_eq!(hit_test_with_slack(C, p, 60.0, 150.0, 6, 0.0), Hit::Outside);
        // 158 px out is beyond the slack too.
        assert_eq!(
            hit_test(C, Point::new(200.0, 42.0), 60.0, 150.0, 6),
            Hit::Outside
        );
    }

    #[test]
    fn boundaries_belong_to_the_ring() {
        // Exactly on the inner edge is ring, exactly on the outer edge is ring.
        assert_eq!(hit(200.0, 140.0, 6), Hit::Segment(0));
        assert_eq!(hit(200.0, 50.0, 6), Hit::Segment(0));
    }

    #[test]
    fn angles_run_clockwise_from_noon() {
        assert_close(angle_of(C, Point::new(200.0, 100.0)), 0.0);
        assert_close(angle_of(C, Point::new(300.0, 200.0)), PI / 2.0);
        assert_close(angle_of(C, Point::new(200.0, 300.0)), PI);
        assert_close(angle_of(C, Point::new(100.0, 200.0)), 3.0 * PI / 2.0);

        let p = point_at(C, 100.0, PI / 2.0);
        assert_close(p.x, 300.0);
        assert_close(p.y, 200.0);
        // point_at and angle_of are inverses all the way round.
        for deg in 0..360 {
            let a = f64::from(deg).to_radians();
            assert_close(angle_of(C, point_at(C, 50.0, a)), a);
        }
    }

    #[test]
    fn index_at_handles_no_slices_and_out_of_range_angles() {
        assert_eq!(index_at(1.0, 0), None);
        // Just left of noon is still slice 0; a full turn wraps back to 0.
        assert_eq!(index_at(TAU - 0.01, 4), Some(0));
        assert_eq!(index_at(TAU, 4), Some(0));
        assert_eq!(index_at(-0.01, 4), Some(0));
        assert_eq!(index_at(-PI / 2.0, 4), Some(3));
    }

    #[test]
    fn slice_boundaries_sit_halfway_between_centres() {
        // With 4 slices the boundary between 0 and 1 is at 45 degrees.
        assert_eq!(index_at(PI / 4.0 - 1e-9, 4), Some(0));
        assert_eq!(index_at(PI / 4.0 + 1e-9, 4), Some(1));
    }

    // Pulls the numbers out of an SVG path so tests can check the points.
    fn numbers(path: &str) -> Vec<f64> {
        path.split_whitespace()
            .filter_map(|t| t.parse::<f64>().ok())
            .collect()
    }

    #[test]
    fn wedge_gap_is_constant_in_pixels() {
        let (r1, r2) = (60.0, 150.0);
        let count = 4;
        let a = slice_path(C, r1, r2, 0, count);
        let b = slice_path(C, r1, r2, 1, count);
        let (na, nb) = (numbers(&a), numbers(&b));
        // Layout: M x y | A r r 0 large 1 x y | L x y | A r r 0 large 0 x y
        let a_outer_end = Point::new(na[7], na[8]);
        let a_inner_start = Point::new(na[9], na[10]);
        let b_outer_start = Point::new(nb[0], nb[1]);
        let b_inner_end = Point::new(nb[16], nb[17]);
        // The chord across the gap is 3 px (to within rounding) at both radii.
        let outer_gap = a_outer_end.distance_to(b_outer_start);
        let inner_gap = a_inner_start.distance_to(b_inner_end);
        assert!((outer_gap - GAP_PX).abs() < 0.01, "outer gap {outer_gap}");
        assert!((inner_gap - GAP_PX).abs() < 0.01, "inner gap {inner_gap}");
    }

    #[test]
    fn wedge_path_shape_and_flags() {
        let path = slice_path(C, 60.0, 150.0, 0, 4);
        assert!(path.starts_with("M "), "{path}");
        assert!(path.ends_with(" Z"), "{path}");
        assert!(path.contains("A 150 150 0 0 1 "), "{path}");
        assert!(path.contains("A 60 60 0 0 0 "), "{path}");
        // Slice 0 is symmetric around noon: its first outer point is left of
        // centre and above it.
        let n = numbers(&path);
        assert!(n[0] < 200.0 && n[1] < 200.0, "{path}");
        assert!((n[0] + n[7] - 400.0).abs() < 0.002, "{path}");
        assert!((n[1] - n[8]).abs() < 0.002, "{path}");
    }

    #[test]
    fn wedges_wider_than_half_a_turn_use_the_large_arc() {
        let path = wedge_path(C, 60.0, 150.0, 0.0, 1.5 * PI);
        assert!(path.contains("A 150 150 0 1 1 "), "{path}");
        assert!(path.contains("A 60 60 0 1 0 "), "{path}");
    }

    #[test]
    fn one_slice_or_none_is_a_full_ring() {
        let ring = ring_path(C, 60.0, 150.0);
        assert_eq!(slice_path(C, 60.0, 150.0, 0, 1), ring);
        assert_eq!(slice_path(C, 60.0, 150.0, 0, 0), ring);
        assert_eq!(
            ring,
            "M 200 50 A 150 150 0 0 1 200 350 A 150 150 0 0 1 200 50 Z \
             M 200 140 A 60 60 0 0 1 200 260 A 60 60 0 0 1 200 140 Z"
        );
    }

    #[test]
    fn tiny_slices_keep_a_sliver() {
        // 200 slices on a small ring: the gap is wider than the slice, so the
        // half-angle clamps instead of going negative.
        let path = slice_path(C, 10.0, 20.0, 0, 200);
        let n = numbers(&path);
        // The outer arc still runs left to right across noon.
        assert!(n[0] < 200.0 && n[7] > 200.0, "{path}");
    }

    #[test]
    fn numbers_are_short_and_never_negative_zero() {
        assert_eq!(num(1.0), "1");
        assert_eq!(num(1.25), "1.25");
        assert_eq!(num(-0.0001), "0");
        assert_eq!(num(2.0 / 3.0), "0.667");
    }
}
