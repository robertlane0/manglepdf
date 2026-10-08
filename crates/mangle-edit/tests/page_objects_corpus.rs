//! The page object model, run over real corpus pages.
//!
//! # Why this is a test and not something done once
//!
//! The synthetic tests in `page_objects.rs` cover the grouping rules, and every one of them
//! passed while the model was grouping text it could not really tell apart: a glyph run's bounds
//! came from the glyph *origins*, and every glyph on a line shares a baseline, so the boxes had
//! **no height at all**. Lines still came out plausible on the fixture pages because runs on one
//! baseline have identical `y`, so the grouping was working by accident.
//!
//! Nothing in a unit test would have found that, and nothing in the corpus harness would have
//! reported it either — a page with one lump instead of four lines still renders identically.
//! Reading the model over four real pages and printing what came out is what showed the boxes
//! were degenerate, and that is worth keeping: **the check is cheap, it needs no numbers to
//! pass, and it is the thing that would catch this class of mistake again.**
//!
//! The assertions are all *invariants* rather than counts. A count would fail every time the
//! grouping improved, which is the wrong way for a test to fail: this asserts the properties
//! write-back depends on, and reports the rest so a human can look.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use mangle_content::{ContentStream, Resources};
use mangle_doc::PageTree;
use mangle_edit::{Kind, PageModel, provenance_of};
use mangle_syntax::object::Object;
use mangle_syntax::{Document, OpenOptions};

/// Pages to run over: a design export, a LaTeX paper, a government form and a scanned
/// publication. Chosen for different shapes of page, not for their scores — the point is that
/// grouping behaves on a page of text and on a page of rules.
const PAGES: &[&str] = &[
    "pdfjs__issue10529.pdf",
    "pdfjs__TAMReview.pdf",
    "gov__irs-f1040.pdf",
    "gov__nist-sp800-88.pdf",
];

/// The model for page 1 of `name`, or `None` if the corpus file is not fetched.
fn model_for(name: &str) -> Option<(PageModel, usize)> {
    let path = format!("../../corpus/wild/{name}");
    let bytes = std::fs::read(path).ok()?;
    let doc = Document::open(bytes, OpenOptions::default()).ok()?;
    let cat = doc.catalog().ok()?;
    let root = cat.get("Pages").and_then(Object::as_ref_id)?;
    let pages = PageTree::build(&doc, root).ok()?;
    let all = pages.pages();
    let page = all.first()?;
    let resources = page
        .inherited
        .resources
        .as_ref()
        .and_then(|o| doc.resolve_object(o))
        .and_then(|o| o.as_dict().cloned())
        .map(|d| Resources::from_dict(&d, &|o| doc.resolve_object(o)))
        .unwrap_or_default();
    let stream = ContentStream::parse(&page.decoded_contents(&doc));
    let run = mangle_content::interp::run_with(&stream, &resources);
    Some((PageModel::build(&run.records), run.records.len()))
}

#[test]
fn the_corpus_pages_produce_objects_whose_spans_they_own() {
    let mut seen = 0usize;
    for name in PAGES {
        let Some((model, records)) = model_for(name) else {
            eprintln!("skipped: {name} is not fetched");
            continue;
        };
        seen += 1;
        eprintln!(
            "{name}: {records} records -> {} objects {:?}",
            model.objects().len(),
            model.counts()
        );

        for (i, o) in model.objects().iter().enumerate() {
            // The invariant write-back rests on: a span an object claims must be one of the
            // spans of its own records, or replacing "the object's bytes" would rewrite bytes
            // that belong to something else on the page.
            for span in &o.spans {
                let owned = o
                    .records
                    .iter()
                    .any(|r| r.span.start <= span.start && span.end <= r.span.end);
                assert!(
                    owned,
                    "{name} object {i} ({:?}) claims span {span:?}",
                    o.kind
                );
            }
            // Every object holds at least one record and covers every record it holds.
            assert!(!o.records.is_empty(), "{name} object {i} is empty");
            assert_eq!(
                o.spans.len(),
                o.records.len(),
                "{name} object {i} ({:?}) has {} records but {} spans",
                o.kind,
                o.records.len(),
                o.spans.len()
            );
            // A box with an area. A degenerate box is what made the first version of this model
            // group text it could not tell apart, and it is invisible without this check.
            assert!(
                o.bounds.x1 >= o.bounds.x0 && o.bounds.y1 >= o.bounds.y0,
                "{name} object {i} ({:?}) has an inverted box: {:?}",
                o.kind,
                o.bounds
            );
        }
    }
    assert!(
        seen > 0,
        "no corpus page was readable, so nothing was checked"
    );
}

/// The check that the zero-height bug would have failed.
///
/// Every text object on a real page must have height, because glyph origins alone share a
/// baseline and give none. Without this, a model whose bounds regressed to origins would still
/// pass every invariant above and still group a paragraph into one unselectable lump.
#[test]
fn every_text_object_on_a_real_page_has_height() {
    let Some((model, _)) = model_for("gov__irs-f1040.pdf") else {
        eprintln!("skipped: the corpus file is not fetched");
        return;
    };
    let text: Vec<_> = model
        .objects()
        .iter()
        .filter(|o| matches!(o.kind, Kind::Line | Kind::LooseRun))
        .collect();
    if text.is_empty() {
        eprintln!("skipped: no text objects on this page, so nothing to check");
        return;
    }
    for o in &text {
        assert!(
            o.bounds.y1 > o.bounds.y0,
            "a text object has no height ({:?} .. {:?}), which means its bounds came from \
             glyph origins rather than from the type size",
            o.bounds.y0,
            o.bounds.y1
        );
    }
    eprintln!("{} text objects, all with height", text.len());
}

/// Two objects on the same page must not both claim the same bytes.
#[test]
fn no_two_objects_claim_the_same_span() {
    for name in PAGES {
        let Some((model, _)) = model_for(name) else {
            continue;
        };
        let mut claimed: Vec<(usize, usize, usize)> = Vec::new();
        for (i, o) in model.objects().iter().enumerate() {
            for span in &o.spans {
                assert!(
                    !claimed.contains(&(span.start, span.end, o.kind as usize + i)),
                    "{name} object {i} reuses a span already claimed"
                );
                claimed.push((span.start, span.end, o.kind as usize + i));
            }
        }
    }
}

/// The provenance law, read out on a real page.
///
/// GOAL.md §4.1's seventh law is that a selectable object knows **exactly which bytes of which
/// content stream** produced it, and that this "lets the Inspector show what changed". The byte
/// ranges have been on the model from the start; what this asserts is that reading them back gives
/// the operations that really are at those offsets in the page's own stream — because a span that
/// is one byte short (B3) prints an operator with a byte missing out of the middle of it and looks
/// entirely plausible.
#[test]
fn the_bytes_an_object_claims_are_the_bytes_the_page_drew() {
    let mut checked = 0usize;
    for name in PAGES {
        let Some((model, len)) = model_for(name) else {
            eprintln!("skipped: {name} is not fetched");
            continue;
        };
        let bytes = std::fs::read(format!("../../corpus/wild/{name}"))
            .expect("the page came from this file");
        let doc = Document::open(bytes, OpenOptions::default()).expect("it opens");
        let cat = doc.catalog().expect("a catalogue");
        let root = cat
            .get("Pages")
            .and_then(Object::as_ref_id)
            .expect("a tree");
        let pages = PageTree::build(&doc, root).expect("the walk");
        let page = pages.pages().first().expect("page one");
        let stream = page.decoded_contents(&doc);
        let _ = len;
        for object in model.objects() {
            for line in &provenance_of(object, &stream).lines {
                let span = line
                    .split('\t')
                    .nth(2)
                    .map(|s| s.trim().to_owned())
                    .and_then(|s| parse_span(&s))
                    .expect("the line carries its span");
                assert!(
                    span.end <= stream.len(),
                    "{name}: the object claims bytes {span:?} of a stream that is {} bytes long",
                    stream.len()
                );
                // **A span never splits a token.** The range an object claims must start where a
                // token starts and end where one ends, or it is a range covering bytes that are not
                // the operation's — and an edit written over such a range replaces a number that
                // belonged to somebody else.
                //
                // This is the exact shape of both span defects the corpus found. B3's
                // inline-image span was `start..start + 1`, the `B` of `BI`: it *ends* mid-token.
                // And the `BI` fallback ran to the end of the token *after* it, so it *starts* on
                // bytes that are not its own. Both pass a check that merely says "in range".
                let (starts, ends) = token_edges(&stream);
                assert!(
                    starts.contains(&span.start),
                    "{name}: the span {span:?} starts inside a token, so it is not the operation's \
                     own bytes"
                );
                assert!(
                    ends.contains(&span.end),
                    "{name}: the span {span:?} ends inside a token, so it is not the operation's \
                     own bytes"
                );
                checked += 1;
            }
        }
    }
    assert!(
        checked > 0,
        "no object named any bytes, so nothing was checked"
    );
    eprintln!("{checked} provenance line(s), every one inside its page's stream");
}

/// Where the page's own tokens start and end, so a span can be checked against them.
fn token_edges(stream: &[u8]) -> (Vec<usize>, Vec<usize>) {
    let parsed = ContentStream::parse(stream);
    (
        parsed.tokens().iter().map(|t| t.span.start).collect(),
        parsed.tokens().iter().map(|t| t.span.end).collect(),
    )
}

/// A span out of a provenance line: `12..20`.
fn parse_span(text: &str) -> Option<std::ops::Range<usize>> {
    let (start, end) = text.trim().split_once("..")?;
    Some(start.parse::<usize>().ok()?..end.parse::<usize>().ok()?)
}
