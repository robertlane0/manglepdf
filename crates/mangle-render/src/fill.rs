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
//! # A stroke is a fill too
//!
//! `SCN` names a pattern as the *stroke* colour exactly as `scn` names one as the fill
//! colour, so both paint operators reach this module and neither is a special case of the
//! other: they differ in how the shape is arrived at, not in what colour it takes. A
//! stroke's shape is its own outline — the dashes, the caps and the joins — which
//! [`Device::stroke_outline`] computes and this module then fills.
//!
//! That gap was found the way gaps in a renderer usually are: a page whose every mark was a
//! patterned stroke came out with no ink on it at all, because the stroke asked a colour
//! converter for one colour and a pattern has none to give.
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
use crate::{Device, Image, Polygon, StrokeStyle, over};

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
    /// A `/PatternType 1` tiling pattern: one rendered cell, repeated.
    ///
    /// The same reasoning as the shading variant applies and a little more strongly. A tiling
    /// pattern's colour depends on position *and* on which repeat of the cell the pixel falls
    /// in, so it cannot be reduced to a colour; and the cell is rendered once rather than per
    /// pixel because a cell is a texture.
    Tiling {
        cell: Image,
        /// Where one cell's own top-left pixel sits on the device.
        origin: (f64, f64),
        /// Device pixels per unit of pattern space, along x and then y.
        scale: (f64, f64),
        /// The pattern's `/Matrix` composed inside the page's own placement, which is the same
        /// composition the shading variant uses and for the same reason: a pattern says where
        /// it lives in the page's space, so a transformation that moves the shape it paints
        /// leaves the pattern where it was.
        to_pattern: Matrix,
        /// The pitch in pattern space. **Zero means not repeated in that direction.**
        step: (f64, f64),
        /// The cell's own box in pattern space, which clips it.
        bbox: (f64, f64, f64, f64),
        /// For `/PaintType 2`, the colour the operator asked for rather than the cell's own.
        uncoloured: Option<Rgba>,
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
            Self::Tiling {
                cell,
                origin,
                scale,
                to_pattern,
                step,
                bbox,
                uncoloured,
            } => Some(Sampler::Tiling {
                cell,
                origin: *origin,
                scale: *scale,
                inverse: to_pattern.inverse()?,
                step: *step,
                bbox: *bbox,
                uncoloured: *uncoloured,
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
    /// One rendered cell of a tiling pattern, repeated.
    ///
    /// The cell is held as pixels rather than as a content stream because the stream is run
    /// once, not once per pixel: a cell is a texture, and re-executing it for every pixel of
    /// every shape it paints would cost more than the rest of the page put together.
    Tiling {
        cell: &'a Image,
        /// Where one cell's own top-left pixel sits on the device.
        origin: (f64, f64),
        /// Device pixels per unit of pattern space, along x and then y.
        scale: (f64, f64),
        /// Maps a device point *back* into pattern space.
        inverse: Matrix,
        /// The pitch in pattern space. **Zero means the cell is not repeated in that
        /// direction** and is drawn once, which is why this is a pair and not an extent.
        step: (f64, f64),
        /// The cell's own box, in pattern space, which clips it.
        bbox: (f64, f64, f64, f64),
        /// For an uncoloured pattern, the colour the operator asked for.
        uncoloured: Option<Rgba>,
    },
}

impl Sampler<'_> {
    /// One pixel of one cell of a tiling pattern.
    ///
    /// The arithmetic is: put the device pixel back into pattern space, find which repeat of
    /// the cell it lands in, take its offset inside that repeat, and read the cell. A **zero
    /// step means the cell is not repeated in that direction**, so the repeat index is forced to
    /// zero rather than left to a division by zero — and the cell is then drawn once, anchored
    /// at the pattern-space origin, which is where the specification puts it.
    #[allow(clippy::too_many_arguments, clippy::unused_self)]
    fn tile(
        &self,
        cell: &Image,
        origin: (f64, f64),
        scale: (f64, f64),
        inverse: &Matrix,
        step: (f64, f64),
        bbox: (f64, f64, f64, f64),
        uncoloured: Option<Rgba>,
        x: usize,
        y: usize,
    ) -> Sample {
        let nothing = Sample {
            colour: [0.0, 0.0, 0.0],
            coverage: 0.0,
        };
        let (px, py) = inverse.apply(x as f64 + 0.5, y as f64 + 0.5);
        // Which repeat, and where inside it. `floor` rather than a truncation so that a pixel
        // just below an origin belongs to the cell *before* it, which is the difference between
        // a tiling and a staircase.
        let (ix, lx) = split_cell(px, step.0);
        let (iy, ly) = split_cell(py, step.1);
        // The cell's own box clips it, and a box the wrong way round is read as the same box
        // with its corners in order rather than as an empty one — the same choice the form
        // XObject path makes, and for the same reason: an ordering mistake in a file should not
        // be allowed to delete the page.
        let (lo_x, lo_y) = (bbox.0.min(bbox.2), bbox.1.min(bbox.3));
        let (hi_x, hi_y) = (bbox.0.max(bbox.2), bbox.1.max(bbox.3));
        if lx < lo_x || lx > hi_x || ly < lo_y || ly > hi_y {
            return nothing;
        }
        // Device pixel within the rendered cell. **y is measured from the box's *top***,
        // because the cell image was drawn with pattern space inverted — the two must agree,
        // or the cell is drawn one way and read the other and comes out mirrored.
        let cx = (origin.0 + (lx - bbox.0) * scale.0).floor();
        let cy = (origin.1 + (bbox.3 - ly) * scale.1).floor();
        // The repeat offsets the cell within the cell image, which is itself one repeat wide.
        let _ = (ix, iy);
        let (cx, cy) = (
            cx.rem_euclid(cell.width as f64),
            cy.rem_euclid(cell.height as f64),
        );
        let (ux, uy) = (cx as usize, cy as usize);
        let rgba = cell.get(ux, uy).unwrap_or([0, 0, 0, 0]);
        // An uncoloured pattern paints in the operator's colour whatever the cell drew, so its
        // own alpha is what is kept and its colour is discarded.
        let colour = match uncoloured {
            Some(c) => [c.r, c.g, c.b],
            None => [
                f64::from(rgba[0]) / 255.0,
                f64::from(rgba[1]) / 255.0,
                f64::from(rgba[2]) / 255.0,
            ],
        };
        Sample {
            colour,
            coverage: f64::from(rgba[3]) / 255.0,
        }
    }
}

/// Which repeat of a cell a pattern-space coordinate falls in, and where inside it.
///
/// A **zero step is not tiled in that direction**: the coordinate is used as it stands, so the
/// single cell is drawn once and anchored at the pattern-space origin, which is what the
/// specification says a zero `/XStep` or `/YStep` means.
fn split_cell(coord: f64, step: f64) -> (i64, f64) {
    if step == 0.0 {
        return (0, coord);
    }
    let index = (coord / step).floor();
    (index as i64, coord - index * step)
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
        if let Self::Tiling {
            cell,
            origin,
            scale,
            inverse,
            step,
            bbox,
            uncoloured,
        } = self
        {
            return Some(self.tile(
                cell,
                *origin,
                *scale,
                inverse,
                *step,
                *bbox,
                *uncoloured,
                x,
                y,
            ));
        }
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
            // A tiling pattern's coverage is its own alpha — the cell's, or the operator's for
            // an uncoloured one — so it multiplies the shape's coverage exactly as a shading's
            // gradient coverage does. A cell that is transparent between its marks therefore
            // lets the page through there, which is what a texture with gaps in it means.
            Self::Tiling { .. } | Self::Shading { .. } => {
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

/// Stroke a polygon in a colour that changes from pixel to pixel.
///
/// `SCN` can name a pattern as the *stroke* colour just as `scn` can name one as the fill
/// colour, so a stroke is as capable of being a gradient as a fill is — and a gradient rule
/// is how a design tool draws a rule. Nothing about the outline differs: it comes from
/// [`Device::stroke_outline`], which is the same outline a one-colour stroke is filled from,
/// dashes and caps and joins included.
///
/// Returns whether anything was drawn.
pub fn stroke(
    device: &mut Device,
    shape: &Polygon,
    style: &StrokeStyle,
    colour: &FillColour,
    alpha: f64,
) -> bool {
    let outlines = device.stroke_outline(shape, style);
    let mut drawn = false;
    for outline in &outlines {
        // The non-zero rule, because that is the rule a one-colour stroke's outline is
        // filled under, and a dash's own winding has to come out the same either way.
        drawn |= polygon(device, outline, FillRule::NonZero, colour, alpha);
    }
    drawn
}
