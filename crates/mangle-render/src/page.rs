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
    ContentStream, FillRule as ContentRule, Mark, Matrix, PathSegment, Resources, run_with,
};
use mangle_doc::Page;
use mangle_syntax::{Document, Object, Rect as PageRect, stream::decode_stream};

use crate::image::{self, Raster};
use crate::shading::{self, Shading};
use crate::{
    ClipRegion, ClipShape, Device, FillRule, Image, LineCap, LineJoin, Polygon, Rect, StrokeStyle,
    Viewport, transform_path,
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
    // Rounded *up*, not to nearest. A page of 208.33 pixels needs 209 rows to hold its last
    // third of a pixel, and rounding down clips it; rounding up also matches what the
    // comparison oracles do, which is what makes a page-by-page comparison possible at all
    // rather than merely usually possible.
    let size = (
        (pixels_w * shrink).ceil().max(1.0) as usize,
        (pixels_h * shrink).ceil().max(1.0) as usize,
    );

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
    // The image lookup borrows the page's resources and the document and nothing else. It
    // must not capture the notes: they are passed into `draw_mark` on the next line, and a
    // second live borrow of them would not compile. The failure reason comes back as the
    // closure's `Err` and is appended by `draw_mark` itself.
    let mut lookup = |name: &str| image_for(name, resources, doc);
    let mut shade_lookup = |name: &str| shading_for(name, resources, doc);
    // The font lookup is beside the other two rather than inside `draw_mark`, for the same
    // reason: the document and the page's resources are borrowed here, once, and the
    // drawing code only ever sees a name.
    let mut font_lookup = |name: &str| font_for(name, resources, doc);
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
        draw_mark(
            &mut device,
            &record.mark,
            &to_device,
            &placement.matrix,
            record,
            &mut render.notes,
            &mut lookup,
            &mut shade_lookup,
            &mut font_lookup,
        );
        render.marks += 1;
    }
    render.image = device.into_image();
    render
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
    let Some(object) = resources.patterns.get(name).cloned() else {
        return Err(format!(
            "the page names a shading `/{name}` that its resources do not define"
        ));
    };
    let resolved = doc.resolve_object(&object).unwrap_or(object);
    // The pattern's own transformation maps its space into the page's default space, which
    // the mark's transformation then carries to the pixels. Both are needed and neither
    // substitutes for the other.
    let (shading_object, pattern_matrix) = match &resolved {
        Object::Stream(s) => (
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
                doc.resolve_object(inner).unwrap_or_else(|| inner.clone()),
                read_matrix(d.get("Matrix")),
            ),
            None => (resolved.clone(), Matrix::IDENTITY),
        },
        _ => {
            return Err(format!(
                "`/{name}` is named by `sh` but is neither a dictionary nor a stream"
            ));
        }
    };
    let resolver = |o: &Object| doc.resolve_object(o);
    let shading = Shading::parse(&shading_object, &resolver)
        .ok_or_else(|| format!("the shading `/{name}` is of a kind this does not paint yet"))?;
    Ok((shading, pattern_matrix))
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

/// Find and decode the image a `Do` names.
///
/// A page's resource table holds the XObjects by name, and an XObject that is not an image
/// is a note rather than a failure: a form XObject is named by `Do` too, and reaching one is
/// a later step than this.
fn image_for(name: &str, resources: &Resources, doc: &Document) -> Result<Raster, String> {
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
    image::decode(&stream, &resolver, &mut notes).ok_or_else(|| {
        if notes.is_empty() {
            format!("the image `/{name}` could not be decoded")
        } else {
            notes.join("; ")
        }
    })
}

/// A shading lookup: a name to the shading it names, and the pattern's own transformation.
///
/// Passed into `draw_mark` rather than resolved there so that the borrowing stays with
/// whoever owns the document and the page's resources.
type ShadingLookup<'a> = &'a mut dyn FnMut(&str) -> Result<(Shading, Matrix), String>;

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
}

impl std::fmt::Debug for FontProgram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontProgram")
            .field("bytes", &self.program.bytes().len())
            .field("units_per_em", &self.units_per_em)
            .field("encoding", &self.encoding.base().name())
            .field("differences", &self.encoding.differences().len())
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
    let Object::Dict(descriptor) = descriptor else {
        return Err(format!(
            "the font `/{name}` has no `/FontDescriptor`, so there is nothing to read a \
             font program from"
        ));
    };
    let key = ["FontFile3", "FontFile2", "FontFile"]
        .into_iter()
        .find(|key| descriptor.get(key).is_some());
    let Some(key) = key else {
        return Err(format!(
            "the font `/{name}` is not embedded, so it has no outlines of its own to draw"
        ));
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
    // The `/Encoding` is on the *font* dictionary, not the descendant's, and a composite
    // font's is a CMap rather than any of the four shapes an encoding takes — so it is only
    // read for a simple font, where the code really is a character code. A composite font's
    // code is a CID and `/Differences` does not apply to it, which is why that font keeps the
    // path it had rather than having an encoding forced onto a number that is not one.
    //
    // The default base is the standard encoding: a simple font that names no `/Encoding` is
    // addressed through its own built-in encoding where it carries one, and through the
    // standard encoding where it does not, which is what `Type1::glyph_for_code` already does.
    // Reading it here as well means a font that *does* declare one gets it, and one that does
    // not is unaffected.
    let encoding = if composite {
        mangle_font::Encoding::new(mangle_font::EncodingBase::Standard)
    } else {
        mangle_font::Encoding::from_font_dict(&font, mangle_font::EncodingBase::Standard, &|o| {
            doc.resolve_object(o).or_else(|| Some(o.clone()))
        })
        .unwrap_or_else(|| mangle_font::Encoding::new(mangle_font::EncodingBase::Standard))
    };
    Ok(FontProgram {
        program,
        units_per_em,
        encoding,
    })
}

/// A font lookup: a font resource name to the outlines it names.
///
/// Passed into `draw_mark` for the same reason as the image lookup, and with the same
/// shape, so that the document and the page's resources stay borrowed by whoever owns
/// them rather than by a function that has to reach into both.
type FontLookup<'a> = &'a mut dyn FnMut(&str) -> Result<FontProgram, String>;

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
fn draw_mark(
    device: &mut Device,
    mark: &Mark,
    to_device: &Matrix,
    placement: &Matrix,
    record: &mangle_content::Record,
    notes: &mut Vec<String>,
    images: &mut dyn FnMut(&str) -> Result<Raster, String>,
    shadings: ShadingLookup<'_>,
    fonts: FontLookup<'_>,
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
            // The clip is not narrowed to the shape here. `fill_polygon` bounds its own work
            // to the polygon's box intersected with the clip, which is the same answer, and a
            // narrowing kept in the device would outlive this call: the next mark brings its own
            // clip and must not inherit this one's cull.
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
            name => match shadings(name) {
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
        Mark::Image { name, .. } => match name.as_deref() {
            Some(name) => match images(name) {
                Ok(raster) => {
                    // The image's own space is the unit square, so the mark's
                    // transformation is all that is needed to place it. The fill colour is
                    // what paints an image mask's zero bits.
                    let fill = mangle_content::Colour::black().to_rgba(None);
                    image::draw(device, &raster, to_device, record.fill_alpha, fill);
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
            let mut program = match fonts(name) {
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
            let Some(rgba) = fill.to_rgba(None) else {
                notes.push(format!(
                    "the colour text is painted in {} could not be converted, so no text \
                     on the page in that colour was drawn",
                    fill.space.name
                ));
                return;
            };
            let ink = rgba.to_rgba8(record.fill_alpha);
            // `codes` is the character codes and `placements` is one matrix per code, so
            // the two zip; a run whose code has no placement is not drawn, which is what
            // a truncated record means rather than a glyph at the origin. The codes are
            // read from the mark rather than from the string's bytes because a composite
            // font's codes are two bytes each, and the mark is what says so.
            for (code, glyph_matrix) in codes.iter().zip(placements.iter()) {
                // A composite font's code is looked up as a CID — through the (3,0)
                // subtable, or as a glyph number where the font has no such subtable — and
                // a simple font's through its `/Encoding` and then the subtables a simple
                // font uses. Which one applies is the mark's claim, not a guess made here.
                //
                // The two paths are different questions and are kept apart. A CID is an
                // identifier the font was built to be addressed by, and `/Differences` does
                // not apply to one, so no encoding is consulted for it. A character code
                // means nothing until the `/Encoding` turns it into a glyph name, and a page
                // that remapped that code is asking for a different glyph.
                let found = if *two_byte {
                    program.program.outline_for_cid(*code)
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
                device.fill_polygon(&polygon, FillRule::NonZero, ink);
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
        let mut body: Vec<u8> = Vec::new();
        body.extend_from_slice(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n");
        let mut at = [0usize; 4];
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
}
