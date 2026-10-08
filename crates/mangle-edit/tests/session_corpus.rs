//! An editing session over a real file: edit, undo, redo, save, reopen.
//!
//! # Why this needs a corpus file
//!
//! The session's own unit tests use a hand-written two-object PDF, which is the right place to
//! prove the *bookkeeping* — undo restores the bytes, redo returns them, a new edit discards the
//! redo stack. It is the wrong place to find out whether a session survives a real page, because
//! everything that makes a real page hard is absent from a fixture:
//!
//! * `/Contents` is an **array of streams**, so an edit's bytes have to be split back into the
//!   right object rather than the first one;
//! * the streams are **filtered**, so the ones that changed have to be re-encoded and the ones
//!   that did not must not be;
//! * the objects are **mostly text**, which refuses an edit for a reason the caller has to read;
//! * an image's bytes are **not in a content stream**, so a save must leave them entirely alone.
//!
//! This file runs the whole loop on corpus pages and asserts the three things a user would lose
//! and never notice: the undo is exact, the redo returns the same bytes, and the save writes the
//! current state without touching anything else.

#![forbid(unsafe_code)]
// The panic-free rule is about what the product does with a file, not about tests. Indexing comes
// from spans the interpreter produced for this same stream, which is bounds-checked by construction.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_cmp
)]

use mangle_content::interp::run_with;
use mangle_content::{ContentStream, Resources};
use mangle_doc::PageTree;
use mangle_edit::session::Editor;
use mangle_edit::{Arrange, Change, PageModel};
use mangle_syntax::object::Object;
use mangle_syntax::{Document, OpenOptions};

/// A design export with images, and a government form of rules and type.
const PAGES: &[&str] = &["pdfjs__TAMReview.pdf", "gov__irs-f1040.pdf"];

/// A corpus file's document and page tree, or `None` when the submodule is not fetched.
fn open_page(name: &str) -> Option<(Document, PageTree, usize)> {
    let bytes = std::fs::read(format!("../../corpus/wild/{name}")).ok()?;
    let doc = Document::open(bytes, OpenOptions::default()).ok()?;
    let cat = doc.catalog().ok()?;
    let root = cat.get("Pages").and_then(Object::as_ref_id)?;
    let pages = PageTree::build(&doc, root).ok()?;
    Some((doc, pages, 0))
}

/// A page's resources, resolved the way the interpreter needs them.
fn resources_of(doc: &Document, page: &mangle_doc::Page) -> Resources {
    page.inherited
        .resources
        .as_ref()
        .and_then(|o| doc.resolve_object(o))
        .and_then(|o| o.as_dict().cloned())
        .map(|d| Resources::from_dict(&d, &|o| doc.resolve_object(o)))
        .unwrap_or_default()
}

/// The loop: edit, undo, redo, save, reopen — on a real page with real streams and real filters.
#[test]
fn a_session_round_trips_a_real_page() {
    let mut sessions = 0usize;
    for name in PAGES {
        let Some((doc, tree, index)) = open_page(name) else {
            eprintln!("skipped: {name} is not fetched");
            continue;
        };
        let page = tree.pages().get(index).expect("page one");
        let resources = resources_of(&doc, page);

        let mut editor = match Editor::open(doc, page, &resources) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("skipped: {name} cannot be edited in place: {e}");
                continue;
            }
        };
        let parts = editor.base().len();
        let was = editor.stream();
        eprintln!("{name}: {parts} content stream(s), {} bytes", was.len());

        // The first object that can actually be moved: a page is mostly text, and text refuses an
        // edit for the reason the session is expected to carry.
        let model = editor.model();
        let moved = model
            .objects()
            .iter()
            .position(|o| o.kind == mangle_edit::Kind::Image || o.kind == mangle_edit::Kind::Path);
        let Some(moved) = moved else {
            eprintln!("skipped: nothing movable on this page");
            continue;
        };

        let label = editor
            .apply(moved, &Change::move_by(9.0, 0.0))
            .expect("a move on a movable object");
        assert_eq!(label, "move by 9, 0");
        let edited = editor.stream();
        assert_ne!(edited, was, "the edit changed the stream");
        assert_eq!(editor.depth(), (1, 0), "one step back");

        // **Undo is exact**, which is FINISH.md U3's first half.
        editor.undo().expect("a step to undo");
        assert_eq!(
            editor.stream(),
            was,
            "{name}: undo gives back the same bytes, not a rendering that looks the same"
        );
        assert_eq!(
            editor.base().len(),
            parts,
            "and the page still has its own streams"
        );

        // **Redo returns the edited state**, which is the other half.
        editor.redo().expect("a step to redo");
        assert_eq!(
            editor.stream(),
            edited,
            "{name}: redo returns the edited state"
        );
        assert!(editor.is_dirty());

        // A second step, then an undo past it, which is what a history is for.
        let recolour = editor.apply(moved, &Change::move_by(-4.0, 0.0));
        if recolour.is_ok() {
            assert_eq!(editor.depth(), (2, 0), "two steps");
            editor.undo().expect("back one");
            assert_eq!(editor.stream(), edited, "back to the first edit exactly");
            editor.redo().expect("forward again");
        }

        // **Save the current state and reopen it.**
        let save = editor.save().expect("and it saves");
        assert!(
            save.appended_only(editor.document().bytes()),
            "{name}: the saved file is the original with a revision appended"
        );
        let reopened = Document::open(save.bytes, OpenOptions::default()).expect("it reopens");
        let cat = reopened.catalog().expect("a catalogue");
        let root = cat
            .get("Pages")
            .and_then(Object::as_ref_id)
            .expect("a tree");
        let tree2 = PageTree::build(&reopened, root).expect("the walk");
        let page2 = tree2.pages().first().expect("a page");
        let resources2 = resources_of(&reopened, page2);
        let stream2 = page2.decoded_contents(&reopened);
        assert_eq!(
            stream2,
            editor.stream(),
            "{name}: the reopened page holds exactly what the session had"
        );
        // And the interpreter agrees it is a page, which is the check that a re-encode went wrong.
        let run = run_with(&ContentStream::parse(&stream2), &resources2);
        assert!(
            !PageModel::build(&run.records).objects().is_empty(),
            "{name}: the page still has objects"
        );
        sessions += 1;
    }
    assert!(
        sessions > 0,
        "no corpus page could be edited, so nothing was checked"
    );
    eprintln!("{sessions} session round trip(s)");
}

/// An arrange inside a session moves an object past its neighbour and can be undone like any
/// other step.
#[test]
fn an_arrange_in_a_session_can_be_undone() {
    let Some((doc, tree, index)) = open_page("pdfjs__TAMReview.pdf") else {
        eprintln!("skipped: the corpus file is not fetched");
        return;
    };
    let page = tree.pages().get(index).expect("page one");
    let resources = resources_of(&doc, page);
    let mut editor = match Editor::open(doc, page, &resources) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let was = editor.stream();
    let model = editor.model();
    let movable = model
        .objects()
        .iter()
        .position(|o| o.kind == mangle_edit::Kind::Image || o.kind == mangle_edit::Kind::Path);
    let Some(movable) = movable else {
        eprintln!("skipped: nothing movable");
        return;
    };
    match editor.arrange(movable, Arrange::Forward) {
        Ok(label) => {
            assert!(label.contains("forward"), "{label}");
            let arranged = editor.stream();
            assert_ne!(arranged, was, "the arrange moved bytes");
            editor.undo().expect("a step to undo");
            assert_eq!(editor.stream(), was, "and an undo puts them back exactly");
        }
        Err(e) => eprintln!("skipped: this object cannot be arranged: {e}"),
    }
}
