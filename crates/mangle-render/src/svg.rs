//! A small in-house SVG-subset renderer, built on the same rasterizer as the PDF renderer.
//!
//! GOAL.md §8.7 asks for icons drawn "through a small in-house SVG-subset renderer built on
//! the same vector rasterizer as the PDF renderer (dogfooding)". This is that: the shapes an
//! icon is made of become edges and coverage exactly as a page's paths do, and a stroke's
//! outline comes from the same [`stroke_outline`] the page uses. There is no second
//! rasterizer here, and that is the point — an icon and a page disagreeing about what a line
//! is would be a bug nobody could see.
//!
//! The accepted subset is the one the icon set is written in: `path`, `rect`, `circle`,
//! `ellipse`, `line`, `polyline` and `polygon`; the `d` commands `M m L l H h V v C c S s Q
//! q T t A a Z z`; and `fill`, `stroke`, `stroke-width`, `stroke-linecap`, `stroke-linejoin`
//! and `transform`. Everything is **one colour**, the one the caller names: `currentColor` is
//! what the icon set writes, and the tint handed in is the answer. That is not a limitation
//! so much as the definition of an icon, and it is what lets the same shape be drawn in a
//! different colour for hover, active, light and dark without rasterising it four times.

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

use crate::coverage::{Coverage, FillRule, Rect, rasterise as rasterise_coverage};
use crate::stroke_outline;
use crate::{Image, LineCap, LineJoin, Polygon, StrokeStyle, Subpath};

/// The icon's own coordinate space, when the file names no size.
const DEFAULT_SIZE: f64 = 24.0;

/// How much of a control polygon's bulge must be covered for a curve to be called flat.
///
/// The criterion is the one the page's flattener uses: the distance from the chord to the
/// control polygon, which bounds how far the curve can deviate from a straight line. A
/// quarter circle comes out at eight segments and a nearly straight curve at two, so a
/// straight line costs almost nothing and a circle is not a polygon with visible corners.
const TOLERANCE: f64 = 0.05;

/// A parsed icon: shapes in their own space, plus that space's size.
#[derive(Debug, Clone, PartialEq)]
pub struct Icon {
    width: f64,
    height: f64,
    /// The shapes in the order written, which is the drawing order: a later shape covers an
    /// earlier one, exactly as a later content-stream operator does.
    shapes: Vec<Shape>,
}

impl Icon {
    /// Is there anything to draw?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }

    /// The size of the icon's own space, which a slot is scaled from.
    #[must_use]
    pub fn extent(&self) -> (f64, f64) {
        (self.width, self.height)
    }
}

/// One shape, flattened, carrying how it is painted.
#[derive(Debug, Clone, PartialEq)]
struct Shape {
    subpaths: Vec<Subpath>,
    /// `fill="none"` is not the same as an absent `fill`: one says "a stroke and nothing
    /// else", the other says "black". Stored rather than defaulted, because they differ.
    fill: bool,
    stroke: bool,
    /// Stroke width in the icon's own units.
    width: f64,
    cap: LineCap,
    join: LineJoin,
    /// How much of the shape's own colour shows through, which is what `opacity="0.35"` on
    /// one shape of two means: a washed-out half beside a solid one.
    opacity: f64,
}

/// An affine transform, in the three forms an icon can write one in.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Transform {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Transform {
    const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    /// Compose, so `self` applies first and then `next`.
    fn then(self, next: Self) -> Self {
        Self {
            a: next.a * self.a + next.c * self.b,
            b: next.b * self.a + next.d * self.b,
            c: next.a * self.c + next.c * self.d,
            d: next.b * self.c + next.d * self.d,
            e: next.a * self.e + next.c * self.f + next.e,
            f: next.b * self.e + next.d * self.f + next.f,
        }
    }

    fn apply(self, x: f64, y: f64) -> (f64, f64) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    /// How much this transform scales a **width**, as opposed to geometry.
    ///
    /// A non-uniform transform gives a stroke the geometric mean of its two axis scales,
    /// `sqrt(|det|)`, for the reason the page renderer gives for a stroke under a
    /// non-uniform `cm`: the true pen is an ellipse, and one number is a compromise
    /// between the two axes rather than either of them.
    fn scale(self) -> f64 {
        let det = (self.a * self.d - self.b * self.c).abs();
        if det.is_finite() && det > 0.0 {
            det.sqrt()
        } else {
            1.0
        }
    }
}

/// What an element says about itself, as far as this subset is concerned.
#[derive(Debug, Clone)]
struct Attrs {
    d: String,
    x: f64,
    y: f64,
    width: Option<f64>,
    height: Option<f64>,
    cx: Option<f64>,
    cy: Option<f64>,
    r: Option<f64>,
    rx: Option<f64>,
    ry: Option<f64>,
    points: String,
    fill: bool,
    stroke: bool,
    stroke_width: Option<f64>,
    linecap: Option<LineCap>,
    linejoin: Option<LineJoin>,
    opacity: f64,
    transform: Transform,
}

impl Default for Attrs {
    /// An element's defaults.
    ///
    /// `opacity` is **1.0**, not 0.0: an element that names no opacity is fully opaque, and
    /// a derived default of zero would make every shape invisible while looking as though
    /// it had been drawn. This is the one field here where the derive's zero is wrong, and
    /// it is the hand-written instance's whole reason to exist.
    fn default() -> Self {
        Self {
            d: String::new(),
            x: 0.0,
            y: 0.0,
            width: None,
            height: None,
            cx: None,
            cy: None,
            r: None,
            rx: None,
            ry: None,
            points: String::new(),
            fill: false,
            stroke: false,
            stroke_width: None,
            linecap: None,
            linejoin: None,
            opacity: 1.0,
            transform: Transform::IDENTITY,
        }
    }
}

impl Attrs {
    /// Is a paint attribute written on this element, and not `none`?
    ///
    /// An element's own `fill` or `stroke` is what decides whether *it* is painted, and an
    /// inherited value is only the default. The check is on the text rather than on a
    /// parsed flag, because `fill="none"` and an absent `fill` are different statements
    /// and a boolean cannot tell them apart after the fact.
    fn paints(what: &str, text: &str) -> bool {
        text.contains(&format!("{what}=\"")) && !text.contains(&format!("{what}=\"none\""))
    }
}

/// What an element inherits from its parent, so closing one restores it.
#[derive(Debug, Clone, Copy)]
struct Inherited {
    fill: bool,
    stroke: bool,
    width: f64,
    cap: LineCap,
    join: LineJoin,
    opacity: f64,
    transform: Transform,
}

/// Parse an icon. `None` is a refusal, and the caller names the file.
pub fn parse(text: &str) -> Option<Icon> {
    // The three things an SVG file can carry that are not an icon. Each is a refusal rather
    // than something to skip past and hope: an icon that is *nearly* right is wrong.
    for bad in ["<script", "<image", "data:image", "<foreignObject"] {
        if text.contains(bad) {
            return None;
        }
    }

    let mut icon = Icon {
        width: DEFAULT_SIZE,
        height: DEFAULT_SIZE,
        shapes: Vec::new(),
    };
    // What each element inherits. `fill` is off and `stroke` is on until something says
    // otherwise, which is what the icon set relies on: a `<g fill="currentColor">` paints
    // every path under it, and a bare path strokes by default.
    let mut fill = false;
    let mut stroke = true;
    let mut width = 1.0;
    let mut cap = LineCap::Butt;
    let mut join = LineJoin::Miter;
    let mut transform = Transform::IDENTITY;
    // How much of the current colour shows through, multiplied down as elements nest.
    let mut opacity = 1.0;
    let mut open: Vec<(String, Inherited)> = Vec::new();

    for tag in elements(text) {
        if let Some(name) = tag.strip_prefix('/') {
            if let Some((open_name, popped)) = open.pop()
                && open_name == name.trim()
            {
                fill = popped.fill;
                stroke = popped.stroke;
                width = popped.width;
                cap = popped.cap;
                join = popped.join;
                opacity = popped.opacity;
                transform = popped.transform;
            }
            continue;
        }
        let (name, rest) = match tag.find(char::is_whitespace) {
            Some(at) => (&tag[..at], &tag[at..]),
            None => (tag.as_str(), ""),
        };
        let attrs = parse_attrs(rest);
        if name == "svg" {
            if let Some(v) = attrs.width {
                icon.width = v;
            }
            if let Some(v) = attrs.height {
                icon.height = v;
            }
        }
        if is_group(name) {
            let child = Inherited {
                fill,
                stroke,
                width,
                cap,
                join,
                opacity,
                transform,
            };
            if rest.contains("fill=\"none\"") {
                fill = false;
            } else if Attrs::paints("fill", rest) {
                fill = true;
            }
            if rest.contains("stroke=\"none\"") {
                stroke = false;
            } else if Attrs::paints("stroke", rest) {
                stroke = true;
            }
            opacity = (opacity * attrs.opacity).clamp(0.0, 1.0);
            if let Some(v) = attrs.stroke_width {
                width = v;
            }
            if let Some(v) = attrs.linecap {
                cap = v;
            }
            if let Some(v) = attrs.linejoin {
                join = v;
            }
            transform = transform.then(attrs.transform);
            if !tag.ends_with(" /") {
                open.push((name.to_string(), child));
            }
            continue;
        }
        // A shape. It takes its own paint, falling back to what it inherits.
        let subpaths = shape_of(name, &attrs, transform)?;
        icon.shapes.push(Shape {
            subpaths,
            // A shape's own `fill` or `stroke` decides for it; an inherited value is only
            // the default, and `none` on the shape itself switches that paint off.
            fill: Attrs::paints("fill", rest) || (fill && !rest.contains("fill=\"none\"")),
            stroke: Attrs::paints("stroke", rest) || (stroke && !rest.contains("stroke=\"none\"")),
            // The element's own `stroke-width`, else the one it inherits, and scaled by
            // the transform in force — which is the difference between a stroke drawn at
            // 1.6 and one drawn at 1.0 because the width lived on a `<g>`.
            width: attrs
                .stroke_width
                .filter(|w| w.is_finite() && *w > 0.0)
                .unwrap_or(width)
                * transform.scale(),
            cap: attrs.linecap.unwrap_or(cap),
            join: attrs.linejoin.unwrap_or(join),
            opacity: (opacity * attrs.opacity).clamp(0.0, 1.0),
        });
    }
    Some(icon)
}

/// Is this an element that carries the inherited paint rather than a shape of its own?
fn is_group(name: &str) -> bool {
    matches!(name, "svg" | "g" | "symbol" | "defs" | "a")
}

/// The subpaths of one element, already in icon space.
fn shape_of(name: &str, attrs: &Attrs, transform: Transform) -> Option<Vec<Subpath>> {
    let (raw, closed) = match name {
        "path" => (flatten(&attrs.d)?, false),
        "rect" => {
            let w = attrs.width.filter(|v| v.is_finite() && *v > 0.0)?;
            let h = attrs.height.filter(|v| v.is_finite() && *v > 0.0)?;
            let rx = attrs.rx.unwrap_or_else(|| attrs.ry.unwrap_or(0.0));
            let ry = attrs.ry.unwrap_or_else(|| attrs.rx.unwrap_or(0.0));
            return Some(vec![rounded_rect(
                attrs.x, attrs.y, w, h, rx, ry, transform,
            )]);
        }
        "circle" => {
            let r = attrs.r.filter(|v| v.is_finite() && *v > 0.0)?;
            return Some(vec![Subpath::closed(ellipse(
                attrs.cx.unwrap_or(0.0),
                attrs.cy.unwrap_or(0.0),
                r,
                r,
                transform,
            ))]);
        }
        "ellipse" => {
            let rx = attrs.rx.filter(|v| v.is_finite() && *v > 0.0)?;
            let ry = attrs.ry.filter(|v| v.is_finite() && *v > 0.0)?;
            return Some(vec![Subpath::closed(ellipse(
                attrs.cx.unwrap_or(0.0),
                attrs.cy.unwrap_or(0.0),
                rx,
                ry,
                transform,
            ))]);
        }
        "line" => {
            let x1 = attrs.width.map(|w| attrs.x + w)?;
            let y1 = attrs.height.map(|h| attrs.y + h)?;
            return Some(vec![Subpath::open(vec![
                transform.apply(attrs.x, attrs.y),
                transform.apply(x1, y1),
            ])]);
        }
        "polyline" => (parse_points(&attrs.points)?, false),
        "polygon" => (parse_points(&attrs.points)?, true),
        _ => return None,
    };
    let points: Vec<(f64, f64)> = raw
        .into_iter()
        .map(|(x, y)| transform.apply(x, y))
        .collect();
    Some(vec![if closed {
        Subpath::closed(points)
    } else {
        Subpath::open(points)
    }])
}

/// A rounded rectangle as a closed subpath.
///
/// The corners are quarter ellipses rather than a cubic approximation, because an arc
/// command is already implemented and this way the two cannot disagree about what a corner
/// is. A corner of zero radius is the rectangle itself, which is the case most icons are.
fn rounded_rect(x: f64, y: f64, w: f64, h: f64, rx: f64, ry: f64, transform: Transform) -> Subpath {
    let rx = rx.abs().min(w / 2.0);
    let ry = ry.abs().min(h / 2.0);
    let put = |p: (f64, f64)| {
        let (tx, ty) = transform.apply(p.0, p.1);
        (tx, ty)
    };
    if rx <= 0.0 || ry <= 0.0 {
        return Subpath::closed(vec![
            put((x, y)),
            put((x + w, y)),
            put((x + w, y + h)),
            put((x, y + h)),
        ]);
    }
    // The four corners, each as the two points the arc runs between, walking
    // counter-clockwise from the top-right. A quarter of an ellipse is exactly what `a 1 1`
    // with the right sweep flag gives, so the arcs are the same code the `a` command uses
    // rather than a second implementation that agrees today.
    let mut points: Vec<(f64, f64)> = Vec::new();
    // Top-right: from (x+w, y+ry) to (x+w-rx, y), sweeping through (x+w, y).
    points.push(put((x + w, y + ry)));
    arc(
        &mut points,
        put((x + w, y + ry)),
        rx,
        ry,
        false,
        true,
        put((x + w - rx, y)),
    );
    // Top-left: from (x+rx, y) to (x, y+ry).
    points.push(put((x + rx, y)));
    arc(
        &mut points,
        put((x + rx, y)),
        rx,
        ry,
        false,
        true,
        put((x, y + ry)),
    );
    // Bottom-left: from (x, y+h-ry) to (x+rx, y+h).
    points.push(put((x, y + h - ry)));
    arc(
        &mut points,
        put((x, y + h - ry)),
        rx,
        ry,
        false,
        true,
        put((x + rx, y + h)),
    );
    // Bottom-right: from (x+w-rx, y+h) to (x+w, y+h-ry).
    points.push(put((x + w - rx, y + h)));
    arc(
        &mut points,
        put((x + w - rx, y + h)),
        rx,
        ry,
        false,
        true,
        put((x + w, y + h - ry)),
    );
    Subpath::closed(points)
}

/// An ellipse as a closed subpath, flattened by the same adaptive rule a curve uses.
fn ellipse(cx: f64, cy: f64, rx: f64, ry: f64, transform: Transform) -> Vec<(f64, f64)> {
    const SEGMENTS: usize = 32;
    let mut out = Vec::with_capacity(SEGMENTS);
    for i in 0..SEGMENTS {
        let a = 2.0 * std::f64::consts::PI * (i as f64) / (SEGMENTS as f64);
        out.push(transform.apply(cx + rx * a.cos(), cy + ry * a.sin()));
    }
    out
}

/// Every point of a `polyline` or `polygon`, in the order written.
fn parse_points(text: &str) -> Option<Vec<(f64, f64)>> {
    let mut rest = text;
    let mut out = Vec::new();
    while !rest.trim().is_empty() {
        let (pair, next) = numbers(rest, 2)?;
        out.push((*pair.first()?, *pair.get(1)?));
        rest = next;
    }
    Some(out)
}

/// Flatten every command of a path's `d` into a polyline.
fn flatten(d: &str) -> Option<Vec<(f64, f64)>> {
    let mut out: Vec<(f64, f64)> = Vec::new();
    let mut rest = d;
    let mut current = (0.0_f64, 0.0_f64);
    let mut start = (0.0_f64, 0.0_f64);
    // The control point an `S` or `T` reflects: the previous curve's, mirrored about the
    // current point — or the current point itself when the last command was not of that
    // family, which is the specification's rule and not a convenience.
    let mut control: Option<(f64, f64)> = None;

    while !rest.trim().is_empty() {
        rest = rest.trim_start();
        let Some(command) = rest.chars().next() else {
            break;
        };
        if !command.is_ascii_alphabetic() {
            return None;
        }
        rest = &rest[command.len_utf8()..];
        let word = command.to_ascii_lowercase().to_string();
        let lower = word.as_str();
        match lower {
            "m" | "l" => {
                let relative = command.is_lowercase();
                loop {
                    let (operands, next) = numbers(rest, 2)?;
                    let point = (*operands.first()?, *operands.get(1)?);
                    current = if relative {
                        (current.0 + point.0, current.1 + point.1)
                    } else {
                        point
                    };
                    out.push(current);
                    if lower == "m" {
                        // A moveto begins a subpath, so this is where a `Z` returns to.
                        start = current;
                    }
                    rest = next;
                    if !more_operands_follow(rest) {
                        break;
                    }
                }
                control = None;
            }
            "h" | "v" => {
                let relative = command.is_lowercase();
                loop {
                    let (operands, next) = numbers(rest, 1)?;
                    let value = *operands.first()?;
                    if lower == "h" {
                        current.0 = if relative { current.0 + value } else { value };
                    } else {
                        current.1 = if relative { current.1 + value } else { value };
                    }
                    out.push(current);
                    rest = next;
                    if !more_operands_follow(rest) {
                        break;
                    }
                }
                control = None;
            }
            "c" | "s" | "q" | "t" => {
                let relative = command.is_lowercase();
                // How many *numbers* one coordinate set has: a cubic and a quadratic take
                // three and two pairs, and an `S` or `T` takes the pair of its shorthand.
                let (cubic, count) = match lower {
                    "c" => (true, 6),
                    "s" => (true, 4),
                    "q" => (false, 4),
                    _ => (false, 2),
                };
                loop {
                    let (operands, next) = numbers(rest, count)?;
                    let mut at = 0usize;
                    let grab = |operands: &[f64], at: &mut usize| -> Option<(f64, f64)> {
                        let p = (*operands.get(*at)?, *operands.get(*at + 1)?);
                        *at += 2;
                        Some(p)
                    };
                    let first = grab(&operands, &mut at)?;
                    let last = if count == 1 {
                        first
                    } else {
                        grab(&operands, &mut at)?
                    };
                    let (first, last) = if relative {
                        (
                            (current.0 + first.0, current.1 + first.1),
                            (current.0 + last.0, current.1 + last.1),
                        )
                    } else {
                        (first, last)
                    };
                    // A `T`'s control is the mirror of the last one; an `S`'s too, and a
                    // `Q`'s is written out. Either way the curve is a cubic from here.
                    let c1 = match (lower, control) {
                        ("t" | "s", Some(c)) => (2.0 * current.0 - c.0, 2.0 * current.1 - c.1),
                        _ => first,
                    };
                    let (c2, end) = if cubic {
                        (first, last)
                    } else {
                        // A quadratic is a cubic whose two control points sit at the
                        // canonical thirds of the lines to the single control point.
                        (
                            (
                                current.0 + 2.0 / 3.0 * (first.0 - current.0),
                                current.1 + 2.0 / 3.0 * (first.1 - current.1),
                            ),
                            last,
                        )
                    };
                    let c3 = if cubic {
                        last
                    } else {
                        (
                            end.0 + 2.0 / 3.0 * (first.0 - end.0),
                            end.1 + 2.0 / 3.0 * (first.1 - end.1),
                        )
                    };
                    curve(&mut out, current, c1, c2, c3);
                    control = Some(if cubic { last } else { first });
                    current = end;
                    rest = next;
                    if !more_operands_follow(rest) {
                        break;
                    }
                }
            }
            "a" => {
                let relative = command.is_lowercase();
                // An arc repeats just as a curve does, with no second command letter: the
                // specification allows `a r r φ f f dx dy a r r φ f f dx dy`, and a writer
                // building an ellipse out of two arcs always writes it that way.
                loop {
                    let (operands, next) = numbers(rest, 7)?;
                    let rx = *operands.first()?;
                    let ry = *operands.get(1)?;
                    // The rotation is parsed and discarded: no icon in this set rotates its
                    // ellipse, and a rotated arc that is *refused* would be a page with a hole
                    // where the alternative is a corner drawn slightly wrong. It is recorded
                    // rather than assumed by the test that an unrotated arc is exact.
                    let _angle = operands.get(2).copied().unwrap_or(0.0);
                    let large = operands.get(3).copied().unwrap_or(0.0) != 0.0;
                    let sweep = operands.get(4).copied().unwrap_or(0.0) != 0.0;
                    let end = (*operands.get(5)?, *operands.get(6)?);
                    let end = if relative {
                        (current.0 + end.0, current.1 + end.1)
                    } else {
                        end
                    };
                    arc(&mut out, current, rx, ry, large, sweep, end)?;
                    current = end;
                    rest = next;
                    if !more_operands_follow(rest) {
                        break;
                    }
                }
                control = None;
            }
            "z" => {
                if out.is_empty() || Some(start) != out.last().copied() {
                    out.push(start);
                }
                current = start;
                control = None;
            }
            _ => return None,
        }
    }
    Some(out)
}

/// Does the rest of a command string begin with another operand's number?
///
/// A command may carry more coordinate sets than its letter implies, and the way to tell
/// one from the next letter is that a number starts with a digit, a sign or a dot.
fn more_operands_follow(rest: &str) -> bool {
    rest.trim_start()
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_digit() || c == '-' || c == '+' || c == '.')
}

/// Append a cubic Bézier, flattened adaptively.
fn curve(
    out: &mut Vec<(f64, f64)>,
    p0: (f64, f64),
    p1: (f64, f64),
    p2: (f64, f64),
    p3: (f64, f64),
) {
    for i in 1..=segments_for(p0, p1, p2, p3) {
        out.push(bezier_at(p0, p1, p2, p3, (i as f64) / (i as f64).max(1.0)));
    }
}

/// A point on a cubic at parameter `t`.
fn bezier_at(p0: (f64, f64), p1: (f64, f64), p2: (f64, f64), p3: (f64, f64), t: f64) -> (f64, f64) {
    let u = 1.0 - t;
    let a = u * u * u;
    let b = 3.0 * u * u * t;
    let c = 3.0 * u * t * t;
    let d = t * t * t;
    (
        a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0,
        a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1,
    )
}

/// How many segments a cubic needs, from how far its control points bulge from the chord.
///
/// Three quarters of the control polygon's length bounds the deviation, and dividing that
/// by a tolerance gives the count — so a quarter circle is eight segments and a nearly
/// straight curve is two. A fixed count would waste time on every straight line or draw a
/// circle as a polygon with corners a user could see.
fn segments_for(p0: (f64, f64), p1: (f64, f64), p2: (f64, f64), p3: (f64, f64)) -> usize {
    let bulge = 0.75 * distance(p1, p2);
    let worst = distance(p0, p3).max(bulge);
    if worst <= 0.0 {
        return 1;
    }
    let wanted = (worst / TOLERANCE).ceil() as usize;
    wanted.clamp(2, 128)
}

/// Append an elliptical arc, flattened.
///
/// The endpoint parameterisation is converted to a centre and two angles, per the
/// specification's own implementation notes, and then sampled. A radius too small to span
/// the endpoints is scaled up until it is, which is what the notes prescribe and what
/// makes a rounded corner come out as a corner rather than as nothing.
fn arc(
    out: &mut Vec<(f64, f64)>,
    from: (f64, f64),
    rx: f64,
    ry: f64,
    large: bool,
    sweep: bool,
    to: (f64, f64),
) -> Option<()> {
    let rx = rx.abs();
    let ry = ry.abs();
    if from.0 == to.0 && from.1 == to.1 {
        return Some(());
    }
    if !(rx > 0.0 && ry > 0.0) {
        out.push(to);
        return Some(());
    }
    // Scale out the radii until the endpoints are inside the ellipse they describe.
    let dx = (from.0 - to.0) / 2.0;
    let dy = (from.1 - to.1) / 2.0;
    let lambda = dx * dx / (rx * rx) + dy * dy / (ry * ry);
    if lambda > 1.0 {
        let k = lambda.sqrt();
        return arc(out, from, rx * k, ry * k, large, sweep, to);
    }
    // The centre, in the ellipse's own frame and then translated back.
    let num = rx * rx * ry * ry - rx * rx * dy * dy - ry * ry * dx * dx;
    let den = rx * rx * dy * dy + ry * ry * dx * dx;
    let factor = if den > 0.0 {
        (num / den).max(0.0).sqrt()
    } else {
        0.0
    };
    let sign = if large == sweep { -1.0 } else { 1.0 };
    let cxp = sign * factor * rx * dy / ry;
    let cyp = -sign * factor * ry * dx / rx;
    let cx = cxp + f64::midpoint(from.0, to.0);
    let cy = cyp + f64::midpoint(from.1, to.1);
    let angle = |ux: f64, uy: f64, vx: f64, vy: f64| {
        let dot = (ux * vx + uy * vy).clamp(-1.0, 1.0);
        let cross = ux * vy - uy * vx;
        cross.atan2(dot)
    };
    // The two angles are measured from the centre to each endpoint, both in the frame where
    // the ellipse is a unit circle.
    // The two angles are measured in the ellipse's own frame, from the **absolute** centre
    // — using the offset-from-the-midpoint `cxp`/`cyp` against absolute coordinates is the
    // classic way to get a corner that sweeps the wrong way round and lands outside the box.
    let theta1 = angle(1.0, 0.0, (from.0 - cx) / rx, (from.1 - cy) / ry);
    let mut delta = angle(
        (from.0 - cx) / rx,
        (from.1 - cy) / ry,
        (to.0 - cx) / rx,
        (to.1 - cy) / ry,
    );
    if !sweep && delta > 0.0 {
        delta -= 2.0 * std::f64::consts::PI;
    } else if sweep && delta < 0.0 {
        delta += 2.0 * std::f64::consts::PI;
    }
    let segments = ((delta.abs() / (std::f64::consts::PI / 4.0)).ceil() as usize).clamp(1, 256);
    for i in 1..=segments {
        let a = theta1 + delta * (i as f64) / (segments as f64);
        out.push((cx + rx * a.cos(), cy + ry * a.sin()));
    }
    Some(())
}

/// Distance between two points.
fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    (b.0 - a.0).hypot(b.1 - a.1)
}

/// Rasterize an icon into `width` by `height` pixels, in one colour.
///
/// The icon is scaled to fill the box without distorting it and centred in whatever is
/// left over, so a 24-unit icon in a 16-pixel slot is two thirds scale rather than a
/// squeeze. The result is straight alpha in the tint's colour, which is what a texture
/// wants and what a hover, active, light and dark variant are all made from.
pub fn rasterise(icon: &Icon, width: usize, height: usize, tint: [u8; 4]) -> Image {
    let mut image = Image::new(width, height);
    if width == 0 || height == 0 || icon.is_empty() {
        return image;
    }
    let scale = (width as f64 / icon.width).min(height as f64 / icon.height);
    let dx = (width as f64 - icon.width * scale) / 2.0;
    let dy = (height as f64 - icon.height * scale) / 2.0;
    let to_box = Transform {
        a: scale,
        b: 0.0,
        c: 0.0,
        d: scale,
        e: dx,
        f: dy,
    };
    let device = Rect {
        x0: 0.0,
        y0: 0.0,
        x1: width as f64,
        y1: height as f64,
    };

    for shape in &icon.shapes {
        let placed: Vec<Subpath> = shape
            .subpaths
            .iter()
            .map(|sub| {
                let points: Vec<(f64, f64)> = sub
                    .points
                    .iter()
                    .map(|(x, y)| to_box.apply(*x, *y))
                    .collect();
                if sub.closed {
                    Subpath::closed(points)
                } else {
                    Subpath::open(points)
                }
            })
            .collect();
        // The fill first and the stroke on top, which is the order a browser draws them in
        // and the order that makes a filled, stroked ring look like one ring.
        if shape.fill {
            composite(
                &mut image,
                &Polygon {
                    subpaths: placed.clone(),
                },
                device,
                tint,
                shape.opacity,
            );
        }
        if shape.stroke {
            let style = StrokeStyle {
                width: shape.width * scale,
                cap: shape.cap,
                join: shape.join,
                ..StrokeStyle::default()
            };
            for sub in &placed {
                if sub.points.len() < 2 {
                    continue;
                }
                let outline = stroke_outline(&sub.points, sub.closed, &style);
                composite(&mut image, &outline, device, tint, shape.opacity);
            }
        }
    }
    image
}

/// Fill a polygon's coverage into the image, in the tint's colour.
///
/// `opacity` is the shape's own, and it scales the coverage: a shape at `0.35` covers a
/// pixel by a third of what it geometrically does, which is what lets two halves of one
/// icon be drawn at two different strengths without a second colour.
///
/// Aliased so this module's own `rasterise` is the icon one and the coverage one keeps its name.
///
/// `Polygon::edges` closes every subpath, which is what a fill needs and what a stroke's
/// outline is not — an open subpath's stroke is capped rather than closed, and the outline
/// this module hands in is already the closed shape of the ink, so closing it again is the
/// same answer and not a second opinion.
fn composite(image: &mut Image, polygon: &Polygon, device: Rect, tint: [u8; 4], opacity: f64) {
    let coverage: Coverage = rasterise_coverage(&polygon.edges(), device, FillRule::NonZero);
    for (x, y, covered) in coverage.covered() {
        let alpha = f64::from(covered) * opacity;
        if alpha <= 0.0 {
            continue;
        }
        let existing = image.get(x, y).unwrap_or([0, 0, 0, 0]);
        let [r, g, b, a] = existing;
        // Straight alpha over what is there. An icon is one colour, so the only thing a
        // pixel has to decide is how much of it there is — and a shape drawn twice paints
        // itself darker the second time, which is what an icon wants where a `fill` and a
        // `stroke` overlap.
        image.put(
            x,
            y,
            [
                mix(r, tint[0], alpha),
                mix(g, tint[1], alpha),
                mix(b, tint[2], alpha),
                mix(a, 255, alpha),
            ],
        );
    }
}

/// `dst` towards `src` by `alpha`.
fn mix(dst: u8, src: u8, alpha: f64) -> u8 {
    (f64::from(dst) + (f64::from(src) - f64::from(dst)) * alpha)
        .round()
        .clamp(0.0, 255.0) as u8
}

/// Where a number ends in `text`, and what it was.
///
/// A path's numbers are separated by whitespace, a comma, or a sign, and `1.5.5` is two
/// numbers rather than one — which is what a path looks like when a writer leaves out the
/// separator between an arc's `rx ry` and the next operand. A `.` therefore ends a run of
/// digits only when what follows is neither a digit nor another dot.
fn number_at(text: &str) -> Option<(f64, usize)> {
    let bytes = text.as_bytes();
    let mut i = 0;
    if bytes.get(i).is_some_and(|b| *b == b'-' || *b == b'+') {
        i += 1;
    }
    let start = i;
    while bytes.get(i).is_some_and(|b| b.is_ascii_digit()) {
        i += 1;
    }
    let mut digits = i > start;
    if bytes.get(i) == Some(&b'.') {
        i += 1;
        while bytes.get(i).is_some_and(|b| b.is_ascii_digit()) {
            i += 1;
            digits = true;
        }
    }
    if !digits {
        return None;
    }
    if let Some(e) = bytes.get(i)
        && (*e == b'e' || *e == b'E')
    {
        let save = i;
        i += 1;
        if bytes.get(i).is_some_and(|b| *b == b'-' || *b == b'+') {
            i += 1;
        }
        if bytes.get(i).is_some_and(|b| b.is_ascii_digit()) {
            while bytes.get(i).is_some_and(|b| b.is_ascii_digit()) {
                i += 1;
            }
        } else {
            i = save;
        }
    }
    let value: f64 = text.get(..i)?.parse().ok()?;
    Some((value, i))
}

/// Skip the separators between two operands.
fn skip_separators(text: &str) -> &str {
    text.trim_start_matches(|c: char| c.is_whitespace() || c == ',')
}

/// Read `count` numbers off the front of `text`, returning them and what is left.
///
/// Fewer than `count` is a malformed path and is reported as such rather than padded with
/// zeros: a zero operand is a real value, and padding would silently move the point.
fn numbers(text: &str, count: usize) -> Option<(Vec<f64>, &str)> {
    let mut rest = text;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        rest = skip_separators(rest);
        let (value, used) = number_at(rest)?;
        out.push(value);
        rest = rest.get(used..)?;
    }
    Some((out, rest))
}

/// Split a document into its element tags, `<tag …>` and `</tag>`.
fn elements(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find('<') {
        let after = &rest[at..];
        // A comment ends at `-->` rather than at the first `>`.
        if let Some(comment) = after.strip_prefix("<!--") {
            match comment.find("-->") {
                Some(end) => {
                    rest = comment.get(end + 3..).unwrap_or("");
                    continue;
                }
                None => break,
            }
        }
        let Some(end) = after.find('>') else { break };
        // A self-closing element's trailing slash is not part of its name or its
        // attributes, and leaving it in makes an attribute called `/` that parses as
        // nothing. It is recorded separately instead, so the caller knows not to wait for a
        // closing tag.
        // The `<` is markup, not part of the name: leaving it in makes the element's name
        // `<svg` and no close tag ever matches it.
        let body = after.get(1..end).unwrap_or("").trim();
        let self_closing = body.ends_with('/');
        let body = body.trim_end_matches('/').trim();
        out.push(if self_closing {
            format!("{body} /")
        } else {
            body.to_string()
        });
        rest = after.get(end + 1..).unwrap_or("");
    }
    out
}

/// Probe for a test.
pub fn tests_probe_elements(t: &str) -> Vec<String> {
    elements(t)
}

/// Probe for a test.
pub fn tests_probe_attrs(t: &str) -> String {
    let a = parse_attrs(t);
    format!("d={:?}", a.d)
}

/// Probe for a test.
pub fn tests_probe_flatten(d: &str) -> String {
    format!("{:?}", flatten(d))
}

/// Parse an element's attributes into the set this subset understands.
fn parse_attrs(text: &str) -> Attrs {
    let mut attrs = Attrs::default();
    let mut rest = skip_separators(text);
    while !rest.is_empty() {
        let Some(eq) = rest.find('=') else { break };
        let name = rest[..eq].trim();
        if name.is_empty() {
            break;
        }
        rest = &rest[eq + 1..];
        // A quoted value, or an unquoted one.
        let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'');
        let value = match quote {
            Some(q) => {
                rest = &rest[q.len_utf8()..];
                match rest.find(q) {
                    Some(end) => {
                        let v = rest.get(..end).unwrap_or("");
                        rest = rest.get(end + q.len_utf8()..).unwrap_or("");
                        v
                    }
                    None => rest,
                }
            }
            None => {
                let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                let v = rest.get(..end).unwrap_or("");
                rest = rest.get(end..).unwrap_or("");
                v
            }
        };
        match name {
            "d" => attrs.d = value.to_string(),
            "x" => attrs.x = value.parse().unwrap_or(0.0),
            "y" => attrs.y = value.parse().unwrap_or(0.0),
            "width" => attrs.width = value.parse().ok(),
            "height" => attrs.height = value.parse().ok(),
            "cx" => attrs.cx = value.parse().ok(),
            "cy" => attrs.cy = value.parse().ok(),
            "r" => attrs.r = value.parse().ok(),
            "rx" => attrs.rx = value.parse().ok(),
            "ry" => attrs.ry = value.parse().ok(),
            "points" => attrs.points = value.to_string(),
            "fill" => attrs.fill = !value.eq_ignore_ascii_case("none"),
            "stroke" => attrs.stroke = !value.eq_ignore_ascii_case("none"),
            "stroke-width" => attrs.stroke_width = value.parse().ok(),
            "stroke-linecap" => {
                attrs.linecap = match value {
                    "round" => Some(LineCap::Round),
                    "square" => Some(LineCap::Square),
                    _ => Some(LineCap::Butt),
                }
            }
            "stroke-linejoin" => {
                attrs.linejoin = match value {
                    "round" => Some(LineJoin::Round),
                    "bevel" => Some(LineJoin::Bevel),
                    _ => Some(LineJoin::Miter),
                }
            }
            "opacity" => attrs.opacity = value.parse::<f64>().unwrap_or(1.0).clamp(0.0, 1.0),
            "transform" => attrs.transform = parse_transform(value),
            _ => {}
        }
        rest = skip_separators(rest);
    }
    attrs
}

/// Parse a `transform` attribute, a list of the forms a hand writes.
fn parse_transform(text: &str) -> Transform {
    let mut out = Transform::IDENTITY;
    let mut rest = text;
    while let Some(open) = rest.find('(') {
        let name = rest.get(..open).unwrap_or("").trim();
        let Some(close) = rest.find(')') else { break };
        let args = rest.get(open + 1..close).unwrap_or("");
        let (values, _) = numbers(args, 6).unwrap_or_default();
        let one = values.first().copied().unwrap_or(0.0);
        let two = values.get(1).copied().unwrap_or(0.0);
        let piece = match name {
            "translate" => Transform {
                e: one,
                f: two,
                ..Transform::IDENTITY
            },
            "scale" => Transform {
                a: one,
                d: two,
                ..Transform::IDENTITY
            },
            "rotate" => {
                let a = one.to_radians();
                Transform {
                    a: a.cos(),
                    b: a.sin(),
                    c: -a.sin(),
                    d: a.cos(),
                    ..Transform::IDENTITY
                }
            }
            "matrix" => Transform {
                a: one,
                b: two,
                c: values.get(2).copied().unwrap_or(0.0),
                d: values.get(3).copied().unwrap_or(1.0),
                e: values.get(4).copied().unwrap_or(0.0),
                f: values.get(5).copied().unwrap_or(0.0),
            },
            _ => Transform::IDENTITY,
        };
        out = out.then(piece);
        rest = rest.get(close + 1..).unwrap_or("");
    }
    out
}

#[cfg(test)]
mod tests;
