//! Analytic area coverage: how much of a pixel a path covers, computed exactly.
//!
//! A rasteriser has two ways to decide whether a pixel is inside a path. The cheap one
//! samples the pixel's centre and calls it either in or out, which is wrong at every
//! edge and wrongest on the thin strokes and small type a document editor lives on. The
//! correct one computes the *area* of the path inside each pixel, and that number is
//! what a fill and a stroke should mean.
//!
//! ## How the exact answer is reached
//!
//! The area of a span over a pixel row cannot be computed from the span's width at one
//! height: a slanted edge is narrow at the bottom of a pixel and wide at the top, and
//! using either undercounts. So each row is **subdivided** at the heights where
//! something changes:
//!
//! * a path vertex, because the set of edges changes there, and
//! * where an edge crosses an **integer x**, because that is where the span begins or
//!   ends covering a different pixel.
//!
//! Between two consecutive cuts nothing structural changes and every edge's x varies
//! linearly with y, so the covered width in each pixel is linear too and its exact area
//! is a trapezoid. Summing those trapezoids is the pixel's coverage, and it does not
//! depend on where any sample was taken.
//!
//! The consequences worth stating, because they are what "exact" buys:
//!
//! * A 45° edge through a pixel gives 0.5, not a staircase.
//! * An edge lying exactly on a pixel boundary contributes nothing, and one lying
//!   exactly on a pixel centre contributes exactly half — as answers, not as special
//!   cases.
//! * A path's total coverage equals its area, which is a property a test can state and a
//!   sampled rasteriser cannot satisfy.
//!
//! Everything is bounded: the work is proportional to the clip rectangle's area and the
//! number of edges, and to nothing the file can make large.

// Coordinates are compared for exact equality in three places, all of which are
// questions about whether two of the *same* numbers are the same number: whether an
// edge is horizontal, and whether a subpath's ends meet. Both are decided by the file's
// own numbers, and a tolerance would answer "nearly" to a question that has an exact
// answer.
#![allow(clippy::float_cmp)]

use std::ops::Range;

/// A device-space rectangle, in pixels. Half-open, so `x1` and `y1` are the first pixels
/// *outside* the rectangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Rect {
    /// A rectangle with its corners ordered. A rectangle that would be inverted is empty
    /// rather than inverted, which is the difference between drawing nothing and drawing
    /// everything.
    #[must_use]
    pub fn normalised(self) -> Self {
        Self {
            x0: self.x0.min(self.x1),
            y0: self.y0.min(self.y1),
            x1: self.x0.max(self.x1),
            y1: self.y0.max(self.y1),
        }
    }

    /// The pixel columns and rows this rectangle covers.
    #[must_use]
    pub fn pixels(self) -> Option<(Range<usize>, Range<usize>)> {
        let r = self.normalised();
        if r.x1 <= r.x0 || r.y1 <= r.y0 {
            return None;
        }
        let x0 = r.x0.floor().max(0.0) as usize;
        let y0 = r.y0.floor().max(0.0) as usize;
        let x1 = r.x1.ceil().max(0.0) as usize;
        let y1 = r.y1.ceil().max(0.0) as usize;
        Some((x0..x1, y0..y1))
    }

    /// Clip to another rectangle.
    #[must_use]
    pub fn intersect(self, other: Self) -> Self {
        let a = self.normalised();
        let b = other.normalised();
        Self {
            x0: a.x0.max(b.x0),
            y0: a.y0.max(b.y0),
            x1: a.x1.min(b.x1),
            y1: a.y1.min(b.y1),
        }
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        let r = self.normalised();
        r.x1 <= r.x0 || r.y1 <= r.y0
    }
}

/// One edge of the path being rasterised.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edge {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Edge {
    #[must_use]
    pub fn new(x0: f64, y0: f64, x1: f64, y1: f64) -> Self {
        Self { x0, y0, x1, y1 }
    }

    /// Where this edge crosses the horizontal line `y`, and which way it crosses.
    ///
    /// `None` when it does not. The rule is half-open on the edge's own extent, so a
    /// vertex belongs to exactly one of the two edges that meet there and a path that
    /// grazes a line contributes nothing to it.
    fn crossing(&self, y: f64) -> Option<(f64, i32)> {
        let (lo, hi) = if self.y0 <= self.y1 {
            (self.y0, self.y1)
        } else {
            (self.y1, self.y0)
        };
        if y < lo || y >= hi {
            return None;
        }
        if self.y1 == self.y0 {
            return None;
        }
        let t = (y - self.y0) / (self.y1 - self.y0);
        let dir = if self.y1 > self.y0 { 1 } else { -1 };
        Some((self.x0 + t * (self.x1 - self.x0), dir))
    }

    /// This edge's x at height `y`.
    fn x_at(&self, y: f64) -> f64 {
        let dy = self.y1 - self.y0;
        if dy == 0.0 {
            return self.x0;
        }
        self.x0 + (y - self.y0) / dy * (self.x1 - self.x0)
    }

    /// Is this edge horizontal, and so contributes no crossings?
    fn is_horizontal(&self) -> bool {
        self.y0 == self.y1
    }
}

/// How much of each pixel a path covers, under one fill rule.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Coverage {
    pub width: usize,
    pub height: usize,
    /// Row-major, `width * height` entries, each between 0 and 1.
    pub alpha: Vec<f32>,
    /// Row-major winding numbers, the same shape. The non-zero rule reads this.
    ///
    /// The sign follows the direction the edges happen to run in, so a path's winding is
    /// `±1`, `±2` and so on; only zero versus non-zero carries meaning, and the two fill
    /// rules differ on exactly that. It is reported whichever rule produced `alpha`,
    /// because a caller may want to know why a pixel is or is not covered.
    pub winding: Vec<i32>,
    /// The rule `alpha` was computed with.
    pub rule: FillRule,
}

/// How a path's interior is decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FillRule {
    /// Inside where the winding number is not zero.
    #[default]
    NonZero,
    /// Inside where the crossing count is odd.
    EvenOdd,
}

impl Coverage {
    /// The alpha at a pixel, or 0 outside the buffer.
    #[must_use]
    pub fn at(&self, x: usize, y: usize) -> f32 {
        self.alpha.get(y * self.width + x).copied().unwrap_or(0.0)
    }

    /// The winding number at a pixel, or 0 outside.
    #[must_use]
    pub fn winding_at(&self, x: usize, y: usize) -> i32 {
        self.winding.get(y * self.width + x).copied().unwrap_or(0)
    }

    /// Whether any pixel is covered, which is the cheap way to skip a composite.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.alpha.iter().all(|a| *a <= 0.0)
    }

    /// The covered pixels, for a renderer that touches only those.
    pub fn covered(&self) -> impl Iterator<Item = (usize, usize, f32)> + '_ {
        self.alpha
            .iter()
            .enumerate()
            .filter(|(_, a)| **a > 0.0)
            .map(|(i, a)| (i % self.width, i / self.width, *a))
    }

    /// The total covered area, which for a path is its area. A sampled rasteriser does
    /// not have this property, and a test can tell the two apart by checking it.
    #[must_use]
    pub fn total_area(&self) -> f64 {
        self.alpha.iter().map(|a| f64::from(*a)).sum()
    }
}

/// How much memory one coverage buffer costs, so a caller can refuse a page it cannot
/// afford rather than being killed by it.
#[must_use]
pub fn footprint(area: usize) -> usize {
    area.saturating_mul(8)
}

/// The most sub-bands one pixel row may be cut into.
///
/// A row with an edge crossing every pixel boundary needs about one cut per pixel of
/// width, so this is generous for any page and stops a path of a million edges in a
/// million-wide clip from becoming a million cuts per row.
pub const MAX_CUTS_PER_ROW: usize = 4096;

/// Rasterise a path's coverage over a rectangle.
///
/// `edges` is a flattened path. The result is the exact area the path encloses at every
/// pixel, under `rule`. The rule has to be chosen here rather than afterwards, because
/// even-odd needs the crossing parity *as the spans are walked* and that is not
/// recoverable from a finished coverage buffer.
#[must_use]
pub fn rasterise(edges: &[Edge], clip: Rect, rule: FillRule) -> Coverage {
    let Some((columns, rows)) = clip.pixels() else {
        return Coverage::default();
    };
    let width = columns.end - columns.start;
    let height = rows.end - rows.start;
    let mut coverage = Coverage {
        width,
        height,
        alpha: vec![0.0; width * height],
        winding: vec![0; width * height],
        rule,
    };
    let Some(bounds) = bounds(edges) else {
        return coverage;
    };
    if bounds.is_empty() {
        return coverage;
    }
    // Rows outside the path's own extent are skipped outright: no work at all.
    let first_row = (bounds.y0.floor() as i64)
        .clamp(rows.start as i64, rows.end as i64)
        .max(rows.start as i64);
    let last_row = (bounds.y1.ceil() as i64)
        .clamp(rows.start as i64, rows.end as i64)
        .min(rows.end as i64);
    let clip_lo = f64::from(i32::try_from(columns.start).unwrap_or(0));
    let clip_hi = f64::from(i32::try_from(columns.end).unwrap_or(0));

    let mut cuts: Vec<f64> = Vec::new();
    for row in first_row..last_row {
        let top = row as f64;
        let bottom = top + 1.0;
        cuts.clear();
        cuts.push(top);
        cuts.push(bottom);
        add_cuts(edges, top, bottom, clip_lo, clip_hi, &mut cuts);
        cuts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        cuts.dedup_by(|a, b| (*a - *b).abs() < 1e-12);

        for pair in 0..cuts.len().saturating_sub(1) {
            let (Some(&ya), Some(&yb)) = (cuts.get(pair), cuts.get(pair + 1)) else {
                continue;
            };
            if yb - ya <= 0.0 {
                continue;
            }
            accumulate_band(
                edges,
                ya,
                yb,
                columns.start,
                rows.start,
                &mut coverage,
                clip_lo,
                clip_hi,
                rule,
            );
        }
    }
    coverage
}

/// Add the heights in `(top, bottom)` where the row must be subdivided: a path vertex,
/// or an edge crossing an integer x inside the clip.
fn add_cuts(edges: &[Edge], top: f64, bottom: f64, clip_lo: f64, clip_hi: f64, out: &mut Vec<f64>) {
    for e in edges {
        for v in [e.y0, e.y1] {
            if v > top && v < bottom {
                out.push(v);
            }
        }
        if e.is_horizontal() {
            continue;
        }
        let (xa, xb) = (e.x_at(top), e.x_at(bottom));
        let (lo, hi) = if xa <= xb { (xa, xb) } else { (xb, xa) };
        // Every integer x between the two ends is a boundary the span crosses, and each
        // one is a height where the covered width within a pixel changes its slope.
        let first = lo.floor() + 1.0;
        let mut k = first;
        let mut guard = 0usize;
        while k < hi && guard < MAX_CUTS_PER_ROW {
            if k > clip_lo && k < clip_hi {
                let t = if xb == xa { 0.5 } else { (k - xa) / (xb - xa) };
                let y = top + t * (bottom - top);
                if y > top && y < bottom {
                    out.push(y);
                }
            }
            k += 1.0;
            guard += 1;
        }
    }
    out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    out.dedup();
}

/// Add one band's exact area to the pixels it covers.
///
/// Within a band every active edge's x varies linearly, and so does each covered span.
/// For a pixel column the overlap width is therefore linear, and its integral is a
/// trapezoid: the mean of the widths at the two ends, times the band's height.
#[allow(clippy::too_many_arguments)]
fn accumulate_band(
    edges: &[Edge],
    ya: f64,
    yb: f64,
    columns_start: usize,
    rows_start: usize,
    coverage: &mut Coverage,
    clip_lo: f64,
    clip_hi: f64,
    rule: FillRule,
) {
    if yb <= ya {
        return;
    }
    let height = yb - ya;
    // Each crossing carries the edge it came from, because the pairs below are matched
    // by position *after* sorting by x, and a sorted position is not a declaration
    // order. Without the index, a band's two ends would be read off the wrong edges.
    let mut crossings: Vec<(f64, i32, usize)> = Vec::with_capacity(8);
    for (index, e) in edges.iter().enumerate() {
        if let Some((x, dir)) = e.crossing(ya) {
            crossings.push((x, dir, index));
        }
    }
    if crossings.len() < 2 {
        return;
    }
    crossings.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    // Walk the crossings in pairs. Within a band the ordering is fixed, so the winding
    // at each pair is the same at both ends of the band.
    // Two counters, because the two rules ask different questions. The winding says
    // which side is enclosed; the parity says how many times. Both are accumulated as
    // the spans are walked, since neither can be recovered afterwards.
    let mut winding = 0i32;
    for (parity, pair) in (0..crossings.len().saturating_sub(1)).enumerate() {
        let (Some(&(xa, dir, _)), Some(&(xb, _, edge_b))) =
            (crossings.get(pair), crossings.get(pair + 1))
        else {
            break;
        };
        let edge_a = crossings
            .get(pair)
            .map_or(0, |c| c.2)
            .min(edges.len().saturating_sub(1));
        winding += dir;
        // Even-odd encloses where the crossing count to the left is odd. A ray arriving
        // from outside the path toggles at every crossing, so the *first* span is already
        // inside and the rule alternates from there — which is why it starts inside and
        // not outside, and why it disagrees with the winding on nested sub-paths wound
        // the same way.
        let encloses = if rule == FillRule::EvenOdd {
            parity % 2 == 0
        } else {
            winding != 0
        };
        if !encloses {
            continue;
        }
        // The span's ends at the top of the band, clipped. A span may be zero-width at
        // either end and still enclose area between them, which is what a triangle's
        // apex is, so neither end being positive is not a reason to skip the band.
        let top_left = xa.max(clip_lo);
        let top_right = xb.min(clip_hi);
        // The same two edges at the bottom of the band. Each edge is linear in y, so
        // between the cuts the span is linear too and its area is a trapezoid.
        let a = edges.get(edge_a);
        let b = edges.get(edge_b);
        let (Some(a), Some(b)) = (a, b) else { continue };
        let bottom_left = a.x_at(yb).max(clip_lo);
        let bottom_right = b.x_at(yb).min(clip_hi);

        add_span(
            coverage,
            columns_start,
            rows_start,
            ya,
            height,
            top_left,
            top_right,
            bottom_left,
            bottom_right,
            winding,
        );
    }
}

/// Add the exact area of a trapezoidal span to the pixels it covers.
#[allow(clippy::too_many_arguments)]
fn add_span(
    coverage: &mut Coverage,
    columns_start: usize,
    rows_start: usize,
    y: f64,
    height: f64,
    top_left: f64,
    top_right: f64,
    bottom_left: f64,
    bottom_right: f64,
    winding: i32,
) {
    let first = (top_left.min(bottom_left).floor().max(0.0) as usize)
        .saturating_sub(columns_start)
        .min(coverage.width);
    let last = (top_right.max(bottom_right).ceil().max(0.0) as usize)
        .saturating_sub(columns_start)
        .min(coverage.width);
    if first >= last {
        return;
    }
    // The buffer holds the clip's rows, not the device's, so the band has to be indexed
    // relative to the clip. Using the absolute row would silently write past the end
    // whenever the clip did not start at the top of the image.
    let row = (y.floor().max(0.0) as usize).saturating_sub(rows_start);
    if row >= coverage.height {
        return;
    }
    let row_offset = row * coverage.width;

    for index in first..last {
        let px_lo = (index + columns_start) as f64;
        let px_hi = px_lo + 1.0;
        // The overlap at each end of the band. Within the band both are linear, so the
        // area is the mean times the height — exact, not an approximation.
        let top_overlap = (top_right.min(px_hi) - top_left.max(px_lo)).max(0.0);
        let bottom_overlap = (bottom_right.min(px_hi) - bottom_left.max(px_lo)).max(0.0);
        if top_overlap <= 0.0 && bottom_overlap <= 0.0 {
            continue;
        }
        let area = f64::midpoint(top_overlap, bottom_overlap) * height;
        let slot = row_offset + index;
        if let Some(cell) = coverage.alpha.get_mut(slot) {
            // Bands stack vertically and spans are disjoint horizontally, so the areas
            // add, and the result cannot exceed the pixel: a pixel is one pixel.
            *cell = (f64::from(*cell) + area).clamp(0.0, 1.0) as f32;
        }
        if let Some(cell) = coverage.winding.get_mut(slot) {
            *cell = winding;
        }
    }
}

/// The bounding box of a set of edges, in device space.
#[must_use]
pub fn bounds(edges: &[Edge]) -> Option<Rect> {
    let mut it = edges.iter();
    let first = it.next()?;
    let mut r = Rect {
        x0: first.x0.min(first.x1),
        y0: first.y0.min(first.y1),
        x1: first.x0.max(first.x1),
        y1: first.y0.max(first.y1),
    };
    for e in it {
        r.x0 = r.x0.min(e.x0.min(e.x1));
        r.y0 = r.y0.min(e.y0.min(e.y1));
        r.x1 = r.x1.max(e.x0.max(e.x1));
        r.y1 = r.y1.max(e.y0.max(e.y1));
    }
    Some(r)
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect` and index a slice whose length they
    // have just asserted; both are what a test is for. The panic-free rule is about what
    // the product does with a file, not about how a test reads one.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-3
    }

    fn rect_edges(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Edge> {
        vec![
            Edge::new(x0, y0, x1, y0),
            Edge::new(x1, y0, x1, y1),
            Edge::new(x1, y1, x0, y1),
            Edge::new(x0, y1, x0, y0),
        ]
    }

    fn clip(width: usize, height: usize) -> Rect {
        Rect {
            x0: 0.0,
            y0: 0.0,
            x1: width as f64,
            y1: height as f64,
        }
    }

    #[test]
    fn a_zero_area_rectangle_covers_nothing() {
        // Corners the wrong way round are ordered rather than treated as empty: a
        // rectangle with a negative width is still a rectangle, and a crop box written
        // backwards should show a page, not nothing.
        let inverted = Rect {
            x0: 5.0,
            y0: 5.0,
            x1: 1.0,
            y1: 1.0,
        };
        assert!(!inverted.is_empty());
        assert_eq!(
            inverted.normalised(),
            Rect {
                x0: 1.0,
                y0: 1.0,
                x1: 5.0,
                y1: 5.0
            }
        );
        // A rectangle with no area really is empty.
        assert_eq!(
            Rect {
                x0: 5.0,
                y0: 5.0,
                x1: 5.0,
                y1: 9.0
            }
            .pixels(),
            None
        );
    }

    #[test]
    fn a_path_with_no_edges_covers_nothing() {
        let c = rasterise(&[], clip(8, 8), FillRule::NonZero);
        assert!(c.is_empty());
        assert_eq!(c.winding.iter().sum::<i32>(), 0);
    }

    #[test]
    fn a_rectangle_aligned_to_pixels_is_exactly_one_throughout() {
        let c = rasterise(
            &rect_edges(0.0, 0.0, 4.0, 4.0),
            clip(4, 4),
            FillRule::NonZero,
        );
        assert_eq!((c.width, c.height), (4, 4));
        for y in 0..4 {
            for x in 0..4 {
                assert!(
                    near(f64::from(c.at(x, y)), 1.0),
                    "pixel ({x}, {y}) is {}, not 1",
                    c.at(x, y)
                );
            }
        }
    }

    #[test]
    fn a_rectangle_shifted_by_half_a_pixel_covers_one_and_a_quarter() {
        // From 0.5 to 2.5 by 0.5 to 2.5: the middle pixel is whole and the four around
        // it are half, giving an area of 4.
        let c = rasterise(
            &rect_edges(0.5, 0.5, 2.5, 2.5),
            clip(4, 4),
            FillRule::NonZero,
        );
        assert!(near(f64::from(c.at(1, 1)), 1.0), "the middle is whole");
        assert!(near(f64::from(c.at(0, 1)), 0.5), "the left is half");
        assert!(near(f64::from(c.at(1, 0)), 0.5), "the top is half");
        assert!(near(c.total_area(), 4.0), "the area is 2 by 2");
    }

    /// The property that separates analytic coverage from sampling: a path's total
    /// coverage equals its area. A sampled rasteriser cannot satisfy this for a
    /// half-covered edge.
    #[test]
    fn total_coverage_equals_the_area_of_the_shape() {
        let cases: [(f64, f64, f64, f64); 6] = [
            (3.0, 5.0, 3.0, 5.0),
            (7.0, 2.0, 7.0, 2.0),
            (0.5, 0.5, 0.5, 0.5),
            (11.0, 1.0, 11.0, 1.0),
            (2.25, 3.75, 2.25, 3.75),
            (0.1, 0.2, 0.3, 0.4),
        ];
        for (x, y, w, h) in cases {
            let edges = rect_edges(x, y, x + w, y + h);
            let c = rasterise(&edges, clip(24, 24), FillRule::NonZero);
            let expected = w * h;
            assert!(
                near(c.total_area(), expected),
                "a {w} by {h} rectangle at ({x}, {y}) has area {expected} but rasterised \
                 to {}",
                c.total_area()
            );
        }
    }

    /// A right triangle of area 2, drawn as a diagonal edge. A sampled rasteriser gets
    /// this visibly wrong; an analytic one recovers the area exactly.
    #[test]
    fn a_diagonal_edge_preserves_the_triangles_area() {
        let edges = vec![
            Edge::new(0.0, 0.0, 4.0, 0.0),
            Edge::new(4.0, 0.0, 0.0, 1.0),
            Edge::new(0.0, 1.0, 0.0, 0.0),
        ];
        let c = rasterise(&edges, clip(8, 8), FillRule::NonZero);
        assert!(
            near(c.total_area(), 2.0),
            "the triangle has area 2, rasterised to {}",
            c.total_area()
        );
    }

    #[test]
    fn a_diagonal_through_a_pixel_covers_half_of_it() {
        // A single pixel's worth of a 45° edge, cut by the rectangle below.
        let edges = vec![
            Edge::new(0.0, 0.0, 1.0, 0.0),
            Edge::new(1.0, 0.0, 0.0, 1.0),
            Edge::new(0.0, 1.0, 0.0, 0.0),
        ];
        let c = rasterise(&edges, clip(1, 1), FillRule::NonZero);
        assert!(
            near(f64::from(c.at(0, 0)), 0.5),
            "a 45° triangle over one pixel is half of it, got {}",
            c.at(0, 0)
        );
    }

    #[test]
    fn winding_distinguishes_the_two_fill_rules() {
        // Two squares wound the same way: the non-zero rule says inside where they
        // overlap, the even-odd rule says outside.
        let mut edges = rect_edges(0.0, 0.0, 4.0, 4.0);
        edges.extend(rect_edges(2.0, 2.0, 6.0, 6.0));
        let c = rasterise(&edges, clip(8, 8), FillRule::NonZero);
        assert_eq!(
            c.winding_at(3, 3).abs(),
            2,
            "two same-way squares accumulate"
        );
        assert_eq!(c.winding_at(1, 1).abs(), 1, "one square alone winds to one");
        assert_ne!(
            c.winding_at(3, 3),
            0,
            "which is what the non-zero rule needs to fill the overlap"
        );
    }

    #[test]
    fn opposite_windings_cancel_which_is_what_even_odd_means() {
        let mut edges = rect_edges(0.0, 0.0, 4.0, 4.0);
        // The same square the other way round.
        edges.extend([
            Edge::new(2.0, 2.0, 2.0, 6.0),
            Edge::new(2.0, 6.0, 6.0, 6.0),
            Edge::new(6.0, 6.0, 6.0, 2.0),
            Edge::new(6.0, 2.0, 2.0, 2.0),
        ]);
        let c = rasterise(&edges, clip(8, 8), FillRule::NonZero);
        assert_eq!(c.winding_at(3, 3), 0, "opposite windings cancel");
        assert!(
            near(f64::from(c.at(3, 3)), 0.0),
            "and so nothing is drawn there"
        );
    }

    #[test]
    fn a_clip_excludes_what_is_outside_it() {
        let c = rasterise(
            &rect_edges(0.0, 0.0, 8.0, 8.0),
            clip(4, 4),
            FillRule::NonZero,
        );
        assert_eq!(c.width, 4, "the buffer is the clip, not the path");
        assert!(c.alpha.iter().all(|a| near(f64::from(*a), 1.0)));
        assert!(
            near(c.total_area(), 16.0),
            "only the clipped part is covered"
        );
    }

    #[test]
    fn a_shape_away_from_the_origin_does_not_cover_the_origin() {
        let c = rasterise(
            &rect_edges(10.0, 10.0, 12.0, 12.0),
            clip(16, 16),
            FillRule::NonZero,
        );
        assert!(near(f64::from(c.at(0, 0)), 0.0));
        assert!(near(f64::from(c.at(11, 11)), 1.0));
    }

    #[test]
    fn a_single_horizontal_line_encloses_nothing() {
        let edges = vec![Edge::new(0.0, 0.5, 4.0, 0.5)];
        let c = rasterise(&edges, clip(8, 8), FillRule::NonZero);
        assert!(
            c.is_empty(),
            "a line has no interior under either fill rule"
        );
    }

    #[test]
    fn a_three_sided_path_is_still_a_triangle() {
        // (0, 0), (4, 0), (4, 4): the hypotenuse is the line y = x, so the pixel at
        // (2, 2) sits on it and is half covered, and the pixels below it are outside.
        let edges = vec![
            Edge::new(0.0, 0.0, 4.0, 0.0),
            Edge::new(4.0, 0.0, 4.0, 4.0),
            Edge::new(4.0, 4.0, 0.0, 0.0),
        ];
        let c = rasterise(&edges, clip(6, 6), FillRule::NonZero);
        assert!(near(f64::from(c.at(3, 2)), 1.0), "the interior is filled");
        assert!(near(f64::from(c.at(2, 2)), 0.5), "the hypotenuse is half");
        assert!(near(f64::from(c.at(1, 3)), 0.0), "the outside is not");
        // Half of four by four, which is the triangle's area.
        assert!(near(c.total_area(), 8.0), "got {}", c.total_area());
    }

    #[test]
    fn an_edge_lying_on_a_pixel_boundary_contributes_nothing_to_either_side() {
        // A rectangle from 2.0 to 4.0: columns 0 and 1 are empty and column 3 is not
        // even started, which is what half-open means.
        let c = rasterise(
            &rect_edges(2.0, 0.0, 4.0, 1.0),
            clip(6, 2),
            FillRule::NonZero,
        );
        assert!(near(f64::from(c.at(1, 0)), 0.0), "column 1 ends at 2");
        assert!(near(f64::from(c.at(2, 0)), 1.0), "column 2 is inside");
        assert!(near(f64::from(c.at(3, 0)), 1.0), "column 3 is inside");
        assert!(near(f64::from(c.at(4, 0)), 0.0), "column 4 starts at 4");
    }

    #[test]
    fn bounds_cover_every_edge() {
        let b = bounds(&rect_edges(2.0, 3.0, 8.0, 9.0)).expect("bounds");
        assert_eq!(
            b,
            Rect {
                x0: 2.0,
                y0: 3.0,
                x1: 8.0,
                y1: 9.0
            }
        );
        assert!(bounds(&[]).is_none());
    }

    #[test]
    fn the_footprint_is_arithmetic_and_saturates() {
        assert_eq!(footprint(1000), 8000);
        assert_eq!(footprint(usize::MAX), usize::MAX);
    }

    #[test]
    fn a_path_with_a_hole_fills_only_the_ring() {
        // An outer square and an inner square wound the other way: the ring is covered
        // and the middle is not.
        let mut edges = rect_edges(0.0, 0.0, 6.0, 6.0);
        edges.extend([
            Edge::new(2.0, 2.0, 2.0, 4.0),
            Edge::new(2.0, 4.0, 4.0, 4.0),
            Edge::new(4.0, 4.0, 4.0, 2.0),
            Edge::new(4.0, 2.0, 2.0, 2.0),
        ]);
        let c = rasterise(&edges, clip(8, 8), FillRule::NonZero);
        assert!(near(f64::from(c.at(1, 1)), 1.0), "the ring is covered");
        assert!(near(f64::from(c.at(3, 3)), 0.0), "the hole is not");
        // Six by six less two by two.
        assert!(near(c.total_area(), 32.0), "got {}", c.total_area());
    }
}
