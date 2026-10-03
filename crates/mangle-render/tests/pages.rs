//! End-to-end rendering, and the comparison with `mutool` that keeps it honest.
//!
//! Two layers of test, deliberately:
//!
//! * **Self-contained.** A file written here, rendered here, and checked against what
//!   the page says should be on it. These run everywhere and are the ones that catch a
//!   regression.
//! * **Oracle.** The same file rendered by `mutool` and compared with ours. `mutool` is a
//!   different codebase with a decade of accumulated knowledge of what a page is meant to
//!   look like; comparing against it is the only check that says something about
//!   *correctness* rather than about internal consistency. It is skipped when `mutool` is
//!   absent, because a test that quietly passes is worse than one that skips.
//!
//! The comparison is deliberately structural rather than exact. Two correct rasterizers
//! disagree about antialiasing at edges and about text hinting, and demanding bit equality
//! would mean demanding a bug. What is compared is the ink: where the dark pixels are, how
//! many there are, and whether a region that must be white is white and one that must be
//! black is black.

#![forbid(unsafe_code)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_cmp
)]
#![allow(clippy::many_single_char_names)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use mangle_content::Resources;
use mangle_doc::PageTree;
use mangle_render::{RenderOptions, SsimOptions, compare, render_page};
use mangle_syntax::{Document, Object, OpenOptions, Rect, Ref, stream::decode_stream};

/// A page with shapes whose positions and colours are known exactly.
///
/// Written here rather than taken from the corpus so that what each pixel *should* be is
/// stated by construction and not by a previous run of this same code.
fn shapes_page() -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut at = [0usize; 5];
    out.extend_from_slice(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n");

    at[1] = out.len();
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    at[2] = out.len();
    out.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>\nendobj\n",
    );
    at[3] = out.len();
    out.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n");

    // A filled black square in the lower-left quadrant, a red square in the upper-left,
    // and a blue square in the upper-right. The canvas is 200 by 200 points at scale one,
    // so each quadrant is exactly 100 by 100 pixels and the boundaries are on integers.
    let content = b"0 0 0 rg 0 0 100 100 re f \
                    1 0 0 rg 0 100 100 100 re f \
                    0 0 1 rg 100 100 100 100 re f";
    at[4] = out.len();
    let mut body = format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).into_bytes();
    body.extend_from_slice(content);
    body.extend_from_slice(b"\nendstream\nendobj\n");
    out.extend_from_slice(&body);

    let xref = out.len();
    out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n1 4\n");
    for offset in at.iter().take(5).skip(1) {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size 5 /Root 1 0 R /ID [<0102> <0304>] >>\nstartxref\n{xref}\n%%EOF\n"
        )
        .as_bytes(),
    );
    out
}

/// A page with a clipped triangle, to exercise the clip against a path rather than a box.
fn clipped_page() -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut at = [0usize; 5];
    out.extend_from_slice(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n");
    at[1] = out.len();
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    at[2] = out.len();
    out.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 100] >>\nendobj\n",
    );
    at[3] = out.len();
    out.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n");
    // The whole page filled black, clipped to its left half. An unclipped renderer paints
    // the entire page; one that clips correctly paints exactly the left half. Nothing else
    // on the page, so any difference between the two halves is the clip and nothing else.
    let content = b"0 0 0 rg 0 0 50 100 re W n 0 0 100 100 re f";
    at[4] = out.len();
    let mut body = format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).into_bytes();
    body.extend_from_slice(content);
    body.extend_from_slice(b"\nendstream\nendobj\n");
    out.extend_from_slice(&body);
    let xref = out.len();
    out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n1 4\n");
    for offset in at.iter().take(5).skip(1) {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size 5 /Root 1 0 R /ID [<0102> <0304>] >>\nstartxref\n{xref}\n%%EOF\n"
        )
        .as_bytes(),
    );
    out
}

/// A page clipped to a diagonal, with content placed to make the comparison mean something.
///
/// **The asymmetry is the point.** Every other fixture here is symmetric about the centre
/// of the page — a shape in each quadrant, a clip down the middle — and a symmetric page
/// scores above 0.99 whether or not the clip is the right shape, because a transform applied
/// twice lands somewhere else and the two somewhere-elses look alike. Nothing here mirrors
/// in either axis: the clip is a triangle whose hypotenuse runs from the lower right to the
/// upper left, the shapes sit in three different corners, and one of them straddles the
/// diagonal so that the clip edge is compared against real ink rather than against paper.
fn diagonal_clip_page() -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut at = [0usize; 5];
    out.extend_from_slice(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n");
    at[1] = out.len();
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    at[2] = out.len();
    out.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>\nendobj\n",
    );
    at[3] = out.len();
    out.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n");
    // The clip is the triangle (0,0) (200,0) (30,200): a diagonal hypotenuse from the lower
    // right to the upper left, and nothing at all above or to the right of it. A renderer
    // that honours only the bounding box paints the whole of that box instead, which is
    // most of the page, so the two cannot be mistaken for one another.
    //
    // Each fill repeats the clip path. That is redundant in a correct renderer and is
    // written this way so that the fixture tests the shape of the clip rather than how long
    // a renderer remembers one.
    let content = b"q 0 0 0 rg 0 0 m 200 0 l 30 200 l h W n 0 0 200 200 re f Q \
                    q 1 0 0 rg 0 0 m 200 0 l 30 200 l h W n 10 10 60 30 re f Q \
                    q 0 0 1 rg 0 0 m 200 0 l 30 200 l h W n 60 120 90 40 re f Q \
                    q 0 153 0 rg 0 0 m 200 0 l 30 200 l h W n 140 150 40 40 re f Q";
    at[4] = out.len();
    let mut body = format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).into_bytes();
    body.extend_from_slice(content);
    body.extend_from_slice(b"\nendstream\nendobj\n");
    out.extend_from_slice(&body);
    let xref = out.len();
    out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n1 4\n");
    for offset in at.iter().take(5).skip(1) {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size 5 /Root 1 0 R /ID [<0102> <0304>] >>\nstartxref\n{xref}\n%%EOF\n"
        )
        .as_bytes(),
    );
    out
}

// ── Four clip fixtures, all asymmetric ─────────────────────────────────────────

/// A one-page square file of `points` points with this content, written here so that what
/// each pixel *should* be is stated by construction rather than by a previous run.
///
/// The clip fixtures below all share this writer. The earlier fixtures each spell out their
/// own, which is the same shape in five copies; rewriting those is not what this change is
/// for.
fn page_with(content: &str, points: i64) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut at = [0usize; 5];
    out.extend_from_slice(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n");
    at[1] = out.len();
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    at[2] = out.len();
    out.extend_from_slice(
        format!(
            "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 {points} \
             {points}] >>\nendobj\n"
        )
        .as_bytes(),
    );
    at[3] = out.len();
    out.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n");
    at[4] = out.len();
    let mut body = format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).into_bytes();
    body.extend_from_slice(content.as_bytes());
    body.extend_from_slice(b"\nendstream\nendobj\n");
    out.extend_from_slice(&body);
    let xref = out.len();
    out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n1 4\n");
    for offset in at.iter().take(5).skip(1) {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size 5 /Root 1 0 R /ID [<0102> <0304>] >>\nstartxref\n{xref}\n%%EOF\n"
        )
        .as_bytes(),
    );
    out
}

/// A 200 point square page with two marks under one clip.
///
/// The content stream is the most ordinary thing a page can do: clip once, then draw twice.
/// Nothing here is a trick, and that is the point — a clip is part of the graphics state, so
/// it is in force for every mark after it until something changes it, and a page that draws
/// twenty things under one `W n` draws twenty clipped things.
///
/// **The asymmetry is the point.** Every fixture above this one is symmetric about the centre
/// of the page — a shape in each quadrant, a clip down the middle — and a symmetric page
/// scores above 0.99 whether or not the clip is the right shape or lasts the right length of
/// time, because a renderer that resets its clip after the first mark paints the second mark
/// whole and a page drawn symmetrically hides that completely. Here nothing mirrors in either
/// axis: the clip is a triangle whose hypotenuse runs from the lower right to the upper left,
/// the red bar straddles that hypotenuse, and each expected colour is named by which side of
/// a diagonal it is on.
fn two_marks_page() -> Vec<u8> {
    page_with(
        "q 0 0 0 rg 0 0 m 200 0 l 30 200 l h W n \
         0 0 200 200 re f \
         1 0 0 rg 120 20 70 30 re f Q",
        200,
    )
}

/// A 200 point square page whose clip is set inside `q` and must not outlive the `Q`.
///
/// The fill inside `q` is clipped: the black rectangle reaches past the hypotenuse and the
/// part beyond it stays paper. The fill after the `Q` is not clipped at all: the red bar's
/// right end is beyond the hypotenuse too, and it is red there.
///
/// Asymmetric for the same reason as every other fixture here: the region that says "clipped"
/// and the region that says "not clipped" are two ends of the same diagonal, so a page that
/// gets either one of them wrong has a visible, one-sided error rather than a symmetric one
/// that averages out of a score.
fn restored_clip_page() -> Vec<u8> {
    page_with(
        "q 0 0 0 rg 0 0 m 200 0 l 30 200 l h W n 0 0 190 80 re f Q \
         1 0 0 rg 20 10 160 40 re f",
        200,
    )
}

/// The shape of [`page_with`], with the content stream's filter chain left to the caller.
///
/// `filter` goes into the stream dictionary verbatim and `body` is written as the stream's
/// bytes, so `/Length` counts the bytes that are actually there — which is the whole
/// difficulty with a filtered stream and the reason the two arguments cannot be one: a
/// `/Length` of the decoded length, over encoded bytes, describes a stream whose end is
/// somewhere else entirely, and the fixture suite's habit of writing the length of the
/// text it can see is exactly the habit that hid D1.
fn page_with_filtered(filter: &str, body: &[u8], points: i64) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut at = [0usize; 5];
    out.extend_from_slice(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n");
    at[1] = out.len();
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    at[2] = out.len();
    out.extend_from_slice(
        format!(
            "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 {points} \
             {points}] >>\nendobj\n"
        )
        .as_bytes(),
    );
    at[3] = out.len();
    out.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n");
    at[4] = out.len();
    let mut stream =
        format!("4 0 obj\n<< /Length {} {filter} >>\nstream\n", body.len()).into_bytes();
    stream.extend_from_slice(body);
    stream.extend_from_slice(b"\nendstream\nendobj\n");
    out.extend_from_slice(&stream);
    let xref = out.len();
    out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n1 4\n");
    for offset in at.iter().take(5).skip(1) {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size 5 /Root 1 0 R /ID [<0102> <0304>] >>\nstartxref\n{xref}\n%%EOF\n"
        )
        .as_bytes(),
    );
    out
}

/// A one-page square file whose content stream is `/FlateDecode` compressed.
///
/// **This fixture is here because its absence was D1.** Every page of every real file has
/// its content behind a filter, and a suite in which every fixture writes its content in
/// the clear cannot tell a renderer that decodes content from one that does not — the two
/// are never compared, because no fixture asks. So the failure was total and silent: all
/// 542 pages of the wild corpus rendered with `marks: 0` and no ink, and the fixture suite
/// was green throughout.
///
/// The bytes are compressed with our own deflate encoder rather than pasted in as a byte
/// string, because that is what a writer does and it keeps the fixture honest about what a
/// filtered stream really is. `text_page`'s note above explains why that fixture goes the
/// other way; the two are complementary, and between them there is now no filter a content
/// stream can carry that this suite has not tried.
fn page_with_flate(content: &str, points: i64) -> Vec<u8> {
    let packed = mangle_filters::deflate(content.as_bytes(), mangle_filters::DeflateLevel::Default);
    assert!(
        packed.len() < content.len(),
        "the fixture should actually be compressed, not stored: {} vs {}",
        packed.len(),
        content.len()
    );
    page_with_filtered("/Filter /FlateDecode", &packed, points)
}

/// A one-page square page with two clips, one inside the other.
///
/// The first clip is the triangle (0,0) (200,0) (30,200) and the second is the triangle
/// (0,0) (20,200) (200,200). They meet in a wedge down the left of the page, and neither one
/// contains the other, so the page shows the intersection and nothing else: black in the
/// wedge, paper in each triangle's own corner, and paper in the fourth corner that belongs to
/// neither.
///
/// Asymmetric, and this is the fixture that needs it most: the intersection is a shape with a
/// curved-looking boundary on three sides and it is not mirrored in either axis, so a renderer
/// that keeps only the *newest* clip — which is the obvious wrong answer once clipping stops
/// being a side effect of the previous mark — paints the second triangle whole and fails on
/// the wedge's left edge and on the first triangle's corner at the same time.
fn nested_clip_page() -> Vec<u8> {
    page_with(
        "q 0 0 0 rg 0 0 m 200 0 l 30 200 l h W n 0 0 m 20 200 l 200 200 l h W n \
         0 0 200 200 re f Q",
        200,
    )
}

fn open(bytes: Vec<u8>) -> Document {
    Document::open(bytes, OpenOptions::default()).expect("the file should open")
}

/// The pages, in document order.
fn pages(doc: &Document) -> Vec<mangle_doc::pages::Page> {
    let catalog = doc.catalog().expect("a catalogue");
    let root = catalog
        .get("Pages")
        .and_then(Object::as_ref_id)
        .expect("the page tree");
    PageTree::build(doc, root)
        .expect("a page tree")
        .pages()
        .to_vec()
}

/// Render the first page of a file.
fn render(bytes: Vec<u8>, scale: f64) -> mangle_render::PageRender {
    let doc = open(bytes);
    let all = pages(&doc);
    let page = all.first().expect("a page");
    let resources = page
        .inherited
        .resources
        .as_ref()
        .and_then(|o| doc.resolve_object(o))
        .and_then(|o| o.as_dict().cloned())
        .map(|d| Resources::from_dict(&d, &|o| doc.resolve_object(o)))
        .unwrap_or_default();
    render_page(
        &doc,
        page,
        &resources,
        RenderOptions {
            scale,
            ..RenderOptions::default()
        },
    )
}

/// A region of a page, given as fractions of the image, in pixel bounds.
///
/// Coordinates written as pixel offsets are correct only at the resolution they were
/// written for, and a check that quietly examines the wrong pixels at another resolution is
/// worse than no check at all. Stating every region in page proportions means the same
/// assertion holds whatever scale the oracle is asked to render at.
#[must_use]
fn region(
    image: &mangle_render::Image,
    fx0: f64,
    fy0: f64,
    fx1: f64,
    fy1: f64,
) -> (usize, usize, usize, usize) {
    let w = image.width;
    let h = image.height;
    (
        (w as f64 * fx0).round() as usize,
        (h as f64 * fy0).round() as usize,
        (w as f64 * fx1).round() as usize,
        (h as f64 * fy1).round() as usize,
    )
}

/// Is a region of the page, given in page proportions, entirely one colour?
#[must_use]
fn region_is_fraction(
    image: &mangle_render::Image,
    fx0: f64,
    fy0: f64,
    fx1: f64,
    fy1: f64,
    want: [u8; 3],
) -> bool {
    let (x0, y0, x1, y1) = region(image, fx0, fy0, fx1, fy1);
    region_is(image, x0, y0, x1, y1, want)
}

/// Is a single pixel, given in page proportions, one colour?
#[must_use]
fn pixel_is_fraction(image: &mangle_render::Image, fx: f64, fy: f64, want: [u8; 3]) -> bool {
    let (x0, y0, x1, y1) = region(image, fx, fy, fx, fy);
    let Some(x) = x0.checked_sub((x1.saturating_sub(x0)) / 2) else {
        return false;
    };
    let Some(y) = y0.checked_sub((y1.saturating_sub(y0)) / 2) else {
        return false;
    };
    let Some([r, g, b, _]) = image.get(x, y) else {
        return false;
    };
    let close = |a: u8, b: u8| (i32::from(a) - i32::from(b)).abs() <= 2;
    close(r, want[0]) && close(g, want[1]) && close(b, want[2])
}

/// How dark is this pixel? 0 is white, 255 is black.
fn darkness(image: &mangle_render::Image, x: usize, y: usize) -> u32 {
    let Some([r, g, b, _]) = image.get(x, y) else {
        return 255;
    };
    // Luma rather than a channel, so a black page is black whichever way it was written.
    let luma = (u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114) / 1000;
    255 - luma
}

/// Is a region of the page entirely one colour?
fn region_is(
    image: &mangle_render::Image,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    want: [u8; 3],
) -> bool {
    for y in y0..y1 {
        for x in x0..x1 {
            let Some([r, g, b, _]) = image.get(x, y) else {
                return false;
            };
            // A tolerance of two, because a solid fill at a non-integer boundary blends
            // and an exact comparison would be testing the arithmetic rather than the
            // intent.
            let close = |a: u8, b: u8| (i32::from(a) - i32::from(b)).abs() <= 2;
            if !(close(r, want[0]) && close(g, want[1]) && close(b, want[2])) {
                return false;
            }
        }
    }
    true
}

#[test]
fn a_page_renders_at_the_size_it_asks_for() {
    let render = render(shapes_page(), 1.0);
    assert_eq!((render.image.width, render.image.height), (200, 200));
    assert_eq!(render.marks, 3, "three filled squares");
    assert!(
        render.notes.is_empty(),
        "nothing to report: {:?}",
        render.notes
    );
}

#[test]
fn each_square_is_drawn_where_the_page_puts_it() {
    let render = render(shapes_page(), 1.0);
    // The page's lower-left quadrant is black. In canvas coordinates that is the *bottom*
    // left, so the fractions run from half the height downward.
    assert!(
        region_is_fraction(&render.image, 0.05, 0.55, 0.45, 0.95, [0, 0, 0]),
        "the black square is in the lower left"
    );
    // The red square is the upper left.
    assert!(
        region_is_fraction(&render.image, 0.05, 0.05, 0.45, 0.45, [255, 0, 0]),
        "the red square is in the upper left"
    );
    // The blue square is the upper right.
    assert!(
        region_is_fraction(&render.image, 0.55, 0.05, 0.95, 0.45, [0, 0, 255]),
        "the blue square is in the upper right"
    );
}

#[test]
fn a_page_is_not_drawn_where_it_has_nothing() {
    let render = render(shapes_page(), 1.0);
    // The lower right quadrant is white paper: nothing was drawn there.
    assert!(
        region_is_fraction(&render.image, 0.55, 0.55, 0.95, 0.95, [255, 255, 255]),
        "the lower right is untouched paper"
    );
}

/// The strongest statement available without a font: the page's total ink equals the sum
/// of its three squares' ink, computed from the page rather than from a previous run.
#[test]
fn the_ink_on_the_page_is_what_the_page_drew() {
    let render = render(shapes_page(), 1.0);
    let image = &render.image;
    // The four quadrants of the page, in proportions, so the measurement means the same
    // thing at every scale.
    let ink = |fx0: f64, fy0: f64, fx1: f64, fy1: f64| -> u64 {
        let (x0, y0, x1, y1) = region(image, fx0, fy0, fx1, fy1);
        (y0..y1)
            .flat_map(|y| (x0..x1).map(move |x| (x, y)))
            .map(|(x, y)| u64::from(darkness(image, x, y)))
            .sum()
    };
    let side = 0.4f64;
    let black = ink(0.05, 0.55, 0.05 + side, 0.95);
    let red = ink(0.05, 0.05, 0.05 + side, 0.45);
    let blue = ink(0.55, 0.05, 0.55 + side, 0.45);
    let paper = ink(0.55, 0.55, 0.55 + side, 0.95);
    let measured = side * image.width.min(image.height) as f64;
    let pixels = (measured as u64) * (measured as u64);
    // A count of pixels scaled by the region, so the expectation holds at any resolution.
    // The level is a per-pixel darkness in 0..255, so it multiplies rather than scales: an
    // earlier division by 255 here reported a fully black region as empty.
    let expect = |level: u64| -> u64 { pixels * level };
    // Black and blue are dark; red is dark to the eye but light to a luma-weighted
    // measurement, which is the point of using one.
    assert_eq!(black, expect(255), "black is fully dark");
    assert_eq!(red, expect(179), "red is 179 dark in luma");
    assert_eq!(blue, expect(226), "blue is 226 dark in luma");
    assert_eq!(paper, 0u64, "paper is not dark at all");
}

#[test]
fn a_clip_keeps_a_path_inside_it() {
    let render = render(clipped_page(), 1.0);
    let image = &render.image;
    assert_eq!(
        image.width, image.height,
        "a square page makes a square canvas"
    );
    assert!(image.width > 0, "and it has pixels");
    assert!(
        region_is_fraction(&render.image, 0.05, 0.05, 0.45, 0.95, [0, 0, 0]),
        "inside the clip, the page is filled"
    );
    assert!(
        region_is_fraction(&render.image, 0.55, 0.05, 0.95, 0.95, [255, 255, 255]),
        "outside the clip, it is paper: an unclipped renderer would paint the whole page"
    );
}

/// The two halves must be *different*, or the test above would pass on a blank page.
#[test]
fn the_clipped_and_unclipped_halves_are_really_different() {
    let render = render(clipped_page(), 1.0);
    let image = &render.image;
    assert!(
        pixel_is_fraction(image, 0.25, 0.5, [0, 0, 0]),
        "black inside the clip"
    );
    assert!(
        pixel_is_fraction(image, 0.75, 0.5, [255, 255, 255]),
        "white outside it"
    );
}

/// A clip is in force for every mark after it, not only for the first.
///
/// A clip is part of the graphics state. It takes effect at the `W n` and stays in force
/// until something changes it, so a page that clips once and then draws twice draws two
/// clipped marks. The renderer used to apply a clip when it reached the `W n` and drop it
/// after the next mark — the mark's own cull and that reset were the same rectangle — which
/// meant a page that drew twenty things under one clip drew nineteen of them unclipped.
///
/// The fixture is the diagonal one, so this cannot pass by accident: the red bar straddles the
/// hypotenuse, and the assertion that the bar's far end is paper is the assertion that the
/// *second* mark was clipped. A page whose second mark is unclipped has red ink where the
/// clip says there is none, and that is a difference a symmetric fixture cannot hide.
#[test]
fn a_clip_is_in_force_for_every_mark_after_it() {
    let render = render(two_marks_page(), 1.0);
    let image = &render.image;
    assert_eq!((image.width, image.height), (200, 200));
    assert!(
        render.notes.is_empty(),
        "nothing to report: {:?}",
        render.notes
    );

    // The first mark fills the whole page and is clipped to the triangle, so inside the
    // triangle it is black.
    assert!(
        region_is_fraction(image, 0.075, 0.70, 0.50, 0.975, [0, 0, 0]),
        "the first mark is clipped: black inside the triangle"
    );

    // The second mark paints the bar, and the part of the bar inside the triangle is red. If
    // the second mark were not drawn at all this would be black, and if it were clipped away
    // it would be paper, so this pins that it was drawn rather than merely that it was not
    // left unclipped.
    assert!(
        region_is_fraction(image, 0.625, 0.775, 0.75, 0.875, [255, 0, 0]),
        "the second mark is clipped and is drawn where the clip keeps it"
    );

    // And the part of the bar beyond the hypotenuse is paper, which is the whole claim: the
    // second mark was in force of the clip.
    assert!(
        region_is_fraction(image, 0.91, 0.775, 0.95, 0.875, [255, 255, 255]),
        "the second mark is clipped: the bar stops at the hypotenuse"
    );
}

/// `Q` restores the clip, so a mark after it is not clipped at all.
///
/// A clip is saved and restored by `q` and `Q` along with everything else in the graphics
/// state, so the clip set inside the pair does not reach past the `Q`. Both fills reach beyond
/// the hypotenuse and the two answers are on either side of it: black stops at the diagonal,
/// red does not.
#[test]
fn a_clip_is_restored_by_q() {
    let render = render(restored_clip_page(), 1.0);
    let image = &render.image;

    // Inside the pair: the black rectangle is clipped to the triangle. The region is above the
    // red bar rather than beside it, because the red bar is drawn afterwards and would cover
    // black anywhere they overlap — which is the point of the fixture, not something to work
    // around.
    assert!(
        region_is_fraction(image, 0.075, 0.61, 0.50, 0.74, [0, 0, 0]),
        "the fill inside q is clipped: black inside the triangle"
    );
    assert!(
        region_is_fraction(image, 0.825, 0.625, 0.925, 0.725, [255, 255, 255]),
        "and the part of the rectangle beyond the hypotenuse is paper"
    );

    // After the `Q`: the red bar is drawn whole. Its right end is beyond the hypotenuse, so a
    // `Q` that did not restore the clip would leave paper here instead.
    assert!(
        region_is_fraction(image, 0.825, 0.76, 0.89, 0.80, [255, 0, 0]),
        "the fill after q is not clipped: red beyond the hypotenuse"
    );
}

/// Two clips nest: what is drawn is their intersection.
///
/// `W n` narrows the clipping path rather than replacing it, so the region in force after two
/// of them is where both are. The fixture's two triangles overlap in a wedge and neither
/// contains the other, so a renderer that kept only one of them paints a triangle and this
/// test fails on both edges of the wedge at once.
#[test]
fn two_clips_nest_rather_than_replacing_each_other() {
    let render = render(nested_clip_page(), 1.0);
    let image = &render.image;

    // Inside both triangles: the wedge, in two places.
    assert!(
        region_is_fraction(image, 0.10, 0.525, 0.40, 0.575, [0, 0, 0]),
        "where both clips are, the page is filled"
    );
    assert!(
        region_is_fraction(image, 0.15, 0.15, 0.22, 0.25, [0, 0, 0]),
        "and higher up the page as well"
    );

    // Inside the first triangle only — below its partner's hypotenuse. A renderer that had
    // forgotten the first clip would paint this.
    assert!(
        region_is_fraction(image, 0.30, 0.90, 0.50, 0.975, [255, 255, 255]),
        "the first clip's own corner is outside the second and is paper"
    );

    // Inside the second triangle only — left of the first triangle's left edge, and inside
    // both boxes. This is the region that says the *older* clip is still in force: it is inside
    // the newer clip's path and inside the intersection of the two boxes, so only the first
    // path's shape keeps it out.
    assert!(
        region_is_fraction(image, 0.025, 0.10, 0.075, 0.30, [255, 255, 255]),
        "the second clip's corner is outside the first and is paper"
    );
}

/// The same page rendered twice is the same image, byte for byte.
///
/// This is the property the clip lifetime is really about. A renderer that carries its clip
/// from one mark to the next as a side effect of drawing it has an output that depends on the
/// order it happened to draw things in; one that installs the clip each mark's own record
/// carries has an output that depends only on the page. Rendering twice and asking for exact
/// equality is the operational form of that: it is the check a viewer would fail the first
/// time a thread pool, a tile boundary or a future reordering moved the work around.
///
/// All three clip fixtures are checked, because a reproducibility check that only ever runs
/// the simple case is a check on the simple case.
#[test]
fn the_same_page_rendered_twice_is_the_same_image() {
    for (name, page) in [
        ("two marks under one clip", two_marks_page()),
        ("a clip inside q and q", restored_clip_page()),
        ("two nested clips", nested_clip_page()),
    ] {
        let first = render(page.clone(), 1.0);
        let second = render(page, 1.0);
        assert_eq!(
            first.image.pixels, second.image.pixels,
            "{name}: the same page rendered twice is not the same image"
        );
        assert_eq!(
            first.marks, second.marks,
            "{name}: and not the same number of marks either"
        );
    }
}

#[test]
fn a_scale_grows_the_page_and_its_ink_with_it() {
    let small = render(shapes_page(), 1.0);
    let large = render(shapes_page(), 2.0);
    assert_eq!(
        (large.image.width, large.image.height),
        (400, 400),
        "twice the scale is twice the pixels"
    );
    let ink = |r: &mangle_render::PageRender| -> u64 {
        (0..r.image.height)
            .flat_map(|y| (0..r.image.width).map(move |x| (x, y)))
            .map(|(x, y)| u64::from(darkness(&r.image, x, y)))
            .sum()
    };
    let (a, b) = (ink(&small), ink(&large));
    // Four times the area means about four times the ink, since the shapes cover the same
    // fraction of the page.
    let ratio = b as f64 / a as f64;
    assert!(
        (3.0..5.0).contains(&ratio),
        "four times the area is about four times the ink, got {ratio}"
    );
}

#[test]
fn an_absurd_scale_is_brought_back_within_bounds() {
    let render = render(shapes_page(), 100_000.0);
    assert_eq!(
        render.scale,
        mangle_render::MAX_SCALE,
        "the scale is capped"
    );
    let side = render.image.width.max(render.image.height) as f64;
    assert!(
        side <= mangle_render::page::MAX_SIDE,
        "and the buffer is within its bound: {side}"
    );
}

#[test]
fn a_blank_page_renders_as_blank_rather_than_failing() {
    let mut out: Vec<u8> = Vec::new();
    let mut at = [0usize; 5];
    out.extend_from_slice(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n");
    at[1] = out.len();
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    at[2] = out.len();
    out.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 50 50] >>\nendobj\n",
    );
    at[3] = out.len();
    out.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n");
    let xref = out.len();
    out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n1 3\n");
    for offset in at.iter().take(4).skip(1) {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    let render = render(out, 1.0);
    assert!(render.is_blank(), "a page with no content is paper");
    assert_eq!(render.marks, 0);
}

/// A compressed content stream is decoded before it is interpreted.
///
/// **This is the regression test for D1.** The content is the same three squares
/// [`shapes_page`] draws, byte for byte, and the only difference is that it arrives behind
/// `/FlateDecode`. Before the fix `render_page` handed the *encoded* bytes to the content
/// interpreter, which reported the compressed bytes as operator names and drew nothing:
/// `marks: 0`, and a blank page whose notes were a row of mojibake rather than a reason.
///
/// Asserting on the marks as well as the pixels is deliberate. A renderer that drew blank
/// paper because it failed to decode would pass an "is it white?" check, which is why the
/// mark count is here: it distinguishes "drew nothing" from "drew the wrong thing", and
/// those are different bugs.
#[test]
fn a_compressed_content_stream_is_decoded_and_not_read_as_operators() {
    let content = "0 0 0 rg 0 0 100 100 re f \
                   1 0 0 rg 0 100 100 100 re f \
                   0 0 1 rg 100 100 100 100 re f";
    let plain = render(page_with(content, 200), 1.0);
    let packed = render(page_with_flate(content, 200), 1.0);

    assert_eq!(
        packed.marks, plain.marks,
        "a filtered stream is the same page as an unfiltered one, so it draws the same marks: \
         plain {} vs filtered {}",
        plain.marks, packed.marks
    );
    assert_eq!(
        packed.marks, 3,
        "three squares, so three marks — anything else means the content was not read"
    );
    assert!(
        packed
            .notes
            .iter()
            .all(|n| !n.contains("is not in the table")),
        "no compressed bytes should reach the operator table: {:?}",
        packed.notes
    );

    // The pixels, not just the count: the content says `re f` with the origin at the
    // bottom left of the page, and the image's row zero is the top, so the black square
    // is the *lower* left and reads at a large `y`.
    let at = |x: usize, y: usize| packed.image.get(x, y).expect("a pixel");
    assert_eq!(
        at(50, 150),
        [0, 0, 0, 255],
        "the lower-left square is black"
    );
    assert_eq!(at(50, 50), [255, 0, 0, 255], "the upper-left square is red");
    assert_eq!(
        at(150, 50),
        [0, 0, 255, 255],
        "the upper-right square is blue"
    );
    assert_eq!(
        at(150, 150),
        [255, 255, 255, 255],
        "and the fourth quadrant is paper"
    );
    assert!(
        !packed.is_blank(),
        "and the page is not blank paper, which is what the bug produced"
    );
}

/// A content stream that will not decode is reported rather than drawn as blank paper.
///
/// The constraint is the interesting half of D1. Decoding the stream is the fix; *saying
/// so* is what keeps the next file from being another two-hour mystery. A page that lost
/// its content to a broken filter must reach the caller with the reason attached, because
/// "this page is empty" and "this page's content is unreadable" look identical on screen
/// and mean opposite things.
///
/// Nothing here is guessed at either. The damaged bytes are not run through the operator
/// table as though they were content, and no substitute is drawn in their place — the page
/// reports and stops.
#[test]
fn a_content_stream_that_will_not_decode_is_reported() {
    // A `/FlateDecode` stream whose bytes are not a deflate stream at all. This is the
    // shape of a truncated or corrupted file, and it is the case where a decoder that
    // returns something anyway must say that it did.
    let damaged = page_with_filtered("/Filter /FlateDecode", b"\x00\x01\x02 not deflate", 200);
    let render = render(damaged, 1.0);

    assert!(
        render.notes.iter().any(|n| n.contains("FlateDecode")),
        "the reason a content stream could not be read belongs in the notes: {:?}",
        render.notes
    );
    assert!(
        render
            .notes
            .iter()
            .all(|n| !n.contains("is not in the table")),
        "and the damaged bytes must not be read as operators: {:?}",
        render.notes
    );
}

/// A content stream behind a filter nobody implements is reported by name.
///
/// The other half of refusing. An unknown filter is not a corrupt file and not an empty
/// one: the content is there and this build cannot read it. Naming it is the difference
/// between a report a reader can act on and a page that is mysteriously blank.
#[test]
fn a_content_stream_behind_an_unknown_filter_is_reported_by_name() {
    let bytes = page_with_filtered("/Filter /NoSuchDecode", b"0 0 0 rg 0 0 100 100 re f", 200);
    let render = render(bytes, 1.0);

    assert!(
        render
            .notes
            .iter()
            .any(|n| n.contains("NoSuchDecode") && n.contains("content stream")),
        "the filter is named, and it is named as the page's content: {:?}",
        render.notes
    );
    // The bytes behind `/NoSuchDecode` are still encoded. They are deliberately written as
    // readable content here so that drawing them would be *possible*, which is exactly the
    // case a test has to rule out: a renderer that hands encoded bytes to the operator
    // table will pass this file and get a plausible page out of it. Zero marks is the
    // honest answer — the file says its content is filtered and we cannot read it.
    assert_eq!(
        render.marks, 0,
        "and nothing is drawn from bytes we cannot read, even when they would parse: {:?}",
        render.notes
    );
    assert!(render.is_blank(), "so the page is paper, and says why");
}

/// A page whose content is an *array* of streams decodes each one and says which failed.
///
/// `/Contents` is an array at least as often as it is a single stream, and an array has
/// more than one thing that can go wrong. A note that does not say which of the streams
/// failed leaves the reader with a page and a filter and no way to tell whether the
/// missing part was the first or the last.
#[test]
fn an_array_of_content_streams_names_the_one_that_failed() {
    // Two streams: the first draws a black square and decodes; the second is damaged.
    let good = mangle_filters::deflate(
        b"0 0 0 rg 0 0 100 100 re f",
        mangle_filters::DeflateLevel::Default,
    );
    let bad = b"\x00\x01\x02 not deflate";

    let mut out: Vec<u8> = Vec::new();
    let mut at = [0usize; 6];
    out.extend_from_slice(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n");
    at[1] = out.len();
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    at[2] = out.len();
    out.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>\nendobj\n",
    );
    at[3] = out.len();
    out.extend_from_slice(
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents [4 0 R 5 0 R] >>\nendobj\n",
    );
    let parts: [(&[u8], &[u8]); 2] = [
        (b"/Filter /FlateDecode", good.as_slice()),
        (b"/Filter /FlateDecode", bad.as_slice()),
    ];
    for (i, (filter, body)) in parts.iter().enumerate() {
        let num = i + 4;
        at[num] = out.len();
        let mut stream = format!(
            "{num} 0 obj\n<< /Length {} {} >>\nstream\n",
            body.len(),
            String::from_utf8_lossy(filter)
        )
        .into_bytes();
        stream.extend_from_slice(body);
        stream.extend_from_slice(b"\nendstream\nendobj\n");
        out.extend_from_slice(&stream);
    }
    let xref = out.len();
    out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n1 5\n");
    for offset in at.iter().take(6).skip(1) {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size 6 /Root 1 0 R /ID [<0102> <0304>] >>\nstartxref\n{xref}\n%%EOF\n"
        )
        .as_bytes(),
    );

    let render = render(out, 1.0);
    assert!(
        render
            .notes
            .iter()
            .any(|n| n.contains("content stream 2 of 2")),
        "the note says which of the two failed: {:?}",
        render.notes
    );
    assert_eq!(
        render.marks, 1,
        "and the stream that did decode is still drawn — a broken neighbour is not a reason \
         to drop the whole page"
    );
    assert_eq!(
        render.image.get(50, 150),
        Some([0, 0, 0, 255]),
        "which means the square from the first stream is on the page (its `re f` is in the \
         lower left of the page, which is a large y in the image)"
    );
}

#[test]
fn the_corpus_pages_render_without_a_panic() {
    // Every Tier-A fixture, rendered at two scales. This does not check that the result
    // is *right* — the oracle test below does that — but it does check that no file makes
    // the renderer hang, allocate without bound, or fail.
    let Some(dir) = corpus_dir() else {
        eprintln!("skipped: run `cargo xtask fixtures` first");
        return;
    };
    let mut rendered = 0usize;
    for entry in std::fs::read_dir(&dir).expect("the corpus") {
        let path = entry.expect("an entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("pdf") {
            continue;
        }
        let bytes = std::fs::read(&path).expect("a fixture");
        let doc = Document::open(bytes, OpenOptions::default())
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        for page in pages(&doc) {
            let resources = page
                .inherited
                .resources
                .as_ref()
                .and_then(|o| doc.resolve_object(o))
                .and_then(|o| o.as_dict().cloned())
                .map(|d| Resources::from_dict(&d, &|o| doc.resolve_object(o)))
                .unwrap_or_default();
            for scale in [1.0, 2.5] {
                let render = render_page(
                    &doc,
                    &page,
                    &resources,
                    RenderOptions {
                        scale,
                        ..RenderOptions::default()
                    },
                );
                assert!(
                    render.image.width > 0 && render.image.height > 0,
                    "{}: a page rendered to nothing",
                    path.display()
                );
            }
            rendered += 1;
        }
    }
    assert!(rendered >= 4, "only {rendered} pages were checked");
}

fn corpus_dir() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)?;
    let dir = root.join("fixtures");
    dir.join("MANIFEST.toml").is_file().then_some(dir)
}

// ── The oracle ────────────────────────────────────────────────────────────────

/// Where `mutool` is, if it is installed.
///
/// This is a test-only lookup and the only process this project starts anywhere: the
/// product opens files and runs nothing.
fn mutool() -> Option<PathBuf> {
    let output = Command::new("mutool").arg("-v").output().ok()?;
    output.status.success().then(|| PathBuf::from("mutool"))
}

/// Render a PDF with `mutool` into a PNM, and read it back.
///
/// `mutool draw` writes a PPM by default; the alpha channel is not written, so the result
/// is compared on the colour channels alone.
fn mutool_render(pdf: &Path, scale: f64, out: &Path) -> Option<Vec<u8>> {
    let tool = mutool()?;
    let dpi = (scale * 72.0).round().to_string();
    let status = Command::new(&tool)
        .args([
            "draw",
            "-r",
            &dpi,
            "-F",
            "pam",
            "-o",
            &out.to_string_lossy(),
            &pdf.to_string_lossy(),
            "1",
        ])
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    std::fs::read(out).ok()
}

/// A PAM file's pixels: the dimensions and the rows.
///
/// The format is a header of `KEY value` lines terminated by `ENDHDR`, then the bytes.
/// Everything before `ENDHDR\n` is text and everything after is pixels, so the split is
/// a search for that marker rather than a count of lines — `WIDTH` and `DEPTH` may be in
/// either order and `TUPLTYPE` is optional.
fn read_pam(data: &[u8]) -> Option<(usize, usize, usize, Vec<u8>)> {
    let marker = b"ENDHDR\n";
    let split = data.windows(marker.len()).position(|w| w == marker)?;
    let header = String::from_utf8_lossy(&data[..split]).to_string();
    if !header.starts_with("P7") {
        return None;
    }
    let field = |name: &str| -> Option<usize> {
        header
            .lines()
            .find_map(|l| l.trim().strip_prefix(name))
            .and_then(|v| v.trim().parse().ok())
    };
    let (w, h, depth) = (field("WIDTH")?, field("HEIGHT")?, field("DEPTH")?);
    let body = data.get(split + marker.len()..)?.to_vec();
    // A truncated file is a failure rather than a shorter image: silently comparing a
    // quarter of a page would pass on the part that happened to agree.
    if body.len() < w * h * depth {
        return None;
    }
    Some((w, h, depth, body))
}

/// Turn a PAM's pixels into an image, keeping the alpha channel.
///
/// `mutool` writes an unpainted page as fully transparent with black in the colour
/// channels, while this renderer starts a page as opaque white paper. Both are defensible
/// conventions and they are not the same picture, so the alpha is kept here and the
/// comparison flattens both onto one background first. Comparing the raw buffers would be
/// comparing conventions rather than renderers, and would report a disagreement on every
/// pixel of a page's margin.
fn pam_to_image(w: usize, h: usize, depth: usize, body: Vec<u8>) -> mangle_render::Image {
    let mut image = mangle_render::Image::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * depth;
            let at = |c: usize| body.get(i + c).copied().unwrap_or(255);
            image.put(
                x,
                y,
                [at(0), at(1), at(2), if depth > 3 { at(3) } else { 255 }],
            );
        }
    }
    image
}

/// An image composited onto opaque white paper, which is what a page looks like on a desk.
fn flatten_onto_paper(image: &mangle_render::Image) -> mangle_render::Image {
    let mut out = mangle_render::Image::filled(image.width, image.height, [255, 255, 255, 255]);
    for y in 0..image.height {
        for x in 0..image.width {
            let src = image.get(x, y).unwrap_or([255, 255, 255, 255]);
            let a = f64::from(src[3]) / 255.0;
            let mut over = [255u8; 4];
            for c in 0..3 {
                let s = f64::from(src[c]) / 255.0;
                over[c] = ((s * a + 1.0 * (1.0 - a)) * 255.0).round() as u8;
            }
            out.put(x, y, over);
        }
    }
    out
}

/// The structural comparison against `mutool`, for the shapes page.
///
/// This is the check that says something about correctness rather than about consistency:
/// `mutool` is an independent implementation, and if the two agree about where the ink is
/// then our rasterizer is doing what the format means rather than what we intended.
#[test]
fn our_rendering_agrees_with_mutools_about_where_the_ink_is() {
    if mutool().is_none() {
        eprintln!("skipped: mutool is not installed");
        return;
    }
    let dir = std::env::temp_dir().join("mangle-render-oracle");
    std::fs::create_dir_all(&dir).expect("a place to work");
    let pdf = dir.join("shapes.pdf");
    std::fs::write(&pdf, shapes_page()).expect("a file to render");

    // 150 DPI is the resolution the acceptance criteria name, so this measures the thing
    // the bar is written against rather than an easier version of it.
    let scale = 150.0 / 72.0;
    let ours = render(shapes_page(), scale);
    let theirs_path = dir.join("shapes.pam");
    let Some(data) = mutool_render(&pdf, scale, &theirs_path) else {
        eprintln!("skipped: mutool could not render the page");
        return;
    };
    let Some((w, h, depth, body)) = read_pam(&data) else {
        eprintln!("skipped: could not read mutool's output");
        return;
    };
    let theirs = flatten_onto_paper(&pam_to_image(w, h, depth, body));

    let comparison = compare(&ours.image, &theirs, &SsimOptions::default());
    assert!(
        comparison.is_valid(),
        "the comparison did not happen: {:?}",
        comparison.refused
    );
    let metrics = &comparison.metrics;
    eprintln!("shapes page: {}", metrics.summary());
    assert!(
        metrics.meets_fidelity_bar(0.95),
        "our render scores {:.4} against mutool, below the 0.95 fidelity bar: {}",
        metrics.ssim,
        metrics.summary()
    );

    // The structural claims hold in the oracle's rendering too, which is what makes them
    // claims about the page rather than about our code.
    assert!(
        region_is_fraction(&theirs, 0.05, 0.55, 0.45, 0.95, [0, 0, 0]),
        "mutool also puts the black square in the lower left"
    );
    assert!(
        region_is_fraction(&theirs, 0.55, 0.05, 0.95, 0.45, [0, 0, 255]),
        "and the blue square in the upper right"
    );
    assert!(
        region_is_fraction(&theirs, 0.55, 0.55, 0.95, 0.95, [255, 255, 255]),
        "and nothing in the lower right"
    );
}

/// The clipped page, against the oracle.
///
/// A clip is where a renderer is most likely to be subtly wrong, and where a difference
/// shows up as ink in the wrong place rather than as a slightly soft edge.
#[test]
fn our_clip_agrees_with_mutools() {
    if mutool().is_none() {
        eprintln!("skipped: mutool is not installed");
        return;
    }
    let dir = std::env::temp_dir().join("mangle-render-oracle");
    std::fs::create_dir_all(&dir).expect("a place to work");
    let pdf = dir.join("clipped.pdf");
    std::fs::write(&pdf, clipped_page()).expect("a file to render");

    let scale = 150.0 / 72.0;
    let ours = render(clipped_page(), scale);
    let theirs_path = dir.join("clipped.pam");
    let Some(data) = mutool_render(&pdf, scale, &theirs_path) else {
        eprintln!("skipped: mutool could not render the page");
        return;
    };
    let Some((w, h, depth, body)) = read_pam(&data) else {
        eprintln!("skipped: could not read mutool's output");
        return;
    };
    let theirs = flatten_onto_paper(&pam_to_image(w, h, depth, body));

    // Outside the clip, mutool must also draw paper. An unclipped renderer would put ink
    // there, and this is the assertion that catches it.
    assert!(
        region_is_fraction(&theirs, 0.6, 0.1, 0.95, 0.95, [255, 255, 255]),
        "mutool also keeps the clip"
    );
    let comparison = compare(&ours.image, &theirs, &SsimOptions::default());
    assert!(
        comparison.is_valid(),
        "the comparison did not happen: {:?}",
        comparison.refused
    );
    let metrics = &comparison.metrics;
    eprintln!("clipped page: {}", metrics.summary());
    assert!(
        metrics.meets_fidelity_bar(0.95),
        "our render scores {:.4} against mutool, below the 0.95 fidelity bar: {}",
        metrics.ssim,
        metrics.summary()
    );
}

/// The diagonal-clip page, against the oracle.
///
/// A clip is where a renderer is most likely to be subtly wrong, and where a difference
/// shows up as ink in the wrong place rather than as a slightly soft edge. This is the
/// check that a clip is a *region* and not the box around it: with the box, the whole of
/// the upper right of this page would be black, and the score below would say so.
#[test]
fn our_diagonal_clip_agrees_with_mutools() {
    if mutool().is_none() {
        eprintln!("skipped: mutool is not installed");
        return;
    }
    let dir = std::env::temp_dir().join("mangle-render-oracle");
    std::fs::create_dir_all(&dir).expect("a place to work");
    let pdf = dir.join("diagonal-clip.pdf");
    std::fs::write(&pdf, diagonal_clip_page()).expect("a file to render");

    let scale = 150.0 / 72.0;
    let ours = render(diagonal_clip_page(), scale);
    let theirs_path = dir.join("diagonal-clip.pam");
    let Some(data) = mutool_render(&pdf, scale, &theirs_path) else {
        eprintln!("skipped: mutool could not render the page");
        return;
    };
    let Some((w, h, depth, body)) = read_pam(&data) else {
        eprintln!("skipped: could not read mutool's output");
        return;
    };
    let theirs = flatten_onto_paper(&pam_to_image(w, h, depth, body));

    // The page's own claims, checked on *both* renderings. The upper right is outside the
    // hypotenuse, so it is paper in a renderer that clips to the region and black in one
    // that clips to the box: this is the assertion that tells the two apart, and it is why
    // the fixture is asymmetric.
    for (image, who) in [(&ours.image, "ours"), (&theirs, "mutool")] {
        assert!(
            region_is_fraction(image, 0.6, 0.05, 0.95, 0.45, [255, 255, 255]),
            "{who} puts ink where the diagonal clip says there is none"
        );
        assert!(
            region_is_fraction(image, 0.4, 0.85, 0.75, 0.95, [0, 0, 0]),
            "{who} leaves the lower right bare inside the clip"
        );
        assert!(
            region_is_fraction(image, 0.33, 0.36, 0.45, 0.39, [0, 0, 255]),
            "{who} loses the part of the blue bar that the clip keeps"
        );
    }

    let comparison = compare(&ours.image, &theirs, &SsimOptions::default());
    assert!(
        comparison.is_valid(),
        "the comparison did not happen: {:?}",
        comparison.refused
    );
    let metrics = &comparison.metrics;
    eprintln!("diagonal clip page: {}", metrics.summary());
    assert!(
        metrics.meets_fidelity_bar(0.95),
        "our render scores {:.4} against mutool, below the 0.95 fidelity bar: {}",
        metrics.ssim,
        metrics.summary()
    );
}

/// The two-mark clip page, against the oracle.
///
/// This is the check for a page whose *second* mark is clipped, which is the thing a
/// single-fill clip fixture cannot see: one fill under one `W n` is right whether or not the
/// clip outlives the mark, so the earlier clipped page scored 0.98537 while this defect was
/// live. Here the second mark straddles the clip's hypotenuse, so a renderer that dropped the
/// clip after the first mark paints ink the oracle leaves as paper, and the score says so.
///
/// The disagreement this comparison leaves on the table is a deliberate one and is recorded in
/// `docs/STATUS.md`: this renderer antialiases a clip edge and `mutool` hard-steps it onto the
/// pixel grid, which cost about 0.014 SSIM on the earlier clipped page. The bar here is 0.95
/// for that reason — the point of the comparison is where the ink is, not whether the two
/// programs round a diagonal edge the same way.
#[test]
fn our_two_mark_clip_agrees_with_mutools() {
    if mutool().is_none() {
        eprintln!("skipped: mutool is not installed");
        return;
    }
    let dir = std::env::temp_dir().join("mangle-render-oracle");
    std::fs::create_dir_all(&dir).expect("a place to work");
    let pdf = dir.join("two-marks.pdf");
    std::fs::write(&pdf, two_marks_page()).expect("a file to render");

    let scale = 150.0 / 72.0;
    let ours = render(two_marks_page(), scale);
    let theirs_path = dir.join("two-marks.pam");
    let Some(data) = mutool_render(&pdf, scale, &theirs_path) else {
        eprintln!("skipped: mutool could not render the page");
        return;
    };
    let Some((w, h, depth, body)) = read_pam(&data) else {
        eprintln!("skipped: could not read mutool's output");
        return;
    };
    let theirs = flatten_onto_paper(&pam_to_image(w, h, depth, body));

    // The page's own claims, on both renderings. The red bar's far end is beyond the
    // hypotenuse and is the assertion that tells a second mark drawn unclipped from a second
    // mark drawn clipped; it is checked on the oracle's rendering too, so it is a claim about
    // the page rather than about this renderer.
    for (image, who) in [(&ours.image, "ours"), (&theirs, "mutool")] {
        assert!(
            region_is_fraction(image, 0.625, 0.775, 0.75, 0.875, [255, 0, 0]),
            "{who} draws the second mark where the clip keeps it"
        );
        assert!(
            region_is_fraction(image, 0.91, 0.775, 0.95, 0.875, [255, 255, 255]),
            "{who} clips the second mark: the bar stops at the hypotenuse"
        );
    }

    let comparison = compare(&ours.image, &theirs, &SsimOptions::default());
    assert!(
        comparison.is_valid(),
        "the comparison did not happen: {:?}",
        comparison.refused
    );
    let metrics = &comparison.metrics;
    eprintln!("two marks under one clip: {}", metrics.summary());
    assert!(
        metrics.meets_fidelity_bar(0.95),
        "our render scores {:.4} against mutool, below the 0.95 fidelity bar: {}",
        metrics.ssim,
        metrics.summary()
    );
}

/// A property of the page that both renderers must agree on, stated directly: a rotated
/// page's ink moves with it.
#[test]
fn a_rotated_page_puts_its_ink_where_the_rotation_says() {
    // The same shapes page with `/Rotate 90`, written here so the two differ only in the
    // rotation.
    let mut bytes = shapes_page();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let rotated = text.replace(
        "/Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200]",
        "/Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] /Rotate 90",
    );
    assert_ne!(rotated, text, "the page should have been rotated");
    bytes = rotated.into_bytes();

    let doc = open(bytes);
    let all = pages(&doc);
    let page = all.first().expect("a page");
    assert_eq!(page.inherited.rotation(), 90);
    let render = render_page(
        &doc,
        page,
        &Resources::default(),
        RenderOptions {
            scale: 1.0,
            ..RenderOptions::default()
        },
    );
    // Rotated a quarter turn, the page is landscape: 200 by 200 points becomes 200 by 200
    // points either way here, so the size is not the tell — the ink is.
    assert_eq!(render.marks, 3, "the same three squares");
    // The black square was in the lower left; after a quarter turn it is in the upper left.
    assert!(
        region_is_fraction(&render.image, 0.05, 0.05, 0.45, 0.45, [0, 0, 0]),
        "the black square moved to the upper left"
    );
    // The red square was in the upper left and is now in the upper right, and the lower
    // right, which held the blue square, still holds blue: a quarter turn moves the three
    // squares round and leaves the empty quadrant in the lower left.
    assert!(
        region_is_fraction(&render.image, 0.55, 0.05, 0.95, 0.45, [255, 0, 0]),
        "the red square moved to the upper right"
    );
    assert!(
        region_is_fraction(&render.image, 0.55, 0.55, 0.95, 0.95, [0, 0, 255]),
        "the blue square stayed in the lower right"
    );
    assert!(
        region_is_fraction(&render.image, 0.05, 0.55, 0.45, 0.95, [255, 255, 255]),
        "and the quadrant that was empty is now paper"
    );
}

/// A rectangle read straight out of the document model, which is what a caller building
/// its own canvas needs.
#[test]
fn a_page_rectangle_comes_from_the_crop_box() {
    let doc = open(shapes_page());
    let all = pages(&doc);
    let page = all.first().expect("a page");
    let rect: Rect = page.inherited.crop_rect();
    assert_eq!(rect.width(), 200.0);
    assert_eq!(rect.height(), 200.0);
    assert_eq!(rect.left, 0.0);
    assert_eq!(rect.bottom, 0.0);
}

/// The decoded content of a page, which is what the renderer ran.
#[test]
fn a_pages_content_decodes() {
    let doc = open(shapes_page());
    let all = pages(&doc);
    let page = all.first().expect("a page");
    let raw = page.contents(&doc);
    let streams = page.content_streams(&doc);
    assert_eq!(streams.len(), 1, "one content stream");
    let decoded = decode_stream(&streams[0]);
    assert!(
        String::from_utf8_lossy(&decoded.data).contains("re f"),
        "and it is the shapes: {:?}",
        String::from_utf8_lossy(&decoded.data)
    );
    assert_eq!(raw.len(), decoded.data.len().max(raw.len()));
}

/// A reference that resolves, which is what the resource tables depend on.
#[test]
fn a_reference_resolves_to_its_object() {
    let doc = open(shapes_page());
    let catalog = doc.catalog().expect("a catalogue");
    let r: Ref = catalog
        .get("Pages")
        .and_then(Object::as_ref_id)
        .expect("a ref");
    let resolved = doc.object(r).expect("the object");
    assert!(resolved.as_dict().is_some());
    assert!(
        doc.resolve_object(&Object::Ref(r)).is_some(),
        "and the one-step resolver finds it too"
    );
}

/// A 100 by 100 point page with a two-by-two red image XObject filling its left half.
///
/// Four samples rather than a photograph: the point is that the samples are read, the
/// colour space is honoured and the placement matrix is applied, not that anything is
/// pretty.
fn image_page() -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut at = [0usize; 6];
    out.extend_from_slice(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n");
    at[1] = out.len();
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    at[2] = out.len();
    out.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 100] \
          /Resources << /XObject << /Im0 5 0 R >> >> >>\nendobj\n",
    );
    at[3] = out.len();
    out.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n");
    // The unit square scaled to the left half of the page.
    let content = b"q 50 0 0 100 0 0 cm /Im0 Do Q";
    at[4] = out.len();
    let mut body = format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).into_bytes();
    body.extend_from_slice(content);
    body.extend_from_slice(b"\nendstream\nendobj\n");
    out.extend_from_slice(&body);
    // A two by two image, every sample pure red.
    at[5] = out.len();
    let samples = [255u8, 0, 0, 255, 0, 0, 255, 0, 0, 255, 0, 0];
    let mut image = format!(
        "5 0 obj\n<< /Type /XObject /Subtype /Image /Width 2 /Height 2 \
         /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length {} >>\nstream\n",
        samples.len()
    )
    .into_bytes();
    image.extend_from_slice(&samples);
    image.extend_from_slice(b"\nendstream\nendobj\n");
    out.extend_from_slice(&image);
    let xref = out.len();
    out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n1 5\n");
    for offset in at.iter().take(6).skip(1) {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    out
}

/// A 100 by 100 point page whose only content is an eight-by-eight `/ImageMask` scaled to
/// fill the whole page, painted in the colour `colour` names.
///
/// The mask's painting bits — the zeros — are the left half of it, so a renderer that
/// paints the whole of a mask paints the right half too, and one that paints the wrong
/// bits paints nothing at all. Either shows up in the regions below.
///
/// `pattern` makes the fill a pattern rather than a colour, which is the case that must be
/// reported rather than guessed at.
fn mask_page(colour: &str, pattern: bool) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut at = [0usize; 7];
    out.extend_from_slice(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n");
    at[1] = out.len();
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    at[2] = out.len();
    out.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 100] \
          /Resources << /XObject << /Im0 5 0 R >> /Pattern << /P0 6 0 R >> >> >>\nendobj\n",
    );
    at[3] = out.len();
    out.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n");
    let fill = if pattern {
        "/Pattern cs /P0 scn".to_owned()
    } else {
        format!("{colour} rg")
    };
    let content = format!("q {fill} 100 0 0 100 0 0 cm /Im0 Do Q");
    at[4] = out.len();
    let mut body = format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).into_bytes();
    body.extend_from_slice(content.as_bytes());
    body.extend_from_slice(b"\nendstream\nendobj\n");
    out.extend_from_slice(&body);
    // Eight by eight one-bit samples, one byte per row. The four zero bits at the bottom
    // of each byte are the left four pixels, and they are the ones that paint.
    at[5] = out.len();
    let samples = [0b0000_1111u8; 8];
    let mut image = format!(
        "5 0 obj\n<< /Type /XObject /Subtype /Image /ImageMask true /Width 8 /Height 8 \
         /BitsPerComponent 1 /Length {} >>\nstream\n",
        samples.len()
    )
    .into_bytes();
    image.extend_from_slice(&samples);
    image.extend_from_slice(b"\nendstream\nendobj\n");
    out.extend_from_slice(&image);
    // A shading pattern, which the page only names when it is the fill colour in force. It
    // is never painted — the point is that a mask painted in one is reported instead — but a
    // file that names one names a real one.
    at[6] = out.len();
    out.extend_from_slice(
        b"6 0 obj\n<< /Type /Pattern /PatternType 2 /Shading << /ShadingType 2 \
          /ColorSpace /DeviceGray /Coords [0 0 1 0] /Function << /FunctionType 2 \
          /Domain [0 1] /C0 [0] /C1 [1] /N 1 /Range [0 1] >> /Extend [false false] >> \
          /Matrix [100 0 0 100 0 0] >>\nendobj\n",
    );
    let xref = out.len();
    out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n1 6\n");
    for offset in at.iter().take(7).skip(1) {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size 7 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    out
}

/// An image mask paints its zero bits in the graphics state's colour and leaves its one
/// bits as paper.
#[test]
fn an_image_mask_paints_its_zero_bits_and_leaves_the_rest_as_paper() {
    let render = render(mask_page("1 0 0", false), 1.0);
    assert!(
        render.notes.is_empty(),
        "a mask in a colour this can read should draw without complaint: {:?}",
        render.notes
    );
    // The zero bits are the left half of every row, and in device space that is the left
    // half of the page: image row zero is the top, so the shape runs the full height.
    assert!(
        region_is_fraction(&render.image, 0.05, 0.05, 0.45, 0.95, [255, 0, 0]),
        "the mask's zero bits are painted in the fill colour"
    );
    assert!(
        region_is_fraction(&render.image, 0.55, 0.05, 0.95, 0.95, [255, 255, 255]),
        "and its one bits are paper, not ink in the fill colour"
    );
}

/// `Do` names no colour, so the fill colour has to travel with the mark: two pages that
/// differ only in it must differ in what they paint.
#[test]
fn an_image_mask_takes_the_fill_colour_the_graphics_state_set() {
    let red = render(mask_page("1 0 0", false), 1.0);
    let blue = render(mask_page("0 0 1", false), 1.0);
    for (name, render, want) in [("red", &red, [255, 0, 0]), ("blue", &blue, [0, 0, 255])] {
        assert!(
            region_is_fraction(&render.image, 0.05, 0.05, 0.45, 0.95, want),
            "the mask is painted {name}"
        );
        assert!(
            region_is_fraction(&render.image, 0.55, 0.05, 0.95, 0.95, [255, 255, 255]),
            "and the {name} does not reach the one bits"
        );
    }
}

/// A pattern colour is a pattern, not a colour. Drawing the mask in anything else puts a
/// flat block where the page asked for a shading, so the answer is to say so.
#[test]
fn an_image_mask_in_a_pattern_colour_is_reported_rather_than_drawn() {
    let render = render(mask_page("", true), 1.0);
    assert!(
        render
            .notes
            .iter()
            .any(|n| n.contains("pattern colour") && n.contains("not drawn")),
        "the pattern colour is reported: {:?}",
        render.notes
    );
    assert!(
        region_is_fraction(&render.image, 0.05, 0.05, 0.95, 0.95, [255, 255, 255]),
        "and no block of any colour is drawn in its place"
    );
}

/// An image that is not a mask carries its own colour and is not affected by any of this.
#[test]
fn an_image_that_is_not_a_mask_is_unaffected_by_the_fill_colour() {
    let render = render(image_page(), 1.0);
    assert!(
        render.notes.is_empty(),
        "a plain image should draw without complaint: {:?}",
        render.notes
    );
    assert!(
        region_is_fraction(&render.image, 0.05, 0.05, 0.45, 0.95, [255, 0, 0]),
        "it paints its own red samples"
    );
    assert!(
        region_is_fraction(&render.image, 0.55, 0.05, 0.95, 0.95, [255, 255, 255]),
        "and leaves the rest of the page alone"
    );
}

/// A page whose content scales the coordinate system by two, clips to a quarter of that
/// space, and then fills the whole of it.
///
/// The page is 100 by 100 points. The clip is `0 0 25 25` in a space the CTM has doubled,
/// so on the page it is `0 0 50 50`: the lower left quadrant. The fill is `0 0 50 50` in
/// that same space, which is the entire page. A renderer that applied the CTM to the clip a
/// second time would find the clip larger than the fill and paint everything.
fn scaled_clip_page() -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut at = [0usize; 5];
    out.extend_from_slice(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n");
    at[1] = out.len();
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    at[2] = out.len();
    out.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 100] >>\nendobj\n",
    );
    at[3] = out.len();
    out.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n");
    let content = b"q 2 0 0 2 0 0 cm 0 0 25 25 re W n 0 0 50 50 re f Q";
    at[4] = out.len();
    let mut body = format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).into_bytes();
    body.extend_from_slice(content);
    body.extend_from_slice(b"\nendstream\nendobj\n");
    out.extend_from_slice(&body);
    let xref = out.len();
    out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n1 4\n");
    for offset in at.iter().take(5).skip(1) {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    out
}

/// An image XObject, drawn through the page's resource table.
#[test]
fn an_image_is_drawn_through_the_resources() {
    let bytes = image_page();
    let doc = open(bytes);
    let all = pages(&doc);
    let page = all.first().expect("a page");
    let resources = page
        .inherited
        .resources
        .as_ref()
        .and_then(|o| doc.resolve_object(o))
        .and_then(|o| o.as_dict().cloned())
        .map(|d| Resources::from_dict(&d, &|o| doc.resolve_object(o)))
        .unwrap_or_default();
    assert_eq!(resources.xobjects.len(), 1, "the page defines one image");

    let render = render_page(
        &doc,
        page,
        &resources,
        RenderOptions {
            scale: 1.0,
            ..RenderOptions::default()
        },
    );
    assert!(
        render.notes.is_empty(),
        "the image should draw without complaint: {:?}",
        render.notes
    );
    // A 100 by 100 point page with a red image filling its left half.
    assert!(
        region_is_fraction(&render.image, 0.05, 0.05, 0.45, 0.95, [255, 0, 0]),
        "the image fills the left half in red"
    );
    assert!(
        region_is_fraction(&render.image, 0.55, 0.05, 0.95, 0.95, [255, 255, 255]),
        "and the right half is untouched paper"
    );
}

/// A 100 by 100 point page with an axial black-to-white shading painted into a rectangle
/// filling its top half.
///
/// The pattern's own matrix scales the shading's unit axis across the rectangle, which is
/// how a page says "this gradient goes from here to there" without a coordinate system of
/// its own.
fn shading_page() -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut at = [0usize; 6];
    out.extend_from_slice(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n");
    at[1] = out.len();
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    at[2] = out.len();
    out.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 100] \
          /Resources << /Pattern << /P0 5 0 R >> >> >>\nendobj\n",
    );
    at[3] = out.len();
    out.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n");
    // Clip to the top half and paint the shading there.
    let content = b"q 0 50 100 50 re W n /P0 sh Q";
    at[4] = out.len();
    let mut body = format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).into_bytes();
    body.extend_from_slice(content);
    body.extend_from_slice(b"\nendstream\nendobj\n");
    out.extend_from_slice(&body);
    // A pattern stream whose shading runs left to right across the unit square, and whose
    // matrix stretches that square over the clipped rectangle.
    at[5] = out.len();
    out.extend_from_slice(
        b"5 0 obj\n<< /Type /Pattern /PatternType 2 /Shading << /ShadingType 2 \
          /ColorSpace /DeviceGray /Coords [0 0 1 0] /Function << /FunctionType 2 \
          /Domain [0 1] /C0 [0] /C1 [1] /N 1 /Range [0 1] >> /Extend [false false] >> \
          /Matrix [100 0 0 50 0 50] >>\nendobj\n",
    );
    let xref = out.len();
    out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n1 5\n");
    for offset in at.iter().take(6).skip(1) {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    out
}

/// An axial shading, painted through a pattern, lands where the pattern's matrix says and
/// is as dark at one end as it is light at the other.
#[test]
fn a_shading_is_painted_through_its_pattern() {
    let doc = open(shading_page());
    let all = pages(&doc);
    let page = all.first().expect("a page");
    let resources = page
        .inherited
        .resources
        .as_ref()
        .and_then(|o| doc.resolve_object(o))
        .and_then(|o| o.as_dict().cloned())
        .map(|d| Resources::from_dict(&d, &|o| doc.resolve_object(o)))
        .unwrap_or_default();
    assert_eq!(resources.patterns.len(), 1, "the page defines one pattern");

    let render = render_page(
        &doc,
        page,
        &resources,
        RenderOptions {
            scale: 1.0,
            ..RenderOptions::default()
        },
    );
    assert!(
        render.notes.is_empty(),
        "the shading should paint without complaint: {:?}",
        render.notes
    );
    // The gradient runs left to right across the top half: black at the left edge, white at
    // the right, and increasing in between. The values are the closed form at each pixel's
    // centre rather than a snapshot, so the check means the same thing at any resolution.
    let level = |x: usize| -> u8 { ((x as f64 + 0.5) / 100.0 * 255.0).round() as u8 };
    for x in [0usize, 10, 25, 50, 75, 99] {
        let got = render.image.get(x, 25).map(|p| p[0]);
        assert_eq!(
            got,
            Some(level(x)),
            "at x = {x} the gradient's parameter is {} and the colour follows it",
            (x as f64 + 0.5) / 100.0
        );
    }
    // Outside the clipped rectangle the gradient does not reach, and the paper is intact.
    assert!(
        region_is_fraction(&render.image, 0.25, 0.55, 0.75, 0.95, [255, 255, 255]),
        "below the rectangle is untouched paper"
    );
}

// ── Text, from a real font program ───────────────────────────────────────────

/// TrueType fonts to embed, in the order they are tried.
///
/// The first that exists and parses wins, so a machine with a different set of fonts still
/// runs the oracle comparison rather than skipping it. Every candidate is a real,
/// complete, unmodified font program. A test that built a font itself would be measuring
/// this renderer's agreement with a font this renderer was written against, which is not a
/// comparison — and a *fabricated* outline would render, so the comparison would then
/// measure nothing at all.
///
/// The first entry is Hack, a monospaced face derived from Bitstream Vera, which is what
/// this machine has: 309 kB.
const FONT_CANDIDATES: [&str; 5] = [
    "/usr/share/fonts/TTF/Hack-Regular.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/LiberationSans-Regular.ttf",
    "/Library/Fonts/Arial.ttf",
];

/// The bytes of a TrueType font to embed, or the reason there are none.
fn true_type_font() -> Result<Vec<u8>, String> {
    for path in FONT_CANDIDATES {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        // A file that does not parse is worse than no file: the test would go on to
        // compare a blank page against a drawn one and report a difference between the
        // renderers rather than between this machine and the candidate list.
        if mangle_font::Program::new(bytes.clone())
            .units_per_em()
            .is_some()
        {
            return Ok(bytes);
        }
    }
    Err(format!(
        "no TrueType font found; looked in {}",
        FONT_CANDIDATES.join(", ")
    ))
}

/// A page with a known string set in an embedded TrueType font.
///
/// Everything a viewer needs is here and nothing else is: a font dictionary, a descriptor,
/// a `/FontFile2` carrying the whole program, and a content stream that shows one string.
///
/// The font is embedded whole rather than subset, 309 kB against a few hundred bytes, and
/// that is the right trade for a fixture. Subsetting is a real feature with a real chance
/// of being wrong, and a fixture that depended on it would be testing the subsetter as
/// much as the renderer; a fixture wants the fewest moving parts between the file and the
/// pixels.
///
/// The program is written into the stream as it is, uncompressed, and that is a choice
/// worth stating. `/FlateDecode` is the obvious way to shrink it and it is what a real
/// writer does, but this fixture is not testing filters: a page whose font is unreadable
/// by the oracle would compare a drawn page against a differently-drawn one and the score
/// would be about the font loader rather than about the outlines. `/Length` is written
/// correctly, so a reader that believes it has no trouble with binary bytes.
fn text_page(font: &[u8], text: &str, size: f64) -> Vec<u8> {
    let mut program = mangle_font::Program::new(font.to_vec());
    let units = program.units_per_em().unwrap_or(1000);
    // `/Widths` in thousandths of an em, from the font's own advances, over the ASCII range.
    // Both renderers read this array, so the pen walks the same distance on both and the
    // comparison is about glyph shapes rather than about where each put the second letter.
    let widths: Vec<String> = (32u8..=126)
        .map(|code| {
            program
                .glyph_for_code(u32::from(code))
                .and_then(|g| program.advance(g))
                .map(|a| u32::from(a) * 1000 / u32::from(units))
                .unwrap_or(500)
                .to_string()
        })
        .collect();
    let widths = widths.join(" ");

    let packed = font.to_vec();

    let mut out: Vec<u8> = Vec::new();
    let mut at = [0usize; 8];
    out.extend_from_slice(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n");
    at[1] = out.len();
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    at[2] = out.len();
    out.extend_from_slice(
        // A 400 point page, which is not a multiple of 0.48 — the resolution a 150 DPI
        // render divides by — so both renderers round the pixel count the same way. A page
        // width that lands exactly on a whole number of pixels at 150 DPI is a case where
        // `ceil` of a product that is a hair over an integer in binary disagrees with the
        // oracle by one pixel, and a comparison that refuses on a size mismatch would then
        // measure that rather than the glyphs.
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 400 100] \
          /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n",
    );
    at[3] = out.len();
    out.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n");
    at[4] = out.len();
    let content = format!("BT 0 0 0 rg /F1 {size} Tf 18 60 Td ({text}) Tj ET");
    let mut body = format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).into_bytes();
    body.extend_from_slice(content.as_bytes());
    body.extend_from_slice(b"\nendstream\nendobj\n");
    out.extend_from_slice(&body);

    at[5] = out.len();
    out.extend_from_slice(
        format!(
            "5 0 obj\n<< /Type /Font /Subtype /TrueType /BaseFont /Embedded /FirstChar 32 \
             /LastChar 126 /Widths [{widths}] /Encoding /WinAnsiEncoding /FontDescriptor \
             6 0 R >>\nendobj\n"
        )
        .as_bytes(),
    );
    at[6] = out.len();
    out.extend_from_slice(
        b"6 0 obj\n<< /Type /FontDescriptor /FontName /Embedded /Flags 32 /FontBBox \
          [0 -200 1000 800] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 \
          /StemV 80 /MissingWidth 500 /FontFile2 7 0 R >>\nendobj\n",
    );
    at[7] = out.len();
    let mut file = format!(
        "7 0 obj\n<< /Length {} /Length1 {} >>\nstream\n",
        packed.len(),
        font.len()
    )
    .into_bytes();
    file.extend_from_slice(&packed);
    file.extend_from_slice(b"\nendstream\nendobj\n");
    out.extend_from_slice(&file);

    let xref = out.len();
    out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n1 7\n");
    for offset in at.iter().take(8).skip(1) {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size 8 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    out
}

/// The text page, rendered, or the reason there is none.
fn render_text_page(
    text: &str,
    size: f64,
    scale: f64,
) -> Result<mangle_render::PageRender, String> {
    let font = true_type_font()?;
    let doc = open(text_page(&font, text, size));
    let all = pages(&doc);
    let page = all.first().expect("a page");
    let resources = page
        .inherited
        .resources
        .as_ref()
        .and_then(|o| doc.resolve_object(o))
        .and_then(|o| o.as_dict().cloned())
        .map(|d| Resources::from_dict(&d, &|o| doc.resolve_object(o)))
        .unwrap_or_default();
    Ok(render_page(
        &doc,
        page,
        &resources,
        RenderOptions {
            scale,
            ..RenderOptions::default()
        },
    ))
}

/// How much ink is on the page, summed over every pixel.
fn total_ink(render: &mangle_render::PageRender) -> u64 {
    (0..render.image.height)
        .flat_map(|y| (0..render.image.width).map(move |x| (x, y)))
        .map(|(x, y)| u64::from(darkness(&render.image, x, y)))
        .sum()
}

/// A glyph the font does not draw leaves the page worth showing.
///
/// A space, and a code no subtable answers, are the common case in a document and are not
/// failures. The assertion is that the note list stays empty and the letters that *are*
/// there are drawn: a renderer that reported a missing space on every word would make the
/// notes the only thing a user ever saw.
#[test]
fn a_glyph_with_no_outline_draws_nothing_and_says_nothing() {
    let spaced = match render_text_page("A A", 48.0, 2.0) {
        Ok(render) => render,
        Err(reason) => {
            eprintln!("skipped: {reason}");
            return;
        }
    };
    assert!(
        spaced.notes.is_empty(),
        "a space and two drawn letters are not findings: {:?}",
        spaced.notes
    );
    let tight = render_text_page("AA", 48.0, 2.0).expect("a font");

    // The same two letters, so the same ink, with the spaces only moving them apart. This
    // is the space drawn: a space has an advance and no outline, and a renderer that gave
    // it an outline would make this ratio wrong.
    let (a, b) = (total_ink(&tight) as f64, total_ink(&spaced) as f64);
    assert!(a > 0.0, "the letters are drawn at all");
    assert!(
        (a / b - 1.0).abs() < 0.02,
        "two letters have the same ink whether or not a space separates them, \
         got {a} against {b}"
    );
}

/// Four times the scale is sixteen times the ink.
///
/// Ink is an area, so it goes with the square of the scale: twice the scale is four times
/// the ink and four times the scale is sixteen. A check expecting a factor of *four* at
/// four times the scale would be asserting that area does not scale with length — it would
/// pass on a renderer that drew the same number of pixels at every zoom, and fail on a
/// correct one. The factor of four is the two-times case, and it is checked too.
#[test]
fn glyph_ink_scales_with_the_square_of_the_scale() {
    let one = match render_text_page("H", 40.0, 1.0) {
        Ok(render) => render,
        Err(reason) => {
            eprintln!("skipped: {reason}");
            return;
        }
    };
    let two = render_text_page("H", 40.0, 2.0).expect("a font");
    let four = render_text_page("H", 40.0, 4.0).expect("a font");

    let (a, b, c) = (
        total_ink(&one) as f64,
        total_ink(&two) as f64,
        total_ink(&four) as f64,
    );
    assert!(a > 0.0, "the letter is drawn at all");
    for (measured, want, scale) in [(b / a, 4.0, 2.0), (c / a, 16.0, 4.0)] {
        let error = (measured - want).abs() / want;
        assert!(
            error < 0.05,
            "at {scale} times the scale the ink should be {want} times as much; it is \
             {measured}, which is {:.1}% out",
            error * 100.0
        );
    }
}

/// The comparison that says the glyphs are the right shape.
///
/// A rendering that has never been compared with another renderer is a guess. `mutool` is a
/// different codebase with a decade of accumulated knowledge of what a page is meant to look
/// like, and an SSIM against it is a statement about the outlines, the `cmap` lookup, the em
/// scale, the placement matrix and the fill rule all at once, and about nothing else.
#[test]
fn our_glyphs_agree_with_mutools() {
    if mutool().is_none() {
        eprintln!("skipped: mutool is not installed");
        return;
    }
    let font = match true_type_font() {
        Ok(font) => font,
        Err(reason) => {
            eprintln!("skipped: {reason}");
            return;
        }
    };
    let text = "Hamburgefonstiv";
    let dir = std::env::temp_dir().join("mangle-render-oracle");
    std::fs::create_dir_all(&dir).expect("a place to work");
    let pdf = dir.join("text.pdf");
    std::fs::write(&pdf, text_page(&font, text, 36.0)).expect("a file to render");

    let scale = 150.0 / 72.0;
    let ours = render_text_page(text, 36.0, scale).expect("a font");
    assert!(
        ours.notes.is_empty(),
        "an embedded font should draw without complaint: {:?}",
        ours.notes
    );
    let theirs_path = dir.join("text.pam");
    let Some(data) = mutool_render(&pdf, scale, &theirs_path) else {
        eprintln!("skipped: mutool could not render the page");
        return;
    };
    let Some((w, h, depth, body)) = read_pam(&data) else {
        eprintln!("skipped: could not read mutool's output");
        return;
    };
    let theirs = flatten_onto_paper(&pam_to_image(w, h, depth, body));

    // mutool must have drawn the text too, or this compares a blank page against a drawn
    // one and the score says nothing at all.
    let mut their_ink = 0u64;
    for y in 0..theirs.height {
        for x in 0..theirs.width {
            their_ink += u64::from(darkness(&theirs, x, y));
        }
    }
    assert!(
        their_ink > 0,
        "mutool drew no text, so there is nothing here to compare against"
    );

    let comparison = compare(&ours.image, &theirs, &SsimOptions::default());
    assert!(
        comparison.is_valid(),
        "the comparison did not happen: {:?}",
        comparison.refused
    );
    let metrics = &comparison.metrics;
    eprintln!(
        "text page: {} — our ink {}, theirs {}",
        metrics.summary(),
        total_ink(&ours),
        their_ink
    );
    assert!(
        metrics.meets_fidelity_bar(0.95),
        "our render scores {:.4} against mutool, below the 0.95 fidelity bar: {}",
        metrics.ssim,
        metrics.summary()
    );
}

// ── Composite fonts: two-byte codes ───────────────────────────────────────────

/// The three widths the composite fixture declares, in thousandths of an em.
///
/// Deliberately all different, and deliberately not the font's own advances: this face is
/// monospaced, so a fixture that used them would have three equal advances and could not
/// tell a renderer that read `/W` from one that used a single figure for the run. Both
/// renderers read `/W`, so a difference between them is a difference in how they read it.
///
/// Each is wider than the face's own 602/1000 em, so one glyph's ink cannot reach into the
/// next glyph's cell and the three can be told apart by where they are.
const COMPOSITE_WIDTHS: [u32; 3] = [700, 950, 800];

/// The CIDs the composite fixture shows: three different glyphs.
const COMPOSITE_CIDS: [u32; 3] = [12, 151, 103];

/// A page set in a composite (Type 0) font, with `/Identity-H` and an embedded program.
///
/// Everything a composite font needs and nothing else: the `/Type0` dictionary that says
/// the codes are two bytes, a descendant `/CIDFontType2` that carries the `/W` widths and
/// the `/FontDescriptor`, and the whole program in a `/FontFile2`.
///
/// The codes are CIDs, and the font is used as its own `/CIDToGIDMap` — the specification's
/// default, and what a CID font with no map means — so CID *n* is glyph *n*.
///
/// The `/W` array is written in its run-length form and mixes both of that form's shapes: a
/// listed run for the codes in use, and a range run above them, so the boundary between the
/// two is on the page and both are read.
fn composite_page(font: &[u8], cids: &[u32], size: f64) -> Vec<u8> {
    let first = cids.first().copied().unwrap_or(0);
    let listed: Vec<String> = cids
        .iter()
        .enumerate()
        .map(|(i, _)| COMPOSITE_WIDTHS.get(i).copied().unwrap_or(500).to_string())
        .collect();
    // The range run starts one code above the last one in use, so the listed run's last
    // code is the last code the listed run answers for.
    let after = u32::from(u16::try_from(first + cids.len() as u32).unwrap_or(1));
    let w = format!("[ {first} [ {} ] {after} 65535 1000 ]", listed.join(" "));
    // The shown string, two bytes per code, high byte first.
    let mut shown = String::new();
    for cid in cids {
        let _ = write!(shown, "{cid:04X}");
    }

    let mut out: Vec<u8> = Vec::new();
    let mut at = [0usize; 9];
    out.extend_from_slice(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n");
    at[1] = out.len();
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    at[2] = out.len();
    out.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 400 100] \
          /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n",
    );
    at[3] = out.len();
    out.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n");
    at[4] = out.len();
    let content = format!("BT 0 0 0 rg /F1 {size} Tf 18 60 Td <{shown}> Tj ET");
    let mut body = format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).into_bytes();
    body.extend_from_slice(content.as_bytes());
    body.extend_from_slice(b"\nendstream\nendobj\n");
    out.extend_from_slice(&body);

    at[5] = out.len();
    out.extend_from_slice(
        b"5 0 obj\n<< /Type /Font /Subtype /Type0 /BaseFont /Embedded /Encoding \
          /Identity-H /DescendantFonts [6 0 R] >>\nendobj\n",
    );
    at[6] = out.len();
    out.extend_from_slice(
        format!(
            "6 0 obj\n<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Embedded \
             /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> \
             /DW 1000 /W {w} /CIDToGIDMap /Identity /FontDescriptor 7 0 R >>\nendobj\n"
        )
        .as_bytes(),
    );
    at[7] = out.len();
    out.extend_from_slice(
        b"7 0 obj\n<< /Type /FontDescriptor /FontName /Embedded /Flags 4 /FontBBox \
          [0 -200 1000 800] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 \
          /StemV 80 /MissingWidth 500 /FontFile2 8 0 R >>\nendobj\n",
    );
    at[8] = out.len();
    out.extend_from_slice(
        format!(
            "8 0 obj\n<< /Length {} /Length1 {} >>\nstream\n",
            font.len(),
            font.len()
        )
        .as_bytes(),
    );
    out.extend_from_slice(font);
    out.extend_from_slice(b"\nendstream\nendobj\n");

    let xref = out.len();
    out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n1 8\n");
    for offset in at.iter().take(9).skip(1) {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size 9 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    out
}

/// The composite page, rendered, or the reason there is none.
fn render_composite_page(
    cids: &[u32],
    size: f64,
    scale: f64,
) -> Result<mangle_render::PageRender, String> {
    let font = true_type_font()?;
    let doc = open(composite_page(&font, cids, size));
    let all = pages(&doc);
    let page = all.first().expect("a page");
    let resources = page
        .inherited
        .resources
        .as_ref()
        .and_then(|o| doc.resolve_object(o))
        .and_then(|o| o.as_dict().cloned())
        .map(|d| Resources::from_dict(&d, &|o| doc.resolve_object(o)))
        .unwrap_or_default();
    Ok(render_page(
        &doc,
        page,
        &resources,
        RenderOptions {
            scale,
            ..RenderOptions::default()
        },
    ))
}

/// How much ink is in a band of the page, given in page fractions of x.
///
/// The band is the whole height, because the claim is about where the glyphs are
/// horizontally and a band that also said something about their height would be testing
/// two things at once.
fn ink_in_columns(image: &mangle_render::Image, fx0: f64, fx1: f64) -> u64 {
    let (x0, _, x1, _) = region(image, fx0, 0.0, fx1, 1.0);
    (0..image.height)
        .flat_map(|y| (x0..x1).map(move |x| (x, y)))
        .map(|(x, y)| u64::from(darkness(image, x, y)))
        .sum()
}

/// Two codes in a composite font are two glyphs, at the positions their own widths say.
///
/// The count is the assertion that matters. A renderer that treated the four bytes of a
/// string as four one-byte codes would draw four glyphs — or fewer, if the stray high bytes
/// name nothing in the font — and the ink and the positions would both be wrong.
///
/// The positions are checked against the widths the fixture declared rather than against
/// the font's own advances, so a renderer that used one figure for the whole run, or that
/// read the widths one byte at a time, puts the second glyph somewhere else and fails here.
#[test]
fn a_composite_font_draws_one_glyph_per_two_byte_code_at_its_own_width() {
    if let Err(reason) = true_type_font() {
        eprintln!("skipped: {reason}");
        return;
    }
    let size = 36.0;
    let scale = 2.0;
    let cids = COMPOSITE_CIDS;
    let render = match render_composite_page(&cids, size, scale) {
        Ok(render) => render,
        Err(reason) => {
            eprintln!("skipped: {reason}");
            return;
        }
    };
    assert!(
        render.notes.is_empty(),
        "an embedded composite font should draw without complaint: {:?}",
        render.notes
    );

    // The pen starts 18 points in and each glyph is its own declared width, in ems of the
    // 36 point size. The page is 400 points wide, so the fractions below are where the
    // pen must be — and the three widths are all different, so a renderer that placed the
    // glyphs evenly would put the third in the wrong place.
    let page = 400.0;
    let start = 18.0;
    let mut at = start;
    let mut bands = Vec::new();
    for (i, w) in COMPOSITE_WIDTHS.iter().enumerate() {
        let left = at;
        let right = at + f64::from(*w) / 1000.0 * size;
        let ink = ink_in_columns(&render.image, left / page + 0.01, right / page - 0.01);
        assert!(
            ink > 0,
            "glyph {i} has ink between x {left} and {right}, its own declared width"
        );
        bands.push((left / page, right / page));
        at = right;
    }
    // Three glyphs and no more: the ink beyond the last one's advance is paper. A renderer
    // that split the bytes would have put a fourth glyph out here, and one that advanced by
    // the first width for every glyph would have drawn them all on top of each other.
    let past = ink_in_columns(&render.image, at / page + 0.02, 0.95);
    assert_eq!(
        past, 0,
        "nothing is drawn past the last glyph's own advance, which ends at {at}"
    );
    // The gap between two glyphs' bands is the difference between one advance and the
    // next, and it is not the same for either pair — which is what a run of equal advances
    // would produce.
    let gap = |(a, b): (f64, f64), (c, _): (f64, f64)| c - b - (b - a);
    assert!(
        (gap(bands[0], bands[1]) - gap(bands[1], bands[2])).abs() > 0.01,
        "the two gaps differ, so an evenly-spaced run would not pass: {bands:?}"
    );
}

/// A composite-font page compared with `mutool`.
///
/// The fixture is deliberately **asymmetric**: three CIDs of different shapes at three
/// different positions on a page that is 400 by 100, and the `/W` array mixes the listed
/// and ranged forms. A symmetric fixture — the same glyph repeated, centred, evenly spaced —
/// would score well against a renderer that drew the *wrong* glyphs, because the error
/// would be a mirror image of itself. Asymmetry is what makes the score mean something: a
/// glyph drawn at its neighbour's position, or a code split into two bytes, moves ink to
/// where there is none, and SSIM notices.
#[test]
fn our_composite_glyphs_agree_with_mutools() {
    if mutool().is_none() {
        eprintln!("skipped: mutool is not installed");
        return;
    }
    let font = match true_type_font() {
        Ok(font) => font,
        Err(reason) => {
            eprintln!("skipped: {reason}");
            return;
        }
    };
    // Three different glyphs, so the page has no symmetry a wrong answer could match.
    let cids = COMPOSITE_CIDS;
    let dir = std::env::temp_dir().join("mangle-render-oracle");
    std::fs::create_dir_all(&dir).expect("a place to work");
    let pdf = dir.join("composite.pdf");
    std::fs::write(&pdf, composite_page(&font, &cids, 36.0)).expect("a file to render");

    let scale = 150.0 / 72.0;
    let ours = render_composite_page(&cids, 36.0, scale).expect("a font");
    assert!(
        ours.notes.is_empty(),
        "an embedded composite font should draw without complaint: {:?}",
        ours.notes
    );
    let theirs_path = dir.join("composite.pam");
    let Some(data) = mutool_render(&pdf, scale, &theirs_path) else {
        eprintln!("skipped: mutool could not render the page");
        return;
    };
    let Some((w, h, depth, body)) = read_pam(&data) else {
        eprintln!("skipped: could not read mutool's output");
        return;
    };
    let theirs = flatten_onto_paper(&pam_to_image(w, h, depth, body));

    let mut their_ink = 0u64;
    for y in 0..theirs.height {
        for x in 0..theirs.width {
            their_ink += u64::from(darkness(&theirs, x, y));
        }
    }
    assert!(
        their_ink > 0,
        "mutool drew no text, so there is nothing here to compare against"
    );

    let comparison = compare(&ours.image, &theirs, &SsimOptions::default());
    assert!(
        comparison.is_valid(),
        "the comparison did not happen: {:?}",
        comparison.refused
    );
    let metrics = &comparison.metrics;
    eprintln!(
        "composite page: {} — our ink {}, theirs {}",
        metrics.summary(),
        total_ink(&ours),
        their_ink
    );
    assert!(
        metrics.meets_fidelity_bar(0.95),
        "our render scores {:.4} against mutool, below the 0.95 fidelity bar: {}",
        metrics.ssim,
        metrics.summary()
    );
}

/// A clip under a scaled coordinate system scales with it.
///
/// The interpreter's bounds already carry the CTM, so the renderer must not apply it again.
/// Under an identity CTM the two agree, which is why a test at 72 DPI with no scaling could
/// never have found this.
#[test]
fn a_clip_under_a_scaled_ctm_is_not_transformed_twice() {
    let render = render(scaled_clip_page(), 1.0);
    // The clip is `0 0 25 25` in a space doubled by the CTM, so on the page it is
    // `0 0 50 50`: the lower left quadrant, because the canvas counts y downward.
    assert!(
        region_is_fraction(&render.image, 0.05, 0.55, 0.45, 0.95, [0, 0, 0]),
        "the lower left quadrant is inside the clip"
    );
    for (x0, y0, x1, y1, which) in [
        (0.55f64, 0.55f64, 0.95f64, 0.95f64, "the lower right"),
        (0.05, 0.05, 0.45, 0.45, "the upper left"),
        (0.55, 0.05, 0.95, 0.45, "the upper right"),
    ] {
        assert!(
            region_is_fraction(&render.image, x0, y0, x1, y1, [255, 255, 255]),
            "{which} is outside the clip and is still paper; a clip applied twice would be \
             the whole page and every quadrant would be black"
        );
    }
}

// ── CFF fonts: Type 2 charstrings, executed rather than walked ─────────────────────

/// Where a CFF font might be, most-likely first.
///
/// The whole program is embedded rather than subset, up to two megabytes, and that is the
/// right trade for a fixture: subsetting is a real feature with a real chance of being wrong,
/// and a fixture that depended on it would be testing the subsetter as much as the renderer.
const CFF_CANDIDATES: [&str; 6] = [
    "/usr/share/fonts/gnu-free/FreeSerif.otf",
    "/usr/share/fonts/gnu-free/FreeSans.otf",
    "/usr/share/fonts/gnu-free/FreeSerifBold.otf",
    "/usr/share/fonts/gsfonts/NimbusSans-Regular.otf",
    "/usr/share/fonts/gsfonts/NimbusRoman-Regular.otf",
    "/usr/share/fonts/gsfonts/URWBookman-Demi.otf",
];

/// A CFF font program to embed, or the reason there is none.
///
/// A file that parses is the only one that goes in: a fixture built round a font this cannot
/// read would compare a blank page against a drawn one and report a difference between the
/// renderers rather than between this reader and the machine.
fn cff_font() -> Result<Vec<u8>, String> {
    for path in CFF_CANDIDATES {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        if mangle_font::cff::cff_bytes(&bytes).is_some() {
            return Ok(bytes);
        }
    }
    Err(format!(
        "no CFF font found; looked in {}",
        CFF_CANDIDATES.join(", ")
    ))
}

/// The `CFF ` table out of an OpenType-CFF program, or the whole program back.
///
/// A `/FontFile3` is written in one of two shapes and the test uses both. The bare table,
/// `/Subtype /Type1C`, is a font that has no `sfnt` wrapper and so no `cmap` and no `hmtx`
/// beside it; the wrapped one, `/Subtype /OpenType`, is the whole `sfnt`. They are the same
/// outlines, and a reader that handled only one of them would draw half the embedded CFF
/// fonts in the world.
fn bare_cff(program: &[u8]) -> Option<Vec<u8>> {
    mangle_font::cff::cff_bytes(program).map(<[u8]>::to_vec)
}

/// One line of text: its text, its size, and where its baseline starts.
struct Line {
    text: &'static str,
    size: f64,
    at: (f64, f64),
}

/// The lines the CFF fixture shows, in the order the content stream writes them.
///
/// **Deliberately asymmetric, and the asymmetry is the point.** Three lines of different
/// lengths, at three different sizes, starting at three different x positions, with a filled
/// rectangle in one corner. A symmetric fixture — the same word repeated, centred, evenly
/// spaced, on white — scores well against a renderer that draws the *wrong* glyphs, because
/// the error is a mirror image of itself and the two cancel. This one cannot be got right by
/// accident: a glyph drawn at its neighbour's position, a charstring whose width was read off
/// the wrong end of the stack, or an outline scaled by the em twice, each moves ink to where
/// there is none and SSIM notices.
const CFF_LINES: [Line; 3] = [
    Line {
        text: "Hamburgefonstiv",
        size: 36.0,
        at: (18.0, 96.0),
    },
    Line {
        text: "Wxyz 0189 @",
        size: 20.0,
        at: (31.0, 58.0),
    },
    Line {
        text: "Qg",
        size: 48.0,
        at: (268.0, 34.0),
    },
];

/// A page set in an embedded CFF font, with a shape in one corner.
///
/// The shape is there so that part of the page is *not* text. Without it the comparison is
/// only ever about glyphs, and a fixture that cannot tell "nothing was drawn" from "the page
/// is blank" is a fixture that scores well when everything fails.
fn cff_page(program: &[u8], subtype: &str, packed: &[u8]) -> Vec<u8> {
    let reader = mangle_font::Program::new(program.to_vec());
    // `/Widths` in thousandths of an em, from the font's own advances, over the ASCII range.
    // Both renderers read this array, so the pen walks the same distance on both and the
    // comparison is about glyph shapes rather than about where each put the second letter.
    let mut widths = Vec::with_capacity(95);
    for code in 32u8..=126 {
        let width = reader
            .glyph_for_code(u32::from(code))
            .and_then(|glyph| reader.advance(glyph))
            .unwrap_or(500);
        widths.push(width.to_string());
    }
    let widths = widths.join(" ");

    let mut content = String::from("0.86 0.86 0.86 rg 300 8 92 26 re f\n");
    for line in &CFF_LINES {
        // `writeln!` because the line ends in a newline, and a format string that carries its
        // own trailing newline is the same thing said twice.
        let _ = writeln!(
            content,
            "BT 0 0 0 rg /F1 {} Tf {} {} Td ({}) Tj ET",
            line.size, line.at.0, line.at.1, line.text
        );
    }

    let mut out: Vec<u8> = Vec::new();
    let mut at = [0usize; 8];
    out.extend_from_slice(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n");
    at[1] = out.len();
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    at[2] = out.len();
    out.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 400 130] \
          /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n",
    );
    at[3] = out.len();
    out.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n");
    at[4] = out.len();
    let mut body = format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).into_bytes();
    body.extend_from_slice(content.as_bytes());
    body.extend_from_slice(b"\nendstream\nendobj\n");
    out.extend_from_slice(&body);

    at[5] = out.len();
    // `/Subtype /TrueType` with a `/FontFile3` is the PDF's name for an OpenType-CFF program:
    // the outlines are CFF but the font is still a simple font, addressed by one-byte codes
    // through `/Encoding`. A `/Subtype /CIDFontType0C` descendant is CID-keyed and is a
    // different reader's job; see the note in the test below.
    out.extend_from_slice(
        format!(
            "5 0 obj\n<< /Type /Font /Subtype /TrueType /BaseFont /Embedded /FirstChar 32 \
             /LastChar 126 /Widths [{widths}] /Encoding /WinAnsiEncoding /FontDescriptor \
             6 0 R >>\nendobj\n"
        )
        .as_bytes(),
    );
    at[6] = out.len();
    out.extend_from_slice(
        b"6 0 obj\n<< /Type /FontDescriptor /FontName /Embedded /Flags 4 /FontBBox \
          [-212 -293 1043 1085] /ItalicAngle 0 /Ascent 900 /Descent -300 /CapHeight 700 \
          /StemV 80 /MissingWidth 500 /FontFile3 7 0 R >>\nendobj\n",
    );
    at[7] = out.len();
    // `/Length1` is the length of the *uncompressed* program, which is what tells a reader
    // the stream really is a font program and not, say, an image. It is the same number here
    // because the stream is written uncompressed.
    let mut file = format!(
        "7 0 obj\n<< /Length {} /Length1 {} /Subtype /{} >>\nstream\n",
        packed.len(),
        packed.len(),
        subtype
    )
    .into_bytes();
    file.extend_from_slice(packed);
    file.extend_from_slice(b"\nendstream\nendobj\n");
    out.extend_from_slice(&file);

    let xref = out.len();
    out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n1 7\n");
    for offset in at.iter().take(8).skip(1) {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size 8 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    out
}

/// A page in a CFF font, rendered, or the reason there is none.
fn render_cff_page(pdf_bytes: Vec<u8>, scale: f64) -> Result<mangle_render::PageRender, String> {
    let doc = open(pdf_bytes);
    let all = pages(&doc);
    let page = all.first().ok_or("the page has no page tree")?;
    let resources = page
        .inherited
        .resources
        .as_ref()
        .and_then(|o| doc.resolve_object(o))
        .and_then(|o| o.as_dict().cloned())
        .map(|d| Resources::from_dict(&d, &|o| doc.resolve_object(o)))
        .unwrap_or_default();
    Ok(render_page(
        &doc,
        page,
        &resources,
        RenderOptions {
            scale,
            ..RenderOptions::default()
        },
    ))
}

/// The shared comparison: our render, `mutool`'s, and the score between them.
///
/// `mutool` must have drawn something, or the score is a statement about two blank pages
/// rather than about glyphs, so its ink is asserted before the comparison happens.
fn score_against_mutool(
    ours: &mangle_render::PageRender,
    pdf: &Path,
    scale: f64,
    label: &str,
) -> Option<mangle_render::Comparison> {
    let dir = std::env::temp_dir().join("mangle-render-oracle");
    std::fs::create_dir_all(&dir).expect("a place to work");
    let theirs_path = pdf.with_extension("pam");
    let Some(data) = mutool_render(pdf, scale, &theirs_path) else {
        eprintln!("skipped: mutool could not render the page");
        return None;
    };
    let Some((w, h, depth, body)) = read_pam(&data) else {
        eprintln!("skipped: could not read mutool's output");
        return None;
    };
    let theirs = flatten_onto_paper(&pam_to_image(w, h, depth, body));
    let their_ink: u64 = (0..theirs.height)
        .flat_map(|y| (0..theirs.width).map(move |x| (x, y)))
        .map(|(x, y)| u64::from(darkness(&theirs, x, y)))
        .sum();
    assert!(
        their_ink > 0,
        "mutool drew nothing, so there is nothing to compare against"
    );

    let comparison = compare(&ours.image, &theirs, &SsimOptions::default());
    assert!(
        comparison.is_valid(),
        "the comparison did not happen: {:?}",
        comparison.refused
    );
    eprintln!(
        "{label}: {} — our ink {}, theirs {their_ink}",
        comparison.metrics.summary(),
        total_ink(ours)
    );
    Some(comparison)
}

/// An embedded CFF page, compared with `mutool`.
///
/// This is the check that says something about *correctness*. Every other CFF test here is
/// self-consistent, and a self-consistent charstring interpreter can be self-consistently
/// wrong — reading the width off the wrong end of its stack, applying the em twice, skipping
/// a hint mask at the wrong length. `mutool` is a different codebase with a decade of
/// accumulated knowledge of what a glyph is meant to look like, and an SSIM against it is a
/// statement about the outlines, the code lookup, the em scale, the placement matrix and the
/// fill rule all at once, and about nothing else.
#[test]
fn our_cff_glyphs_agree_with_mutools() {
    if mutool().is_none() {
        eprintln!("skipped: mutool is not installed");
        return;
    }
    let font = match cff_font() {
        Ok(font) => font,
        Err(reason) => {
            eprintln!("skipped: {reason}");
            return;
        }
    };
    let dir = std::env::temp_dir().join("mangle-render-oracle");
    std::fs::create_dir_all(&dir).expect("a place to work");
    let pdf = dir.join("cff.pdf");
    let bytes = cff_page(&font, "OpenType", &font);
    std::fs::write(&pdf, &bytes).expect("a file to render");

    let scale = 150.0 / 72.0;
    let ours = render_cff_page(bytes, scale).expect("a font");
    assert!(
        ours.notes.is_empty(),
        "an embedded CFF font should draw without complaint: {:?}",
        ours.notes
    );
    let Some(comparison) = score_against_mutool(&ours, &pdf, scale, "CFF page") else {
        return;
    };
    assert!(
        comparison.metrics.meets_fidelity_bar(0.95),
        "our render scores {:.4} against mutool, below the 0.95 fidelity bar: {}",
        comparison.metrics.ssim,
        comparison.metrics.summary()
    );
}

// There is deliberately no oracle test for the bare `/Subtype /Type1C` form.
//
// `mutool` resolves a bare CFF font's character codes through the charset and the standard
// encoding. This renderer resolves them through the charset too, but the same glyphs are
// already compared against `mutool` through the `sfnt` wrapper's `cmap` (the test above), and
// comparing the *same outlines* twice would measure the page fixture rather than the charset.
// What is tested below is the thing a charset reader can get quietly wrong: whether the codes
// reach a glyph at all.

/// A bare CFF font whose codes name glyphs through an `/Encoding` draws them.
///
/// A `/Subtype /Type1C` program is the font with no `sfnt` wrapper and so no `cmap`: the codes
/// in a content stream name glyphs through the font's `/Encoding`, and the `charset` is what
/// resolves those names to glyph numbers. This used to be a refusal, with the reason on the
/// page, because the charset was not read; it is read now, in all three of its formats.
///
/// So this asserts the thing the reading is *for*: the page has ink in it, and the notes say
/// nothing about a charset. The failure this guards against is the one a charset reader can
/// produce quietly — a lookup that returns `None` for every code, which draws nothing and
/// looks exactly like the refusal it replaced.
#[test]
fn a_bare_name_keyed_cff_font_draws_through_its_charset() {
    let font = match cff_font() {
        Ok(font) => font,
        Err(reason) => {
            eprintln!("skipped: {reason}");
            return;
        }
    };
    let Some(bare) = bare_cff(&font) else {
        eprintln!("skipped: the font has no `CFF ` table to unwrap");
        return;
    };
    let render = render_cff_page(cff_page(&font, "Type1C", &bare), 2.0).expect("a font");
    assert!(
        !render
            .notes
            .iter()
            .any(|note| note.contains("charset") && note.contains("does not read")),
        "a readable charset must not produce a refusal: {:?}",
        render.notes
    );
    // And the page is not blank. The fixture draws a filled rectangle as well as three lines
    // of text, so this counts every non-white pixel rather than only the text: a charset that
    // resolved nothing would leave the rectangle and lose the words.
    let ink = render
        .image
        .pixels
        .chunks_exact(4)
        .filter(|px| px[0] < 250 || px[1] < 250 || px[2] < 250)
        .count();
    assert!(
        ink > 1_000,
        "only {ink} non-white pixels: the page's text is not being drawn"
    );
}

// ── Type 1 fonts: PostScript, encrypted twice ─────────────────────────────────────

/// Where a Type 1 font might be, most-likely first.
///
/// Ghostscript's resource directory holds the URW and Nimbus faces as **bare PFA programs**,
/// which is exactly what a `/FontFile` carries. A Linux distribution's own `type1`
/// directory is usually empty — the outlines ship as OpenType — so Ghostscript's copy is
/// where a real one is found.
const TYPE1_CANDIDATES: [&str; 5] = [
    "/usr/share/ghostscript/Resource/Font/NimbusSans-Regular",
    "/usr/share/ghostscript/Resource/Font/NimbusRoman-Regular",
    "/usr/share/ghostscript/Resource/Font/NimbusMonoPS-Regular",
    "/usr/share/ghostscript/Resource/Font/URWGothic-Book",
    "/usr/share/ghostscript/Resource/Font/P052-Roman",
];

/// A Type 1 font program to embed in a `/FontFile`, or the reason there is none.
fn type1_font() -> Result<Vec<u8>, String> {
    for path in TYPE1_CANDIDATES {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        if mangle_font::Program::new(bytes.clone()).inspect().is_ok() {
            return Ok(bytes);
        }
    }
    Err(format!(
        "no Type 1 font found; looked in {}",
        TYPE1_CANDIDATES.join(", ")
    ))
}

/// The lines the Type 1 fixture shows, in the order the content stream writes them.
///
/// **Deliberately asymmetric, and the asymmetry is the point.** Three lines of different
/// lengths at three different sizes, starting at three different x positions, and a filled
/// rectangle in one corner. A symmetric fixture — the same word repeated, centred, evenly
/// spaced, on white — scores well against a renderer that draws the *wrong* glyphs, because
/// the error is a mirror image of itself and the two cancel. This one cannot be got right by
/// accident: a glyph offset by a mis-decrypted `eexec` layer, a first `rrcurveto` whose last
/// y was read as a delta rather than a position, or an outline scaled by the em twice, each
/// moves ink to where there is none and SSIM notices.
const TYPE1_LINES: [Line; 3] = [
    Line {
        text: "Hamburgefonstiv",
        size: 34.0,
        at: (18.0, 96.0),
    },
    Line {
        text: "Qqjx 0258 @",
        size: 19.0,
        at: (33.0, 57.0),
    },
    Line {
        text: "Bg",
        size: 52.0,
        at: (274.0, 33.0),
    },
];

/// A page set in an embedded Type 1 font, with a shape in one corner.
///
/// The font is embedded whole rather than subset, and no `/Encoding` is written: a simple font
/// with none is addressed through the font's *own* built-in encoding, which is what both this
/// renderer and the oracle do, so the comparison is about glyph shapes and not about two
/// different encodings. The shape is there so that part of the page is *not* text — without
/// it the comparison is only ever about glyphs, and a fixture that cannot tell "nothing was
/// drawn" from "the page is blank" is a fixture that scores well when everything fails.
fn type1_page(program: &[u8]) -> Vec<u8> {
    let reader = mangle_font::Program::new(program.to_vec());
    let mut widths = Vec::with_capacity(95);
    for code in 32u8..=126 {
        let width = reader
            .glyph_for_code(u32::from(code))
            .and_then(|glyph| reader.advance(glyph))
            .unwrap_or(500);
        widths.push(width.to_string());
    }
    let widths = widths.join(" ");

    let mut content = String::from("0.86 0.86 0.86 rg 300 8 92 26 re f\n");
    for line in &TYPE1_LINES {
        let _ = writeln!(
            content,
            "BT 0 0 0 rg /F1 {} Tf {} {} Td ({}) Tj ET",
            line.size, line.at.0, line.at.1, line.text
        );
    }

    let mut out: Vec<u8> = Vec::new();
    let mut at = [0usize; 8];
    out.extend_from_slice(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n");
    at[1] = out.len();
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    at[2] = out.len();
    out.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 400 130] \
          /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n",
    );
    at[3] = out.len();
    out.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n");
    at[4] = out.len();
    let mut body = format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).into_bytes();
    body.extend_from_slice(content.as_bytes());
    body.extend_from_slice(b"\nendstream\nendobj\n");
    out.extend_from_slice(&body);

    at[5] = out.len();
    out.extend_from_slice(
        format!(
            "5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Embedded /FirstChar 32 \
             /LastChar 126 /Widths [{widths}] /FontDescriptor 6 0 R >>\nendobj\n"
        )
        .as_bytes(),
    );
    at[6] = out.len();
    out.extend_from_slice(
        b"6 0 obj\n<< /Type /FontDescriptor /FontName /Embedded /Flags 32 /FontBBox \
          [-210 -299 1032 1075] /ItalicAngle 0 /Ascent 900 /Descent -300 /CapHeight 700 \
          /StemV 80 /MissingWidth 500 /FontFile 7 0 R >>\nendobj\n",
    );
    at[7] = out.len();
    let mut file = format!(
        "7 0 obj\n<< /Length {} /Length1 {} /Subtype /Type1 >>\nstream\n",
        program.len(),
        program.len()
    )
    .into_bytes();
    file.extend_from_slice(program);
    file.extend_from_slice(b"\nendstream\nendobj\n");
    out.extend_from_slice(&file);

    let xref = out.len();
    out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n1 7\n");
    for offset in at.iter().take(8).skip(1) {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size 8 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    out
}

/// A page in a Type 1 font, rendered, or the reason there is none.
fn render_type1_page(pdf_bytes: Vec<u8>, scale: f64) -> Result<mangle_render::PageRender, String> {
    let doc = open(pdf_bytes);
    let all = pages(&doc);
    let page = all.first().ok_or("the page has no page tree")?;
    let resources = page
        .inherited
        .resources
        .as_ref()
        .and_then(|o| doc.resolve_object(o))
        .and_then(|o| o.as_dict().cloned())
        .map(|d| Resources::from_dict(&d, &|o| doc.resolve_object(o)))
        .unwrap_or_default();
    Ok(render_page(
        &doc,
        page,
        &resources,
        RenderOptions {
            scale,
            ..RenderOptions::default()
        },
    ))
}

/// An embedded Type 1 page, compared with `mutool`.
///
/// This is the check that says something about *correctness*. Every other Type 1 test here is
/// self-consistent, and a self-consistent interpreter can be self-consistently wrong — undoing
/// the two ciphers in the wrong order, reading the first `rrcurveto`'s last y as a delta, or
/// resolving a character code through a table that does not exist. `mutool` is a different
/// codebase with a decade of accumulated knowledge of what a glyph is meant to look like, and
/// an SSIM against it is a statement about the decryption, the charstring dialect, the code
/// lookup, the em scale, the placement matrix and the fill rule all at once, and about
/// nothing else.
#[test]
fn our_type1_glyphs_agree_with_mutools() {
    if mutool().is_none() {
        eprintln!("skipped: mutool is not installed");
        return;
    }
    let font = match type1_font() {
        Ok(font) => font,
        Err(reason) => {
            eprintln!("skipped: {reason}");
            return;
        }
    };
    let dir = std::env::temp_dir().join("mangle-render-oracle");
    std::fs::create_dir_all(&dir).expect("a place to work");
    let pdf = dir.join("type1.pdf");
    let bytes = type1_page(&font);
    std::fs::write(&pdf, &bytes).expect("a file to render");

    // 150 DPI is the resolution the acceptance criteria name, so this measures the thing the
    // bar is written against rather than an easier version of it.
    let scale = 150.0 / 72.0;
    let ours = render_type1_page(bytes, scale).expect("a font");
    assert!(
        ours.notes.is_empty(),
        "an embedded Type 1 font should draw without complaint: {:?}",
        ours.notes
    );
    let Some(comparison) = score_against_mutool(&ours, &pdf, scale, "Type 1 page") else {
        return;
    };
    assert!(
        comparison.metrics.meets_fidelity_bar(0.95),
        "our render scores {:.4} against mutool, below the 0.95 fidelity bar: {}",
        comparison.metrics.ssim,
        comparison.metrics.summary()
    );
}

/// A `/FontFile` that is not a Type 1 font says so, rather than drawing nothing for every
/// character on the page.
///
/// Drawing nothing with nothing in the report is the failure this guards against. And
/// *mis-parsing* the bytes — taking them for a CFF, or for a `sfnt` — is worse, because that
/// draws the wrong letters and looks like a font problem rather than a reader one.
#[test]
fn a_font_file_that_is_not_a_type1_font_says_so() {
    for (what, bytes) in [
        (
            "a TrueType program",
            b"%!PS-AdobeFont-1.0: Not 001.001\n".to_vec(),
        ),
        (
            "a PDF",
            b"%PDF-1.7\n1 0 obj\n<< >>\nendobj\n%%EOF\n".to_vec(),
        ),
        (
            "a PFB container",
            b"\x80\x01\x2a\x00\x00\x00%!PS-AdobeFont-1.0: Not 001.001\n".to_vec(),
        ),
    ] {
        let render = render_type1_page(type1_page(&bytes), 2.0).expect("a page");
        let said = render.notes.iter().any(|note| {
            note.contains("FontFile")
                && note.contains(what.split_once(' ').map_or(what, |(w, _)| w))
        });
        assert!(
            said,
            "{what} in a /FontFile must say why it cannot be drawn: {:?}",
            render.notes
        );
    }
}

/// A Type 1 program truncated at every length gives a reason rather than a panic.
///
/// A PDF's `/FontFile` stream can be cut short by anything from a truncated download to a
/// damaged cross-reference table, and a crash on one is a crash on a file a user opened.
#[test]
fn a_type1_font_cut_at_every_length_does_not_panic() {
    let Ok(font) = type1_font() else {
        eprintln!("skipped: no Type 1 font on this machine");
        return;
    };
    for cut in 1..font.len() {
        let _ = mangle_font::from_type1(&font[..cut]);
    }
}

// ── Encodings: a code means nothing until the `/Encoding` says what it is ──────────

/// One line of the `/Differences` page, with the byte the content stream writes.
///
/// `bytes` is written into the content stream as written, so a code above 127 is an escaped
/// octal and the fixture really does exercise the codes where the encodings disagree.
struct Encoded {
    /// What the page draws, in words, for whoever reads this next. Not written anywhere: it
    /// is the fixture's own record of what it asks a renderer to do.
    #[allow(
        dead_code,
        reason = "read by whoever reads the fixture, not by the test"
    )]
    note: &'static str,
    bytes: &'static str,
    size: f64,
    at: (f64, f64),
}

/// The lines of the `/Differences` page.
///
/// **Deliberately asymmetric, and the asymmetry is the point.** Every line is a different
/// length at a different size from a different x, and the remaps are not a permutation of
/// each other. Three things are being caught, and a symmetric fixture catches none of them:
///
/// * a renderer that **ignores `/Differences`** and uses the base encoding draws different
///   letters in the same places — the most visible kind of wrong there is;
/// * a renderer that **stops at the end of the base** draws the base's glyphs for the codes
///   past it, which is a different failure from ignoring the array;
/// * a renderer that **transposes two codes** in its table draws the right letters on the
///   wrong sides, which a fixture whose remaps were a permutation would let pass.
///
/// Every name below is one the embedded font actually carries, so a failure is about which
/// glyph was chosen rather than about a glyph that is not there.
const ENCODED_LINES: [Encoded; 5] = [
    Encoded {
        note: "A and v, both remapped away from what the base says",
        bytes: r"\101\166",
        size: 30.0,
        at: (16.0, 108.0),
    },
    Encoded {
        note: "B, C and D remapped to a bullet, a Euro sign and a caron",
        bytes: r"\102\103\104",
        size: 38.0,
        at: (22.0, 66.0),
    },
    Encoded {
        note: "four codes inside the Latin-1 range, remapped to Ccedilla and accented letters",
        bytes: r"\307\310\311\312",
        size: 26.0,
        at: (30.0, 32.0),
    },
    Encoded {
        note: "two codes the base also assigns, remapped to a dagger and a section sign",
        bytes: r"\133\134",
        size: 44.0,
        at: (196.0, 78.0),
    },
    Encoded {
        note: "a code the base leaves unassigned, given a name",
        bytes: r"\200",
        size: 34.0,
        at: (24.0, 14.0),
    },
];

/// A page whose font `/Encoding` remaps several codes through `/Differences`.
///
/// The base is `WinAnsiEncoding` and the differences are *not* drawn from the base's own
/// names, so a reader that quietly ignores the array draws the base's glyphs and is caught;
/// and the array reaches codes the base assigns as well as codes it does not, so a reader that
/// stops at the end of the base is caught too.
///
/// The font is the standard Helvetica by name rather than an embedded program, because the
/// question here is *which* glyph is chosen, not what its outline looks like — an embedded
/// font would make a failure ambiguous between "the wrong glyph" and "the wrong outlines".
/// Both renderers resolve a standard-14 font the same way, so the comparison is about the
/// encoding and about nothing else.
///
/// A grey rectangle sits in one corner so that part of the page is not text. Without it the
/// comparison is only ever about glyphs, and a fixture that cannot tell "drew the wrong
/// letters" from "drew nothing" scores well when everything fails.
fn differences_page() -> Vec<u8> {
    let mut content = String::from("0.87 0.87 0.87 rg 316 8 76 24 re f\n");
    for line in &ENCODED_LINES {
        let _ = writeln!(
            content,
            "BT 0 0 0 rg /F1 {} Tf {} {} Td ({}) Tj ET",
            line.size, line.at.0, line.at.1, line.bytes
        );
    }

    let mut out: Vec<u8> = Vec::new();
    let mut at = [0usize; 8];
    out.extend_from_slice(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n");
    at[1] = out.len();
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    at[2] = out.len();
    out.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 400 130] \
          /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n",
    );
    at[3] = out.len();
    out.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n");
    at[4] = out.len();
    let mut body = format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).into_bytes();
    body.extend_from_slice(content.as_bytes());
    body.extend_from_slice(b"\nendstream\nendobj\n");
    out.extend_from_slice(&body);

    // The font program is embedded whole where one is on the machine, so the page draws the
    // same outlines whichever renderer reads it and the comparison is about the encoding
    // rather than about two different font substitutions.
    let embedded = type1_font().ok();
    let base_font = if embedded.is_some() {
        "Embedded"
    } else {
        "Helvetica"
    };
    // Every advance comes from the embedded font's own `hsbw`, looked up *through this page's
    // encoding* so that a remapped code gets the width of the glyph it actually draws.
    // Deliberate: a `/Widths` array is indexed by code, so writing the base encoding's width
    // for a remapped code would be the width of the wrong letter and would put every glyph
    // after it a fraction out of position — which reads as a layout difference rather than as
    // the letter difference this test is about.
    let widths: String = match &embedded {
        Some(program) => {
            let font = mangle_font::Type1::parse(program).expect("a Type 1 font");
            let mut encoding = mangle_font::Encoding::new(mangle_font::EncodingBase::WinAnsi);
            for (code, name) in DIFFERENCES {
                encoding.push(code, name.to_string());
            }
            (32u32..=255)
                .map(|code| {
                    encoding
                        .glyph_for(code)
                        .and_then(|name| font.glyph_for_named_code(code, Some(name)))
                        .and_then(|glyph| font.advance(glyph).ok())
                        .unwrap_or(0)
                        .to_string()
                })
                .collect::<Vec<_>>()
                .join(" ")
        }
        // No Type 1 program on this machine: a constant width, which keeps the page
        // readable and the structural test below meaningful.
        None => (32u32..=255)
            .map(|_| String::from("500"))
            .collect::<Vec<_>>()
            .join(" "),
    };
    at[5] = out.len();
    out.extend_from_slice(
        format!(
            "5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /{base_font} /FirstChar 32 \
             /LastChar 255 /Widths [{widths}] /FontDescriptor 6 0 R \
             /Encoding << /Type /Encoding /BaseEncoding /WinAnsiEncoding \
             /Differences [{}] >> >>\nendobj\n",
            DIFFERENCES
                .iter()
                .map(|(code, name)| format!("{code} /{name}"))
                .collect::<Vec<_>>()
                .join(" ")
        )
        .as_bytes(),
    );
    at[6] = out.len();
    let descriptor = if embedded.is_some() {
        "6 0 obj\n<< /Type /FontDescriptor /FontName /Embedded /Flags 32 /FontBBox \
         [-210 -299 1032 1075] /ItalicAngle 0 /Ascent 900 /Descent -300 /CapHeight 700 \
         /StemV 80 /MissingWidth 500 /FontFile 7 0 R >>\nendobj\n"
            .to_string()
    } else {
        // No Type 1 program on this machine. The descriptor names the standard face and
        // carries no `/FontFile`; the oracle test skips in this case and the structural test
        // below still runs, because what it checks is the encoding rather than the outlines.
        "6 0 obj\n<< /Type /FontDescriptor /FontName /Helvetica /Flags 32 /FontBBox \
         [-166 -225 1000 931] /ItalicAngle 0 /Ascent 718 /Descent -207 /CapHeight 718 \
         /StemV 88 >>\nendobj\n"
            .to_string()
    };
    out.extend_from_slice(descriptor.as_bytes());

    if let Some(program) = &embedded {
        at[7] = out.len();
        let mut file = format!(
            "7 0 obj\n<< /Length {} /Length1 {} /Subtype /Type1 >>\nstream\n",
            program.len(),
            program.len()
        )
        .into_bytes();
        file.extend_from_slice(program);
        file.extend_from_slice(b"\nendstream\nendobj\n");
        out.extend_from_slice(&file);
    }

    // Objects 1 to 6 always; object 7 is the font program when one was embedded. The
    // cross-reference subsection header counts the entries in it, which is one per object
    // from 1 up, so it is 7 with the program and 6 without.
    let xref = out.len();
    let last = if embedded.is_some() { 7 } else { 6 };
    out.extend_from_slice(format!("xref\n0 1\n0000000000 65535 f \n1 {last}\n").as_bytes());
    for offset in at.iter().take(last + 1).skip(1) {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            last + 1
        )
        .as_bytes(),
    );
    out
}

/// The `/Differences` the page is built from: `(code, name)`, in the order written.
///
/// Codes 32 and 34 are remapped to the same glyph on purpose. `space` is one name at two
/// codes, so a reader that maps name-to-code rather than code-to-name gets one of the two
/// wrong — and a page where the two codes carry the same glyph draws identically either way,
/// which is the point: the fixture cannot pass on a lucky lookup.
const DIFFERENCES: [(u32, &str); 18] = [
    (0x41, "germandbls"),
    (0x42, "bullet"),
    (0x43, "Euro"),
    (0x44, "caron"),
    (0x76, "Thorn"),
    (0xC7, "Ccedilla"),
    (0xC8, "eacute"),
    (0xC9, "oe"),
    (0xCA, "oslash"),
    (0x5B, "dagger"),
    (0x5C, "section"),
    (0xC0, "divide"),
    (0xC1, "degree"),
    (0xC2, "multiply"),
    (0xC3, "ydieresis"),
    (0xC4, "exclam"),
    (0x20, "space"),
    (0x22, "space"),
];

/// A page whose `/Differences` array decides which glyph each code draws, compared with
/// `mutool`.
///
/// This is the only check in the file that says anything about *encodings*. Every other
/// rendering test here is self-consistent, and a self-consistent renderer can be
/// self-consistently wrong about an encoding: resolving 0xC7 through `WinAnsiEncoding` when
/// the file named `Ccedilla`, or ignoring `/Differences` altogether and using the font's
/// built-in encoding, each draws plausible letters in plausible places and passes every
/// structural check there is. `mutool` read the same `/Encoding` from the same bytes, so where
/// the two disagree about a letter, one of them is wrong, and the SSIM says how wrong.
///
/// It is the last thing standing between the font layer and correct text: a page that remaps
/// even one code renders that code with the wrong glyph, and this is the test that would have
/// said so.
#[test]
fn our_differences_encodings_agree_with_mutools() {
    if mutool().is_none() {
        eprintln!("skipped: mutool is not installed");
        return;
    }
    let dir = std::env::temp_dir().join("mangle-render-oracle");
    std::fs::create_dir_all(&dir).expect("a place to work");
    let pdf = dir.join("differences.pdf");
    let bytes = differences_page();
    std::fs::write(&pdf, &bytes).expect("a file to render");

    // 150 DPI is the resolution the acceptance criteria name, so this measures the thing the
    // bar is written against rather than an easier version of it.
    let scale = 150.0 / 72.0;
    let ours = render_type1_page(bytes, scale).expect("a page");
    let Some(comparison) = score_against_mutool(&ours, &pdf, scale, "/Differences page") else {
        return;
    };
    assert!(
        comparison.metrics.meets_fidelity_bar(0.95),
        "our render scores {:.4} against mutool, below the 0.95 fidelity bar: {}",
        comparison.metrics.ssim,
        comparison.metrics.summary()
    );
}

/// The `/Differences` array decides which glyph a code draws, and not the font's own idea.
///
/// Structural rather than an oracle check, so it runs everywhere: this renderer and this file
/// must agree about which letter each remapped code is, and the way to say that without
/// comparing outlines is to compare *which* glyph was chosen. A renderer that ignored the
/// array would draw `B C D` where the file asked for a bullet, a Euro sign and a caron, and
/// the ink would be in the same places — so the check is on the glyphs, not the pixels.
#[test]
fn the_differences_array_and_not_the_fonts_own_encoding_choose_the_glyph() {
    let font = dict_from(&[(
        "Encoding",
        Object::Dict(
            [
                ("Type", Object::name("Encoding")),
                ("BaseEncoding", Object::name("WinAnsiEncoding")),
                (
                    "Differences",
                    Object::Array(vec![
                        Object::Int(66), // B -> bullet
                        Object::name("bullet"),
                        Object::Int(67), // C -> Euro
                        Object::name("Euro"),
                    ]),
                ),
            ]
            .into_iter()
            .fold(mangle_syntax::object::Dict::new(), |mut d, (k, v)| {
                d.insert(mangle_syntax::Name::new(k), v);
                d
            }),
        ),
    )]);
    let base = dict_from(&[("Encoding", Object::name("WinAnsiEncoding"))]);
    assert_eq!(
        encoding_names(&font),
        [(66u32, String::from("bullet")), (67, String::from("Euro"))],
        "the array decides the names"
    );
    assert_eq!(
        encoding_names(&base),
        [(66u32, String::from("B")), (67, String::from("C"))],
        "and with no array the base decides them"
    );
}

// ── Substitution: a standard font the document did not embed ─────────────────────

/// A page naming a Standard-14 font with no `/FontDescriptor` and no `/Widths`, laid out so
/// that every glyph's position is known from the built-in metrics alone.
///
/// `base` is the `/BaseFont`; `size` the type size in points. Nothing else about the font is
/// declared, which is what the case is: the standard fourteen have no descriptor by
/// definition, and a producer that names one without embedding it is describing a font, not
/// omitting one.
///
/// The text is `iiii` and the glyphs are drawn from x = `origin_x` in steps of the advance of
/// `i`. That is what makes this a test of *width* as well as of outlines: a renderer with no
/// widths for the font falls back to one average advance, and every glyph after the first
/// lands in the wrong column — which is where the column assertions below catch it.
fn unembedded_standard_page(base: &str, text: &str, size: f64, origin_x: f64) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut at = [0usize; 6];
    out.extend_from_slice(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n");
    at[1] = out.len();
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    at[2] = out.len();
    out.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 100] >>\nendobj\n",
    );
    at[3] = out.len();
    out.extend_from_slice(
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 5 0 R >> >> \
          /Contents 4 0 R >>\nendobj\n",
    );
    let content = format!("BT /F1 {size} Tf 1 0 0 1 {origin_x} 30 Tm ({text}) Tj ET");
    at[4] = out.len();
    let mut body = format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).into_bytes();
    body.extend_from_slice(content.as_bytes());
    body.extend_from_slice(b"\nendstream\nendobj\n");
    out.extend_from_slice(&body);
    // No `/FontDescriptor`, no `/Widths`, no `/FirstChar`: the shape a document that names a
    // standard font without embedding it actually has.
    at[5] = out.len();
    out.extend_from_slice(
        format!("5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /{base} >>\nendobj\n")
            .as_bytes(),
    );

    let xref = out.len();
    out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n1 5\n");
    for offset in at.iter().take(6).skip(1) {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size 6 /Root 1 0 R /ID [<0102> <0304>] >>\nstartxref\n{xref}\n%%EOF\n"
        )
        .as_bytes(),
    );
    out
}

/// The advance of a glyph in a standard font, in thousandths of an em.
fn standard_advance(base: &str, glyph: &str) -> f64 {
    let table = mangle_font::metrics::widths(base).expect("a standard font's widths");
    f64::from(
        table
            .width_of(glyph)
            .unwrap_or_else(|| panic!("{base} should have a width for {glyph}")),
    )
}

/// A standard font the document did not embed still puts ink on the page.
///
/// Before a substitute existed this page rendered blank and said the font was not embedded,
/// which is what every producer of such a file assumes a reader will not do. The glyphs come
/// from a bundled metric-compatible face rather than the document's own, so what is asserted
/// is ink where the glyphs are and a notice saying which face it is — not that the outlines
/// are the original's, which they are not and cannot be.
#[test]
fn a_standard_font_the_document_did_not_embed_is_drawn_from_a_substitute() {
    let scale = 4.0;
    for (base, size) in [("Helvetica", 48.0), ("Times-Roman", 48.0)] {
        // Four narrow glyphs, each `i` wide, so the columns are far apart and a wrong advance
        // cannot land one glyph in the next column by luck.
        let render = render(unembedded_standard_page(base, "iiii", size, 20.0), scale);
        let note = render
            .notes
            .iter()
            .find(|note| note.contains("stands in for"));
        assert!(
            note.is_some(),
            "{base} was not embedded, so a substitute must say so: {:?}",
            render.notes
        );
        assert!(
            !render
                .notes
                .iter()
                .any(|note| note.contains("no `/FontDescriptor`")),
            "{base} must not also be reported as a font that cannot be read at all: {:?}",
            render.notes
        );

        let advance = standard_advance(base, "i") / 1000.0 * size * scale;
        // `origin` is where the text matrix put the run; `region` takes page proportions, so
        // the band is the glyph's own column of a 200-point-wide page.
        let origin = 20.0;
        // `advance` is already in pixels — it carries the scale — so only the origin, which is
        // in points, is multiplied by it.
        let origin_px = origin * scale;
        for (index, column) in [0.0, 1.0, 2.0, 3.0].into_iter().enumerate() {
            let x0 = origin_px + advance * column;
            let fx0 = (x0 / render.image.width as f64).clamp(0.0, 1.0);
            let fx1 = ((x0 + advance) / render.image.width as f64).clamp(0.0, 1.0);
            assert!(
                ink_in_columns(&render.image, fx0, fx1) > 0,
                "glyph {index} of {base} should have ink in its own column at x {x0}px, and \
                 that column is blank: advance {advance}px"
            );
        }
    }
}

/// A family that stands in for a Standard 14 one by measurement draws from the same face.
///
/// `Arial`, `Arial-BoldMT`, `TimesNewRomanPSMT` and `CourierNewPSMT` are the names a producer
/// actually writes, and none of them is on the list of fourteen. Before an alias existed each
/// of these pages rendered blank and reported a font with no `/FontDescriptor`, which is a
/// correct report about a font this had never heard of and no use at all to the reader.
///
/// The assertion is that the alias renders the *same pixels* as the family it stands in for.
/// That is stronger than asserting ink: the widths have to agree and the outlines have to be
/// the same program, or text laid out for one would be drawn in another's letters.
#[test]
fn a_metric_compatible_alias_draws_exactly_as_the_family_it_stands_in_for() {
    let scale = 4.0;
    for (alias, target) in [
        ("Arial", "Helvetica"),
        ("Arial-BoldMT", "Helvetica-Bold"),
        ("TimesNewRomanPSMT", "Times-Roman"),
        ("TimesNewRomanPS-ItalicMT", "Times-Italic"),
        ("CourierNewPSMT", "Courier"),
    ] {
        let ours = render(unembedded_standard_page(alias, "WiWi", 48.0, 10.0), scale);
        let theirs = render(unembedded_standard_page(target, "WiWi", 48.0, 10.0), scale);
        assert!(
            ours.notes.iter().any(|note| note.contains("stands in for")),
            "{alias} was not embedded, so a substitute must say so: {:?}",
            ours.notes
        );
        assert!(
            !ours
                .notes
                .iter()
                .any(|note| note.contains("no `/FontDescriptor`")),
            "{alias} must not be reported as a font that cannot be read at all: {:?}",
            ours.notes
        );
        assert!(
            ink_in_columns(&ours.image, 0.0, 1.0) > 0,
            "{alias} drew nothing at all, which is the failure this guards"
        );
        assert_eq!(
            ours.image.width, theirs.image.width,
            "{alias} and {target} should lay out at the same size"
        );
        assert_eq!(
            ours.image.pixels, theirs.image.pixels,
            "{alias} must draw exactly what {target} draws: the same advances and the same \
             outlines, or the glyphs land somewhere the producer did not put them"
        );
    }
}

/// A name that stands in for nothing is still reported rather than drawn in someone's letters.
///
/// The companion to the test above, and the one that keeps the alias table from growing: a
/// family that is *not* metric-compatible with a standard one — Helvetica Neue above all — must
/// keep refusing, because a wrong number on the page is worse than a refusal to draw.
#[test]
fn a_family_that_stands_in_for_nothing_is_still_reported_rather_than_drawn() {
    let scale = 4.0;
    for base in ["HelveticaNeueLTStd-Roman", "Arial-Black", "ArialNova"] {
        let render = render(unembedded_standard_page(base, "WiWi", 48.0, 10.0), scale);
        assert!(
            ink_in_columns(&render.image, 0.0, 1.0) == 0,
            "{base} is not metric-compatible with a standard face, so it must not be drawn \
             in one: it has ink in {:?}",
            render.notes
        );
    }
}

/// The substitute's advances are the standard font's, or the text would not stay where the
/// producer laid it out.
///
/// The first test uses one glyph repeated, so it cannot tell a correct advance from a plausible
/// one. This one alternates `W` and `i`, which differ by a factor of four in both families, and
/// asserts that the *last* glyph is where the standard advances put it. A renderer with one
/// average advance for every code — what a font with no `/Widths` and no built-in table gets —
/// accumulates the difference over three glyphs and puts it somewhere else entirely.
#[test]
fn a_substituted_font_advances_each_glyph_by_its_own_standard_width() {
    let scale = 4.0;
    let size = 48.0;
    for base in ["Helvetica", "Times-Roman"] {
        let render = render(unembedded_standard_page(base, "WiWi", size, 10.0), scale);
        let narrow = standard_advance(base, "i") / 1000.0 * size * scale;
        let wide = standard_advance(base, "W") / 1000.0 * size * scale;
        assert!(
            wide > narrow * 2.0,
            "the fixture is only a test if W is much wider than i in {base}: {wide} vs {narrow}"
        );
        // Where each glyph belongs: the origin, then each previous glyph's own width.
        let origin_px = 10.0 * scale;
        let positions = [
            origin_px,
            origin_px + wide,
            origin_px + wide + narrow,
            origin_px + wide + narrow + wide,
        ];
        // One average advance for every code is what a font with neither a `/Widths` array nor
        // a built-in table gets, and it is a quarter of the width here, so by the fourth glyph
        // the two predictions are more than a glyph apart.
        let average = 500.0 / 1000.0 * size * scale;
        assert!(
            positions[3] - (origin_px + average * 3.0) > narrow,
            "the fixture is only a test if the last glyph's real and average-advance positions \
             are more than a glyph apart in {base}"
        );
        // The last glyph is the narrow one, so it is the one whose position the accumulated
        // difference decides, and it is the last thing on the page — so nothing else can put ink
        // in its band.
        let last_i = positions[3];
        let page_width = render.image.width as f64;
        let (fx0, fx1) = (
            (last_i - narrow * 0.4) / page_width,
            (last_i + narrow * 0.4) / page_width,
        );
        assert!(
            ink_in_columns(&render.image, fx0.clamp(0.0, 1.0), fx1.clamp(0.0, 1.0)) > 0,
            "the last `i` of {base} should be at x {last_i}px, after a wide `W`, a narrow `i` and \
             another wide `W`, and there is no ink there"
        );
    }
}

/// A name with no metric-compatible stand-in is still reported rather than invented.
///
/// `Symbol` and `ZapfDingbats` are standard fonts with no Liberation equivalent, and a page
/// naming one must still get the refusal: substituting a Latin face for a dingbat font would
/// put entirely wrong glyphs on the page and call it a font. The same is true of a name that
/// is not standard at all.
#[test]
fn a_standard_font_with_no_substitute_is_still_reported() {
    for base in ["Symbol", "ZapfDingbats", "NoSuchFont"] {
        let render = render(unembedded_standard_page(base, "iiii", 48.0, 20.0), 4.0);
        let said = render
            .notes
            .iter()
            .any(|note| note.contains("no `/FontDescriptor`") || note.contains("not embedded"));
        assert!(
            said,
            "{base} has no substitute, so the refusal must stand: {:?}",
            render.notes
        );
        assert!(
            !render
                .notes
                .iter()
                .any(|note| note.contains("stands in for")),
            "{base} must not be silently drawn in someone else's face: {:?}",
            render.notes
        );
    }
}

/// The codes the `/Encoding` gives names, as `(code, name)`.
fn encoding_names(font: &mangle_syntax::object::Dict) -> Vec<(u32, String)> {
    let encoding =
        mangle_font::Encoding::from_font_dict(font, mangle_font::EncodingBase::Standard, &|o| {
            Some(o.clone())
        })
        .expect("an encoding");
    let mut out = Vec::new();
    for code in 32..256u32 {
        if (code == 66 || code == 67)
            && let Some(name) = encoding.glyph_for(code)
        {
            out.push((code, name.to_string()));
        }
    }
    out
}

/// A font dictionary with the given entries.
fn dict_from(entries: &[(&str, Object)]) -> mangle_syntax::object::Dict {
    let mut d = mangle_syntax::object::Dict::new();
    for (key, value) in entries {
        d.insert(mangle_syntax::Name::new(key), value.clone());
    }
    d
}
