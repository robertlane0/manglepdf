//! Filling a shape, when the fill colour is not one colour.
//!
//! A `/Pattern` colour space means the content stream's operands named a *pattern resource*
//! rather than a colour value, so there is nothing for a colour-space converter to convert:
//! the pattern decides what is painted, and it decides it per pixel. A `/PatternType 2`
//! pattern names a shading, whose colour changes with position across the shape being
//! filled; a `/PatternType 1` pattern tiles a content stream, which needs a loop over cells
//! in the pattern's own space. Only the first is here, and the second is refused by name
//! rather than approximated by one cell — a page with a visible hole in it is a bug report,
//! and a page with a wrong texture is a wrong answer.
//!
//! ## One evaluator, and why that matters
//!
//! The inverse-mapping and the antialiased edge of a shading were written for `sh`, and they
//! live here now: [`Sampler::sample`] is the evaluator both use, and `shading::paint` calls
//! it. They are shared rather than written twice because two evaluators that agree today can
//! drift apart tomorrow — one gains a `/Extend` rule the other does not — and the symptom is
//! a gradient that looks right in a `sh` and wrong in a fill, which is very hard to see and
//! easy to introduce. The test that compares a pattern fill against a `sh` over the same
//! pattern exists to keep it that way.
//!
//! ## The shape and the colour are separate
//!
//! Coverage is computed exactly as an ordinary fill computes it — the same analytic
//! rasteriser, the same clip — and then *this* module decides what colour each covered
//! pixel takes. A flat colour answers the same for every pixel, which is why
//! [`FillColour::Flat`] exists and why a page with an ordinary colour fill never comes
//! through here at all.

use mangle_content::{Matrix, Rgba};

use crate::coverage::{FillRule, rasterise};
use crate::shading::Shading;
use crate::{Device, Polygon, over};

/// The colour a fill paints in, which is not always a single colour.
#[derive(Debug, Clone)]
pub enum FillColour {
    /// One colour for every pixel of the fill.
    Flat(Rgba),
    /// A shading, which gives every pixel its own.
    ///
    /// The colour varies with *position*, so this cannot be reduced to an `Rgba` once and
    /// reused: a gradient across a square has no single colour, and picking one of them
    /// paints a flat block where the page asked for a ramp.
    Shading {
        shading: Shading,
        /// The shading's own space onto the device.
        ///
        /// The pattern's `/Matrix` composed inside the page's own placement — and *not* the
        /// mark's transformation, which is the specification's rule and the one that is
        /// easiest to get backwards. A pattern says where it lives in the page's space, so a
        /// transformation that moves the shape it paints leaves the pattern where it was.
        to_shading: Matrix,
    },
}

impl FillColour {
    /// One colour, which is the ordinary case and the only one a caller can state without a
    /// pattern resource in hand.
    #[must_use]
    pub fn flat(colour: Rgba) -> Self {
        Self::Flat(colour)
    }

    /// Prepare this colour to be asked, pixel by pixel.
    ///
    /// `None` when the colour has nothing to give: a shading whose transformation cannot be
    /// inverted maps every device point to the same place, and answering for all of them at
    /// once would be a flat fill wearing a gradient's name.
    #[must_use]
    pub fn sampler(&self) -> Option<Sampler<'_>> {
        match self {
            Self::Flat(colour) => Some(Sampler::Flat(*colour)),
            Self::Shading {
                shading,
                to_shading,
            } => Some(Sampler::Shading {
                shading,
                inverse: to_shading.inverse()?,
                extend: shading.extend(),
            }),
        }
    }
}

/// What a shading says about one device pixel: a colour, and how much of that pixel it
/// covers.
///
/// Kept apart rather than folded into one RGBA value because the two are answers to
/// different questions, and the caller has its own third: the shape being filled decides how
/// much of the pixel it covers, the gradient decides how much of its own extent reaches it,
/// and the clip decides the rest.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// Red, green and blue in 0..=1.
    pub colour: [f64; 3],
    /// How much of the pixel the gradient covers, in 0..=1.
    pub coverage: f64,
}

/// A fill colour with its per-pixel work already done.
///
/// Two variants and no more, because there are only two things a fill can be: one colour, or
/// a rule for producing a colour at a position. The shading variant holds the transformation
/// *inverted*, which is the expensive half — mapping a pixel back into the shading's own
/// space is what makes a rotated or skewed gradient work at all — and doing it once here
/// rather than once per pixel is what a per-pixel colour source costs if it does not.
#[derive(Debug, Clone)]
pub enum Sampler<'a> {
    /// One colour for every pixel of the fill.
    Flat(Rgba),
    /// A shading, with the transformation into its own space inverted.
    Shading {
        shading: &'a Shading,
        /// Maps a device point *back* into the shading's own space.
        inverse: Matrix,
        extend: [bool; 2],
    },
}

impl Sampler<'_> {
    /// The shading's colour and its own coverage at a device pixel, or `None` where the
    /// gradient reaches nothing.
    ///
    /// This is the evaluator `shading::paint` uses and it is the same one a fill uses, down
    /// to the three evaluations that give the edge its antialiasing: `t` at the pixel's
    /// centre and half a pixel either side, differenced into a slope, and `t / |∇t|` as the
    /// distance from the centre to the gradient's boundary.
    ///
    /// `None` for a flat colour, which has no gradient and therefore nothing to sample.
    #[must_use]
    pub fn sample(&self, x: usize, y: usize) -> Option<Sample> {
        let Self::Shading {
            shading,
            inverse,
            extend,
        } = self
        else {
            return None;
        };
        let (sx, sy) = inverse.apply(x as f64 + 0.5, y as f64 + 0.5);
        let t = shading.parameter_at(sx, sy)?;
        // How far `t` moves per pixel, by evaluating it either side. Differencing beats
        // a closed form because it is the same three evaluations for an axial gradient
        // and a radial one, and a radial gradient's gradient is a conic section nobody
        // wants to write twice.
        let step = 0.5f64;
        let (ax, ay) = inverse.apply(x as f64 + 0.5 + step, y as f64 + 0.5);
        let (bx, by) = inverse.apply(x as f64 + 0.5, y as f64 + 0.5 + step);
        let (tx, ty) = (shading.parameter_at(ax, ay), shading.parameter_at(bx, by));
        let slope = match (tx, ty) {
            (Some(a), Some(b)) => {
                let dx = (a - t) / step;
                let dy = (b - t) / step;
                (dx * dx + dy * dy).sqrt()
            }
            // A gradient whose ends coincide has no slope; treat it as steep, which
            // means no antialiasing rather than a smeared edge.
            _ => f64::INFINITY,
        };
        // The gradient's own first circle is filled with its first colour, and for a
        // radial gradient the parameter there is negative rather than absent, so the
        // disc is recognised by geometry instead.
        let coverage = if shading.inner_fill(sx, sy) {
            1.0
        } else if slope.is_finite() && slope > 0.0 {
            let low = if extend[0] {
                1.0
            } else {
                (t / slope).clamp(0.0, 1.0)
            };
            let high = if extend[1] {
                1.0
            } else {
                ((1.0 - t) / slope).clamp(0.0, 1.0)
            };
            low.min(high)
        } else if slope.is_finite() {
            // A flat gradient covers everything inside and nothing outside.
            if extend[0] || extend[1] { 1.0 } else { 0.0 }
        } else {
            1.0
        };
        if coverage <= 0.0 {
            return None;
        }
        let colour = shading.colour_at(t)?;
        Some(Sample { colour, coverage })
    }

    /// The colour this fill puts down at a device pixel, in bytes, or `None` to leave the
    /// paper alone.
    ///
    /// `alpha` is the graphics state's fill alpha. For a gradient it multiplies the
    /// gradient's own coverage rather than replacing it, so a pixel that the gradient only
    /// half reaches is half as strong *and* partly uncovered — which is what an antialiased
    /// boundary means.
    #[must_use]
    pub fn at(&self, x: usize, y: usize, alpha: f64) -> Option<[u8; 4]> {
        match self {
            Self::Flat(colour) => Some(colour.to_rgba8(alpha)),
            Self::Shading { .. } => {
                let sample = self.sample(x, y)?;
                let a = (sample.coverage * alpha).clamp(0.0, 1.0);
                Some([
                    (sample.colour[0].clamp(0.0, 1.0) * 255.0).round() as u8,
                    (sample.colour[1].clamp(0.0, 1.0) * 255.0).round() as u8,
                    (sample.colour[2].clamp(0.0, 1.0) * 255.0).round() as u8,
                    (a * 255.0).round() as u8,
                ])
            }
        }
    }
}

/// Fill a polygon whose colour changes from pixel to pixel.
///
/// The shape's coverage is the ordinary analytic one, computed over the polygon's box
/// intersected with the clip, and the clip's own coverage multiplies it rather than deciding
/// it — so a pattern fill crossing a diagonal clip has that edge antialiased instead of
/// stopping at it. Each covered pixel then asks the fill colour what to put there.
///
/// Returns whether anything was drawn.
pub fn polygon(
    device: &mut Device,
    shape: &Polygon,
    rule: FillRule,
    colour: &FillColour,
    alpha: f64,
) -> bool {
    let Some(bounds) = shape.bounds() else {
        return false;
    };
    let area = bounds.intersect(device.clip());
    if area.is_empty() {
        return false;
    }
    let Some(sampler) = colour.sampler() else {
        return false;
    };
    let coverage = rasterise(&shape.edges(), area, rule);
    let origin = (
        area.x0.floor().max(0.0) as usize,
        area.y0.floor().max(0.0) as usize,
    );
    let mut drawn = 0usize;
    for (x, y, a) in coverage.covered() {
        let (Some(px), Some(py)) = (origin.0.checked_add(x), origin.1.checked_add(y)) else {
            continue;
        };
        let Some(src) = sampler.at(px, py, alpha) else {
            continue;
        };
        // The clip's coverage and the shape's own are both fractions of this pixel, so they
        // multiply into one effective opacity for the blend below.
        let clip = device.clip_coverage(px, py).unwrap_or(1.0);
        let src_alpha = f64::from(src[3]) / 255.0 * f64::from(a) * clip;
        if src_alpha <= 0.0 {
            continue;
        }
        let Some(dst) = device.image().get(px, py) else {
            continue;
        };
        device.put(px, py, over(dst, src, src_alpha));
        drawn += 1;
    }
    drawn > 0
}
