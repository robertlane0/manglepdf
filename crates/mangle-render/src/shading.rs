//! Shadings: the gradients a page can fill a region with.
//!
//! Two things are here, and they are separable on purpose:
//!
//! 1. **Shadings.** An axial or a radial gradient: two endpoints in a space, a function from
//!    position to colour, and whether the gradient continues past its ends.
//! 2. **Painting.** Each pixel of the clip is mapped *back* into the shading's own space,
//!    turned into a parameter `t` between zero and one, and coloured by the function.
//!
//! The third thing this file used to hold — the PDF **functions** themselves — lives in
//! [`mangle_content::function`], because a `/Separation`'s `/TintTransform` is one and the
//! conversion that needs it is in `mangle-content`. They are re-exported here unchanged, so
//! a gradient's function is still reached through `shading`.
//!
//! ## Why the parameter is computed backwards
//!
//! Painting forwards — walk the gradient's axis and fill as you go — works for an
//! axis-aligned gradient and fails for a rotated or skewed one, which every page with a
//! diagonal banner uses. Working backwards from each pixel through the inverse
//! transformation costs one matrix multiply per pixel and has no special cases at all.
//!
//! ## The edge is antialiased
//!
//! A gradient's boundary is a line in the shading's space and an antialiased edge on the
//! page. The distance from a pixel's centre to that boundary is `t / |∇t|`, where `∇t` is
//! how much `t` changes per pixel; evaluating `t` at three points and differencing gives it
//! without any case analysis, which matters because the boundary of a radial gradient is a
//! conic section and has no formula worth writing twice.

use std::ops::Range;

use mangle_content::Matrix;
use mangle_syntax::object::{Dict, Object};

use crate::{Device, Rect};

pub use mangle_content::function::{
    Calculator, Exponential, Function, MAX_PROGRAM, MAX_STACK, Sampled, Stitching, Token,
    lex_program,
};

/// A shading this project can paint.
#[derive(Debug, Clone, PartialEq)]
pub enum Shading {
    /// Type 2: a linear gradient between two points.
    Axial {
        coords: Vec<f64>,
        function: Function,
        extend: [bool; 2],
    },
    /// Type 3: a gradient between two circles.
    Radial {
        coords: Vec<f64>,
        function: Function,
        extend: [bool; 2],
    },
}

impl Shading {
    /// Read a `/Shading` dictionary.
    #[must_use]
    pub fn parse(object: &Object, resolve: &dyn Fn(&Object) -> Option<Object>) -> Option<Self> {
        let raw = object.clone();
        let resolved = resolve(&raw).unwrap_or(raw);
        let (dict, is_stream): (Dict, bool) = match &resolved {
            Object::Dict(d) => (d.clone(), false),
            Object::Stream(s) => (s.dict.clone(), true),
            _ => return None,
        };
        let _ = is_stream;
        let kind = dict.get("ShadingType").and_then(Object::as_i64)?;
        let extend = read_extend(dict.get("Extend"));
        let coords: Vec<f64> = dict
            .get("Coords")
            .and_then(Object::as_array)
            .map(|a| a.iter().filter_map(Object::as_f64).collect())
            .unwrap_or_default();
        let function_object = dict.get("Function")?;
        // `/Function` is either one function or an array of them; an array is stitched
        // together, which is how a gradient with more than two stops is written.
        let function = if let Some(array) = function_object.as_array() {
            let parts: Vec<Function> = array
                .iter()
                .filter_map(|o| Function::parse(o, resolve))
                .collect();
            stitch(
                &parts,
                &dict,
                coords.first().copied().unwrap_or(0.0),
                coords.get(1).copied().unwrap_or(1.0),
            )?
        } else {
            Function::parse(function_object, resolve)?
        };

        match kind {
            2 if coords.len() >= 4 => Some(Self::Axial {
                coords,
                function,
                extend,
            }),
            3 if coords.len() >= 6 => Some(Self::Radial {
                coords,
                function,
                extend,
            }),
            // Types 1, 4, 5, 6 and 7 need either a pattern colour or a mesh this project
            // does not build. Reporting them is better than painting them wrongly.
            _ => None,
        }
    }

    /// The parameter at a point in the shading's own space, or `None` if it is outside.
    ///
    /// `t` is *not* clamped: a caller needs to know how far outside the gradient a point
    /// is in order to work out how much of it is covered. Clamping belongs to the colour
    /// lookup, which is where "the nearest end's colour" is the right answer.
    #[must_use]
    pub fn parameter_at(&self, x: f64, y: f64) -> Option<f64> {
        match self {
            Self::Axial { coords, .. } => {
                let p0 = (coords.first().copied()?, coords.get(1).copied()?);
                let p1 = (coords.get(2).copied()?, coords.get(3).copied()?);
                let d = (p1.0 - p0.0, p1.1 - p0.1);
                let length_squared = d.0 * d.0 + d.1 * d.1;
                if length_squared <= 0.0 {
                    // A zero-length axis has no direction, and every point is at the same
                    // place along it. The specification's answer is the first stop.
                    return Some(0.0);
                }
                let t = ((x - p0.0) * d.0 + (y - p0.1) * d.1) / length_squared;
                Some(t)
            }
            Self::Radial { coords, .. } => {
                let (x0, y0, r0) = (
                    coords.first().copied()?,
                    coords.get(1).copied()?,
                    coords.get(2).copied()?,
                );
                let (x1, y1, r1) = (
                    coords.get(3).copied()?,
                    coords.get(4).copied()?,
                    coords.get(5).copied()?,
                );
                radial_parameter(x, y, x0, y0, r0, x1, y1, r1)
            }
        }
    }

    /// Whether the gradient continues past each end.
    #[must_use]
    pub fn extend(&self) -> [bool; 2] {
        match self {
            Self::Axial { extend, .. } | Self::Radial { extend, .. } => *extend,
        }
    }

    /// Whether a point in the shading's own space lies inside a radial gradient's *first*
    /// circle.
    ///
    /// That disc is painted with the first colour, and the parameter cannot say so: a
    /// point at the centre of the family is on no circle in it, and the quadratic's root
    /// comes out negative. An axial gradient has no such disc and never reports one.
    #[must_use]
    pub fn inner_fill(&self, x: f64, y: f64) -> bool {
        let Self::Radial { coords, .. } = self else {
            return false;
        };
        let cx = coords.first().copied().unwrap_or(0.0);
        let cy = coords.get(1).copied().unwrap_or(0.0);
        let r = coords.get(2).copied().unwrap_or(0.0);
        (cx - x).powi(2) + (cy - y).powi(2) <= r * r
    }

    /// The colour at a parameter, as RGB in 0..1, or `None` if the function cannot answer.
    #[must_use]
    pub fn colour_at(&self, t: f64) -> Option<[f64; 3]> {
        // A point past either end still has a colour and it is that end's. Whether the
        // point is *visible* is the coverage's business, decided from the raw parameter.
        let t = t.clamp(0.0, 1.0);
        let function = match self {
            Self::Axial { function, .. } | Self::Radial { function, .. } => function,
        };
        let values = function.apply1(t)?;
        // A gradient's function produces colour components in the space its `/ColorSpace`
        // names. Everything here is in RGB, and the spaces this paints are RGB and Gray,
        // so a three-component result is RGB and a one-component result is gray.
        match values.len() {
            1 => {
                let v = values.first().copied()?.clamp(0.0, 1.0);
                Some([v, v, v])
            }
            3 => Some([
                values.first().copied()?.clamp(0.0, 1.0),
                values.get(1).copied()?.clamp(0.0, 1.0),
                values.get(2).copied()?.clamp(0.0, 1.0),
            ]),
            4 => {
                // Subtractive, and the components are ink rather than light.
                let c = values.first().copied()?.clamp(0.0, 1.0);
                let m = values.get(1).copied()?.clamp(0.0, 1.0);
                let y = values.get(2).copied()?.clamp(0.0, 1.0);
                let k = values.get(3).copied()?.clamp(0.0, 1.0);
                Some([
                    (1.0 - c) * (1.0 - k),
                    (1.0 - m) * (1.0 - k),
                    (1.0 - y) * (1.0 - k),
                ])
            }
            _ => None,
        }
    }
}

/// The parameter along a radial gradient, by solving the intersection of a ray and a
/// family of circles.
///
/// The radius at parameter `t` is `r0 + t·(r1 − r0)` and the centre is
/// `p0 + t·(p1 − p0)`, so requiring the point to be at distance `r0 + t·dr` from the centre
/// gives a quadratic in `t`. The smaller root is the one the specification wants, because
/// the larger one belongs to the far side of the circle.
#[must_use]
pub fn radial_parameter(
    x: f64,
    y: f64,
    x0: f64,
    y0: f64,
    r0: f64,
    x1: f64,
    y1: f64,
    r1: f64,
) -> Option<f64> {
    let d = (x1 - x0, y1 - y0);
    let f = (x - x0, y - y0);
    let dr = r1 - r0;
    let a = d.0 * d.0 + d.1 * d.1 - dr * dr;
    let b = -2.0 * (f.0 * d.0 + f.1 * d.1 + r0 * dr);
    let c = f.0 * f.0 + f.1 * f.1 - r0 * r0;
    let t = if a.abs() < f64::EPSILON {
        // A cone, where the radii change but the centres do not: the quadratic degenerates
        // to a line and the smaller root is the only one.
        if b.abs() < f64::EPSILON {
            return None;
        }
        -c / b
    } else {
        let discriminant = b * b - 4.0 * a * c;
        if discriminant < 0.0 {
            // The ray misses the cone entirely.
            return None;
        }
        let root = discriminant.sqrt();
        let candidates = [(-b - root) / (2.0 * a), (-b + root) / (2.0 * a)];
        // The specification's rule is the larger of the two `t` values that are at most one.
        // A point past the outer circle has neither root in `[0, 1]`, and then the smallest
        // root above one says "outside" without pretending the gradient ends there.
        let inside = candidates
            .iter()
            .copied()
            .filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
            .fold(f64::NEG_INFINITY, f64::max);
        if inside.is_finite() {
            inside
        } else {
            let beyond = candidates
                .iter()
                .copied()
                .filter(|v| v.is_finite() && *v > 1.0)
                .fold(f64::INFINITY, f64::min);
            if beyond.is_finite() {
                beyond
            } else {
                candidates
                    .iter()
                    .copied()
                    .filter(|v| v.is_finite())
                    .fold(f64::NEG_INFINITY, f64::max)
            }
        }
    };
    if !t.is_finite() {
        return None;
    }
    Some(t)
}

fn read_extend(object: Option<&Object>) -> [bool; 2] {
    let Some(array) = object.and_then(Object::as_array) else {
        return [false, false];
    };
    [
        array.first().and_then(Object::as_bool).unwrap_or(false),
        array.get(1).and_then(Object::as_bool).unwrap_or(false),
    ]
}

/// Combine a run of functions into one, as a `/Function` array means.
fn stitch(parts: &[Function], dict: &Dict, start: f64, end: f64) -> Option<Function> {
    if parts.len() == 1 {
        return parts.first().cloned();
    }
    // An `/Encode` on the array, when present, gives the sub-domains; the even split it
    // defaults to is what a file without one means.
    let mut encode: Vec<[f64; 2]> = Vec::with_capacity(parts.len());
    for i in 0..parts.len() {
        let lo = start + (end - start) * i as f64 / parts.len() as f64;
        let hi = start + (end - start) * (i + 1) as f64 / parts.len() as f64;
        encode.push([0.0, 1.0]);
        let _ = (lo, hi);
    }
    if let Some(array) = dict.get("Function").and_then(Object::as_array) {
        let array: &[Object] = array;
        for (i, item) in array.iter().enumerate() {
            let sub = match item {
                Object::Dict(d) => d.clone(),
                Object::Stream(s) => s.dict.clone(),
                _ => continue,
            };
            if let Some(enc) = sub.get("Encode").and_then(Object::as_array)
                && let Some(slot) = encode.get_mut(i)
                && let (Some(lo), Some(hi)) = (
                    enc.first().and_then(Object::as_f64),
                    enc.get(1).and_then(Object::as_f64),
                )
            {
                *slot = [lo, hi];
            }
        }
    }
    let bounds: Vec<f64> = (1..parts.len())
        .map(|i| start + (end - start) * i as f64 / parts.len() as f64)
        .collect();
    Some(Function::Stitching(Stitching {
        domain: vec![[start, end]],
        functions: parts.to_vec(),
        bounds,
        encode,
    }))
}

/// Paint a shading through a transformation.
///
/// `matrix` maps the shading's own space onto the page. Each pixel of the clip is mapped
/// *back* through its inverse, which is what makes a rotated gradient work with no special
/// cases. Where the clip is a region rather than a box, the pixel's share of that region
/// multiplies the gradient's own coverage.
///
/// The per-pixel work is [`crate::fill::Sampler`]'s, which a pattern fill uses as well. That
/// is deliberate: a gradient reached by `sh` and the same gradient used as a fill colour
/// have to be the same evaluator, or a change to one of them silently stops applying to the
/// other.
pub fn paint(device: &mut Device, shading: &Shading, matrix: &Matrix, alpha: f64) -> bool {
    let area = device.clip();
    let Some((columns, rows)) = area.pixels() else {
        return false;
    };
    let Some(inverse) = matrix.inverse() else {
        return false;
    };
    let sampler = crate::fill::Sampler::Shading {
        shading,
        inverse,
        extend: shading.extend(),
    };
    let mut painted = 0usize;

    // Every pixel is written at its own coordinates, which are the clip's own: a shading
    // inside a clip that does not start at the origin is still painted where it belongs.
    for y in rows.clone() {
        for x in columns.clone() {
            let Some(sample) = sampler.sample(x, y) else {
                continue;
            };
            let a = (sample.coverage * alpha).clamp(0.0, 1.0);
            // The clip mask multiplies the gradient's own coverage rather than deciding it,
            // so a shading inside a diagonal clip has that edge antialiased instead of
            // stopping at it.
            let a = a * device.clip_coverage(x, y).unwrap_or(1.0);
            if a <= 0.0 {
                continue;
            }
            device.put(
                x,
                y,
                [
                    (sample.colour[0].clamp(0.0, 1.0) * 255.0).round() as u8,
                    (sample.colour[1].clamp(0.0, 1.0) * 255.0).round() as u8,
                    (sample.colour[2].clamp(0.0, 1.0) * 255.0).round() as u8,
                    (a * 255.0).round() as u8,
                ],
            );
            painted += 1;
        }
    }
    painted > 0
}

/// The rectangle a shading covers, in the shading's own space.
///
/// A caller may want this to decide what to paint at all, and it is the honest answer for
/// a gradient whose `/Coords` are a bounding box.
#[must_use]
pub fn bounds(shading: &Shading) -> Option<Rect> {
    match shading {
        Shading::Axial { coords, .. } => {
            let x0 = coords.first().copied()?;
            let y0 = coords.get(1).copied()?;
            let x1 = coords.get(2).copied()?;
            let y1 = coords.get(3).copied()?;
            Some(Rect {
                x0: x0.min(x1),
                y0: y0.min(y1),
                x1: x0.max(x1),
                y1: y0.max(y1),
            })
        }
        Shading::Radial { coords, .. } => {
            let (x0, y0, r0) = (
                coords.first().copied()?,
                coords.get(1).copied()?,
                coords.get(2).copied()?,
            );
            let (x1, y1, r1) = (
                coords.get(3).copied()?,
                coords.get(4).copied()?,
                coords.get(5).copied()?,
            );
            // The region spans both circles, so the left edge is the further of the two
            // lefts and the right edge the further of the two rights. Reusing the names
            // here would make each line read the value the previous line had just written.
            let lo = (x0 - r0).min(x1 - r1);
            let hi = (x0 + r0).max(x1 + r1);
            let bottom = (y0 - r0).min(y1 - r1);
            let top = (y0 + r0).max(y1 + r1);
            Some(Rect {
                x0: lo,
                y0: bottom,
                x1: hi,
                y1: top,
            })
        }
    }
}

/// The pixel ranges a clip covers, for a caller that walks one.
#[must_use]
pub fn clip_pixels(rect: Rect) -> Option<(Range<usize>, Range<usize>)> {
    rect.pixels()
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

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    /// The formula the acceptance criteria's analytic checks use: an axial gradient from
    /// black to white over `[0, 1]`, sampled at a point, is the parameter at that point.
    fn axial_black_to_white() -> Shading {
        Shading::Axial {
            coords: vec![0.0, 0.0, 1.0, 0.0],
            function: Function::Exponential(Exponential {
                domain: vec![[0.0, 1.0]],
                range: vec![[0.0, 1.0, 1.0]],
                c0: vec![0.0],
                c1: vec![1.0],
            }),
            extend: [false, false],
        }
    }

    #[test]
    fn an_axial_gradient_is_linear_along_its_axis() {
        let s = axial_black_to_white();
        assert!(close(s.parameter_at(0.0, 0.0).unwrap_or(-1.0), 0.0));
        assert!(close(s.parameter_at(0.5, 0.0).unwrap_or(-1.0), 0.5));
        assert!(close(s.parameter_at(1.0, 0.0).unwrap_or(-1.0), 1.0));
    }

    #[test]
    fn an_axial_gradient_is_constant_along_a_perpendicular() {
        let s = axial_black_to_white();
        for y in [-50.0, -1.0, 0.0, 1.0, 500.0] {
            assert!(
                close(s.parameter_at(0.25, y).unwrap_or(-1.0), 0.25),
                "at (0.25, {y}) the parameter is the projection, not the distance"
            );
        }
    }

    #[test]
    fn a_diagonal_gradient_measures_along_its_own_axis() {
        // From (0,0) to (10,10): the point (5,5) is halfway along it, and (5,0) is a third
        // of the way because the projection onto (10,10) is 50 out of 200.
        let s = Shading::Axial {
            coords: vec![0.0, 0.0, 10.0, 10.0],
            function: Function::Exponential(Exponential {
                domain: vec![[0.0, 1.0]],
                range: vec![[0.0, 1.0, 1.0]],
                c0: vec![0.0],
                c1: vec![1.0],
            }),
            extend: [false, false],
        };
        assert!(close(s.parameter_at(5.0, 5.0).unwrap_or(-1.0), 0.5));
        assert!(close(s.parameter_at(5.0, 0.0).unwrap_or(-1.0), 0.25));
    }

    /// The parameter is raw on purpose: a caller needs to know how far outside a point is
    /// in order to work out how much of it is covered. Clamping here is what left the
    /// paper outside a gradient unpainted.
    #[test]
    fn a_parameter_outside_the_gradient_reports_how_far_out() {
        let s = axial_black_to_white();
        assert_eq!(s.parameter_at(-5.0, 0.0), Some(-5.0));
        assert_eq!(s.parameter_at(5.0, 0.0), Some(5.0));
        // And the colour is still the nearest end's, because that is what a point past the
        // end is painted.
        let c = s.colour_at(5.0).unwrap_or([-1.0; 3]);
        assert!(close(c[0], 1.0), "white at the far end, got {c:?}");
    }

    #[test]
    fn a_zero_length_axis_has_one_parameter_rather_than_dividing_by_zero() {
        let s = Shading::Axial {
            coords: vec![1.0, 1.0, 1.0, 1.0],
            function: Function::Exponential(Exponential {
                domain: vec![[0.0, 1.0]],
                range: vec![[0.0, 1.0, 1.0]],
                c0: vec![0.0],
                c1: vec![1.0],
            }),
            extend: [false, false],
        };
        assert_eq!(s.parameter_at(1.0, 1.0), Some(0.0));
        assert_eq!(
            s.parameter_at(100.0, 100.0),
            Some(0.0),
            "and no division by zero"
        );
    }

    /// The closed form the acceptance criteria name: for concentric circles the parameter
    /// along a ray is the distance from the inner circle over the ring's width.
    #[test]
    fn a_radial_gradient_is_the_distance_over_the_ring_width() {
        // r0 = 10, r1 = 20, concentric at the origin: at (0, 20) the point is on the outer
        // circle and the parameter is 1.
        let t = radial_parameter(0.0, 20.0, 0.0, 0.0, 10.0, 0.0, 0.0, 20.0);
        assert!(
            close(t.unwrap_or(-1.0), 1.0),
            "on the outer circle, got {t:?}"
        );
        let t = radial_parameter(0.0, 15.0, 0.0, 0.0, 10.0, 0.0, 0.0, 20.0);
        assert!(
            close(t.unwrap_or(-1.0), 0.5),
            "halfway out the ring, got {t:?}"
        );
        let t = radial_parameter(0.0, 10.0, 0.0, 0.0, 10.0, 0.0, 0.0, 20.0);
        assert!(
            close(t.unwrap_or(-1.0), 0.0),
            "at the inner circle, got {t:?}"
        );
        // A point beyond the outer circle has no root in range, and the parameter says so
        // with a number larger than one rather than by refusing.
        let t = radial_parameter(0.0, 25.0, 0.0, 0.0, 10.0, 0.0, 0.0, 20.0);
        assert!(
            t.is_some_and(|v| v > 1.0),
            "outside the outer circle, got {t:?}"
        );
    }

    /// No circle in the family passes through the family's own centre, so the parameter
    /// there is negative — and the disc is painted all the same.
    #[test]
    fn the_disc_inside_the_first_circle_is_filled_with_the_first_colour() {
        let radial = Shading::Radial {
            coords: vec![0.0, 0.0, 10.0, 0.0, 0.0, 20.0],
            function: Function::Exponential(Exponential {
                domain: vec![[0.0, 1.0]],
                range: vec![[0.0, 1.0, 1.0]],
                c0: vec![0.0],
                c1: vec![1.0],
            }),
            extend: [false, false],
        };
        assert!(
            radial.inner_fill(0.0, 0.0),
            "the centre is inside the first circle"
        );
        assert!(
            radial.inner_fill(0.0, 9.0),
            "and so is a point near its edge"
        );
        assert!(!radial.inner_fill(0.0, 11.0), "but not one in the ring");
        // The parameter alone cannot say so.
        let t = radial.parameter_at(0.0, 0.0);
        assert!(
            t.is_some_and(|v| v < 0.0),
            "which is exactly why the geometry is consulted: got {t:?}"
        );
        let mut device = Device::new(crate::Image::filled(40, 40, [255, 255, 255, 255]));
        paint(&mut device, &radial, &Matrix::scale(1.0, 1.0), 1.0);
        // The disc is centred on the shading's own origin, which is the image's corner.
        assert_eq!(
            device.image().get(0, 0),
            Some([0, 0, 0, 255]),
            "the centre is painted with the first colour, not left as paper"
        );
    }

    #[test]
    fn a_radial_gradient_is_the_same_at_every_angle() {
        for (x, y) in [(15.0, 0.0), (0.0, 15.0), (-15.0, 0.0), (0.0, -15.0)] {
            let t = radial_parameter(x, y, 0.0, 0.0, 10.0, 0.0, 0.0, 20.0);
            assert!(
                close(t.unwrap_or(-1.0), 0.5),
                "at ({x}, {y}) concentric circles are still concentric, got {t:?}"
            );
        }
    }

    #[test]
    fn a_point_inside_the_inner_circle_is_at_the_start() {
        let t = radial_parameter(0.0, 0.0, 0.0, 0.0, 10.0, 0.0, 0.0, 20.0);
        assert!(
            t.is_some_and(|v| v < 0.0),
            "the centre is on no circle in the family, so the parameter is negative: {t:?}"
        );
    }

    /// ISO 32000-1 Table 42: a type 2 function is `C0 + x^N · (C1 − C0)`.
    ///
    /// `C0` and `C1` are the two ends of the ramp, so the ends are the first two assertions
    /// and the middle one is the exponent bending the ramp between them.
    ///
    /// **`C0` must not be zero here.** With `C0 = 0` this reduces to `x^N · C1`, which is
    /// also what the simpler `C0 + x^N · C1` gives, so a fixture with a zero `C0` cannot tell
    /// the two rules apart and a test built on one passes against the other. Every fixture in
    /// this repository that exercises a type 2 function writes `C0 [0]`, which is why the case
    /// went unnoticed; the test that used to pin it asserted `C0 + C1` at `t = 1`, which is
    /// one step from this one and no further.
    #[test]
    fn an_exponential_function_is_c0_plus_t_to_the_n_times_c1_minus_c0() {
        let f = Function::Exponential(Exponential {
            domain: vec![[0.0, 1.0]],
            range: vec![[0.0, 1.0, 2.0]],
            c0: vec![0.25],
            c1: vec![0.75],
        });
        let at = |t: f64| f.apply1(t).and_then(|v| v.first().copied()).unwrap_or(-1.0);
        // By hand, from `0.25 + t²·(0.75 − 0.25)`:
        assert!(close(at(0.0), 0.25), "C0 at t = 0, got {}", at(0.0));
        assert!(
            close(at(1.0), 0.75),
            "C1 at t = 1, which the wrong rule makes 1.0; got {}",
            at(1.0)
        );
        assert!(
            close(at(0.5), 0.25 + 0.25 * 0.5),
            "n = 2 squares the parameter and the ramp runs from C0 to C1: \
             0.25 + 0.5²·0.5 = 0.375, got {}",
            at(0.5)
        );
        // The other half of the rule: the ends are the ends, so every value between them is
        // between them. A quarter of the way is a quarter of the way from C0 to C1.
        assert!(
            close(at(0.25), 0.25 + 0.0625 * 0.5),
            "0.25 + 0.25²·0.5 = 0.28125, got {}",
            at(0.25)
        );
    }

    #[test]
    fn a_function_outside_its_domain_has_no_answer() {
        let f = Function::Exponential(Exponential {
            domain: vec![[0.2, 0.8]],
            range: vec![[0.0, 1.0, 1.0]],
            c0: vec![0.0],
            c1: vec![1.0],
        });
        assert!(f.apply1(0.1).is_none(), "below the domain");
        assert!(f.apply1(0.9).is_none(), "above it");
        assert!(f.apply1(0.5).is_some(), "and inside it");
    }

    /// A function that cannot produce a finite number has no answer, and a gradient with a
    /// hole in it is a bug report rather than a plausible wrong colour.
    #[test]
    fn an_exponent_that_overflows_has_no_answer() {
        let f = Function::Exponential(Exponential {
            domain: vec![[0.0, 10.0]],
            // 2 to the two thousandth is not a number.
            range: vec![[0.0, 1.0, 2000.0]],
            c0: vec![0.0],
            c1: vec![1.0],
        });
        assert!(
            f.apply1(2.0).is_none(),
            "there is no answer, so there is no colour"
        );
        assert!(
            f.apply1(0.5).is_some(),
            "and a small enough exponent is fine"
        );
    }

    /// Two functions stitched end to end have to meet. The second half runs from `0.5` to
    /// `1.0`, which is `C0 = 0.5, C1 = 1.0` under the specification's rule and not
    /// `C0 = 0.5, C1 = 0.5`: the second of those is a constant function of value `0.5`, so a
    /// stitch built from it would end the gradient halfway and stay there.
    #[test]
    fn a_stitching_function_picks_the_right_part() {
        let part = |c0: f64, c1: f64| {
            Function::Exponential(Exponential {
                domain: vec![[0.0, 1.0]],
                range: vec![[0.0, 1.0, 1.0]],
                c0: vec![c0],
                c1: vec![c1],
            })
        };
        let s = Function::Stitching(Stitching {
            domain: vec![[0.0, 1.0]],
            functions: vec![part(0.0, 0.5), part(0.5, 1.0)],
            bounds: vec![0.5],
            encode: vec![[0.0, 1.0], [0.0, 1.0]],
        });
        let at = |t: f64| s.apply1(t).and_then(|v| v.first().copied()).unwrap_or(-1.0);
        assert!(
            close(at(0.0), 0.0),
            "the first part's start, got {}",
            at(0.0)
        );
        assert!(close(at(0.5), 0.5), "the join, got {}", at(0.5));
        assert!(
            close(at(1.0), 1.0),
            "the second part's end, got {}",
            at(1.0)
        );
    }

    #[test]
    fn a_calculator_adds_two_numbers() {
        let f = Function::Calculator(Calculator {
            domain: vec![[0.0, 1.0]],
            range: vec![[0.0, 2.0]],
            program: lex_program("{ 1 add }"),
        });
        let v = f.apply1(0.25).and_then(|v| v.first().copied());
        assert!(close(v.unwrap_or(-1.0), 1.25), "got {v:?}");
    }

    #[test]
    fn a_calculator_multiplies_and_exponentiates() {
        let f = |program: &str| {
            Function::Calculator(Calculator {
                domain: vec![[0.0, 10.0]],
                range: vec![[0.0, 1000.0]],
                program: lex_program(program),
            })
        };
        let at =
            |f: &Function, t: f64| f.apply1(t).and_then(|v| v.first().copied()).unwrap_or(-1.0);
        assert!(close(at(&f("{ 2 mul }"), 3.0), 6.0));
        assert!(
            close(at(&f("{ 2 exp }"), 3.0), 9.0),
            "`a b exp` is a to the b, so the input is the base"
        );
        assert!(
            close(at(&f("{ dup mul }"), 3.0), 9.0),
            "dup then mul squares it"
        );
        assert!(
            close(at(&f("{ 1 2 exch sub }"), 0.0), 1.0),
            "exch reverses, so 2 - 1"
        );
    }

    #[test]
    fn a_calculator_divides_by_zero_rather_than_producing_an_infinity() {
        let f = Function::Calculator(Calculator {
            domain: vec![[0.0, 1.0]],
            range: vec![[0.0, 1.0]],
            program: lex_program("{ 1 0 div }"),
        });
        assert!(
            f.apply1(0.5).is_none(),
            "there is no answer, so there is no colour"
        );
    }

    #[test]
    fn a_calculator_takes_a_logarithm_only_of_a_positive_number() {
        let good = Function::Calculator(Calculator {
            domain: vec![[0.0, 100.0]],
            range: vec![[0.0, 3.0]],
            program: lex_program("{ log }"),
        });
        assert!(close(
            good.apply1(100.0)
                .and_then(|v| v.first().copied())
                .unwrap_or(-1.0),
            2.0
        ));
        let bad = Function::Calculator(Calculator {
            domain: vec![[0.0, 1.0]],
            range: vec![[0.0, 1.0]],
            program: lex_program("{ 0 ln }"),
        });
        assert!(bad.apply1(0.5).is_none());
    }

    #[test]
    fn a_calculator_reports_postscrips_own_truth() {
        let f = |program: &str| {
            Function::Calculator(Calculator {
                domain: vec![[0.0, 1.0]],
                range: vec![[0.0, 1.0]],
                program: lex_program(program),
            })
        };
        let at =
            |f: &Function, t: f64| f.apply1(t).and_then(|v| v.first().copied()).unwrap_or(-1.0);
        assert_eq!(at(&f("{ true }"), 0.0), 1.0);
        assert_eq!(at(&f("{ false }"), 0.0), 0.0);
        assert_eq!(at(&f("{ 0 not }"), 0.0), 1.0, "not of zero is true");
        assert_eq!(at(&f("{ 1 not }"), 0.0), 0.0);
        assert_eq!(at(&f("{ 1 2 lt }"), 0.0), 1.0);
        assert_eq!(at(&f("{ 2 1 lt }"), 0.0), 0.0);
        assert_eq!(at(&f("{ 1 1 eq }"), 0.0), 1.0);
        assert_eq!(
            at(&f("{ 1 0 and }"), 0.0),
            0.0,
            "and is logical, not numeric"
        );
        assert_eq!(at(&f("{ 1 1 or }"), 0.0), 1.0);
    }

    #[test]
    fn a_calculator_clamps_its_output_to_the_declared_range() {
        let f = Function::Calculator(Calculator {
            domain: vec![[0.0, 1.0]],
            range: vec![[0.0, 1.0]],
            // A program that would return 100 if it were not clamped.
            program: lex_program("{ 100 }"),
        });
        let v = f.apply1(0.5).and_then(|v| v.first().copied());
        assert_eq!(
            v,
            Some(1.0),
            "the specification clamps, and a file that relies on \
             an out-of-range value gets the range's end"
        );
    }

    #[test]
    fn a_calculator_does_not_divide_by_zero_when_it_reads_an_operator_from_the_stack() {
        let f = Function::Calculator(Calculator {
            domain: vec![[0.0, 1.0]],
            range: vec![[0.0, 1.0]],
            program: lex_program("{ 1 0 div }"),
        });
        assert!(f.apply1(0.5).is_none());
    }

    #[test]
    fn a_program_that_grows_the_stack_without_bound_is_refused() {
        let f = Function::Calculator(Calculator {
            domain: vec![[0.0, 1.0]],
            range: vec![[0.0, 1.0]],
            program: vec![Token::Number(1.0); MAX_STACK + 10],
        });
        assert!(
            f.apply1(0.5).is_none(),
            "a runaway program is damage, not a long loop"
        );
    }

    #[test]
    fn a_bitshift_past_the_word_is_zero_not_a_wrap() {
        // The specification says a shift of more than the word's width is zero, which is not
        // what a machine shift does and is the usual way to get this wrong.
        let f = Function::Calculator(Calculator {
            domain: vec![[0.0, 1.0]],
            range: vec![[0.0, 1000.0]],
            program: lex_program("{ 1 32 bitshift }"),
        });
        let v = f.apply1(0.5).and_then(|v| v.first().copied());
        assert_eq!(v, Some(0.0));
        let g = Function::Calculator(Calculator {
            domain: vec![[0.0, 1.0]],
            range: vec![[0.0, 1000.0]],
            program: lex_program("{ 1 4 bitshift }"),
        });
        assert_eq!(g.apply1(0.5).and_then(|v| v.first().copied()), Some(16.0));
    }

    #[test]
    fn the_program_lexiser_reads_numbers_names_blocks_and_hex() {
        let tokens = lex_program("{ 1 2 add } /MyName <48656C6C6F>");
        assert_eq!(tokens.len(), 3);
        assert!(matches!(tokens.first(), Some(Token::Block(b)) if b.len() == 3));
        assert!(matches!(tokens.get(1), Some(Token::Name(n)) if n == b"MyName"));
        assert!(matches!(tokens.get(2), Some(Token::HexString(h)) if h == b"Hello"));
    }

    #[test]
    fn the_program_lexiser_skips_comments() {
        let tokens = lex_program("% a comment\n 1 2 add");
        assert_eq!(tokens.len(), 3, "the comment contributes nothing");
    }

    #[test]
    fn a_sampled_function_interpolates_between_its_entries() {
        let f = Function::Sampled(Sampled {
            domain: vec![[0.0, 1.0]],
            size: vec![2],
            bits: 8,
            range: vec![[0.0, 1.0]],
            encode: vec![[0.0, 1.0]],
            samples: vec![0.0, 1.0],
        });
        let at = |t: f64| f.apply1(t).and_then(|v| v.first().copied()).unwrap_or(-1.0);
        assert!(close(at(0.0), 0.0));
        assert!(close(at(1.0), 1.0));
        assert!(
            close(at(0.5), 0.5),
            "linearly between the two samples, got {}",
            at(0.5)
        );
    }

    #[test]
    fn a_sampled_function_honours_its_encode_range() {
        // Four samples over a domain of 0..3, so index 3 is the fourth.
        let f = Function::Sampled(Sampled {
            domain: vec![[0.0, 3.0]],
            size: vec![4],
            bits: 8,
            range: vec![[0.0, 1.0]],
            encode: vec![[0.0, 3.0]],
            samples: vec![0.0, 0.25, 0.5, 1.0],
        });
        let at = |t: f64| f.apply1(t).and_then(|v| v.first().copied()).unwrap_or(-1.0);
        assert!(close(at(0.0), 0.0));
        assert!(close(at(1.0), 0.25), "got {}", at(1.0));
        assert!(close(at(2.0), 0.5), "got {}", at(2.0));
        assert!(close(at(3.0), 1.0), "got {}", at(3.0));
    }

    #[test]
    fn a_sampled_function_with_no_samples_has_no_answer() {
        let f = Function::Sampled(Sampled {
            domain: vec![[0.0, 1.0]],
            size: vec![2],
            bits: 8,
            range: vec![[0.0, 1.0]],
            encode: vec![[0.0, 1.0]],
            samples: Vec::new(),
        });
        assert!(f.apply1(0.5).is_none());
    }

    #[test]
    fn extend_is_read_from_the_dictionary_and_defaults_to_off() {
        let mut d = Dict::new();
        d.set("ShadingType", Object::Int(2));
        d.set(
            "Coords",
            Object::Array(vec![
                Object::Int(0),
                Object::Int(0),
                Object::Int(1),
                Object::Int(0),
            ]),
        );
        let mut f = Dict::new();
        f.set("FunctionType", Object::Int(2));
        f.set(
            "Domain",
            Object::Array(vec![Object::Int(0), Object::Int(1)]),
        );
        f.set(
            "Range",
            Object::Array(vec![Object::Int(0), Object::Int(1), Object::Int(1)]),
        );
        d.set("Function", Object::Dict(f));

        let s = Shading::parse(&Object::Dict(d.clone()), &|o| Some(o.clone()));
        assert!(s.is_some(), "a two-point axial gradient is the common case");
        assert_eq!(s.map(|v| v.extend()), Some([false, false]));

        let mut on = d;
        on.set(
            "Extend",
            Object::Array(vec![Object::Bool(true), Object::Bool(false)]),
        );
        assert_eq!(
            Shading::parse(&Object::Dict(on), &|o| Some(o.clone())).map(|v| v.extend()),
            Some([true, false])
        );
    }

    #[test]
    fn a_shading_of_an_unsupported_type_is_refused() {
        // Type 1 is function-based and needs a pattern colour; type 4 is a mesh.
        for kind in [1, 4, 5, 6, 7] {
            let mut d = Dict::new();
            d.set("ShadingType", Object::Int(kind));
            d.set(
                "Coords",
                Object::Array(vec![Object::Int(0), Object::Int(0)]),
            );
            assert!(
                Shading::parse(&Object::Dict(d), &|o| Some(o.clone())).is_none(),
                "type {kind} needs something this does not build yet"
            );
        }
    }

    #[test]
    fn a_shading_with_too_few_coordinates_is_refused() {
        let mut d = Dict::new();
        d.set("ShadingType", Object::Int(2));
        d.set(
            "Coords",
            Object::Array(vec![Object::Int(0), Object::Int(0)]),
        );
        let mut f = Dict::new();
        f.set("FunctionType", Object::Int(2));
        f.set(
            "Domain",
            Object::Array(vec![Object::Int(0), Object::Int(1)]),
        );
        f.set(
            "Range",
            Object::Array(vec![Object::Int(0), Object::Int(1), Object::Int(1)]),
        );
        d.set("Function", Object::Dict(f));
        assert!(Shading::parse(&Object::Dict(d), &|o| Some(o.clone())).is_none());
    }

    #[test]
    fn a_gradient_colour_is_read_as_gray_or_rgb() {
        let s = axial_black_to_white();
        let at = |t: f64| s.colour_at(t).unwrap_or([-1.0; 3]);
        assert!(close(at(0.0)[0], 0.0), "black at the start");
        assert!(close(at(1.0)[0], 1.0), "white at the end");
        assert!(close(at(0.25)[0], 0.25), "and gray in between");
        assert!(
            close(at(0.5)[1], 0.5),
            "every channel matches, because it is gray"
        );
    }

    #[test]
    fn a_gradient_colour_can_be_cmyk_and_is_subtractive() {
        let s = Shading::Axial {
            coords: vec![0.0, 0.0, 1.0, 0.0],
            function: Function::Exponential(Exponential {
                domain: vec![[0.0, 1.0]],
                range: vec![
                    [0.0, 1.0, 1.0],
                    [0.0, 1.0, 1.0],
                    [0.0, 1.0, 1.0],
                    [0.0, 1.0, 1.0],
                ],
                c0: vec![0.0, 0.0, 0.0, 0.0],
                c1: vec![1.0, 0.0, 0.0, 0.0],
            }),
            extend: [false, false],
        };
        let at = |t: f64| s.colour_at(t).unwrap_or([-1.0; 3]);
        let end = at(1.0);
        assert!(
            close(end[0], 0.0) && close(end[1], 1.0) && close(end[2], 1.0),
            "full cyan at the end, got {end:?}"
        );
    }

    #[test]
    fn a_gradient_paints_inside_its_own_extent_and_nowhere_else() {
        // A black-to-white gradient across the left half of a 20 by 20 canvas.
        let mut device = Device::new(crate::Image::filled(20, 20, [255, 255, 255, 255]));
        let s = axial_black_to_white();
        let drawn = paint(
            &mut device,
            &s,
            &Matrix::new(10.0, 0.0, 0.0, 10.0, 0.0, 0.0),
            1.0,
        );
        assert!(drawn, "something was painted");
        // The gradient runs from x = 0 to x = 10, so a pixel's centre at device x has
        // t = (x + 0.5) / 10. Stating the formula rather than a pixel makes the check a
        // measurement of the gradient rather than a snapshot of one resolution.
        let at = |x: usize| -> u8 {
            let t = (x as f64 + 0.5) / 10.0;
            (t * 255.0).round() as u8
        };
        for x in 0..10 {
            assert_eq!(
                device.image().get(x, 10).map(|p| p[0]),
                Some(at(x)),
                "at x = {x} the parameter is {} and that is the colour",
                (x as f64 + 0.5) / 10.0
            );
        }
        // Past the right end nothing is painted, because the gradient does not extend.
        assert_eq!(
            device.image().get(15, 10),
            Some([255, 255, 255, 255]),
            "outside the gradient is paper"
        );
    }

    #[test]
    fn an_extended_gradient_paints_past_its_ends() {
        let mut device = Device::new(crate::Image::filled(20, 20, [255, 255, 255, 255]));
        let s = match axial_black_to_white() {
            Shading::Axial {
                coords, function, ..
            } => Shading::Axial {
                coords,
                function,
                extend: [true, true],
            },
            other @ Shading::Radial { .. } => other,
        };
        paint(
            &mut device,
            &s,
            &Matrix::new(10.0, 0.0, 0.0, 10.0, 0.0, 0.0),
            1.0,
        );
        assert_eq!(
            device.image().get(19, 10),
            Some([255, 255, 255, 255]),
            "extended past the right end is the end colour, which is white"
        );
    }

    #[test]
    fn a_rotated_gradient_is_painted_through_its_own_transformation() {
        // The same gradient, but rotated a quarter turn, so it runs down the page.
        let mut device = Device::new(crate::Image::filled(20, 20, [255, 255, 255, 255]));
        let s = axial_black_to_white();
        paint(
            &mut device,
            &s,
            &Matrix::new(0.0, 10.0, -10.0, 0.0, 10.0, 0.0),
            1.0,
        );
        // The transformation puts the shading's own u along the page's y and its v along
        // `10 - x`, so the gradient runs down the page and stops ten pixels from the top.
        let at = |y: usize| -> u8 {
            let t = (y as f64 + 0.5) / 10.0;
            (t * 255.0).round() as u8
        };
        for y in 0..10 {
            assert_eq!(
                device.image().get(10, y).map(|p| p[0]),
                Some(at(y)),
                "the rotated gradient runs down the page, and at y = {y} the colour is {}",
                at(y)
            );
        }
        // Past the end of the axis the gradient does not reach, and the paper is still there.
        assert_eq!(
            device.image().get(10, 15),
            Some([255, 255, 255, 255]),
            "below the gradient's own extent is paper"
        );
    }

    #[test]
    fn a_gradient_that_cannot_be_inverted_paints_nothing() {
        let mut device = Device::new(crate::Image::filled(4, 4, [255, 255, 255, 255]));
        let s = axial_black_to_white();
        let flat = Matrix::scale(0.0, 0.0);
        assert!(
            !paint(&mut device, &s, &flat, 1.0),
            "a degenerate transform draws nothing"
        );
    }

    #[test]
    fn bounds_cover_the_gradient() {
        let axial = axial_black_to_white();
        let b = bounds(&axial).expect("bounds");
        assert_eq!((b.x0, b.y0, b.x1, b.y1), (0.0, 0.0, 1.0, 0.0));

        let radial = Shading::Radial {
            coords: vec![5.0, 5.0, 2.0, 5.0, 5.0, 4.0],
            function: Function::Exponential(Exponential {
                domain: vec![[0.0, 1.0]],
                range: vec![[0.0, 1.0, 1.0]],
                c0: vec![0.0],
                c1: vec![1.0],
            }),
            extend: [false, false],
        };
        let r = bounds(&radial).expect("bounds");
        assert_eq!(
            (r.x0, r.y0, r.x1, r.y1),
            (1.0, 1.0, 9.0, 9.0),
            "the outer circle, got {r:?}"
        );
    }

    #[test]
    fn the_clip_pixels_helper_agrees_with_the_rectangle() {
        let r = Rect {
            x0: 0.0,
            y0: 0.0,
            x1: 4.0,
            y1: 4.0,
        };
        let (columns, rows) = clip_pixels(r).expect("pixels");
        assert_eq!((columns.end - columns.start, rows.end - rows.start), (4, 4));
    }

    #[test]
    fn the_stack_and_program_bounds_are_arithmetic() {
        const {
            assert!(MAX_STACK > 16);
            assert!(MAX_PROGRAM > 16);
        }
    }
}
