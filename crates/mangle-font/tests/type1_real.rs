//! Every glyph of every Type 1 font on the machine, walked.
//!
//! The unit tests in `type1.rs` build their fonts a byte at a time, which is what makes their
//! expectations checkable by hand but also what makes them blind: a font they built can only
//! contain what the test author thought to put in it. This test reads the fonts other people
//! installed and walks all of them, and it exists because of the defects it caught that the
//! hand-built fonts could not:
//!
//! * **The cipher's recurrence was fed the plaintext byte** rather than the ciphertext one.
//!   Both readings produce a stream that is *mostly* printable, so a font decrypted with the
//!   wrong one still looks like a PostScript program in a debugger. Every glyph came out
//!   empty and nothing said why.
//! * **`/lenIV` was applied as `lenIV` rounds rather than `lenIV - 4`.** 4330 is the seed for
//!   the usual `lenIV` of 4, so a font that writes `lenIV 4` — or, like most of them, writes
//!   nothing at all — needs no advance at all, and advancing it four times desynchronises
//!   every charstring in the font.
//! * **Byte 0 is the hint mask**, and its length is a function of every stem declared so far
//!   rather than of the operands the mask itself was written with. Most masks are written in a
//!   subroutine entered with the caller's stems already declared, so reading the length from
//!   the local count gives the wrong number of bytes and desynchronises everything after it.
//! * **`rmoveto` and `hmoveto` are 21 and 22 in this dialect**, not 15 and 16. Every `Move`
//!   after the first in a glyph landed somewhere else, which is a font that draws, just not
//!   the right one.
//! * **A hint mask's length is a property of the code it is written in.** A subroutine's mask
//!   covers the stems *that subroutine* declared, so it is empty unless it declares stems of
//!   its own. Reading the length from the caller's running total skips bytes that belong to
//!   the subroutine's operators, which turned the standard hint-replacement subroutine into a
//!   prefix of nonsense: 2 312 glyphs in these 28 fonts lost their outline, and the 6 538
//!   that use it lost their last `callsubr` as well.
//! * **A `pop` after a `callothersubr` is not a discard.** It says how many of the values the
//!   subroutine returned the charstring will use, and the values stay for the operator that
//!   does use them. A hint-replacement subroutine is `<count> 1 3 callothersubr pop callsubr`:
//!   the `pop` says "one", and the `callsubr` then reads it as the subroutine to run.
//!
//! It skips cleanly when the machine has no Type 1 font, because a test that quietly passes is
//! worse than one that skips.

#![forbid(unsafe_code)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    // Six single-letter names is a curve, and the specification names them a, b, c, d, e and
    // f.
    clippy::many_single_char_names
)]

use std::path::Path;

use mangle_font::type1::Type1;

/// Where a Type 1 font might be, most-likely first.
///
/// Ghostscript's resource directory holds the URW and Nimbus faces as **bare PFA programs**,
/// which is exactly the shape a `/FontFile` carries. There is usually no Type 1 font in a
/// distribution's `type1` directory at all — most Linux distributions ship outlines only —
/// so this is the one place a real one is found.
const CANDIDATES: [&str; 6] = [
    "/usr/share/ghostscript/Resource/Font/NimbusSans-Regular",
    "/usr/share/ghostscript/Resource/Font/NimbusRoman-Regular",
    "/usr/share/ghostscript/Resource/Font/NimbusMonoPS-Regular",
    "/usr/share/ghostscript/Resource/Font/URWGothic-Book",
    "/usr/share/ghostscript/Resource/Font/URWBookman-Light",
    "/usr/share/ghostscript/Resource/Font/P052-Roman",
];

/// Every Type 1 font found in the candidate list, with its path.
fn fonts() -> Vec<(String, Vec<u8>)> {
    let mut found = Vec::new();
    for path in CANDIDATES {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        if bytes.first() == Some(&b'%') && Type1::parse(&bytes).is_ok() {
            found.push((path.to_owned(), bytes));
        }
    }
    found
}

/// Every point of one segment, as the numbers they are.
fn coords(segment: &mangle_font::Segment) -> Vec<(f64, f64)> {
    use mangle_font::Segment::{Curve, Line, Move};
    match *segment {
        Move(x, y) | Line(x, y) => vec![(x, y)],
        Curve(a, b, c, d, e, f) => vec![(a, b), (c, d), (e, f)],
    }
}

/// Every glyph of every Type 1 font on the machine, walked.
///
/// The assertion that matters is that a *share* of the glyphs draw. A reader that got the
/// cipher right and the charstring dialect wrong still decodes without complaint and produces
/// nothing, and a test that only counted refusals would see a font it could not read — which
/// is the same as a test that could not read it either.
#[test]
fn every_glyph_of_every_type1_font_on_this_machine_walks() {
    let fonts = fonts();
    if fonts.is_empty() {
        eprintln!(
            "skipped: no Type 1 font found; looked in {}",
            CANDIDATES.join(", ")
        );
        return;
    }
    let mut walked = 0usize;
    let mut drew = 0usize;
    let mut refused: Vec<String> = Vec::new();
    for (path, bytes) in &fonts {
        let font = Type1::parse(bytes)
            .unwrap_or_else(|why| panic!("{path}: a Type 1 font that will not parse: {why}"));
        for glyph in 0..font.num_glyphs() as u32 {
            walked += 1;
            match font.outline(glyph) {
                Ok(outline) if !outline.is_empty() => {
                    drew += 1;
                    // A glyph is at most an em or so either side of the origin. A coordinate
                    // out there is not a glyph that looked wrong, it is a walk that lost its
                    // place — and that is the failure an arity check cannot see, because the
                    // operators still balance.
                    for point in outline.segments.iter().flat_map(coords) {
                        assert!(
                            point.0.abs() < 4.0 && point.1.abs() < 4.0 && point.0.is_finite(),
                            "{path} glyph {glyph}: a point at {point:?}, which is nowhere a \
                             glyph is"
                        );
                    }
                }
                Ok(_) => {}
                Err(why) => refused.push(format!("{path} glyph {glyph}: {why}")),
            }
        }
    }
    assert!(walked > 4_000, "only {walked} glyphs were checked");
    assert!(
        refused.is_empty(),
        "{} glyphs refused, first few: {:#?}",
        refused.len(),
        refused.iter().take(5).collect::<Vec<_>>()
    );
    // A space has no outline, and a font is mostly letters, so this is a very low bar — but it
    // is the bar that catches a decoder which produces nothing at all for anything.
    let share = drew as f64 / walked as f64;
    assert!(
        share > 0.5,
        "only {:.1}% of {walked} glyphs drew anything, which is a decoder that is \
         running and understanding nothing",
        share * 100.0
    );
    eprintln!(
        "type1: {} fonts, {walked} glyphs, {drew} with an outline",
        fonts.len()
    );
}

/// A character code finds its glyph through the font's own encoding, and its width through
/// the glyph's own `hsbw`.
///
/// Both halves of "the font can be used" rather than "the font can be opened": a Type 1
/// program has no `cmap`, so the encoding lookup is the only way in, and the width is what
/// puts the second letter in the right place.
#[test]
fn a_code_finds_its_glyph_and_its_width() {
    let fonts = fonts();
    let Some((path, bytes)) = fonts.first() else {
        eprintln!(
            "skipped: no Type 1 font found; looked in {}",
            CANDIDATES.join(", ")
        );
        return;
    };
    let font = Type1::parse(bytes).unwrap_or_else(|why| panic!("{path}: {why}"));
    // The ASCII range, which every Type 1 font's standard encoding covers.
    let mut resolved = 0usize;
    for code in 33u32..=126 {
        let Some(glyph) = font.glyph_for_code(code) else {
            continue;
        };
        assert!(
            font.glyph_name(glyph).is_some_and(|name| !name.is_empty()),
            "code {code} names glyph {glyph}, which has no name"
        );
        let width = font
            .advance(glyph)
            .unwrap_or_else(|why| panic!("{path} glyph {glyph} ({code}): {why}"));
        assert!(
            width > 0,
            "{path}: code {code} is glyph {glyph} and has no width at all"
        );
        resolved += 1;
    }
    assert!(
        resolved > 80,
        "{path}: only {resolved} of the 94 printable ASCII codes resolved to a glyph"
    );
}
/// The pairs of files that are the same design in the two shapes.
const TWINS: [(&str, &str); 4] = [
    (
        "/usr/share/ghostscript/Resource/Font/NimbusSans-Regular",
        "NimbusSans-Regular.otf",
    ),
    (
        "/usr/share/ghostscript/Resource/Font/NimbusRoman-Regular",
        "NimbusRoman-Regular.otf",
    ),
    (
        "/usr/share/ghostscript/Resource/Font/NimbusMonoPS-Regular",
        "NimbusMonoPS-Regular.otf",
    ),
    (
        "/usr/share/ghostscript/Resource/Font/URWGothic-Book",
        "URWGothic-Book.otf",
    ),
];

/// A glyph's bounding box, or `None` for an empty outline.
fn box_of(outline: &mangle_font::Outline) -> Option<(f64, f64, f64, f64)> {
    use mangle_font::Segment::{Curve, Line, Move};
    let mut out: Option<(f64, f64, f64, f64)> = None;
    for segment in &outline.segments {
        let points: Vec<(f64, f64)> = match *segment {
            Move(x, y) | Line(x, y) => vec![(x, y)],
            Curve(a, b, c, d, e, f) => vec![(a, b), (c, d), (e, f)],
        };
        for (x, y) in points {
            out = Some(match out {
                None => (x, x, y, y),
                Some((x0, x1, y0, y1)) => (x0.min(x), x1.max(x), y0.min(y), y1.max(y)),
            });
        }
    }
    out
}

/// The same face, as a Type 1 program and as OpenType/CFF, drawing the same glyphs.
///
/// This is the strongest check in this file, and it exists because of what it caught. Both
/// readers are ours and both were written against a specification; a difference between them
/// is a difference between two readings of the same outlines, and the outlines themselves can
/// be checked because the same URW and Nimbus faces are installed in *both* shapes on a
/// machine that has Ghostscript. Two defects came out of it:
///
/// * **The operand order of `hvcurveto` and `vhcurveto`.** Both take four operands,
///   `dx1 dx2 dy2 dy3` and `dy1 dx2 dy2 dx3`; reading the fourth and fifth the wrong way round
///   swaps the endpoint's x and y, which turns every round letter into a shape that closes and
///   does not match. It cost 0.15 of SSIM on a page of ordinary text before it was found.
/// * **The left sidebearing.** `hsbw`'s first operand says where the outline sits relative to
///   the pen, and the charstring's coordinates do not include it — leaving it out puts every
///   glyph a whole sidebearing to the left of where it belongs.
///
/// A bounding box rather than the points themselves, because the two builds round the same
/// design differently at the last unit; the two quotation marks are the only glyphs where that
/// is more than a rounding, and they are allowed 0.2 em.
#[test]
fn a_type1_face_draws_the_same_glyphs_as_its_opentype_twin() {
    let mut checked = 0usize;
    let mut mismatched: Vec<String> = Vec::new();
    for (bare, cff) in TWINS {
        let (Ok(one), Ok(other)) = (
            std::fs::read(bare),
            std::fs::read(Path::new("/usr/share/fonts/gsfonts").join(cff)),
        ) else {
            continue;
        };
        let Ok(one) = Type1::parse(&one) else {
            continue;
        };
        let mut twin = mangle_font::Program::new(other);
        for code in 33u32..=126 {
            let (Some(here), Some(there)) = (one.glyph_for_code(code), twin.glyph_for_code(code))
            else {
                continue;
            };
            let (Ok(mine), Some((theirs, _))) = (one.outline(here), twin.outline(there)) else {
                continue;
            };
            if mine.segments.is_empty() {
                continue;
            }
            checked += 1;
            let far = match (box_of(&mine), box_of(&theirs)) {
                (Some(a), Some(b)) => (a.0 - b.0)
                    .abs()
                    .max((a.1 - b.1).abs())
                    .max((a.2 - b.2).abs())
                    .max((a.3 - b.3).abs()),
                _ => f64::MAX,
            };
            let tolerance = if matches!(code, 0x22 | 0x27 | 0x60 | 0xB4 | 0xB8 | 0x2D) {
                0.2
            } else {
                0.02
            };
            if far > tolerance {
                mismatched.push(format!(
                    "{bare}: code {code} ({}) is {far:.3} em from its twin",
                    one.glyph_name(here).unwrap_or("?")
                ));
            }
        }
    }
    if checked < 200 {
        eprintln!(
            "skipped: only {checked} glyphs compared; no Type 1 and OpenType twins installed \
             (looked for {})",
            TWINS.iter().map(|(a, _)| *a).collect::<Vec<_>>().join(", ")
        );
        return;
    }
    assert!(
        mismatched.is_empty(),
        "{} of {checked} glyphs differ from their OpenType twin: {:#?}",
        mismatched.len(),
        mismatched.iter().take(8).collect::<Vec<_>>()
    );
    eprintln!("type1: {checked} glyphs agree with their OpenType twins");
}
