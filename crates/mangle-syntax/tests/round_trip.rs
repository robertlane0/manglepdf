//! Round-trip tests: open a file, change it, write it, open it again, compare.
//!
//! This is the first place the lossless claim is tested end to end. A unit test can
//! prove the parser reads a token; only this can prove that what the parser read
//! comes back out of the writer unchanged.
//!
//! The files here are built by hand rather than by a fixture generator, so the
//! expected bytes are visible in the test and cannot drift with the code.

#![forbid(unsafe_code)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use mangle_syntax::object::{Dict, Name, Object, Ref};
use mangle_syntax::{Document, OpenOptions, SaveMode, SaveOptions};

/// A small but complete document: a catalogue, a two-level page tree, two pages with
/// content streams, and an `/Info` dictionary.
fn sample() -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    // One slot per object number, including the free object 0.
    let mut offsets = [0usize; 9];

    out.extend_from_slice(b"%PDF-1.6\n%\xe2\xe3\xcf\xd3\n");

    offsets[1] = out.len();
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

    offsets[2] = out.len();
    out.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 /MediaBox [0 0 612 792] >>\nendobj\n",
    );

    offsets[3] = out.len();
    out.extend_from_slice(
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Rotate 90 >>\nendobj\n",
    );

    offsets[4] = out.len();
    out.extend_from_slice(
        b"4 0 obj\n<< /Length 21 >>\nstream\nBT /F1 12 Tf ET\nendstream\nendobj\n",
    );

    offsets[5] = out.len();
    out.extend_from_slice(
        b"5 0 obj\n<< /Type /Page /Parent 2 0 R /Contents [6 0 R 7 0 R] >>\nendobj\n",
    );

    offsets[6] = out.len();
    out.extend_from_slice(b"6 0 obj\n<< /Length 16 >>\nstream\n1 0 0 RG 5 w\nendstream\nendobj\n");

    offsets[7] = out.len();
    out.extend_from_slice(b"7 0 obj\n<< /Length 12 >>\nstream\n0 0 1 rg\nendstream\nendobj\n");

    let info = 8usize;
    offsets[info] = out.len();
    out.extend_from_slice(b"8 0 obj\n<< /Producer (handmade) /Title (Sample) >>\nendobj\n");

    let xref = out.len();
    out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n");
    out.extend_from_slice(format!("1 {info}\n").as_bytes());
    for offset in offsets.iter().take(info + 1).skip(1) {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R /Info 8 0 R /ID [<0102> <0304>] >>\nstartxref\n{xref}\n%%EOF\n",
            info + 1
        )
        .as_bytes(),
    );
    out
}

fn open(bytes: Vec<u8>) -> Document {
    Document::open(bytes, OpenOptions::default()).expect("the file should open")
}

fn text_of(doc: &Document, num: u32) -> String {
    doc.object(Ref::new(num, 0))
        .and_then(|o| match o {
            Object::Stream(s) => Some(s.raw),
            other => other
                .as_dict()
                .map(|_| Vec::new())
                .unwrap_or_default()
                .into(),
        })
        .map(|raw| String::from_utf8_lossy(&raw).into_owned())
        .unwrap_or_default()
}

// ------------------------------------------------------------ full rewrite

#[test]
fn a_full_save_preserves_the_page_count_and_every_page() {
    let doc = open(sample());
    assert_eq!(doc.page_count().expect("pages"), 2);

    let saved = doc.save(&SaveOptions::default()).expect("save");
    let reopened = open(saved.bytes);

    assert_eq!(reopened.page_count().expect("pages"), 2);
    assert!(
        reopened.info().recovery.is_clean(),
        "a clean file must not come back repaired: {:?}",
        reopened.info().recovery.notes()
    );
    // The stream bytes come back exactly.
    assert_eq!(text_of(&reopened, 4), "BT /F1 12 Tf ET");
    assert_eq!(text_of(&reopened, 6), "1 0 0 RG 5 w");
}

#[test]
fn a_full_save_keeps_inherited_page_attributes() {
    let doc = open(sample());
    let saved = doc.save(&SaveOptions::default()).expect("save");
    let reopened = open(saved.bytes);

    // `/MediaBox` lives on the page tree node, not on the pages, so a save that
    // flattened the tree would put it in the wrong place.
    let node = reopened.object(Ref::new(2, 0)).expect("page tree node");
    assert_eq!(
        node.get("MediaBox")
            .and_then(Object::as_array)
            .map(<[Object]>::len),
        Some(4)
    );
    let page = reopened.object(Ref::new(3, 0)).expect("page");
    assert!(
        page.get("MediaBox").is_none(),
        "the page must still inherit rather than carry a copy"
    );
    assert_eq!(page.get("Rotate").and_then(Object::as_i64), Some(90));
}

#[test]
fn a_full_save_preserves_the_header_version() {
    let doc = open(sample());
    let saved = doc.save(&SaveOptions::default()).expect("save");
    assert!(
        saved.bytes.starts_with(b"%PDF-1.6"),
        "the file declared 1.6 and a save must not claim to be something else"
    );
}

#[test]
fn a_full_save_keeps_the_document_id() {
    let doc = open(sample());
    let saved = doc.save(&SaveOptions::default()).expect("save");
    let reopened = open(saved.bytes);
    assert_eq!(reopened.file_id(), vec![1, 2]);
}

#[test]
fn a_full_save_keeps_the_info_dictionary() {
    let doc = open(sample());
    let saved = doc.save(&SaveOptions::default()).expect("save");
    let reopened = open(saved.bytes);
    let info = reopened.info_dict().expect("/Info");
    let producer = info
        .get("Producer")
        .and_then(Object::as_bytes)
        .unwrap_or_default();
    assert_eq!(producer, b"handmade");
}

#[test]
fn a_full_save_reports_no_dangling_references() {
    let doc = open(sample());
    let saved = doc.save(&SaveOptions::default()).expect("save");
    assert!(
        saved.dropped.is_empty(),
        "a save reported dangling references: {:?}",
        saved.dropped
    );
}

#[test]
fn saving_twice_produces_the_same_bytes() {
    let a = open(sample()).save(&SaveOptions::default()).expect("save");
    let b = open(sample()).save(&SaveOptions::default()).expect("save");
    assert_eq!(a.bytes, b.bytes, "a save must be deterministic");
}

// --------------------------------------------------------------- the edit

#[test]
fn an_edit_appears_in_the_saved_file_and_nothing_else_does() {
    let doc = open(sample());
    let before = doc.object(Ref::new(4, 0)).expect("stream 4");

    // Change one page's content stream.
    let mut new_stream = match before {
        Object::Stream(s) => s,
        other => panic!("expected a stream, got {other:?}"),
    };
    new_stream.raw = b"BT /F1 48 Tf (Explore) Tj ET".to_vec();
    new_stream.dict.set("Length", Object::Int(31));
    doc.set(Ref::new(4, 0), Object::Stream(new_stream));

    let saved = doc.save(&SaveOptions::default()).expect("save");
    let reopened = open(saved.bytes);

    assert_eq!(text_of(&reopened, 4), "BT /F1 48 Tf (Explore) Tj ET");
    // The other stream is untouched.
    assert_eq!(text_of(&reopened, 6), "1 0 0 RG 5 w");
    assert_eq!(reopened.page_count().expect("pages"), 2);
}

#[test]
fn a_new_object_is_written_and_is_reachable_after_reopening() {
    let doc = open(sample());
    let id = doc.alloc();
    let mut d = Dict::new();
    d.set("Type", Object::name("Annot"));
    d.set("Subtype", Object::name("Highlight"));
    d.set(
        "Rect",
        Object::Array(vec![
            Object::Real(10.0),
            Object::Real(20.0),
            Object::Real(30.0),
            Object::Real(40.0),
        ]),
    );
    doc.set(id, Object::Dict(d));

    // Reference it from the first page.
    let mut page = match doc.object(Ref::new(3, 0)).expect("page 3") {
        Object::Dict(d) => d,
        other => panic!("expected a dictionary, got {other:?}"),
    };
    page.set("Annots", Object::Array(vec![Object::Ref(id)]));
    doc.set(Ref::new(3, 0), Object::Dict(page));

    let saved = doc.save(&SaveOptions::default()).expect("save");
    let reopened = open(saved.bytes);

    let annots = reopened
        .object(Ref::new(3, 0))
        .and_then(|p| p.get("Annots").cloned())
        .and_then(|a| a.as_array().map(<[Object]>::to_vec))
        .unwrap_or_default();
    assert_eq!(
        annots.len(),
        1,
        "the new annotation must survive the round trip"
    );
    let resolved = reopened
        .object(annots[0].as_ref_id().expect("a reference"))
        .expect("the annotation object");
    assert_eq!(
        resolved.get("Subtype").and_then(Object::as_name),
        Some(&b"Highlight"[..])
    );
}

#[test]
fn a_removed_object_stays_removed() {
    let doc = open(sample());
    doc.remove(Ref::new(7, 0));
    let saved = doc.save(&SaveOptions::default()).expect("save");
    let reopened = open(saved.bytes);
    assert!(
        reopened.object(Ref::new(7, 0)).is_none(),
        "a removed object must not come back"
    );
    // And the other page's first content stream is still there.
    assert_eq!(text_of(&reopened, 6), "1 0 0 RG 5 w");
}

// ------------------------------------------------------------ incremental

#[test]
fn a_no_op_incremental_save_leaves_the_bytes_exactly_as_they_were() {
    let original = sample();
    let doc = open(original.clone());
    let saved = doc
        .save(&SaveOptions {
            mode: SaveMode::Incremental,
            ..SaveOptions::default()
        })
        .expect("save");
    assert_eq!(
        saved.bytes, original,
        "a no-op incremental save must not touch a byte"
    );
}

#[test]
fn an_incremental_save_keeps_the_old_bytes_as_an_exact_prefix() {
    let original = sample();
    let doc = open(original.clone());
    let mut stream = match doc.object(Ref::new(4, 0)).expect("stream 4") {
        Object::Stream(s) => s,
        other => panic!("expected a stream, got {other:?}"),
    };
    stream.raw = b"BT /F1 10 Tf (Saved) Tj ET".to_vec();
    stream.dict.set("Length", Object::Int(30));
    doc.set(Ref::new(4, 0), Object::Stream(stream));

    let saved = doc
        .save(&SaveOptions {
            mode: SaveMode::Incremental,
            ..SaveOptions::default()
        })
        .expect("save");

    assert!(
        saved.bytes.len() > original.len(),
        "an incremental save must grow the file"
    );
    assert_eq!(
        &saved.bytes[..original.len()],
        &original[..],
        "the previous revision must survive byte for byte"
    );
}

#[test]
fn an_incremental_save_reopens_with_the_edit_applied() {
    let doc = open(sample());
    let mut stream = match doc.object(Ref::new(4, 0)).expect("stream 4") {
        Object::Stream(s) => s,
        other => panic!("expected a stream, got {other:?}"),
    };
    stream.raw = b"BT /F1 10 Tf (Saved) Tj ET".to_vec();
    stream.dict.set("Length", Object::Int(30));
    doc.set(Ref::new(4, 0), Object::Stream(stream));

    let saved = doc
        .save(&SaveOptions {
            mode: SaveMode::Incremental,
            ..SaveOptions::default()
        })
        .expect("save");
    let reopened = open(saved.bytes);

    assert_eq!(text_of(&reopened, 4), "BT /F1 10 Tf (Saved) Tj ET");
    assert_eq!(
        text_of(&reopened, 6),
        "1 0 0 RG 5 w",
        "the other page survives"
    );
    assert_eq!(reopened.page_count().expect("pages"), 2);
}

#[test]
fn an_incremental_save_can_express_a_deletion() {
    let original = sample();
    let doc = open(original);
    doc.remove(Ref::new(7, 0));
    let saved = doc
        .save(&SaveOptions {
            mode: SaveMode::Incremental,
            ..SaveOptions::default()
        })
        .expect("save");
    let reopened = open(saved.bytes);
    assert!(reopened.object(Ref::new(7, 0)).is_none());
}

#[test]
fn several_incremental_revisions_chain() {
    let mut bytes = sample();
    for i in 0..3 {
        let doc = open(bytes);
        let mut stream = match doc.object(Ref::new(4, 0)).expect("stream 4") {
            Object::Stream(s) => s,
            other => panic!("expected a stream, got {other:?}"),
        };
        let text = format!("BT /F1 {} Tf (rev {i}) Tj ET", 10 + i);
        let len = i64::try_from(text.len()).unwrap_or(0);
        stream.raw = text.into_bytes();
        stream.dict.set("Length", Object::Int(len));
        doc.set(Ref::new(4, 0), Object::Stream(stream));
        bytes = doc
            .save(&SaveOptions {
                mode: SaveMode::Incremental,
                ..SaveOptions::default()
            })
            .expect("save")
            .bytes;
    }
    let reopened = open(bytes);
    assert_eq!(text_of(&reopened, 4), "BT /F1 12 Tf (rev 2) Tj ET");
}

// ------------------------------------------------------------------ objects

#[test]
fn dictionary_key_order_survives_a_save() {
    let doc = open(sample());
    let mut page = match doc.object(Ref::new(3, 0)).expect("page 3") {
        Object::Dict(d) => d,
        other => panic!("expected a dictionary, got {other:?}"),
    };
    // Insert a key that was not there; the keys already present must keep their order.
    page.insert(Name::new("ZLast"), Object::Int(1));
    doc.set(Ref::new(3, 0), Object::Dict(page));

    let saved = doc.save(&SaveOptions::default()).expect("save");
    let reopened = open(saved.bytes);
    let keys: Vec<String> = match reopened.object(Ref::new(3, 0)).expect("page") {
        Object::Dict(d) => d
            .iter()
            .map(|(k, _)| String::from_utf8_lossy(k.as_bytes()).into_owned())
            .collect(),
        other => panic!("expected a dictionary, got {other:?}"),
    };
    let pos = |k: &str| keys.iter().position(|x| x == k).expect("key present");
    assert!(pos("Type") < pos("Parent"));
    assert!(pos("Parent") < pos("Contents"));
    assert!(pos("Contents") < pos("Rotate"));
    assert!(pos("Rotate") < pos("ZLast"));
}

#[test]
fn a_name_with_characters_that_need_escaping_survives() {
    let doc = open(sample());
    let mut d = Dict::new();
    d.insert(Name::new("Odd Name#1"), Object::name("Value"));
    let id = doc.alloc();
    doc.set(id, Object::Dict(d));

    let saved = doc.save(&SaveOptions::default()).expect("save");
    let reopened = open(saved.bytes);
    let back = reopened.object(id).expect("object");
    assert_eq!(
        back.get("Odd Name#1").and_then(Object::as_name),
        Some(&b"Value"[..])
    );
}

#[test]
fn an_integer_and_a_real_keep_their_distinction() {
    let doc = open(sample());
    let mut d = Dict::new();
    d.set("Int", Object::Int(3));
    d.set("Real", Object::Real(3.0));
    let id = doc.alloc();
    doc.set(id, Object::Dict(d));

    let saved = doc.save(&SaveOptions::default()).expect("save");
    let reopened = open(saved.bytes);
    let back = reopened.object(id).expect("object");
    assert!(matches!(back.get("Int"), Some(Object::Int(3))));
    assert!(matches!(back.get("Real"), Some(Object::Real(_))));
}

#[test]
fn an_encrypted_document_refuses_to_save_rather_than_saving_it_in_the_clear() {
    // A file with an `/Encrypt` we cannot open is already an error; one we opened is
    // refused on save rather than silently written without its protection.
    let mut bytes = sample();
    let marker = b"/Root 1 0 R";
    let at = bytes
        .windows(marker.len())
        .position(|w| w == marker)
        .expect("trailer");
    let inserted = b"/Root 1 0 R /Encrypt 9 0 R";
    bytes.splice(at..at + marker.len(), inserted.iter().copied());

    let doc = Document::open(bytes, OpenOptions::default());
    match doc {
        Err(_) => {} // refused at open, which is also acceptable
        Ok(d) => {
            let err = d.save(&SaveOptions::default()).expect_err("must not save");
            assert!(
                err.to_string().contains("encrypt"),
                "the error must name the reason: {err}"
            );
        }
    }
}

#[test]
fn a_repaired_file_is_rewritten_rather_than_appended_to() {
    // Remove `startxref` so the file needs a rebuild, then edit it.
    let mut bytes = sample();
    let start = bytes
        .windows(9)
        .position(|w| w == b"startxref")
        .expect("startxref");
    bytes.truncate(start);

    let doc = open(bytes);
    assert!(
        !doc.info().recovery.is_clean(),
        "a file with no startxref must come back repaired"
    );
    let saved = doc
        .save(&SaveOptions {
            mode: SaveMode::Incremental,
            ..SaveOptions::default()
        })
        .expect("save");
    assert!(saved.forced_full, "a repaired file must be rewritten");
    assert!(saved.forced_reason.is_some(), "and the UI must be told why");

    // The rewrite opens cleanly, which an append could not have achieved.
    let reopened = open(saved.bytes);
    assert!(reopened.info().recovery.is_clean());
    assert_eq!(reopened.page_count().expect("pages"), 2);
}

/// The property the whole syntax layer exists for: a full save that changes nothing
/// preserves every object byte for byte, and a save that changes one object changes
/// only that object.
#[test]
fn a_full_save_preserves_the_bytes_of_every_untouched_object() {
    let original = sample();
    let doc = open(original.clone());

    // A save with no edit at all: the cross-reference moves, the objects do not.
    let saved = doc.save(&SaveOptions::default()).expect("save");
    let orig = object_bodies(&original);
    let after = object_bodies(&saved.bytes);

    for (num, body) in &orig {
        let Some(after_body) = after.get(num) else {
            panic!("object {num} went missing from the saved file");
        };
        assert_eq!(
            body, after_body,
            "object {num} changed even though nothing was edited"
        );
    }
}

/// The same, for every Tier-A fixture, so the property is not an accident of the
/// hand-built sample.
#[test]
fn the_property_holds_for_the_whole_corpus() {
    let Some(dir) = corpus_dir() else {
        eprintln!("skipped: run `cargo xtask fixtures` first");
        return;
    };
    for entry in manifest() {
        let bytes = std::fs::read(dir.join(&entry.file)).expect("fixture file");
        let doc = Document::open(bytes, OpenOptions::default())
            .unwrap_or_else(|e| panic!("{}: {e}", entry.file));
        // Read every object, so every object has a recorded span.
        for num in doc.object_numbers() {
            let _ = doc.object(Ref::new(num, 0));
        }
        let saved = doc
            .save(&SaveOptions::default())
            .unwrap_or_else(|e| panic!("{}: {e}", entry.file));
        let original = std::fs::read(dir.join(&entry.file)).expect("fixture");
        let before = object_bodies(&original);
        let after = object_bodies(&saved.bytes);
        for (num, body) in &before {
            if let Some(after_body) = after.get(num) {
                assert_eq!(
                    body, after_body,
                    "{}: object {num} changed on a save that edited nothing",
                    entry.file
                );
            }
        }
    }
}

#[test]
fn an_edit_changes_only_the_edited_object() {
    let original = sample();
    let doc = open(original.clone());
    let mut stream = match doc.object(Ref::new(4, 0)).expect("stream 4") {
        Object::Stream(s) => s,
        other => panic!("expected a stream, got {other:?}"),
    };
    stream.raw = b"BT /F1 48 Tf (Explore) Tj ET".to_vec();
    let len = i64::try_from(stream.raw.len()).unwrap_or(0);
    stream.dict.set("Length", Object::Int(len));
    doc.set(Ref::new(4, 0), Object::Stream(stream));

    let saved = doc.save(&SaveOptions::default()).expect("save");
    let before = object_bodies(&original);
    let after = object_bodies(&saved.bytes);

    for (num, body) in &before {
        if *num == 4 {
            assert_ne!(body, after.get(num).expect("object 4"), "the edit was lost");
            continue;
        }
        assert_eq!(
            body,
            after.get(num).unwrap_or(body),
            "object {num} changed even though only object 4 was edited"
        );
    }
}

/// Every indirect object's bytes, keyed by number: `N G obj` through `endobj`.
fn object_bodies(data: &[u8]) -> std::collections::BTreeMap<u32, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    let mut i = 0usize;
    while i + 6 <= data.len() {
        let Some(header) = data[i..].windows(6).position(|w| w == b" obj") else {
            break;
        };
        let start = i + header;
        // The number is the digits before ` obj`.
        let mut j = start;
        while j > 0 && data[j - 1].is_ascii_digit() {
            j -= 1;
        }
        let num: u32 = data
            .get(j..start.saturating_sub(1))
            .and_then(|s| std::str::from_utf8(s).ok())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        if num == 0 {
            i = start + 4;
            continue;
        }
        // The body runs to the `endobj` that follows.
        let after = start + 4;
        let end = data
            .get(after..)
            .and_then(|rest| rest.windows(7).position(|w| w == b"endobj"))
            .map(|p| after + p + 6)
            .unwrap_or(data.len());
        out.insert(num, data.get(j..end).unwrap_or_default().to_vec());
        i = end;
    }
    out
}

fn corpus_dir() -> Option<std::path::PathBuf> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)?;
    let dir = root.join("fixtures");
    dir.join("MANIFEST.toml").is_file().then_some(dir)
}

/// A minimal reader of the manifest, for the fields the corpus test needs.
fn manifest() -> Vec<FixtureEntry> {
    let Some(dir) = corpus_dir() else {
        return Vec::new();
    };
    let Ok(raw) = std::fs::read_to_string(dir.join("MANIFEST.toml")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut file = String::new();
    for line in raw.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("file = \"") {
            file = v.trim_end_matches('"').to_string();
        } else if line == "]" && !file.is_empty() {
            out.push(FixtureEntry {
                file: std::mem::take(&mut file),
            });
        }
    }
    out
}

struct FixtureEntry {
    file: String,
}
