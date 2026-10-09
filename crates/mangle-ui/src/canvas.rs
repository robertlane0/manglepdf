//! The mapping between a window point and a page point.
//!
//! # Why this is not arithmetic
//!
//! A click at (400, 300) has to become a point in the page's own coordinates, and the naive
//! version — subtract the page rect's corner, flip the y, divide by the zoom — is wrong the moment
//! the page has a `/Rotate` or a `/MediaBox` that does not start at the origin. Both are ordinary.
//!
//! So this reuses the renderer's own placement: `Placement::fit` is the matrix that takes page
//! space to the buffer the UI draws, so its inverse takes a pixel back to the page. A window point
//! becomes a pixel by where it falls in the page's rectangle, and the rectangle is where the
//! texture was drawn. One inverse, and the click agrees with the pixel — which is the property
//! that makes selection feel right and its absence is the classic "clicked one word, selected the
//! one above it" bug.
//!
//! # What this does not do
//!
//! It does not know about zoom. The window points arrive from a canvas that has already scaled the
//! page's rectangle, so zoom is baked into the rectangle and a divide by it here would apply it
//! twice — the same mistake as rendering a page at the square of its zoom.

use egui::{Pos2, Rect};

/// Where a page sits in the window, and how to get from one to the other.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PagePlacement {
    /// The page's rectangle in the canvas, in window points.
    pub rect: Rect,
    /// The texture's size in pixels, which is what the rectangle was filled from.
    pub pixels: (usize, usize),
    /// The renderer's placement: page space to buffer pixels.
    pub matrix: mangle_content::Matrix,
}

impl PagePlacement {
    /// The placement of a page drawn in `rect` from a buffer of `pixels`.
    #[must_use]
    pub fn new(rect: Rect, pixels: (usize, usize), matrix: mangle_content::Matrix) -> Self {
        Self {
            rect,
            pixels,
            matrix,
        }
    }

    /// The page point under a window point, or `None` when it is not on the page.
    ///
    /// A point outside the page is `None` rather than a clamped one: a canvas that reported a
    /// page point for a click that landed in the margin would select whatever happened to be
    /// near the edge, which is a selection the user did not make.
    #[must_use]
    pub fn to_page(&self, at: Pos2) -> Option<(f64, f64)> {
        if self.pixels.0 == 0 || self.pixels.1 == 0 {
            return None;
        }
        if !self.rect.contains(at) {
            return None;
        }
        let (px, py) = self.pixels;
        let across = (f64::from(at.x) - f64::from(self.rect.left())) / f64::from(self.rect.width())
            * px as f64;
        let down = (f64::from(at.y) - f64::from(self.rect.top())) / f64::from(self.rect.height())
            * py as f64;
        // The texture is the page as drawn, so the placement's inverse is the way back. Its own
        // bounds are checked by the rectangle above, which is the same box.
        let inverse = self.matrix.inverse()?;
        Some(inverse.apply(across, down))
    }

    /// The window point a page point is drawn at, or `None` when it is off the page.
    #[must_use]
    pub fn to_window(&self, x: f64, y: f64) -> Option<Pos2> {
        if self.pixels.0 == 0 || self.pixels.1 == 0 {
            return None;
        }
        let (px, py) = self.pixels;
        let (across, down) = self.matrix.apply(x, y);
        let rect_x =
            f64::from(self.rect.left()) + (across / px as f64) * f64::from(self.rect.width());
        let rect_y =
            f64::from(self.rect.top()) + (down / py as f64) * f64::from(self.rect.height());
        let point = Pos2::new(rect_x as f32, rect_y as f32);
        self.rect.contains(point).then_some(point)
    }

    /// The page point under a window point, clamped into the page.
    ///
    /// For a drag rather than a click: the gesture starts on the page and the pointer may leave
    /// it, and a delta computed from a clamped start is still the delta the user meant to make.
    #[must_use]
    pub fn to_page_clamped(&self, at: Pos2) -> Option<(f64, f64)> {
        let clamped = Pos2::new(
            at.x.clamp(self.rect.left(), self.rect.right()),
            at.y.clamp(self.rect.top(), self.rect.bottom()),
        );
        self.to_page(clamped)
    }
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect` and `unwrap`, which is what a test is for;
    // the panic-free rule is about what the product does with a file, not about tests.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use mangle_content::Matrix;

    /// A letter page drawn into a rectangle, at one pixel per point.
    fn placement() -> PagePlacement {
        // The renderer's own placement for a 612x792 canvas at scale 1: `y` is flipped and the
        // page's lower-left corner lands at the buffer's lower-left.
        let matrix = Matrix::new(1.0, 0.0, 0.0, -1.0, 0.0, 792.0);
        PagePlacement::new(
            Rect::from_min_size(Pos2::new(10.0, 20.0), egui::vec2(612.0, 792.0)),
            (612, 792),
            matrix,
        )
    }

    #[test]
    fn a_click_on_the_pages_lower_left_is_its_lower_left() {
        let p = placement();
        let got = p
            .to_page(Pos2::new(10.0, 20.0 + 792.0))
            .expect("on the page");
        assert!((got.0 - 0.0).abs() < 1e-9, "x was {}", got.0);
        assert!((got.1 - 0.0).abs() < 1e-9, "y was {}", got.1);
    }

    /// The y axis is the whole of the problem: a page counts up, a window counts down, and getting
    /// it backwards is invisible until someone clicks a glyph.
    #[test]
    fn a_click_low_on_the_page_is_high_on_the_window() {
        let p = placement();
        let top = p.to_page(Pos2::new(10.0, 20.0)).expect("on the page");
        assert!((top.1 - 792.0).abs() < 1e-9, "y was {}", top.1);
        let bottom = p
            .to_page(Pos2::new(10.0, 20.0 + 792.0))
            .expect("on the page");
        assert!((bottom.1 - 0.0).abs() < 1e-9, "y was {}", bottom.1);
    }

    /// A zoomed page is drawn bigger, and the same click lands on the same page point.
    #[test]
    fn a_zoomed_page_answers_the_same_way() {
        let p = PagePlacement::new(
            Rect::from_min_size(Pos2::new(10.0, 20.0), egui::vec2(1224.0, 1584.0)),
            (1224, 1584),
            Matrix::new(2.0, 0.0, 0.0, -2.0, 0.0, 1584.0),
        );
        let got = p
            .to_page(Pos2::new(10.0 + 612.0, 20.0 + 792.0))
            .expect("on the page");
        assert!((got.0 - 306.0).abs() < 1e-9, "x was {}", got.0);
        assert!((got.1 - 396.0).abs() < 1e-9, "y was {}", got.1);
    }

    /// A click outside the page is not a click at all, rather than one clamped to the edge.
    #[test]
    fn a_click_off_the_page_is_none() {
        let p = placement();
        assert!(p.to_page(Pos2::new(0.0, 0.0)).is_none());
        assert!(p.to_page(Pos2::new(10.0 + 613.0, 20.0)).is_none());
        assert!(p.to_page_clamped(Pos2::new(0.0, 0.0)).is_some());
    }

    /// The two directions are each other's inverse: a box drawn from page points must land under
    /// the pointer that dragged it.
    #[test]
    fn the_two_directions_round_trip() {
        let p = placement();
        let page = (123.5, 456.25);
        let window = p.to_window(page.0, page.1).expect("on the page");
        let back = p.to_page(window).expect("and back");
        assert!((back.0 - page.0).abs() < 1e-9, "x was {}", back.0);
        assert!((back.1 - page.1).abs() < 1e-9, "y was {}", back.1);
    }

    /// A rotated page: `/Rotate 90` turns the page's own coordinates, and a click must answer in
    /// them rather than in the window's.
    #[test]
    fn a_rotated_page_answers_in_its_own_coordinates() {
        use mangle_render::page::Placement;
        use mangle_syntax::Rect as PageRect;
        let rotated = Placement::fit(&PageRect::new(0.0, 0.0, 612.0, 792.0), (792, 612), 1.0, 90);
        let p = PagePlacement::new(
            Rect::from_min_size(Pos2::new(0.0, 0.0), egui::vec2(792.0, 612.0)),
            (792, 612),
            rotated.matrix,
        );
        // The page's lower-left corner appears top-left under a quarter turn.
        let got = p.to_page(Pos2::new(0.0, 0.0)).expect("on the page");
        assert!((got.0 - 0.0).abs() < 1e-9, "x was {}", got.0);
        assert!((got.1 - 0.0).abs() < 1e-9, "y was {}", got.1);
    }
}
