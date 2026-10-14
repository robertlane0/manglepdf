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
/// Pages to run over, chosen for having several content streams each — the first page of the
/// corpus is refused an edit, so a session needs somewhere it can work.
const PAGES: &[&str] = &["gov__irs-f1040.pdf", "gov__nist-sp800-88.pdf"];

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

        let mut editor = match Editor::open(std::sync::Arc::new(doc), page, &resources) {
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

        let outcome = editor
            .apply(moved, &Change::move_by(9.0, 0.0))
            .expect("a move on a movable object");
        assert_eq!(outcome.label, "move by 9, 0");
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

/// An installed font on a real page rides the save and reopens with it.
///
/// The unit test proves the bookkeeping on a fixture; this proves the revision a real
/// file's readers accept — indirect resources, filtered streams, and all.
#[test]
fn an_installed_font_rides_a_real_save() {
    let Some((doc, tree, index)) = open_page("gov__irs-f1040.pdf") else {
        eprintln!("skipped: the corpus file is not fetched");
        return;
    };
    let page = tree.pages().get(index).expect("page one");
    let resources = resources_of(&doc, page);
    let mut editor = match Editor::open(std::sync::Arc::new(doc), page, &resources) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let program = std::fs::read("../../assets/fonts/LiberationSans-Regular.ttf")
        .expect("the bundled face is checked in");
    let name = editor.free_font_name("F");
    if let Err(e) = editor.install_truetype(&name, &program, &['H', 'i'], "LiberationSans") {
        eprintln!("skipped: {e}");
        return;
    }
    let save = editor.save().expect("and it saves");
    assert!(
        save.appended_only(editor.document().bytes()),
        "the saved file is the original with a revision appended"
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
    let font = resources2.fonts.get(&name).expect("the installed font");
    let dict = font.as_dict().expect("a font dictionary");
    assert_eq!(
        dict.get("Subtype").and_then(|o| o.as_name()),
        Some(&b"Type0"[..]),
        "a composite wrapper with a ToUnicode behind it"
    );
}

/// An arrange inside a session moves an object past its neighbour and can be undone like any
/// other step.
#[test]
fn an_arrange_in_a_session_can_be_undone() {
    let Some((doc, tree, index)) = open_page("gov__irs-f1040.pdf") else {
        eprintln!("skipped: the corpus file is not fetched");
        return;
    };
    let page = tree.pages().get(index).expect("page one");
    let resources = resources_of(&doc, page);
    let mut editor = match Editor::open(std::sync::Arc::new(doc), page, &resources) {
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

/// A missing glyph on a real page falls back to an embedded subset through a session.
///
/// The first single-run `Tj` object whose font lacks `α` is retyped with it: the label
/// names the fallback, the save carries the new font objects, and the reopened run
/// reads back through the installed `ToUnicode`.
#[test]
fn a_missing_glyph_falls_back_on_a_real_page() {
    // Several pages, because whether a run can fall back depends on its family: a run
    // set in HelveticaNeue — which is not metric-compatible with anything bundled —
    // keeps its refusal, and only a run with a bundled stand-in takes the fallback.
    let mut fell_back = false;
    for name in [
        "gov__irs-f1040.pdf",
        "gov__nist-sp800-88.pdf",
        "pdfjs__TAMReview.pdf",
    ] {
        let Some((doc, tree, index)) = open_page(name) else {
            eprintln!("skipped: the corpus file is not fetched");
            continue;
        };
        let page = tree.pages().get(index).expect("page one");
        let resources = resources_of(&doc, page);
        let mut editor = match Editor::open(std::sync::Arc::new(doc), page, &resources) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("skipped: {e}");
                continue;
            }
        };
        let mut no_content = 0usize;
        let mut no_alpha = 0usize;
        let mut own_font = 0usize;
        let mut refused = 0usize;
        for (at, object) in editor.model().objects().iter().enumerate() {
            if !matches!(
                object.kind,
                mangle_edit::Kind::Line | mangle_edit::Kind::Block | mangle_edit::Kind::LooseRun
            ) || object.records.len() != 1
            {
                continue;
            }
            let Some(content) = summaries_content(&editor, at) else {
                no_content += 1;
                continue;
            };
            let Some(alpha) = with_alpha(&content) else {
                no_alpha += 1;
                continue;
            };
            match editor.apply(
                at,
                &Change::Retype {
                    text: alpha.clone(),
                },
            ) {
                Ok(outcome) if outcome.label.contains("fallback") => {
                    assert!(
                        outcome.label.contains("Liberation"),
                        "the notice names the face: {}",
                        outcome.label
                    );
                    eprintln!("fell back on {name} object {at}: {}", outcome.label);
                    let save = editor.save().expect("and it saves");
                    assert!(
                        save.appended_only(editor.document().bytes()),
                        "the saved file is the original with a revision appended"
                    );
                    let reopened =
                        Document::open(save.bytes, OpenOptions::default()).expect("it reopens");
                    assert!(
                        reads_through_tounicode(&reopened, &alpha),
                        "the page says {alpha:?} through its ToUnicode"
                    );
                    fell_back = true;
                    break;
                }
                Ok(_) => {
                    own_font += 1;
                }
                Err(_) => {
                    refused += 1;
                }
            }
        }
        eprintln!(
            "{name}: no-content={no_content} no-alpha={no_alpha} own-font={own_font} refused={refused}"
        );
        if fell_back {
            break;
        }
    }
    assert!(fell_back, "no run fell back on any corpus page");
}

/// The object's decoded text, read the way the panel reads it.
fn summaries_content(editor: &Editor, at: usize) -> Option<String> {
    editor
        .model()
        .objects()
        .get(at)
        .and_then(|o| {
            o.records.first().and_then(|r| match &r.mark {
                mangle_content::interp::Mark::Glyphs {
                    text,
                    placements,
                    size,
                    ..
                } => {
                    let run = mangle_text::Run {
                        form: r.form.as_deref(),
                        font: r.text.font.as_deref(),
                        text,
                        origins: placements.iter().map(|m| (m.e, m.f)).collect(),
                        size: *size
                            * (r.text_matrix.a * r.text_matrix.d
                                - r.text_matrix.b * r.text_matrix.c)
                                .abs()
                                .sqrt(),
                    };
                    let doc = editor.document();
                    // The page's own resources, resolved the way the interpreter needs them.
                    let page = editor.page();
                    let resources = page
                        .inherited
                        .resources
                        .as_ref()
                        .and_then(|o| doc.resolve_object(o))
                        .and_then(|o| o.as_dict().cloned())
                        .map(|d| Resources::from_dict(&d, &|o| doc.resolve_object(o)))
                        .unwrap_or_default();
                    mangle_text::decode(&resources, &|o| doc.resolve_object(o), &run)
                        .map(|d| d.text)
                }
                _ => None,
            })
        })
        .filter(|t| !t.is_empty())
}

/// The text with its last alphanumeric character replaced by `α`, when it has one.
fn with_alpha(text: &str) -> Option<String> {
    let (at, found) = text
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_ascii_alphanumeric())?;
    let mut out = text.to_string();
    out.replace_range(at..at + found.len_utf8(), "α");
    Some(out)
}

/// Whether any text-showing run of the file's first page reads as `want` through its
/// own `ToUnicode`: the installed CMap is what makes the new text extractable.
fn reads_through_tounicode(doc: &Document, want: &str) -> bool {
    let Some(cat) = doc.catalog().ok() else {
        return false;
    };
    let Some(root) = cat.get("Pages").and_then(Object::as_ref_id) else {
        return false;
    };
    let Some(tree) = PageTree::build(doc, root).ok() else {
        return false;
    };
    let Some(page) = tree.pages().first() else {
        return false;
    };
    let resources = resources_of(doc, page);
    // Every composite font's map, collected first: the run does not say which font it
    // fell back to, and a map that names the run's CIDs is the one.
    let mut maps: Vec<(String, std::collections::BTreeMap<u32, char>)> = Vec::new();
    for (name, font) in &resources.fonts {
        let Some(dict) = doc.resolve_object(font).and_then(|o| o.as_dict().cloned()) else {
            continue;
        };
        if dict.get("Subtype").and_then(|o| o.as_name()) != Some(&b"Type0"[..]) {
            continue;
        }
        let Some(text) = dict
            .get("ToUnicode")
            .and_then(|o| doc.resolve_object(o))
            .and_then(|o| match o {
                Object::Stream(s) => String::from_utf8(s.raw).ok(),
                _ => None,
            })
        else {
            continue;
        };
        maps.push((name.clone(), to_unicode_map(&text)));
    }
    if maps.is_empty() {
        return false;
    }
    let stream = page.decoded_contents(doc);
    let run = run_with(&ContentStream::parse(&stream), &resources);
    for record in &run.records {
        let mangle_content::interp::Mark::Glyphs { text: raw, .. } = &record.mark else {
            continue;
        };
        if raw.len() % 2 != 0 || raw.is_empty() {
            continue;
        }
        for (_, map) in &maps {
            let decoded: String = raw
                .chunks_exact(2)
                .map(|pair| {
                    let cid = (u32::from(pair[0]) << 8) | u32::from(pair[1]);
                    map.get(&cid).copied().unwrap_or('�')
                })
                .collect();
            if decoded == want {
                return true;
            }
        }
    }
    false
}

/// One `beginbfchar` block read as CID to character.
fn to_unicode_map(cmap: &str) -> std::collections::BTreeMap<u32, char> {
    let mut out = std::collections::BTreeMap::new();
    let mut inside = false;
    for line in cmap.lines() {
        let line = line.trim();
        if line.ends_with("beginbfchar") {
            inside = true;
            continue;
        }
        if line.starts_with("endbfchar") {
            inside = false;
            continue;
        }
        if !inside {
            continue;
        }
        let mut parts = line.split_whitespace();
        if let (Some(cid), Some(uni)) = (parts.next(), parts.next()) {
            if let (Some(cid), Some(uni)) = (hex_of(cid), hex_of(uni)) {
                if let Some(ch) = char::from_u32(uni) {
                    out.insert(cid, ch);
                }
            }
        }
    }
    out
}

/// `<1F>` to its number, or `None` for anything that is not one hex word.
fn hex_of(word: &str) -> Option<u32> {
    let word = word.strip_prefix('<')?.strip_suffix('>')?;
    if word.is_empty() || word.len() > 8 || !word.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(word, 16).ok()
}
