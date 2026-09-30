//! The content layer against real files, and the property ADR-0002 exists for.
//!
//! The unit tests in the crate drive one function each. These drive the whole chain —
//! open a file, read a page's streams, tokenise them, run them, and look at the marks —
//! and check the thing that makes the design worth anything: that every mark knows
//! which bytes of the file it came from, and that those bytes are exactly the ones
//! that would have to change to change the mark.

#![forbid(unsafe_code)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};

use mangle_content::{ContentStream, Mark, Resources, run, run_with};
use mangle_doc::PageTree;
use mangle_syntax::{Document, OpenOptions, stream::decode_stream};

fn corpus_dir() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)?;
    let dir = root.join("fixtures");
    dir.join("MANIFEST.toml").is_file().then_some(dir)
}

/// A two-page document: one page with shapes, one with text.
fn sample() -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut at = [0usize; 10];
    out.extend_from_slice(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n");

    at[1] = out.len();
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    at[2] = out.len();
    out.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 /MediaBox [0 0 612 792] \
          /Resources << /Font << /F1 4 0 R >> /ExtGState << /GS1 6 0 R /GS2 7 0 R >> >> >>\nendobj\n",
    );
    at[3] = out.len();
    out.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 8 0 R >>\nendobj\n");
    at[4] = out.len();
    out.extend_from_slice(
        b"4 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n",
    );
    at[5] = out.len();
    out.extend_from_slice(b"5 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 9 0 R >>\nendobj\n");
    at[6] = out.len();
    out.extend_from_slice(b"6 0 obj\n<< /Type /ExtGState /LW 3 /ca 0.5 >>\nendobj\n");
    at[7] = out.len();
    out.extend_from_slice(b"7 0 obj\n<< /Type /ExtGState /CA 0.25 /LJ 1 >>\nendobj\n");

    let shapes = b"q 2 0 0 2 0 0 cm /GS1 gs 0.5 0.5 0.5 rg 20 20 100 50 re f \
                  200 700 m 300 750 l 400 700 l h B Q";
    at[8] = out.len();
    let mut body = format!("8 0 obj\n<< /Length {} >>\nstream\n", shapes.len()).into_bytes();
    body.extend_from_slice(shapes);
    body.extend_from_slice(b"\nendstream\nendobj\n");
    out.extend_from_slice(&body);

    let text = b"BT /F1 24 Tf 72 700 Td (Beautiful) Tj 0 -30 Td (Places) Tj ET";
    at[9] = out.len();
    let mut body = format!("9 0 obj\n<< /Length {} >>\nstream\n", text.len()).into_bytes();
    body.extend_from_slice(text);
    body.extend_from_slice(b"\nendstream\nendobj\n");
    out.extend_from_slice(&body);

    let xref = out.len();
    out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n1 9\n");
    for offset in at.iter().take(10).skip(1) {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size 10 /Root 1 0 R /ID [<0102> <0304>] >>\nstartxref\n{xref}\n%%EOF\n"
        )
        .as_bytes(),
    );
    out
}

fn open(bytes: Vec<u8>) -> Document {
    Document::open(bytes, OpenOptions::default()).expect("the file should open")
}

/// A page's resource tables, following the inheritance a reader has to follow.
fn resources_of(doc: &Document, page: &mangle_doc::pages::Page) -> Resources {
    let Some(dict) = page
        .inherited
        .resources
        .as_ref()
        .and_then(|o| doc.resolve_object(o))
        .and_then(|o| o.as_dict().cloned())
    else {
        return Resources::default();
    };
    Resources::from_dict(&dict, &|o| doc.resolve_object(o))
}

/// The pages, in document order.
fn pages(doc: &Document) -> Vec<mangle_doc::pages::Page> {
    let catalog = doc.catalog().expect("a catalogue");
    let root = catalog
        .get("Pages")
        .and_then(mangle_syntax::object::Object::as_ref_id)
        .expect("the page tree");
    PageTree::build(doc, root)
        .expect("a page tree")
        .pages()
        .to_vec()
}

/// One page's decoded content, concatenated the way a reader must concatenate it: in
/// array order, with a newline between the parts.
fn page_content(doc: &Document, page: &mangle_doc::pages::Page) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, s) in page.content_streams(doc).iter().enumerate() {
        if i > 0 {
            out.push(b'\n');
        }
        out.extend_from_slice(&decode_stream(s).data);
    }
    out
}

#[test]
fn a_shapes_page_produces_the_marks_it_drew() {
    let doc = open(sample());
    let all = pages(&doc);
    let content = page_content(&doc, &all[0]);
    let resources = resources_of(&doc, &all[0]);
    let out = run_with(&ContentStream::parse(&content), &resources);

    assert!(
        out.unknown_operators.is_empty(),
        "{:?}",
        out.unknown_operators
    );
    assert!(out.notes.is_empty(), "{:?}", out.notes);

    // The page fills a rectangle and strokes a triangle, inside one `q`/`Q`.
    let painted: Vec<&Mark> = out
        .records
        .iter()
        .map(|r| &r.mark)
        .filter(|m| matches!(m, Mark::Path { .. }))
        .collect();
    assert_eq!(painted.len(), 2, "a fill and a fill-and-stroke");

    let Mark::Path { segments, .. } = painted[0] else {
        panic!("expected a path");
    };
    // The `2 0 0 2 0 0 cm` doubled everything: the rectangle at (20, 20) 100 by 50
    // becomes one at (40, 40) 200 by 100.
    assert!(
        matches!(segments.first(), Some(mangle_content::PathSegment::Move(x, y)) if (*x - 40.0).abs() < 1e-9 && (*y - 40.0).abs() < 1e-9),
        "got {:?}",
        segments.first()
    );
}

#[test]
fn an_extgstate_from_the_resources_is_applied() {
    let doc = open(sample());
    // The resources are on the page tree, not the catalogue, so reading them means
    // following the inheritance a reader has to follow anyway.
    let page = pages(&doc).into_iter().next().expect("a page");
    let r = resources_of(&doc, &page);
    let counts = r.counts();
    assert_eq!(counts.fonts, 1);
    assert_eq!(
        counts.ext_gstates, 2,
        "both dictionaries in the table were read"
    );

    let gs = r.ext_gstate(b"GS1").expect("/GS1");
    assert_eq!(gs.line_width, Some(3.0));
    assert_eq!(gs.fill_alpha, Some(0.5));
    let other = r.ext_gstate(b"GS2").expect("/GS2");
    assert_eq!(other.stroke_alpha, Some(0.25));
    assert!(
        r.ext_gstates.missing.is_empty(),
        "a name that resolved to a dictionary is not missing: {:?}",
        r.ext_gstates.missing
    );
}

#[test]
fn a_text_page_produces_two_glyph_runs() {
    let doc = open(sample());
    let all = pages(&doc);
    let content = page_content(&doc, &all[1]);
    let out = run(&ContentStream::parse(&content));

    let glyphs: Vec<&mangle_content::Record> = out
        .records
        .iter()
        .filter(|r| matches!(r.mark, Mark::Glyphs { .. }))
        .collect();
    assert_eq!(glyphs.len(), 2, "one mark per show operation");

    let Mark::Glyphs { text, font, .. } = &glyphs[0].mark else {
        panic!("expected glyphs");
    };
    assert_eq!(text, b"Beautiful");
    assert_eq!(font.as_deref(), Some("F1"));

    let Mark::Glyphs { text, .. } = &glyphs[1].mark else {
        panic!("expected glyphs");
    };
    assert_eq!(text, b"Places");
}

#[test]
fn every_mark_names_the_bytes_that_drew_it() {
    let doc = open(sample());
    for page in pages(&doc) {
        let content = page_content(&doc, &page);
        let out = run(&ContentStream::parse(&content));
        for record in &out.records {
            let bytes = content.get(record.span.clone()).unwrap_or_else(|| {
                panic!(
                    "a mark's span {:?} is outside the {}-byte page content",
                    record.span,
                    content.len()
                )
            });
            assert!(!bytes.is_empty(), "a mark must name at least one byte");
            // A path mark's bytes must end with the operator that drew it; that is the
            // check that would fail if the span covered only the operands.
            if let Mark::Path { .. } = &record.mark {
                let last = record.span.end.saturating_sub(1);
                let operator = ContentStream::parse(&content)
                    .token_at(last)
                    .and_then(|t| t.operator())
                    .map(<[u8]>::to_vec)
                    .unwrap_or_default();
                assert!(
                    matches!(
                        operator.as_slice(),
                        b"f" | b"F" | b"f*" | b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*"
                    ),
                    "a painted path's bytes must end with its operator, got {:?} in {bytes:?}",
                    String::from_utf8_lossy(&operator)
                );
            }
            if let Mark::Glyphs { text, .. } = &record.mark {
                // The shown string is in the bytes, which is what makes a text edit
                // surgical.
                let shown = String::from_utf8_lossy(text);
                assert!(
                    bytes.windows(shown.len()).any(|w| w == shown.as_bytes()),
                    "the bytes {text:?} must contain the string {shown:?}"
                );
            }
        }
    }
}

#[test]
fn a_text_spans_only_its_own_show_operation() {
    let doc = open(sample());
    let all = pages(&doc);
    let content = page_content(&doc, &all[1]);
    let out = run(&ContentStream::parse(&content));
    let second = out
        .records
        .iter()
        .find(|r| matches!(&r.mark, Mark::Glyphs { text, .. } if text == b"Places"))
        .expect("the second show");
    let bytes = content.get(second.span.clone()).expect("the span");
    let text = String::from_utf8_lossy(bytes);
    assert!(text.contains("(Places)"), "{text:?}");
    assert!(
        !text.contains("(Beautiful)"),
        "changing the second show must not need the first one's bytes: {text:?}"
    );
}

#[test]
fn replacing_a_mark_in_the_source_changes_only_that_mark() {
    // The point of the whole design: rewrite the bytes a mark names, and the other
    // marks still parse and still report their own bytes.
    let doc = open(sample());
    let all = pages(&doc);
    let content = page_content(&doc, &all[1]);
    let out = run(&ContentStream::parse(&content));
    let first = out
        .records
        .iter()
        .find(|r| matches!(&r.mark, Mark::Glyphs { text, .. } if text == b"Beautiful"))
        .expect("the first show");

    let string_span = match &first.mark {
        Mark::Glyphs { text_spans, .. } => text_spans
            .first()
            .cloned()
            .expect("a glyph run knows its own string's bytes"),
        other => panic!("expected a glyph run, got {other:?}"),
    };
    assert_eq!(
        content.get(string_span.clone()),
        Some(&b"(Beautiful)"[..]),
        "the string's own bytes, and not the `Tj` beside it"
    );
    let string_start = string_span.start;
    let mut edited = content.clone();
    edited.splice(string_span, b"(Splendid)".iter().copied());

    let after = run(&ContentStream::parse(&edited));
    let texts: Vec<Vec<u8>> = after
        .records
        .iter()
        .filter_map(|r| match &r.mark {
            Mark::Glyphs { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec![b"Splendid".to_vec(), b"Places".to_vec()]);

    // Every mark before the edit kept its exact bytes. Marks after it shift by one,
    // because the replacement is a byte shorter, and that is arithmetic rather than
    // damage: what matters for them is that their *content* is unchanged, which the list
    // above already says.
    for record in &out.records {
        if record.span.start >= string_start {
            continue;
        }
        let before = content.get(record.span.clone()).expect("before");
        let after_bytes = edited.get(record.span.clone()).expect("after");
        assert_eq!(
            before, after_bytes,
            "a mark before the edit had its bytes changed"
        );
    }
}

#[test]
fn the_fixtures_that_draw_something_produce_marks() {
    let Some(dir) = corpus_dir() else {
        eprintln!("skipped: run `cargo xtask fixtures` first");
        return;
    };
    let mut pages_seen = 0usize;
    for entry in std::fs::read_dir(&dir).expect("the corpus") {
        let path = entry.expect("an entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("pdf") {
            continue;
        }
        let bytes = std::fs::read(&path).expect("a fixture");
        let doc = Document::open(bytes, OpenOptions::default())
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        for page in pages(&doc) {
            let content = page_content(&doc, &page);
            if content.is_empty() {
                continue;
            }
            pages_seen += 1;
            let stream = ContentStream::parse(&content);
            let out = run(&stream);
            for record in &out.records {
                let bytes = content.get(record.span.clone()).unwrap_or_else(|| {
                    panic!(
                        "{}: a mark's span {:?} is outside {} bytes",
                        path.display(),
                        record.span,
                        content.len()
                    )
                });
                assert!(!bytes.is_empty(), "{}: an empty mark", path.display());
            }
        }
    }
    assert!(
        pages_seen >= 4,
        "only {pages_seen} pages had content to check"
    );
}
