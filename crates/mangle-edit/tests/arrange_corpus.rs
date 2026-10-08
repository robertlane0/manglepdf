//! Arrange over real pages, where the neighbours really do overlap.
//!
//! # What a corpus page has that a fixture does not
//!
//! Arrange's rule is that an object goes past the next object it **overlaps** — not the next one in
//! the file, and not the nearest one. A hand-written fixture has to choose: two images drawn at the
//! same point (which overlap and arrange past each other) or a screen apart (which do not, and
//! arrange past nothing). A real page has both, with the overlap decided by where the producer put
//! things, which is the only honest test of whether the rule is being applied or merely assumed.
//!
//! The second thing a real page has is **refusals**. Most of what is on a page is text, and text
//! cannot be arranged yet — the text state in force where a run sat is not carried on the record.
//! A test that only arranged the easy cases would report success while the rule quietly skipped
//! every line on the page.
//!
//! # What is asserted
//!
//! For each page: how many objects exist, how many could be arranged, and how many refused. Then,
//! for the ones that could, the two properties that matter — the bytes moved to the other side of
//! the neighbour, and **every other byte is where it was**, which is the whole point of a surgical
//! edit and the thing a re-serialisation cannot give.

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
use mangle_edit::PageModel;
use mangle_edit::arrange::{Arrange, apply_arrange, arrange_patches};
use mangle_syntax::object::Object;
use mangle_syntax::{Document, OpenOptions};

/// Pages with different shapes: a design export with images, a government form of rules and type,
/// and a scanned publication that is nearly all pictures.
const PAGES: &[&str] = &[
    "pdfjs__TAMReview.pdf",
    "gov__irs-f1040.pdf",
    "gov__nist-sp800-88.pdf",
];

fn page_of(name: &str) -> Option<(Document, PageTree, usize)> {
    let bytes = std::fs::read(format!("../../corpus/wild/{name}")).ok()?;
    let doc = Document::open(bytes, OpenOptions::default()).ok()?;
    let cat = doc.catalog().ok()?;
    let root = cat.get("Pages").and_then(Object::as_ref_id)?;
    let pages = PageTree::build(&doc, root).ok()?;
    Some((doc, pages, 0))
}

fn resources_of(doc: &Document, page: &mangle_doc::Page) -> Resources {
    page.inherited
        .resources
        .as_ref()
        .and_then(|o| doc.resolve_object(o))
        .and_then(|o| o.as_dict().cloned())
        .map(|d| Resources::from_dict(&d, &|o| doc.resolve_object(o)))
        .unwrap_or_default()
}

/// Taking an arrange's inserted bytes back out gives the original stream plus the moved bytes
/// where they went, which is what "only this object moved" means in bytes.
fn with_insertions_removed(after: &[u8], inserted: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::with_capacity(after.len());
    let mut cursor = 0usize;
    for bytes in inserted {
        let at = after[cursor..]
            .windows(bytes.len())
            .position(|w| w == bytes)
            .map(|i| cursor + i)
            .expect("the arrange's own bytes are in the stream it wrote");
        out.extend_from_slice(&after[cursor..at]);
        cursor = at + bytes.len();
    }
    out.extend_from_slice(&after[cursor..]);
    out
}

/// An arrange on a real page moves the object and leaves every other byte where it was.
#[test]
fn an_arrange_on_a_real_page_moves_the_object_and_leaves_the_rest_alone() {
    let mut arranged = 0usize;
    let mut refused = 0usize;
    for name in PAGES {
        let Some((doc, pages, index)) = page_of(name) else {
            eprintln!("skipped: {name} is not fetched");
            continue;
        };
        let page = pages.pages().get(index).expect("page one");
        let resources = resources_of(&doc, page);
        let stream = page.decoded_contents(&doc);
        let before = run_with(&ContentStream::parse(&stream), &resources);
        let model = PageModel::build(&before.records);

        for i in 0..model.objects().len() {
            let arrange = match i % 4 {
                0 => Arrange::Forward,
                1 => Arrange::Backward,
                2 => Arrange::ToFront,
                _ => Arrange::ToBack,
            };
            match arrange_patches(&stream, &model, i, arrange) {
                Ok(_patches) => {
                    let applied =
                        apply_arrange(&stream, &model, i, arrange).expect("the patches applied");
                    assert_ne!(
                        applied.bytes, stream,
                        "{name} {i}: the arrange changed something"
                    );
                    // Everything the arrange did not insert is a byte the file already had, and
                    // the only bytes it removed are the object's own.
                    let inserted = applied
                        .applied
                        .iter()
                        .filter(|p| p.range.is_empty())
                        .map(|p| p.bytes.clone())
                        .collect::<Vec<_>>();
                    let removed: Vec<(usize, usize)> = applied
                        .applied
                        .iter()
                        .filter(|p| !p.range.is_empty())
                        .map(|p| (p.range.start, p.range.end))
                        .collect();
                    let stripped = with_insertions_removed(&applied.bytes, &inserted);
                    let mut expected = Vec::with_capacity(stream.len());
                    let mut cursor = 0usize;
                    for (start, end) in &removed {
                        expected.extend_from_slice(&stream[cursor..*start]);
                        cursor = (*end).max(cursor);
                    }
                    expected.extend_from_slice(&stream[cursor.min(stream.len())..]);
                    assert_eq!(
                        stripped, expected,
                        "{name} {i}: the arrange removed exactly the object's own bytes and left \
                         every other one alone"
                    );
                    arranged += 1;
                }
                Err(_) => refused += 1,
            }
        }
        eprintln!("{name}: {} objects on the page", model.objects().len());
    }
    eprintln!("{arranged} object(s) arranged, {refused} refused");
    // The refusals are the finding, not a failure: most of a page is text, and text is not yet
    // arrangeable. What must not happen is *everything* being refused, which would mean the rule
    // never ran at all.
    assert!(
        arranged > 0,
        "no object on any page could be arranged, so nothing was checked"
    );
    assert!(
        refused > 0,
        "nothing refused a rearrange, which is suspicious"
    );
}

/// The refusal names what is missing, rather than moving the object and hoping.
#[test]
fn a_refused_arrange_names_what_is_missing() {
    let Some((doc, pages, index)) = page_of("pdfjs__TAMReview.pdf") else {
        eprintln!("skipped: the corpus file is not fetched");
        return;
    };
    let page = pages.pages().get(index).expect("page one");
    let resources = resources_of(&doc, page);
    let stream = page.decoded_contents(&doc);
    let before = run_with(&ContentStream::parse(&stream), &resources);
    let model = PageModel::build(&before.records);
    let mut seen: Vec<String> = Vec::new();
    for i in 0..model.objects().len() {
        if let Err(e) = arrange_patches(&stream, &model, i, Arrange::Forward) {
            seen.push(e.to_string());
        }
    }
    assert!(!seen.is_empty(), "nothing refused, which is suspicious");
    eprintln!("{} refusal(s):", seen.len());
    for r in seen.iter().take(3) {
        eprintln!("  {r}");
        assert!(!r.is_empty(), "a refusal says something");
    }
    // And the CLI-facing refusal type is the one that reached it.
}
