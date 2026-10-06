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

use mangle_content::{
    ContentStream, FillRule as ContentRule, Mark, Matrix, PathSegment, Resources, Rgba, run_with,
};
use mangle_doc::Page;
use mangle_syntax::{Document, Object, Rect as PageRect, object::Dict, stream::decode_stream};

use crate::BlendMode;
use crate::fill::{self, FillColour};
use crate::image::{self, Raster};
use crate::shading::{self, Shading};
use crate::{
    ClipRegion, ClipShape, Device, FillRule, Image, LineCap, LineJoin, Polygon, Rect, StrokeStyle,
    Subpath, Viewport, transform_path,
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

/// How close to a whole number of pixels a page size must be before it counts as whole.
///
/// This is relative rather than absolute because the error it exists to absorb is relative:
/// `792.0 * (150.0 / 72.0)` is not 1650 in binary floating point, it is `1650.0000000000002`,
/// one unit in the last place above 1650 and a relative error of 1.4e-16. The identical
/// mistake on a 100-point page is `208.33333333333334`, which is 1.3e-15 from 208.33 and
/// nowhere near the next integer — so an absolute epsilon wide enough to catch the Letter
/// page would also swallow a quarter of a pixel on a small page, and one narrow enough to
/// spare the small page would miss the Letter page entirely. An **absolute** epsilon therefore
/// cannot separate "one ulp above a whole pixel" from "a quarter of a pixel"; only a relative
/// one can, and this is that.
///
/// The value is **1e-6 relative, measured against `mutool`** rather than chosen. `mutool` snaps
/// a page size that is within about 1e-6 of a whole pixel and ceils it otherwise: feeding it
/// pages whose 150-DPI height is 1530.001, 1530.005 and 1530.05 pixels gives heights 1530, 1531
/// and 1531, so its threshold sits between the first two — 1.3e-6 and 3.3e-6 relative.
///
/// That threshold is not an arbitrary tolerance on `mutool`'s part; it is what a reader has to
/// use anyway. A producer writes a `/CropBox` as decimal, and `pdfjs__freeculture.pdf` writes
/// one as `[86.399997 248.40001 525.6 680.4]` — eight significant decimals, so the true height
/// is 734.4 and the file says 734.400024. Carried through to 150 DPI that is 1530.00005, which
/// a 1e-9 tolerance cannot see and a `ceil` then rounds **up**, making this renderer one pixel
/// taller than the oracle. A buffer one pixel off is not a slightly worse picture, it is a
/// *size disagreement*, and the comparison is refused rather than scored — so a tolerance that
/// is too tight does not degrade a measurement, it deletes it.
const WHOLE_PIXEL_EPSILON: f64 = 1e-6;

/// Draw every annotation on the page, in the order the page lists them.
///
/// An annotation's ink is an **appearance stream**: a form XObject whose own `/BBox` is mapped
/// onto the annotation's `/Rect`. That is the whole of the geometry, and ISO 32000-1 §12.5.5
/// gives the composition in a fixed order — the box onto the rectangle, then the form's
/// `/Matrix`, then `/Transform` — which is applied here in that order because every other order
/// puts the annotation somewhere plausible and wrong.
///
/// The appearance is drawn by `paint_records`, the same marks-to-pixels loop the page's own
/// content goes through, with the annotation's own `/Resources`. A file whose annotations name
/// their own fonts therefore gets those fonts, which is the same rule forms follow and for the
/// same reason.
///
/// What is **not** synthesised is an appearance the annotation does not carry. A `/Highlight`
/// with no `/AP` is a coloured rectangle by convention, and inventing one would put colour on the
/// page that the file never asked for; it is reported by name instead.
fn paint_annotations(
    doc: &Document,
    page: &Page,
    placement: &Placement,
    device: &mut Device,
    notes: &mut Vec<String>,
) {
    for (index, annot) in page.annots(doc).iter().enumerate() {
        let Some(object) = doc.resolve_object(annot) else {
            notes.push(format!(
                "annotation {} of the page is an indirect reference that does not resolve, so \
                 it was not drawn",
                index + 1
            ));
            continue;
        };
        let Some(dict) = object.as_dict() else {
            notes.push(format!(
                "annotation {} of the page is a {} rather than a dictionary, so it was not drawn",
                index + 1,
                object.type_name()
            ));
            continue;
        };
        // `/F` bit 2 is Hidden and bit 6 is NoView. Both mean the annotation is not shown in a
        // printed or displayed page, and drawing it would put ink the file says is not there.
        let flags = dict.get("F").and_then(Object::as_i64).unwrap_or(0);
        if flags & (ANNOT_HIDDEN | ANNOT_NO_VIEW) != 0 {
            continue;
        }
        let subtype = dict
            .get("Subtype")
            .and_then(Object::as_name)
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .unwrap_or_else(|| "unknown".to_owned());
        // `/AP` is a dictionary holding `/N` (normal) and `/R`; only `/N` is the page's ink,
        // because `/R` is what a rubber stamp shows while it is being dragged. `/N` is then
        // either one appearance stream or a dictionary of them keyed by appearance state.
        let ap = dict.get("AP").and_then(|ap| doc.resolve_object(ap));
        let normal = ap
            .as_ref()
            .and_then(|o| o.as_dict())
            .and_then(|d| d.get("N"))
            .and_then(|n| doc.resolve_object(n));
        let chosen = match normal {
            Some(Object::Dict(states)) => {
                // `Dict::get` takes a `str`, so the state's name is read as a string rather
                // than as a `Name`. A state that is not in the dictionary is named rather than
                // defaulted: guessing which of several looks right is how the wrong one gets
                // drawn silently.
                let wanted = dict
                    .get("AS")
                    .and_then(Object::as_name)
                    .and_then(|n| std::str::from_utf8(n).ok().map(str::to_owned));
                let found = wanted.as_deref().and_then(|w| states.get(w)).cloned();
                match found.and_then(|o| doc.resolve_object(&o)) {
                    Some(p) => Some(p),
                    None => {
                        notes.push(format!(
                            "the /{subtype} annotation {} names the appearance state `{}`, \
                             which its /AP does not define, so it was not drawn",
                            index + 1,
                            wanted.as_deref().unwrap_or("(none)")
                        ));
                        continue;
                    }
                }
            }
            Some(other) => Some(other),
            None => {
                // A `/Link` and a `/Popup` have no appearance of their own, so nothing is
                // missing and there is nothing to report — a note per link on every page would
                // be noise that teaches a reader to skip notes.
                if ap.is_none() && !NO_APPEARANCE_BY_DESIGN.contains(&subtype.as_str()) {
                    notes.push(format!(
                        "the /{subtype} annotation {} carries no /AP appearance, and one was \
                         not invented for it",
                        index + 1
                    ));
                } else if ap.is_some() {
                    notes.push(format!(
                        "the /{subtype} annotation {} has an /AP with no normal appearance, so \
                         it was not drawn",
                        index + 1
                    ));
                }
                continue;
            }
        };
        let Some(appearance) = chosen else {
            continue;
        };
        let Object::Stream(form) = appearance else {
            notes.push(format!(
                "the /{subtype} annotation {} has a normal appearance that is not a stream, so \
                 it was not drawn",
                index + 1
            ));
            continue;
        };
        let Some(rect) = dict.get("Rect").and_then(|r| {
            PageRect::from_object(&doc.resolve_object(r).unwrap_or_else(|| r.clone())).ok()
        }) else {
            notes.push(format!(
                "the /{subtype} annotation {} has no usable /Rect, so it has nowhere to be \
                 drawn",
                index + 1
            ));
            continue;
        };
        if rect.right <= rect.left || rect.top <= rect.bottom {
            notes.push(format!(
                "the /{subtype} annotation {} has a /Rect with no area, so it was not drawn",
                index + 1
            ));
            continue;
        }
        // The appearance's own box, mapped onto the rectangle. A stream with no `/BBox` is
        // taken to fill its rectangle, which is what a viewer does with one.
        let bbox = match form.dict.get("BBox") {
            Some(b) => {
                let resolved = doc.resolve_object(b).unwrap_or_else(|| b.clone());
                match PageRect::from_object(&resolved) {
                    Ok(b) => b,
                    Err(_) => {
                        notes.push(format!(
                            "the /{subtype} annotation {} has a /BBox that is not four numbers, \
                             so it was not drawn",
                            index + 1
                        ));
                        continue;
                    }
                }
            }
            None => rect,
        };
        if bbox.right <= bbox.left || bbox.top <= bbox.bottom {
            notes.push(format!(
                "the /{subtype} annotation {} has a /BBox with no area, so it was not drawn",
                index + 1
            ));
            continue;
        }
        // §12.5.5: the box onto the rectangle, then the form's `/Matrix`, then `/Transform`.
        // A `/Transform` is in the *rectangle's* space, so it goes outside the box mapping.
        let sx = (rect.right - rect.left) / (bbox.right - bbox.left);
        let sy = (rect.top - rect.bottom) / (bbox.top - bbox.bottom);
        let mut to_rect = Matrix::new(
            sx,
            0.0,
            0.0,
            sy,
            rect.left - bbox.left * sx,
            rect.bottom - bbox.bottom * sy,
        );
        if let Some(m) = form.dict.get("Matrix") {
            to_rect = to_rect.concat(read_matrix(Some(
                &doc.resolve_object(m).unwrap_or_else(|| m.clone()),
            )));
        }
        if let Some(t) = form.dict.get("Transform") {
            to_rect = to_rect.concat(read_matrix(Some(
                &doc.resolve_object(t).unwrap_or_else(|| t.clone()),
            )));
        }

        let decoded = decode_stream(&form);
        if decoded.encoded {
            notes.push(format!(
                "the /{subtype} annotation {} is still encoded after its filters, so it was not \
                 drawn rather than read as operators",
                index + 1
            ));
            continue;
        }
        if decoded.data.is_empty() {
            continue;
        }
        for note in &decoded.notes {
            notes.push(format!("the /{subtype} annotation {}: {note}", index + 1));
        }
        // An annotation names its own fonts and images.
        let own = match form.dict.get("Resources") {
            Some(r) => {
                let resolved = doc.resolve_object(r).unwrap_or_else(|| r.clone());
                resolved
                    .as_dict()
                    .map(|d| Resources::from_dict(d, &|o| doc.resolve_object(o)))
                    .unwrap_or_default()
            }
            None => Resources::default(),
        };
        let executed = run_with(&ContentStream::parse(&decoded.data), &own);
        for note in &executed.notes {
            notes.push(format!("the /{subtype} annotation {}: {note}", index + 1));
        }
        let annot_placement = Placement {
            matrix: placement.matrix.concat(to_rect),
            size: placement.size,
        };
        paint_records(&executed, device, &annot_placement, &own, doc, notes);
    }
}

/// Annotation kinds the specification gives **no** appearance of its own, so a missing `/AP` on
/// one of them is the file saying what it meant rather than a gap in the page.
const NO_APPEARANCE_BY_DESIGN: &[&str] = &["Link", "Popup"];

/// `/F` bit 2: the annotation is hidden entirely.
const ANNOT_HIDDEN: i64 = 1 << 1;
/// `/F` bit 6: the annotation is not shown when the page is displayed or printed.
const ANNOT_NO_VIEW: i64 = 1 << 5;

/// One side of a page buffer, in pixels.
///
/// **Rounded up**, because rounding down clips the page: 100 points at 150 DPI is 208.33
/// pixels and a buffer of 208 loses the last third of a pixel off the bottom of the page.
///
/// **Except when it is already a whole number of pixels**, which is not the same thing as
/// when floating point says it is. `792.0 * (150.0 / 72.0)` is `1650.0000000000002`, and
/// `ceil` of that is 1651 — so a US-Letter page, the most common page size there is, came
/// out one row too tall, and a 960-point side came out one column too wide. That is not a
/// cosmetic difference: a buffer one pixel larger than the oracle's is a *size
/// disagreement*, not a worse picture, so every comparison of such a page is refused
/// rather than scored, and 80 pages of the wild corpus produced no measurement at all.
///
/// So a value within `WHOLE_PIXEL_EPSILON` of an integer is taken to *be* that integer
/// before the ceiling is applied. The ceiling itself is right and stays: it is what makes
/// 208.33 into 209, and it is what `mutool` does for everything past its own snapping
/// threshold — measured, at 1530.005 pixels and above, where `mutool` returns 1531 and this
/// returns 1531 too.
fn pixels_for(pixels: f64, shrink: f64) -> usize {
    let exact = pixels * shrink;
    let whole = exact.round();
    let settled = if (exact - whole).abs() <= WHOLE_PIXEL_EPSILON * exact.abs().max(1.0) {
        whole
    } else {
        exact
    };
    settled.ceil().max(1.0) as usize
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
    // Rounded *up*, not to nearest, so a page of 208.33 pixels has the 209 rows it needs to
    // hold its last third of a pixel. `pixels_for` is where that happens, and where the one
    // exception to it lives.
    let size = (pixels_for(pixels_w, shrink), pixels_for(pixels_h, shrink));

    // The canvas is already `points × scale` pixels wide, so the page is fitted into it
    // with a scale of one. Passing `scale` here as well would apply it twice and draw the
    // page at the *square* of the zoom, hanging off the sides of its own canvas by a
    // factor of `scale`. That is invisible on a fixture whose content is symmetric about
    // the centre of the page — a quadrant in each corner lands in a quadrant either way —
    // and obvious on anything that is not: a glyph near the left margin ends up off the
    // edge of the page and is not drawn at all.
    let placement = Placement::fit(&crop, size, 1.0, rotate);
    let mut render = PageRender {
        image: Image::filled(size.0, size.1, options.paper),
        scale,
        marks: 0,
        notes: Vec::new(),
    };

    // The page's content, decoded. The raw bytes are never handed to the interpreter: a
    // compressed stream read as if it were operators is a page of nonsense, and a page of
    // nonsense is a blank page with a plausible-looking reason attached to it. Whatever
    // the filters had to say goes into the notes before anything is parsed, so a page
    // that lost its content says so instead of reporting itself as empty.
    let decoded = page.decoded_contents_full(doc);
    render.notes.extend(decoded.notes.iter().cloned());
    let content = decoded.data;

    let mut device = Device::new(render.image);
    // An empty `/Contents` is **not** an early return. A page whose only ink is its annotations
    // has no content stream at all, which is legal and is how a form made only of stamps is
    // written; returning there would drop every annotation on it and say nothing.
    if !content.is_empty() {
        let stream = ContentStream::parse(&content);
        let executed = run_with(&stream, resources);
        render.notes.extend(executed.notes.iter().cloned());
        render.marks += paint_records(
            &executed,
            &mut device,
            &placement,
            resources,
            doc,
            &mut render.notes,
        );
    }
    // Annotations are drawn after the page's own content, in the order the page lists them,
    // and into the same device — so an annotation's ink lands on top of the text it annotates,
    // which is what a highlight is for.
    paint_annotations(doc, page, &placement, &mut device, &mut render.notes);
    render.image = device.into_image();
    render
}

/// Draw a run of executed marks into a device, and report how many were drawn.
///
/// Split out of `render_page` because a tiling pattern needs the same thing for a cell that is
/// not the page: the cell is a content stream of its own, executed and drawn at a transform of
/// its own, and the only difference between it and the page is which bytes were parsed and which
/// transform they were drawn under. Keeping one loop means a mark inside a cell is drawn by the
/// same code as a mark on the page, rather than by a second implementation that agrees with it
/// today and drifts from it tomorrow.
fn paint_records(
    executed: &mangle_content::PageContent,
    device: &mut Device,
    placement: &Placement,
    resources: &Resources,
    doc: &Document,
    notes: &mut Vec<String>,
) -> usize {
    let mut drawn = 0usize;
    for (name, span) in &executed.unknown_operators {
        notes.push(format!(
            "operator `{}` at byte {} is not in the table and was not executed",
            String::from_utf8_lossy(name),
            span.start
        ));
    }
    // The image lookup borrows the page's resources and the document and nothing else. It
    // must not capture the notes: they are passed into `draw_mark` on the next line, and a
    // second live borrow of them would not compile. The failure reason comes back as the
    // closure's `Err` and is appended by `draw_mark` itself.
    //
    // All three lookups take the resource table to look in rather than capturing the
    // page's, because a mark drawn inside a form resolves its names against **that form's**
    // `/Resources`. Two forms may each name `/F1` and mean different fonts, so a lookup
    // that fell back to the page's table would draw the wrong glyphs with nothing in the
    // report to say so.
    let mut lookup = |name: &str, in_resources: &Resources| image_for(name, in_resources, doc);
    let mut shade_lookup =
        |name: &str, in_resources: &Resources| shading_for(name, in_resources, doc);
    // The font lookup is beside the other two rather than inside `draw_mark`, for the same
    // reason: the document and the page's resources are borrowed here, once, and the
    // drawing code only ever sees a name.
    let mut font_lookup = |name: &str, in_resources: &Resources| font_for(name, in_resources, doc);
    // The clip the device is holding, so a run of marks under one clip costs one install
    // rather than one per mark. The memo is keyed on the record's own clip and installs the
    // whole region when it differs, which keeps the cost proportional to how often the page
    // changed its clip without making the result depend on anything but the record: installing
    // the same region twice gives the same clip.
    //
    // `None` here is also the device's starting state — a fresh device has the whole image as
    // its clip — so a page whose first mark has no clip needs no first install.
    let mut installed: Option<mangle_content::Clip> = None;
    for record in &executed.records {
        // Before the mark, from that mark's own record. This is what makes drawing a mark
        // independent of what was drawn before it, and it is why `Mark::ClipChanged` has
        // nothing to do here: the record carrying the *next* mark already holds the new clip.
        if installed.as_ref() != record.clip.as_ref() {
            match &record.clip {
                Some(clip) => device.install_clip(Some(&clip_region(clip, &placement.matrix))),
                None => device.install_clip(None),
            }
            installed.clone_from(&record.clip);
        }
        let to_device = placement.matrix.concat(record.ctm);
        // The table this mark's names resolve against: the form's own where it was drawn
        // inside one, and the page's otherwise. One line, and the reason a font named only
        // inside a form is found at all.
        let own = record
            .form
            .as_ref()
            .and_then(|name| resources.form_resources(name));
        let named_in = own.as_deref().unwrap_or(resources);
        draw_mark(
            device,
            &record.mark,
            &to_device,
            &placement.matrix,
            record,
            notes,
            &mut lookup,
            &mut shade_lookup,
            &mut font_lookup,
            named_in,
            doc,
        );
        drawn += 1;
    }
    drawn
}

/// Find and read the shading a `sh` names.
///
/// A page names a *pattern*, and the pattern names a shading inside it. A pattern whose
/// `/Shading` is a shading dictionary in its own right is unusual but legal, so both shapes
/// are accepted here rather than being a second code path for a caller to trip over.
fn shading_for(
    name: &str,
    resources: &Resources,
    doc: &Document,
) -> Result<(Shading, Matrix), String> {
    let source = pattern_source(name, resources, doc)?;
    let resolver = |o: &Object| doc.resolve_object(o);
    let shading = Shading::parse(&source.shading, &resolver)
        .ok_or_else(|| format!("the shading `/{name}` is of a kind this does not paint yet"))?;
    Ok((shading, source.matrix))
}

/// Which kind of pattern a page named.
///
/// `/PatternType` is optional and its default is **1**, which is the part that matters: a
/// pattern that omits the key is a tiling pattern, and reading it as a shading would look
/// for a `/Shading` entry that is not there and report the wrong thing — "a kind this does
/// not paint yet" instead of "a tiling pattern", which is a feature rather than a fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PatternKind {
    /// `/PatternType 1`: a content stream tiled across the fill.
    Tiling,
    /// `/PatternType 2`: a shading, clipped to what is being filled.
    Shading,
    /// A `/PatternType` this does not know. Carried rather than refused, so the note can
    /// quote the number the file actually used.
    Other(i64),
}

impl PatternKind {
    /// Read `/PatternType`, defaulting to 1 as the specification requires.
    fn read(dict: &Dict) -> Self {
        match dict.get("PatternType").and_then(Object::as_i64) {
            Some(1) | None => Self::Tiling,
            Some(2) => Self::Shading,
            Some(other) => Self::Other(other),
        }
    }
}

/// Whether a tiling pattern's cell carries its own colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "nothing reads this until the cell itself is drawn; only the tests do"
)]
enum PaintType {
    /// `/PaintType 1`: the cell paints the colours it names. The default.
    Coloured,
    /// `/PaintType 2`: the cell is a stencil and takes the colour of the operator that set the
    /// pattern, which is what lets one cell serve as any colour.
    Uncoloured,
}

impl PaintType {
    /// Read `/PaintType`, whose default is 1.
    ///
    /// Anything that is not 2 is read as 1. A `/PaintType` this does not recognise is not a
    /// reason to refuse the pattern, and both readings hand the cell's colour to somebody
    /// else anyway.
    fn read(object: Option<&Object>) -> Self {
        match object.and_then(Object::as_i64) {
            Some(2) => Self::Uncoloured,
            _ => Self::Coloured,
        }
    }
}

/// A `/PatternType 1` tiling pattern, read: one cell of it, and how the cells are laid out.
///
/// The cell is the pattern's own content stream clipped to its `/BBox`, repeated once per
/// `/XStep` by `/YStep` cell in pattern space, so these numbers are everything the cell needs
/// to know where it goes. Nothing here decides how to run the cell — that is the tiling loop,
/// which walks the cells this describes.
#[derive(Debug)]
#[allow(
    dead_code,
    reason = "the tiling loop reads this; until it lands, only the tests do"
)]
struct TilingPattern {
    /// The cell's clip, in pattern space. Required, and the one entry with no default that
    /// means anything: an unclipped cell is not the file's cell.
    bbox: PageRect,
    /// How far the cells step along x. **Zero is a value and not an absence**: it means the
    /// cell is not repeated along x, which is drawn once at the pattern-space origin. A
    /// negative step is legal — the cells then run the other way — so it is kept as written.
    x_step: f64,
    /// The same along y, and independent of `x_step`: a pattern may repeat along one axis and
    /// not the other, and reading the two as one "pitch" loses half of those patterns.
    y_step: f64,
    /// Pattern space to the space the pattern was set in. Absent is the identity, which is
    /// what a pattern drawn where it is laid usually means.
    matrix: Matrix,
    /// Whether the cell carries its colour or takes the paint operator's.
    paint_type: PaintType,
    /// The cell's content stream, still filtered, carrying the cell's own `/Resources` in its
    /// dictionary — a pattern's cell names its own fonts and images rather than the page's.
    content: Object,
}

/// Read a tiling pattern: its cell, its pitch, its own space and its paint type.
///
/// Only `/BBox` is refused rather than defaulted, because the other entries all have
/// defaults that mean something and this one does not: a cell clipped to a rectangle the file
/// never wrote is a plausible texture that belongs to nobody. A missing or unreadable
/// `/BBox`, and a `/XStep` or `/YStep` that is present but is not a number, each come back as
/// an error naming the pattern and the entry at fault.
///
/// The default that is easiest to get wrong is the pitch. `/XStep` and `/YStep` both default
/// to zero, and **zero means not tiled in that direction** — the cell is drawn once, at the
/// pattern-space origin — so it must not be folded together with "the entry is not there".
#[allow(
    dead_code,
    reason = "the tiling loop calls this; until it lands, only the tests do"
)]
fn tiling_pattern(
    name: &str,
    object: &Object,
    resolve: &dyn Fn(&Object) -> Option<Object>,
) -> Result<TilingPattern, String> {
    let raw = object.clone();
    let resolved = resolve(&raw).unwrap_or(raw);
    // A tiling pattern's cell *is* its content, so a dictionary with no stream behind it has
    // nothing to tile however well formed the rest of it is.
    let Object::Stream(stream) = &resolved else {
        return Err(format!(
            "the tiling pattern `/{name}` is not a stream, so it has no cell to draw"
        ));
    };
    // Entries are normally direct, but a file may make any of them indirect, and one that
    // resolves to nothing is then left as the entry the file wrote.
    let dict = &stream.dict;
    let entry = |key: &str| -> Option<Object> {
        dict.get(key)
            .map(|found| resolve(found).unwrap_or_else(|| found.clone()))
    };
    let Some(bbox) = entry("BBox") else {
        return Err(format!(
            "the tiling pattern `/{name}` has no /BBox, so its cell has no clip and was not drawn"
        ));
    };
    Ok(TilingPattern {
        bbox: read_bbox(name, &bbox)?,
        x_step: read_step(name, "XStep", entry("XStep"))?,
        y_step: read_step(name, "YStep", entry("YStep"))?,
        matrix: read_matrix(entry("Matrix").as_ref()),
        paint_type: PaintType::read(entry("PaintType").as_ref()),
        content: resolved.clone(),
    })
}

/// The four numbers of a `/BBox`, or the error that says they are not there.
///
/// `Rect::from_object` would fill a missing number with zero, which for a `/BBox` silently
/// clips the cell to something the file never said, so the four are read one at a time.
fn read_bbox(name: &str, object: &Object) -> Result<PageRect, String> {
    let unreadable = || {
        format!(
            "the /BBox of the tiling pattern `/{name}` is not four numbers, so its cell has no \
             clip and was not drawn"
        )
    };
    let Some(values) = object.as_array() else {
        return Err(unreadable());
    };
    let mut numbers = [0.0f64; 4];
    for (index, slot) in numbers.iter_mut().enumerate() {
        *slot = values
            .get(index)
            .and_then(Object::as_f64)
            .ok_or_else(unreadable)?;
    }
    let [x0, y0, x1, y1] = numbers;
    Ok(PageRect::new(x0, y0, x1, y1))
}

/// One of `/XStep` or `/YStep`: how far the cells step in one direction.
///
/// Absent takes the specified default of zero, and a zero is carried through as itself —
/// **the cell is not repeated in this direction**, which is drawn once rather than not at all.
/// Filtering it out as though it meant nothing is what makes a one-cell pattern disappear.
/// Negative is legal and kept: the cells run the other way, which is a picture, not a fault.
fn read_step(name: &str, field: &str, object: Option<Object>) -> Result<f64, String> {
    let Some(object) = object else {
        return Ok(0.0);
    };
    object.as_f64().ok_or_else(|| {
        format!(
            "the /{field} of the tiling pattern `/{name}` is not a number, so the cells could \
             not be placed"
        )
    })
}

/// A pattern resource, resolved: what kind it is, what is inside it, and how to reach it.
struct PatternSource {
    kind: PatternKind,
    /// The shading inside it, or the pattern itself where a shading pattern carries its own
    /// shading dictionary.
    shading: Object,
    /// The pattern's own transformation, which maps its space into the page's default space.
    matrix: Matrix,
}

/// Resolve a named pattern to its kind, its contents and its own transformation.
///
/// Shared by `sh` and by a pattern used as a fill colour, because both read the same
/// resource and both need the pattern's `/Matrix`: the difference between them is what they
/// do with the shading afterwards, not where it comes from.
fn pattern_source(
    name: &str,
    resources: &Resources,
    doc: &Document,
) -> Result<PatternSource, String> {
    let Some(object) = resources.patterns.get(name).cloned() else {
        return Err(format!(
            "the page names a shading `/{name}` that its resources do not define"
        ));
    };
    let resolved = doc.resolve_object(&object).unwrap_or(object);
    // The pattern's own transformation maps its space into the page's default space, which
    // the mark's transformation then carries to the pixels. Both are needed and neither
    // substitutes for the other.
    let (kind, shading, matrix) = match &resolved {
        Object::Stream(s) => (
            PatternKind::read(&s.dict),
            match s.dict.get("Shading") {
                Some(inner) => doc.resolve_object(inner).unwrap_or_else(|| inner.clone()),
                // No `/Shading`: the pattern stream is itself one, which is how a
                // single-use gradient is usually written.
                None => resolved.clone(),
            },
            read_matrix(s.dict.get("Matrix")),
        ),
        Object::Dict(d) => match d.get("Shading") {
            Some(inner) => (
                PatternKind::read(d),
                doc.resolve_object(inner).unwrap_or_else(|| inner.clone()),
                read_matrix(d.get("Matrix")),
            ),
            None => (PatternKind::read(d), resolved.clone(), Matrix::IDENTITY),
        },
        _ => {
            return Err(format!(
                "`/{name}` is named by `sh` but is neither a dictionary nor a stream"
            ));
        }
    };
    Ok(PatternSource {
        kind,
        shading,
        matrix,
    })
}

/// The colour space a shading's function produces that this cannot convert, if it names one.
///
/// Gray, RGB and CMYK are converted by counting what the function produces, which is why a
/// shading with no readable `/ColorSpace` still paints. `Indexed` needs its palette and
/// `Separation` and `DeviceN` need a tint transform, and all three are one component wide —
/// exactly what a grey function produces. Reading one as grey would paint a spot colour as a
/// picture of a grey, so the space is reported instead.
fn unconvertible_space(shading: &Object, doc: &Document) -> Option<String> {
    let dict = match shading {
        Object::Dict(d) => d,
        Object::Stream(s) => &s.dict,
        _ => return None,
    };
    let object = dict.get("ColorSpace")?;
    let resolved = doc.resolve_object(object).unwrap_or_else(|| object.clone());
    // A space is named either by a name (`/DeviceRGB`) or by an array whose first entry is
    // one (`[/Separation /PANTONE 123 /TintTransform …]`), and the array form is the one
    // that carries the transform this cannot use.
    let name = match resolved.as_name() {
        Some(name) => String::from_utf8_lossy(name).into_owned(),
        None => {
            let first = resolved.as_array()?.first()?;
            let first = doc.resolve_object(first).unwrap_or_else(|| first.clone());
            String::from_utf8_lossy(first.as_name()?).into_owned()
        }
    };
    matches!(name.as_str(), "Separation" | "DeviceN" | "Indexed").then_some(name)
}

/// The colour a fill in a pattern paints in, or why there is none.
///
/// A `/Pattern` colour space means `scn`'s operands named a pattern rather than a colour
/// value, so there is nothing to convert: the pattern decides, and its colour varies with
/// position.
///
/// `placement` is the transformation from the page's *default* user space to the device,
/// which is what a pattern's own `/Matrix` maps into — **not** the mark's own `to_device`.
/// That difference is the whole of the rule here and it is easy to get backwards: a pattern
/// says where it lives in the page's space, so a `cm` that moves or scales the *shape* leaves
/// the pattern where it was, and a page that means to put a gradient into a scaled square
/// says so with the pattern's `/Matrix`. `sh` is the opposite case — a shading painted by
/// that operator is in the CTM in force at the operator — which is why the same pattern
/// resource reaches the device by two different routes on one page.
///
/// A `/PatternType 1` pattern is a content stream tiled across the fill, which needs a loop
/// over cells in the pattern's own space.
///
/// **The loop is implemented** — the cell is rendered once and reused across the fill, and both
/// `/PaintType`s are honoured ([D25](docs/known-diffs.md)). This comment previously said the
/// opposite, and said it *in the paragraph immediately above the call that does the work*, which
/// is the worst place for a false claim to sit: a reader would either believe tiling is missing or
/// "fix" working code to match. The remaining gap is a pattern type this does not paint, and that
/// one is refused by name — one cell of a tiling is a texture that looks plausible and is wrong
/// everywhere.
fn pattern_fill(
    paint: Paint,
    name: &str,
    resources: &Resources,
    doc: &Document,
    placement: &Matrix,
    _uncoloured: Option<Rgba>,
) -> Result<FillColour, String> {
    let op = paint.name();
    let source = pattern_source(name, resources, doc)?;
    match source.kind {
        PatternKind::Shading => {}
        PatternKind::Tiling => {
            return tiling_fill(op, name, resources, doc, placement, _uncoloured);
        }
        PatternKind::Other(kind) => {
            return Err(format!(
                "a {op} in the PatternType {kind} pattern `/{name}` was found and not drawn: \
                 only PatternType 1 and 2 patterns can be a {op} colour"
            ));
        }
    }
    if let Some(space) = unconvertible_space(&source.shading, doc) {
        return Err(format!(
            "a {op} in the pattern `/{name}` is in the {space} colour space, which this \
             cannot convert without a tint transform, so the shape was not drawn"
        ));
    }
    let resolver = |o: &Object| doc.resolve_object(o);
    let shading = Shading::parse(&source.shading, &resolver).ok_or_else(|| {
        format!("the shading in the {op} pattern `/{name}` is of a kind this does not paint yet")
    })?;
    Ok(FillColour::Shading {
        shading,
        // The pattern's own matrix sits inside the page's placement, so the two compose in
        // that order and the gradient is sampled in its own space throughout.
        to_shading: placement.concat(source.matrix),
    })
}

/// Render one cell of a tiling pattern and hand it back as a fill colour.
///
/// The cell is rendered **once**, into an image one cell wide and tall, and the sampler repeats
/// it. That is the whole reason this is affordable: a cell is a texture, and a page may paint
/// one across a thousand shapes, so running the cell's content stream per pixel would cost more
/// than the rest of the page together.
///
/// The cell is drawn at the pattern's own scale — one device pixel per `scale` of pattern space —
/// and the *repeat* is what the sampler does, so nothing here needs to know how many cells the
/// page will use.
fn tiling_fill(
    op: &str,
    name: &str,
    resources: &Resources,
    doc: &Document,
    placement: &Matrix,
    uncoloured: Option<Rgba>,
) -> Result<FillColour, String> {
    let Some(object) = resources.patterns.get(name).cloned() else {
        return Err(format!(
            "a {op} names the tiling pattern `/{name}`, which its resources do not define"
        ));
    };
    let pattern = tiling_pattern(name, &object, &|o| doc.resolve_object(o))?;
    // `/PaintType 2` is an **uncoloured** cell: it paints a shape and takes its colour from
    // whatever the graphics state held when the pattern was selected. That colour survives the
    // `cs` on the colour's own components, and `Colour::under` carries the space they belong to,
    // so there is something to convert. A cell with no such colour — because the page selected a
    // pattern before it ever set one, so the components are whatever the state started as — is
    // **reported by name** rather than painted some arbitrary colour.
    let uncoloured = match pattern.paint_type {
        PaintType::Coloured => None,
        PaintType::Uncoloured => match uncoloured {
            Some(c) => Some(c),
            None => {
                return Err(format!(
                    "a {op} in the uncoloured tiling pattern `/{name}` was found and not drawn: \
                     a /PaintType 2 cell paints in the colour in force when the pattern is \
                     selected, and the page had set none"
                ));
            }
        },
    };
    // The cell's own size on the device, which is its box scaled — and which is asked of the
    // dictionary before anything is allocated, so a hostile `/BBox` costs a few bytes of
    // parsing rather than a gigabyte of image.
    let to_pattern = placement.concat(pattern.matrix);
    // `mean_scale` rather than a separate figure per axis: a cell is a texture and is drawn at
    // one scale, and using the mean keeps a cell square rather than stretching it along one
    // axis to match a skew it will not be tiled with anyway.
    let scale = to_pattern.mean_scale().abs();
    if scale <= 0.0 || !scale.is_finite() {
        return Err(format!(
            "the tiling pattern `/{name}` has a transformation with no scale on it, so its \
             cell has no size on the page and was not drawn"
        ));
    }
    let cell_w = ((pattern.bbox.right - pattern.bbox.left).abs() * scale)
        .ceil()
        .max(1.0);
    let cell_h = ((pattern.bbox.top - pattern.bbox.bottom).abs() * scale)
        .ceil()
        .max(1.0);
    if cell_w * cell_h > MAX_CELL_PIXELS {
        return Err(format!(
            "the tiling pattern `/{name}` has a cell of {cell_w:.0} by {cell_h:.0} pixels, \
             above the {MAX_CELL_PIXELS} this draws, so it was not drawn"
        ));
    }
    let (cell_w, cell_h) = (cell_w as usize, cell_h as usize);

    // The cell's content, decoded and executed on its own resources.
    let Object::Stream(stream) = &pattern.content else {
        return Err(format!(
            "the tiling pattern `/{name}` has no cell to draw, so it was not drawn"
        ));
    };
    let cell_image = render_cell(
        name,
        stream,
        doc,
        pattern.matrix,
        pattern.bbox,
        scale,
        cell_w,
        cell_h,
    )?;

    Ok(FillColour::Tiling {
        cell: cell_image,
        // The cell image starts at pattern-space `bbox.x0, bbox.y0`, and the sampler measures
        // from there, so the origin in device pixels is the top-left of the *box*, not of the
        // pattern space.
        origin: (0.0, 0.0),
        scale: (scale, scale),
        to_pattern,
        step: (pattern.x_step, pattern.y_step),
        bbox: (
            pattern.bbox.left,
            pattern.bbox.bottom,
            pattern.bbox.right,
            pattern.bbox.top,
        ),
        uncoloured,
    })
}

/// The most a single tiling cell may be, in pixels. A cell is a small tile by definition; a
/// pattern whose box covers a whole page is not a texture and is refused rather than rendered.
const MAX_CELL_PIXELS: f64 = 4_000_000.0;

/// Execute a cell's content stream and draw it into an image one cell wide.
#[allow(clippy::too_many_arguments)]
fn render_cell(
    name: &str,
    stream: &mangle_syntax::Stream,
    doc: &Document,
    matrix: Matrix,
    bbox: PageRect,
    scale: f64,
    cell_w: usize,
    cell_h: usize,
) -> Result<Image, String> {
    let decoded = decode_stream(stream);
    if decoded.encoded {
        return Err(format!(
            "the cell of the tiling pattern `/{name}` is still encoded after its filters, so \
             it was not drawn rather than read as operators"
        ));
    }
    if decoded.data.is_empty() {
        return Err(format!(
            "the cell of the tiling pattern `/{name}` decoded to nothing, so it was not drawn"
        ));
    }
    // A cell names its own fonts and images, so it carries its own resources and they are the
    // ones its names resolve against.
    let own = Resources::from_dict(
        stream
            .dict
            .get("Resources")
            .and_then(|r| doc.resolve_object(r))
            .and_then(|r| r.as_dict().cloned())
            .as_ref()
            .unwrap_or(&Dict::new()),
        &|o| doc.resolve_object(o),
    );
    let content = ContentStream::parse(&decoded.data);
    let executed = run_with(&content, &own);
    let mut notes = Vec::new();
    // Pattern space to the cell image's own pixels: the box's top-left at the origin, at the
    // cell's scale, **with y inverted**, because pattern space counts up and an image counts
    // down. The page's own placement is deliberately not applied here — the cell image is
    // already in device-oriented pixel space, and composing the placement in as well would
    // place the cell twice.
    let to_cell = Matrix::new(
        scale,
        0.0,
        0.0,
        -scale,
        -bbox.left * scale,
        bbox.top * scale,
    );
    let cell_placement = Placement {
        matrix: to_cell.concat(matrix),
        size: (cell_w, cell_h),
    };
    let mut device = Device::new(Image::new(cell_w, cell_h));
    let _ = paint_records(
        &executed,
        &mut device,
        &cell_placement,
        &own,
        doc,
        &mut notes,
    );
    let image = device.into_image();
    for note in &notes {
        log::debug!("tiling pattern `/{name}`: {note}");
    }
    Ok(image)
}
///
/// A note that reads "a fill in the PatternType 1 tiling pattern was not drawn" when the
/// content stream stroked a line is a note pointing at the wrong operator, and a note is
/// read by someone looking for the line they asked for. The word costs one enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Paint {
    Fill,
    Stroke,
}

impl Paint {
    /// The word a note about this operator uses.
    fn name(self) -> &'static str {
        match self {
            Self::Fill => "fill",
            Self::Stroke => "stroke",
        }
    }
}

/// The name a `Pattern`-coloured mark carries, or the reason it does not.
fn pattern_name(colour: &mangle_content::Colour) -> Result<&str, String> {
    colour
        .space
        .colorant
        .as_deref()
        .filter(|n| !n.is_empty())
        .ok_or_else(|| "a pattern colour names no pattern, so nothing was drawn in it".to_string())
}

/// What was being painted, for the note that says a colour could not be converted.
///
/// The four marks that can only be flat — a fill, a stroke, the colour of an image mask,
/// and text — each carry the space name in their own note, because what the reader loses
/// differs: a fill loses a shape, a stroke loses an outline, text loses every glyph in
/// that colour on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flat {
    Fill,
    Stroke,
    ImageMask,
    Text,
}

/// The colour for a mark that can only paint one flat colour, or the reason there is none.
///
/// Every one of those four sites goes through here, which is the point: a space this
/// cannot convert has to be refused the same way everywhere it can be painted, and a
/// conversion that worked for a fill and not for a stroke or for text would leave the
/// same colour on a page in two different colours. The note is pushed only on the first
/// refusal of a given page, because a colour repeated forty times says the same thing
/// forty times over.
fn flat_colour(
    colour: &mangle_content::Colour,
    what: Flat,
    notes: &mut Vec<String>,
) -> Option<Rgba> {
    if let Some(rgba) = colour.to_rgba(None) {
        return Some(rgba);
    }
    let space = colour.space.describe();
    let note = match what {
        Flat::Fill => {
            format!("a fill colour in {space} could not be converted, so the shape was not drawn")
        }
        Flat::Stroke => format!(
            "a stroke colour in {space} could not be converted, so the outline was not drawn"
        ),
        // A mask has no colours of its own: this is the only colour it is painted in, so
        // there is nothing left to draw it with.
        Flat::ImageMask => {
            format!("an image mask painted in {space} could not be converted, so it was not drawn")
        }
        Flat::Text => format!(
            "the colour text is painted in {space} could not be converted, so no text on the \
             page in that colour was drawn"
        ),
    };
    if !notes.contains(&note) {
        notes.push(note);
    }
    None
}

/// A `/Matrix` array, or the identity when the dictionary has none.
fn read_matrix(object: Option<&Object>) -> Matrix {
    let Some(values) = object.and_then(Object::as_array) else {
        return Matrix::IDENTITY;
    };
    // A six-element array is `[a b c d e f]`; anything else leaves the transformation
    // alone rather than producing a degenerate one that would collapse the page.
    if values.len() != 6 {
        return Matrix::IDENTITY;
    }
    let at = |i: usize| values.get(i).and_then(Object::as_f64).unwrap_or(0.0);
    Matrix::new(at(0), at(1), at(2), at(3), at(4), at(5))
}

/// Find and decode the image a `Do` names, with whatever the decode had to say about it.
///
/// A page's resource table holds the XObjects by name, and an XObject that is not an image
/// is a note rather than a failure: a form XObject is named by `Do` too, and reaching one is
/// a later step than this.
///
/// The notes come back beside the raster rather than only on the failure path, because an
/// image that *did* draw can still have something to report — a soft mask whose size does not
/// match its image's, or whose samples were short. Dropping those on the way out would say
/// the picture was drawn and say nothing about the part of it that could not be believed, and
/// a note that only survives when nothing was drawn is not a report.
fn image_for(
    name: &str,
    resources: &Resources,
    doc: &Document,
) -> Result<(Raster, Vec<String>), String> {
    let Some(object) = resources.xobjects.get(name).cloned() else {
        return Err(format!(
            "the page names an image `/{name}` that its resources do not define"
        ));
    };
    let resolved = doc.resolve_object(&object).unwrap_or(object);
    let Object::Stream(stream) = resolved else {
        return Err(format!("`/{name}` is named by `Do` but is not a stream"));
    };
    let resolver = |o: &Object| doc.resolve_object(o);
    let mut notes = Vec::new();
    let raster = image::decode(&stream, &resolver, &mut notes).ok_or_else(|| {
        if notes.is_empty() {
            format!("the image `/{name}` could not be decoded")
        } else {
            notes.join("; ")
        }
    })?;
    // The name goes on the front so that a page drawing the same picture from three resources
    // says which of them carried the mask that could not be read.
    let notes = notes
        .into_iter()
        .map(|note| format!("`/{name}`: {note}"))
        .collect();
    Ok((raster, notes))
}

/// A shading lookup: a name to the shading it names, and the pattern's own transformation.
///
/// Passed into `draw_mark` rather than resolved there so that the borrowing stays with
/// whoever owns the document and the page's resources. The resource table is an argument
/// rather than captured, because a mark inside a form names its shadings in the *form's*
/// table.
type ShadingLookup<'a> = &'a mut dyn FnMut(&str, &Resources) -> Result<(Shading, Matrix), String>;

/// A font resource's glyph outlines, and how big its em is.
///
/// `units_per_em` is carried alongside the program rather than left to be asked for
/// separately, because the conversion from a font's own units to ems is the one number
/// that decides whether a glyph comes out at the size the page asked for, and a caller
/// that had to derive it might get it wrong.
pub struct FontProgram {
    /// The font, with its outlines cached by glyph number.
    program: mangle_font::Program,
    /// The size of the font's em, in the font's own units.
    pub units_per_em: u16,
    /// What the font dictionary says a code is called.
    ///
    /// A character code means nothing on its own: it is the `/Encoding` that turns it into a
    /// glyph *name*, and the name that the font is looked up by. A page that remaps even one
    /// code through `/Differences` renders that code with the wrong glyph without this, and
    /// the codes where the three base encodings disagree — 0xA0 to 0xFF — are exactly the ones
    /// a document is most likely to remap.
    encoding: mangle_font::Encoding,
    /// How a CID-keyed font's identifiers name its glyphs, from the descendant's
    /// `/CIDToGIDMap`.
    ///
    /// Carried because it is a property of the *font dictionary* rather than of the font
    /// program: the same TrueType file is reached as a composite font's descendant, where its
    /// codes are CIDs numbered as this says, or as a simple font, where they are character
    /// codes and this does not apply at all. Nothing inside the program can tell those two
    /// apart, which is the whole reason a CID must not be looked up through the font's
    /// `cmap` — the map is the only thing in the file that says which numbering it means.
    cid_to_gid: mangle_font::CidToGid,
    /// The notice to carry when this program is not the document's own font, or `None`.
    ///
    /// A substituted face draws the right glyphs in someone else's outlines, and the user
    /// is entitled to know that: the project does not substitute silently. Carried on the
    /// program rather than pushed from `font_for` because a note is about the page being
    /// drawn, and `font_for` does not know whether the page drew anything with it.
    substituted: Option<String>,
}

impl std::fmt::Debug for FontProgram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontProgram")
            .field("bytes", &self.program.bytes().len())
            .field("units_per_em", &self.units_per_em)
            .field("encoding", &self.encoding.base().name())
            .field("differences", &self.encoding.differences().len())
            .field("cid_to_gid", &self.cid_to_gid)
            .field("substituted", &self.substituted.is_some())
            .finish()
    }
}

/// Find and read the font program a font resource names.
///
/// The program is reached through the font's `/FontDescriptor` rather than directly: the
/// descriptor is where a file says how to interpret it, and going around it would mean
/// guessing. The stream is decoded through the same path `image::decode` uses, because a
/// font program is compressed exactly as an image is and a second filter chain would be a
/// second set of bugs.
///
/// **Three keys, and the difference between them is the whole of the font's geometry.** A
/// `/FontFile2` is a TrueType program; a `/FontFile3` is a CFF one, whose glyphs are Type 2
/// charstrings that have to be executed rather than points that can be walked; and a
/// `/FontFile` is Type 1, which is neither — a PostScript program encrypted twice, with a
/// third charstring dialect inside the second layer. The descriptor names the key, and the
/// key is named in the reason when there is none, because "this font is not embedded" and
/// "this font is embedded in a form this does not read" are different findings.
///
/// The three are tried in that order — 3, then 2, then 1 — which is the order the keys were
/// added to the format and the only order that matters: a descriptor names at most one of
/// them, so the first that is present is the one.
///
/// A font with none is not a failure. The standard fourteen have none by definition, and a
/// document that names one without embedding it is a document whose glyphs come from
/// somewhere else — a substitution, or a font of its own that this does not draw. Either way
/// the reason belongs in the report, and the page is still worth showing.
fn font_for(name: &str, resources: &Resources, doc: &Document) -> Result<FontProgram, String> {
    let Some(object) = resources.fonts.get(name).cloned() else {
        return Err(format!(
            "the page names a font `/{name}` that its resources do not define"
        ));
    };
    let resolved = doc.resolve_object(&object).unwrap_or(object);
    let Object::Dict(font) = resolved else {
        return Err(format!("the font `/{name}` is not a dictionary"));
    };
    // A Type 3 font's glyphs are **content streams**, not outlines, so it has no
    // `/FontDescriptor` and no embedded program to read. It is refused here rather than
    // further down, and the reason is why: further down the search ends at
    // `substitute_for`, which asks only what the `/BaseFont` is called — and a Type 3 font
    // is free to be called anything at all. The two pdfjs regression files in the corpus
    // name theirs `/Helvetica`, which has a metric-compatible face bundled for it, so the
    // substitution *succeeds*: the page draws Helvetica's glyphs in another face's outlines
    // while the document defined each glyph as a stream. That is a plausible wrong answer,
    // not a visible gap, and it is the one thing this project is built not to do.
    if font.get("Subtype").and_then(Object::as_name) == Some(b"Type3") {
        return Err(format!(
            "the font `/{name}` is a Type 3 font, whose glyphs are content streams rather \
             than outlines, so they are not drawn"
        ));
    }
    // A composite (Type 0) font holds no font program of its own. Its `/DescendantFonts`
    // array names the CIDFont, and *that* is the dictionary with the `/FontDescriptor` and
    // the `/FontFile2` in it — so a page in a composite font reads exactly the same
    // descriptor it would for a simple font, one indirection further along.
    let composite = font.get("Subtype").and_then(Object::as_name) == Some(b"Type0");
    let descendant = if composite {
        doc.resolve_object(font.get("DescendantFonts").unwrap_or(&Object::Null))
            .and_then(|o| o.as_array().map(<[Object]>::to_vec))
            .and_then(|a| a.first().cloned())
            .and_then(|o| doc.resolve_object(&o))
            .and_then(|o| match o {
                Object::Dict(d) => Some(d),
                _ => None,
            })
    } else {
        None
    };
    let owner = descendant.as_ref().unwrap_or(&font);
    let descriptor = owner
        .get("FontDescriptor")
        .map(|o| doc.resolve_object(o).unwrap_or_else(|| o.clone()))
        .unwrap_or(Object::Null);
    // No embedded program of any kind is not a failure on its own: it is what the standard
    // fourteen are, and a metric-compatible face may stand in. So both of the reasons that
    // used to end the search here are asked of `substitute_for` first, and each of them is
    // what is still said when there is nothing to substitute. The distinction between "not
    // embedded" and "embedded in a form this does not read" is preserved rather than
    // flattened, because they are different findings and one of them is worth chasing.
    let key = match &descriptor {
        Object::Dict(descriptor) => ["FontFile3", "FontFile2", "FontFile"]
            .into_iter()
            .find(|key| descriptor.get(key).is_some()),
        _ => None,
    };
    let Some(key) = key else {
        // `Ok(None)` is "nothing to stand in", which leaves the reason below. `Err` is a
        // fault in the bundle rather than in the document, so it says so and is not folded
        // into the document's own finding.
        match substitute_for(name, &font, composite, doc) {
            Ok(Some(program)) => return Ok(program),
            Err(why) => return Err(why),
            Ok(None) => {}
        }
        return Err(match &descriptor {
            Object::Dict(_) => format!(
                "the font `/{name}` is not embedded, so it has no outlines of its own to draw"
            ),
            _ => format!(
                "the font `/{name}` has no `/FontDescriptor`, so there is nothing to read a \
                 font program from"
            ),
        });
    };
    let file = doc
        .resolve_object(descriptor.get(key).unwrap_or(&Object::Null))
        .unwrap_or_else(|| descriptor.get(key).unwrap_or(&Object::Null).clone());
    let Object::Stream(stream) = file else {
        return Err(format!("the `/{key}` of `/{name}` is not a stream"));
    };
    // The filters below a font program are the same filters below an image, and they are
    // the reason this is `decode_stream` rather than `stream.raw`.
    let decoded = decode_stream(&stream);
    let mut program = mangle_font::Program::new(decoded.data);
    let units_per_em = program
        .inspect()
        .map_err(|why| format!("the `/{key}` of `/{name}` {why}"))?;
    // A program this can read but cannot turn a *character code* into a glyph for is a
    // different finding again, and one that costs every character on the page rather than
    // one glyph. Saying so here means the page reports one reason instead of drawing
    // nothing and saying nothing.
    if let Some(reason) = program.code_refusal() {
        return Err(format!("the `/{key}` of `/{name}` {reason}"));
    }
    // The `/Encoding` is the font dictionary's own, so it is read the same way whichever way
    // the outlines were found — see `encoding_for`.
    Ok(FontProgram {
        program,
        units_per_em,
        encoding: encoding_for(&font, composite, doc),
        cid_to_gid: cid_to_gid_for(owner, doc),
        substituted: None,
    })
}

/// The `/CIDToGIDMap` of the descendant font whose codes are being drawn.
///
/// `owner` is the descendant for a composite font and the font itself otherwise, so a simple
/// font answers the identity map: it has no CIDs, so it declares no map, and a caller that
/// had one anyway would be the caller at fault.
fn cid_to_gid_for(owner: &Dict, doc: &Document) -> mangle_font::CidToGid {
    mangle_font::CidToGid::from_descendant_dict(owner, &|o| doc.resolve_object(o))
}

/// The `/Encoding` a font's character codes are addressed through.
///
/// The `/Encoding` is on the *font* dictionary, not the descendant's, and a composite
/// font's is a CMap rather than any of the four shapes an encoding takes — so it is only
/// read for a simple font, where the code really is a character code. A composite font's
/// code is a CID and `/Differences` does not apply to it, which is why that font keeps the
/// path it had rather than having an encoding forced onto a number that is not one.
///
/// The default base is the standard encoding: a simple font that names no `/Encoding` is
/// addressed through its own built-in encoding where it carries one, and through the
/// standard encoding where it does not, which is what `Type1::glyph_for_code` already does.
/// Reading it here as well means a font that *does* declare one gets it, and one that does
/// not is unaffected.
fn encoding_for(font: &Dict, composite: bool, doc: &Document) -> mangle_font::Encoding {
    if composite {
        return mangle_font::Encoding::new(mangle_font::EncodingBase::Standard);
    }
    mangle_font::Encoding::from_font_dict(font, mangle_font::EncodingBase::Standard, &|o| {
        doc.resolve_object(o).or_else(|| Some(o.clone()))
    })
    .unwrap_or_else(|| mangle_font::Encoding::new(mangle_font::EncodingBase::Standard))
}

/// A metric-compatible face for a font the document did not embed.
///
/// `Ok(None)` is "the name has no stand-in", which is an ordinary answer and says nothing
/// about the document. `Err` is a fault in the bundle, which is not: those are kept apart
/// so a broken bundled face is never reported as a document's missing font.
///
/// **This is not a fallback of last resort; it is what a reader does.** The standard
/// fourteen are defined by their metrics rather than by any one program, so a document that
/// names one without embedding it — which is most of the LaTeX and office output in the
/// wild, because those producers assume the reader has the font — is naming a font, not
/// naming nothing. Refusing it draws a page with a hole where the text was, and that is a
/// worse answer than drawing the text in a face that agrees about every width.
///
/// The name goes to [`mangle_font::substitute`] whole, which is where the subset prefix
/// (`ABCDEF+`), the style suffix and the older comma spelling are already dealt with, so
/// there is no second name parser here that can disagree with the first. A name with no
/// metric-compatible stand-in — `Symbol`, `ZapfDingbats`, anything non-standard — answers
/// `None`, and the refusal is then free to say so.
///
/// The substitution is reported rather than made quietly: `substituted` carries the notice
/// so the page can name the face standing in, which is the one fact about this that a user
/// cannot read off the picture.
fn substitute_for(
    name: &str,
    font: &Dict,
    composite: bool,
    doc: &Document,
) -> Result<Option<FontProgram>, String> {
    // A composite font's codes are CIDs the document chose itself, so a simple face is not a
    // stand-in for one however well the widths happen to agree.
    if composite {
        return Ok(None);
    }
    let Some(base) = font.get("BaseFont").and_then(Object::as_name) else {
        return Ok(None);
    };
    let base = String::from_utf8_lossy(base).into_owned();
    let Some(bytes) = mangle_font::substitute(&base) else {
        return Ok(None);
    };
    let mut program = mangle_font::Program::new(bytes.to_vec());
    let units_per_em = program
        .inspect()
        .map_err(|why| format!("the face that stands in for `{base}` {why}"))?;
    Ok(Some(FontProgram {
        program,
        units_per_em,
        encoding: encoding_for(font, composite, doc),
        cid_to_gid: mangle_font::CidToGid::Identity,
        substituted: Some(format!(
            "the font `/{name}` is not embedded, so a metric-compatible face stands in for \
             `/{base}`: the glyphs are that face's, not the original's"
        )),
    }))
}

/// A font lookup: a font resource name to the outlines it names.
///
/// Passed into `draw_mark` for the same reason as the image lookup, and with the same
/// shape, so that the document and the page's resources stay borrowed by whoever owns
/// them rather than by a function that has to reach into both. The table is an argument
/// because a font named inside a form lives in the form's own resources, which is the only
/// thing that says which font `/F1` means.
type FontLookup<'a> = &'a mut dyn FnMut(&str, &Resources) -> Result<FontProgram, String>;

/// An image lookup: an XObject name to the samples it names.
///
/// For the same reason as the other two, and with the same shape.
type ImageLookup<'a> = &'a mut dyn FnMut(&str, &Resources) -> Result<(Raster, Vec<String>), String>;

/// A rectangle carried through the page placement, as a device-space rectangle.
///
/// Every corner goes through the matrix and the result is re-bounded, because a
/// transformation that rotates or flips does not map an axis-aligned rectangle to an
/// axis-aligned one.
fn placed_rect(bounds: &mangle_content::ClipBounds, placement: &Matrix) -> Rect {
    let corners = [
        placement.apply(bounds.x0, bounds.y0),
        placement.apply(bounds.x1, bounds.y0),
        placement.apply(bounds.x1, bounds.y1),
        placement.apply(bounds.x0, bounds.y1),
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
    Rect {
        x0: min_x,
        y0: min_y,
        x1: max_x,
        y1: max_y,
    }
}

/// A record's clip, as the device takes it.
///
/// The interpreter has already put the region's paths and its box through the CTM that was in
/// force when the clip was set, so they are in the page's own space and the *only*
/// transformation left is the page placement. Pushing them through a mark's matrix as well
/// applies the CTM twice, which is invisible under an identity CTM and turns a scaled clip
/// into no clip at all.
///
/// Every path in force goes over, not only the newest one: two `W n` operations nest, and a
/// device handed one path and the intersected box would paint the older clip with its own
/// shape replaced by its own box.
fn clip_region(clip: &mangle_content::Clip, placement: &Matrix) -> ClipRegion {
    ClipRegion {
        bounds: placed_rect(&clip.bounds, placement),
        paths: clip
            .paths
            .iter()
            .map(|path| ClipShape {
                polygon: transform_path(&path.segments, placement),
                rule: fill_rule_of(path.rule),
            })
            .collect(),
    }
}

/// Draw one mark.
#[allow(clippy::too_many_arguments)]
/// The blend mode a mark composites under, and a note if it is one we do not implement.
///
/// **A mode this renderer does not implement is reported and composited as `Normal`**, rather
/// than guessed at. Substituting `Normal` for `Difference` or `Luminosity` puts the wrong colours
/// on the page while looking entirely plausible, which is the failure this project keeps finding;
/// a note says which mode was dropped, so the page is at least self-describing. The four modes
/// that *are* implemented are the ones documents write in practice — `Multiply` above all, since
/// it is how a highlight is normally expressed.
fn blend_for(record: &mangle_content::Record, notes: &mut Vec<String>) -> BlendMode {
    let name = record.blend_mode.trim();
    if name.is_empty() || name == "Normal" {
        return BlendMode::Normal;
    }
    match BlendMode::parse(name) {
        Some(mode) => mode,
        None => {
            notes.push(format!(
                "the blend mode `{name}` is not one this renderer implements, so what it drew \
                 was composited as Normal instead"
            ));
            BlendMode::Normal
        }
    }
}

fn draw_mark(
    device: &mut Device,
    mark: &Mark,
    to_device: &Matrix,
    placement: &Matrix,
    record: &mangle_content::Record,
    notes: &mut Vec<String>,
    images: ImageLookup<'_>,
    shadings: ShadingLookup<'_>,
    fonts: FontLookup<'_>,
    // The resources this mark's names resolve against — the form's own where it was drawn
    // inside one, the page's otherwise — and the document. They are here for the one thing
    // a name alone cannot answer: a pattern colour is a reference to a pattern resource, so
    // the resource has to be read rather than looked up by the caller. The three lookups
    // above are closures so that the borrows stay with `render_page`; this is the same
    // information, taken directly, because it is needed in three arms rather than in one.
    resources: &Resources,
    doc: &Document,
) {
    match mark {
        Mark::Path {
            segments,
            fill,
            stroke,
            rule,
        } => {
            // The path's points arrive already multiplied by the CTM that was in force when
            // the operator ran — `GraphicsState::device_path` does that, and it is why a
            // `Mark::Path`'s geometry is in the page's own space rather than in user space.
            // The placement is therefore the *only* part of `to_device` these points have not
            // seen, so applying `to_device` applies the CTM a second time: a 1×1 square
            // under `50 0 0 50 10 10 cm` becomes a 2500-unit one, off the page, drawn
            // nowhere. This is the same rule `clip_region` follows, and for the same reason.
            let polygon = transform_path(segments, placement);
            if polygon.is_empty() {
                return;
            }
            // The clip is not narrowed to the shape here. `fill_polygon` bounds its own work
            // to the polygon's box intersected with the clip, which is the same answer, and a
            // narrowing kept in the device would outlive this call: the next mark brings its own
            // clip and must not inherit this one's cull.
            let rule = match rule {
                mangle_content::FillRule::EvenOdd => FillRule::EvenOdd,
                mangle_content::FillRule::NonZero => FillRule::NonZero,
            };
            // Resolved once per mark and reported once, rather than per pixel or per leg.
            let blend = blend_for(record, notes);
            if let Some(colour) = fill {
                // A `Pattern` colour space means the operands named a pattern rather than a
                // colour value, so the pattern has to be read and its own evaluator built.
                // Every other space is converted here as it always was, on the same code
                // path as before — a page with an ordinary colour fill does not come near
                // the per-pixel one.
                if colour.space.name == "Pattern" {
                    match pattern_name(colour).and_then(
                        |name| // A `/PaintType 2` cell paints in the colour in force when the pattern
                            // is used, and a `Pattern` colour space has replaced that colour by
                            // now, so there is nothing here to give it. Refused by name in
                            // `tiling_fill` rather than guessed at.
                            pattern_fill(
                                Paint::Fill,
                                name,
                                resources,
                                doc,
                                placement,
                                colour.to_rgba(None),
                            ),
                    ) {
                        Ok(paint) => {
                            fill::polygon(device, &polygon, rule, &paint, record.fill_alpha, blend);
                        }
                        Err(reason) => notes.push(reason),
                    }
                } else if let Some(rgba) = flat_colour(colour, Flat::Fill, notes) {
                    device.fill_polygon(&polygon, rule, rgba.to_rgba8(record.fill_alpha), blend);
                }
            }
            if let Some(colour) = stroke {
                // The one place a stroke's width becomes a number of device pixels, and it
                // is here because that is the layer that knows the page scale — the same
                // reason `polygon` above is transformed by the placement and not by
                // `to_device`. `record.device_line_width` is the content stream's own
                // factor; the placement's is applied alongside the geometry.
                let width = device_line_width(record.device_line_width, placement);
                let style = line_style_of(record.line_cap, record.line_join);
                let style = StrokeStyle {
                    width,
                    // The dash lengths are user-space lengths too, and they are
                    // carried into device space by the same code that carries the
                    // width and beside the geometry they are drawn along.
                    dash: device_dash(&record.dash, &record.ctm, placement),
                    ..style
                };
                // A stroke's paint is a paint operator's paint, and `SCN` names a pattern
                // here exactly as `scn` does for the fill above. The shape is the stroke's
                // own outline either way, so the only question is whether one colour answers
                // for every pixel of it — and asking a colour converter for a pattern's
                // colour answers nothing, which is why a page whose ink was all patterned
                // strokes came out blank.
                if colour.space.name == "Pattern" {
                    match pattern_name(colour).and_then(|name| {
                        pattern_fill(Paint::Stroke, name, resources, doc, placement, None)
                    }) {
                        Ok(paint) => {
                            fill::stroke(
                                device,
                                &polygon,
                                &style,
                                &paint,
                                record.stroke_alpha,
                                blend,
                            );
                        }
                        Err(reason) => notes.push(reason),
                    }
                } else if let Some(rgba) = flat_colour(colour, Flat::Stroke, notes) {
                    device.stroke_polygon(
                        &polygon,
                        &style,
                        rgba.to_rgba8(record.stroke_alpha),
                        blend,
                    );
                }
            }
        }
        // Nothing to do, and deliberately so. A clip is part of the graphics state, so the
        // clip in force for the marks after this one is the state they were created in — and
        // every one of those records carries it, so `render_page` has installed it before
        // this mark and will install the new one before the next. Installing it here as well
        // would make this mark reach past itself to change what the next one sees, which is
        // how a clip ends up in force for one mark rather than for the rest of the page.
        //
        // The variant stays because the interpreter still emits it: it is where on the page a
        // clip changed, which is a fact about the page rather than about any one mark, and it
        // is what the display list and any future feature that has to know a clip changed will
        // ask for.
        Mark::ClipChanged(_) => {}
        // A `sh` with no operand at all names nothing, and the interpreter leaves the name
        // empty rather than guessing one.
        Mark::Shading { name, .. } => match name.as_str() {
            "" => notes.push("a shading with no name was found and not painted".into()),
            name => match shadings(name, resources) {
                Ok((shading, pattern_matrix)) => {
                    // The mark's transformation carries the pattern's space to the pixels
                    // and the pattern's own matrix sits inside it, so the two compose in
                    // that order and the gradient is sampled in its own space throughout.
                    let to_shading = to_device.concat(pattern_matrix);
                    // A shading paints the current clip, which for `sh` is whatever path the
                    // page set with `W n`: painting outside it would put a gradient where the
                    // page drew nothing. That clip is already installed — the record carries it
                    // — and it goes in whole rather than only as its box, so a gradient inside
                    // a diagonal clip is diagonal and inside a circular one it is round.
                    shading::paint(device, &shading, &to_shading, record.fill_alpha);
                }
                Err(reason) => notes.push(reason),
            },
        },
        Mark::Image { name, fill, .. } => match name.as_deref() {
            // An XObject that is neither an image nor a form never reaches here: the
            // interpreter reports it by name rather than recording a mark for it, because a
            // mark is a claim that something was drawn.
            Some(name) => match images(name, resources) {
                Ok((raster, said)) => {
                    // Whatever the decode had to report goes on the page even though the
                    // picture was drawn — an image whose soft mask could not be read is on
                    // the page, and so is the reason it has no transparency.
                    //
                    // Once per page rather than once per mark, for the reason the font's
                    // notices are: a form drawn forty times draws the same picture forty
                    // times, and the note names the resource rather than the mark, so
                    // repeating it says the same thing forty times over. A page whose report
                    // is forty identical lines is a page whose report is not read.
                    for reason in said {
                        if !notes.contains(&reason) {
                            notes.push(reason);
                        }
                    }
                    // The image's own space is the unit square, so the mark's
                    // transformation is all that is needed to place it. A mask is painted in
                    // the graphics state's non-stroking colour, which `Do` does not name and
                    // so has to travel with the mark.
                    let paint = if raster.is_stencil && fill.space.name == "Pattern" {
                        // A pattern colour is a reference to a pattern rather than a colour
                        // value, and the pattern's colour varies with position: the mask is
                        // painted in whatever the pattern says at each pixel it covers. The
                        // mask is filled rather than stroked, so it names itself as a fill.
                        match pattern_name(fill).and_then(|name| {
                            // A `/PaintType 2` cell paints in the colour in force when the pattern
                            // is used, and a `Pattern` colour space has replaced that colour by
                            // now, so there is nothing here to give it. Refused by name in
                            // `tiling_fill` rather than guessed at.
                            pattern_fill(Paint::Fill, name, resources, doc, placement, None)
                        }) {
                            Ok(paint) => Some(paint),
                            Err(reason) => {
                                notes.push(format!(
                                    "an image mask painted in a pattern was found and not \
                                     drawn: {reason}"
                                ));
                                None
                            }
                        }
                    } else if raster.is_stencil {
                        // A mask is painted entirely in the graphics state's colour — its
                        // own samples say only which pixels — so that colour is the whole
                        // of it and a space this cannot convert leaves nothing to draw
                        // with. An image that is not a mask paints its own samples and
                        // never asks, which is why the conversion is only made here for
                        // the one case that needs it.
                        flat_colour(fill, Flat::ImageMask, notes).map(FillColour::flat)
                    } else {
                        None
                    };
                    image::draw(
                        device,
                        &raster,
                        to_device,
                        record.fill_alpha,
                        paint.as_ref(),
                    );
                }
                Err(reason) => notes.push(reason),
            },
            None => {
                // An inline image's samples are in the content stream rather than in a
                // resource, so the content layer has to carry them out; it does not yet.
                notes.push(
                    "an inline image was found and not drawn: its samples live in the content \
                     stream and are not carried out of it yet"
                        .into(),
                );
            }
        },
        Mark::Glyphs {
            font,
            codes,
            two_byte,
            fill,
            placements,
            ..
        } => {
            // `Tf` with no font means the state is damaged and the bytes name nothing.
            let Some(name) = font.as_deref().filter(|n| !n.is_empty()) else {
                return;
            };
            let mut program = match fonts(name, resources) {
                Ok(program) => program,
                Err(reason) => {
                    // Once per run rather than once per page. A page of prose in a
                    // non-embedded font would otherwise produce a note per line, which
                    // buries the one finding that mattered.
                    if !notes.contains(&reason) {
                        notes.push(reason);
                    }
                    return;
                }
            };
            // The same once-per-page rule for the notice that a face is standing in: it is
            // a fact about the font, so saying it again per run says the same thing more
            // loudly.
            if let Some(reason) = program.substituted.clone()
                && !notes.contains(&reason)
            {
                notes.push(reason);
            }
            let Some(rgba) = flat_colour(fill, Flat::Text, notes) else {
                return;
            };
            let ink = rgba.to_rgba8(record.fill_alpha);
            // `codes` is the character codes and `placements` is one matrix per code, so
            // the two zip; a run whose code has no placement is not drawn, which is what
            // a truncated record means rather than a glyph at the origin. The codes are
            // read from the mark rather than from the string's bytes because a composite
            // font's codes are two bytes each, and the mark is what says so.
            for (code, glyph_matrix) in codes.iter().zip(placements.iter()) {
                // A composite font's code is looked up as a CID, through the descendant's
                // `/CIDToGIDMap` — which is the whole of how a CID reaches a glyph, and is
                // the identity map unless the file says otherwise. A simple font's code
                // goes through its `/Encoding` and then the subtables a simple font uses.
                // Which one applies is the mark's claim, not a guess made here.
                //
                // The two paths are different questions and are kept apart. A CID is an
                // identifier the font was built to be addressed by, and `/Differences` does
                // not apply to one, so no encoding is consulted for it and no `cmap` is
                // either: a `cmap` numbers characters, which is a different numbering from
                // the one the file declared. A character code means nothing until the
                // `/Encoding` turns it into a glyph name, and a page that remapped that code
                // is asking for a different glyph.
                //
                // In two steps because the map is read while the program is written to, and
                // they are two fields of one value.
                let found = if *two_byte {
                    program
                        .program
                        .glyph_for_cid(*code, &program.cid_to_gid)
                        .and_then(|glyph| program.program.outline(glyph))
                } else {
                    let name = program.encoding.glyph_for(*code);
                    program.program.outline_for_code_named(*code, name)
                };
                // No outline is a space, or a code the font does not have. Both are the
                // common case and neither is a failure: a page of prose is mostly spaces
                // and a report listing one note per space is a report nobody reads.
                let Some((outline, _)) = found.filter(|(o, _)| !o.is_empty()) else {
                    continue;
                };
                // The outline is in ems and the placement matrix carries the font size as
                // its innermost factor, so the two compose directly — there is no
                // `units_per_em` step here, because the font has already divided by it.
                let to_device = placement.concat(*glyph_matrix);
                let segments: Vec<PathSegment> = outline
                    .segments
                    .iter()
                    .map(|s| match *s {
                        mangle_font::Segment::Move(x, y) => PathSegment::Move(x, y),
                        mangle_font::Segment::Line(x, y) => PathSegment::Line(x, y),
                        mangle_font::Segment::Curve(a, b, c, d, e, f) => {
                            PathSegment::Curve(a, b, c, d, e, f)
                        }
                    })
                    .collect();
                let polygon = transform_path(&segments, &to_device);
                if polygon.is_empty() {
                    continue;
                }
                // No culling of the clip to the outline's box, for the same reason a path
                // does not cull it: `fill_polygon` bounds its own work, and a cull left in the
                // device would reach into the next glyph and the next mark.
                //
                // Non-zero, because that is what TrueType outlines are wound for: an
                // outer contour and a hole wound the other way both come out filled, and
                // a point inside two same-wound contours is inside the glyph, which is
                // what the data says it is.
                device.fill_polygon(&polygon, FillRule::NonZero, ink, blend_for(record, notes));
            }
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
        // As in `render_page`: the canvas is already in pixels, so the fit is into it and
        // not into a page-sized box that the zoom is then applied to.
        1.0,
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

/// The thinnest line a device can draw, in device pixels.
///
/// ISO 32000-1 Table 52: a `/LineWidth` of zero "shall be rendered as the thinnest line that
/// can be rendered", which is one pixel and not zero — the line is a hairline, not an absence.
/// One pixel is a property of the *device* rather than of the page, so it is one pixel at
/// every scale: a hairline does not thicken when the page is zoomed, which is what makes it a
/// hairline. Poppler, asked, gives a single hard row for `0 w` at 72, 150 and 300 DPI alike.
const HAIRLINE_WIDTH: f64 = 1.0;

/// The width a stroke is drawn at, in device pixels.
///
/// `Record::device_line_width` carries the content stream's own factor — `w × mean(ctm)` — and
/// nothing else, because the interpreter knows the `cm` and no more: not the canvas, not the
/// scale, not `/Rotate`. The factor that turns points into pixels is the renderer's, exactly as
/// it is for `Placement::fit` and for the transformation of the path's own points, so it is
/// applied here, at the same time and by the same code as the geometry beside it. Leaving it
/// off is what made a stroke a hairline at every zoom: `2 w` on `0 0 10 10 re` under
/// `4 0 0 4 50 50 cm` was 48 device pixels wide at scale 1 and 88 at scale 2, where the
/// closed form is 96.
///
/// The two factors are **multiplied**, not composed into one matrix, because the geometry has
/// already had the CTM applied to it by `device_path`. Asking `placement ∘ ctm` for its scale
/// would apply the CTM to the width a second time, which is D9's defect wearing a different
/// hat.
///
/// **A non-uniform `cm` has no single answer**, and this is the documented one: the geometric
/// mean of the two axis scales, `sqrt(|det|)`, which is [`Matrix::mean_scale`]. A transform
/// multiplies an area by `|det| = s²`, so `s` is the factor by which it scales a region, and it
/// is the midpoint *between* the two axes rather than outside them — which is the most that a
/// single number can be when the truth is an ellipse.
///
/// For `4 0 0 2` with `2 w` the stroke is the ellipse the matrix gives a circle of radius 1:
/// semi-axes `1 × 4 = 4` and `1 × 2 = 2`, so 8 across and 4 up where the path runs
/// horizontally, and both oracles on this machine draw exactly that — 48 by 24 device pixels
/// for the square of `0 0 10 10 re`, which is 40 by 20 with 4 and 2 either side. This draws
/// `2 × sqrt(8) ≈ 5.657` either side, which is 45.7 by 25.7: the right order of magnitude and
/// the right ink on average, and a shape that is a compromise rather than the answer. Getting
/// the ellipse means stroking in user space and transforming the outline afterwards, which is
/// a change to the stroker and not to this arithmetic; `docs/known-diffs.md` records the
/// difference until it is made.
fn device_line_width(stream_width: f64, placement: &Matrix) -> f64 {
    // `abs` because a negative width is invalid and `stroke_outline` already assumed one
    // would be drawn from it; a width that cannot be a number at all is a hairline rather
    // than an exception.
    let width = stream_width.abs() * placement.mean_scale();
    if width.is_finite() && width > 0.0 {
        width
    } else {
        HAIRLINE_WIDTH
    }
}

/// A dash pattern carried from the page's own units into device pixels.
///
/// Every number in a dash array is a **length in the space current when `d` ran**, which is
/// user space, and so is `/Phase`: a pattern is a distance along the path, and a distance
/// has to be multiplied by everything that turns a unit into a pixel. `mutool` was asked and
/// does: `[6 3] 0 d` gives 6, 12 and 24 device pixels of on-run at 72, 144 and 288 DPI, while
/// an unscaled array gives six at all three — a line that is dashed with a fixed pattern
/// however far the page is zoomed in.
///
/// The factors are applied **here**, beside the geometry, and not in the interpreter, for
/// `device_line_width`'s reason: the interpreter knows the content stream's `cm` and no more,
/// so scaling the array there would put one factor where the canvas's is not, and the record
/// would carry a pattern already half-converted. The two factors are therefore multiplied
/// rather than composed into one matrix — `mean_scale(ctm) × mean_scale(placement)` — because
/// the path's own points have already had the CTM applied to them by `device_path`, and asking
/// `placement ∘ ctm` for its scale would apply the CTM to the lengths a second time.
///
/// The **phase** is scaled by the same factor as the lengths, which is what keeps it a phase:
/// `walk_dashes` measures the phase against the pattern's own total, and multiplying both by
/// one factor leaves the fraction of the pattern it names unchanged. Scaling the lengths alone
/// would move the start of every dash as the page is zoomed, which is a different picture
/// rather than the same one drawn larger.
///
/// A zero-length element is legal and means "the same colour twice". It stays zero here —
/// a zero times a finite factor is a zero, so nothing is scaled *into* a division by zero —
/// and `walk_dashes` already walks a zero-length element as a hairline rather than a spin. An
/// array that sums to zero has no pattern to walk at all, so `walk_dashes` draws the line solid
/// and returns before it divides by the total; an array that cannot be a number of pixels is
/// passed through unscaled rather than turned into an infinity the walk would read as "no gap".
fn device_dash(
    dash: &mangle_content::Dash,
    ctm: &Matrix,
    placement: &Matrix,
) -> mangle_content::Dash {
    if dash.array.is_empty() {
        return dash.clone();
    }
    let scale = ctm.mean_scale() * placement.mean_scale();
    if !scale.is_finite() || scale <= 0.0 {
        return dash.clone();
    }
    let array: Vec<f64> = dash.array.iter().map(|v| v * scale).collect();
    let phase = dash.phase * scale;
    if !phase.is_finite() || array.iter().any(|v| !v.is_finite()) {
        return dash.clone();
    }
    mangle_content::Dash { array, phase }
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
        // Closed, because a rectangle's border is a ring: a stroked rectangle is an outline
        // with a hole in it rather than a loop of ink.
        subpaths: vec![Subpath::closed(vec![
            (x0, y0),
            (x1, y0),
            (x1, y1),
            (x0, y1),
            (x0, y0),
        ])],
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

    /// A dash pattern's lengths and phase are both lengths in user space, so both are scaled.
    ///
    /// The arithmetic is the same one `device_line_width` does — the mark's own `cm` and the
    /// placement, multiplied rather than composed — and the phase is scaled **by the same
    /// factor as the lengths**, which is what keeps it a phase: a phase is a fraction of the
    /// pattern's own total, so scaling both leaves the fraction named unchanged.
    #[test]
    fn a_dash_pattern_and_its_phase_are_scaled_by_both_factors() {
        let identity = Matrix::IDENTITY;
        let ctm = Matrix::new(4.0, 0.0, 0.0, 4.0, 50.0, 50.0);
        let placement = Matrix::new(3.0, 0.0, 0.0, 3.0, 0.0, 0.0);
        let pattern = mangle_content::Dash {
            array: vec![6.0, 3.0],
            phase: 2.0,
        };
        let got = device_dash(&pattern, &ctm, &placement);
        assert_eq!(
            got.array,
            vec![72.0, 36.0],
            "`6 3` under a `cm` of four and a placement of three is 72 and 36 device pixels"
        );
        assert!(
            near(got.phase, 24.0),
            "and the phase is the same twelve times over — 24 device pixels, not 2: a phase \
             left in user space moves every dash along the line when the page is zoomed"
        );
        // The total is what a phase is measured against, and both numbers moved by one factor,
        // so the fraction of the pattern the phase names is the fraction the file asked for.
        let fraction = got.phase / (got.array[0] + got.array[1]);
        assert!(
            (fraction - 2.0 / 9.0).abs() < 1e-12,
            "two ninths of the pattern is still two ninths of it: got {fraction}"
        );
        // The identity is the identity: a page with no `cm` and a placement of one is not
        // changed at all, which is every page in the corpus.
        let same = device_dash(&pattern, &identity, &identity);
        assert_eq!(
            same.array,
            vec![6.0, 3.0],
            "an unscaled page keeps its pattern"
        );
        assert!(near(same.phase, 2.0), "and its phase");
    }

    /// The cases where a dash pattern must be handed on untouched.
    ///
    /// A pattern that cannot be scaled must not be scaled into something that cannot be walked:
    /// an empty array is the specification's solid line, a zero-length entry is legal and stays
    /// zero — a factor times a zero is a zero, so nothing here can divide by a zero-length
    /// element — and a page whose scale is zero or not a number leaves a pattern alone rather
    /// than turning every length into an infinity the walk would read as "no gaps".
    #[test]
    fn a_dash_pattern_that_cannot_be_scaled_is_handed_on_unchanged() {
        let identity = Matrix::IDENTITY;
        let scaled = Matrix::new(2.0, 0.0, 0.0, 2.0, 0.0, 0.0);
        let flat = Matrix::new(0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        let broken = Matrix::new(f64::NAN, 0.0, 0.0, 1.0, 0.0, 0.0);

        let empty = mangle_content::Dash::default();
        assert_eq!(
            device_dash(&empty, &scaled, &scaled),
            empty,
            "an empty array is solid, and stays solid however large the page is"
        );

        let zeroed = mangle_content::Dash {
            array: vec![6.0, 0.0, 3.0, 0.0],
            phase: 0.0,
        };
        let got = device_dash(&zeroed, &scaled, &scaled);
        assert_eq!(
            got.array,
            vec![24.0, 0.0, 12.0, 0.0],
            "a zero-length entry is legal and scales to a zero-length entry, never to a \
             division by zero"
        );

        for bad in [flat, broken] {
            assert_eq!(
                device_dash(&zeroed, &identity, &bad),
                zeroed,
                "a placement that is not a scale leaves the pattern as the file wrote it"
            );
        }
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

    #[test]
    fn a_page_buffer_rounds_up_so_the_edge_is_not_clipped() {
        // 100 points at 150 DPI is 208.33 pixels, which must become 209 and not 208: a
        // buffer that rounds down clips the last third of a pixel off the page, and a size
        // that disagrees with the oracle's makes a page-by-page comparison impossible.
        let scale = 150.0 / 72.0;
        let page = page_of_size(100);
        let doc = Document::open(page_bytes(100), mangle_syntax::OpenOptions::default())
            .expect("the file opens");
        let render = render_page(
            &doc,
            &page,
            &Resources::default(),
            RenderOptions {
                scale,
                ..RenderOptions::default()
            },
        );
        assert_eq!(
            render.image.width, 209,
            "100 points at 150 DPI is 208.33 pixels, which rounds up"
        );
    }

    /// A `/CropBox` written to eight decimals does not make the buffer a pixel taller.
    ///
    /// The test above covers a page whose size is a whole number of pixels *in exact
    /// arithmetic*, which fails by one ulp. This covers the other case, which is far more
    /// common and much larger: the producer wrote a decimal that is not quite the number it
    /// meant. `pdfjs__freeculture.pdf` writes `/CropBox [86.399997 248.40001 525.6 680.4]`, so
    /// its height is 734.400024 where 734.4 was meant, and at 150 DPI that is 1530.00005 — five
    /// hundred times further from a whole pixel than one ulp, and invisible to a tolerance
    /// narrow enough to spare a genuinely-fractional page.
    ///
    /// **Both numbers were measured against `mutool draw -r 150`**, which is what makes the
    /// tolerance a decision rather than a guess. Feeding it pages whose height lands 0.001,
    /// 0.005 and 0.05 pixels above a whole 1530 gives 1530, 1531 and 1531, so it snaps within
    /// about 1e-6 relative and ceils beyond it. Two corpus pages stopped disagreeing with it
    /// when this tolerance was widened from 1e-9 to 1e-6, and none started.
    ///
    /// The two bands are asserted separately because the failure in each direction is
    /// different: too narrow and the page is a pixel too big and the comparison is *refused*,
    /// too wide and a genuinely-fractional page loses its last pixel of canvas.
    #[test]
    fn a_page_size_a_hair_above_a_whole_pixel_is_snapped_but_a_real_fraction_is_not() {
        let scale = 150.0 / 72.0;
        let width = 612i64;
        // (height in points, expected pixels) — the points are chosen so the height at 150
        // DPI lands on the stated pixel value.
        for (points, expect) in [
            // A hair over a whole pixel: the file's own decimal error.
            (734.400024f64, 1530usize),
            // Exactly whole.
            (734.4, 1530),
            // A real fraction of a pixel: ceiled, not snapped away.
            (734.424, 1531),
            (734.43, 1531),
        ] {
            let doc = Document::open(
                rect_page_fractional_bytes(width, points),
                mangle_syntax::OpenOptions::default(),
            )
            .expect("the file opens");
            let page = page_of(&doc);
            let render = render_page(
                &doc,
                &page,
                &Resources::default(),
                RenderOptions {
                    scale,
                    ..RenderOptions::default()
                },
            );
            assert_eq!(
                render.image.height,
                expect,
                "a page {points} points tall is {expect} pixels at 150 DPI, but the buffer is {} \
                 rows. Its exact size is {} pixels.",
                render.image.height,
                points * scale
            );
        }
    }

    /// An annotation's appearance stream is drawn, on top of the text it annotates.
    ///
    /// The geometry is the whole of the test: the appearance's `/BBox` is mapped onto the
    /// annotation's `/Rect`, and **not** onto the page. A `/BBox` of `[0 0 1 1]` mapped onto a
    /// 200-unit rectangle is the difference between a highlight the width of its line and a
    /// highlight a hair wide, and the mistake is invisible until you look for it.
    #[test]
    fn an_annotations_appearance_is_drawn_over_the_page() {
        // A highlight whose appearance box is the unit square, so only a renderer that maps
        // the box onto the rectangle can put ink where the rectangle is.
        let page = highlight_page(
            "/AP << /N 5 0 R >>",
            &appearance(0.0, 0.0, 1.0, 1.0, "1 1 0 rg 0 0 1 1 re f"),
        );
        let doc = Document::open(page, mangle_syntax::OpenOptions::default()).expect("opens");
        let page = page_of(&doc);
        let render = render_page(
            &doc,
            &page,
            &Resources::default(),
            RenderOptions {
                scale: 1.0,
                ..RenderOptions::default()
            },
        );
        // Inside the rectangle. The canvas counts down and the page counts up, so page y
        // 600 to 620 is device y 792-620 to 792-600, and 182 is the middle of that.
        assert_eq!(
            render.image.get(200, 182),
            Some([255, 255, 0, 255]),
            "the highlight paints its rectangle: {:?}",
            render.notes
        );
        // Outside it, the same colour must not appear: a box mapped onto the page instead of
        // the rectangle would put a 1x1 pixel somewhere else entirely, or nowhere.
        assert_eq!(
            render.image.get(200, 400),
            Some([255, 255, 255, 255]),
            "and nothing outside the rectangle, which is page y 392: {:?}",
            render.notes
        );
        assert!(
            render.notes.is_empty(),
            "an annotation that draws is not reported as undrawn: {:?}",
            render.notes
        );
    }

    /// A hidden annotation is not drawn, and its absence is not a finding.
    ///
    /// `/F` bit 2 is Hidden and bit 6 is NoView. Both mean the file says this ink is not on the
    /// displayed page, so drawing it would put on the page something the file denies.
    #[test]
    fn a_hidden_annotation_is_not_drawn_and_is_not_a_note() {
        for (flags, why) in [(2, "/F 2 is Hidden"), (32, "/F 32 is NoView")] {
            let page = highlight_page(
                &format!("/F {flags} /AP << /N 5 0 R >>"),
                &appearance(0.0, 0.0, 1.0, 1.0, "1 1 0 rg 0 0 1 1 re f"),
            );
            let doc = Document::open(page, mangle_syntax::OpenOptions::default()).expect("opens");
            let page = page_of(&doc);
            let render = render_page(
                &doc,
                &page,
                &Resources::default(),
                RenderOptions {
                    scale: 1.0,
                    ..RenderOptions::default()
                },
            );
            assert_eq!(
                render.image.get(200, 182),
                Some([255, 255, 255, 255]),
                "{why}, so the highlight is not on the page: {:?}",
                render.notes
            );
        }
    }

    /// An annotation with no `/AP` is reported by name rather than invented.
    ///
    /// A `/Highlight` with no appearance is a coloured rectangle *by convention*, and drawing one
    /// would put colour on the page that the file never asked for. The note is what says the
    /// rectangle is missing rather than absent.
    #[test]
    fn an_annotation_with_no_appearance_is_reported_and_not_invented() {
        let page = page_with_annotations(
            "/Subtype /Highlight /Rect [100 600 300 620] /C [1 1 0] \
             /QuadPoints [100 600 300 600 100 620 300 620]",
            "",
        );
        let doc = Document::open(page, mangle_syntax::OpenOptions::default()).expect("opens");
        let page = page_of(&doc);
        let render = render_page(
            &doc,
            &page,
            &Resources::default(),
            RenderOptions {
                scale: 1.0,
                ..RenderOptions::default()
            },
        );
        assert_eq!(
            render.image.get(200, 182),
            Some([255, 255, 255, 255]),
            "nothing is drawn in its place: {:?}",
            render.notes
        );
        assert!(
            render
                .notes
                .iter()
                .any(|n| n.contains("carries no /AP") && n.contains("was not invented")),
            "and the note says why, rather than leaving a blank where a highlight should be: {:?}",
            render.notes
        );
    }

    /// An annotation whose `/AP` is still encoded is reported rather than read as operators.
    ///
    /// A filter this project does not implement is the case that matters: the bytes are still
    /// compressed, and handing them to the operator table would draw whatever the compressed
    /// stream happens to contain as a page of marks.
    #[test]
    fn an_encoded_annotation_appearance_is_reported_rather_than_executed() {
        let page = highlight_page(
            "/AP << /N 5 0 R >>",
            &appearance_with_filter(
                0.0,
                0.0,
                1.0,
                1.0,
                "/Filter /LZWDecodeX",
                "1 1 0 rg 0 0 1 1 re f",
            ),
        );
        let doc = Document::open(page, mangle_syntax::OpenOptions::default()).expect("opens");
        let page = page_of(&doc);
        let render = render_page(
            &doc,
            &page,
            &Resources::default(),
            RenderOptions {
                scale: 1.0,
                ..RenderOptions::default()
            },
        );
        assert!(
            render.notes.iter().any(|n| n.contains("still encoded")),
            "the note says the appearance was not read as operators: {:?}",
            render.notes
        );
    }

    /// A page whose only annotation is a `/Link` draws no ink and reports nothing.
    ///
    /// A link has no appearance of its own, so "nothing" is what the file said — and a note
    /// about every link on every page would be noise that trains a reader to ignore notes.
    #[test]
    fn a_link_with_no_appearance_is_silent() {
        let page =
            page_with_annotations("/Subtype /Link /Rect [100 600 300 620] /Border [0 0 0]", "");
        let doc = Document::open(page, mangle_syntax::OpenOptions::default()).expect("opens");
        let page = page_of(&doc);
        let render = render_page(
            &doc,
            &page,
            &Resources::default(),
            RenderOptions {
                scale: 1.0,
                ..RenderOptions::default()
            },
        );
        assert_eq!(render.image.get(200, 182), Some([255, 255, 255, 255]));
        assert!(
            render.notes.is_empty(),
            "a link that draws nothing is what the file said: {:?}",
            render.notes
        );
    }

    /// A page whose size in points is a whole number of pixels stays that number of pixels.
    ///
    /// This is the other half of `a_page_buffer_rounds_up_so_the_edge_is_not_clipped`, and it
    /// is the half that was wrong. A US-Letter page is 612 by 792 points, which at 150 DPI is
    /// 1275 by 1650 pixels — not 1651. `792.0 * (150.0 / 72.0)` is not 1650 in binary
    /// floating point; it is `1650.0000000000002`, one unit in the last place above 1650, and
    /// `ceil()` turns that into 1651. The same happens to a 960-point side, which is
    /// `2000.0000000000002` and became 2001.
    ///
    /// The error is invisible at small sizes, because one ulp of 208.33 is far too small to
    /// reach the next integer, which is why a 100-point page rounds up correctly and a
    /// Letter page does not. It is only visible on pages big enough that one ulp is
    /// comparable to a pixel boundary — which is to say, on the page size most documents in
    /// the world use.
    ///
    /// Both numbers here were checked against `mutool draw -r 150`, which produces 1275x1650
    /// and 1650x1275 for these two page sizes. A size disagreement is not a rendering
    /// difference: it suppresses the comparison entirely, which is how 80 pages of the wild
    /// corpus ended up with no measurement at all.
    #[test]
    fn a_whole_number_of_pixels_is_not_rounded_up_to_one_more() {
        let scale = 150.0 / 72.0;
        for (w, h, expect_w, expect_h) in [
            (612, 792, 1275, 1650),
            (792, 612, 1650, 1275),
            (960, 540, 2000, 1125),
            (1224, 1584, 2550, 3300),
        ] {
            let doc = Document::open(rect_page_bytes(w, h), mangle_syntax::OpenOptions::default())
                .expect("the file opens");
            let page = page_of(&doc);
            let render = render_page(
                &doc,
                &page,
                &Resources::default(),
                RenderOptions {
                    scale,
                    ..RenderOptions::default()
                },
            );
            assert_eq!(
                (render.image.width, render.image.height),
                (expect_w, expect_h),
                "{w} by {h} points at 150 DPI is {expect_w} by {expect_h} pixels, but the \
                 buffer is {} by {}. 792.0 * (150.0/72.0) is 1650.0000000000002 and ceil() \
                 makes that 1651.",
                render.image.width,
                render.image.height
            );
        }
    }

    /// A fractional page size still rounds up, and the whole-number rule did not take it.
    ///
    /// The epsilon that fixes the Letter page must not be wide enough to swallow a real
    /// fraction. 100 points at 150 DPI is 208.33 pixels and has to become 209, or the last
    /// third of a pixel is clipped off the page — which is the case the ceiling exists for.
    /// 595x842 (A4) is 1239.58 by 1754.17 and has the same obligation on its long side.
    #[test]
    fn a_fractional_page_size_still_rounds_up() {
        let scale = 150.0 / 72.0;
        for (w, h, expect_w, expect_h) in [
            (100, 100, 209, 209),
            (595, 842, 1240, 1755),
            (288, 288, 600, 600),
        ] {
            let doc = Document::open(rect_page_bytes(w, h), mangle_syntax::OpenOptions::default())
                .expect("the file opens");
            let page = page_of(&doc);
            let render = render_page(
                &doc,
                &page,
                &Resources::default(),
                RenderOptions {
                    scale,
                    ..RenderOptions::default()
                },
            );
            assert_eq!(
                (render.image.width, render.image.height),
                (expect_w, expect_h),
                "{w} by {h} points at 150 DPI rounds up to {expect_w} by {expect_h} pixels, \
                 but the buffer is {} by {}",
                render.image.width,
                render.image.height
            );
        }
    }

    /// A page's own corner lands on the canvas's corner, at every zoom.
    ///
    /// The assertion a page fitted into a canvas cannot fail unless the zoom is applied
    /// twice: if the page is drawn at the square of the scale it is bigger than the canvas
    /// it was fitted into, and its corners are outside it. A fixture whose content is
    /// symmetric about the centre of the page — a shape in each quadrant, a clip down the
    /// middle — hides this completely, because a centred, over-large page still puts each
    /// quadrant in a quadrant. So this checks the corners, which do not move.
    #[test]
    fn the_pages_corners_land_on_the_canvases_corners() {
        for scale in [0.5, 1.0, 2.0, 4.0, 150.0 / 72.0] {
            let doc = Document::open(page_bytes(100), mangle_syntax::OpenOptions::default())
                .expect("the file opens");
            let page = page_of_size(100);
            let render = render_page(
                &doc,
                &page,
                &Resources::default(),
                RenderOptions {
                    scale,
                    ..RenderOptions::default()
                },
            );
            let (w, h) = (render.image.width as f64, render.image.height as f64);
            // The page is square and the canvas is square, so it fills it exactly.
            for (fx, fy, which) in [(0.0, 0.0, "top left"), (1.0, 1.0, "bottom right")] {
                let x = ((w - 1.0) * fx).round() as usize;
                let y = ((h - 1.0) * fy).round() as usize;
                let got = render.image.get(x, y);
                assert_eq!(
                    got.map(|p| p[0]),
                    Some(255),
                    "the {which} corner of a blank page is paper at scale {scale}, \
                     image {}x{scale}",
                    render.image.width
                );
            }
            // And the page occupies the canvas rather than overflowing it, which is the
            // property a centred over-large page also satisfies only by accident. The
            // margin is what tells them apart: a page fitted with the zoom applied twice is
            // twice as big as the canvas and has no margin at all.
            assert!(
                (w - h).abs() < 1.0,
                "a square page makes a square canvas at scale {scale}, got {w} by {h}"
            );
        }
    }

    /// A one-page file of the given size in points, with no content, as bytes.
    fn page_bytes(points: i64) -> Vec<u8> {
        rect_page_bytes(points, points)
    }

    /// A one-page file of the given width and height in points, with no content, as bytes.
    /// A form XObject appearance stream with the given box, no filter.
    fn appearance(x0: f64, y0: f64, x1: f64, y1: f64, content: &str) -> String {
        appearance_with_filter(x0, y0, x1, y1, "", content)
    }

    /// The same, with a `/Filter`, for the case where the bytes must not reach the interpreter.
    fn appearance_with_filter(
        x0: f64,
        y0: f64,
        x1: f64,
        y1: f64,
        filter: &str,
        content: &str,
    ) -> String {
        format!(
            "<< /Type /XObject /Subtype /Form /FormType 1 /BBox [{x0} {y0} {x1} {y1}] \
             /Resources << >> {filter} /Length {} >>\nstream\n{content}\nendstream\nendobj\n",
            content.len()
        )
    }

    /// A one-page 612x792 document whose `/Annots` holds one annotation with one appearance.
    ///
    /// Offsets are recorded as the objects are written rather than computed afterwards: a
    /// hand-built cross-reference table whose entries are arithmetic on string lengths is wrong
    /// the moment a dictionary is one character longer, and the symptom is an object that quietly
    /// does not resolve.
    fn page_with_annotations(annot_extra: &str, appearance_body: &str) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        let mut offsets: Vec<usize> = Vec::new();
        out.extend_from_slice(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n");
        offsets.push(out.len());
        out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        offsets.push(out.len());
        out.extend_from_slice(
            b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>\nendobj\n",
        );
        offsets.push(out.len());
        out.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << >> /Annots [4 0 R] >>\nendobj\n",
        );
        offsets.push(out.len());
        out.extend_from_slice(format!("4 0 obj\n<< {annot_extra} >>\nendobj\n").as_bytes());
        offsets.push(out.len());
        out.extend_from_slice(format!("5 0 obj\n{appearance_body}endobj\n").as_bytes());

        let xref = out.len();
        out.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
        for offset in &offsets {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
        );
        out
    }

    /// A page carrying one `/Highlight` whose appearance fills its own box.
    fn highlight_page(annot_extra: &str, appearance_body: &str) -> Vec<u8> {
        page_with_annotations(
            &format!("/Subtype /Highlight /Rect [100 600 300 620] {annot_extra}"),
            appearance_body,
        )
    }

    /// The same, with a height that is not a whole number of points    /// The same, with a height that is not a whole number of points — which is the whole
    /// point, since a producer writing a `/MediaBox` as a decimal does not round it.
    fn rect_page_fractional_bytes(points_w: i64, points_h: f64) -> Vec<u8> {
        let mut body: Vec<u8> = Vec::new();
        body.extend_from_slice(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n");
        let mut at = [0usize; 4];
        at[1] = body.len();
        body.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        at[2] = body.len();
        body.extend_from_slice(
            format!(
                "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 {points_w} \
                 {points_h}] >>\nendobj\n"
            )
            .as_bytes(),
        );
        at[3] = body.len();
        body.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n");
        let xref = body.len();
        body.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n1 3\n");
        for offset in at.iter().take(4).skip(1) {
            body.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        body.extend_from_slice(
            format!("trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
        );
        body
    }

    fn rect_page_bytes(points_w: i64, points_h: i64) -> Vec<u8> {
        let mut body: Vec<u8> = Vec::new();
        body.extend_from_slice(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n");
        let mut at = [0usize; 4];
        at[1] = body.len();
        body.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        at[2] = body.len();
        body.extend_from_slice(
            format!(
                "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 {points_w} \
                 {points_h}] >>\nendobj\n"
            )
            .as_bytes(),
        );
        at[3] = body.len();
        body.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n");
        let xref = body.len();
        body.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n1 3\n");
        for offset in at.iter().take(4).skip(1) {
            body.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        body.extend_from_slice(
            format!("trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
        );
        body
    }

    /// The first page of a document.
    fn page_of(doc: &Document) -> Page {
        let catalog = doc.catalog().expect("a catalogue");
        let root = catalog
            .get("Pages")
            .and_then(Object::as_ref_id)
            .expect("the page tree");
        mangle_doc::PageTree::build(doc, root)
            .expect("a page tree")
            .pages()
            .first()
            .cloned()
            .expect("a page")
    }

    /// A one-page file of the given size in points, filled black and clipped to its left
    /// half. A clip that is not transformed clips at the unscaled position, which is the
    /// whole of the bug this fixture exists for.
    fn clip_page(points: i64) -> Vec<u8> {
        let half = points / 2;
        let content = format!("0 0 0 rg 0 0 {half} {points} re W n 0 0 {points} {points} re f");
        let mut body: Vec<u8> = Vec::new();
        body.extend_from_slice(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n");
        let mut at = [0usize; 5];
        at[1] = body.len();
        body.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        at[2] = body.len();
        body.extend_from_slice(
            format!(
                "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 {points} {points}] >>\nendobj\n"
            )
            .as_bytes(),
        );
        at[3] = body.len();
        body.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n",
        );
        at[4] = body.len();
        body.extend_from_slice(
            format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        );
        body.extend_from_slice(content.as_bytes());
        body.extend_from_slice(b"\nendstream\nendobj\n");
        let xref = body.len();
        body.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n1 4\n");
        for offset in at.iter().take(5).skip(1) {
            body.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        body.extend_from_slice(
            format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
        );
        body
    }

    /// The one page of a file of the given size in points.
    fn page_of_size(points: i64) -> Page {
        let doc = Document::open(page_bytes(points), mangle_syntax::OpenOptions::default())
            .expect("the file opens");
        let catalog = doc.catalog().expect("a catalogue");
        let root = catalog
            .get("Pages")
            .and_then(Object::as_ref_id)
            .expect("the page tree");
        mangle_doc::PageTree::build(&doc, root)
            .expect("a page tree")
            .pages()
            .first()
            .cloned()
            .expect("a page")
    }

    /// A clip has to be transformed like everything else. This is invisible at scale one,
    /// where user space and device space are the same, and wrong at every other scale.
    #[test]
    fn a_clip_scales_with_the_page() {
        // A 100 by 100 point page, filled black, clipped to its left half.
        let pdf = clip_page(100);
        let doc =
            Document::open(pdf, mangle_syntax::OpenOptions::default()).expect("the file opens");
        let page = page_of(&doc);
        let resources = Resources::default();
        for scale in [1.0, 150.0 / 72.0] {
            let render = render_page(
                &doc,
                &page,
                &resources,
                RenderOptions {
                    scale,
                    ..RenderOptions::default()
                },
            );
            let image = &render.image;
            let w = image.width as f64;
            let h = image.height as f64;
            // The left half is inside the clip and black; the right half is paper.
            let inside = image.get((w * 0.25) as usize, (h * 0.5) as usize);
            let outside = image.get((w * 0.75) as usize, (h * 0.5) as usize);
            assert_eq!(
                inside.map(|p| p[0]),
                Some(0),
                "inside the clip is black at scale {scale}, image {}x{}",
                image.width,
                image.height
            );
            assert_eq!(
                outside.map(|p| p[0]),
                Some(255),
                "outside the clip is paper at scale {scale}, image {}x{}",
                image.width,
                image.height
            );
        }
    }

    // ── Tiling patterns: one cell, and how many of it there are ──────────────────

    /// The content stream every pattern below carries as its cell. Written as bytes here
    /// because the cell is what a pattern streams, not what it says about itself.
    const CELL: &[u8] = b"1 0 0 rg 0 0 20 20 re f";

    /// A tiling pattern stream carrying exactly the entries named, and nothing else.
    ///
    /// Built as an object rather than as a file so that what each test adds is visible in the
    /// test: a fixture that also supplied `/XStep` to the case about a missing `/XStep` would
    /// be testing the fixture.
    fn tiling(entries: &[(&str, Object)]) -> Object {
        let mut dict = Dict::new();
        for (key, value) in entries {
            dict.set(key, value.clone());
        }
        Object::Stream(mangle_syntax::Stream::new(dict, CELL.to_vec()))
    }

    /// A resolver for a pattern whose entries are all direct, which is how a file writes them
    /// and what these tests write: every object stands for itself.
    fn direct(object: &Object) -> Option<Object> {
        Some(object.clone())
    }

    /// `[a b c d]`, as a file writes a rectangle or a matrix.
    fn numbers(values: &[f64]) -> Object {
        Object::Array(values.iter().copied().map(Object::Real).collect())
    }

    /// Everything a tiling pattern says, read back. A cell that is not clipped to its own
    /// `/BBox`, stepped by its own pitch, in its own space, with its own paint type, is not
    /// the file's cell, and each field is asserted because each is read from somewhere else.
    #[test]
    fn a_tiling_pattern_with_every_entry_reads_every_entry() {
        let pattern = tiling(&[
            ("BBox", numbers(&[1.0, 2.0, 21.0, 22.0])),
            ("XStep", Object::Real(20.0)),
            ("YStep", Object::Int(30)),
            ("Matrix", numbers(&[2.0, 0.0, 0.0, 2.0, 5.0, 7.0])),
            ("PaintType", Object::Int(2)),
        ]);
        let read = tiling_pattern("P0", &pattern, &direct)
            .expect("a tiling pattern with every entry is a tiling pattern");
        assert_eq!(
            read.bbox,
            PageRect::new(1.0, 2.0, 21.0, 22.0),
            "the cell is clipped to its /BBox, in pattern space"
        );
        assert_eq!(read.x_step, 20.0, "the cells step twenty along x");
        assert_eq!(read.y_step, 30.0, "and thirty along y");
        assert_eq!(
            read.matrix,
            Matrix::new(2.0, 0.0, 0.0, 2.0, 5.0, 7.0),
            "the pattern's own space is kept as written"
        );
        assert_eq!(
            read.paint_type,
            PaintType::Uncoloured,
            "/PaintType 2 is a cell that takes the paint operator's colour"
        );
        let Object::Stream(cell) = &read.content else {
            panic!("the cell is the pattern's content stream");
        };
        assert_eq!(
            cell.raw, CELL,
            "the cell's bytes come through as they were stored"
        );
    }

    /// **A zero step is a value and not a missing one.** It means the cell is not repeated in
    /// that direction — drawn once, at the pattern-space origin — so a read that folds zero in
    /// with "the entry is not there" loses the pattern rather than drawing it.
    #[test]
    fn a_zero_step_is_read_as_zero_and_not_as_missing() {
        let pattern = tiling(&[
            ("BBox", numbers(&[0.0, 0.0, 10.0, 10.0])),
            ("XStep", Object::Int(0)),
            ("YStep", Object::Real(0.0)),
        ]);
        let read =
            tiling_pattern("P0", &pattern, &direct).expect("a zero step is a value, not a fault");
        assert_eq!(read.x_step, 0.0, "an explicit /XStep 0 is zero");
        assert_eq!(read.y_step, 0.0, "and an explicit /YStep 0 is zero too");

        // One axis not tiled and the other tiled is a whole pattern in itself: a stripe or a
        // row of motifs. Reading the two as one pitch loses it entirely.
        let striped = tiling(&[
            ("BBox", numbers(&[0.0, 0.0, 10.0, 10.0])),
            ("XStep", Object::Int(0)),
            ("YStep", Object::Int(12)),
        ]);
        let read =
            tiling_pattern("P0", &striped, &direct).expect("a pattern may repeat along y only");
        assert_eq!(
            read.x_step, 0.0,
            "the x step is still the zero the file wrote"
        );
        assert_eq!(
            read.y_step, 12.0,
            "and the y step is not dragged down with it"
        );
    }

    /// A negative step is legal: the cells run the other way. Clamping it to zero would draw
    /// one cell and report a fault, where the file asked for a picture.
    #[test]
    fn a_negative_step_survives_the_read() {
        let pattern = tiling(&[
            ("BBox", numbers(&[0.0, 0.0, 10.0, 10.0])),
            ("XStep", Object::Real(-12.5)),
            ("YStep", Object::Int(-4)),
        ]);
        let read =
            tiling_pattern("P0", &pattern, &direct).expect("a negative step is legal and reads");
        assert_eq!(read.x_step, -12.5, "the x step keeps its sign");
        assert_eq!(read.y_step, -4.0, "and so does the y step");
    }

    /// Absent is not broken. Every one of these entries has a specified default, and a
    /// pattern that relies on them — a cell in the space it was set in, repeated nowhere,
    /// carrying its own colour — is ordinary rather than damaged.
    #[test]
    fn an_absent_optional_entry_takes_its_default() {
        let pattern = tiling(&[("BBox", numbers(&[0.0, 0.0, 12.0, 12.0]))]);
        let read = tiling_pattern("P0", &pattern, &direct)
            .expect("a pattern with nothing but a /BBox is a pattern");
        assert_eq!(read.x_step, 0.0, "no /XStep means no repeat along x");
        assert_eq!(read.y_step, 0.0, "no /YStep means no repeat along y");
        assert_eq!(
            read.matrix,
            Matrix::IDENTITY,
            "no /Matrix means the cell is already in the space the pattern was set in"
        );
        assert_eq!(
            read.paint_type,
            PaintType::Coloured,
            "no /PaintType means 1, a cell that carries its own colours"
        );
    }

    /// A cell with no `/BBox` has no clip, and there is no default clip: inventing one would
    /// draw a plausible texture that belongs to nobody. The error names the pattern because a
    /// page can name thirty of them and the reader is looking for one of them.
    #[test]
    fn a_tiling_pattern_with_no_bbox_is_an_error_that_names_it() {
        let pattern = tiling(&[("XStep", Object::Int(10)), ("YStep", Object::Int(10))]);
        let err = tiling_pattern("P0", &pattern, &direct)
            .expect_err("a missing /BBox is refused rather than defaulted");
        assert!(err.contains("/P0"), "the error names the pattern: {err}");
        assert!(err.contains("/BBox"), "and the entry at fault: {err}");
    }

    /// A `/BBox` that is there but is not four numbers is the same fault as a missing one:
    /// there is no clip to draw the cell inside. `Rect::from_object` would have filled the
    /// gaps with zeros and produced a cell the file never described.
    #[test]
    fn a_bbox_that_is_not_four_numbers_is_an_error_that_names_it() {
        for bbox in [Object::name("Square"), numbers(&[0.0, 0.0, 20.0])] {
            let pattern = tiling(&[("BBox", bbox.clone())]);
            let err = tiling_pattern("P0", &pattern, &direct)
                .expect_err("a /BBox of {bbox:?} is not a clip");
            assert!(err.contains("/P0"), "the error names the pattern: {err}");
            assert!(err.contains("/BBox"), "and the entry at fault: {err}");
        }
    }

    /// A step that is present and is not a number is a file that says where the cells go and
    /// does not say it. The error has to quote the field, because `/XStep` and `/YStep` are
    /// two entries and knowing which one is wrong is most of the answer.
    #[test]
    fn a_step_that_is_not_a_number_is_an_error_that_names_the_field() {
        let pattern = tiling(&[
            ("BBox", numbers(&[0.0, 0.0, 10.0, 10.0])),
            ("XStep", Object::name("Wide")),
        ]);
        let err = tiling_pattern("P0", &pattern, &direct)
            .expect_err("a /XStep that is not a number is refused rather than defaulted");
        assert!(err.contains("XStep"), "the error names the field: {err}");
        assert!(err.contains("/P0"), "and the pattern: {err}");

        let other = tiling(&[
            ("BBox", numbers(&[0.0, 0.0, 10.0, 10.0])),
            ("YStep", Object::name("Tall")),
        ]);
        let err = tiling_pattern("P0", &other, &direct)
            .expect_err("a /YStep that is not a number is refused too");
        assert!(
            err.contains("YStep"),
            "the y field is named as itself: {err}"
        );
        assert!(!err.contains("XStep"), "and not as the other one: {err}");
    }

    /// A tiling pattern's cell *is* its content stream, so a pattern resource that is a plain
    /// dictionary has nothing to tile however complete the rest of its entries are. It is
    /// refused by name rather than drawn as an empty cell, which would look like a page that
    /// meant to paint nothing there.
    #[test]
    fn a_tiling_pattern_with_no_stream_behind_it_is_an_error() {
        let mut dict = Dict::new();
        dict.set("BBox", numbers(&[0.0, 0.0, 10.0, 10.0]));
        let err = tiling_pattern("P0", &Object::Dict(dict), &direct)
            .expect_err("a dictionary is not a cell");
        assert!(err.contains("/P0"), "the error names the pattern: {err}");
        assert!(err.contains("stream"), "and says what is missing: {err}");
    }

    /// A Type 3 font is refused by name, and the regression it pins is a **substitution**.
    ///
    /// This is not "we have no Type 3 support" being restated. The bug was that a Type 3 font
    /// reached `substitute_for`, which asks only what the `/BaseFont` is called — and a Type 3
    /// font may be called anything. Both pdfjs regression files in the corpus name theirs
    /// `/Helvetica`, which has a bundled stand-in, so the substitution **succeeded** and the
    /// page drew Helvetica's glyphs while the document defined each glyph as a content
    /// stream. A plausible wrong answer is worse than a named gap, and that is what this
    /// asserts is gone.
    #[test]
    fn a_type3_font_is_refused_rather_than_substituted() {
        let doc =
            Document::open(page_bytes(1), mangle_syntax::OpenOptions::default()).expect("opens");
        let mut font = Dict::new();
        font.set("Type", Object::name("Font"));
        font.set("Subtype", Object::name("Type3"));
        // The name that made the substitution succeed. Asserted deliberately: it is the whole
        // point of the test, and a name with no stand-in would pass without this fix at all.
        font.set("BaseFont", Object::name("Helvetica"));
        font.set(
            "FontMatrix",
            Object::Array(vec![
                Object::Real(0.01),
                Object::Real(0.0),
                Object::Real(0.0),
                Object::Real(0.01),
                Object::Real(0.0),
                Object::Real(0.0),
            ]),
        );
        let mut charprocs = Dict::new();
        charprocs.set("square", Object::name("square"));
        font.set("CharProcs", Object::Dict(charprocs));
        let mut font_table = Dict::new();
        font_table.set("T3", Object::Dict(font));
        let mut resources_dict = Dict::new();
        resources_dict.set("Font", Object::Dict(font_table));
        let resources = Resources::from_dict(&resources_dict, &|o| Some(o.clone()));

        let reason = font_for("T3", &resources, &doc).expect_err("a Type 3 font is refused");
        assert!(
            reason.contains("Type 3"),
            "the reason must name the kind of font, so a reader does not go looking for an \
             embedding that was never supposed to be there: {reason}"
        );
        assert!(
            !reason.contains("metric-compatible"),
            "and it must not have been answered with a substitution: {reason}"
        );
    }
}
