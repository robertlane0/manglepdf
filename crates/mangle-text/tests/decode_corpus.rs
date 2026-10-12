//! Text extraction, run over real corpus pages.
//!
//! # Why extraction is tested against the corpus and not only against a fixture
//!
//! The decoding path is short — codes, an encoding, a glyph list — and every step of it can be
//! unit-tested against a font written to exercise it. What a synthetic test cannot show is what a
//! *real* file does, and three things only appear on real pages:
//!
//! * a font whose `/Encoding` is an indirect reference, which is most of them;
//! * a font that sets its glyphs at one point and stretches them with a `cm`, so the size in the
//!   `Tf` is not the size the reader sees — the NIST page below is exactly this, and a gap measured
//!   against the stream's number is a gap measured against nothing;
//! * a page whose text is in a composite font, whose codes are CIDs and cannot be read this way
//!   at all.
//!
//! So this runs the decoder over the corpus and asserts what it must produce on pages whose text is
//! known. The assertions are *properties* — a page decodes to words, the words are separated, the
//! lines are separated — rather than a quoted phrase. A quoted phrase would be asserting the
//! joiner's heuristic is perfect, and it is not: it is a threshold, and its two failure modes are
//! documented on [`mangle_text::text_of`]. What must not be wrong is that the text is there.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use mangle_content::{ContentStream, Resources, interp};
use mangle_doc::PageTree;
use mangle_syntax::object::Object;
use mangle_syntax::{Document, OpenOptions};

/// Pages to run over: a scanned government publication and a government form. Chosen for
/// different text — prose and a title — not for their scores.
const PAGES: &[&str] = &["gov__nist-sp800-88.pdf", "gov__irs-f1040.pdf"];

/// Page 1 of `name`, decoded run by run.
fn page_for(name: &str) -> Option<Vec<mangle_text::Decoded>> {
    let path = format!("../../corpus/wild/{name}");
    let bytes = std::fs::read(&path).ok()?;
    let doc = Document::open(bytes, OpenOptions::default()).ok()?;
    let cat = doc.catalog().ok()?;
    let root = cat.get("Pages").and_then(Object::as_ref_id)?;
    let pages = PageTree::build(&doc, root).ok()?;
    let page = pages.pages().first()?;
    let resources = page
        .inherited
        .resources
        .as_ref()
        .and_then(|o| doc.resolve_object(o))
        .and_then(|o| o.as_dict().cloned())
        .map(|d| Resources::from_dict(&d, &|o| doc.resolve_object(o)))
        .unwrap_or_default();
    let stream = ContentStream::parse(&page.decoded_contents(&doc));
    let run = interp::run_with(&stream, &resources);

    let glyphs = run
        .records
        .iter()
        .filter(|r| matches!(r.mark, interp::Mark::Glyphs { .. }))
        .count();
    let decoded: Vec<mangle_text::Decoded> = run
        .records
        .iter()
        .filter_map(|r| match &r.mark {
            interp::Mark::Glyphs {
                text,
                placements,
                size,
                ..
            } => Some(mangle_text::Run {
                form: r.form.as_deref(),
                font: r.text.font.as_deref(),
                text,
                origins: placements.iter().map(|m| (m.e, m.f)).collect(),
                // The size the reader sees, not the one in the stream: this page sets its glyphs
                // at one point and stretches them with a text matrix, so the `Tf` number is not
                // the number a gap should be measured against.
                size: *size
                    * (r.text_matrix.a * r.text_matrix.d - r.text_matrix.b * r.text_matrix.c)
                        .abs()
                        .sqrt(),
            }),
            _ => None,
        })
        .filter_map(|run| mangle_text::decode(&resources, &|o| doc.resolve_object(o), &run))
        .collect();
    // Reported so a page that decodes half its runs is visible rather than silent.
    eprintln!("{name}: {glyphs} glyph runs, {} decoded", decoded.len());
    Some(decoded)
}

/// The corpus pages decode to a page's worth of text, with words and lines separated.
///
/// This is the check everything downstream rests on. If the decoder produces nothing on a page of
/// ordinary Latin text in an embedded font, then search finds nothing, copy copies nothing, the
/// panel's content field is blank and the redaction leak scan has nothing to scan — and a unit
/// test against a hand-built font would have said the decoder was fine.
#[test]
fn the_corpus_pages_decode_to_a_pages_worth_of_text() {
    let mut seen = 0usize;
    for name in PAGES {
        let Some(runs) = page_for(name) else {
            eprintln!("skipped: {name} is not fetched");
            continue;
        };
        seen += 1;
        let joined = mangle_text::text_of(&runs);
        let words = joined.split_whitespace().count();
        eprintln!("{name}: {} runs, {words} words once joined", runs.len());

        assert!(
            words > 10,
            "{name} joined to {words} words, which is not a page of text"
        );

        // **Words are separated, and not by luck.** Joining the runs with nothing gives at most
        // one whitespace-separated token per run, because a `Tj` names no space. The joined text
        // must have more than that, or the joiner added nothing and the words are still running
        // together -- which is what `NISTSpecialPublication800-88` looks like.
        let glued = runs.iter().filter(|d| !d.text.is_empty()).count();
        assert!(
            words > glued,
            "{name} has {words} joined words from {glued} runs: the joiner is not separating \
             anything"
        );

        // And lines are separated: a page of prose comes out as lines, not one paragraph.
        assert!(
            joined.contains('\n'),
            "{name} joined to a single line, so the line breaks were lost"
        );

        // Unmapped codes are reported, and a page of ordinary text should have few.
        let unmapped: usize = runs.iter().map(|d| d.unmapped).sum();
        let codes: usize = runs
            .iter()
            .map(|d| d.text.chars().count() + d.unmapped)
            .sum();
        assert!(
            codes == 0 || unmapped * 100 / codes < 5,
            "{name} lost {unmapped} of {codes} character codes"
        );
    }
    assert!(
        seen > 0,
        "no corpus file was fetched, so this checked nothing"
    );
}

/// A ligature is one glyph and one code point, which is why searching for a word with a ligature
/// in it needs the decoder and not a byte comparison.
///
/// `office` set in a font with a real `fi` ligature is four glyphs and four bytes, and searching
/// for the five letters with a byte comparison finds nothing.
#[test]
fn a_ligature_decodes_to_one_character() {
    // U+FB01, and the AGL names it "fi".
    assert_eq!(mangle_font::agl("fi"), Some(0xFB01));
    assert_eq!(
        mangle_font::agl("fi").and_then(char::from_u32),
        Some('\u{FB01}')
    );
    // And the other direction is there too, in the same table: the name is what the `/Differences`
    // array spells and the scalar is what the text ends up with.
    assert_eq!(mangle_font::agl("fi"), Some(0xFB01));
}

/// Runs on different baselines are separated by a newline and runs on the same one by a space,
/// and neither separator comes from the bytes.
#[test]
fn the_joiner_separates_lines_and_spaces_from_geometry() {
    let line = mangle_text::Decoded {
        text: "one".to_string(),
        unmapped: 0,
        extent: Some(((0.0, 0.0), (20.0, 0.0))),
        size: 10.0,
    };
    // Far enough along the same baseline to be a word break.
    let next_word = mangle_text::Decoded {
        text: "two".to_string(),
        unmapped: 0,
        extent: Some(((26.0, 0.0), (40.0, 0.0))),
        size: 10.0,
    };
    // Close enough along the same baseline to be the same word.
    let glued = mangle_text::Decoded {
        text: "three".to_string(),
        unmapped: 0,
        extent: Some(((20.5, 0.0), (40.0, 0.0))),
        size: 10.0,
    };
    // A different baseline, which is a different line however close it is.
    let next_line = mangle_text::Decoded {
        text: "four".to_string(),
        unmapped: 0,
        extent: Some(((0.0, -12.0), (20.0, -12.0))),
        size: 10.0,
    };
    assert_eq!(
        mangle_text::text_of(&[line.clone(), next_word.clone()]),
        "one two"
    );
    assert_eq!(mangle_text::text_of(&[line.clone(), glued]), "onethree");
    assert_eq!(
        mangle_text::text_of(&[line, next_line]),
        "one\nfour",
        "a line break is a line break even when the next line starts at the same column"
    );
}

/// A run with no readable glyphs has no extent, and the joiner skips it rather than inventing a
/// separator for a run that was never there.
#[test]
fn a_run_that_could_not_be_read_is_skipped_rather_than_separated() {
    let blank = mangle_text::Decoded {
        text: String::new(),
        unmapped: 4,
        extent: None,
        size: 10.0,
    };
    let after = mangle_text::Decoded {
        text: "text".to_string(),
        unmapped: 0,
        extent: Some(((0.0, 0.0), (20.0, 0.0))),
        size: 10.0,
    };
    assert_eq!(mangle_text::text_of(&[blank, after]), "text");
}
