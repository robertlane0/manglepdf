//! Tier B — the wild corpus: fetching it and proving it is the corpus we pinned.
//!
//! The PDFs are not in the repository. `corpus/wild/MANIFEST.toml` records where each one
//! comes from, what licence it is under and what its SHA-256 is, and this module is the
//! only thing that puts bytes into `corpus/wild/`.
//!
//! The hash is the point. A corpus whose contents are not pinned is a test that changes
//! under you: upstream moves a file, the fetcher silently fetches the new one, and a
//! failure recorded last month cannot be reproduced today. So a file whose bytes do not
//! match the manifest is *refused*, not overwritten, and the name is printed.
//!
//! HTTP is `curl`, not a crate. This is build tooling whose whole job is to run other
//! programs — `G0.4` says so explicitly — and adding a TLS stack to the build for one
//! command is not worth the dependency.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::workspace::repo_root;

/// One `[[corpus]]` block, read with the small amount of TOML this manifest uses.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    pub id: String,
    pub file: String,
    pub url: String,
    pub licence: String,
    pub purpose: String,
    pub bytes: u64,
    pub expect: String,
    pub sha256: String,
}

/// `cargo xtask corpus <fetch|check|list> [--only ID,ID]`
pub(crate) fn cmd(args: &[String]) -> Result<(), String> {
    let action = args.first().map_or("check", String::as_str);
    let rest = args.get(1..).unwrap_or_default();

    let mut only: Vec<String> = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        match rest.get(i).map_or("", String::as_str) {
            "--only" => {
                i += 1;
                only = args_of(rest.get(i).ok_or("--only needs a list")?);
            }
            other => return Err(format!("unknown corpus option `{other}`")),
        }
        i += 1;
    }

    let root = repo_root()?;
    let dir = root.join("corpus/wild");
    let entries = manifest(&dir)?;
    if entries.is_empty() {
        return Err(format!(
            "{} lists no entries",
            dir.join("MANIFEST.toml").display()
        ));
    }
    let selected: Vec<&Entry> = entries
        .iter()
        .filter(|e| only.is_empty() || only.contains(&e.id))
        .collect();

    match action {
        "fetch" => fetch(&dir, &selected),
        "check" => check(&dir, &selected),
        "list" => {
            for e in &selected {
                println!("{:<5} {:>9}  {:<12} {}", e.id, e.bytes, e.expect, e.file);
            }
            Ok(())
        }
        other => Err(format!(
            "unknown corpus action `{other}`; expected fetch, check or list"
        )),
    }
}

fn args_of(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn manifest(dir: &Path) -> Result<Vec<Entry>, String> {
    let path = dir.join("MANIFEST.toml");
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut out: Vec<Entry> = Vec::new();
    let mut current: Option<Entry> = None;
    for line in raw.lines() {
        let line = line.trim();
        if line == "[[corpus]]" {
            if let Some(e) = current.take() {
                out.push(e);
            }
            current = Some(Entry {
                id: String::new(),
                file: String::new(),
                url: String::new(),
                licence: String::new(),
                purpose: String::new(),
                bytes: 0,
                expect: String::new(),
                sha256: String::new(),
            });
            continue;
        }
        let Some(e) = current.as_mut() else { continue };
        let Some((key, value)) = line.split_once(" = ") else {
            continue;
        };
        let text = unquote(value);
        match key {
            "id" => e.id = text,
            "file" => e.file = text,
            "url" => e.url = text,
            "licence" => e.licence = text,
            "purpose" => e.purpose = text,
            "bytes" => e.bytes = value.trim().parse().unwrap_or(0),
            "expect" => e.expect = text,
            "sha256" => e.sha256 = text,
            _ => {}
        }
    }
    if let Some(e) = current.take() {
        out.push(e);
    }
    Ok(out)
}

fn unquote(v: &str) -> String {
    v.trim()
        .trim_matches('"')
        .replace("\\\"", "\"")
        .replace("\\\\", "\\")
}

/// Verify what is already on disk. Needs no network, so it is the one to run in CI.
fn check(dir: &Path, entries: &[&Entry]) -> Result<(), String> {
    let mut present = 0usize;
    let mut missing = Vec::new();
    let mut wrong = Vec::new();
    for e in entries {
        let path = dir.join(&e.file);
        let Ok(bytes) = std::fs::read(&path) else {
            missing.push(e.id.clone());
            continue;
        };
        let got = sha256_hex(&bytes);
        if got == e.sha256 {
            present += 1;
        } else {
            wrong.push(format!("{}: expected {}, got {got}", e.file, e.sha256));
        }
    }
    for w in &wrong {
        println!("hash    {w}");
    }
    println!(
        "checked {}: {present} match, {} missing, {} wrong",
        entries.len(),
        missing.len(),
        wrong.len()
    );
    if !missing.is_empty() {
        println!(
            "missing {} — run `cargo xtask corpus fetch`{}",
            missing.len(),
            if missing.len() == entries.len() {
                ""
            } else {
                " (use --only for the rest)"
            }
        );
    }
    if wrong.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} file(s) do not match their pinned hash; refusing to trust them",
            wrong.len()
        ))
    }
}

/// Download anything not already present and correct, then verify every byte.
fn fetch(dir: &Path, entries: &[&Entry]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut fetched = 0usize;
    let mut kept = 0usize;
    let mut failed: Vec<String> = Vec::new();

    for e in entries {
        let path = dir.join(&e.file);
        if let Ok(bytes) = std::fs::read(&path) {
            if sha256_hex(&bytes) == e.sha256 {
                kept += 1;
                continue;
            }
            println!(
                "stale   {}: on disk but not the pinned bytes; refetching",
                e.file
            );
        }
        match download(e) {
            Ok(()) => {
                fetched += 1;
                println!("ok      {}  {:>9}  {}", e.id, e.bytes, e.file);
            }
            Err(why) => failed.push(format!("{} ({}): {why}", e.id, e.file)),
        }
    }

    for f in &failed {
        println!("failed  {f}");
    }
    println!(
        "fetched {fetched}, already correct {kept}, failed {}",
        failed.len()
    );

    // The verification runs whatever happened above. A fetch that reported success and a
    // check that disagrees is worse than either alone, so the exit status comes from here.
    check(dir, entries)?;
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} file(s) could not be fetched; the corpus is incomplete",
            failed.len()
        ))
    }
}

fn download(e: &Entry) -> Result<(), String> {
    let path = dir_path(e);
    let out = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--retry",
            "2",
            "--max-time",
            "180",
            "--output",
            &path.to_string_lossy(),
            &e.url,
        ])
        .output()
        .map_err(|err| format!("running curl: {err}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "curl failed ({}) {}{}",
            out.status,
            e.url,
            stderr.trim()
        ));
    }
    let bytes = std::fs::read(&path).map_err(|err| format!("{}: {err}", path.display()))?;
    if bytes.len() as u64 != e.bytes {
        return Err(format!(
            "downloaded {} bytes, the manifest says {}",
            bytes.len(),
            e.bytes
        ));
    }
    let got = sha256_hex(&bytes);
    if got != e.sha256 {
        // Left on disk on purpose: a human should be able to look at what arrived before
        // it is deleted, and "the upstream file changed" is a finding, not a nuisance.
        return Err(format!(
            "hash mismatch — the manifest pins {}, this is {got}. Upstream may have \
             changed the file; the downloaded copy has been left in place to look at",
            e.sha256
        ));
    }
    Ok(())
}

fn dir_path(e: &Entry) -> PathBuf {
    repo_root().map_or_else(
        |_| PathBuf::from(&e.file),
        |root| root.join("corpus/wild").join(&e.file),
    )
}

/// SHA-256, written out rather than depended on.
///
/// `crates/mangle-syntax/tests/corpus.rs` does the same thing for the Tier-A manifest. The
/// alternative is a crypto crate in the build graph for the benefit of two commands, and the
/// charter asks for few dependencies. `mangle-crypto` already has one, but it is product code
/// and this is build tooling, so pulling it in here would be worse than the duplication.
///
/// Indexing is allowed on one line and the line is the whole point: a SHA-256 message
/// schedule indexes a 64-byte block into a 64-word array, and both shapes are fixed by the
/// standard, so the bounds are the constant and not the data.
#[allow(clippy::many_single_char_names, clippy::indexing_slicing)]
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
            // FIPS 180-4 spells T2 as Σ0(a) + Maj(a,b,c) and nothing else: W[i] and e
            // belong to T1, which already carries them. Adding them here too looks
            // reasonable and is wrong.
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
    let mut out = String::with_capacity(64);
    for word in h {
        for byte in word.to_be_bytes() {
            use std::fmt::Write as _;
            let _ = write!(out, "{byte:02x}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::sha256_hex;

    #[test]
    fn sha256_matches_the_published_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn sha256_spans_a_block_boundary() {
        // 56, 57 and 64 bytes are the lengths where the padding rules change.
        for len in [55usize, 56, 63, 64, 65, 119, 120] {
            let data = vec![b'a'; len];
            assert_eq!(sha256_hex(&data).len(), 64, "{len} bytes");
        }
        assert_eq!(
            sha256_hex(&vec![b'a'; 1000]),
            "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3"
        );
    }
}
