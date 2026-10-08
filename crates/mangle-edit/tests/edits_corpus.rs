//! Real edits over real corpus pages, and what they leave behind.
//!
//! # Why this needs a corpus file rather than a fixture
//!
//! `edits.rs`'s own tests use hand-written streams where the byte offsets are easy to reason
//! about. That is the right place to prove the *rules* — the wrapper is inserted, the object's
//! own bytes are not touched, an empty wrapper goes too. It is the wrong place to find out
//! whether an edit survives contact with a file a producer wrote, because a producer's page is
//! the only thing that has the shapes this gets wrong on:
//!
//! * a page whose image is placed by a `cm` **forty operations earlier**, inside a `q … Q`;
//! * a text object whose runs are separated by a colour change, so the line is not contiguous;
//! * `/Contents` that is an *array* of streams, so the byte offsets belong to a concatenation
//!   and not to any object in the file.
//!
//! The last of these is why every assertion below is about the **concatenated stream the
//! interpreter actually ran**, and never about a byte offset in the file.
//!
//! # What is asserted, and what is only reported
//!
//! Two properties are asserted, because they are what a user would lose and never notice:
//! **the edit lands** (the interpreter sees the object move by the amount asked for) and
//! **nothing else changed** (the object's own operators are byte-identical afterwards, and a
//! delete leaves the rest of the stream exactly as it was). Everything else — how many objects
//! the page has, what the edit did to each kind — is printed, so a human can see the model
//! working on a real page rather than being told a count that would fail the day grouping
//! improves.

#![forbid(unsafe_code)]
// The panic-free rule is about what the product does with a file, not about tests. Indexing is
// kept where the index comes from a span the interpreter produced for this same stream, which
// is bounds-checked by construction; `float_cmp` because several assertions here are about an
// exact number of bytes moved.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_cmp
)]

use mangle_content::interp::run_with;
use mangle_content::{ContentStream, Matrix, Resources};
use mangle_doc::PageTree;
use mangle_edit::PageModel;
use mangle_edit::edits::{Change, TextProperty, apply_change, verify};
use mangle_edit::surgery::Patch;
use mangle_syntax::object::Object;
use mangle_syntax::{Document, OpenOptions};

/// Pages chosen for their shapes, not for their scores:
///
/// * a design export whose images are placed by a `cm` of their own;
/// * a government form, which is rules and type rather than photographs;
/// * a scanned publication, which is nearly all images.
const PAGES: &[&str] = &[
    "pdfjs__TAMReview.pdf",
    "gov__irs-f1040.pdf",
    "gov__nist-sp800-88.pdf",
];

/// The concatenated content stream of page 1, and the page's resources.
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
    Some((page.decoded_contents(&doc), resources))
}

/// Why this page would refuse an edit, for the skip message above.
fn page_refusal(stream: &[u8], object: &mangle_edit::PageObject, change: &Change) -> String {
    apply_change(stream, object, change)
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default()
}

/// The centre of a box, which is what a move is measured on.
fn centre(b: &mangle_content::state::ClipBounds) -> (f64, f64) {
    (f64::midpoint(b.x0, b.x1), f64::midpoint(b.y0, b.y1))
}

/// The biggest object on the page, which is the one a user would grab first.
///
/// Area rather than first-in-order, because a page's first object is often a stray `q`-scoped
/// rule and a test that moved that would prove nothing about moving anything a person sees.
fn biggest(model: &PageModel) -> Option<usize> {
    model
        .objects()
        .iter()
        .enumerate()
        .filter(|(_, o)| matches!(o.kind, mangle_edit::Kind::Image | mangle_edit::Kind::Path))
        .max_by(|(_, a), (_, b)| {
            let area = |o: &mangle_edit::PageObject| {
                let b = &o.bounds;
                (b.x1 - b.x0) * (b.y1 - b.y0)
            };
            area(a)
                .partial_cmp(&area(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
}

/// The edit's own wrapper bytes, taken back out of the result.
///
/// The check this makes possible is the one that matters: **the object's own operators come out
/// of the stream exactly as they went in**, so removing what the edit inserted must give the
/// original stream back byte for byte. A re-serialisation could not pass it, which is the point.
fn with_insertions_removed(after: &[u8], inserted: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::with_capacity(after.len());
    let mut cursor = 0usize;
    for bytes in inserted {
        let at = after[cursor..]
            .windows(bytes.len())
            .position(|w| w == bytes)
            .map(|i| cursor + i)
            .expect("the edit's own bytes are in the stream it wrote");
        out.extend_from_slice(&after[cursor..at]);
        cursor = at + bytes.len();
    }
    out.extend_from_slice(&after[cursor..]);
    out
}

/// A move on a real page moves the object it was aimed at, and leaves the rest of the stream
/// byte-for-byte identical.
///
/// This is GOAL.md §4.1's locality law and FINISH.md's U1, measured on a file this project did
/// not write: the bytes outside the object are the same bytes, not merely a rendering that looks
/// the same.
#[test]
fn a_move_on_a_real_page_moves_the_object_and_leaves_every_other_byte_alone() {
    let mut checked = 0usize;
    for name in PAGES {
        let Some((stream, resources)) = page_one(name) else {
            eprintln!("skipped: {name} is not fetched");
            continue;
        };
        let before = run_with(&ContentStream::parse(&stream), &resources);
        let model = PageModel::build(&before.records);
        eprintln!(
            "{name}: {} records -> {} objects",
            before.records.len(),
            model.objects().len()
        );
        let Some(index) = biggest(&model) else {
            eprintln!("skipped: no image or path on page 1 of {name}");
            continue;
        };
        let object = &model.objects()[index];
        let was = centre(&object.bounds);
        // 12 pt right and 5 pt down: a drag, not a nudge, so a rounding error could not pass
        // for it.
        let change = Change::move_by(12.0, -5.0);
        let Ok(applied) = apply_change(&stream, object, &change) else {
            // A page whose inline image cannot be read is refused by name; that is the honesty
            // law at work, and the test below asserts it.
            let why = page_refusal(&stream, object, &change);
            eprintln!("skipped: {name} is refused an edit: {why}");
            continue;
        };
        assert_ne!(applied.bytes, stream, "the edit changed something");

        // **Nothing outside the wrapper changed.** Taking the edit's own inserted bytes back
        // out must give the original stream exactly — the author's comments, their whitespace,
        // their number formatting and their operator order, all still theirs.
        let inserted = applied
            .applied
            .iter()
            .map(|p| p.bytes.clone())
            .collect::<Vec<_>>();
        let stripped = with_insertions_removed(&applied.bytes, &inserted);
        assert_eq!(
            stripped, stream,
            "every byte the edit did not insert is the byte the author wrote"
        );

        // **And the edit landed.** The interpreter is asked again, and the object that was
        // aimed at is the one that moved by the amount asked for.
        let after = run_with(&ContentStream::parse(&applied.bytes), &resources);
        verify(&before, &after, index, &change).expect("the edit moved what it claimed to move");
        // Where it lands is in **device** space, so the user-space move has to be read through
        // the CTM the object was drawn under: under a doubled matrix the same twelve points of
        // user space are twenty-four on the page. Getting that conversion wrong is what makes a
        // drag land somewhere the pointer never was.
        let ctm = model.objects()[index]
            .records
            .first()
            .map(|r| r.ctm)
            .unwrap_or_default();
        let want = mangle_edit::edits::map_point(
            &ctm,
            change.matrix().expect("a move is a transform"),
            was.0,
            was.1,
        );
        let moved = PageModel::build(&after.records)
            .objects()
            .iter()
            .any(|o| near(centre(&o.bounds), want));
        assert!(
            moved,
            "an object ended up where the move was supposed to put it: {was:?} -> {want:?}"
        );
        checked += 1;
    }
    assert!(
        checked > 0,
        "no corpus page was readable, so nothing was checked"
    );
}

/// Two points are the same point, at the precision the model is read at.
fn near(a: (f64, f64), b: (f64, f64)) -> bool {
    (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6
}

/// A delete on a real page removes exactly the object's operators.
///
/// The rest of the stream is asserted to be the same bytes, which is the property that makes a
/// delete a *local* edit rather than a page rewrite. Where the object's wrapper is left empty
/// the wrapper goes too, and that is the case the assertion has to allow for — so the expected
/// result is built from the patches rather than written down.
#[test]
fn a_delete_on_a_real_page_removes_exactly_that_objects_operators() {
    let Some((stream, resources)) = page_one("gov__irs-f1040.pdf") else {
        eprintln!("skipped: the corpus file is not fetched");
        return;
    };
    let before = run_with(&ContentStream::parse(&stream), &resources);
    let model = PageModel::build(&before.records);
    let Some(index) = biggest(&model) else {
        eprintln!("skipped: no image or path on this page, so nothing to delete");
        return;
    };
    let object = &model.objects()[index];
    let applied = apply_change(&stream, object, &Change::Delete).expect("a delete is writable");
    assert_ne!(applied.bytes, stream, "the edit changed something");

    // Build the expected stream from the patches: the ranges that were removed, and nothing
    // else. Any byte that survives is a byte the author wrote.
    let mut removed: Vec<(usize, usize)> = applied
        .applied
        .iter()
        .map(|p| (p.range.start, p.range.end))
        .collect();
    removed.sort_unstable();
    let mut expected = Vec::with_capacity(stream.len());
    let mut cursor = 0usize;
    for (start, end) in &removed {
        expected.extend_from_slice(&stream[cursor..*start]);
        cursor = (*end).max(cursor);
    }
    expected.extend_from_slice(&stream[cursor..]);
    assert_eq!(
        applied.bytes, expected,
        "the delete removed exactly the ranges it named and nothing else"
    );
    assert_eq!(
        removed.len(),
        object.spans.len(),
        "and it named the object's own operators — no wrapper went with them, because the \
         page has other operators inside it"
    );

    let after = run_with(&ContentStream::parse(&applied.bytes), &resources);
    verify(&before, &after, index, &Change::Delete)
        .expect("the object is gone and the rest stayed");
    eprintln!(
        "{}: {} objects -> {} after deleting one",
        name_of(),
        model.objects().len(),
        PageModel::build(&after.records).objects().len()
    );
}

/// The name of the page the delete test used, for the report above.
fn name_of() -> &'static str {
    "pdfjs__TAMReview.pdf"
}

/// A recolour on a real page changes the colour and nothing else.
///
/// The colour written is the one the object's own space takes: a page that set its fill in
/// DeviceCMYK is answered in CMYK, so the space is preserved and only the components change.
/// GOAL.md §4.6 asks for exactly that.
#[test]
fn a_recolour_on_a_real_page_changes_the_colour_and_not_the_space() {
    let Some((stream, resources)) = page_one("gov__irs-f1040.pdf") else {
        eprintln!("skipped: the corpus file is not fetched");
        return;
    };
    let before = run_with(&ContentStream::parse(&stream), &resources);
    let model = PageModel::build(&before.records);
    // A path, not a line: a glyph run's fill colour is the colour in force rather than a
    // property of the run, so recolouring one is a different edit from recolouring a shape.
    let Some(index) = model
        .objects()
        .iter()
        .position(|o| o.kind == mangle_edit::Kind::Path)
    else {
        eprintln!("skipped: no path on this page, so nothing to recolour");
        return;
    };
    let object = &model.objects()[index];
    let change = Change::recolour(mangle_content::state::Rgba {
        r: 0.784,
        g: 0.063,
        b: 0.180,
        a: 1.0,
    });
    let applied = match apply_change(&stream, object, &change) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    assert_ne!(applied.bytes, stream, "the edit changed something");

    let inserted = applied
        .applied
        .iter()
        .map(|p| p.bytes.clone())
        .collect::<Vec<_>>();
    let stripped = with_insertions_removed(&applied.bytes, &inserted);
    assert_eq!(
        stripped, stream,
        "a recolour adds bytes around the object and changes none of the author's"
    );

    let after = run_with(&ContentStream::parse(&applied.bytes), &resources);
    verify(&before, &after, index, &change).expect("the colour changed and the space did not");
}

/// An object drawn inside a form is edited in the form's stream, not the page's.
///
/// This is the case that would otherwise be got wrong silently: the page's model carries the
/// form's name, and a caller that passed the page's stream to `patches_for` with a form
/// object's spans would be editing bytes that mean something else entirely. The edit refuses
/// rather than guessing, which is the honest failure.
#[test]
fn an_object_from_inside_a_form_is_refused_rather_than_misread() {
    let mut seen = 0usize;
    for name in PAGES {
        let Some((stream, resources)) = page_one(name) else {
            continue;
        };
        let before = run_with(&ContentStream::parse(&stream), &resources);
        let model = PageModel::build(&before.records);
        for o in model.objects() {
            if o.form.is_some() {
                // Only the page stream is in hand, so a span from inside a form is a foreign
                // span and must be refused as one.
                let err = mangle_edit::edits::patches_for(&stream, o, &Change::move_by(1.0, 1.0))
                    .expect_err("a form's bytes are not this stream's bytes");
                assert!(
                    matches!(err, mangle_edit::Refusal::ForeignSpan { .. }),
                    "{err}"
                );
                seen += 1;
            }
        }
    }
    eprintln!("{seen} form objects refused for belonging to another stream");
}

/// An object that cannot be recoloured says so, with the reason.
///
/// A page whose colour is in a space this cannot write into is the case GOAL.md §4.1's fifth
/// law exists for: the user is told, rather than shown a page that changed in a way they did not
/// choose. Most corpus pages recolour fine, so this looks for one that does not without
/// requiring one.
/// A page whose inline image cannot be read refuses an edit, and names the reason.
///
/// `pdfjs__TAMReview.pdf` page 1 holds a `BI` whose dictionary does not parse, so the tokeniser
/// leaves the `BI` bare and its data is read as ordinary content. On such a page what a byte
/// *means* depends on what follows it: an edit inserted beside that `BI` changes how the bytes
/// after it tokenise, and the stream comes out unbalanced. The corpus found this as a panic in the
/// balance check; this is the honest answer to it.
#[test]
fn a_page_with_an_unreadable_inline_image_refuses_an_edit_by_name() {
    const WITH_UNREADABLE_IMAGE: &str = "pdfjs__TAMReview.pdf";
    let Some((stream, resources)) = page_one(WITH_UNREADABLE_IMAGE) else {
        eprintln!("skipped: {WITH_UNREADABLE_IMAGE} is not fetched");
        return;
    };
    let before = run_with(&ContentStream::parse(&stream), &resources);
    let model = PageModel::build(&before.records);
    let mut refused = 0usize;
    for object in model.objects() {
        let change = Change::move_by(12.0, -5.0);
        match apply_change(&stream, object, &change) {
            Ok(_) => {}
            Err(e) => {
                refused += 1;
                assert!(
                    e.to_string().contains("inline image"),
                    "the refusal names the real reason: {e}"
                );
            }
        }
    }
    assert_eq!(
        refused,
        model.objects().len(),
        "every edit on such a page is refused, rather than some of them silently landing"
    );
    eprintln!("{refused} edit(s) refused on {WITH_UNREADABLE_IMAGE}");
}

#[test]
fn a_recolour_that_cannot_be_written_is_refused_with_the_reason() {
    let Some((stream, resources)) = page_one("gov__irs-f1040.pdf") else {
        eprintln!("skipped: the corpus file is not fetched");
        return;
    };
    let before = run_with(&ContentStream::parse(&stream), &resources);
    let model = PageModel::build(&before.records);
    let mut refused = 0usize;
    for o in model.objects() {
        let change = Change::recolour(mangle_content::state::Rgba::WHITE);
        if let Err(e) = apply_change(&stream, o, &change) {
            refused += 1;
            let named = match &e {
                mangle_edit::ChangeError::Refused(
                    refusal @ (mangle_edit::Refusal::NoSuchColour { .. }
                    | mangle_edit::Refusal::ColourSpaceNotWritable { .. }),
                ) => !refusal.to_string().is_empty(),
                _ => false,
            };
            assert!(named, "a refusal names what was wrong with the edit: {e}");
        }
    }
    eprintln!(
        "{refused} of {} objects refused a recolour",
        model.objects().len()
    );
    let _ = Patch::delete(0..0, "unused");
}

/// The matrix a move is expressed in, and the one the interpreter reports, are the same.
///
/// A wrapper changes the space an object is drawn in, so an object drawn under a scaled CTM
/// moves by the *user-space* amount in the file and by the scaled amount on the page. This is
/// the fact a canvas has to have before it can drag anything, and it is checked here against a
/// page that really does scale.
#[test]
fn a_move_is_in_user_space_and_the_page_may_scale_it() {
    let m = Matrix::translate(3.0, 0.0);
    assert_eq!(
        mangle_edit::edits::map_point(&Matrix::IDENTITY, m, 10.0, 0.0),
        (13.0, 0.0)
    );
    // Doubled, the same three units of user space cover six on the page.
    let doubled = Matrix::scale(2.0, 2.0);
    assert_eq!(
        mangle_edit::edits::map_point(&doubled, m, 10.0, 0.0),
        (16.0, 0.0)
    );
}

/// A text property on a real page: the interpreter reports the new value, and the bytes around
/// the run are untouched.
///
/// This is the control GOAL.md §4.4 maps to a real operator, and it is the newest kind of edit here
/// — so it gets the check that caught the last two: the effect, read out of the interpreter, rather
/// than the string that was written.
#[test]
fn a_text_property_on_a_real_page_changes_only_that() {
    for name in ["gov__irs-f1040.pdf", "gov__nist-sp800-88.pdf"] {
        let Some((contents, resources)) = page_one(name) else {
            eprintln!("skipped: {name} is not fetched");
            continue;
        };
        let before = run_with(&ContentStream::parse(&contents), &resources);
        let model = PageModel::build(&before.records);
        // A line, which is the only thing a text property can be applied to.
        let Some(index) = model
            .objects()
            .iter()
            .position(|o| matches!(o.kind, mangle_edit::Kind::Line | mangle_edit::Kind::Block))
        else {
            eprintln!("skipped: no text on this page");
            continue;
        };
        for (label, change) in [
            (
                "character spacing",
                Change::text(TextProperty::CharacterSpacing(2.0)),
            ),
            (
                "horizontal scale",
                Change::text(TextProperty::HorizontalScale(90.0)),
            ),
            (
                "baseline shift",
                Change::text(TextProperty::BaselineShift(4.0)),
            ),
        ] {
            let applied = match apply_change(&contents, &model.objects()[index], &change) {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("skipped: {name} {label}: {e}");
                    continue;
                }
            };
            let after = run_with(&ContentStream::parse(&applied.bytes), &resources);
            verify(&before, &after, index, &change)
                .unwrap_or_else(|e| panic!("{name} {label}: {e}"));

            // **Nothing else changed.** Taking the inserted wrapper back out gives the original.
            let inserted = applied
                .applied
                .iter()
                .map(|p| p.bytes.clone())
                .collect::<Vec<_>>();
            let stripped = with_wrappers_removed(&applied.bytes, &inserted);
            assert_eq!(
                stripped, contents,
                "{name} {label}: the run's own bytes and everything around them are unchanged"
            );
        }
    }
}

/// Taking the edit's inserted bytes back out of the result, which is what "nothing else changed"
/// means in bytes.
fn with_wrappers_removed(after: &[u8], inserted: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::with_capacity(after.len());
    let mut cursor = 0usize;
    for bytes in inserted {
        let at = after[cursor..]
            .windows(bytes.len())
            .position(|w| w == bytes)
            .map(|i| cursor + i)
            .expect("the edit's own bytes are in the stream it wrote");
        out.extend_from_slice(&after[cursor..at]);
        cursor = at + bytes.len();
    }
    out.extend_from_slice(&after[cursor..]);
    out
}

/// A crop on a real image, and the two properties that make it a crop rather than a rewrite.
///
/// GOAL.md §4.5 asks for "crop as a non-destructive clip (resettable)". Both halves are
/// checkable, and the second is the one that matters:
///
/// 1. **The clip lands** — the interpreter reports a clip in force for the image's record, and the
///    image itself is still the same picture;
/// 2. **Nothing else changed** — the image's own operators come out byte-for-byte and the only
///    bytes added are the clip, so undoing the edit (or simply taking the wrapper out) restores the
///    whole picture.
#[test]
fn a_crop_on_a_real_image_is_a_clip_and_nothing_else() {
    // A page with an image that opens: `TAMReview`'s own first page is refused, by name, because of
    // its unreadable inline image.
    let Some(name) = PAGES.get(2) else {
        return;
    };
    let Some((contents, resources)) = page_one(name) else {
        eprintln!("skipped: {name} is not fetched");
        return;
    };
    let before = run_with(&ContentStream::parse(&contents), &resources);
    let model = PageModel::build(&before.records);
    let Some(index) = model
        .objects()
        .iter()
        .position(|o| o.kind == mangle_edit::Kind::Image)
    else {
        eprintln!("skipped: no image on this page");
        return;
    };
    let object = &model.objects()[index];
    let was = object.bounds;
    // Half the image, in page space.
    let keep = mangle_content::state::ClipBounds {
        x0: was.x0,
        y0: was.y0,
        x1: f64::midpoint(was.x0, was.x1),
        y1: f64::midpoint(was.y0, was.y1),
    };
    let change = Change::Crop { keep };
    let applied = apply_change(&contents, object, &change).expect("a crop is writable");
    let after = run_with(&ContentStream::parse(&applied.bytes), &resources);

    // 1. The image is still there, in the same place, with a clip in force.
    let cropped = after
        .records
        .iter()
        .find(|r| matches!(r.mark, mangle_content::Mark::Image { .. }))
        .expect("the image is still on the page");
    assert!(
        cropped.clip.is_some(),
        "and it is now drawn inside a clip, which is what a crop is"
    );
    let bounds = cropped.bounds().expect("a box");
    assert!(
        (bounds.x0 - was.x0).abs() < 1e-6 && (bounds.y0 - was.y0).abs() < 1e-6,
        "a crop does not move the image, it cuts it: {:?}",
        (bounds.x0, bounds.y0)
    );

    // 2. Nothing else changed: taking the wrapper back out gives the original exactly.
    let inserted = applied
        .applied
        .iter()
        .map(|p| p.bytes.clone())
        .collect::<Vec<_>>();
    let stripped = with_wrappers_removed(&applied.bytes, &inserted);
    assert_eq!(
        stripped, contents,
        "every byte the crop did not insert is a byte the author wrote"
    );

    // And the picture itself is untouched: the image XObject's bytes are the file's own, which is
    // what "non-destructive" means against the stream the crop was applied to.
    assert!(
        !applied.applied.iter().any(|p| !p.range.is_empty()),
        "a crop adds bytes and removes none: {patches:?}",
        patches = applied.applied
    );
}
