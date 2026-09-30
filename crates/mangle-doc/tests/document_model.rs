//! Tests for the document model.
//!
//! The cases here are the ones the specification's happy path does not cover: an
//! inherited attribute, a `/Count` that lies, a `/Kids` entry that is not a page, a
//! page tree that loops, a name tree with a missing value, an outline that points at
//! nothing, and a page label with a non-string prefix.

// A test states its expectations with `expect` and fails loudly when they are not met.
// The panic-free rule is about what the product does with a file, not about tests.
#![forbid(unsafe_code)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use mangle_doc::labels::Style;
use mangle_doc::outlines::{decode_pdf_text, resolve_destination};
use mangle_doc::pages::{Inheritable, PageTree};
use mangle_doc::{Error, LabelRange, MapResolver, NameTree, PageLabel, is_page, normalise_rotate};
use mangle_syntax::{Dict, Object, Rect, Ref};

fn r(n: u32) -> Ref {
    Ref::new(n, 0)
}

fn o(n: u32) -> Object {
    Object::Ref(r(n))
}

fn leaf(resources: Option<Object>) -> Object {
    let mut d = Dict::new();
    d.set("Type", Object::name("Page"));
    if let Some(res) = resources {
        d.set("Resources", res);
    }
    Object::Dict(d)
}

// ---------------------------------------------------------------- page tree

#[test]
fn pages_are_flattened_in_document_order() {
    let mut root = Dict::new();
    root.set("Type", Object::name("Pages"));
    root.set("Kids", Object::Array(vec![o(1), o(2)]));
    // A `/Count` that lies must not change what we find.
    root.set("Count", Object::Int(99));

    let res = MapResolver::new()
        .with(0, Object::Dict(root))
        .with(1, leaf(None))
        .with(2, leaf(None));

    let tree = PageTree::build(&res, r(0)).expect("tree");
    assert_eq!(tree.len(), 2);
    assert_eq!(tree.get(0).map(|p| p.index), Some(0));
    assert_eq!(tree.get(1).map(|p| p.index), Some(1));
    assert_eq!(tree.claimed_count(&res, r(0)), Some(99));
}

#[test]
fn inheritable_attributes_come_from_ancestors() {
    let mut root = Dict::new();
    root.set("Type", Object::name("Pages"));
    root.set("Kids", Object::Array(vec![o(1)]));
    root.set(
        "MediaBox",
        Object::Array(rect_array(0.0, 0.0, 595.0, 842.0)),
    );
    root.set("Resources", Object::Dict(Dict::new()));
    root.set("Rotate", Object::Int(90));

    let res = MapResolver::new()
        .with(0, Object::Dict(root))
        .with(1, leaf(None));

    let tree = PageTree::build(&res, r(0)).expect("tree");
    let page = tree.get(0).expect("page");
    // The leaf sets none of these; all three come from the root.
    assert_eq!(
        page.inherited.media_rect(),
        Rect::new(0.0, 0.0, 595.0, 842.0)
    );
    assert_eq!(page.inherited.rotation(), 90);
    assert!(page.inherited.resources.is_some());
    // A rotated page is laid out with its sides swapped.
    assert_eq!(page.inherited.displayed_size(), (842.0, 595.0));
}

#[test]
fn a_leaf_overrides_its_ancestor() {
    let mut root = Dict::new();
    root.set("Type", Object::name("Pages"));
    root.set("Kids", Object::Array(vec![o(1)]));
    root.set(
        "MediaBox",
        Object::Array(rect_array(0.0, 0.0, 612.0, 792.0)),
    );

    let mut child = Dict::new();
    child.set("Type", Object::name("Page"));
    child.set(
        "MediaBox",
        Object::Array(rect_array(0.0, 0.0, 200.0, 100.0)),
    );

    let res = MapResolver::new()
        .with(0, Object::Dict(root))
        .with(1, Object::Dict(child));

    let tree = PageTree::build(&res, r(0)).expect("tree");
    assert_eq!(
        tree.get(0).map(|p| p.inherited.media_rect()),
        Some(Rect::new(0.0, 0.0, 200.0, 100.0))
    );
}

#[test]
fn a_page_without_a_type_key_is_still_a_page() {
    let mut bare = Dict::new();
    bare.set(
        "MediaBox",
        Object::Array(rect_array(0.0, 0.0, 100.0, 100.0)),
    );
    let mut root = Dict::new();
    root.set("Type", Object::name("Pages"));
    root.set("Kids", Object::Array(vec![o(1)]));

    let res = MapResolver::new()
        .with(0, Object::Dict(root))
        .with(1, Object::Dict(bare));

    let tree = PageTree::build(&res, r(0)).expect("tree");
    assert_eq!(tree.len(), 1, "a leaf is a page even with no /Type");
}

#[test]
fn a_kid_that_is_not_a_dictionary_is_skipped_not_fatal() {
    let mut root = Dict::new();
    root.set("Type", Object::name("Pages"));
    root.set("Kids", Object::Array(vec![o(1), Object::Int(7), o(2)]));
    let res = MapResolver::new()
        .with(0, Object::Dict(root))
        .with(1, leaf(None))
        .with(2, leaf(None));
    let tree = PageTree::build(&res, r(0)).expect("tree");
    assert_eq!(tree.len(), 2);
}

#[test]
fn a_page_tree_cycle_terminates() {
    // 0 -> 1 -> 0, with a page hanging off the cycle.
    let mut root = Dict::new();
    root.set("Type", Object::name("Pages"));
    root.set("Kids", Object::Array(vec![o(1)]));
    let mut mid = Dict::new();
    mid.set("Type", Object::name("Pages"));
    mid.set("Kids", Object::Array(vec![o(0), o(2)]));

    // `Self` is what makes cycle detection possible; a real file always has it.
    root.set("Self", o(0));
    mid.set("Self", o(1));

    let res = MapResolver::new()
        .with(0, Object::Dict(root))
        .with(1, Object::Dict(mid))
        .with(2, leaf(None));

    let tree = PageTree::build(&res, r(0)).expect("the walk must terminate");
    assert_eq!(tree.len(), 1);
}

#[test]
fn a_missing_page_tree_is_an_error_not_a_panic() {
    let res = MapResolver::new();
    assert!(matches!(
        PageTree::build(&res, r(0)),
        Err(Error::Dangling(_))
    ));
}

#[test]
fn a_crop_box_outside_the_media_box_is_clamped() {
    let inh = Inheritable {
        media_box: Some(Object::Array(rect_array(0.0, 0.0, 100.0, 100.0))),
        crop_box: Some(Object::Array(rect_array(-50.0, -50.0, 500.0, 500.0))),
        ..Inheritable::default()
    };
    assert_eq!(inh.crop_rect(), Rect::new(0.0, 0.0, 100.0, 100.0));
}

#[test]
fn a_page_with_no_media_box_falls_back_to_letter() {
    let inh = Inheritable::default();
    assert_eq!(inh.media_rect(), Rect::new(0.0, 0.0, 612.0, 792.0));
}

#[test]
#[allow(clippy::unwrap_used, clippy::expect_used)]
fn contents_may_be_one_stream_or_an_array() {
    let mut single = Dict::new();
    single.set("Type", Object::name("Page"));
    single.set("Contents", o(10));
    let mut array = Dict::new();
    array.set("Type", Object::name("Page"));
    array.set("Contents", Object::Array(vec![o(10), o(11)]));

    let mut root = Dict::new();
    root.set("Type", Object::name("Pages"));
    root.set("Kids", Object::Array(vec![o(1), o(2)]));

    let res = MapResolver::new()
        .with(0, Object::Dict(root))
        .with(1, Object::Dict(single))
        .with(2, Object::Dict(array))
        .with(10, stream(b"BT (a) Tj ET".to_vec()))
        .with(11, stream(b"0 0 m 1 1 l".to_vec()));

    let tree = PageTree::build(&res, r(0)).expect("tree");
    assert_eq!(tree.get(0).expect("one").contents(&res), b"BT (a) Tj ET");
    // Parts are joined with a newline, which is what the specification requires.
    assert_eq!(
        tree.get(1).expect("two").contents(&res),
        b"BT (a) Tj ET\n0 0 m 1 1 l"
    );
}

#[test]
fn is_page_discriminates_by_kids_when_type_is_missing() {
    let mut with_kids = Dict::new();
    with_kids.set("Kids", Object::Array(vec![]));
    let mut without = Dict::new();
    without.set("Contents", Object::Int(1));
    assert!(!is_page(&with_kids));
    assert!(is_page(&without));

    let mut wrong_type = Dict::new();
    wrong_type.set("Type", Object::name("Pages"));
    wrong_type.set("Contents", Object::Int(1));
    assert!(
        !is_page(&wrong_type),
        "an explicit /Type /Pages wins over /Kids"
    );
}

#[test]
fn rotate_is_rounded_and_normalised() {
    assert_eq!(normalise_rotate(0), 0);
    assert_eq!(normalise_rotate(90), 90);
    assert_eq!(normalise_rotate(-90), 270);
    assert_eq!(normalise_rotate(360), 0);
    assert_eq!(normalise_rotate(450), 90);
    assert_eq!(normalise_rotate(30), 0);
    assert_eq!(normalise_rotate(200), 180);
}

// --------------------------------------------------------------- name trees

#[test]
fn a_name_tree_is_flattened() {
    // 0 is the root, 1 its only kid; 2 and 3 are the destinations.
    let mut leaf = Dict::new();
    leaf.set(
        "Names",
        Object::Array(vec![
            Object::String(b"a".to_vec()),
            o(2),
            Object::String(b"b".to_vec()),
            o(3),
        ]),
    );
    let mut root = Dict::new();
    root.set("Kids", Object::Array(vec![o(1)]));
    root.set(
        "Names",
        Object::Array(vec![Object::String(b"z".to_vec()), o(2)]),
    );

    let res = MapResolver::new()
        .with(0, Object::Dict(root))
        .with(1, Object::Dict(leaf))
        .with(2, Object::Int(22))
        .with(3, Object::Int(33));

    let tree = NameTree::build(&res, r(0)).expect("names");
    assert_eq!(tree.len(), 3);
    assert_eq!(tree.get(b"a").and_then(Object::as_i64), Some(22));
    assert_eq!(tree.get(b"b").and_then(Object::as_i64), Some(33));
    assert_eq!(tree.get(b"z").and_then(Object::as_i64), Some(22));
    assert!(tree.get(b"missing").is_none());
}

#[test]
fn a_dangling_name_value_does_not_hide_its_neighbour() {
    let leaf = {
        let mut d = Dict::new();
        d.set(
            "Names",
            Object::Array(vec![
                Object::String(b"good".to_vec()),
                Object::Int(1),
                Object::String(b"bad".to_vec()),
                o(99),
            ]),
        );
        Object::Dict(d)
    };
    let res = MapResolver::new().with(0, leaf);
    let tree = NameTree::build(&res, r(0)).expect("names");
    assert_eq!(tree.len(), 1);
    assert!(tree.get(b"good").is_some());
}

#[test]
fn a_name_tree_cycle_terminates() {
    let mut d = Dict::new();
    d.set("Self", o(0));
    d.set("Kids", Object::Array(vec![o(0)]));
    let res = MapResolver::new().with(0, Object::Dict(d));
    assert!(NameTree::build(&res, r(0)).is_ok());
}

#[test]
fn the_legacy_dests_dictionary_is_read_too() {
    let legacy = {
        let mut d = Dict::new();
        d.set("Intro", Object::Int(7));
        Object::Dict(d)
    };
    let tree = NameTree::from_legacy(&MapResolver::new(), legacy.as_dict().expect("dict"));
    assert_eq!(tree.get(b"Intro").and_then(Object::as_i64), Some(7));
}

// --------------------------------------------------------- page labelling

#[test]
fn page_labels_use_the_style_of_the_range_covering_them() {
    let labels = labels_from(&[
        (0, Some(b"r"), b"", 1),
        (4, Some(b"R"), b"", 1),
        (8, Some(b"A"), b"App-", 1),
        (34, Some(b"a"), b"", 1),
        (60, None, b"front-", 1),
    ]);
    assert_eq!(labels.label_text(0), "i");
    assert_eq!(labels.label_text(3), "iv");
    assert_eq!(labels.label_text(4), "I");
    assert_eq!(labels.label_text(7), "IV");
    assert_eq!(labels.label_text(8), "App-A");
    assert_eq!(labels.label_text(33), "App-Z");
    // 34 is the first page of the letters range, so it is `a` and not `aa`.
    assert_eq!(labels.label_text(34), "a");
    assert_eq!(labels.label_text(35), "b");
    assert_eq!(labels.label_text(59), "z");
    assert_eq!(labels.label_text(60), "front-1");
    assert_eq!(labels.label_text(61), "front-2");
}

#[test]
fn spreadsheet_letters_wrap_past_z() {
    let labels = labels_from(&[(0, Some(b"a"), b"", 1)]);
    assert_eq!(labels.label_text(25), "z");
    assert_eq!(labels.label_text(26), "aa");
    assert_eq!(labels.label_text(27), "ab");
    assert_eq!(labels.label_text(51), "az");
    assert_eq!(labels.label_text(52), "ba");
}

#[test]
fn a_page_with_no_label_scheme_is_numbered_from_one() {
    let labels = PageLabel::none();
    assert_eq!(labels.label_text(0), "1");
    assert_eq!(labels.label_text(9), "10");
}

#[test]
fn a_range_starting_at_a_page_skips_the_rest() {
    let labels = labels_from(&[(2, Some(b"D"), b"", 5)]);
    // Pages 0 and 1 are not covered, so they fall back to decimal from 1.
    assert_eq!(labels.label_text(0), "1");
    assert_eq!(labels.label_text(1), "2");
    assert_eq!(labels.label_text(2), "5");
    assert_eq!(labels.label_text(3), "6");
}

#[test]
fn a_range_with_no_style_hides_its_pages() {
    let labels = labels_from(&[(0, Some(b"N"), b"", 1)]);
    assert_eq!(labels.label(3), b"");
    assert_eq!(labels.ranges()[0].style, Style::None);
}

#[test]
fn a_non_string_prefix_still_numbers_the_page() {
    // `/P 42` is not a string. The specification says the prefix is then the label and
    // the number is not shown, but the page still must not be blank by accident.
    let catalog = {
        let mut d = Dict::new();
        d.set(
            "PageLabels",
            Object::Array(vec![{
                let mut r = Dict::new();
                r.set("St", Object::Int(0));
                r.set("P", Object::Int(42));
                Object::Dict(r)
            }]),
        );
        Object::Dict(d)
    };
    let labels = PageLabel::build(&MapResolver::new(), catalog.as_dict().expect("dict"));
    assert!(!labels.label(0).is_empty());
}

// -------------------------------------------------------------- outlines

#[test]
fn an_outline_is_read_as_a_tree_not_a_list() {
    // 10 is the outline root, whose `/First` is "one". "one" has "three" as a child
    // and "two" as its next sibling, which is the shape a real bookmark tree has.
    let make = |title: &str, next: Option<Ref>, first: Option<Ref>| {
        let mut d = Dict::new();
        d.set("Title", Object::String(title.as_bytes().to_vec()));
        if let Some(n) = next {
            d.set("Next", Object::Ref(n));
        }
        if let Some(f) = first {
            d.set("First", Object::Ref(f));
        }
        Object::Dict(d)
    };
    let res = MapResolver::new()
        .with(10, make("", None, Some(r(11))))
        .with(11, make("one", Some(r(12)), Some(r(13))))
        .with(12, make("two", None, None))
        .with(13, make("three", None, None));

    let outline = mangle_doc::Outline::build(&res, r(10)).expect("outline");
    // "two" follows "one" through /Next, so it is a sibling, not a child.
    assert_eq!(outline.items.len(), 2);
    assert_eq!(outline.items[0].title_text(), "one");
    assert_eq!(outline.items[1].title_text(), "two");
    assert_eq!(outline.items[0].children.len(), 1);
    assert_eq!(outline.items[0].children[0].title_text(), "three");
    assert_eq!(outline.items[1].children.len(), 0);
    assert_eq!(outline.flatten().len(), 3);
}

#[test]
fn an_outline_cycle_terminates() {
    let mut d = Dict::new();
    d.set("Self", o(0));
    d.set("First", o(0));
    d.set("Next", o(0));
    let res = MapResolver::new().with(0, Object::Dict(d));
    let outline = mangle_doc::Outline::build(&res, r(0)).expect("outline");
    assert!(outline.items.len() <= 1);
}

#[test]
fn a_destination_resolves_to_a_page_and_a_frame() {
    let dest = Object::Array(vec![
        o(5),
        Object::name("XYZ"),
        Object::Null,
        Object::Real(400.0),
        Object::Null,
    ]);
    let d = resolve_destination(&dest, &|r: Ref| {
        if r.num == 5 { Some(4) } else { None }
    })
    .expect("destination");
    assert_eq!(d.page, 4);
    assert_eq!(d.xyz, Some((None, Some(400.0), None)));
}

#[test]
fn a_destination_naming_a_missing_page_lands_on_the_first() {
    let dest = Object::Array(vec![o(999), Object::name("Fit")]);
    let d = resolve_destination(&dest, &|_| None).expect("destination");
    assert_eq!(d.page, 0);
    assert_eq!(d.kind, mangle_doc::outlines::Fit::Fit);
}

#[test]
fn text_strings_decode_from_both_byte_orders() {
    assert_eq!(decode_pdf_text(b"plain"), "plain");
    let utf16: Vec<u8> = vec![0xFE, 0xFF, 0x00, b'H', 0x00, b'i'];
    assert_eq!(decode_pdf_text(&utf16), "Hi");
    let le: Vec<u8> = vec![0xFF, 0xFE, b'H', 0x00, b'i', 0x00];
    assert_eq!(decode_pdf_text(&le), "Hi");
}

// ------------------------------------------------------------------ helpers

fn rect_array(a: f64, b: f64, c: f64, d: f64) -> Vec<Object> {
    vec![
        Object::Real(a),
        Object::Real(b),
        Object::Real(c),
        Object::Real(d),
    ]
}

fn stream(data: Vec<u8>) -> Object {
    let mut d = Dict::new();
    d.set(
        "Length",
        Object::Int(i64::try_from(data.len()).unwrap_or(0)),
    );
    Object::Stream(mangle_syntax::Stream {
        dict: d,
        raw: data,
        file_offset: None,
        synthetic: true,
    })
}

/// A labelling range as a test states it: start page, `/S`, `/P`, and the first number.
type Spec = (i64, Option<&'static [u8]>, &'static [u8], i64);

/// Build a labelling scheme straight from ranges, for the cases that are about
/// numbering rather than about parsing.
fn labels_from(rows: &[Spec]) -> PageLabel {
    PageLabel::from_ranges(
        rows.iter()
            .map(|(st, style, prefix, first)| LabelRange {
                start: *st as usize,
                // An absent `/S` is decimal; only an explicit `/N` hides a page.
                style: style.map_or(Style::Decimal, Style::from_name),
                prefix: prefix.to_vec(),
                first: *first as u32,
            })
            .collect(),
    )
}
