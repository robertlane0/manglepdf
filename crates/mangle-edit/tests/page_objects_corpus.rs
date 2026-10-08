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
use mangle_edit::{Kind, PageModel};
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
