//! A 2-D affine transform, in PDF's own convention.
//!
//! Six numbers, laid out as PDF writes them: `[a b c d e f]` maps `(x, y)` to
//! `(a·x + c·y + e, b·x + d·y + f)`. Keeping the specification's layout rather than
//! picking a friendlier one matters, because a matrix read from a file has to mean what
//! it means and be written back the same way.
//!
//! This lives here rather than in the renderer because the *interpreter* needs it: the
//! current transformation is part of the graphics state, and text positioning is a
//! matrix. The renderer will use the same type.
//!
//! The six components are named as the specification names them, throughout the whole
//! module, so a line of this file can be compared with the page of ISO 32000-1 it came
//! from.
#![allow(clippy::many_single_char_names)]

/// `[a b c d e f]`, in PDF's layout.
///
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Default for Matrix {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Matrix {
    pub const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    #[must_use]
    pub fn new(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) -> Self {
        Self { a, b, c, d, e, f }
    }

    /// Read `[a b c d e f]` from an array of at least six numbers.
    #[must_use]
    pub fn from_slice(v: &[f64]) -> Option<Self> {
        let [a, b, c, d, e, f, ..] = v else {
            return None;
        };
        Some(Self::new(*a, *b, *c, *d, *e, *f))
    }

    /// The six numbers, in the order PDF writes them.
    #[must_use]
    pub fn to_array(self) -> [f64; 6] {
        [self.a, self.b, self.c, self.d, self.e, self.f]
    }

    /// `self` applied to `other`, which is what `cm` means: the new matrix acts on the
    /// user space *first*, and the current transformation acts on the result.
    ///
    /// With `apply` reading the six numbers as rows `[a c e]` and `[b d f]`, the
    /// product is the ordinary one:
    ///
    /// ```text
    /// a' = a·A + c·B     b' = b·A + d·B
    /// c' = a·C + c·D     d' = b·C + d·D
    /// e' = a·E + c·F + e f' = b·E + d·F + f
    /// ```
    #[must_use]
    pub fn concat(self, other: Self) -> Self {
        Self {
            a: self.a * other.a + self.c * other.b,
            b: self.b * other.a + self.d * other.b,
            c: self.a * other.c + self.c * other.d,
            d: self.b * other.c + self.d * other.d,
            e: self.a * other.e + self.c * other.f + self.e,
            f: self.b * other.e + self.d * other.f + self.f,
        }
    }

    #[must_use]
    pub fn translate(dx: f64, dy: f64) -> Self {
        Self::new(1.0, 0.0, 0.0, 1.0, dx, dy)
    }

    #[must_use]
    pub fn scale(sx: f64, sy: f64) -> Self {
        Self::new(sx, 0.0, 0.0, sy, 0.0, 0.0)
    }

    /// A rotation, in degrees, counter-clockwise.
    #[must_use]
    pub fn rotate(degrees: f64) -> Self {
        let r = degrees.to_radians();
        Self::new(r.cos(), r.sin(), -r.sin(), r.cos(), 0.0, 0.0)
    }

    /// Map a point.
    #[must_use]
    pub fn apply(self, x: f64, y: f64) -> (f64, f64) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    /// Map a direction, ignoring the translation. A stroke width is scaled by this, not
    /// by the full transform, which is the difference between a hairline and a wall.
    #[must_use]
    pub fn apply_vector(self, x: f64, y: f64) -> (f64, f64) {
        (self.a * x + self.c * y, self.b * x + self.d * y)
    }

    /// The determinant, which is zero exactly when the transform collapses a dimension.
    #[must_use]
    pub fn determinant(self) -> f64 {
        self.a * self.d - self.b * self.c
    }

    /// The inverse, or `None` when there is none.
    #[must_use]
    pub fn inverse(self) -> Option<Self> {
        let det = self.determinant();
        if det == 0.0 || !det.is_finite() {
            return None;
        }
        let inv = 1.0 / det;
        Some(Self {
            a: self.d * inv,
            b: -self.b * inv,
            c: -self.c * inv,
            d: self.a * inv,
            e: (self.c * self.f - self.d * self.e) * inv,
            f: (self.b * self.e - self.a * self.f) * inv,
        })
    }

    /// Is this the identity, to within a tolerance a float comparison can justify?
    #[must_use]
    pub fn is_identity(self) -> bool {
        const EPS: f64 = 1e-9;
        (self.a - 1.0).abs() < EPS
            && self.b.abs() < EPS
            && self.c.abs() < EPS
            && (self.d - 1.0).abs() < EPS
            && self.e.abs() < EPS
            && self.f.abs() < EPS
    }

    /// The scale factor to apply to a stroke width: the square root of the determinant
    /// of the linear part, which is the mean of the two axis scales.
    #[must_use]
    pub fn mean_scale(self) -> f64 {
        let det = self.determinant().abs();
        if det.is_finite() && det > 0.0 {
            det.sqrt()
        } else {
            // A degenerate transform: fall back to the larger axis so a line still draws.
            (self.a * self.a + self.b * self.b)
                .max(self.c * self.c + self.d * self.d)
                .sqrt()
        }
    }

    /// The angle of the x axis, in degrees counter-clockwise, in `0..360`.
    #[must_use]
    pub fn rotation_degrees(self) -> f64 {
        if self.a == 0.0 && self.b == 0.0 {
            return 0.0;
        }
        let deg = self.b.atan2(self.a).to_degrees();
        deg.rem_euclid(360.0)
    }
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect`, which is what a test is for; the
    // panic-free rule is about what the product does with a file, not about tests.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn the_identity_changes_nothing() {
        let m = Matrix::IDENTITY;
        assert_eq!(m.apply(3.0, 4.0), (3.0, 4.0));
        assert!(m.is_identity());
        assert_eq!(m.inverse().expect("an identity inverts"), Matrix::IDENTITY);
    }

    #[test]
    fn a_translation_moves_a_point_but_not_a_direction() {
        let m = Matrix::translate(10.0, -5.0);
        assert_eq!(m.apply(1.0, 2.0), (11.0, -3.0));
        assert_eq!(
            m.apply_vector(1.0, 2.0),
            (1.0, 2.0),
            "a direction is a point at the origin, translated back"
        );
    }

    #[test]
    fn concatenation_is_in_the_order_the_specification_says() {
        // `2 0 0 2 0 0 cm 1 0 0 1 10 0 cm`: the shift is in the space that already has
        // the scale applied, so the origin lands at 20. The other order gives 10, and
        // getting it wrong moves every subsequent mark on the page.
        let scale = Matrix::scale(2.0, 2.0);
        let shift = Matrix::translate(10.0, 0.0);
        assert!(near(scale.concat(shift).apply(0.0, 0.0).0, 20.0));
        assert!(near(shift.concat(scale).apply(0.0, 0.0).0, 10.0));
    }

    #[test]
    fn a_translation_after_a_rotation_composes_correctly() {
        // Rotate a quarter turn, then shift right in the *rotated* space: the shift
        // appears to go up on the page.
        let r = Matrix::rotate(90.0);
        let s = r.concat(Matrix::translate(10.0, 0.0));
        let (x, y) = s.apply(0.0, 0.0);
        assert!(near(x, 0.0) && near(y, 10.0), "got ({x}, {y})");
    }

    #[test]
    fn an_inverse_undoes_the_transform() {
        let m = Matrix::new(2.0, 0.5, -0.25, 3.0, 7.0, -4.0);
        let inv = m.inverse().expect("a non-degenerate matrix inverts");
        let (x, y) = m.apply(11.0, -5.0);
        let (bx, by) = inv.apply(x, y);
        assert!(near(bx, 11.0) && near(by, -5.0), "got ({bx}, {by})");
    }

    #[test]
    fn a_degenerate_matrix_has_no_inverse() {
        assert!(Matrix::scale(0.0, 1.0).inverse().is_none());
        assert!(
            Matrix::new(1.0, 2.0, 2.0, 4.0, 0.0, 0.0)
                .inverse()
                .is_none()
        );
    }

    #[test]
    fn a_rotation_turns_the_x_axis() {
        let m = Matrix::rotate(90.0);
        let (x, y) = m.apply(1.0, 0.0);
        assert!(near(x, 0.0) && near(y, 1.0), "got ({x}, {y})");
        assert!(near(m.rotation_degrees(), 90.0));
        // A full turn is the identity, not a rotation of zero by a rounding error.
        assert!(Matrix::rotate(360.0).is_identity());
        assert!(
            !Matrix::rotate(-90.0).is_identity(),
            "a quarter turn is not the identity"
        );
        assert!(near(Matrix::rotate(-90.0).rotation_degrees(), 270.0));
    }

    #[test]
    fn a_degenerate_transform_still_reports_a_scale() {
        // A renderer must draw *something* for a zero-width transform rather than
        // dividing by zero.
        let s = Matrix::scale(0.0, 0.0).mean_scale();
        assert!(s.is_finite());
    }

    #[test]
    fn the_six_numbers_round_trip() {
        let m = Matrix::new(1.5, -2.0, 3.25, 4.0, 5.0, 6.0);
        let arr = m.to_array();
        assert_eq!(Matrix::from_slice(&arr), Some(m));
        assert_eq!(
            Matrix::from_slice(&[1.0, 2.0]),
            None,
            "a short array is not a matrix"
        );
    }
}
