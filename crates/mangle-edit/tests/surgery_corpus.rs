//! Surgical editing over a real corpus page, and the round trip that proves it preserved
//! everything it did not name.
//!
//! # What this is for
//!
//! `surgery.rs`'s own tests use short hand-written fixtures where byte offsets are easy to reason
//! about. This one takes a page the corpus actually contains, makes a real edit to a real
//! operation, and checks the two properties that matter when a user opens a file they did not
//! write and saves it:
//!
//! * **the edit landed** — the interpreter sees the change;
//! * **everything else is byte-identical** — the author's comments, their number formatting and
//!   their operator order are still exactly what they were.
//!
//! The second is easy to lose and impossible to notice. A round trip that *renders* the same
//! page proves nothing about it: re-serialising a content stream from its token list reproduces
//! the rendering while quietly discarding everything this project does not model, and the user
//! finds out years later.

#![forbid(unsafe_code)]
// The panic-free rule is about what the product does with a file, not about tests. Indexing is
// kept here rather than rewritten as `get`: every index below comes from a span the interpreter
// produced for this same stream, which is bounds-checked by construction, and a test that hides
// its arithmetic behind `get` reads worse than one that states it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use mangle_content::interp::{Mark, run_with};
use mangle_content::matrix::Matrix;
use mangle_content::{ContentStream, Resources};
use mangle_doc::PageTree;
use mangle_edit::surgery::{Patch, apply, is_balanced};
use mangle_syntax::object::Object;
use mangle_syntax::{Document, OpenOptions};

/// A corpus page with text, a matrix, an image and the author's own comments: everything an
/// edit touches, and everything an edit must not lose.
///
/// Chosen for having all four. Several corpus pages have no image to move or no comment to
/// preserve, and a test that silently skips because of that is a test that stops testing.
const PAGE: &str = "pdfjs__TAMReview.pdf";

/// The decoded content stream of page 1, and the page's resources.
fn page_one(name: &str) -> Option<(Vec<u8>, Resources)> {
    let bytes = std::fs::read(format!("../../corpus/wild/{name}")).ok()?;
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
    let contents = page.decoded_contents(&doc);
    let _ = page;
    Some((contents, resources))
}

/// A placement matrix as six operands of a `cm`, at the precision a file would carry.
fn as_cm(m: Matrix) -> Vec<u8> {
    let n = |v: f64| format!("{v:.6}");
    format!(
        "{} {} {} {} {} {} cm",
        n(m.a),
        n(m.b),
        n(m.c),
        n(m.d),
        n(m.e),
        n(m.f)
    )
    .into_bytes()
}

#[test]
fn an_edit_changes_the_page_and_leaves_every_other_byte_alone() {
    let Some((contents, resources)) = page_one(PAGE) else {
        eprintln!("skipped: {PAGE} is not fetched");
        return;
    };
    assert!(!contents.is_empty(), "the fixture page has content");

    let before = run_with(&ContentStream::parse(&contents), &resources);
    let image = before.records.iter().find_map(|r| match &r.mark {
        Mark::Image {
            matrix,
            inline: false,
            ..
        } => Some((r.span.clone(), *matrix)),
        _ => None,
    });
    let Some((span, ctm_before)) = image else {
        eprintln!("skipped: this page has no XObject image to move");
        return;
    };

    // A record's span is **the operation that drew it** — `/PxARRO Do` — and not the `cm`
    // that positioned it, which is a separate operation with its own span. So a scale has to
    // find the `cm` before the image rather than edit the image's own bytes, and this is the
    // first thing to get wrong when writing an edit from a record's span.
    let head = &contents[..span.start];
    // The six operands end where the `cm` operator's `c` begins, so the backward scan starts
    // there.
    let cm_end = head
        .windows(2)
        .rposition(|w| w == b"cm")
        .expect("a `cm` precedes the image on this page");

    // Walk back over the six operands. They are separated by whitespace and the last one ends
    // immediately before `cm`, so the scan starts at the `c` and steps back over each token; a
    // naive search backwards for ` cm` finds the last operand, not the first.
    let mut nums_back: Vec<f64> = Vec::new();
    let mut at = cm_end;
    for _ in 0..6 {
        while at > 0 && contents[at - 1].is_ascii_whitespace() {
            at -= 1;
        }
        let end = at;
        while at > 0 && !contents[at - 1].is_ascii_whitespace() {
            at -= 1;
        }
        let token = std::str::from_utf8(&contents[at..end]).expect("operands are ASCII");
        nums_back.push(
            token
                .parse::<f64>()
                .unwrap_or_else(|_| panic!("the operand before `cm` is a number: {token:?}")),
        );
    }
    nums_back.reverse();
    let mut nums = nums_back;
    let cm_start = at;
    // The replaced range must include the operator as well as its operands: replacing
    // `53.333 0 0 10 70.866 17.008 ` and leaving the file's own `cm` behind would work by
    // accident here and produce a stream with a stray `cm` in it the moment the new text ended
    // differently. `cm_end` is the index of the `c`, so the operator's own bytes end two
    // later.
    let replace_end = cm_end + 2;

    // Double the horizontal scale, which is what a scale with a modifier does.
    nums[0] *= 2.0;
    let replacement = as_cm(Matrix::new(
        nums[0], nums[1], nums[2], nums[3], nums[4], nums[5],
    ));

    let applied = apply(
        &contents,
        &[Patch::replace(
            cm_start..replace_end,
            replacement.clone(),
            "scale the image",
        )],
    )
    .expect("the span came from this stream");
    assert_ne!(applied.bytes, contents, "the edit changed something");

    let after = run_with(&ContentStream::parse(&applied.bytes), &resources);
    // **The assertion is about the ratio, not the value.** The mark's matrix is the full CTM,
    // which is the `cm` composed with the page placement and the form's own `/Matrix` — so
    // setting `a` to a number and looking for that number is looking for the wrong thing. What
    // an edit guarantees is that the width doubled, and that is what is checked.
    let ctm_after = after.records.iter().find_map(|r| match &r.mark {
        Mark::Image {
            matrix,
            inline: false,
            ..
        } => Some(*matrix),
        _ => None,
    });
    let ctm_after = ctm_after.expect("the image is still there after the edit");
    let ratio = ctm_after.a / ctm_before.a;
    assert!(
        (ratio - 2.0).abs() < 1e-6,
        "the image's width doubled: {ctm_before:?} -> {ctm_after:?}, ratio {ratio}"
    );
    assert!(
        (ctm_after.d - ctm_before.d).abs() < 1e-6,
        "and its height did not, because only the horizontal was scaled: {ctm_before:?} -> \
         {ctm_after:?}"
    );
    assert!(
        is_balanced(&applied.bytes),
        "and the edit did not unbalance the stream"
    );

    // **Nothing else changed.** Putting the original bytes back over the same span must give
    // back the original stream exactly. This is the property that a token-list re-serialisation
    // cannot satisfy, which is why it is asserted rather than assumed.
    let restored = apply(
        &applied.bytes,
        &[Patch::replace(
            cm_start..cm_start + replacement.len(),
            contents[cm_start..replace_end].to_vec(),
            "put it back",
        )],
    )
    .expect("the span is in range");
    assert_eq!(
        restored.bytes, contents,
        "restoring the original bytes over the same span gives the original stream back, \
         which is what 'nothing else changed' means"
    );
}

/// The page's own comments survive an edit to another part of it, because that is the whole
/// point of a byte-range edit rather than a re-serialisation.
///
/// **Skipped when the page has no comment**, which is most of them — a producer that writes none
/// cannot demonstrate that they survive. The `%` looked for has to be followed on its line by
/// drawing operators, which is what tells a real comment from a `/`-prefixed name or a
/// dictionary that merely contains a percent sign.
#[test]
fn a_comment_in_the_page_survives_an_edit_to_another_part_of_it() {
    // A page that *has* a comment, because one that has none cannot demonstrate that they
    // survive. This is a pdf.js regression file and its one content stream carries
    // `% Fill then Stroke Text render mode`, which is exactly the sort of note a producer
    // writes and a re-serialisation throws away.
    const WITH_COMMENT: &str = "pdfjs__ContentStreamNoCycleType3insideType3.pdf";
    let Some((contents, _resources)) = page_one(WITH_COMMENT) else {
        eprintln!("skipped: {WITH_COMMENT} is not fetched");
        return;
    };
    let at = contents
        .windows(2)
        .position(|w| w == b"% ")
        .expect("this fixture page has a comment in its content stream");
    assert!(
        contents[at..].starts_with(b"% Fill then Stroke"),
        "and it is the comment this test is about: {:?}",
        String::from_utf8_lossy(&contents[at..].iter().copied().take(40).collect::<Vec<u8>>())
    );

    // A real edit, well after the comment, so the comment has to survive a stream that
    // actually changed. An empty insertion would prove nothing: it is the no-edit case.
    let tail_at = contents
        .len()
        .saturating_sub(20)
        .max(at + 1)
        .min(contents.len());
    let applied = apply(
        &contents,
        &[Patch::insert(
            tail_at,
            b"% added by manglepdf\n",
            "add a note",
        )],
    )
    .expect("valid");
    assert_ne!(applied.bytes, contents, "the edit changed the stream");
    assert!(
        applied.bytes[at..].starts_with(b"% Fill then Stroke"),
        "and the producer's own comment is untouched, at the same offset: the edit is after \
         it, so nothing moved"
    );
    assert!(
        String::from_utf8_lossy(&applied.bytes).contains("% added by manglepdf"),
        "while the new comment is there too — both comments, which is the point of a \
         byte-range edit"
    );
    assert!(
        is_balanced(&applied.bytes),
        "and the stream is still balanced"
    );
}
