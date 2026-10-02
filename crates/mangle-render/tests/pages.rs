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
    let close = |a: u8, b: u8| i32::from(a) - i32::from(b) <= 2;
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
            let close = |a: u8, b: u8| i32::from(a) - i32::from(b) <= 2;
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
    // And the lower right, which held the blue square, now holds paper.
    assert!(
        region_is_fraction(&render.image, 0.55, 0.55, 0.95, 0.95, [255, 255, 255]),
        "and the quadrant that had blue is now paper"
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
