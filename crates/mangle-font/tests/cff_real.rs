//! Every glyph of every CFF font on the machine, walked.
//!
//! The unit tests in `cff.rs` build their fonts a byte at a time, which is what makes their
//! expectations checkable by hand but also what makes them blind: a font they built can only
//! contain what the test author thought to put in it. This test reads the fonts other people
//! installed and walks all of them, and it exists because of the four defects it caught that
//! the hand-built fonts could not:
//!
//! * Bytes 29 and 30 rejected as operand encodings. They are a DICT's five-byte integer and
//!   its real number; in a charstring they are `callgsubr` and `vhcurveto`, and refusing them
//!   lost most of a real font.
//! * A hint mask counted only the stems its own operands declared. The format lets a font
//!   leave out a `vstemhm` whose definitions are followed straight by a mask, so the mask
//!   counts stems too — and a mask read at the wrong length desynchronises everything after
//!   it.
//! * A `callsubr` that emptied the whole operand stack instead of popping its number, so
//!   every subroutine that began with a `moveto` found an empty stack. This one refused 54% of
//!   a real font's glyphs and drew nothing for any of them.
//! * An empty INDEX sized as four bytes rather than two, which put every table after the
//!   Name INDEX a whole CFF font's string list out of place.
//!
//! It skips cleanly when the machine has no CFF font, because a test that quietly passes is
//! worse than one that skips.

#![forbid(unsafe_code)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    // Six single-letter names is a curve. `ttf-parser`'s own builder spells them a, b, c, d,
    // e and f, and spelling them anything else here would be worse.
    clippy::many_single_char_names,
    // A quadratic segment cannot occur in a CFF outline, so the arm has nothing to do. A
    // panic is the right answer to a trait method with no answer, and this is a test.
    clippy::unimplemented,
    // These helpers walk a list of segments they have just built and whose length they have
    // just reasoned about, which is what indexing is for.
    clippy::indexing_slicing
)]

use mangle_font::cff::{Cff, cff_bytes};

/// Where a CFF font might be, most-likely first.
///
/// Deliberately not an exhaustive search: this is a regression net, and a walk of the whole
/// of `/usr/share/fonts` would spend most of its time on TrueType files that answer `None`.
const CANDIDATES: [&str; 8] = [
    "/usr/share/fonts/gnu-free/FreeSerif.otf",
    "/usr/share/fonts/gnu-free/FreeSans.otf",
    "/usr/share/fonts/gnu-free/FreeMono.otf",
    "/usr/share/fonts/gnu-free/FreeSerifBold.otf",
    "/usr/share/fonts/gsfonts/NimbusSans-Regular.otf",
    "/usr/share/fonts/gsfonts/NimbusRoman-Regular.otf",
    "/usr/share/fonts/gsfonts/URWBookman-Demi.otf",
    "/usr/share/fonts/gsfonts/D050000L.otf",
];

/// Every CFF font found in the candidate list, with its path.
fn fonts() -> Vec<(String, Vec<u8>)> {
    CANDIDATES
        .iter()
        .filter_map(|path| {
            let data = std::fs::read(path).ok()?;
            cff_bytes(&data)
                .is_some()
                .then(|| ((*path).to_string(), data))
        })
        .collect()
}

/// No glyph of a real CFF font is refused.
///
/// "Refused" is the word that matters here: a glyph with no outline is a space and is the
/// common case in a real font, but a glyph this cannot interpret is a wrong answer waiting
/// for a user, and the four defects above were all of that shape — a plausible-looking
/// refusal that read as "this font is difficult" rather than "this reader is wrong".
///
/// The count is reported rather than asserted, because a machine with one small font and a
/// machine with fourteen large ones are both correct and the number of glyphs walked is not
/// something to assert on.
#[test]
fn every_glyph_of_every_cff_font_on_this_machine_is_walked() {
    let fonts = fonts();
    if fonts.is_empty() {
        eprintln!(
            "skipped: no CFF font found; looked in {}",
            CANDIDATES.join(", ")
        );
        return;
    }
    let (mut walked, mut blank, mut refused) = (0usize, 0usize, Vec::new());
    for (path, data) in &fonts {
        let table = cff_bytes(data).expect("a CFF table this test chose");
        let cff = Cff::parse(table, 0).unwrap_or_else(|why| panic!("{path} should parse: {why}"));
        for glyph in 0..cff.num_glyphs() {
            let g = u32::try_from(glyph).expect("a glyph number");
            match cff.outline(g) {
                Ok(outline) if outline.is_empty() => blank += 1,
                Ok(_) => walked += 1,
                Err(why) => refused.push(format!("{path} glyph {glyph}: {why}")),
            }
        }
    }
    eprintln!(
        "{} CFF fonts: {walked} glyphs walked, {blank} with no outline, {} refused",
        fonts.len(),
        refused.len()
    );
    assert!(
        walked > 500,
        "only {walked} glyphs walked, which is too few for this to have tested anything; \
         the fonts found were {:?}",
        fonts.iter().map(|(p, _)| p).collect::<Vec<_>>()
    );
    assert!(
        refused.is_empty(),
        "every glyph of a real CFF font should walk:\n  {}",
        refused.join("\n  ")
    );
}

/// Every glyph's width is the width the font's own `hmtx` table says it has.
///
/// This is the check that matters for the width rule, and it is a cross-check rather than a
/// restatement: the width here comes from executing a charstring and taking one operand off
/// the front of its stack, and the width there comes from a table this code never reads. A
/// reader that took the wrong end of the stack — the top rather than the bottom, or the
/// parity for `hmoveto` the wrong way round — would still produce a plausible number, and the
/// only thing that says it is wrong is a different part of the same font disagreeing.
///
/// A glyph with no width operand at all gets `defaultWidthX`, and a glyph with no stems gets
/// `nominalWidthX`; both are in the table too, so the comparison covers the three cases at
/// once rather than only the easy one.
#[test]
fn a_real_cff_font_agrees_with_its_own_metrics_about_every_width() {
    use ttf_parser::{Face, GlyphId};

    let Some((path, data)) = fonts().into_iter().next() else {
        eprintln!(
            "skipped: no CFF font found; looked in {}",
            CANDIDATES.join(", ")
        );
        return;
    };
    let table = cff_bytes(&data).expect("a CFF table this test chose");
    let cff = Cff::parse(table, 0).unwrap_or_else(|why| panic!("{path} should parse: {why}"));
    assert!(
        !cff.is_cff2(),
        "{path} is a CFF 1 font, and its glyphs have widths"
    );

    // The comparison needs the `sfnt` wrapper for `hmtx`. A candidate list of bare CFF files
    // would not have one, and the test would then be comparing nothing.
    let Ok(face) = Face::parse(&data, 0) else {
        eprintln!("skipped: {path} has no `hmtx` table to check the widths against");
        return;
    };
    let mut mismatches = Vec::new();
    let mut compared = 0usize;
    for glyph in 0..cff.num_glyphs().min(usize::from(face.number_of_glyphs())) {
        let g = u32::try_from(glyph).expect("a glyph number");
        // `hmtx` is in the font's own units and the CFF width is in thousandths of an em, so
        // the two only mean the same number when the em is a thousand units — which is what
        // every CFF font whose FontMatrix is the default has.
        let Some(expected) = face.glyph_hor_advance(GlyphId(u16::try_from(glyph).expect("a gid")))
        else {
            continue;
        };
        let mils = u32::from(expected) * 1000 / u32::from(face.units_per_em());
        match cff.advance(g) {
            Ok(got) if u32::from(got) == mils => compared += 1,
            Ok(got) => mismatches.push(format!(
                "glyph {glyph}: charstring says {got}, hmtx says {mils}"
            )),
            Err(why) => mismatches.push(format!("glyph {glyph}: {why}")),
        }
    }
    eprintln!(
        "{path}: {compared} widths agree with `hmtx`, {} do not",
        mismatches.len()
    );
    assert!(
        compared > 100,
        "only {compared} glyphs compared, which is too few"
    );
    assert!(
        mismatches.is_empty(),
        "a glyph's width from its charstring must agree with the font's own metrics:\n  {}",
        mismatches
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}

/// Collects segments the way `ttf-parser` does, for a comparison against our own.
#[derive(Default)]
struct Collected {
    segments: Vec<(char, Vec<f64>)>,
}

impl ttf_parser::OutlineBuilder for Collected {
    fn move_to(&mut self, x: f32, y: f32) {
        self.segments.push(('M', vec![f64::from(x), f64::from(y)]));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.segments.push(('L', vec![f64::from(x), f64::from(y)]));
    }
    fn curve_to(&mut self, a: f32, b: f32, c: f32, d: f32, e: f32, f: f32) {
        self.segments.push((
            'C',
            vec![
                f64::from(a),
                f64::from(b),
                f64::from(c),
                f64::from(d),
                f64::from(e),
                f64::from(f),
            ],
        ));
    }
    fn quad_to(&mut self, _x1: f32, _y1: f32, _x2: f32, _y2: f32) {
        unimplemented!("a CFF outline has no quadratic segments")
    }
    /// A CFF contour is closed by the fill rule, so there is nothing to do here.
    fn close(&mut self) {}
}

/// One segment, in the form both readers' output is compared in.
type Step = (char, Vec<f64>);

/// Our segments in that form.
fn ours(segments: &[mangle_font::Segment]) -> Vec<Step> {
    segments
        .iter()
        .map(|s| match s {
            mangle_font::Segment::Move(x, y) => ('M', vec![*x, *y]),
            mangle_font::Segment::Line(x, y) => ('L', vec![*x, *y]),
            mangle_font::Segment::Curve(a, b, c, d, e, f) => ('C', vec![*a, *b, *c, *d, *e, *f]),
        })
        .collect()
}

/// Either reader's segments with each contour's closing edge removed.
///
/// A CFF contour is closed by the winding rule and no operator writes the closing edge, so
/// a reader may or may not produce one. This reader has to, because a fill by winding needs
/// the edge to exist; `ttf-parser` does not. And where a contour happens to end exactly where
/// it began the edge is redundant, so one reader drops it and the other keeps it. Putting
/// both sides through the same normalisation is what lets the rest be compared segment for
/// segment — and it is the *only* thing normalised, because every other difference is one
/// worth knowing about.
fn without_closing_edges(segments: &[Step]) -> Vec<Step> {
    let mut out: Vec<Step> = Vec::with_capacity(segments.len());
    let mut at = 0usize;
    while at < segments.len() {
        if segments[at].0 != 'M' {
            out.push(segments[at].clone());
            at += 1;
            continue;
        }
        let mut end = at + 1;
        while end < segments.len() && segments[end].0 != 'M' {
            end += 1;
        }
        let last = end.saturating_sub(1);
        let start = &segments[at].1;
        let redundant = segments[last].0 == 'L'
            && segments[last].1.len() == start.len()
            && segments[last]
                .1
                .iter()
                .zip(start)
                .all(|(a, b)| (a - b).abs() < 1e-12);
        let stop = if redundant { last } else { end };
        out.extend_from_slice(&segments[at..stop]);
        at = end;
    }
    out
}

/// Every glyph's outline is the outline `ttf-parser` reads from the same font.
///
/// This is the strongest check in the file and it is a cross-check rather than a restatement.
/// `ttf-parser` has its own CFF interpreter, written from the same specification and by
/// different hands; walking a font with this reader and with that one and comparing every
/// coordinate of every segment says something about *correctness* that no self-consistent
/// test can.
///
/// It found the defect that mattered most here. A charstring's curve operators give three
/// points as offsets **from the point before each of them** — the first from the pen, the
/// second from the first, the third from the second — and reading all three as offsets from
/// the pen produces a closed, plausible, wrong glyph rather than an error. Every letter came
/// out too narrow and every curve too flat: 0.99 SSIM against `mutool` for a TrueType face
/// and 0.82 for this one, on the same code.
///
/// The tolerance is a thousandth of a font unit, which is a millionth of an em. The
/// interpreter's arithmetic is `f32`, so a coordinate built by adding a few hundred deltas
/// lands within a hundred-thousandth of a unit of the same value computed in `f64`, and a
/// millionth of an em is far below what any renderer can show.
#[test]
fn every_outline_agrees_with_ttf_parsers_cff_reader() {
    use ttf_parser::{Face, GlyphId};

    let fonts = fonts();
    if fonts.is_empty() {
        eprintln!(
            "skipped: no CFF font found; looked in {}",
            CANDIDATES.join(", ")
        );
        return;
    }
    let (mut compared, mut mismatches, mut unread) = (0usize, Vec::new(), 0usize);
    for (path, data) in &fonts {
        // The comparison needs the `sfnt` wrapper, because `ttf-parser` reaches its
        // outlines through the table directory. A candidate list of bare CFF files would have
        // none.
        let (Ok(face), Some(table)) = (Face::parse(data, 0), cff_bytes(data)) else {
            continue;
        };
        let units = f64::from(face.units_per_em());
        let cff = Cff::parse(table, 0).expect("a font this test chose");
        for glyph in 0..cff.num_glyphs().min(usize::from(face.number_of_glyphs())) {
            let id = GlyphId(u16::try_from(glyph).expect("a glyph number"));
            let mut theirs = Collected::default();
            face.outline_glyph(id, &mut theirs);
            // A glyph the other reader could not walk at all says nothing about this one, and
            // every large font has a few.
            if theirs.segments.is_empty() {
                unread += 1;
                continue;
            }
            let g = u32::try_from(glyph).expect("a glyph number");
            let mine = without_closing_edges(&ours(&cff.outline(g).expect("a glyph").segments));
            let theirs = without_closing_edges(&theirs.segments);
            compared += 1;
            let differs = mine.len() != theirs.len()
                || mine.iter().zip(&theirs).any(|(a, b)| {
                    a.0 != b.0
                        || a.1
                            .iter()
                            .zip(&b.1)
                            .any(|(p, q)| (p * units - q).abs() > 1e-3)
                });
            if differs {
                let at = mine
                    .iter()
                    .zip(&theirs)
                    .position(|(a, b)| {
                        a.0 != b.0
                            || a.1
                                .iter()
                                .zip(&b.1)
                                .any(|(p, q)| (p * units - q).abs() > 1e-3)
                    })
                    .unwrap_or(mine.len());
                mismatches.push(format!(
                    "{path} glyph {glyph}: {} segments here, {} there, first at {at}\n    \
                     here  {:?}\n    there {:?}",
                    mine.len(),
                    theirs.len(),
                    mine.get(at).unwrap_or(&('?', Vec::new())),
                    theirs.get(at).unwrap_or(&('?', Vec::new()))
                ));
            }
        }
    }
    eprintln!("{compared} outlines agree with `ttf-parser`, {unread} it could not walk");
    assert!(
        compared > 500,
        "only {compared} outlines compared, which is too few to prove much"
    );
    assert!(
        mismatches.is_empty(),
        "every glyph of a real CFF font should read the same way in both readers:\n  {}",
        mismatches
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}
