//! An object's content, read over real corpus pages.
//!
//! # Why this is checked against the corpus
//!
//! The read-out is one call into `mangle_text`, and the honest thing to test is that a real page's
//! objects come back saying what a reader would say they say. The NIST publication is used because
//! its first page has a table with labels beside values, which is the shape that catches a
//! read-out that is reading the wrong run: the label "Series/Number:" and the value are two objects,
//! and a summary that carried the wrong one would show the wrong text with nothing else wrong.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use mangle_content::{ContentStream, Resources, interp};
use mangle_doc::PageTree;
use mangle_edit::{Kind, PageModel, summarise};
use mangle_syntax::object::Object;
use mangle_syntax::{Document, OpenOptions};

/// Page 1 of `name`, with its resources, or `None` if the corpus file is not fetched.
fn page_for(name: &str) -> Option<(PageModel, Resources, Document)> {
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
    Some((PageModel::build(&run.records), resources, doc))
}

/// Text objects come back saying what is on the page.
///
/// This is the check the panel's content field rests on: a summary that carried the wrong run would
/// show the wrong words with nothing else about the selection looking wrong, because every other
/// field on it — the box, the spans, the size — is about geometry the model computed correctly.
#[test]
fn text_objects_come_back_saying_what_is_on_the_page() {
    let name = "gov__nist-sp800-88.pdf";
    let Some((model, resources, doc)) = page_for(name) else {
        eprintln!("skipped: {name} is not fetched");
        return;
    };
    let summaries: Vec<_> = model
        .objects()
        .iter()
        .map(|o| summarise(&doc, &resources, o))
        .collect();
    let with_text = summaries.iter().filter(|s| s.content.is_some()).count();
    eprintln!("{name}: {} objects, {with_text} with text", summaries.len());
    assert!(
        with_text > 10,
        "{name} has {with_text} objects with text on them, which is not a page of text"
    );

    // Every object with text is a text-shaped object, and every one without is not: a photograph
    // says nothing, and an object that said something would not be one.
    for s in summaries.iter().filter(|s| s.content.is_some()) {
        assert!(
            matches!(s.kind, Kind::Line | Kind::Block | Kind::LooseRun),
            "{:?} has text but is not text-shaped",
            s.kind
        );
    }
    for s in summaries.iter().filter(|s| s.content.is_none()) {
        assert!(
            !matches!(s.kind, Kind::Line | Kind::Block | Kind::LooseRun),
            "{:?} is text-shaped but has no text, so the read-out skipped a run it should have \\
             read",
            s.kind
        );
    }

    // And the words are the page's own: the first page says it is an archived publication.
    let all: String = summaries
        .iter()
        .filter_map(|s| s.content.clone())
        .collect::<Vec<_>>()
        .join("\n");
    for wanted in ["Archived", "Publication", "Series/Number:"] {
        assert!(all.contains(wanted), "{name} does not contain {wanted:?}");
    }
}
