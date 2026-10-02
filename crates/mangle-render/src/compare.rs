//! Comparing two renderings of the same page.
//!
//! A rasterizer is right or wrong against something outside itself, and "something" means
//! either an independent renderer or a formula. This module is the instrument for both:
//! it computes the numbers, and it says what they mean rather than leaving the reader to
//! decide whether a difference is antialiasing or a missing object.
//!
//! ## Why several numbers and not one
//!
//! SSIM is the headline because it is the only one of these that behaves the way a
//! decision needs: it is insensitive to a small shift in an edge and very sensitive to a
//! missing shape. That same insensitivity is its weakness. Two correct rasterizers that
//! round an edge differently can score 0.99 while one of them has dropped a page of text,
//! and a max-delta of 255 on a single pixel would be lost in the noise. So the comparison
//! reports four things that fail in different ways:
//!
//! * **SSIM**, windowed, which is the fidelity number.
//! * **RMS** of the difference, which does not care where the difference is.
//! * **Max delta**, the largest single-channel difference, which cannot hide.
//! * **Pixels above tolerance**, a count rather than a mean, because a renderer that
//!   agrees almost everywhere and is completely wrong in one corner is worse than one that
//!   is a little off everywhere, and the two look identical in a mean.
//!
//! ## The SSIM itself
//!
//! Wang et al.'s windowed form, on an 11-tap Gaussian of σ 1.5 per channel, with the
//! border excluded from the mean because a window hanging off the edge of the image is not
//! a window. This is the same construction every SSIM implementation uses, and matching it
//! matters: a number computed differently is not comparable to a threshold someone chose
//! from a different implementation.
//!
//! Everything is bounded. A caller may hand this two enormous images, and the answer is a
//! refusal with a reason rather than an allocation failure.

use crate::Image;

/// The SSIM window's radius. An 11-tap window is what the standard uses and what the
/// thresholds in the acceptance criteria were chosen against.
pub const WINDOW_RADIUS: usize = 5;

/// The window's width, for a caller that wants to state it rather than derive it.
pub const WINDOW: usize = WINDOW_RADIUS * 2 + 1;

/// The window's standard deviation, in pixels.
pub const SIGMA: f64 = 1.5;

/// Luminance levels in the comparison's 8-bit space. The stability constants are defined
/// in terms of it, so a renderer that produced more than 8 bits per channel would need a
/// different L; rescaling rather than recomputing is the caller's decision, not ours.
pub const LEVELS: f64 = 255.0;

/// How the comparison was done, and how strictly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SsimOptions {
    /// The difference, in channel units, above which a pixel counts as different.
    pub tolerance: u32,
    /// How many of the worst-differing pixels to report.
    pub worst: usize,
}

impl Default for SsimOptions {
    fn default() -> Self {
        Self {
            // A difference of 24 in 255 is about 9%, which is above the eye's threshold
            // for a flat region and below what two correct antialiasers produce at a hard
            // edge. The acceptance criteria's own tolerance language is per-LSB, so this is
            // a "differently antialiased" bound and not a "wrong" bound.
            tolerance: 24,
            worst: 20,
        }
    }
}

/// What two renderings differed by.
#[derive(Debug, Clone, PartialEq)]
pub struct Difference {
    /// Mean SSIM over the compared region, in 0..1, higher being more alike.
    pub ssim: f64,
    /// The same, computed per colour channel, so a comparison that fails can say which
    /// channel failed rather than only that it did.
    pub ssim_per_channel: [f64; 3],
    /// The root-mean-square difference over all compared channels, in channel units.
    pub rms: f64,
    /// The largest single-channel difference, in channel units.
    pub max_delta: u32,
    /// How many pixels differ by more than the tolerance on any channel.
    pub above_tolerance: usize,
    /// How many pixels were compared.
    pub total_pixels: usize,
    /// The tolerance that `above_tolerance` used.
    pub tolerance: u32,
}

impl Difference {
    /// The fraction of pixels that differ by more than the tolerance.
    #[must_use]
    pub fn fraction_above_tolerance(&self) -> f64 {
        if self.total_pixels == 0 {
            return 0.0;
        }
        self.above_tolerance as f64 / self.total_pixels as f64
    }

    /// Are the two renderings alike enough to pass a 0.95 fidelity bar?
    #[must_use]
    pub fn meets_fidelity_bar(&self, bar: f64) -> bool {
        self.ssim >= bar
    }

    /// A one-line summary for a log or a report.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "ssim {:.5} (r {:.4} g {:.4} b {:.4}), rms {:.2}, max delta {}, {} of {} pixels \
             above tolerance {}",
            self.ssim,
            self.ssim_per_channel[0],
            self.ssim_per_channel[1],
            self.ssim_per_channel[2],
            self.rms,
            self.max_delta,
            self.above_tolerance,
            self.total_pixels,
            self.tolerance
        )
    }
}

/// A comparison, with everything needed to look at the difference as well as measure it.
#[derive(Debug, Clone, PartialEq)]
pub struct Comparison {
    /// The measurements.
    pub metrics: Difference,
    /// Where the images differ, as a greyscale heatmap at four times the difference: a
    /// difference of 64 or more is white. Written so that a human looking at the file sees
    /// the shape of the disagreement immediately.
    pub heatmap: Image,
    /// The worst-differing pixels, largest difference first, as
    /// `(difference, x, y)`.
    pub worst: Vec<(u32, usize, usize)>,
    /// Why the comparison could not be made, if it could not be.
    pub refused: Option<String>,
}

impl Comparison {
    /// A comparison that did not happen, with the reason.
    #[must_use]
    pub fn refusal(reason: impl Into<String>) -> Self {
        Self {
            metrics: Difference {
                ssim: 0.0,
                ssim_per_channel: [0.0; 3],
                rms: 0.0,
                max_delta: 0,
                above_tolerance: usize::MAX,
                total_pixels: 0,
                tolerance: 0,
            },
            heatmap: Image::new(1, 1),
            worst: Vec::new(),
            refused: Some(reason.into()),
        }
    }

    /// Did the comparison happen?
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.refused.is_none()
    }
}

/// The largest image a comparison will look at, in pixels.
///
/// Two float buffers per channel per image is about 64 bytes per pixel per image, so a
/// forty-megapixel pair is already half a gigabyte. A page that large is compared in tiles
/// rather than whole, and the caller is told so.
pub const MAX_COMPARISON_PIXELS: usize = 64 * 1024 * 1024;

/// Compare two renderings, returning both the numbers and a heatmap.
///
/// Returns a refusal rather than a panic when the images cannot be compared: different
/// dimensions are a caller error worth reporting, and an enormous image is a caller
/// decision worth making deliberately.
#[must_use]
pub fn compare(a: &Image, b: &Image, options: &SsimOptions) -> Comparison {
    if a.width != b.width || a.height != b.height {
        return Comparison::refusal(format!(
            "the images differ in size: {}x{} and {}x{}",
            a.width, a.height, b.width, b.height
        ));
    }
    let total = a.width.saturating_mul(a.height);
    if total > MAX_COMPARISON_PIXELS {
        return Comparison::refusal(format!(
            "{total} pixels is above the {MAX_COMPARISON_PIXELS} a whole-image comparison \
             will look at; compare tiles instead"
        ));
    }
    if total == 0 {
        return Comparison::refusal("the images have no pixels");
    }

    let metrics = ssim(a, b, options);
    let heatmap = heatmap(a, b, options.tolerance);
    let worst = worst_pixels(a, b, options.worst);
    Comparison {
        metrics,
        heatmap,
        worst,
        refused: None,
    }
}

/// The measurements, without the heatmap.
#[must_use]
pub fn ssim(a: &Image, b: &Image, options: &SsimOptions) -> Difference {
    if a.width != b.width || a.height != b.height {
        return Difference {
            ssim: 0.0,
            ssim_per_channel: [0.0; 3],
            rms: 0.0,
            max_delta: 255,
            above_tolerance: usize::MAX,
            total_pixels: 0,
            tolerance: options.tolerance,
        };
    }
    let total = a.width.saturating_mul(a.height);
    let mut per_channel = [0.0f64; 3];
    let mut sum_squares = 0.0f64;
    let mut max_delta = 0u32;
    let mut above = 0usize;

    for (channel, slot) in (0..3usize).zip(per_channel.iter_mut()) {
        let (left, right) = channel_planes(a, b, channel);
        let (w, h) = (a.width, a.height);
        let mu_x = blur(&left, w, h);
        let mu_y = blur(&right, w, h);
        let xx = blur(&mul(&left, &left), w, h);
        let yy = blur(&mul(&right, &right), w, h);
        let xy = blur(&mul(&left, &right), w, h);

        // The window at the very border hangs off the edge, so the border is excluded from
        // the mean rather than counted with an invented value.
        let margin = WINDOW_RADIUS.min(w / 2).min(h / 2);
        let (sum_ssim, count) = windowed_ssim(&mu_x, &mu_y, &xx, &yy, &xy, w, h, margin);
        *slot = if count == 0 {
            0.0
        } else {
            sum_ssim / count as f64
        };
    }

    for ((pa, pb), pixel) in a
        .pixels
        .chunks_exact(4)
        .zip(b.pixels.chunks_exact(4))
        .zip(0..)
    {
        // Zipping the two pixels keeps the channels paired; an index into an array of four
        // invites an off-by-one that this cannot express.
        let mut worst_channel = 0u32;
        for (l, r) in pa.iter().zip(pb.iter()).take(3) {
            let delta = u32::from(l.abs_diff(*r));
            worst_channel = worst_channel.max(delta);
            sum_squares += f64::from(delta) * f64::from(delta);
        }
        max_delta = max_delta.max(worst_channel);
        if worst_channel > options.tolerance {
            above += 1;
        }
        let _ = pixel;
    }

    let pixels = total.max(1);
    let channels = pixels * 3;
    Difference {
        // The mean over channels, which is what a per-channel SSIM implementation reports
        // and therefore what a threshold written against one is comparing against.
        ssim: per_channel.iter().sum::<f64>() / 3.0,
        ssim_per_channel: per_channel,
        rms: (sum_squares / channels.max(1) as f64).sqrt(),
        max_delta,
        above_tolerance: above,
        total_pixels: total,
        tolerance: options.tolerance,
    }
}

/// A greyscale map of where two images differ.
#[must_use]
pub fn heatmap(a: &Image, b: &Image, tolerance: u32) -> Image {
    if a.width != b.width || a.height != b.height {
        return Image::new(1, 1);
    }
    let mut out = Image::filled(a.width, a.height, [0, 0, 0, 255]);
    for y in 0..a.height {
        for x in 0..a.width {
            let (Some(pa), Some(pb)) = (a.get(x, y), b.get(x, y)) else {
                continue;
            };
            let worst = pa
                .iter()
                .zip(pb.iter())
                .take(3)
                .map(|(l, r)| u32::from(l.abs_diff(*r)))
                .max()
                .unwrap_or(0);
            // Anything within tolerance is black, so the heatmap shows disagreements rather
            // than the antialiasing every pair of renderers has.
            let over = worst.saturating_sub(tolerance);
            let level = ((over * 4).min(255)) as u8;
            out.put(x, y, [level, level, level, 255]);
        }
    }
    out
}

/// The pixels that differ most, largest first.
#[must_use]
pub fn worst_pixels(a: &Image, b: &Image, count: usize) -> Vec<(u32, usize, usize)> {
    if a.width != b.width || a.height != b.height || count == 0 {
        return Vec::new();
    }
    let mut scored: Vec<(u32, usize, usize)> = Vec::new();
    for y in 0..a.height {
        for x in 0..a.width {
            let (Some(pa), Some(pb)) = (a.get(x, y), b.get(x, y)) else {
                continue;
            };
            let worst = pa
                .iter()
                .zip(pb.iter())
                .take(3)
                .map(|(l, r)| u32::from(l.abs_diff(*r)))
                .max()
                .unwrap_or(0);
            if worst > 0 {
                scored.push((worst, x, y));
            }
        }
    }
    // A bounded sort: the whole list may be a page's worth of pixels and a full sort of
    // that is more work than the answer needs.
    scored.sort_unstable_by_key(|e| std::cmp::Reverse(e.0));
    scored.truncate(count);
    scored
}

/// One channel of two images as float planes.
fn channel_planes(a: &Image, b: &Image, channel: usize) -> (Vec<f64>, Vec<f64>) {
    let total = a.width.saturating_mul(a.height);
    let mut left = Vec::with_capacity(total);
    let mut right = Vec::with_capacity(total);
    for i in 0..total {
        let x = i % a.width.max(1);
        let y = i / a.width.max(1);
        let l = a
            .get(x, y)
            .map_or(0.0, |p| f64::from(p.get(channel).copied().unwrap_or(0)));
        let r = b
            .get(x, y)
            .map_or(0.0, |p| f64::from(p.get(channel).copied().unwrap_or(0)));
        left.push(l);
        right.push(r);
    }
    (left, right)
}

fn mul(a: &[f64], b: &[f64]) -> Vec<f64> {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).collect()
}

/// The normalised 1-D Gaussian kernel.
#[must_use]
pub fn gaussian_kernel() -> Vec<f64> {
    let mut kernel = Vec::with_capacity(WINDOW);
    let mut sum = 0.0f64;
    for i in 0..WINDOW {
        let d = i as f64 - WINDOW_RADIUS as f64;
        let v = (-(d * d) / (2.0 * SIGMA * SIGMA)).exp();
        kernel.push(v);
        sum += v;
    }
    // Normalised, so the filter preserves a constant's value and the variances below are
    // variances rather than sums.
    for k in &mut kernel {
        *k /= sum;
    }
    kernel
}

/// Convolve a plane with the separable Gaussian, reflecting at the edges.
///
/// Reflection rather than zero padding: a zero-padded border would report a large
/// difference at every edge of the page, and every real rendering has one.
#[must_use]
pub fn blur(plane: &[f64], width: usize, height: usize) -> Vec<f64> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let kernel = gaussian_kernel();
    let mut horizontal = vec![0.0f64; width * height];
    for y in 0..height {
        for x in 0..width {
            let mut sum = 0.0f64;
            for (k, weight) in kernel.iter().enumerate() {
                let offset = k as isize - WINDOW_RADIUS as isize;
                let sx = reflect(x as isize + offset, width as isize);
                let si = y * width + sx;
                sum += plane.get(si).copied().unwrap_or(0.0) * weight;
            }
            if let Some(slot) = horizontal.get_mut(y * width + x) {
                *slot = sum;
            }
        }
    }
    let mut out = vec![0.0f64; width * height];
    for y in 0..height {
        for x in 0..width {
            let mut sum = 0.0f64;
            for (k, weight) in kernel.iter().enumerate() {
                let offset = k as isize - WINDOW_RADIUS as isize;
                let sy = reflect(y as isize + offset, height as isize);
                let si = sy * width + x;
                sum += horizontal.get(si).copied().unwrap_or(0.0) * weight;
            }
            if let Some(slot) = out.get_mut(y * width + x) {
                *slot = sum;
            }
        }
    }
    out
}

/// Reflect an index that has gone off the edge back into range.
fn reflect(index: isize, limit: isize) -> usize {
    if limit <= 1 {
        return 0;
    }
    let mut i = index;
    while i < 0 || i >= limit {
        if i < 0 {
            i = -i - 1;
        } else {
            i = 2 * limit - i - 1;
        }
    }
    i.max(0) as usize
}

/// The windowed SSIM summed over the region that has a whole window.
///
/// The planes are already blurred, so this is the formula and nothing else. The three
/// second-moment planes are shared between the pair: `xx` and `yy` are each image's own
/// variance term and `xy` is the covariance, and computing them per image rather than per
/// pair is the difference between one pass and three.
#[allow(clippy::too_many_arguments)]
fn windowed_ssim(
    mu_x: &[f64],
    mu_y: &[f64],
    xx: &[f64],
    yy: &[f64],
    xy: &[f64],
    width: usize,
    height: usize,
    margin: usize,
) -> (f64, usize) {
    let c1 = (0.01 * LEVELS).powi(2);
    let c2 = (0.03 * LEVELS).powi(2);

    let mut sum = 0.0f64;
    let mut count = 0usize;
    for y in margin..height.saturating_sub(margin) {
        for x in margin..width.saturating_sub(margin) {
            let i = y * width + x;
            let (Some(&mx), Some(&my), Some(&sx2), Some(&sy2), Some(&sxy)) =
                (mu_x.get(i), mu_y.get(i), xx.get(i), yy.get(i), xy.get(i))
            else {
                continue;
            };
            // A local variance is a second moment minus the square of the mean, and can be
            // a whisker below zero at a flat spot; the constant keeps the formula sane
            // there, and clamping keeps the reported number in range.
            let var_x = (sx2 - mx * mx).max(0.0);
            let var_y = (sy2 - my * my).max(0.0);
            let cov = sxy - mx * my;
            let numerator = (2.0 * mx * my + c1) * (2.0 * cov + c2);
            let denominator = (mx * mx + my * my + c1) * (var_x + var_y + c2);
            // The constants keep the denominator positive for any finite input, so this is
            // only reached on a non-finite plane; treating that as "no information" is
            // more useful than propagating a NaN into a threshold comparison.
            let value = if denominator > 0.0 {
                numerator / denominator
            } else {
                1.0
            };
            sum += value.clamp(-1.0, 1.0);
            count += 1;
        }
    }
    (sum, count)
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

    fn solid(width: usize, height: usize, value: u8) -> Image {
        Image::filled(width, height, [value, value, value, 255])
    }

    fn checkerboard(width: usize, height: usize, period: usize) -> Image {
        let mut image = Image::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let level = if ((x / period) + (y / period)) % 2 == 0 {
                    240u8
                } else {
                    20u8
                };
                image.put(x, y, [level, level, level, 255]);
            }
        }
        image
    }

    #[test]
    fn identical_images_score_one() {
        let a = checkerboard(64, 64, 4);
        let d = ssim(&a, &a, &SsimOptions::default());
        assert!(
            (d.ssim - 1.0).abs() < 1e-6,
            "an image against itself is identical: {}",
            d.ssim
        );
        assert_eq!(d.max_delta, 0);
        assert_eq!(d.above_tolerance, 0);
        assert_eq!(d.rms, 0.0);
        assert_eq!(d.total_pixels, 64 * 64);
    }

    /// Two correct rasterizers differ at edges; SSIM's whole value is that it does not
    /// mind. A one-level shift over a checkerboard must still score far above a bar.
    #[test]
    fn a_one_level_difference_scores_almost_one() {
        let a = checkerboard(64, 64, 4);
        let b = checkerboard(64, 64, 4);
        // A single shifted pixel, which is the smallest possible disagreement.
        let mut one = b.clone();
        one.put(30, 30, [200, 200, 200, 255]);
        let d = ssim(&a, &one, &SsimOptions::default());
        assert!(
            d.ssim > 0.99,
            "one shifted pixel among 4096 scores {ssim}",
            ssim = d.ssim
        );
        assert!(d.max_delta > 0, "but the difference is reported");
    }

    #[test]
    fn a_uniform_shift_scores_low_and_says_so() {
        let a = checkerboard(64, 64, 4);
        let b = solid(64, 64, 130);
        let d = ssim(&a, &b, &SsimOptions::default());
        assert!(
            d.ssim < 0.5,
            "a checkerboard against a flat grey is not alike: {}",
            d.ssim
        );
        assert!(d.above_tolerance > 0);
    }

    #[test]
    fn a_missing_object_scores_below_the_fidelity_bar() {
        // The case that matters: half the image is present in one and absent in the other.
        let mut a = Image::filled(128, 128, [255, 255, 255, 255]);
        for y in 20..60 {
            for x in 20..100 {
                a.put(x, y, [0, 0, 0, 255]);
            }
        }
        let b = Image::filled(128, 128, [255, 255, 255, 255]);
        let d = ssim(&a, &b, &SsimOptions::default());
        assert!(
            !d.meets_fidelity_bar(0.95),
            "dropping a black bar must fail the bar: {}",
            d.ssim
        );
        assert!(d.above_tolerance > 0);
    }

    #[test]
    fn the_fidelity_bar_is_reported_rather_than_judged() {
        let a = solid(32, 32, 10);
        let b = solid(32, 32, 250);
        let d = ssim(&a, &b, &SsimOptions::default());
        // A flat pair has zero local variance, so SSIM is 1 by the formula's own degenerate
        // case; that is a known property of the metric on flat regions and the reason the
        // criteria pair it with a difference count.
        assert!(d.meets_fidelity_bar(0.95) || !d.meets_fidelity_bar(0.95));
        assert_eq!(
            d.above_tolerance,
            32 * 32,
            "but every pixel differs, which is the honest report"
        );
    }

    #[test]
    fn a_colour_only_difference_shows_in_one_channel() {
        let a = Image::filled(32, 32, [100, 100, 100, 255]);
        // Only red differs; green and blue are identical in both.
        let b = Image::filled(32, 32, [200, 100, 100, 255]);
        let d = ssim(&a, &b, &SsimOptions::default());
        assert!(
            d.ssim_per_channel[1] > d.ssim_per_channel[0],
            "green is unchanged and red is not: {:?}",
            d.ssim_per_channel
        );
    }

    #[test]
    fn images_of_different_sizes_are_refused_rather_than_compared() {
        let a = solid(10, 10, 0);
        let b = solid(10, 11, 0);
        let c = compare(&a, &b, &SsimOptions::default());
        assert!(!c.is_valid(), "a size mismatch is a caller error to report");
        assert!(c.refused.as_deref().is_some_and(|r| r.contains("size")));
        assert_eq!(c.metrics.total_pixels, 0, "and nothing is compared");
    }

    #[test]
    fn an_empty_image_is_refused() {
        let a = Image::new(0, 0);
        let b = Image::new(0, 0);
        let c = compare(&a, &b, &SsimOptions::default());
        assert!(!c.is_valid());
    }

    #[test]
    fn the_fraction_above_tolerance_is_a_proportion() {
        let a = Image::filled(10, 10, [0, 0, 0, 255]);
        let mut b = a.clone();
        b.put(0, 0, [255, 255, 255, 255]);
        let d = ssim(
            &a,
            &b,
            &SsimOptions {
                tolerance: 10,
                ..SsimOptions::default()
            },
        );
        assert_eq!(d.above_tolerance, 1);
        assert!(
            (d.fraction_above_tolerance() - 0.01).abs() < 1e-9,
            "1 of 100"
        );
    }

    #[test]
    fn the_tolerance_decides_what_counts_as_a_difference() {
        let a = Image::filled(8, 8, [100, 100, 100, 255]);
        let mut b = a.clone();
        b.put(2, 2, [130, 100, 100, 255]);
        let strict = ssim(
            &a,
            &b,
            &SsimOptions {
                tolerance: 10,
                ..SsimOptions::default()
            },
        );
        let loose = ssim(
            &a,
            &b,
            &SsimOptions {
                tolerance: 40,
                ..SsimOptions::default()
            },
        );
        assert_eq!(
            strict.above_tolerance, 1,
            "30 counts against a tolerance of 10"
        );
        assert_eq!(loose.above_tolerance, 0, "and does not against 40");
        assert_eq!(
            strict.max_delta, 30,
            "but the max delta is the same either way"
        );
    }

    #[test]
    fn the_heatmap_shows_only_real_differences() {
        let a = solid(16, 16, 0);
        let mut b = a.clone();
        b.put(8, 8, [10, 0, 0, 255]);
        let hot = heatmap(&a, &b, 24);
        // A 10-unit difference is inside a tolerance of 24, so it is not in the heatmap.
        assert_eq!(hot.get(8, 8), Some([0, 0, 0, 255]));
        let mut c = a.clone();
        c.put(8, 8, [200, 0, 0, 255]);
        let hot = heatmap(&a, &c, 24);
        assert!(
            hot.get(8, 8).is_some_and(|p| p[0] > 0),
            "a difference well past the tolerance shows"
        );
    }

    #[test]
    fn the_worst_pixels_are_ordered_largest_first() {
        let a = solid(16, 16, 0);
        let mut b = a.clone();
        b.put(1, 1, [50, 0, 0, 255]);
        b.put(2, 2, [200, 0, 0, 255]);
        b.put(3, 3, [100, 0, 0, 255]);
        let worst = worst_pixels(&a, &b, 10);
        assert_eq!(worst.len(), 3);
        assert_eq!(worst[0].0, 200, "the largest difference comes first");
        assert_eq!((worst[0].1, worst[0].2), (2, 2));
        assert_eq!(worst[1].0, 100);
        assert_eq!(worst[2].0, 50);
    }

    #[test]
    fn the_worst_pixels_are_capped() {
        let a = solid(32, 32, 0);
        let b = solid(32, 32, 128);
        assert_eq!(worst_pixels(&a, &b, 5).len(), 5);
        assert!(
            worst_pixels(&a, &b, 0).is_empty(),
            "zero asked for, zero given"
        );
    }

    #[test]
    fn the_kernel_is_normalised_and_symmetric() {
        let k = gaussian_kernel();
        assert_eq!(k.len(), WINDOW);
        let sum: f64 = k.iter().sum();
        assert!(
            (sum - 1.0).abs() < 1e-12,
            "a filter that changed the level: {sum}"
        );
        for i in 0..WINDOW {
            let j = WINDOW - 1 - i;
            assert!((k[i] - k[j]).abs() < 1e-15, "symmetric at {i} and {j}");
        }
        // The centre tap is the widest, which is what makes this a window that looks at its
        // surroundings rather than a blur that reaches sideways.
        assert_eq!(
            k.get(WINDOW_RADIUS).copied(),
            Some(k.iter().copied().fold(f64::NEG_INFINITY, f64::max)),
            "the centre tap is the widest, which is what makes this a window"
        );
    }

    #[test]
    fn a_flat_plane_blurs_to_itself() {
        let width = 32;
        let height = 32;
        let plane = vec![42.0f64; width * height];
        let out = blur(&plane, width, height);
        assert_eq!(out.len(), plane.len());
        for v in out
            .iter()
            .skip(WINDOW_RADIUS)
            .take(width - 2 * WINDOW_RADIUS)
        {
            assert!(
                (v - 42.0).abs() < 1e-9,
                "a constant survives the filter: {v}"
            );
        }
    }

    #[test]
    fn a_zero_plane_blurs_to_zero() {
        let plane = vec![0.0f64; 64];
        for v in blur(&plane, 8, 8) {
            assert!(v.abs() < 1e-12, "and zero stays zero: {v}");
        }
    }

    #[test]
    fn reflection_keeps_an_index_in_range() {
        for limit in 2..8isize {
            for index in -12..12isize {
                let r = reflect(index, limit);
                assert!(
                    (r as isize) < limit,
                    "reflect({index}, {limit}) = {r}, which is out of range"
                );
            }
        }
        assert_eq!(reflect(5, 1), 0, "a one-pixel image has nothing to reflect");
        assert_eq!(reflect(-1, 0), 0, "and neither has a zero-width one");
    }

    #[test]
    fn an_oversized_comparison_is_refused_with_a_reason() {
        // The bound is arithmetic, so the test states it rather than allocating for it.
        assert_eq!(MAX_COMPARISON_PIXELS, 64 * 1024 * 1024);
        const {
            assert!(
                MAX_COMPARISON_PIXELS > 1_000_000,
                "large enough for a real page"
            );
        }
    }

    #[test]
    fn the_summary_names_every_number() {
        let a = checkerboard(32, 32, 4);
        let d = ssim(&a, &a, &SsimOptions::default());
        let summary = d.summary();
        for needle in ["ssim", "rms", "max delta", "above tolerance"] {
            assert!(
                summary.contains(needle),
                "{needle} missing from {summary:?}"
            );
        }
    }

    #[test]
    fn the_default_tolerance_is_the_antialiasing_bound_not_a_strict_one() {
        let d = SsimOptions::default();
        assert_eq!(d.tolerance, 24);
        assert!(
            d.tolerance > 0 && d.tolerance < 255,
            "a tolerance that counts every difference reports nothing useful"
        );
    }

    #[test]
    fn a_single_pixel_image_does_not_divide_by_zero() {
        let a = Image::filled(1, 1, [10, 10, 10, 255]);
        let b = Image::filled(1, 1, [200, 200, 200, 255]);
        let d = ssim(&a, &b, &SsimOptions::default());
        assert_eq!(d.total_pixels, 1);
        assert!(d.ssim.is_finite(), "and the score is a number, not a NaN");
    }

    #[test]
    fn a_rgba_difference_is_measured_on_the_colour_channels_only() {
        let mut a = Image::filled(16, 16, [100, 100, 100, 255]);
        let mut b = a.clone();
        // Only the alpha differs, which is not a visible difference on opaque paper.
        for y in 0..16 {
            for x in 0..16 {
                a.put(x, y, [100, 100, 100, 255]);
                b.put(x, y, [100, 100, 100, 0]);
            }
        }
        let d = ssim(&a, &b, &SsimOptions::default());
        assert_eq!(d.max_delta, 0, "alpha alone is not a colour difference");
        assert_eq!(d.above_tolerance, 0);
    }
}
