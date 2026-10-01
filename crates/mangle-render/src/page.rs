//! A page, turned into pixels.
//!
//! This is where the layers meet: the content interpreter says what was drawn and where
//! its bytes are, the rasterizer knows how to make that into coverage, and this file
//! runs one page's worth of marks through both and hands back an image.
//!
//! Two things are decided here and nowhere else.
//!
//! **Where the page goes on the canvas.** PDF's origin is the crop box's lower left and
//! y increases upward; a canvas's origin is its upper left and y increases downward. The
//! transformation between them is a scale and a flip, and getting the flip wrong renders
//! every page upside down — a mistake that looks like a bug in the rasterizer and is not.
//! `/Rotate` then turns the result by a right angle, which is a second, separate thing.
//!
//! **How far away the page may be scaled.** A viewer is asked for a zoom level and the
//! page is asked for its size in points, and the product of the two is a pixel count. A
//! file can ask for a page the size of a city block, so the scale is bounded and a
//! request beyond the bound is answered at the bound rather than by exhausting memory.

use mangle_content::{ContentStream, FillRule as ContentRule, Mark, Matrix, Resources, run_with};
use mangle_doc::Page;
use mangle_syntax::{Document, Rect as PageRect};

use crate::{
    Device, FillRule, Image, LineCap, LineJoin, Polygon, Rect, StrokeStyle, Viewport,
    transform_path,
};

/// The furthest a page may be scaled from its printed size.
///
/// A letter page at this scale is about 1400 pixels wide, which is past the point where
/// more pixels show anything a person can see. A viewer asking for more than this is
/// asking for detail the rasterizer does not have, and honouring it would turn a zoom
/// gesture into an allocation failure.
pub const MAX_SCALE: f64 = 16.0;

/// The largest pixel buffer one page may need, as a side in pixels.
///
/// The area bound matters more than the side: a page of unusual proportions is still
/// bounded in area, and a page that is square and enormous is caught by the side.
pub const MAX_SIDE: f64 = 16_384.0;

/// The most pixels one page may occupy.
pub const MAX_AREA: f64 = 64.0 * 1024.0 * 1024.0;

/// How a page was rendered, and what could not be rendered.
///
/// The reasons are the point. "Nothing was drawn" and "nothing was drawn because the one
/// form XObject referenced a font we do not have" are different facts, and a user
/// deciding whether to trust a page needs the second one.
#[derive(Debug, Clone)]
pub struct PageRender {
    /// The page, in pixels.
    pub image: Image,
    /// The scale that was used, which may be less than the one asked for.
    pub scale: f64,
    /// The marks that were drawn.
    pub marks: usize,
    /// Per-mark notes: an XObject that could not be resolved, a colour space that could
    /// not be converted, and so on.
    pub notes: Vec<String>,
}

impl Default for PageRender {
    fn default() -> Self {
        Self {
            image: Image::new(1, 1),
            scale: 1.0,
            marks: 0,
            notes: Vec::new(),
        }
    }
}

impl PageRender {
    /// Is there nothing to show?
    #[must_use]
    pub fn is_blank(&self) -> bool {
        self.image
            .pixels
            .chunks_exact(4)
            .all(|p| p == [255, 255, 255, 255])
    }
}

/// How a page should be drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderOptions {
    /// The desired pixels per point. Clamped to [`MAX_SCALE`].
    pub scale: f64,
    /// The paper colour, which is white unless the file says otherwise.
    pub paper: [u8; 4],
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            scale: 1.0,
            paper: [255, 255, 255, 255],
        }
    }
}

/// A page's rectangle, as the canvas should place it.
///
/// This is the whole of the page-placement problem: PDF's lower-left origin becomes the
/// canvas's upper-left, `/Rotate` turns the result, and `/UserUnit` scales it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// The transformation from page space to device pixels.
    pub matrix: Matrix,
    /// The canvas, in pixels.
    pub size: (usize, usize),
}

impl Placement {
    /// Where a page goes on a canvas of the given size.
    ///
    /// The canvas is fitted to the page rather than the page to the canvas, so the result
    /// has exactly the page's aspect ratio: a letter page in a square canvas is a
    /// letter-sized rectangle in the middle, not a stretched square.
    ///
    /// The order of the three steps is the whole of the problem, and getting it wrong
    /// shows up as a page that is flipped, rotated about the wrong corner, or both:
    ///
    /// 1. **Rotate in page space.** `/Rotate` turns the page's own coordinates, about
    ///    its own lower-left corner.
    /// 2. **Normalise.** The rotated corners may have negative coordinates; shifting them
    ///    so the lowest-left of their own bounding box is at the origin makes the page's
    ///    position depend on nothing but its size.
    /// 3. **Fit and flip.** Scale to the canvas, centre what is left over, and invert y
    ///    because a canvas counts down and a page counts up.
    #[must_use]
    pub fn fit(page: &PageRect, size: (usize, usize), scale: f64, rotate: i32) -> Self {
        let (w, h) = size;
        if w == 0 || h == 0 || page.width() <= 0.0 || page.height() <= 0.0 {
            return Self {
                matrix: Matrix::IDENTITY,
                size,
            };
        }
        let rotation = rotate_matrix(rotate);
        // The four corners, turned. A rectangle's corners bound it whatever it has been
        // through, which is why rotating the corners rather than the rectangle is the
        // whole of the rotation problem.
        let corners = [
            rotation.apply(page.left, page.bottom),
            rotation.apply(page.right, page.bottom),
            rotation.apply(page.right, page.top),
            rotation.apply(page.left, page.top),
        ];
        let min_x = corners.iter().map(|c| c.0).fold(f64::INFINITY, f64::min);
        let max_x = corners
            .iter()
            .map(|c| c.0)
            .fold(f64::NEG_INFINITY, f64::max);
        let min_y = corners.iter().map(|c| c.1).fold(f64::INFINITY, f64::min);
        let max_y = corners
            .iter()
            .map(|c| c.1)
            .fold(f64::NEG_INFINITY, f64::max);
        let (box_w, box_h) = ((max_x - min_x).max(0.0), (max_y - min_y).max(0.0));
        if box_w <= 0.0 || box_h <= 0.0 {
            return Self {
                matrix: Matrix::IDENTITY,
                size,
            };
        }

        // Fit inside the canvas: the smaller of the two ratios is what leaves no gap.
        let fit = (w as f64 / box_w).min(h as f64 / box_h) * scale;
        let drawn_w = box_w * fit;
        let drawn_h = box_h * fit;
        let offset_x = (w as f64 - drawn_w) / 2.0;
        let offset_y = (h as f64 - drawn_h) / 2.0;

        let matrix = Matrix::new(fit, 0.0, 0.0, -fit, offset_x, offset_y + drawn_h)
            .concat(Matrix::translate(-min_x, -min_y))
            .concat(rotation);
        Self { matrix, size }
    }
}

/// The transformation for a `/Rotate` value, applied to the page's own coordinates.
///
/// PDF's rotation is clockwise as the page is *displayed*, and the y-axis inversion that
/// turns page space into canvas space is applied after this — so the sign here is the
/// opposite of what "clockwise" suggests, and the only honest way to pin it is to say
/// what each quarter turn must do to a page's lower-left corner:
///
/// | `/Rotate` | the page's lower-left corner appears |
/// |---|---|
/// | 0 | bottom left |
/// | 90 | top left |
/// | 180 | top right |
/// | 270 | bottom right |
///
/// A value that is not a right angle is treated as none. `/Rotate` is defined to be a
/// multiple of ninety, and turning a page by an eighth of a turn would be a worse answer
/// than leaving it upright.
#[must_use]
pub fn rotate_matrix(degrees: i32) -> Matrix {
    match degrees.rem_euclid(360) {
        90 => Matrix::new(0.0, -1.0, 1.0, 0.0, 0.0, 0.0),
        180 => Matrix::new(-1.0, 0.0, 0.0, -1.0, 0.0, 0.0),
        270 => Matrix::new(0.0, 1.0, -1.0, 0.0, 0.0, 0.0),
        _ => Matrix::IDENTITY,
    }
}

/// Where a page's lower-left corner lands, for every right-angle `/Rotate`.
///
/// This is the property the rotation signs are pinned by, stated once so that a future
/// change to [`rotate_matrix`] has something to fail against.
#[must_use]
pub fn lower_left_corner(page: &PageRect, size: (usize, usize), rotate: i32) -> (f64, f64) {
    Placement::fit(page, size, 1.0, rotate)
        .matrix
        .apply(page.left, page.bottom)
}

/// The scale a page will actually be drawn at, after the bounds are applied.
#[must_use]
pub fn effective_scale(asked: f64) -> f64 {
    if !asked.is_finite() || asked <= 0.0 {
        return 1.0;
    }
    asked.min(MAX_SCALE)
}

/// Render one page.
///
/// The page's content is run by the content layer and each mark is drawn in turn, in the
/// order it was drawn. Marks that cannot be drawn become notes rather than exceptions: a
/// page with one unsupported feature is still worth showing.
#[must_use]
pub fn render_page(
    doc: &Document,
    page: &Page,
    resources: &Resources,
    options: RenderOptions,
) -> PageRender {
    let scale = effective_scale(options.scale);
    let crop = page.inherited.crop_rect();
    let rotate = page.inherited.rotation();
    let (disp_w, disp_h) = page.inherited.displayed_size();

    // The buffer is bounded in both dimensions before it is allocated, because a page
    // can ask for either.
    let pixels_w = (disp_w.max(1.0) * scale).min(MAX_SIDE);
    let pixels_h = (disp_h.max(1.0) * scale).min(MAX_SIDE);
    let area = pixels_w * pixels_h;
    let shrink = if area > MAX_AREA {
        (MAX_AREA / area).sqrt()
    } else {
        1.0
    };
    let size = (
        (pixels_w * shrink).round().max(1.0) as usize,
        (pixels_h * shrink).round().max(1.0) as usize,
    );

    let placement = Placement::fit(&crop, size, scale, rotate);
    let mut render = PageRender {
        image: Image::filled(size.0, size.1, options.paper),
        scale,
        marks: 0,
        notes: Vec::new(),
    };

    let content = page.contents(doc);
    if content.is_empty() {
        return render;
    }
    let stream = ContentStream::parse(&content);
    let executed = run_with(&stream, resources);
    render.notes.extend(executed.notes.iter().cloned());

    let mut device = Device::new(render.image);
    for (name, span) in &executed.unknown_operators {
        render.notes.push(format!(
            "operator `{}` at byte {} is not in the table and was not executed",
            String::from_utf8_lossy(name),
            span.start
        ));
    }
    for record in &executed.records {
        let to_device = placement.matrix.concat(record.ctm);
        draw_mark(
            &mut device,
            &record.mark,
            &to_device,
            record,
            &mut render.notes,
        );
        render.marks += 1;
    }
    render.image = device.into_image();
    render
}

/// Draw one mark.
fn draw_mark(
    device: &mut Device,
    mark: &Mark,
    to_device: &Matrix,
    record: &mangle_content::Record,
    notes: &mut Vec<String>,
) {
    match mark {
        Mark::Path {
            segments,
            fill,
            stroke,
            rule,
        } => {
            let polygon = transform_path(segments, to_device);
            if polygon.is_empty() {
                return;
            }
            if let Some(bounds) = polygon.bounds() {
                device.clip_to(bounds);
            }
            let rule = match rule {
                mangle_content::FillRule::EvenOdd => FillRule::EvenOdd,
                mangle_content::FillRule::NonZero => FillRule::NonZero,
            };
            if let Some(colour) = fill {
                match colour.to_rgba(None) {
                    Some(rgba) => {
                        device.fill_polygon(&polygon, rule, rgba.to_rgba8(record.fill_alpha));
                    }
                    None => notes.push(format!(
                        "a fill colour in {} could not be converted, so the shape was not drawn",
                        colour.space.name
                    )),
                }
            }
            if let Some(colour) = stroke
                && record.device_line_width > 0.0
            {
                match colour.to_rgba(None) {
                    Some(rgba) => {
                        let style = line_style_of(record.line_cap, record.line_join);
                        let style = StrokeStyle {
                            width: record.device_line_width,
                            dash: record.dash.clone(),
                            ..style
                        };
                        device.stroke_polygon(
                            &polygon,
                            &style,
                            rgba.to_rgba8(record.stroke_alpha),
                        );
                    }
                    None => notes.push(format!(
                        "a stroke colour in {} could not be converted, so the outline was not drawn",
                        colour.space.name
                    )),
                }
            }
            device.reset_clip();
        }
        Mark::ClipChanged(Some(bounds)) => {
            // The interpreter records a clip as a device-space rectangle, which is the
            // part of the clip that decides what is visible for nearly every page. A clip
            // from a path rather than a rectangle is honoured as its bounding box, which
            // draws slightly more than it should rather than slightly less.
            device.clip_to(Rect {
                x0: bounds.x0,
                y0: bounds.y0,
                x1: bounds.x1,
                y1: bounds.y1,
            });
        }
        Mark::ClipChanged(None) => device.reset_clip(),
        Mark::Shading { .. } => {
            notes.push("a shading was found but no shading renderer exists yet".into());
        }
        Mark::Image { name, .. } => {
            let what = name.as_deref().unwrap_or("an inline image");
            notes.push(format!("{what} was found but no image renderer exists yet"));
        }
        Mark::Glyphs { .. } => {
            // Glyph drawing needs font metrics, which the content layer deliberately does
            // not guess at. Saying so is better than drawing boxes.
        }
    }
}

/// A viewport for a page, for a caller that wants the placement without the drawing.
#[must_use]
pub fn viewport_for(page: &Page, canvas: (usize, usize), scale: f64) -> Viewport {
    let crop = page.inherited.crop_rect();
    let placement = Placement::fit(
        &crop,
        canvas,
        effective_scale(scale),
        page.inherited.rotation(),
    );
    Viewport {
        device: Rect {
            x0: 0.0,
            y0: 0.0,
            x1: canvas.0 as f64,
            y1: canvas.1 as f64,
        },
        page: (crop.width(), crop.height()),
        scale: effective_scale(scale),
        placement: Some(placement.matrix),
    }
}

/// A rule conversion, for a caller that holds a content fill rule.
#[must_use]
pub fn fill_rule_of(rule: mangle_content::FillRule) -> FillRule {
    match rule {
        mangle_content::FillRule::EvenOdd => FillRule::EvenOdd,
        mangle_content::FillRule::NonZero => FillRule::NonZero,
    }
}

/// A cap and join, for a caller building a style from the content layer's enums.
#[must_use]
pub fn line_style_of(cap: mangle_content::LineCap, join: mangle_content::LineJoin) -> StrokeStyle {
    StrokeStyle {
        cap: match cap {
            mangle_content::LineCap::Butt => LineCap::Butt,
            mangle_content::LineCap::Round => LineCap::Round,
            mangle_content::LineCap::Square => LineCap::Square,
            // The content layer has a third state for a projecting cap because the
            // specification names it, and a renderer draws it as square.
            mangle_content::LineCap::Projecting => LineCap::Square,
        },
        join: match join {
            mangle_content::LineJoin::Miter => LineJoin::Miter,
            mangle_content::LineJoin::Round => LineJoin::Round,
            mangle_content::LineJoin::Bevel => LineJoin::Bevel,
        },
        ..StrokeStyle::default()
    }
}

/// A polygon for a rectangle, which a page background or a border is.
#[must_use]
pub fn rect_polygon(x0: f64, y0: f64, x1: f64, y1: f64) -> Polygon {
    Polygon {
        subpaths: vec![vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)]],
    }
}

/// The content layer's rule, named so a caller does not have to match on it twice.
#[must_use]
pub fn content_rule(rule: FillRule) -> ContentRule {
    match rule {
        FillRule::EvenOdd => ContentRule::EvenOdd,
        FillRule::NonZero => ContentRule::NonZero,
    }
}

impl Viewport {
    /// A viewport with an explicit matrix, for the case where the caller has already
    /// worked out the placement.
    #[must_use]
    pub fn with_matrix(mut self, matrix: Matrix) -> Self {
        self.placement = Some(matrix);
        self
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

    fn letter() -> PageRect {
        PageRect::new(0.0, 0.0, 612.0, 792.0)
    }

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 0.5
    }

    /// PDF's y axis points up and a canvas's points down. If this is wrong every page
    /// renders upside down, and it looks like a rasterizer bug rather than a sign error.
    #[test]
    fn the_page_origin_lands_at_the_bottom_left() {
        let p = Placement::fit(&letter(), (612, 792), 1.0, 0);
        assert!(
            near(p.matrix.apply(0.0, 0.0).1, 792.0),
            "the origin is at the bottom"
        );
        assert!(near(p.matrix.apply(0.0, 0.0).0, 0.0), "and at the left");
    }

    #[test]
    fn the_far_corner_lands_at_the_top_right() {
        let p = Placement::fit(&letter(), (612, 792), 1.0, 0);
        let (x, y) = p.matrix.apply(612.0, 792.0);
        assert!(near(x, 612.0) && near(y, 0.0), "got ({x}, {y})");
    }

    #[test]
    fn a_page_that_does_not_start_at_the_origin_is_moved_into_place() {
        // A crop box at (100, 200): its lower left, not the media box's, is page (0, 0).
        let crop = PageRect::new(100.0, 200.0, 400.0, 500.0);
        let p = Placement::fit(&crop, (300, 300), 1.0, 0);
        let (x, y) = p.matrix.apply(100.0, 200.0);
        assert!(
            near(x, 0.0) && near(y, 300.0),
            "the crop box's corner is at (0, 0)"
        );
        assert_eq!(p.size, (300, 300), "the canvas size is carried through");
    }

    /// A letter page in a square canvas is letterboxed, not stretched.
    #[test]
    fn a_page_is_fitted_not_stretched() {
        let p = Placement::fit(&letter(), (400, 400), 1.0, 0);
        let (x0, y0) = p.matrix.apply(0.0, 0.0);
        let (x1, y1) = p.matrix.apply(612.0, 792.0);
        let width = (x1 - x0).abs();
        let height = (y1 - y0).abs();
        assert!(
            near(width / height, 612.0 / 792.0),
            "the aspect ratio is kept"
        );
        assert!(
            width <= 400.0 && height <= 400.0,
            "and it fits: {width} by {height}"
        );
    }

    #[test]
    fn a_rotation_turns_the_page_and_swaps_its_dimensions() {
        // A quarter turn: the page's top edge becomes the canvas's right edge.
        let p = Placement::fit(&letter(), (792, 612), 1.0, 90);
        let (x, y) = p.matrix.apply(0.0, 0.0);
        assert!(
            near(x, 0.0) && near(y, 0.0),
            "the corner that was lower left"
        );
        let (x, y) = p.matrix.apply(612.0, 792.0);
        assert!(
            near(x, 792.0) && near(y, 612.0),
            "becomes the bottom right: ({x}, {y})"
        );
    }

    #[test]
    fn a_degenerate_canvas_leaves_the_page_where_it_is() {
        let p = Placement::fit(&letter(), (0, 0), 1.0, 0);
        assert_eq!(p.size, (0, 0), "and asks for nothing");
        // A page with no area does not divide by zero.
        let empty = Placement::fit(&PageRect::new(0.0, 0.0, 0.0, 0.0), (100, 100), 1.0, 0);
        assert_eq!(empty.matrix, Matrix::IDENTITY);
    }

    #[test]
    fn an_unknown_rotation_is_treated_as_none() {
        // `/Rotate` is a right angle; a file that says 45 has said something wrong, and
        // turning the page by an eighth of a turn would be a worse answer than not
        // turning it.
        let p = Placement::fit(&letter(), (612, 792), 1.0, 45);
        let upright = Placement::fit(&letter(), (612, 792), 1.0, 0);
        assert_eq!(p.matrix.apply(1.0, 1.0), upright.matrix.apply(1.0, 1.0));
    }

    #[test]
    fn a_scale_beyond_the_bound_is_honoured_only_up_to_it() {
        assert_eq!(effective_scale(1.0), 1.0);
        assert_eq!(effective_scale(4.0), 4.0);
        assert_eq!(effective_scale(1000.0), MAX_SCALE, "a huge zoom is capped");
    }

    #[test]
    fn a_nonsense_scale_falls_back_to_one() {
        // A zero scale would divide by zero; a negative one would mirror the page.
        assert_eq!(effective_scale(0.0), 1.0);
        assert_eq!(effective_scale(-4.0), 1.0);
        assert_eq!(effective_scale(f64::NAN), 1.0);
        assert_eq!(effective_scale(f64::INFINITY), 1.0);
    }

    #[test]
    fn a_rectangle_polygon_is_a_closed_box() {
        let p = rect_polygon(1.0, 2.0, 5.0, 6.0);
        assert_eq!(p.subpaths.len(), 1);
        let edges = p.edges();
        assert_eq!(edges.len(), 4, "four sides");
        assert!((p.signed_area() - 16.0).abs() < 1e-9, "four by four");
    }

    #[test]
    fn fill_rules_map_across_unchanged() {
        assert_eq!(fill_rule_of(ContentRule::EvenOdd), FillRule::EvenOdd);
        assert_eq!(fill_rule_of(ContentRule::NonZero), FillRule::NonZero);
        assert_eq!(content_rule(FillRule::EvenOdd), ContentRule::EvenOdd);
        assert_eq!(content_rule(FillRule::NonZero), ContentRule::NonZero);
    }

    #[test]
    fn the_content_layers_three_cap_states_map_onto_two_shapes() {
        // The specification names a projecting cap and a square cap; a renderer draws
        // both as a square, but they are different states and both must be understood.
        let projecting = line_style_of(
            mangle_content::LineCap::Projecting,
            mangle_content::LineJoin::Miter,
        );
        let square = line_style_of(
            mangle_content::LineCap::Square,
            mangle_content::LineJoin::Miter,
        );
        assert_eq!(projecting.cap, LineCap::Square);
        assert_eq!(square.cap, LineCap::Square);

        let round = line_style_of(
            mangle_content::LineCap::Round,
            mangle_content::LineJoin::Bevel,
        );
        assert_eq!(round.cap, LineCap::Round);
        assert_eq!(round.join, LineJoin::Bevel);
    }

    #[test]
    fn a_blank_render_is_reported_as_blank() {
        let render = PageRender {
            image: Image::filled(4, 4, [255, 255, 255, 255]),
            scale: 1.0,
            marks: 0,
            notes: Vec::new(),
        };
        assert!(render.is_blank());

        let drawn = PageRender {
            image: Image::filled(4, 4, [255, 255, 255, 255]),
            scale: 1.0,
            marks: 1,
            notes: Vec::new(),
        };
        let mut inked = drawn.clone();
        inked.image.put(0, 0, [0, 0, 0, 255]);
        assert!(!inked.is_blank(), "one black pixel is not blank");
    }
}
