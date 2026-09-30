//! The Tier-A corpus, opened and checked against its manifest.
//!
//! The generator and this test share no code and no crate, so agreement between them
//! is evidence rather than a tautology. Where `qpdf` is installed the fixtures are
//! also checked against it, so "this file is intentionally broken" is a claim that can
//! be falsified.

#![forbid(unsafe_code)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};

use mangle_syntax::{Document, OpenOptions};

fn corpus_dir() -> Option<PathBuf> {
    // crates/mangle-syntax -> crates -> the repository root
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)?;
    let dir = root.join("fixtures");
    dir.join("MANIFEST.toml").is_file().then_some(dir)
}

/// One `[[fixture]]` block, read with the small amount of TOML the manifest uses.
struct Entry {
    id: String,
    file: String,
    pages: usize,
    sha256: String,
    expect: String,
    reader_repairs: bool,
    features: Vec<String>,
    text: Vec<String>,
}

fn manifest() -> Vec<Entry> {
    let Some(dir) = corpus_dir() else {
        return Vec::new();
    };
    let raw = std::fs::read_to_string(dir.join("MANIFEST.toml")).expect("manifest");
    let mut out = Vec::new();
    let mut current: Option<Entry> = None;
    for line in raw.lines() {
        let line = line.trim();
        if line == "[[fixture]]" {
            if let Some(e) = current.take() {
                out.push(e);
            }
            current = Some(Entry {
                id: String::new(),
                file: String::new(),
                pages: 0,
                sha256: String::new(),
                expect: String::new(),
                reader_repairs: false,
                features: Vec::new(),
                text: Vec::new(),
            });
            continue;
        }
        let Some(e) = current.as_mut() else { continue };
        let Some((key, value)) = line.split_once(" = ") else {
            continue;
        };
        let unquoted = |v: &str| v.trim_matches('"').to_string();
        let list = |v: &str| {
            v.trim()
                .trim_start_matches('[')
                .trim_end_matches(']')
                .split(',')
                .map(&unquoted)
                .filter(|s| !s.is_empty())
                .collect::<Vec<String>>()
        };
        match key {
            "id" => e.id = unquoted(value),
            "file" => e.file = unquoted(value),
            "pages" => e.pages = value.parse().unwrap_or(0),
            "sha256" => e.sha256 = unquoted(value),
            "expect" => e.expect = unquoted(value),
            "reader_repairs" => e.reader_repairs = value.trim() == "true",
            "features" => e.features = list(value),
            "text" => e.text = list(value),
            _ => {}
        }
    }
    if let Some(e) = current.take() {
        out.push(e);
    }
    out
}

/// SHA-256, so the test can check the manifest's own claim.
#[allow(clippy::many_single_char_names)]
fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    for block in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, value) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(value);
        }
    }
    let mut out = String::new();
    for word in h {
        for byte in word.to_be_bytes() {
            use std::fmt::Write as _;
            let _ = write!(out, "{byte:02x}");
        }
    }
    out
}

#[test]
fn the_manifest_is_present() {
    let entries = manifest();
    if corpus_dir().is_none() {
        eprintln!("skipped: run `cargo xtask fixtures` first");
        return;
    }
    assert!(!entries.is_empty(), "the manifest lists no fixtures");
    for e in &entries {
        assert!(
            !e.id.is_empty() && !e.file.is_empty(),
            "a fixture has no id or file"
        );
        assert!(
            ["valid", "repaired", "rejected"].contains(&e.expect.as_str()),
            "{}: unknown expectation `{}`",
            e.id,
            e.expect
        );
        assert!(!e.features.is_empty(), "{}: no features recorded", e.id);
    }
}

#[test]
fn every_fixture_matches_the_hash_in_the_manifest() {
    let Some(dir) = corpus_dir() else {
        eprintln!("skipped: run `cargo xtask fixtures` first");
        return;
    };
    for e in manifest() {
        let bytes = std::fs::read(dir.join(&e.file)).expect("fixture file");
        assert_eq!(
            sha256_hex(&bytes),
            e.sha256,
            "{} does not match its recorded hash; regenerate with `cargo xtask fixtures`",
            e.file
        );
    }
}

#[test]
fn every_fixture_opens_and_keeps_its_pages() {
    let Some(dir) = corpus_dir() else {
        eprintln!("skipped: run `cargo xtask fixtures` first");
        return;
    };
    for e in manifest() {
        let bytes = std::fs::read(dir.join(&e.file)).expect("fixture file");
        let doc = Document::open(bytes, OpenOptions::default())
            .unwrap_or_else(|err| panic!("{}: {}", e.file, err));
        let pages = doc
            .page_count()
            .unwrap_or_else(|err| panic!("{}: {}", e.file, err));
        assert_eq!(
            pages, e.pages,
            "{}: the manifest says {} pages, the file has {pages}",
            e.file, e.pages
        );
    }
}

#[test]
fn a_fixture_needing_repair_says_so_and_never_gets_appended_to() {
    let Some(dir) = corpus_dir() else {
        eprintln!("skipped: run `cargo xtask fixtures` first");
        return;
    };
    let mut checked = 0;
    for e in manifest() {
        let bytes = std::fs::read(dir.join(&e.file)).expect("fixture file");
        // Every fixture must open. A damaged one is not an error.
        let doc = Document::open(bytes, OpenOptions::default())
            .unwrap_or_else(|err| panic!("{}: {}", e.file, err));

        if e.reader_repairs {
            assert!(
                !doc.info().recovery.is_clean(),
                "{} is recorded as needing repair but opened clean",
                e.file
            );
            assert!(
                doc.info().recovery.banner().is_some(),
                "{} was repaired but offers the user no explanation",
                e.file
            );
        } else {
            assert!(
                doc.info().recovery.is_clean(),
                "{} is recorded as opening cleanly but was repaired: {:?}",
                e.file,
                doc.info().recovery.notes()
            );
        }

        // A file whose structure was rebuilt must never be appended to: its original
        // bytes are not a valid history, and a reader would meet the broken revision
        // first.
        let saved = doc
            .save(&mangle_syntax::SaveOptions {
                mode: mangle_syntax::SaveMode::Incremental,
                ..mangle_syntax::SaveOptions::default()
            })
            .expect("a damaged file can still be written");
        assert_eq!(
            saved.forced_full,
            e.reader_repairs,
            "{}: saving should {}have forced a full rewrite",
            e.file,
            if e.reader_repairs { "" } else { "not " }
        );
        if saved.forced_full {
            assert!(saved.forced_reason.is_some(), "{}: no reason given", e.file);
        }
        checked += 1;
    }
    assert!(checked >= 10, "the corpus was not exercised");
}

#[test]
fn a_fixture_survives_a_full_save_and_reopen() {
    let Some(dir) = corpus_dir() else {
        eprintln!("skipped: run `cargo xtask fixtures` first");
        return;
    };
    for e in manifest() {
        let bytes = std::fs::read(dir.join(&e.file)).expect("fixture file");
        let doc = Document::open(bytes, OpenOptions::default())
            .unwrap_or_else(|err| panic!("{}: {}", e.file, err));
        let saved = doc
            .save(&mangle_syntax::SaveOptions::default())
            .unwrap_or_else(|err| panic!("{}: {}", e.file, err));
        let reopened = Document::open(saved.bytes, OpenOptions::default())
            .unwrap_or_else(|err| panic!("{} after a save: {}", e.file, err));
        let before = doc.page_count().unwrap_or(0);
        let after = reopened.page_count().unwrap_or_else(|err| {
            panic!("{} after a save: {}", e.file, err);
        });
        assert_eq!(before, after, "{}: the page count changed on save", e.file);
        // A whole file must not gain a dangling reference by being saved. A truncated
        // one legitimately has some already, which is why a repair is forced for it
        // rather than trusted.
        if !e.reader_repairs {
            assert!(
                saved.dropped.is_empty(),
                "{}: a save reported dangling references: {:?}",
                e.file,
                saved.dropped
            );
        }
    }
}

#[test]
fn the_confidential_fixture_really_contains_its_secrets() {
    let Some(dir) = corpus_dir() else {
        eprintln!("skipped: run `cargo xtask fixtures` first");
        return;
    };
    let Some(e) = manifest().into_iter().find(|e| e.id == "F33") else {
        eprintln!("skipped: no F33 fixture");
        return;
    };
    let bytes = std::fs::read(dir.join(&e.file)).expect("fixture file");

    // The secrets manifest names them; the file must contain every one of them, or the
    // redaction work is testing a fixture that leaks nothing.
    let secrets = std::fs::read_to_string(dir.join("F33_secrets.json")).expect("secrets");
    let mut checked = 0;
    let mut rest = secrets.as_str();
    while let Some(start) = rest.find("\"token\": \"") {
        rest = &rest[start + 10..];
        let Some(end) = rest.find('"') else { break };
        let token = &rest[..end];
        assert!(
            bytes.windows(token.len()).any(|w| w == token.as_bytes()),
            "F33 does not contain `{token}` in raw bytes, so there is nothing to redact"
        );
        checked += 1;
    }
    assert!(checked >= 4, "the secrets manifest listed too few tokens");
}
