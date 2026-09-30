//! Oracle tests: what we write must be accepted by an implementation that is not us.
//!
//! `qpdf` is used as a test oracle only. It is never linked, never called from product
//! code, and `cargo test` passes without it — when it is absent the test records the
//! skip and returns. What it tells us that our own round trip cannot is whether the
//! file is *valid*, rather than merely re-readable by the code that wrote it.

#![forbid(unsafe_code)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::process::Command;

use mangle_syntax::object::{Dict, Object, Ref, Stream};
use mangle_syntax::{Document, OpenOptions, SaveMode, SaveOptions};

/// A scratch directory that cleans itself up.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Option<Self> {
        let dir = std::env::temp_dir().join(format!("manglepdf-oracle-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).ok()?;
        Some(Self(dir))
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).expect("scratch file");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// A small complete document with two pages, a font, an image-free content stream and
/// an annotation, so the oracle is exercised on something representative.
fn build() -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut at = [0usize; 12];
    out.extend_from_slice(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n");

    at[1] = out.len();
    out.extend_from_slice(
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R /Outlines 8 0 R /PageLabels 10 0 R >>\nendobj\n",
    );
    at[2] = out.len();
    out.extend_from_slice(
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 /MediaBox [0 0 612 792] \
          /Resources << /Font << /F1 7 0 R >> >> >>\nendobj\n",
    );
    at[3] = out.len();
    out.extend_from_slice(
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Annots [9 0 R] \
          /Rotate 90 >>\nendobj\n",
    );
    at[4] = out.len();
    out.extend_from_slice(&stream_object(4, b"BT /F1 48 Tf 72 700 Td (Explore) Tj ET"));
    at[5] = out.len();
    out.extend_from_slice(b"5 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>\nendobj\n");
    at[6] = out.len();
    out.extend_from_slice(&stream_object(6, b"1 0 0 RG 5 w 10 10 m 200 200 l S"));
    at[7] = out.len();
    out.extend_from_slice(
        b"7 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n",
    );
    at[8] = out.len();
    out.extend_from_slice(b"8 0 obj\n<< /Type /Outlines /Count 0 >>\nendobj\n");
    at[9] = out.len();
    out.extend_from_slice(
        b"9 0 obj\n<< /Type /Annot /Subtype /Highlight /Rect [72 690 200 720] \
          /QuadPoints [72 720 200 720 200 690 72 690] /F 4 /C [1 0 0] >>\nendobj\n",
    );
    at[10] = out.len();
    out.extend_from_slice(b"10 0 obj\n<< /Nums [0 << /S /r >> 2 << /S /D /St 5 >>] >>\nendobj\n");

    let xref = out.len();
    out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n1 10\n");
    for offset in at.iter().take(11).skip(1) {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size 11 /Root 1 0 R /ID [<0102> <0304>] >>\nstartxref\n{xref}\n%%EOF\n"
        )
        .as_bytes(),
    );
    out
}

/// An indirect stream object with a `/Length` that is actually right.
fn stream_object(num: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = format!("{num} 0 obj\n<< /Length {} >>\nstream\n", payload.len()).into_bytes();
    out.extend_from_slice(payload);
    out.extend_from_slice(b"\nendstream\nendobj\n");
    out
}

/// Run `qpdf --check` and return its verdict.
fn qpdf_check(path: &Path) -> Option<(bool, String)> {
    if !have("qpdf") {
        return None;
    }
    let out = Command::new("qpdf")
        .arg("--check")
        .arg(path)
        .output()
        .expect("qpdf runs");
    // qpdf exits 0 for clean, 3 for "succeeded with warnings" and non-zero for an
    // error. A warning is a real finding: it means some reader had to guess.
    let clean = out.status.success();
    Some((
        clean,
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr),
    ))
}

fn tool_available() -> bool {
    have("qpdf")
}

#[test]
fn the_handmade_fixture_is_itself_valid() {
    // If the input fails the oracle, a failure below says nothing about our writer.
    let Some(scratch) = Scratch::new("input") else {
        eprintln!("skipped: no scratch directory");
        return;
    };
    let path = scratch.write("input.pdf", &build());
    let Some((ok, text)) = qpdf_check(&path) else {
        eprintln!("skipped: qpdf is not installed");
        return;
    };
    assert!(ok, "the hand-built fixture must be valid:\n{text}");
}

#[test]
fn a_full_save_is_accepted_by_qpdf() {
    if !tool_available() {
        eprintln!("skipped: qpdf is not installed");
        return;
    }
    let Some(scratch) = Scratch::new("full") else {
        return;
    };
    let doc = Document::open(build(), OpenOptions::default()).expect("open");
    let saved = doc.save(&SaveOptions::default()).expect("save");
    let path = scratch.write("full.pdf", &saved.bytes);
    let (ok, text) = qpdf_check(&path).expect("qpdf runs");
    assert!(ok, "qpdf rejected our full save:\n{text}");
}

#[test]
fn an_incremental_save_is_accepted_by_qpdf() {
    if !tool_available() {
        eprintln!("skipped: qpdf is not installed");
        return;
    }
    let Some(scratch) = Scratch::new("incr") else {
        return;
    };
    let doc = Document::open(build(), OpenOptions::default()).expect("open");

    let mut stream = match doc.object(Ref::new(4, 0)).expect("content") {
        Object::Stream(s) => s,
        other => panic!("expected a stream, got {other:?}"),
    };
    stream.raw = b"BT /F1 52 Tf 72 700 Td (Beautiful Places) Tj ET".to_vec();
    let len = i64::try_from(stream.raw.len()).unwrap_or(0);
    stream.dict.set("Length", Object::Int(len));
    doc.set(Ref::new(4, 0), Object::Stream(stream));

    let saved = doc
        .save(&SaveOptions {
            mode: SaveMode::Incremental,
            ..SaveOptions::default()
        })
        .expect("save");
    let path = scratch.write("incr.pdf", &saved.bytes);
    let (ok, text) = qpdf_check(&path).expect("qpdf runs");
    assert!(ok, "qpdf rejected our incremental save:\n{text}");
}

#[test]
fn qpdf_agrees_about_the_page_count_after_a_save() {
    if !tool_available() {
        eprintln!("skipped: qpdf is not installed");
        return;
    }
    let Some(scratch) = Scratch::new("pages") else {
        return;
    };
    let doc = Document::open(build(), OpenOptions::default()).expect("open");
    let ours = doc.page_count().expect("pages");
    let saved = doc.save(&SaveOptions::default()).expect("save");
    let path = scratch.write("pages.pdf", &saved.bytes);

    let out = Command::new("qpdf")
        .arg("--show-npages")
        .arg(&path)
        .output()
        .expect("qpdf runs");
    let theirs: usize = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0);
    assert_eq!(ours, theirs, "qpdf disagrees about the page count");
    assert_eq!(ours, 2);
}

#[test]
fn qpdf_sees_the_edit_we_made() {
    if !tool_available() {
        eprintln!("skipped: qpdf is not installed");
        return;
    }
    let Some(scratch) = Scratch::new("edit") else {
        return;
    };
    let doc = Document::open(build(), OpenOptions::default()).expect("open");
    let mut stream = match doc.object(Ref::new(4, 0)).expect("content") {
        Object::Stream(s) => s,
        other => panic!("expected a stream, got {other:?}"),
    };
    stream.raw = b"BT /F1 52 Tf 72 700 Td (Beautiful Places) Tj ET".to_vec();
    let len = i64::try_from(stream.raw.len()).unwrap_or(0);
    stream.dict.set("Length", Object::Int(len));
    doc.set(Ref::new(4, 0), Object::Stream(stream));

    let saved = doc
        .save(&SaveOptions {
            mode: SaveMode::Incremental,
            ..SaveOptions::default()
        })
        .expect("save");
    let path = scratch.write("edit.pdf", &saved.bytes);

    // An independent tool must see the new string and not the old one.
    let out = Command::new("qpdf")
        .arg("--qdf")
        .arg("--object-streams=disable")
        .arg(&path)
        .arg("-")
        .output()
        .expect("qpdf runs");
    assert!(
        out.status.success(),
        "qpdf could not read the file: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("Beautiful Places"),
        "qpdf did not see the edit"
    );
    assert!(
        !text.contains("Explore"),
        "the replaced string is still in the file"
    );
}

#[test]
fn a_saved_file_still_has_the_structure_the_reader_relies_on() {
    if !tool_available() {
        eprintln!("skipped: qpdf is not installed");
        return;
    }
    let Some(scratch) = Scratch::new("struct") else {
        return;
    };
    let doc = Document::open(build(), OpenOptions::default()).expect("open");
    let mut new_page = Dict::new();
    new_page.set("Type", Object::name("XObject"));
    new_page.set("Subtype", Object::name("Form"));
    new_page.set(
        "BBox",
        Object::Array(vec![
            Object::Real(0.0),
            Object::Real(0.0),
            Object::Real(1.0),
            Object::Real(1.0),
        ]),
    );
    new_page.set("Length", Object::Int(0));
    let id = doc.alloc();
    doc.set(
        id,
        Object::Stream(Stream {
            dict: new_page,
            raw: b"0 0 1 rg 0 0 10 10 re f".to_vec(),
            file_offset: None,
            synthetic: true,
        }),
    );

    let saved = doc.save(&SaveOptions::default()).expect("save");
    let path = scratch.write("struct.pdf", &saved.bytes);
    let (ok, text) = qpdf_check(&path).expect("qpdf runs");
    assert!(ok, "qpdf rejected a file with a new object:\n{text}");
    // And our own reader finds it again.
    let reopened = Document::open(saved.bytes, OpenOptions::default()).expect("reopen");
    assert!(matches!(reopened.object(id), Some(Object::Stream(_))));
}
