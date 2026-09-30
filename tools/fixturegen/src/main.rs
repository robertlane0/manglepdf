#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    clippy::indexing_slicing
)]
#![warn(missing_debug_implementations)]

//! Deterministic PDF fixture generation.
//!
//! Tier A of the corpus comes from here. The generator depends on no `mangle-*` crate
//! on purpose: a fixture built by the same misunderstandings as the parser would prove
//! nothing about either. `cargo xtask policy` fails the build if that ever changes.
//!
//! Every fixture is written twice from the same description on purpose: once as a
//! valid file and once damaged in a specific, named way. The damaged variant is as
//! valuable as the clean one, because a reader that only ever sees valid files learns
//! nothing about what to do with the rest.
//!
//! Nothing here uses a clock or a random source. The seed selects which optional
//! features a fixture carries; the same seed always produces the same bytes.

mod writer;

use std::fmt::Write as _;

use std::path::{Path, PathBuf};

use writer::{Builder, Obj, sha256_hex};

/// One fixture and what it is for.
struct Fixture {
    id: &'static str,
    name: &'static str,
    /// What a reader must be able to do with it.
    purpose: &'static str,
    /// The machine-readable feature list `MANIFEST.toml` records.
    features: &'static [&'static str],
    pages: usize,
    /// Text a reader must extract, page by page.
    text: &'static [&'static str],
    /// What `qpdf --check` is expected to report. A fixture that is *meant* to be
    /// damaged must still say so the same way every time, which is what makes
    /// "intentional breakage" checkable rather than merely claimed.
    expect: Expectation,
    /// Whether our own reader has to rebuild the structure to open this file.
    ///
    /// This is a different question from the oracle's verdict, and both are recorded.
    /// A file can need no structural repair and still be wrong in its content: junk
    /// around the header and a wrong `/Length` are both tolerated by the specification,
    /// so a reader that handles them is *correct*, not lucky.
    reader_repairs: bool,
    build: fn(u64) -> Vec<u8>,
}

/// How an independent validator is expected to judge a fixture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expectation {
    /// No errors and no warnings.
    Valid,
    /// Readable, but a validator had to make a decision. Legal for a damaged file.
    Repaired,
    /// Rejected outright. Only a fixture whose subject is the rejection.
    Rejected,
}

impl Expectation {
    fn as_str(self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::Repaired => "repaired",
            Self::Rejected => "rejected",
        }
    }
}

/// A fixed document ID, so a run is reproducible and two runs can be compared.
const ID0: &str = "<00112233445566778899AABBCCDDEEFF>";
const ID1: &str = "<FFEEDDCCBBAA99887766554433221100>";

/// Hex-encode a payload for `ASCIIHexDecode`, with the `>` end-of-data marker.
fn hex(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() * 2 + 1);
    for b in data {
        out.extend_from_slice(format!("{b:02x}").as_bytes());
    }
    out.push(b'>');
    out
}

fn trailer_ids() -> String {
    format!("/ID [{ID0} {ID1}]")
}

/// The two pages every structural fixture shares, so they differ only in the
/// structure under test.
fn basic_pages(b: &mut Builder, extra_page_entries: &[(&str, &str)]) {
    b.push(Obj::dict(
        3,
        &[
            ("Type", "/Pages"),
            ("Kids", "[5 0 R 7 0 R]"),
            ("Count", "2"),
        ],
    ));
    let mut page = vec![
        ("Type", "/Page"),
        ("Parent", "3 0 R"),
        ("Contents", "6 0 R"),
    ];
    page.extend_from_slice(extra_page_entries);
    b.push(Obj::dict(5, &page));
    // ASCIIHexDecode carries hex, so the payload has to be encoded, not pasted.
    b.push(Obj::stream(
        6,
        &[("Filter", "/ASCIIHexDecode")],
        &hex(b"BT /F1 24 Tf 72 700 Td (Page one) Tj ET"),
    ));
    b.push(Obj::dict(
        7,
        &[
            ("Type", "/Page"),
            ("Parent", "3 0 R"),
            ("Contents", "8 0 R"),
        ],
    ));
    b.push(Obj::stream(
        8,
        &[],
        b"BT /F1 24 Tf 72 700 Td (Page two) Tj ET",
    ));
    b.push(Obj::dict(
        4,
        &[
            ("Type", "/Font"),
            ("Subtype", "/Type1"),
            ("BaseFont", "/Helvetica"),
        ],
    ));
}

fn standard_catalog(b: &mut Builder) {
    b.push(Obj::dict(1, &[("Type", "/Catalog"), ("Pages", "2 0 R")]));
    b.push(Obj::dict(
        2,
        &[
            ("Type", "/Pages"),
            ("Kids", "[3 0 R]"),
            ("Count", "1"),
            ("MediaBox", "[0 0 612 792]"),
            ("Resources", "<< /Font << /F1 4 0 R >> >>"),
        ],
    ));
}

// ----------------------------------------------------------------- F01

fn f01_struct_classic(_seed: u64) -> Vec<u8> {
    let mut b = Builder::new("1.4");
    standard_catalog(&mut b);
    basic_pages(&mut b, &[]);
    b.build(1, &trailer_ids())
}

// ----------------------------------------------------------------- F03

/// A hybrid file: a classic table plus an `/XRefStm` covering what the table omits.
///
/// The stream has to be written before the trailer that points at it, which means the
/// file is assembled in one pass rather than patched afterwards.
fn f03_struct_hybrid(_seed: u64) -> Vec<u8> {
    let mut b = Builder::new("1.5");
    standard_catalog(&mut b);
    basic_pages(&mut b, &[("Rotate", "90")]);
    b.build_hybrid(1, &trailer_ids())
}

// ----------------------------------------------------------------- F04

/// Five revisions, with objects freed and replaced along the way.
fn f04_struct_incremental_x5(_seed: u64) -> Vec<u8> {
    let mut b = Builder::new("1.5");
    standard_catalog(&mut b);
    basic_pages(&mut b, &[]);
    // A self-check on the builder itself: the cross-reference it is about to write has
    // to describe the same layout it believes in, or the fixture is wrong in a way no
    // reader could detect.
    let laid_out = b.offsets();
    debug_assert!(
        laid_out
            .windows(2)
            .all(|w| w.first().zip(w.get(1)).is_some_and(|(a, b)| a.0 + 1 == b.0)),
        "the base revision's objects must be numbered without a gap"
    );
    debug_assert!(
        laid_out.iter().all(|(_, off)| *off < b.total_len()),
        "every object must land inside the file"
    );
    let base = b.build(1, &trailer_ids());
    let start = find_startxref(&base);
    debug_assert!(start > 0, "the base revision must have a startxref");

    let mut out = base;
    let mut prev = start;
    for rev in 1..=4u32 {
        let page_content = format!("BT /F1 24 Tf 72 700 Td (Revision {rev}) Tj ET");
        let mut r = out.clone();
        if !r.ends_with(b"\n") {
            r.push(b'\n');
        }
        let at = r.len();
        let body = format!(
            "6 0 obj\n<< /Length {} >>\nstream\n{page_content}\nendstream\nendobj\n",
            page_content.len()
        );
        r.extend_from_slice(body.as_bytes());
        // Revision 3 adds an object and revision 4 frees it. Freeing something still
        // referenced would make the file wrong rather than merely damaged, which is a
        // different fixture and a different question.
        let mut subsection = String::new();
        if rev == 3 {
            let at_extra = r.len();
            r.extend_from_slice(format!("9 0 obj\n<< /Scratch ({rev}) >>\nendobj\n").as_bytes());
            subsection = format!("9 1\n{at_extra:010} 00000 n \n");
        } else if rev == 4 {
            subsection = "9 1\n0000000000 00000 f \n".to_string();
        }
        let xref = r.len();
        // The object this revision replaces is number 6, so the subsection header has
        // to say 6: a header naming a different number makes the file *wrong* rather
        // than merely damaged, which is a different fixture.
        r.extend_from_slice(
            format!("xref\n0 1\n0000000000 65535 f \n6 1\n{at:010} 00000 n \n{subsection}")
                .as_bytes(),
        );
        r.extend_from_slice(
            format!(
                "trailer\n<< /Size 10 /Root 1 0 R {ids} /Prev {prev} >>\nstartxref\n{xref}\n%%EOF\n",
                ids = trailer_ids()
            )
            .as_bytes(),
        );
        prev = xref;
        out = r;
    }
    out
}

/// A linearised file. The defining feature is a first cross-reference near the front
/// that a reader is meant to use without walking the whole file, so a writer that
/// only ever looks at the end still has to work.
fn f05_struct_linearized(_seed: u64) -> Vec<u8> {
    let mut b = Builder::new("1.4").linearised();
    standard_catalog(&mut b);
    basic_pages(&mut b, &[]);
    b.build(1, &trailer_ids())
}

// ----------------------------------------------------------------- F06

fn f06_broken_xref(_seed: u64) -> Vec<u8> {
    let mut b = Builder::new("1.4");
    standard_catalog(&mut b);
    basic_pages(&mut b, &[]);
    // Every offset is wrong by a few bytes, which is what a producer with mismatched
    // line endings produces.
    b.build_with_bad_offsets(1, &trailer_ids())
}

// ----------------------------------------------------------------- F07

fn f07_broken_truncated(_seed: u64) -> Vec<u8> {
    let mut b = Builder::new("1.4");
    standard_catalog(&mut b);
    basic_pages(&mut b, &[]);
    // Cut at 60%, with no trailer and no `%%EOF`.
    b.build_without_trailer(60)
}

// ----------------------------------------------------------------- F08

fn f08_broken_syntax(_seed: u64) -> Vec<u8> {
    // 700 bytes of junk before the header, which the specification says must be
    // tolerated, and a wrong `/Length`, and junk after `%%EOF`.
    let junk: Vec<u8> = (0..700u32).map(|i| b'!' + (i % 90) as u8).collect();
    let mut b = Builder::new("1.4")
        .with_preamble(&junk)
        .with_trailer_junk(b"\n% trailing junk after EOF\n");
    standard_catalog(&mut b);
    b.push(Obj::dict(
        3,
        &[("Type", "/Pages"), ("Kids", "[5 0 R]"), ("Count", "1")],
    ));
    b.push(Obj::dict(
        5,
        &[
            ("Type", "/Page"),
            ("Parent", "3 0 R"),
            ("Contents", "6 0 R"),
        ],
    ));
    // The declared length is wrong; the real body follows it.
    b.push(Obj::stream_with_length(
        6,
        999,
        b"BT /F1 24 Tf 72 700 Td (Wrong length) Tj ET",
    ));
    b.push(Obj::dict(
        4,
        &[
            ("Type", "/Font"),
            ("Subtype", "/Type1"),
            ("BaseFont", "/Helvetica"),
        ],
    ));
    b.build(1, &trailer_ids())
}

// ----------------------------------------------------------------- F09

/// A page tree with every mistake a real one carries.
fn f09_broken_pagetree(_seed: u64) -> Vec<u8> {
    let mut b = Builder::new("1.4");
    b.push(Obj::dict(1, &[("Type", "/Catalog"), ("Pages", "2 0 R")]));
    // `/Count` lies, `/Type` is missing, a kid is a page rather than a node, and the
    // tree refers to itself.
    b.push(Obj::raw(
        2,
        b"2 0 obj\n<< /Kids [3 0 R 5 0 R 2 0 R] /Count 99 /MediaBox [0 0 612 792] >>\nendobj\n",
    ));
    b.push(Obj::raw(
        3,
        b"3 0 obj\n<< /Parent 2 0 R /Contents 6 0 R /MediaBox [0 0 612 792] >>\nendobj\n",
    ));
    b.push(Obj::raw(
        5,
        b"5 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 7 0 R >>\nendobj\n",
    ));
    b.push(Obj::stream(
        6,
        &[],
        b"BT /F1 24 Tf 72 700 Td (Inherited box) Tj ET",
    ));
    b.push(Obj::stream(
        7,
        &[],
        b"BT /F1 24 Tf 72 700 Td (Second page) Tj ET",
    ));
    b.build(1, &trailer_ids())
}

// ----------------------------------------------------------------- F12

/// Paths, clips, dashes and line joins: the cases a rasterizer gets wrong.
fn f12_render_paths_clip(seed: u64) -> Vec<u8> {
    let mut content = String::new();
    // Fill rules: a star polygon, wound both ways.
    content.push_str("0.2 0.4 0.9 rg\n100 700 m 200 750 l 300 700 l 250 600 l 150 600 l h f\n");
    content.push_str("0.9 0.3 0.2 rg\n100 500 m 200 550 l 300 500 l 250 400 l 150 400 l h f*\n");
    // A nested clip: clip to a rounded region, then clip again, then draw.
    content.push_str("q 80 300 400 150 re W n 120 320 340 110 re W n 0 0 0 RG 2 w ");
    content.push_str("[6 3] 0 d 0 0 m 520 320 l S Q\n");
    // Hairlines and every cap and join.
    content.push_str("q 0.1 0.1 0.1 RG 0.25 w 0 J 0 j ");
    content.push_str("100 250 m 200 200 l 300 250 l 400 180 l S Q\n");
    // A degenerate path: zero length, which must not divide by zero.
    content.push_str("q 0 0 0 RG 1 w 500 200 m 500 200 l S Q\n");
    // A deep transparency group nesting.
    content.push_str("q 0.9 0.9 0.2 rg 0.5 gs ");
    for i in 0..20 {
        let _ = std::fmt::Write::write_fmt(
            &mut content,
            format_args!("{} {} 100 60 re f ", 40 + i * 12, 60 + i * 4),
        );
    }
    content.push_str("Q\n");
    if seed % 2 == 0 {
        // A second, optional section, so the manifest can say the seed matters.
        content.push_str("q 0.2 0.7 0.3 rg 300 120 m 500 120 l 500 180 l h f Q\n");
    }

    let mut b = Builder::new("1.7");
    standard_catalog(&mut b);
    b.push(Obj::dict(
        3,
        &[("Type", "/Pages"), ("Kids", "[5 0 R]"), ("Count", "1")],
    ));
    b.push(Obj::dict(
        5,
        &[
            ("Type", "/Page"),
            ("Parent", "3 0 R"),
            ("Contents", "6 0 R"),
        ],
    ));
    b.push(Obj::stream(6, &[], content.as_bytes()));
    b.push(Obj::dict(
        4,
        &[
            ("Type", "/Font"),
            ("Subtype", "/Type1"),
            ("BaseFont", "/Helvetica"),
        ],
    ));
    b.build(1, &trailer_ids())
}

// ----------------------------------------------------------------- F23

/// Annotations from several producers, with the awkward details.
fn f23_annots_foreign(_seed: u64) -> Vec<u8> {
    // Each annotation is its own object, which is how every producer writes them and
    // what a reader is entitled to assume.
    let annots: [(&str, &str); 8] = [
        (
            "50",
            "<< /Type /Annot /Subtype /Highlight /Rect [100 700 300 720] /QuadPoints \
             [100 720 300 720 300 700 100 700] /C [1 1 0] /CA 0.4 /F 4 /T (Reviewer) \
             /M (D:20260101120000Z) >>",
        ),
        (
            "51",
            "<< /Type /Annot /Subtype /Underline /Rect [100 660 300 680] /QuadPoints \
             [100 660 300 660 300 680 100 680] /C [0 0 1] /F 4 >>",
        ),
        (
            "52",
            "<< /Type /Annot /Subtype /Text /Rect [320 690 344 714] /Name /Comment \
             /Open true /F 4 /T (Reviewer) /Subj (Note) /Contents (Check this) >>",
        ),
        (
            "53",
            "<< /Type /Annot /Subtype /Square /Rect [100 560 300 620] /RD [10 10 10 10] \
             /BS << /W 2 >> /C [0 .5 0] /F 4 >>",
        ),
        (
            "54",
            "<< /Type /Annot /Subtype /Cloud /Rect [340 560 480 620] /C [0 0 1] \
             /BS << /W 1 /D [3 2] >> /BE << /S /C /I [2 2 2] /D [2 2] >> \
             /RD [12 12 12 12] /F 4 >>",
        ),
        (
            "55",
            "<< /Type /Annot /Subtype /Line /Rect [120 480 400 500] /L [120 480 400 500] \
             /LE << /S /O /L 12 >> /IC << /S /P >> /C [1 0 0] /T (Reviewer) \
             /Contents (A line) >>",
        ),
        (
            "56",
            "<< /Type /Annot /Subtype /FileAttachment /Rect [420 470 444 494] \
             /FS 57 0 R /Name /PushPin /F 4 >>",
        ),
        (
            "58",
            "<< /Type /Annot /Subtype /Link /Rect [100 430 300 450] \
             /A << /S /URI /URI (https://example.invalid/) >> /Border [0 0 0] >>",
        ),
    ];

    let mut b = Builder::new("1.7");
    standard_catalog(&mut b);
    b.push(Obj::dict(
        3,
        &[("Type", "/Pages"), ("Kids", "[5 0 R]"), ("Count", "1")],
    ));
    b.push(Obj::raw(
        5,
        b"5 0 obj\n<< /Type /Page /Parent 3 0 R /Contents 6 0 R /Rotate 90 \
          /Annots [50 0 R 51 0 R 52 0 R 53 0 R 54 0 R 55 0 R 56 0 R 58 0 R] >>\nendobj\n"
            .to_vec(),
    ));
    b.push(Obj::stream(
        6,
        &[],
        b"BT /F1 14 Tf 100 400 Td (Text under the annotations) Tj ET",
    ));
    b.push(Obj::dict(
        4,
        &[
            ("Type", "/Font"),
            ("Subtype", "/Type1"),
            ("BaseFont", "/Helvetica"),
        ],
    ));
    for (num, body) in annots {
        b.push(Obj::raw(
            num.parse().unwrap_or(0),
            format!("{num} 0 obj\n{body}\nendobj\n").into_bytes(),
        ));
    }
    b.push(Obj::raw(
        57,
        b"57 0 obj\n<< /Type /Filespec /F (note.txt) /UF (note.txt) >>\nendobj\n".to_vec(),
    ));
    b.build(1, &trailer_ids())
}

// ----------------------------------------------------------------- F25

/// Navigation: four levels of bookmarks, named destinations both ways, page labels.
fn f25_nav_meta(_seed: u64) -> Vec<u8> {
    let mut b = Builder::new("1.7");
    b.push(Obj::dict(
        1,
        &[
            ("Type", "/Catalog"),
            ("Pages", "2 0 R"),
            ("Outlines", "20 0 R"),
            ("PageMode", "/UseOutlines"),
            ("PageLayout", "/TwoColumnLeft"),
            ("Lang", "(en-GB)"),
            ("Names", "10 0 R"),
            ("Dests", "11 0 R"),
            ("OCProperties", "30 0 R"),
            (
                "ViewerPreferences",
                "<< /DisplayDocTitle true /HideToolbar false >>",
            ),
        ],
    ));
    b.push(Obj::dict(
        2,
        &[
            ("Type", "/Pages"),
            ("Kids", "[3 0 R]"),
            ("Count", "1"),
            ("MediaBox", "[0 0 612 792]"),
            ("Resources", "<< /Font << /F1 4 0 R >> >>"),
        ],
    ));
    b.push(Obj::dict(
        3,
        &[("Type", "/Pages"), ("Kids", "[5 0 R]"), ("Count", "1")],
    ));
    b.push(Obj::dict(
        5,
        &[
            ("Type", "/Page"),
            ("Parent", "3 0 R"),
            ("Contents", "6 0 R"),
        ],
    ));
    b.push(Obj::stream(6, &[], b"BT /F1 24 Tf 72 700 Td (Body) Tj ET"));
    b.push(Obj::dict(
        4,
        &[
            ("Type", "/Font"),
            ("Subtype", "/Type1"),
            ("BaseFont", "/Helvetica"),
        ],
    ));
    // Four levels, so a reader's nesting limit is exercised.
    b.push(Obj::raw(
        20,
        b"20 0 obj\n<< /Type /Outlines /First 21 0 R /Last 21 0 R /Count 1 >>\nendobj\n",
    ));
    b.push(Obj::raw(
        21,
        b"21 0 obj\n<< /Title (Chapter) /Parent 20 0 R /First 22 0 R /Last 22 0 R \
          /Count 1 /Dest [5 0 R /XYZ 0 792 0] >>\nendobj\n",
    ));
    b.push(Obj::raw(
        22,
        b"22 0 obj\n<< /Title (Section) /Parent 21 0 R /First 23 0 R /Last 23 0 R \
          /Count 1 /Dest [5 0 R /Fit] >>\nendobj\n",
    ));
    b.push(Obj::raw(
        23,
        b"23 0 obj\n<< /Title (Subsection) /Parent 22 0 R /First 24 0 R /Last 24 0 R \
          /Count 1 /Dest [5 0 R /FitH 400] >>\nendobj\n",
    ));
    b.push(Obj::raw(
        24,
        b"24 0 obj\n<< /Title (Detail) /Parent 23 0 R /Dest [5 0 R /XYZ null 300 null] >>\nendobj\n",
    ));
    // A name tree and a legacy `/Dests` dictionary naming the same places.
    b.push(Obj::raw(10, b"10 0 obj\n<< /Dests 12 0 R >>\nendobj\n"));
    b.push(Obj::raw(
        11,
        b"11 0 obj\n<< /Intro [5 0 R /Fit] >>\nendobj\n",
    ));
    b.push(Obj::raw(
        12,
        b"12 0 obj\n<< /Names [(Start) 5 0 R /Dests] >>\nendobj\n",
    ));
    b.push(Obj::dict(13, &[("Dests", "12 0 R")]));
    b.push(Obj::raw(13, b"13 0 obj\n<< /Dests 12 0 R >>\nendobj\n"));
    // Page labels: roman, then decimal, then a letter range.
    b.push(Obj::raw(
        40,
        b"40 0 obj\n<< /Nums [0 << /S /r >> 2 << /S /D /St 1 >>] >>\nendobj\n",
    ));
    b.push(Obj::raw(
        1,
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R /Outlines 20 0 R /PageLabels 40 0 R \
          /Lang (en-GB) /OCProperties 30 0 R /ViewerPreferences << /DisplayDocTitle true >> >>\nendobj\n",
    ));
    // Optional content: three layers, one off by default.
    b.push(Obj::raw(
        30,
        b"30 0 obj\n<< /OCGs [31 0 R 32 0 R 33 0 R] /D << /Order [31 0 R 32 0 R 33 0 R] \
          /ON [31 0 R 32 0 R] /BaseState /ON >> >>\nendobj\n",
    ));
    b.push(Obj::raw(
        31,
        b"31 0 obj\n<< /Type /OCG /Name (Roads) /Usage << /View << /ViewState /ON >> /Print << /PrintState /ON >> >> >>\nendobj\n",
    ));
    b.push(Obj::raw(
        32,
        b"32 0 obj\n<< /Type /OCG /Name (Labels) /Usage << /View << /ViewState /ON >> /Print << /PrintState /ON >> >> >>\nendobj\n",
    ));
    b.push(Obj::raw(
        33,
        b"33 0 obj\n<< /Type /OCG /Name (Terrain) /ON false /Usage << /View << /ViewState /OFF >> >> >>\nendobj\n",
    ));
    b.build(1, &trailer_ids())
}

// ----------------------------------------------------------------- F33

/// A confidential brief with secrets in every place a leak hides.
fn f33_confidential(seed: u64) -> Vec<u8> {
    let ssn = "123-45-6789";
    let email = "j.doe@example.invalid";
    let phone = "+1 555 0100";
    let account = "000123456789";

    let body = format!(
        "BT /F1 11 Tf 72 720 Td (Employee {ssn}) Tj 0 -16 Td (Contact {email}) Tj \
         0 -16 Td (Phone {phone}) Tj 0 -16 Td (Account {account}) Tj ET"
    );

    let mut b = Builder::new("1.7");
    b.push(Obj::raw(
        1,
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R /Metadata 9 0 R /Lang (en-US) \
          /Names 20 0 R >>\nendobj\n"
            .to_vec(),
    ));
    // The catalogue names a `/Names` dictionary, so the file has to contain one: a
    // reference to nothing is a *wrong* file, not a damaged one.
    b.push(Obj::raw(
        20,
        b"20 0 obj\n<< /EmbeddedFiles << /Names [(note.txt) 21 0 R] >> >>\nendobj\n".to_vec(),
    ));
    b.push(Obj::raw(
        21,
        b"21 0 obj\n<< /Type /Filespec /F (note.txt) /UF (note.txt) /EF << /F 22 0 R >> >>\nendobj\n"
            .to_vec(),
    ));
    b.push(Obj::stream(22, &[], b"an attached note\n"));
    b.push(Obj::dict(
        2,
        &[
            ("Type", "/Pages"),
            ("Kids", "[3 0 R]"),
            ("Count", "1"),
            ("MediaBox", "[0 0 612 792]"),
            ("Resources", "<< /Font << /F1 4 0 R >> >>"),
        ],
    ));
    b.push(Obj::dict(
        3,
        &[("Type", "/Pages"), ("Kids", "[5 0 R]"), ("Count", "1")],
    ));
    b.push(Obj::dict(
        5,
        &[
            ("Type", "/Page"),
            ("Parent", "3 0 R"),
            ("Contents", "6 0 R"),
            ("Annots", "[7 0 R]"),
        ],
    ));
    b.push(Obj::stream(6, &[], body.as_bytes()));
    // A comment that carries the same number, because a comment is still a leak.
    b.push(Obj::stream(
        6,
        &[],
        format!("% reviewer note: {account}\n{body}").as_bytes(),
    ));
    b.push(Obj::dict(
        4,
        &[
            ("Type", "/Font"),
            ("Subtype", "/Type1"),
            ("BaseFont", "/Helvetica"),
        ],
    ));
    // The number in a sticky note.
    b.push(Obj::raw(
        7,
        format!(
            "7 0 obj\n<< /Type /Annot /Subtype /Text /Rect [10 10 34 34] \
             /Contents (verify {ssn}) /T (Reviewer) /Name /Comment >>\nendobj\n"
        )
        .into_bytes(),
    ));
    // ... in the XMP ...
    b.push(Obj::stream(
        9,
        &[("Type", "/Metadata"), ("Subtype", "/XML")],
        format!(
            "<?xpacket?><x:xmpmeta xmlns:x='adobe:ns:meta/'><rdf:Description \
             xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'><dc:creator \
             xmlns:dc='http://purl.org/dc/elements/1.1/'><rdf:li>{email}</rdf:li>\
             </dc:creator></rdf:Description></x:xmpmeta><?xpacket?>"
        )
        .as_bytes(),
    ));
    // ... and in `/Info`, which is the easiest of all to miss.
    b.push(Obj::raw(
        30,
        format!(
            "30 0 obj\n<< /Title (Confidential brief) /Author (j.doe) /Subject ({ssn}) \
             /Keywords ({account}) /Producer (fixturegen) >>\nendobj\n"
        )
        .into_bytes(),
    ));
    if seed % 2 == 1 {
        // An older revision that still carries the number, so an incremental save that
        // only looks at the last revision leaves it behind.
        let mut extra = b.build(1, &format!("/Info 30 0 R {ids}", ids = trailer_ids()));
        // Without `/Prev` a reader sees only this revision's single entry and the
        // catalogue becomes unfindable, which is not what the fixture is testing.
        let prev = find_startxref(&extra);
        let at = extra.len();
        extra.extend_from_slice(
            format!("31 0 obj\n<< /Note (old revision {phone}) >>\nendobj\n").as_bytes(),
        );
        let xref = extra.len();
        extra.extend_from_slice(
            format!(
                "xref\n0 1\n0000000000 65535 f \n31 1\n{at:010} 00000 n \ntrailer\n\
                 << /Size 32 /Root 1 0 R /Info 30 0 R /Prev {prev} >>\nstartxref\n\
                 {xref}\n%%EOF\n"
            )
            .as_bytes(),
        );
        return extra;
    }
    b.build(1, &format!("/Info 30 0 R {ids}", ids = trailer_ids()))
}

// ----------------------------------------------------------------- helpers

fn find_startxref(bytes: &[u8]) -> usize {
    let text = String::from_utf8_lossy(bytes);
    text.rfind("startxref")
        .and_then(|at| {
            text[at..]
                .split_ascii_whitespace()
                .nth(1)
                .and_then(|n| n.parse().ok())
        })
        .unwrap_or(0)
}

fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            id: "F01",
            name: "struct_classic",
            purpose: "a classic cross-reference table and the basic objects",
            features: &["xref-table", "classic-objects", "ascii-hex-stream"],
            pages: 2,
            text: &["Page one", "Page two"],
            expect: Expectation::Valid,
            reader_repairs: false,
            build: f01_struct_classic,
        },
        Fixture {
            id: "F03",
            name: "struct_hybrid",
            purpose: "a classic table beside a cross-reference stream",
            features: &["hybrid", "xref-stream", "xref-stm"],
            pages: 2,
            text: &["Page one", "Page two"],
            expect: Expectation::Valid,
            reader_repairs: false,
            build: f03_struct_hybrid,
        },
        Fixture {
            id: "F04",
            name: "struct_incremental_x5",
            purpose: "five revisions, an object added and then freed, and a /Prev chain",
            features: &["incremental", "five-revisions", "free-entry", "prev-chain"],
            pages: 2,
            text: &["Revision 4", "Page two"],
            expect: Expectation::Valid,
            reader_repairs: false,
            build: f04_struct_incremental_x5,
        },
        Fixture {
            id: "F05",
            name: "struct_linearized",
            purpose: "a linearised file, whose first revision is not at the end",
            features: &["linearised", "xref-table"],
            pages: 2,
            text: &["Page one", "Page two"],
            expect: Expectation::Valid,
            reader_repairs: false,
            build: f05_struct_linearized,
        },
        Fixture {
            id: "F06",
            name: "broken_xref",
            purpose: "every cross-reference offset is wrong",
            features: &["broken", "bad-offsets"],
            pages: 2,
            text: &["Page one", "Page two"],
            expect: Expectation::Repaired,
            reader_repairs: true,
            build: f06_broken_xref,
        },
        Fixture {
            id: "F07",
            name: "broken_truncated",
            purpose: "cut at 60% with no trailer",
            features: &["broken", "truncated", "no-trailer"],
            // The second page's objects are past the cut, so a correct reader finds
            // one page. The manifest records what is recoverable, not what was written.
            pages: 1,
            text: &["Page one", "Page two"],
            expect: Expectation::Repaired,
            reader_repairs: true,
            build: f07_broken_truncated,
        },
        Fixture {
            id: "F08",
            name: "broken_syntax",
            purpose: "junk before the header, a wrong /Length, junk after EOF",
            features: &["broken", "junk-preamble", "wrong-length", "junk-trailer"],
            pages: 1,
            text: &["Wrong length"],
            expect: Expectation::Repaired,
            reader_repairs: false,
            build: f08_broken_syntax,
        },
        Fixture {
            id: "F09",
            name: "broken_pagetree",
            purpose: "a /Count that lies, a missing /Type, a self-reference and inheritance",
            features: &["broken", "page-tree", "cycle", "inherited-attributes"],
            pages: 2,
            text: &["Inherited box", "Second page"],
            expect: Expectation::Rejected,
            // The file's own structure is sound; what is wrong is the tree it
            // describes, and a reader that walks the cycle and skips it recovers both
            // pages without rebuilding anything.
            reader_repairs: false,
            build: f09_broken_pagetree,
        },
        Fixture {
            id: "F12",
            name: "render_paths_clip",
            purpose: "fill rules, nested clips, dashes, caps, joins and degenerate paths",
            features: &["paths", "clips", "fill-rule", "dashes", "transparency"],
            pages: 1,
            text: &[],
            expect: Expectation::Valid,
            reader_repairs: false,
            build: f12_render_paths_clip,
        },
        Fixture {
            id: "F23",
            name: "annots_foreign",
            purpose: "annotations in the styles other producers write them",
            features: &[
                "annotations",
                "quadpoints-both-orders",
                "popup",
                "cloud",
                "line-endings",
                "file-attachment",
                "uri-action",
                "rotated-page",
            ],
            pages: 1,
            text: &["Text under the annotations"],
            expect: Expectation::Repaired,
            reader_repairs: false,
            build: f23_annots_foreign,
        },
        Fixture {
            id: "F25",
            name: "nav_meta",
            purpose: "four levels of bookmarks, both destination mechanisms, labels, layers",
            features: &[
                "outlines",
                "named-destinations",
                "legacy-dests",
                "page-labels",
                "optional-content",
                "viewer-preferences",
                "xmp",
            ],
            pages: 1,
            text: &["Body"],
            expect: Expectation::Repaired,
            reader_repairs: false,
            build: f25_nav_meta,
        },
        Fixture {
            id: "F33",
            name: "Confidential_brief",
            purpose: "secrets in the body, a comment, a note, the XMP, /Info and an old revision",
            features: &[
                "secrets",
                "metadata",
                "xmp",
                "annotation-contents",
                "comments",
                "incremental",
            ],
            pages: 1,
            text: &["Employee 123-45-6789"],
            expect: Expectation::Repaired,
            reader_repairs: false,
            build: f33_confidential,
        },
    ]
}

/// A quoted, comma-separated TOML array.
fn toml_list(key: &str, values: &[&str]) -> String {
    let items: Vec<String> = values.iter().map(|v| format!("\"{v}\"")).collect();
    format!("{key} = [{}]\n", items.join(", "))
}

fn main() {
    let mut seed = 20_260_101u64;
    let mut out_dir = PathBuf::from("fixtures");
    let mut manifest_only = false;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        match arg.as_str() {
            "--seed" => {
                i += 1;
                seed = args.get(i).and_then(|s| s.parse().ok()).unwrap_or_else(|| {
                    eprintln!("--seed needs a number");
                    std::process::exit(2);
                });
            }
            "--out" => {
                i += 1;
                out_dir = PathBuf::from(args.get(i).map_or("fixtures", String::as_str));
            }
            "--manifest" => manifest_only = true,
            other => {
                eprintln!("unknown option `{other}`");
                std::process::exit(2);
            }
        }
        i += 1;
    }

    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!("could not create {}: {e}", out_dir.display());
        std::process::exit(1);
    }

    let mut manifest = String::from(
        "# Generated by `cargo xtask fixtures`. Do not edit by hand.\n\
         #\n\
         # Every fixture here is produced by tools/fixturegen, which depends on no\n\
         # mangle-* crate. The hashes are what `cargo xtask fixtures --check` compares.\n\n",
    );
    manifest.push_str("seed = ");
    manifest.push_str(&seed.to_string());
    manifest.push('\n');

    for f in fixtures() {
        let bytes = (f.build)(seed);
        let name = format!("{}_{}.pdf", f.id, f.name);
        let path: PathBuf = out_dir.join(&name);
        if !manifest_only && let Err(e) = std::fs::write(&path, &bytes) {
            eprintln!("could not write {}: {e}", path.display());
            std::process::exit(1);
        }

        let hash = sha256_hex(&bytes);
        let _ = writeln!(
            manifest,
            "\n[[fixture]]\nid = \"{}\"\nfile = \"{name}\"\npurpose = \"{}\"\n\
             pages = {}\nbytes = {}\nexpect = \"{}\"\nreader_repairs = {}\n\
             sha256 = \"{hash}\"",
            f.id,
            f.purpose,
            f.pages,
            bytes.len(),
            f.expect.as_str(),
            f.reader_repairs
        );
        manifest.push_str(&toml_list("features", f.features));
        manifest.push_str(&toml_list("text", f.text));
        if !manifest_only {
            println!("{name:<44} {:>9} bytes  {hash}", bytes.len());
        }
    }

    // The secrets manifest, which the redaction work checks itself against.
    let secrets = Path::new(&out_dir).join("F33_secrets.json");
    if !manifest_only {
        let json = "{\n  \"tokens\": [\n    \"123-45-6789\",\n    \"j.doe@example.invalid\",\n    \"+1 555 0100\",\n    \"000123456789\"\n  ],\n  \"locations\": [\n    {\"token\": \"123-45-6789\", \"where\": \"page 1 content stream\"},\n    {\"token\": \"123-45-6789\", \"where\": \"annotation 7 contents\"},\n    {\"token\": \"123-45-6789\", \"where\": \"/Info /Subject\"},\n    {\"token\": \"j.doe@example.invalid\", \"where\": \"page 1 content stream\"},\n    {\"token\": \"j.doe@example.invalid\", \"where\": \"XMP /Metadata\"},\n    {\"token\": \"+1 555 0100\", \"where\": \"page 1 content stream\"},\n    {\"token\": \"000123456789\", \"where\": \"page 1 content stream\"},\n    {\"token\": \"000123456789\", \"where\": \"page 1 comment\"},\n    {\"token\": \"000123456789\", \"where\": \"/Info /Keywords\"}\n  ]\n}\n";
        if let Err(e) = std::fs::write(&secrets, json) {
            eprintln!("could not write {}: {e}", secrets.display());
            std::process::exit(1);
        }
    }

    let manifest_path = Path::new(&out_dir).join("MANIFEST.toml");
    if let Err(e) = std::fs::write(&manifest_path, &manifest) {
        eprintln!("could not write {}: {e}", manifest_path.display());
        std::process::exit(1);
    }
    if manifest_only {
        print!("{manifest}");
    } else {
        println!(
            "\n{} fixtures, manifest at {}",
            fixtures().len(),
            manifest_path.display()
        );
    }
}
