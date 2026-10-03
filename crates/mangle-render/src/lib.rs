//! The rasterizer.
//!
//! Everything here turns geometry into pixels. The order is deliberate and is the only
//! order that works:
//!
//! 1. **Flatten.** A path is curves and lines; a rasteriser wants lines. Curves become
//!    as many line segments as the transform makes necessary, and the count is derived
//!    from how far the curve bulges in device space rather than fixed, so a curve that is
//!    nearly straight costs almost nothing.
//! 2. **Stroke, if stroking.** A stroke is an outline, and building it means caps, joins,
//!    dashes and the miter limit. It is done on the polygon's own device-space points with a
//!    width in the same space, so the two are scaled by the same factors and a stroke thickens
//!    when the page is drawn larger.
//! 3. **Rasterise coverage**, analytically, so a fill is exact rather than sampled.
//! 4. **Clip**, intersect the coverage with the clip rather than testing it per pixel.
//! 5. **Composite**, which is where colour, alpha and blending happen.
//!
//! Analytic coverage is the reason this file is careful about edge cases. A sampled
//! rasteriser gets a 45° edge wrong by a whole pixel; this one gets it right, which is
//! the difference between a document that looks the way it was authored and one that
//! does not.

#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    clippy::indexing_slicing
)]
#![warn(missing_debug_implementations)]
// Path points, matrix components and Bézier control points are named the way the
// specification and every textbook name them, so that a line here can be compared with
// the page of ISO 32000-1 it came from.
//
// Coordinates are also compared for exact equality, in both of the places that ask
// whether two of the *same* numbers are the same number: whether a subpath's ends meet,
// and whether a dash pattern's runs do. Both have exact answers, and a tolerance would
// answer "nearly" to a question that does not need it.
#![allow(clippy::many_single_char_names, clippy::float_cmp)]

pub mod compare;
pub mod coverage;
pub mod fill;
pub mod image;
pub mod page;
pub mod shading;

pub use compare::{Comparison, SsimOptions, compare, ssim};
pub use coverage::{Coverage, Edge, FillRule, Rect, bounds, footprint, rasterise};
pub use page::{MAX_SCALE, PageRender, Placement, RenderOptions, effective_scale, render_page};

use mangle_content::PathSegment;

/// An 8-bit-per-channel image with straight (non-premultiplied) alpha.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    /// Row-major RGBA, four bytes per pixel.
    pub pixels: Vec<u8>,
}

impl Image {
    /// A transparent image.
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; width * height * 4],
        }
    }

    /// The four bytes at a pixel, or `None` outside the image.
    #[must_use]
    pub fn get(&self, x: usize, y: usize) -> Option<[u8; 4]> {
        let i = (y * self.width + x) * 4;
        let [r, g, b, a] = self.pixels.get(i..i + 4)? else {
            return None;
        };
        Some([*r, *g, *b, *a])
    }

    /// Set a pixel, ignoring a position outside the image. Writing outside is a bug in
    /// the caller, and silently dropping it would hide it.
    pub fn put(&mut self, x: usize, y: usize, rgba: [u8; 4]) {
        if x >= self.width || y >= self.height {
            return;
        }
        let i = (y * self.width + x) * 4;
        if let Some(slice) = self.pixels.get_mut(i..i + 4) {
            slice.copy_from_slice(&rgba);
        }
    }

    /// The image's rectangle, in pixels.
    #[must_use]
    pub fn rect(&self) -> Rect {
        Rect {
            x0: 0.0,
            y0: 0.0,
            x1: self.width as f64,
            y1: self.height as f64,
        }
    }

    /// A white image of a given size, which is what a page starts as.
    #[must_use]
    pub fn filled(width: usize, height: usize, rgba: [u8; 4]) -> Self {
        let mut image = Self::new(width, height);
        for y in 0..height {
            for x in 0..width {
                image.put(x, y, rgba);
            }
        }
        image
    }
}

/// How much a curve may be flattened before it is rasterised.
///
/// The tolerance is in device pixels: a quarter of a pixel is below what any display
/// can show, and above the noise floor of the coverage arithmetic.
pub const FLATTEN_TOLERANCE: f64 = 0.25;

/// The most segments one curve may become, whatever the transform asks for.
pub const MAX_CURVE_SEGMENTS: usize = 256;

/// A curve to a polyline, in device space.
///
/// The segment count comes from how far the curve bulges from its chord, which is what
/// decides the error, divided by the tolerance. A curve drawn at 4% scale therefore
/// needs almost no segments, and one drawn at 400% needs more.
#[must_use]
pub fn flatten_curve(
    p0: (f64, f64),
    p1: (f64, f64),
    p2: (f64, f64),
    p3: (f64, f64),
) -> Vec<(f64, f64)> {
    // How far the control points sit from the straight line joining the endpoints. That
    // is the bulge, and it is what the error of a chord approximation is bounded by. A
    // curve whose control points are on the chord is straight, and needs no subdivision.
    let (dx, dy) = (p3.0 - p0.0, p3.1 - p0.1);
    let chord = (dx * dx + dy * dy).sqrt();
    if chord <= f64::EPSILON {
        // The endpoints coincide, so the curve is a loop; the control points bound it.
        let span = [(p1.0 - p0.0).abs(), (p1.1 - p0.1).abs()]
            .into_iter()
            .chain([(p2.0 - p0.0).abs(), (p2.1 - p0.1).abs()])
            .fold(0.0f64, f64::max);
        let count =
            ((span / FLATTEN_TOLERANCE).sqrt().ceil() as usize).clamp(2, MAX_CURVE_SEGMENTS);
        return (1..=count)
            .map(|step| cubic_at(p0, p1, p2, p3, step as f64 / count as f64))
            .collect();
    }
    let perpendicular = |p: (f64, f64)| ((p.0 - p0.0) * dy - (p.1 - p0.1) * dx).abs() / chord;
    let deviation = perpendicular(p1).max(perpendicular(p2));

    // Straight lines need only the endpoint; anything else needs enough segments that
    // the chord error stays inside the tolerance.
    let count = if deviation <= f64::EPSILON {
        2
    } else {
        let n = (deviation / FLATTEN_TOLERANCE).sqrt().ceil();
        if n.is_finite() && n > 1.0 {
            (n as usize).clamp(2, MAX_CURVE_SEGMENTS)
        } else {
            2
        }
    };

    let mut out = Vec::with_capacity(count);
    for step in 1..=count {
        let t = step as f64 / count as f64;
        out.push(cubic_at(p0, p1, p2, p3, t));
    }
    out
}

/// A point on a cubic Bézier at `t`.
#[must_use]
pub fn cubic_at(
    p0: (f64, f64),
    p1: (f64, f64),
    p2: (f64, f64),
    p3: (f64, f64),
    t: f64,
) -> (f64, f64) {
    let u = 1.0 - t;
    let (a, b) = (u * u * u, 3.0 * u * u * t);
    let (c, d) = (3.0 * u * t * t, t * t * t);
    (
        a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0,
        a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1,
    )
}

/// A flattened path: subpaths of points, in device space.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Polygon {
    /// Each subpath's points, without a repeated closing point.
    pub subpaths: Vec<Vec<(f64, f64)>>,
}

impl Polygon {
    /// Is there anything here?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.subpaths.iter().all(|s| s.len() < 2)
    }

    /// The edges, with the winding each contributes.
    #[must_use]
    pub fn edges(&self) -> Vec<Edge> {
        let mut out = Vec::new();
        for sub in &self.subpaths {
            if sub.len() < 2 {
                continue;
            }
            for pair in sub.windows(2) {
                let (Some(a), Some(b)) = (pair.first(), pair.get(1)) else {
                    continue;
                };
                out.push(Edge::new(a.0, a.1, b.0, b.1));
            }
            // A subpath that was not explicitly closed is closed implicitly, which is
            // what the fill operators do and what a fill rule depends on.
            if let (Some(first), Some(last)) = (sub.first(), sub.last())
                && (first.0 != last.0 || first.1 != last.1)
            {
                out.push(Edge::new(last.0, last.1, first.0, first.1));
            }
        }
        out
    }

    /// The bounding box.
    #[must_use]
    pub fn bounds(&self) -> Option<Rect> {
        bounds(&self.edges())
    }

    /// The device-space area, computed by the shoelace formula over every subpath.
    ///
    /// This is the *signed* area: a subpath wound the other way subtracts, which is what
    /// makes the non-zero rule fall out of the arithmetic rather than being a special
    /// case.
    #[must_use]
    pub fn signed_area(&self) -> f64 {
        self.subpaths
            .iter()
            .filter(|s| s.len() >= 3)
            .map(|sub| {
                let mut total = 0.0;
                for pair in sub.windows(2) {
                    if let (Some(a), Some(b)) = (pair.first(), pair.get(1)) {
                        total += a.0 * b.1 - b.0 * a.1;
                    }
                }
                // Close the loop.
                if let (Some(first), Some(last)) = (sub.first(), sub.last()) {
                    total += last.0 * first.1 - first.0 * last.1;
                }
                total * 0.5
            })
            .sum()
    }

    /// The winding number at a point, by the even-odd or non-zero rule.
    ///
    /// The standard crossing test: shoot a ray to the right and count what it crosses,
    /// treating a vertex as belonging to the edge that goes down from it so that a
    /// vertex is counted exactly once.
    #[must_use]
    pub fn contains(&self, x: f64, y: f64, rule: FillRule) -> bool {
        let mut winding = 0i32;
        let mut crossings = 0u32;
        for sub in &self.subpaths {
            // Each subpath is closed implicitly: the segment from the last point back to
            // the first is what makes a square a square rather than three sides of one.
            let closing = sub.last().zip(sub.first()).map(|(&a, &b)| (a, b));
            let segments = sub
                .windows(2)
                .filter_map(|w| Some((*w.first()?, *w.get(1)?)))
                .chain(closing);
            for ((x0, y0), (x1, y1)) in segments {
                // The half-open rule: an edge counts when it starts above the ray's
                // height and ends at or below it, or the reverse.
                let (lo, hi, dir) = if y0 <= y1 {
                    (y0, y1, 1i32)
                } else {
                    (y1, y0, -1i32)
                };
                if y < lo || y >= hi {
                    continue;
                }
                let t = (y - y0) / (y1 - y0);
                let at = x0 + t * (x1 - x0);
                if at > x {
                    winding += dir;
                    crossings += 1;
                }
            }
        }
        match rule {
            FillRule::NonZero => winding != 0,
            FillRule::EvenOdd => crossings % 2 == 1,
        }
    }
}

/// Transform a content-stream path into device-space polylines.
///
/// Curves are flattened here, with the control points transformed first so the segment
/// count reflects the device-space error rather than the user-space one.
#[must_use]
pub fn transform_path(segments: &[PathSegment], to_device: &mangle_content::Matrix) -> Polygon {
    let mut subpaths: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut current: Vec<(f64, f64)> = Vec::new();
    let mut cursor = (0.0, 0.0);

    for segment in segments {
        match *segment {
            PathSegment::Move(x, y) => {
                if current.len() > 1 {
                    subpaths.push(std::mem::take(&mut current));
                } else {
                    current.clear();
                }
                cursor = to_device.apply(x, y);
                current.push(cursor);
            }
            PathSegment::Line(x, y) => {
                cursor = to_device.apply(x, y);
                current.push(cursor);
            }
            PathSegment::Curve(x1, y1, x2, y2, x3, y3) => {
                let c1 = to_device.apply(x1, y1);
                let c2 = to_device.apply(x2, y2);
                let end = to_device.apply(x3, y3);
                current.extend(flatten_curve(cursor, c1, c2, end));
                cursor = end;
            }
            PathSegment::Close => {
                if let Some(first) = current.first().copied() {
                    current.push(first);
                }
                if current.len() > 1 {
                    subpaths.push(std::mem::take(&mut current));
                } else {
                    current.clear();
                }
                cursor = first_of(&subpaths);
            }
        }
    }
    if current.len() > 1 {
        subpaths.push(current);
    }
    Polygon { subpaths }
}

fn first_of(subpaths: &[Vec<(f64, f64)>]) -> (f64, f64) {
    subpaths
        .last()
        .and_then(|s| s.first().copied())
        .unwrap_or((0.0, 0.0))
}

/// The dash pattern's on and off lengths, with the dash array expanded to the pairs the
/// walk needs.
#[must_use]
pub fn dash_pairs(dash: &mangle_content::Dash) -> Vec<(f64, f64)> {
    let mut out = Vec::with_capacity(dash.array.len() / 2);
    // A dash array is an alternating list, so an odd length leaves a final element with
    // no partner. It is dropped rather than paired with a zero, which would introduce a
    // dash of no length where the file asked for none.
    for pair in dash.array.chunks_exact(2) {
        let (Some(on), Some(off)) = (pair.first(), pair.get(1)) else {
            break;
        };
        out.push((on.max(0.0), off.max(0.0)));
    }
    out
}

/// Walk a polyline, calling `emit` for each on-dash and off-dash run in turn.
///
/// The phase is consumed first, which is what the specification's `/Phase` means, and a
/// pattern whose total length is zero is solid rather than an infinite loop.
pub fn walk_dashes(
    points: &[(f64, f64)],
    pairs: &[(f64, f64)],
    phase: f64,
    mut emit: impl FnMut(bool, &[(f64, f64)]),
) {
    if points.len() < 2 {
        return;
    }
    if pairs.is_empty() || pairs.iter().all(|(on, _)| *on <= 0.0) {
        emit(true, points);
        return;
    }
    let total: f64 = pairs.iter().map(|(on, off)| on + off).sum();
    if total <= 0.0 {
        emit(true, points);
        return;
    }

    // The pattern is walked as a pair of cursors: which element is current, and whether
    // that element is an on-length or an off-length. A cursor never leaves the array,
    // so there is no index arithmetic to get wrong.
    let mut cursor = Cursor::at(pairs, 0, true);
    let mut remaining = phase.rem_euclid(total);
    while remaining > 0.0 {
        let length = cursor.length();
        if remaining < length {
            break;
        }
        remaining -= length;
        cursor = cursor.advance();
    }

    let mut run: Vec<(f64, f64)> = Vec::new();
    let mut budget = cursor.length() - remaining;
    // Where the current run begins. A run that starts partway through a segment must
    // begin there, or every run after the first would be measured from the segment's
    // own start and the dashes would drift.
    let mut run_start = points.first().copied().unwrap_or((0.0, 0.0));

    for window in points.windows(2) {
        let (Some(a), Some(b)) = (window.first(), window.get(1)) else {
            continue;
        };
        let (tx, ty) = (b.0 - a.0, b.1 - a.1);
        let segment = (tx * tx + ty * ty).sqrt();
        if segment <= 0.0 {
            continue;
        }
        let mut travelled = 0.0;
        while travelled < segment {
            let step = budget.min(segment - travelled);
            travelled += step;
            budget -= step;
            let t = travelled / segment;
            let x = a.0 + tx * t;
            let y = a.1 + ty * t;
            if run.is_empty() {
                run.push(run_start);
            }
            run.push((x, y));
            if budget <= f64::EPSILON {
                if !run.is_empty() {
                    emit(cursor.on, &run);
                }
                run.clear();
                cursor = cursor.advance();
                budget = cursor.length();
                // A zero-length element would spin forever; treat it as a hairline.
                if budget <= 0.0 {
                    budget = f64::MIN_POSITIVE;
                }
                run_start = (x, y);
            }
        }
    }
    if !run.is_empty() {
        emit(cursor.on, &run);
    }
}

/// Where a dash walk currently is: which pair element, and whether it is the on-length
/// or the off-length.
#[derive(Debug, Clone, Copy)]
struct Cursor<'a> {
    pairs: &'a [(f64, f64)],
    index: usize,
    on: bool,
}

impl<'a> Cursor<'a> {
    fn at(pairs: &'a [(f64, f64)], index: usize, on: bool) -> Self {
        Self { pairs, index, on }
    }

    /// The current length. An index past the end can only be reached by arithmetic that
    /// has already been checked, so the zero is a belt rather than a brace.
    fn length(self) -> f64 {
        let Some(pair) = self.pairs.get(self.index) else {
            return 0.0;
        };
        if self.on { pair.0 } else { pair.1 }
    }

    /// The next element: the other half of this pair, then the next pair, alternating.
    fn advance(self) -> Self {
        let (index, on) = if self.on {
            (self.index, false)
        } else {
            ((self.index + 1) % self.pairs.len().max(1), true)
        };
        Self::at(self.pairs, index, on)
    }
}

/// How a stroke ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineCap {
    #[default]
    Butt,
    Round,
    Square,
}

/// How a stroke turns a corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineJoin {
    #[default]
    Miter,
    Round,
    Bevel,
}

/// Everything needed to turn a stroke width into an outline.
#[derive(Debug, Clone, PartialEq)]
pub struct StrokeStyle {
    pub width: f64,
    pub cap: LineCap,
    pub join: LineJoin,
    pub miter_limit: f64,
    pub dash: mangle_content::Dash,
}

impl Default for StrokeStyle {
    fn default() -> Self {
        Self {
            width: 1.0,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            miter_limit: 10.0,
            dash: mangle_content::Dash::default(),
        }
    }
}

/// Build the outline of a stroked polyline.
///
/// A stroke is the set of points within `width / 2` of the path, so the outline is the
/// path offset by that distance on both sides and joined at the corners. The offset is
/// computed per segment and the joins are added explicitly, which is how a stroked path
/// is built without needing a full offset-curve algorithm.
#[must_use]
pub fn stroke_outline(points: &[(f64, f64)], style: &StrokeStyle) -> Polygon {
    let half = (style.width / 2.0).abs();
    if half <= 0.0 || points.len() < 2 {
        return Polygon::default();
    }

    // Whether the path comes back to where it started. A closed path's stroke is a *ring*,
    // and a ring has an inner boundary: drawing it as one loop would enclose the middle
    // and fill it, which is the difference between a stroked rectangle and a solid one.
    let closed = points
        .first()
        .zip(points.last())
        .is_some_and(|(a, b)| a.0 == b.0 && a.1 == b.1)
        && points.len() > 2;
    // A closed path's last point repeats its first, and offsetting it again would close
    // the loop twice over.
    let path: &[(f64, f64)] = if closed {
        match points.get(..points.len().saturating_sub(1)) {
            Some(trimmed) => trimmed,
            None => return Polygon::default(),
        }
    } else {
        points
    };
    if path.len() < 2 {
        return Polygon::default();
    }

    let offsets = offset_sides(path, half, style);
    let mut subpaths = Vec::new();
    if closed {
        // The outer boundary and the inner one, wound opposite ways, so the non-zero rule
        // leaves the middle empty.
        let (left, right) = offsets;
        subpaths.push(loop_with_joins(&left, path, style, half, true));
        let mut inner = right;
        inner.reverse();
        subpaths.push(loop_with_joins(&inner, path, style, half, false));
    } else {
        let (left, right) = offsets;
        let mut outline = left;
        cap(
            &mut outline,
            path.last().copied().unwrap_or((0.0, 0.0)),
            normal_at(path, path.len() - 1, 1).unwrap_or((0.0, 1.0)),
            style.cap,
            half,
            true,
        );
        let mut back = right;
        back.reverse();
        outline.extend_from_slice(&back);
        cap(
            &mut outline,
            path.first().copied().unwrap_or((0.0, 0.0)),
            normal_at(path, 0, 0).unwrap_or((0.0, 1.0)),
            style.cap,
            half,
            false,
        );
        add_joins(&mut outline, path, style, half, false);
        subpaths.push(outline);
    }
    Polygon { subpaths }
}

/// The offset points on the two sides of a polyline.
type Sides = (Vec<(f64, f64)>, Vec<(f64, f64)>);

/// The offset points on each side of a polyline, with the join corrections applied.
///
/// Each point gets the *bisector* of its two adjacent segment normals, which is what turns
/// two offsets that would otherwise leave a notch into one that meets at the corner.
fn offset_sides(path: &[(f64, f64)], half: f64, style: &StrokeStyle) -> Sides {
    let mut left = Vec::with_capacity(path.len());
    let mut right = Vec::with_capacity(path.len());
    for i in 0..path.len() {
        let before = normal_at(path, i.wrapping_sub(1), 0);
        let after = normal_at(path, i, 1);
        let normal = match (before, after) {
            (Some(a), Some(b)) => bisector(a, b, half, style),
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => (0.0, 1.0),
        };
        let Some(p) = path.get(i) else {
            continue;
        };
        left.push((p.0 + normal.0 * half, p.1 + normal.1 * half));
        right.push((p.0 - normal.0 * half, p.1 - normal.1 * half));
    }
    (left, right)
}

/// The bisector of two segment normals, extended to the miter point where the miter limit
/// allows it and left as the bevel where it does not.
fn bisector(a: (f64, f64), b: (f64, f64), half: f64, style: &StrokeStyle) -> (f64, f64) {
    let direction = normalise((a.0 + b.0, a.1 + b.1));
    let dot = a.0 * b.0 + a.1 * b.1;
    let half_angle = ((1.0 + dot).max(0.0) / 2.0).sqrt();
    if half_angle <= f64::EPSILON || half_angle >= 0.998 {
        return a;
    }
    let miter_length = half / half_angle;
    if miter_length <= style.miter_limit.max(1.0) * half {
        (
            direction.0 * miter_length / half,
            direction.1 * miter_length / half,
        )
    } else {
        a
    }
}

/// A closed loop of offset points, which is what one side of a closed path's stroke is.
fn loop_with_joins(
    side: &[(f64, f64)],
    path: &[(f64, f64)],
    style: &StrokeStyle,
    half: f64,
    forward: bool,
) -> Vec<(f64, f64)> {
    let mut outline = side.to_vec();
    add_joins(&mut outline, path, style, half, forward);
    outline
}

/// Add the corner points a stroke outline needs where the offsets did not already meet.
///
/// The offsets are bisected, so they already land on the miter point; what is missing is
/// the case the miter limit refused, where the corner has to be bevelled explicitly or the
/// outline cuts the corner off.
fn add_joins(
    outline: &mut Vec<(f64, f64)>,
    path: &[(f64, f64)],
    style: &StrokeStyle,
    half: f64,
    closed: bool,
) {
    let first = usize::from(closed);
    let last = path.len().saturating_sub(usize::from(closed));
    let limit = style.miter_limit.max(1.0) * half;
    for i in first..last {
        let (Some(p), Some(before), Some(after)) = (
            path.get(i),
            normal_at(path, i.wrapping_sub(1), 0),
            normal_at(path, i, 1),
        ) else {
            continue;
        };
        let dot = before.0 * after.0 + before.1 * after.1;
        let half_angle = ((1.0 + dot).max(0.0) / 2.0).sqrt();
        if half_angle <= f64::EPSILON || half_angle >= 0.998 {
            continue;
        }
        if half / half_angle <= limit {
            continue;
        }
        // Bevelled: both offset corners, in the order the outline runs.
        outline.push((p.0 + before.0 * half, p.1 + before.1 * half));
        outline.push((p.0 + after.0 * half, p.1 + after.1 * half));
    }
}

/// Add a cap to one end of a stroke outline.
fn cap(
    outline: &mut Vec<(f64, f64)>,
    point: (f64, f64),
    normal: (f64, f64),
    kind: LineCap,
    half: f64,
    at_end: bool,
) {
    match kind {
        LineCap::Butt => {}
        LineCap::Square => {
            // A square cap extends the stroke by its own half width.
            let extend = if at_end { 1.0 } else { -1.0 };
            let (Some(last), Some(first)) = (outline.last().copied(), outline.first().copied())
            else {
                return;
            };
            let _ = (last, first);
            let along = (normal.1, -normal.0);
            outline.push((
                point.0 + along.0 * extend * half + normal.0 * half,
                point.1 + along.1 * extend * half + normal.1 * half,
            ));
        }
        LineCap::Round => {
            // A round cap is a semicircle, flattened finely enough that the chord error
            // is below a quarter of a pixel.
            let steps = 16usize;
            let start = normal.1.atan2(normal.0);
            for step in 0..=steps {
                let angle = start
                    + if at_end {
                        -std::f64::consts::PI * step as f64 / steps as f64
                    } else {
                        std::f64::consts::PI * step as f64 / steps as f64
                    };
                outline.push((point.0 + angle.cos() * half, point.1 + angle.sin() * half));
            }
        }
    }
}

/// The unit normal of a segment, to its left.
///
/// `before` and `after` say which neighbour this is: `0` for the segment arriving at the
/// point, `1` for the one leaving it. A closed path's last point has no next segment and a
/// first point has no previous one, so the caller says which end it is at.
fn normal_at(points: &[(f64, f64)], index: usize, after: u8) -> Option<(f64, f64)> {
    let (i, j) = if after == 1 {
        (index, index + 1)
    } else {
        (index.wrapping_sub(1), index)
    };
    let a = points.get(i)?;
    let b = points.get(j)?;
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = (dx * dx + dy * dy).sqrt();
    if length <= 0.0 {
        return None;
    }
    Some(normalise((-dy / length, dx / length)))
}

fn normalise(v: (f64, f64)) -> (f64, f64) {
    let length = (v.0 * v.0 + v.1 * v.1).sqrt();
    if length <= f64::EPSILON {
        return (0.0, 1.0);
    }
    (v.0 / length, v.1 / length)
}

/// Composite a coverage buffer over an image with a colour, source-over.
///
/// Straight (non-premultiplied) alpha, because that is what a page's own colour means
/// and what a caller hands over. The arithmetic is the specification's: the destination
/// contributes `as·(1 - αs)` and the source contributes `αs`, so compositing the same
/// colour twice converges to it rather than overshooting.
pub fn composite(image: &mut Image, cov: &Coverage, rgba: [u8; 4], origin: (usize, usize)) {
    composite_masked(image, cov, rgba, origin, None);
}

/// Composite a coverage buffer, with a per-pixel clip mask folded into the coverage.
///
/// `mask` is the whole image's mask and its width, or `None` for no clip beyond the box.
///
/// The mask is a *coverage* and is multiplied into the shape's own, so the two edges
/// antialias against each other. Treating it as a boundary instead would round each pixel
/// to 1 or 0 at the clip edge, which is the hard step a diagonal clip must not have.
pub fn composite_masked(
    image: &mut Image,
    cov: &Coverage,
    rgba: [u8; 4],
    origin: (usize, usize),
    mask: Option<(&[u8], usize)>,
) {
    let alpha = f64::from(rgba[3]);
    for (x, y, a) in cov.covered() {
        let (Some(px), Some(py)) = (origin.0.checked_add(x), origin.1.checked_add(y)) else {
            continue;
        };
        let Some(dst) = image.get(px, py) else {
            continue;
        };
        // The mask is in the image's coordinates and the coverage in the clipped area's, so
        // the pixel is found by where it will land rather than by where it was rasterised.
        let a = match mask {
            Some((m, width)) if width > 0 && py < image.height => {
                let clipped = f32::from(m.get(py * width + px).copied().unwrap_or(0)) / 255.0;
                a * clipped
            }
            Some(_) => 0.0,
            None => a,
        };
        let src_a = alpha * f64::from(a) / 255.0;
        if src_a <= 0.0 {
            continue;
        }
        image.put(px, py, over(dst, rgba, src_a));
    }
}

/// Source-over one pixel: the destination keeps its own alpha and the source goes on top.
///
/// `src_alpha` is the source's *effective* opacity in 0..1 — its own alpha multiplied by
/// whatever coverage or clip the caller had — and it is passed rather than read out of `src`
/// so that a caller with a colour that varies from pixel to pixel is doing the same
/// arithmetic as one with a colour that does not.
///
/// Straight (non-premultiplied) alpha throughout: the output's alpha is `αs + αd(1 − αs)`,
/// and dividing the colour by it undoes the unpremultiply so the buffer stays straight.
#[must_use]
pub fn over(dst: [u8; 4], src: [u8; 4], src_alpha: f64) -> [u8; 4] {
    let dst_a = f64::from(dst[3]) / 255.0;
    let out_a = src_alpha + dst_a * (1.0 - src_alpha);
    let mut out = [0u8; 4];
    // Walking the two colours together is what keeps the channels paired: an index
    // into an array of four invites an off-by-one that a zip cannot express.
    for ((s, d), slot) in src.iter().zip(dst.iter()).zip(out.iter_mut()).take(3) {
        let s = f64::from(*s) / 255.0;
        let d = f64::from(*d) / 255.0;
        let value = if out_a > 0.0 {
            (s * src_alpha + d * dst_a * (1.0 - src_alpha)) / out_a
        } else {
            0.0
        };
        *slot = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    if let Some(slot) = out.get_mut(3) {
        *slot = (out_a.clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    out
}

/// Where a device space lands on the page: a rectangle in device units and its pixel
/// size, which is the only place a scale factor belongs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    /// The device rectangle the page is drawn into.
    pub device: Rect,
    /// The page's size, in points.
    pub page: (f64, f64),
    /// Pixels per point.
    pub scale: f64,
    /// A placement the caller worked out itself, which takes precedence over the scale
    /// and translation this type would otherwise derive.
    placement: Option<mangle_content::Matrix>,
}

impl Viewport {
    /// The transformation from page space to device space.
    ///
    /// A page's placement is a fit, a flip and possibly a rotation, so it is not
    /// derivable from a scale and a rectangle alone; when the caller has supplied one,
    /// that is what is used.
    #[must_use]
    pub fn matrix(self) -> mangle_content::Matrix {
        if let Some(m) = self.placement {
            return m;
        }
        let (w, h) = self.page;
        let sx = if w > 0.0 {
            self.device.width() / w
        } else {
            self.scale
        };
        let sy = if h > 0.0 {
            self.device.height() / h
        } else {
            self.scale
        };
        mangle_content::Matrix::new(sx, 0.0, 0.0, -sy, self.device.x0, self.device.y1)
    }

    /// The pixel buffer a page of this size needs.
    ///
    /// Rounded up, because a device rectangle of 208.33 pixels needs 209 to hold its last
    /// row, and a buffer that rounds down silently clips the edge of the page.
    #[must_use]
    pub fn buffer(self) -> Image {
        let w = self.device.width().ceil().max(1.0) as usize;
        let h = self.device.height().ceil().max(1.0) as usize;
        Image::new(w, h)
    }
}

impl Rect {
    /// The rectangle's width.
    #[must_use]
    pub fn width(self) -> f64 {
        (self.x1 - self.x0).abs()
    }

    /// The rectangle's height.
    #[must_use]
    pub fn height(self) -> f64 {
        (self.y1 - self.y0).abs()
    }
}

/// A clipping path in device space: the region, and the rule that decides its inside.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipShape {
    /// The path, already transformed into device pixels.
    pub polygon: Polygon,
    /// Non-zero or even-odd, as the `W` operator that set it said.
    pub rule: FillRule,
}

/// A clipping region in device space: the box that bounds it, and every path in force.
///
/// The two are the same clip said twice, which is what makes it worth carrying both: the box
/// rejects a pixel before any coverage is computed, and the paths are what decides the pixels
/// the box lets through.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipRegion {
    /// Nothing outside this rectangle is drawn.
    pub bounds: Rect,
    /// Every clipping path in force, outermost first. The region is where they all overlap;
    /// an empty list is a clip to nothing rather than no clip.
    pub paths: Vec<ClipShape>,
}

/// The whole image's mask, from a coverage buffer rasterised over a rectangle.
///
/// `area` is the rectangle that was rasterised and `width` the image's width. A coverage
/// buffer covers only the pixels its rectangle reaches, so this is where a region is grown
/// into the image's own shape; the rest stays at zero, which is right, because those pixels
/// are outside the clip path by construction.
fn mask_from(cov: &Coverage, area: Rect, width: usize, height: usize) -> Vec<u8> {
    let mut mask = vec![0u8; width * height];
    let Some((columns, rows)) = area.pixels() else {
        return mask;
    };
    for (x, y, alpha) in cov.covered() {
        let (Some(column), Some(row)) = (columns.start.checked_add(x), rows.start.checked_add(y))
        else {
            continue;
        };
        if column >= width || row >= height {
            continue;
        }
        if let Some(slot) = mask.get_mut(row * width + column) {
            *slot = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    mask
}

/// Two masks multiplied together, which is how two successive clips nest.
///
/// Multiplying rather than choosing is what makes a clip to a circle inside a clip to a
/// rectangle the shape the page drew, and it costs one multiply per pixel on a page that has
/// asked for it.
fn intersect_masks(a: &[u8], b: &[u8]) -> Vec<u8> {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| ((u32::from(*x) * u32::from(*y) + 127) / 255) as u8)
        .collect()
}

/// Zero every pixel outside a rectangle, so a mask agrees with a box that has just narrowed.
fn narrow_mask(mask: &mut [u8], width: usize, height: usize, rect: Rect) {
    let area = rect.normalised();
    for y in 0..height {
        let row = y * width;
        let Some(cells) = mask.get_mut(row..row.saturating_add(width)) else {
            continue;
        };
        let row_y = y as f64;
        if row_y < area.y0 || row_y >= area.y1 {
            cells.fill(0);
            continue;
        }
        for (x, cell) in cells.iter_mut().enumerate() {
            let column = x as f64;
            if column < area.x0 || column >= area.x1 {
                *cell = 0;
            }
        }
    }
}

/// The current rendering state: the clip, and the base colour.
#[derive(Debug, Clone)]
pub struct Device {
    image: Image,
    /// The clip's bounding box, which is the fast bound: it rejects a pixel before any
    /// coverage is computed, and it is all a rectangle clip needs.
    clip: Rect,
    /// The clip's per-pixel coverage, one byte per pixel in the image's own order, when the
    /// clip is not a rectangle.
    ///
    /// Coverage rather than a flag, because a clip edge is a place where two renderers
    /// legitimately differ by a fraction of a pixel and a hard boundary there is a
    /// staircase down a diagonal. `None` means the clip is the box alone, which is the case
    /// for almost every page and costs a branch per pixel on the rest.
    clip_mask: Option<Vec<u8>>,
    /// The colour new marks are drawn in, as straight RGBA.
    pub fill: [u8; 4],
    pub stroke: [u8; 4],
    stroke_style: StrokeStyle,
}

impl Device {
    /// A device drawing into an image, with the whole image as its clip.
    #[must_use]
    pub fn new(image: Image) -> Self {
        let clip = image.rect();
        Self {
            image,
            clip,
            clip_mask: None,
            fill: [0, 0, 0, 255],
            stroke: [0, 0, 0, 255],
            stroke_style: StrokeStyle::default(),
        }
    }

    #[must_use]
    pub fn image(&self) -> &Image {
        &self.image
    }

    #[must_use]
    pub fn image_mut(&mut self) -> &mut Image {
        &mut self.image
    }

    pub fn into_image(self) -> Image {
        self.image
    }

    /// Write one pixel, ignoring a position outside the image.
    ///
    /// An image placed at an angle writes into the bounding box of its transformed unit
    /// square, which reaches past the page's edge, and the bounds check belongs here rather
    /// than in every caller.
    pub fn put(&mut self, x: usize, y: usize, rgba: [u8; 4]) {
        if x >= self.image.width || y >= self.image.height {
            return;
        }
        self.image.put(x, y, rgba);
    }

    /// Narrow the clip to a rectangle.
    ///
    /// A clip to nothing means nothing further is drawn, which is a state and not an error.
    /// A mask already in force is narrowed to the rectangle as well: a clip and a mask that
    /// disagree about where the page stops is not a fast path, it is two answers to one
    /// question, and the renderer would draw whatever the looser of the two allows.
    pub fn clip_to(&mut self, rect: Rect) {
        self.clip = self.clip.intersect(rect);
        if let Some(mask) = self.clip_mask.as_mut() {
            narrow_mask(mask, self.image.width, self.image.height, self.clip);
        }
    }

    /// Install the clip in force for a mark, replacing whatever was in force before.
    ///
    /// This is a whole clip rather than a narrowing of the last one, and that is the point of
    /// it: a mark's appearance must follow from the mark and the clip it says it was drawn
    /// under, and not from whatever was drawn before it. Installing the same region twice gives
    /// the same clip, so a caller may install per mark and a per-region comparison can skip the
    /// repeat.
    ///
    /// `None` is no clip beyond the page, which is what `Q` restores and what a page that
    /// never clipped has. `Some` with no paths is a clip to *nothing*, which is a different
    /// state — an empty `W n` sets the region to the empty region — and is why the two are not
    /// the same argument.
    pub fn install_clip(&mut self, clip: Option<&ClipRegion>) {
        let Some(clip) = clip else {
            self.reset_clip();
            return;
        };
        self.clip = self.image.rect().intersect(clip.bounds);
        self.clip_mask = None;
        for shape in &clip.paths {
            self.clip_to_path(&shape.polygon, shape.rule);
        }
    }

    /// Narrow the clip to a path, under a fill rule.
    ///
    /// The path is rasterised with the same code that fills a shape, and the result becomes
    /// the device's mask. An existing mask is *intersected* rather than replaced, because two
    /// successive clip operators nest: a second `W n` inside a first one can only ever
    /// remove pixels, and replacing the mask would widen the clip to the second path on its
    /// own. The rectangle is narrowed to the path's box at the same time, so a caller that
    /// never reads the mask still gets its pixels rejected early.
    pub fn clip_to_path(&mut self, polygon: &Polygon, rule: FillRule) {
        // The mask is the whole image, because it is read by pixel coordinate and a mask
        // that started at some later pixel would have to be offset on every read.
        let area = self.image.rect();
        let coverage = rasterise(&polygon.edges(), area, rule);
        let fresh = mask_from(&coverage, area, self.image.width, self.image.height);
        self.clip_mask = Some(match self.clip_mask.take() {
            Some(existing) => intersect_masks(&existing, &fresh),
            None => fresh,
        });
        if let Some(bounds) = polygon.bounds() {
            self.clip = self.clip.intersect(bounds);
        }
    }

    /// The clip's coverage at a pixel, or `None` when the clip is only a box.
    ///
    /// `None` on the common path is the point: a page whose clips are rectangles — which is
    /// nearly all of them — pays one branch per pixel and nothing else.
    #[must_use]
    pub fn clip_coverage(&self, x: usize, y: usize) -> Option<f64> {
        let mask = self.clip_mask.as_ref()?;
        let w = self.image.width;
        if y >= self.image.height || x >= w {
            return Some(0.0);
        }
        mask.get(y * w + x).map(|c| f64::from(*c) / 255.0)
    }

    /// Restore the clip to the whole image, box and mask both.
    pub fn reset_clip(&mut self) {
        self.clip = self.image.rect();
        self.clip_mask = None;
    }

    #[must_use]
    pub fn clip(&self) -> Rect {
        self.clip
    }

    pub fn set_stroke_style(&mut self, style: StrokeStyle) {
        self.stroke_style = style;
    }

    #[must_use]
    pub fn stroke_style(&self) -> &StrokeStyle {
        &self.stroke_style
    }

    /// Fill a polygon.
    ///
    /// The clip mask is folded into the coverage rather than used to reject pixels, so a
    /// fill that crosses a diagonal clip is antialiased against that edge. Clipping to the
    /// mask as a boundary would draw a staircase, which is exactly what a bounding box does
    /// today and exactly what a clip is supposed not to do.
    pub fn fill_polygon(&mut self, polygon: &Polygon, rule: FillRule, colour: [u8; 4]) {
        let Some(bounds) = polygon.bounds() else {
            return;
        };
        let area = bounds.intersect(self.clip);
        if area.is_empty() {
            return;
        }
        let cov = rasterise(&polygon.edges(), area, rule);
        let origin = (
            area.x0.floor().max(0.0) as usize,
            area.y0.floor().max(0.0) as usize,
        );
        let mask = self
            .clip_mask
            .as_ref()
            .map(|m| (m.as_slice(), self.image.width));
        composite_masked(&mut self.image, &cov, colour, origin, mask);
    }

    /// Stroke a polygon's outline, with caps, joins and dashes.
    pub fn stroke_polygon(&mut self, polygon: &Polygon, style: &StrokeStyle, colour: [u8; 4]) {
        let pairs = dash_pairs(&style.dash);
        for subpath in &polygon.subpaths {
            let closed: Vec<(f64, f64)> = if subpath.len() > 2 {
                let mut c = subpath.clone();
                if let (Some(first), Some(last)) = (c.first().copied(), c.last().copied())
                    && (first.0 != last.0 || first.1 != last.1)
                {
                    c.push(first);
                }
                c
            } else {
                subpath.clone()
            };
            if closed.len() < 2 {
                continue;
            }
            let mut outline = Polygon::default();
            // Only the **on** runs are inked. The walk already alternates and says which is
            // which; drawing the off runs as well fills the gaps back in and turns every
            // pattern into a solid line, which is why a dashed line's length was once
            // measured as the whole line. A gap is not a thinner stroke, it is no stroke.
            walk_dashes(&closed, &pairs, style.dash.phase, |on, run| {
                if !on {
                    return;
                }
                let piece = stroke_outline(run, style);
                outline.subpaths.extend(piece.subpaths);
            });
            if outline.is_empty() {
                continue;
            }
            self.fill_polygon(&outline, FillRule::NonZero, colour);
        }
    }
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
        clippy::indexing_slicing,
        clippy::float_cmp
    )]

    use super::*;
    use mangle_content::Matrix;

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-3
    }

    fn square(x0: f64, y0: f64, x1: f64, y1: f64) -> Polygon {
        Polygon {
            subpaths: vec![vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)]],
        }
    }

    fn hole_ring() -> Polygon {
        Polygon {
            subpaths: vec![
                vec![(0.0, 0.0), (8.0, 0.0), (8.0, 8.0), (0.0, 8.0), (0.0, 0.0)],
                // The other way round, which is what makes a hole under the non-zero rule.
                vec![(2.0, 2.0), (2.0, 6.0), (6.0, 6.0), (6.0, 2.0), (2.0, 2.0)],
            ],
        }
    }

    #[test]
    fn a_curve_that_is_already_straight_is_two_points() {
        let pts = flatten_curve((0.0, 0.0), (1.0, 1.0), (2.0, 2.0), (3.0, 3.0));
        assert_eq!(pts.len(), 2, "no deviation means no subdivision");
        // Two points split the chord in half, so they are the midpoint and the end.
        assert_eq!(pts.first().copied(), Some((1.5, 1.5)));
        assert_eq!(pts.last().copied(), Some((3.0, 3.0)));
    }

    #[test]
    fn a_curve_that_bulges_is_flattened_further() {
        let straight = flatten_curve((0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (3.0, 0.0));
        let curved = flatten_curve((0.0, 0.0), (1.0, 4.0), (2.0, 4.0), (3.0, 0.0));
        assert!(
            curved.len() > straight.len(),
            "a curve that deviates from its chord needs more segments: {} vs {}",
            curved.len(),
            straight.len()
        );
    }

    #[test]
    fn the_flattening_never_stops_on_the_curve_it_was_given() {
        // A wild curve is capped, and the cap is a bound rather than a hang.
        let pts = flatten_curve((0.0, 0.0), (0.0, 1000.0), (1000.0, 1000.0), (1000.0, 0.0));
        assert!(pts.len() <= MAX_CURVE_SEGMENTS);
        let last = pts.last().copied().unwrap_or((0.0, 0.0));
        assert!(
            near(last.0, 1000.0) && near(last.1, 0.0),
            "the end is reached"
        );
    }

    #[test]
    fn a_point_on_a_curve_is_the_bezier_value() {
        // At the midpoint of a symmetric cubic the shape is three quarters of the way to
        // the control points' height: the two control terms each weigh three eighths.
        let (x, y) = cubic_at((0.0, 0.0), (0.0, 3.0), (4.0, 3.0), (4.0, 0.0), 0.5);
        assert!(near(x, 2.0) && near(y, 2.25), "got ({x}, {y})");
    }

    #[test]
    fn a_rectangle_polygon_closes_implicitly() {
        let p = square(0.0, 0.0, 4.0, 4.0);
        let edges = p.edges();
        // Four sides, and the repeated last point adds nothing.
        assert_eq!(edges.len(), 4);
        assert!(near(p.signed_area(), 16.0), "got {}", p.signed_area());
    }

    #[test]
    fn an_open_polygon_still_encloses_when_filled() {
        // Three sides: a fill closes it, which is what the fill operators do.
        let p = Polygon {
            subpaths: vec![vec![(0.0, 0.0), (4.0, 0.0), (4.0, 4.0)]],
        };
        assert_eq!(p.edges().len(), 3, "the closing edge is implicit");
        assert!(p.contains(3.0, 1.0, FillRule::NonZero), "inside");
        assert!(!p.contains(0.5, 3.0, FillRule::NonZero), "outside");
    }

    #[test]
    fn an_opposite_winding_is_a_hole_under_both_rules() {
        let ring = hole_ring();
        for rule in [FillRule::NonZero, FillRule::EvenOdd] {
            let cov = rasterise(&ring.edges(), clip(10.0, 10.0), rule);
            assert!(
                near(f64::from(cov.at(4, 4)), 0.0),
                "the middle is a hole under {rule:?}, got {}",
                cov.at(4, 4)
            );
            assert!(
                near(f64::from(cov.at(1, 1)), 1.0),
                "and the ring is inside under {rule:?}"
            );
            // Eight by eight less four by four, either way.
            assert!(
                near(cov.total_area(), 48.0),
                "the ring's area under {rule:?} is 48, got {}",
                cov.total_area()
            );
        }
    }

    /// The case the two rules exist for: two sub-paths wound the *same* way. Non-zero
    /// counts the winding and fills the middle; even-odd counts the crossings and calls
    /// it outside.
    #[test]
    fn the_two_rules_disagree_about_same_wound_sub_paths() {
        let nested = Polygon {
            subpaths: vec![
                vec![(0.0, 0.0), (8.0, 0.0), (8.0, 8.0), (0.0, 8.0), (0.0, 0.0)],
                vec![(2.0, 2.0), (6.0, 2.0), (6.0, 6.0), (2.0, 6.0), (2.0, 2.0)],
            ],
        };
        let non_zero = rasterise(&nested.edges(), clip(10.0, 10.0), FillRule::NonZero);
        let even_odd = rasterise(&nested.edges(), clip(10.0, 10.0), FillRule::EvenOdd);
        assert!(
            near(f64::from(non_zero.at(4, 4)), 1.0),
            "non-zero winds to two there, so it is inside"
        );
        assert!(
            near(f64::from(even_odd.at(4, 4)), 0.0),
            "even-odd counts two crossings, so it is outside; got {}",
            even_odd.at(4, 4)
        );
        // The outer ring is inside either way.
        assert!(near(f64::from(non_zero.at(1, 1)), 1.0));
        assert!(near(f64::from(even_odd.at(1, 1)), 1.0));
    }

    #[test]
    fn an_opposite_winding_is_a_hole_and_two_same_way_subpaths_are_not() {
        let hole = hole_ring();
        // The same inner square wound the same way as the outer: now the middle is
        // inside twice over, which is filled under both rules.
        let filled = Polygon {
            subpaths: vec![
                hole.subpaths[0].clone(),
                vec![(2.0, 2.0), (6.0, 2.0), (6.0, 6.0), (2.0, 6.0), (2.0, 2.0)],
            ],
        };
        let cov = rasterise(&filled.edges(), clip(10.0, 10.0), FillRule::NonZero);
        assert!(
            near(f64::from(cov.at(4, 4)), 1.0),
            "the middle is inside, not a hole"
        );
        assert!(
            near(cov.total_area(), 64.0),
            "the whole eight by eight square is covered, got {}",
            cov.total_area()
        );
    }

    #[test]
    fn contains_agrees_with_the_rasteriser() {
        let ring = hole_ring();
        for (x, y) in [(1.0, 1.0), (4.0, 4.0), (7.0, 7.0), (0.5, 8.5)] {
            let cov = rasterise(&ring.edges(), clip(10.0, 10.0), FillRule::NonZero);
            let by_maths = ring.contains(x, y, FillRule::NonZero);
            let by_pixels = cov.at(x as usize, y as usize) > 0.5;
            assert_eq!(
                by_maths, by_pixels,
                "at ({x}, {y}) the point test and the rasteriser disagree"
            );
        }
    }

    #[test]
    fn a_transformed_path_lands_where_the_matrix_says() {
        let segments = [
            PathSegment::Move(0.0, 0.0),
            PathSegment::Line(10.0, 0.0),
            PathSegment::Line(10.0, 10.0),
            PathSegment::Close,
        ];
        let m = Matrix::new(2.0, 0.0, 0.0, 2.0, 5.0, 5.0);
        let p = transform_path(&segments, &m);
        let b = p.bounds().expect("bounds");
        assert_eq!((b.x0, b.y0, b.x1, b.y1), (5.0, 5.0, 25.0, 25.0));
    }

    #[test]
    fn a_curve_is_transformed_before_it_is_flattened() {
        // A quarter turn: a curve along the x axis becomes one along the y axis. If the
        // flattening happened first, the segment count would follow the user-space scale.
        let segments = [
            PathSegment::Move(0.0, 0.0),
            PathSegment::Curve(0.0, 10.0, 0.0, 20.0, 0.0, 30.0),
            PathSegment::Close,
        ];
        let flat = transform_path(&segments, &Matrix::IDENTITY);
        let turned = transform_path(&segments, &Matrix::rotate(90.0));
        assert_eq!(flat.subpaths[0].len(), turned.subpaths[0].len());
        let b = turned.bounds().expect("bounds");
        assert!(near(b.x0, -30.0) && near(b.x1, 0.0), "got {b:?}");
    }

    #[test]
    fn a_stroke_widens_the_path_by_half_its_width_on_each_side() {
        let points = [(0.0, 0.0), (10.0, 0.0)];
        let style = StrokeStyle {
            width: 4.0,
            ..StrokeStyle::default()
        };
        let outline = stroke_outline(&points, &style);
        let b = outline.bounds().expect("bounds");
        assert!(
            near(b.y0, -2.0) && near(b.y1, 2.0),
            "two either side: {b:?}"
        );
        assert!(near(b.x0, 0.0) && near(b.x1, 10.0), "and no longer: {b:?}");
    }

    #[test]
    fn a_zero_width_stroke_draws_nothing() {
        let style = StrokeStyle {
            width: 0.0,
            ..StrokeStyle::default()
        };
        assert!(stroke_outline(&[(0.0, 0.0), (10.0, 0.0)], &style).is_empty());
    }

    #[test]
    fn a_butt_cap_stops_at_the_end_and_a_square_cap_does_not() {
        let points = [(0.0, 0.0), (10.0, 0.0)];
        let butt = stroke_outline(&points, &StrokeStyle::default());
        let square = stroke_outline(
            &points,
            &StrokeStyle {
                width: 2.0,
                cap: LineCap::Square,
                ..StrokeStyle::default()
            },
        );
        let (bb, _) = (butt.bounds(), square.bounds());
        let b = bb.expect("butt bounds");
        let s = square.bounds().expect("square bounds");
        assert_eq!(b.x1, 10.0, "a butt cap ends at the last point");
        assert!(
            s.x1 > b.x1,
            "a square cap extends past it: {} vs {}",
            s.x1,
            b.x1
        );
    }

    #[test]
    fn a_sharp_corner_is_beveled_rather_than_spiked() {
        // A near-reversal: the miter limit should stop it reaching across the path.
        let points = [(0.0, 0.0), (10.0, 0.0), (0.1, 0.0)];
        let outline = stroke_outline(
            &points,
            &StrokeStyle {
                width: 2.0,
                miter_limit: 1.0,
                ..StrokeStyle::default()
            },
        );
        let b = outline.bounds().expect("bounds");
        assert!(
            b.x1 - b.x0 <= 12.0,
            "a spike did not reach past the stroke: {b:?}"
        );
    }

    #[test]
    fn a_round_join_reaches_the_miter_point() {
        let points = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)];
        let mut outline = stroke_outline(
            &points,
            &StrokeStyle {
                width: 2.0,
                miter_limit: 10.0,
                ..StrokeStyle::default()
            },
        );
        let b = outline.bounds().expect("bounds");
        // The corner at (10, 0) miters out to (11, -1): past both edges' outer bounds.
        assert!(
            b.x1 >= 11.0 || b.y0 <= -1.0,
            "the corner was not built: {b:?}"
        );
        outline.subpaths.clear();
        assert!(outline.is_empty());
    }

    #[test]
    fn a_solid_dash_pattern_is_one_run() {
        let mut runs = Vec::new();
        walk_dashes(&[(0.0, 0.0), (10.0, 0.0)], &[], 0.0, |on, run| {
            runs.push((on, run.len()));
            let _ = run;
        });
        assert_eq!(runs, vec![(true, 2)], "one on-run covering both points");
    }

    #[test]
    fn a_dash_pattern_splits_a_line_into_runs() {
        // Four on, four off, over a line of twenty: on at 0, 8 and 16, so three runs.
        let pattern = mangle_content::Dash {
            array: vec![4.0, 4.0],
            phase: 0.0,
        };
        let mut on_lengths = Vec::new();
        walk_dashes(
            &[(0.0, 0.0), (20.0, 0.0)],
            &dash_pairs(&pattern),
            0.0,
            |on, run| {
                if on {
                    let length: f64 = run
                        .windows(2)
                        .map(|w| ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt())
                        .sum();
                    on_lengths.push(length);
                }
            },
        );
        assert_eq!(on_lengths.len(), 3, "three on-runs: {on_lengths:?}");
        for length in &on_lengths {
            assert!(near(*length, 4.0), "each is four long, got {length}");
        }
    }

    #[test]
    fn a_dash_phase_shifts_where_the_pattern_starts() {
        let pattern = mangle_content::Dash {
            array: vec![4.0, 4.0],
            phase: 0.0,
        };
        let pairs = dash_pairs(&pattern);
        let first_run = |phase: f64| {
            let mut start = None;
            walk_dashes(&[(0.0, 0.0), (20.0, 0.0)], &pairs, phase, |on, run| {
                if on && start.is_none() {
                    start = run.first().map(|p| p.0);
                }
            });
            start.unwrap_or(-1.0)
        };
        // The pattern is four on then four off, so a phase says where in that sequence
        // the line starts:
        assert_eq!(
            first_run(0.0),
            0.0,
            "the line starts at the beginning of an on-run"
        );
        assert_eq!(
            first_run(2.0),
            0.0,
            "half way through an on-run, so the line starts inside one"
        );
        assert_eq!(
            first_run(4.0),
            4.0,
            "at the start of an off-run, so the first on-run is four units along"
        );
        assert_eq!(
            first_run(6.0),
            2.0,
            "half way through an off-run, so only two units are skipped"
        );
    }

    #[test]
    fn a_zero_length_dash_element_does_not_hang() {
        let pairs = vec![(0.0, 4.0)];
        let mut count = 0;
        walk_dashes(&[(0.0, 0.0), (8.0, 0.0)], &pairs, 0.0, |_, _| count += 1);
        assert!(count < 100, "a zero-length run is a hairline, not a loop");
    }

    /// A dash pattern is drawn as gaps, and this is where that is decided.
    ///
    /// `walk_dashes` already alternates and hands each run a flag saying whether it is ink or
    /// paper; `stroke_polygon` is what does with the flag. Stroking every run — the flag
    /// ignored — fills the gaps back in and draws **every** pattern as one solid line, which is
    /// what a page with a dashed line looks like when the dashes are not the page's fault.
    ///
    /// `[6 3] 0 d` over a line of 20 units with a width of 2 is twenty units long and cannot
    /// hold a whole dash twice over, so the shape is easy to count: two runs of six units of
    /// ink with four of paper between and four after.
    #[test]
    fn a_dash_pattern_leaves_paper_where_the_gaps_are() {
        let pattern = mangle_content::Dash {
            array: vec![6.0, 3.0],
            phase: 0.0,
        };
        let style = StrokeStyle {
            width: 2.0,
            dash: pattern,
            ..StrokeStyle::default()
        };
        let line = Polygon {
            subpaths: vec![vec![(10.0, 50.0), (30.0, 50.0)]],
        };
        let mut device = paper(60, 100);
        device.stroke_polygon(&line, &style, BLACK);

        // A row through the middle of a two-unit stroke at y = 50 is row 50, and the dash
        // boundaries land on whole pixels because every length here is a whole number.
        let ink_at = |x: usize| darkness(device.image(), x, 50) > 128;
        let dashed: Vec<usize> = (10..30).filter(|x| ink_at(*x)).collect();
        assert_eq!(
            dashed,
            vec![
                10, 11, 12, 13, 14, 15, // six of ink, and the butt cap starts on the pixel
                19, 20, 21, 22, 23, 24, // three of paper, then six more
                28, 29 // three of paper, then the line ends two units into the run
            ],
            "six units of ink, three of paper, six more, three of paper, and two — the \
             gaps are paper rather than a thinner stroke"
        );
    }

    #[test]
    fn compositing_an_opaque_colour_replaces_it() {
        let mut image = Image::filled(4, 4, [255, 0, 0, 255]);
        let cov = rasterise(
            &square(0.0, 0.0, 4.0, 4.0).edges(),
            Rect {
                x0: 0.0,
                y0: 0.0,
                x1: 4.0,
                y1: 4.0,
            },
            FillRule::NonZero,
        );
        composite(&mut image, &cov, [0, 0, 255, 255], (0, 0));
        assert_eq!(image.get(2, 2), Some([0, 0, 255, 255]), "blue over red");
    }

    #[test]
    fn compositing_half_covered_pixels_blends_them() {
        let mut image = Image::filled(4, 4, [0, 0, 0, 255]);
        let cov = rasterise(
            &square(0.0, 0.0, 0.5, 1.0).edges(),
            Rect {
                x0: 0.0,
                y0: 0.0,
                x1: 4.0,
                y1: 4.0,
            },
            FillRule::NonZero,
        );
        composite(&mut image, &cov, [255, 255, 255, 255], (0, 0));
        let pixel = image.get(0, 0).unwrap_or([0, 0, 0, 0]);
        // Half white over black: about 128.
        let grey = u16::from(pixel[0]);
        assert!(
            (120..=135).contains(&grey),
            "half a pixel of white over black is mid grey, got {grey}"
        );
    }

    #[test]
    fn compositing_nothing_changes_nothing() {
        let mut image = Image::filled(2, 2, [1, 2, 3, 255]);
        let empty = Coverage::default();
        composite(&mut image, &empty, [255, 255, 255, 255], (0, 0));
        assert_eq!(image.get(0, 0), Some([1, 2, 3, 255]));
    }

    #[test]
    fn a_viewport_puts_the_origin_at_the_bottom_left() {
        // Page space has its origin at the lower left and y increasing upward; device
        // space has y increasing downward. Getting this wrong flips every page.
        let vp = Viewport {
            device: Rect {
                x0: 0.0,
                y0: 0.0,
                x1: 200.0,
                y1: 400.0,
            },
            page: (200.0, 400.0),
            scale: 1.0,
            placement: None,
        };
        let m = vp.matrix();
        assert_eq!(m.apply(0.0, 0.0), (0.0, 400.0), "page origin, bottom left");
        assert_eq!(
            m.apply(200.0, 400.0),
            (200.0, 0.0),
            "page corner, top right"
        );
        assert_eq!(
            m.apply(100.0, 200.0),
            (100.0, 200.0),
            "the centre stays put"
        );
    }

    #[test]
    fn a_viewport_shoots_a_buffer_of_the_right_size() {
        let vp = Viewport {
            device: Rect {
                x0: 0.0,
                y0: 0.0,
                x1: 612.0,
                y1: 792.0,
            },
            page: (612.0, 792.0),
            scale: 1.0,
            placement: None,
        };
        let buffer = vp.buffer();
        assert_eq!((buffer.width, buffer.height), (612, 792));
    }

    #[test]
    fn a_clip_narrows_and_then_restores() {
        let mut device = Device::new(Image::filled(100, 100, [255, 255, 255, 255]));
        let full = device.clip();
        device.clip_to(Rect {
            x0: 10.0,
            y0: 10.0,
            x1: 20.0,
            y1: 20.0,
        });
        assert_eq!(device.clip().x0, 10.0);
        device.reset_clip();
        assert_eq!(device.clip(), full);
    }

    #[test]
    fn a_clip_outside_everything_means_nothing_is_drawn() {
        let mut device = Device::new(Image::filled(10, 10, [255, 255, 255, 255]));
        device.clip_to(Rect {
            x0: 100.0,
            y0: 100.0,
            x1: 110.0,
            y1: 110.0,
        });
        device.fill_polygon(
            &square(0.0, 0.0, 10.0, 10.0),
            FillRule::NonZero,
            [0, 0, 0, 255],
        );
        assert_eq!(
            device.image().get(5, 5),
            Some([255, 255, 255, 255]),
            "the shape is outside the clip, so the page is untouched"
        );
    }

    // ── Clipping by a region rather than by a box ───────────────────────────────

    const BLACK: [u8; 4] = [0, 0, 0, 255];

    /// A device with a white page of the given size.
    fn paper(w: usize, h: usize) -> Device {
        Device::new(Image::filled(w, h, [255, 255, 255, 255]))
    }

    /// How many of the page's pixels are within `tolerance` of black.
    fn near_black(image: &Image, tolerance: u32) -> usize {
        (0..image.height)
            .flat_map(|y| (0..image.width).map(move |x| (x, y)))
            .filter(|(x, y)| darkness(image, *x, *y) > tolerance)
            .count()
    }

    /// How dark is this pixel? 0 is paper and 255 is ink, on luma so that a coloured fill
    /// counts as dark too.
    fn darkness(image: &Image, x: usize, y: usize) -> u32 {
        let Some([r, g, b, _]) = image.get(x, y) else {
            return 0;
        };
        let luma = (u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114) / 1000;
        255 - luma
    }

    #[test]
    fn a_diagonal_clip_is_a_diagonal_and_not_the_box_around_it() {
        // The triangle (0,0) (100,0) (0,100) on a 100 by 100 page, filled black over the
        // whole page. Its box is the whole page, so a renderer that clips to the box paints
        // all of it; the corner just outside the hypotenuse is the pixel that tells the two
        // apart.
        let mut device = paper(100, 100);
        let clip = Polygon {
            subpaths: vec![vec![(0.0, 0.0), (100.0, 0.0), (0.0, 100.0)]],
        };
        device.clip_to_path(&clip, FillRule::NonZero);
        device.fill_polygon(&square(0.0, 0.0, 100.0, 100.0), FillRule::NonZero, BLACK);

        assert_eq!(
            device.image().get(10, 10),
            Some(BLACK),
            "just inside the hypotenuse is inside the clip"
        );
        assert_eq!(
            device.image().get(90, 90),
            Some([255, 255, 255, 255]),
            "just outside the hypotenuse is outside the clip, which a box clip gets wrong"
        );
        // Half the page is inside the triangle and half is outside it, so this is not a check
        // that passes because almost everything was drawn, and the diagonal pixels on the
        // hypotenuse are the ones with partial coverage.
        let black = near_black(device.image(), 200);
        assert!(
            (4_800..5_000).contains(&black),
            "half the page should be ink, got {black} pixels"
        );
    }

    #[test]
    fn a_circular_clip_leaves_the_corners_bare_and_antialiases_its_edge() {
        // A disc of radius 40 about (50,50) on a 100 by 100 page. Every corner of the page is
        // outside it and the middle is well inside, so a clip that leaked would show at the
        // corners first.
        let mut device = paper(100, 100);
        let disc = disc(50.0, 50.0, 40.0);
        device.clip_to_path(&disc, FillRule::NonZero);
        device.fill_polygon(&square(0.0, 0.0, 100.0, 100.0), FillRule::NonZero, BLACK);

        for (x, y) in [(2, 2), (97, 2), (2, 97), (97, 97)] {
            assert_eq!(
                device.image().get(x, y),
                Some([255, 255, 255, 255]),
                "the corner at ({x},{y}) is outside a disc of radius 40"
            );
        }
        assert_eq!(
            device.image().get(50, 50),
            Some(BLACK),
            "the middle is inside"
        );
        // Both halves of the edge are substantial: a solid interior and a solid margin, with
        // a boundary between them. The disc's area is about 5000 of the page's 10000.
        let solid = near_black(device.image(), 250);
        assert!(
            solid > 4_500,
            "the interior should be solidly ink, got {solid} pixels"
        );
        let blank = near_black(device.image(), 5);
        assert!(
            blank > 3_000,
            "the margin should be solidly paper, got {} ink pixels",
            10_000 - blank
        );
        // And the edge itself is a ramp rather than a step: a clip that rounds each pixel to
        // 1 or 0 has no such pixels at all, which is what a staircase down a circle looks
        // like when it is drawn at four times the resolution.
        let partial = (0..100)
            .flat_map(|y| (0..100).map(move |x| (x, y)))
            .filter(|(x, y)| {
                let d = darkness(device.image(), *x, *y);
                (10..245).contains(&d)
            })
            .count();
        assert!(
            partial > 60,
            "the edge of the clip should be antialiased, found {partial} partial pixels"
        );
    }

    /// A disc as the four cubics a page draws one with.
    ///
    /// The offset 0.5522847498 is the one that makes four quarter-circle cubics: it is the
    /// control-point distance that a circle's own arc needs. The segments go through
    /// `transform_path` because that is what a clip path goes through on its way to the
    /// device, and a disc built from points would test something no page writes.
    fn disc(cx: f64, cy: f64, r: f64) -> Polygon {
        let k = 0.552_284_749_8 * r;
        let (right, left) = (cx + r, cx - r);
        let (top, bottom) = (cy + r, cy - r);
        let segments = vec![
            PathSegment::Move(right, cy),
            PathSegment::Curve(right, cy + k, cx + k, top, cx, top),
            PathSegment::Curve(cx - k, top, left, cy + k, left, cy),
            PathSegment::Curve(left, cy - k, cx - k, bottom, cx, bottom),
            PathSegment::Curve(cx + k, bottom, right, cy - k, right, cy),
            PathSegment::Close,
        ];
        transform_path(&segments, &Matrix::IDENTITY)
    }

    #[test]
    fn two_clips_nest_rather_than_replacing_one_another() {
        // The left half, then the top half. Device y grows downward, so the top half is
        // y = 0 to 50. Their intersection is the top left quadrant and only that: a second
        // clip that overwrote the first would paint half the page.
        let mut device = paper(100, 100);
        device.clip_to(Rect {
            x0: 0.0,
            y0: 0.0,
            x1: 50.0,
            y1: 100.0,
        });
        device.clip_to_path(&square(0.0, 0.0, 100.0, 50.0), FillRule::NonZero);
        device.fill_polygon(&square(0.0, 0.0, 100.0, 100.0), FillRule::NonZero, BLACK);

        assert_eq!(device.image().get(25, 25), Some(BLACK), "inside both");
        // Each of these is inside one of the two clips and outside the other, which is the
        // whole of what nesting means.
        for (x, y) in [(75, 25), (25, 75), (75, 75), (10, 90), (90, 10), (98, 98)] {
            assert_eq!(
                device.image().get(x, y),
                Some([255, 255, 255, 255]),
                "({x},{y}) is inside at most one of the two clips"
            );
        }
        let black = near_black(device.image(), 200);
        assert!(
            (2_450..2_550).contains(&black),
            "a quarter of the page should be ink, got {black} pixels"
        );
    }

    #[test]
    fn a_clip_set_from_a_rectangle_behaves_exactly_as_it_did() {
        // The common case, and the fast path: a rectangle clip is a box, so no mask is
        // installed and the coverage is untouched. Every other change to the clip is a change
        // to this case's behaviour if it leaks.
        let mut device = paper(100, 100);
        assert_eq!(device.clip_coverage(50, 50), None, "no mask before a clip");
        device.clip_to(Rect {
            x0: 10.0,
            y0: 10.0,
            x1: 40.0,
            y1: 40.0,
        });
        assert_eq!(device.clip_coverage(20, 20), None, "a box is not a mask");
        device.fill_polygon(&square(0.0, 0.0, 100.0, 100.0), FillRule::NonZero, BLACK);

        for (x, y) in [(10, 10), (25, 25), (39, 39)] {
            assert_eq!(device.image().get(x, y), Some(BLACK), "({x},{y}) is inside");
        }
        for (x, y) in [(9, 20), (40, 20), (20, 9), (20, 40), (60, 60)] {
            assert_eq!(
                device.image().get(x, y),
                Some([255, 255, 255, 255]),
                "({x},{y}) is outside"
            );
        }
        // No antialiasing at the edges, because the box is on integer pixels: the box clip is
        // still a hard edge and is meant to be.
        let black = near_black(device.image(), 200);
        assert_eq!(black, 30 * 30, "exactly the rectangle, with no soft edge");
    }

    #[test]
    fn a_clip_to_nothing_draws_nothing_at_all() {
        // `W n` with an empty path. The clip is empty, so the fill is invisible — and it
        // stays invisible, because a clip that shows nothing is a state rather than an event.
        let mut device = paper(20, 20);
        device.clip_to_path(&Polygon::default(), FillRule::NonZero);
        device.fill_polygon(&square(0.0, 0.0, 20.0, 20.0), FillRule::NonZero, BLACK);
        assert!(
            device
                .image()
                .pixels
                .chunks_exact(4)
                .all(|p| p == [255, 255, 255, 255]),
            "an empty clip draws nothing"
        );

        // And a fill that does not even overlap the box gets nothing either: the clip to
        // nothing is not a box that happens to be empty, it has to reject on its own terms.
        let mut other = paper(20, 20);
        other.clip_to(Rect {
            x0: 100.0,
            y0: 100.0,
            x1: 110.0,
            y1: 110.0,
        });
        other.fill_polygon(&square(0.0, 0.0, 20.0, 20.0), FillRule::NonZero, BLACK);
        assert!(
            other
                .image()
                .pixels
                .chunks_exact(4)
                .all(|p| p == [255, 255, 255, 255]),
            "a box that overlaps nothing draws nothing"
        );
    }

    #[test]
    fn a_rectangle_narrows_a_mask_that_is_already_there() {
        // A box and a mask that disagree about where the page stops is two answers to one
        // question, and a caller that reads the mask would be told the page is wider than the
        // box says it is. The device's own draws use a box to cull with, so this is not a
        // corner case: it is what happens to a clip on every mark after it.
        let mut device = paper(100, 100);
        device.clip_to_path(&square(0.0, 0.0, 60.0, 100.0), FillRule::NonZero);
        assert_eq!(device.clip_coverage(50, 20), Some(1.0), "inside the mask");
        assert_eq!(device.clip_coverage(70, 20), Some(0.0), "outside the mask");

        // A box over the top half. The region is now the top left quadrant: the mask is
        // narrowed by the box rather than left disagreeing with it.
        device.clip_to(Rect {
            x0: 0.0,
            y0: 0.0,
            x1: 100.0,
            y1: 40.0,
        });
        assert_eq!(device.clip_coverage(50, 20), Some(1.0), "inside both");
        for (x, y) in [(50, 60), (70, 20), (70, 60)] {
            assert_eq!(
                device.clip_coverage(x, y),
                Some(0.0),
                "({x},{y}) is outside the mask or the box"
            );
        }
        device.fill_polygon(&square(0.0, 0.0, 100.0, 100.0), FillRule::NonZero, BLACK);
        assert_eq!(device.image().get(50, 20), Some(BLACK), "inside both");
        assert_eq!(
            device.image().get(50, 60),
            Some([255, 255, 255, 255]),
            "outside the box the mask may not allow it"
        );
        assert_eq!(
            device.image().get(70, 20),
            Some([255, 255, 255, 255]),
            "outside the mask the box may not allow it"
        );

        // A box that misses the mask entirely is an empty clip, not a mask that ignores the
        // box: nothing further is drawn until the clip is reset.
        let mut second = paper(100, 100);
        second.clip_to_path(&square(0.0, 0.0, 60.0, 100.0), FillRule::NonZero);
        second.clip_to(Rect {
            x0: 80.0,
            y0: 80.0,
            x1: 90.0,
            y1: 90.0,
        });
        assert_eq!(second.clip_coverage(50, 50), Some(0.0), "the mask is empty");
        second.fill_polygon(&square(0.0, 0.0, 100.0, 100.0), FillRule::NonZero, BLACK);
        assert!(
            second
                .image()
                .pixels
                .chunks_exact(4)
                .all(|p| p == [255, 255, 255, 255]),
            "a clip whose box misses the region shows nothing"
        );
    }

    #[test]
    fn a_clip_is_restored_by_a_reset() {
        // The mask has to go with the box. A mask that survived a reset would go on hiding
        // the page after `Q`, which is the sort of bug a user sees as a page that comes back
        // blank.
        let mut device = paper(100, 100);
        device.clip_to_path(
            &Polygon {
                subpaths: vec![vec![(0.0, 0.0), (30.0, 0.0), (0.0, 30.0)]],
            },
            FillRule::NonZero,
        );
        device.fill_polygon(&square(0.0, 0.0, 100.0, 100.0), FillRule::NonZero, BLACK);
        // The triangle's area is 450, and the fifteen pixels along its hypotenuse are shared
        // with the page rather than fully covered, so the ink is a little under it.
        let black = near_black(device.image(), 200);
        assert!(
            (430..450).contains(&black),
            "the triangle is painted, minus the pixels on the diagonal: {black}"
        );
        assert_eq!(
            device.image().get(20, 20),
            Some([255, 255, 255, 255]),
            "and nothing outside it"
        );

        device.reset_clip();
        assert_eq!(device.clip(), device.image().rect(), "the box is restored");
        assert_eq!(device.clip_coverage(80, 80), None, "and so is the mask");
        device.fill_polygon(&square(0.0, 0.0, 100.0, 100.0), FillRule::NonZero, BLACK);
        assert_eq!(device.image().get(80, 80), Some(BLACK), "the page is back");
    }

    #[test]
    fn filling_draws_only_what_the_polygon_covers() {
        let mut device = Device::new(Image::filled(20, 20, [255, 255, 255, 255]));
        device.fill_polygon(
            &square(0.0, 0.0, 10.0, 10.0),
            FillRule::NonZero,
            [0, 0, 0, 255],
        );
        assert_eq!(device.image().get(5, 5), Some([0, 0, 0, 255]), "inside");
        assert_eq!(
            device.image().get(15, 15),
            Some([255, 255, 255, 255]),
            "outside"
        );
    }

    #[test]
    fn stroking_a_line_draws_along_it_and_not_beside_it() {
        let mut device = Device::new(Image::filled(40, 40, [255, 255, 255, 255]));
        let line = Polygon {
            subpaths: vec![vec![(5.0, 20.0), (35.0, 20.0)]],
        };
        device.stroke_polygon(
            &line,
            &StrokeStyle {
                width: 4.0,
                ..StrokeStyle::default()
            },
            [0, 0, 0, 255],
        );
        let image = device.image();
        assert_eq!(image.get(20, 20), Some([0, 0, 0, 255]), "on the line");
        assert_eq!(image.get(20, 10), Some([255, 255, 255, 255]), "well away");
        assert_eq!(
            image.get(2, 20),
            Some([255, 255, 255, 255]),
            "before the start"
        );
    }

    fn clip(w: f64, h: f64) -> Rect {
        Rect {
            x0: 0.0,
            y0: 0.0,
            x1: w,
            y1: h,
        }
    }
}
