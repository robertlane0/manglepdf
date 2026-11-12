//! Shared test helpers for workspace unit and integration tests.
//!
//! Holds pure helpers used across multiple crates' test suites — fixture and corpus
//! path discovery, bundled test-font loading, hex formatting, PAM image parsing and
//! alpha flattening for `mutool` oracle comparisons, temporary scratch directories,
//! surgical byte-insertion stripping, and one-page PDF assembly — with zero dependencies
//! so every crate can depend on `mangle-testkit` in `[dev-dependencies]`.

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

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The repository root directory.
#[must_use]
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The Tier-A `fixtures/` directory, if `fixtures/MANIFEST.toml` is present.
#[must_use]
pub fn fixtures_dir() -> Option<PathBuf> {
    let dir = workspace_root().join("fixtures");
    dir.join("MANIFEST.toml").is_file().then_some(dir)
}

/// The Tier-B `corpus/wild/` directory, if `corpus/wild/MANIFEST.toml` is present.
#[must_use]
pub fn wild_dir() -> Option<PathBuf> {
    let dir = workspace_root().join("corpus/wild");
    dir.join("MANIFEST.toml").is_file().then_some(dir)
}

/// A named file inside `corpus/wild/`, if it has been fetched to disk.
#[must_use]
pub fn wild_file(name: &str) -> Option<PathBuf> {
    let path = wild_dir()?.join(name);
    path.is_file().then_some(path)
}

/// The bundled `LiberationSans-Regular.ttf` font bytes used by font and editing tests.
#[must_use]
pub fn liberation() -> Option<Vec<u8>> {
    std::fs::read(workspace_root().join("assets/fonts/LiberationSans-Regular.ttf")).ok()
}

/// Format a byte slice as lowercase hexadecimal.
#[must_use]
pub fn hex(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 2);
    for b in data {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Parse a `P7` PAM image (`WIDTH`, `HEIGHT`, `DEPTH`, `ENDHDR\n`) into `(width, height, depth, body)`.
#[must_use]
pub fn read_pam(data: &[u8]) -> Option<(usize, usize, usize, Vec<u8>)> {
    let marker = b"ENDHDR\n";
    let split = data.windows(marker.len()).position(|w| w == marker)?;
    let header = String::from_utf8_lossy(data.get(..split)?).to_string();
    if !header.starts_with("P7") {
        return None;
    }
    let field = |name: &str| -> Option<usize> {
        header
            .lines()
            .find_map(|l| l.trim().strip_prefix(name))
            .and_then(|v| v.trim().parse().ok())
    };
    let (w, h, depth) = (field("WIDTH")?, field("HEIGHT")?, field("DEPTH")?);
    let body = data.get(split + marker.len()..)?.to_vec();
    let needed = w.checked_mul(h)?.checked_mul(depth)?;
    if body.len() < needed {
        return None;
    }
    Some((w, h, depth, body))
}

/// Expand a PAM body of `(w, h, depth)` into row-major RGBA (`w * h * 4` bytes).
#[must_use]
pub fn pam_to_rgba(w: usize, h: usize, depth: usize, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(w.saturating_mul(h).saturating_mul(4));
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * depth;
            let at = |c: usize| body.get(i + c).copied().unwrap_or(255);
            out.extend_from_slice(&[at(0), at(1), at(2), if depth > 3 { at(3) } else { 255 }]);
        }
    }
    out
}

/// Composite straight-alpha RGBA pixels onto opaque white `[255, 255, 255, 255]`.
#[must_use]
pub fn flatten_rgba_onto_paper(pixels: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixels.len());
    for src in pixels.chunks_exact(4) {
        let [red, green, blue, alpha] = *src else {
            continue;
        };
        let opacity = f64::from(alpha) / 255.0;
        let over_c = |chan: u8| -> u8 {
            let sample = f64::from(chan) / 255.0;
            ((sample * opacity + (1.0 - opacity)) * 255.0).round() as u8
        };
        out.extend_from_slice(&[over_c(red), over_c(green), over_c(blue), 255]);
    }
    out
}

/// Remove each inserted byte sequence in order from `after`, returning the remaining bytes.
#[must_use]
pub fn with_insertions_removed(after: &[u8], inserted: &[Vec<u8>]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(after.len());
    let mut cursor = 0usize;
    for bytes in inserted {
        if bytes.is_empty() {
            continue;
        }
        let tail = after.get(cursor..)?;
        let rel = tail
            .windows(bytes.len())
            .position(|w| w == bytes.as_slice())?;
        let at = cursor + rel;
        out.extend_from_slice(after.get(cursor..at)?);
        cursor = at + bytes.len();
    }
    out.extend_from_slice(after.get(cursor..)?);
    Some(out)
}

/// A one-page PDF assembled from parts, with a cross-reference table that is correct.
///
/// The page is 100 by 100 points. `resources` is the body of its `/Resources` dictionary and
/// `content` its content stream. Each of `streams` is the dictionary entries and the data of one
/// more stream object, numbered from 5 in order so that a resource can name it as `5 0 R`;
/// `/Length` is added.
#[must_use]
pub fn single_page_pdf(resources: &str, content: &[u8], streams: &[(&str, &[u8])]) -> Vec<u8> {
    fn stream(entries: &str, data: &[u8]) -> Vec<u8> {
        let mut body = format!("<< {entries} /Length {} >>\nstream\n", data.len()).into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(b"\nendstream");
        body
    }

    let mut bodies: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R \
             /Resources << {resources} >> >>"
        )
        .into_bytes(),
        stream("", content),
    ];
    bodies.extend(streams.iter().map(|(entries, data)| stream(entries, data)));

    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::with_capacity(bodies.len());
    for (index, body) in bodies.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    let size = bodies.len() + 1;
    out.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    out
}

/// A temporary scratch directory that removes itself when dropped.
#[derive(Debug)]
pub struct Scratch(PathBuf);

impl Scratch {
    /// Create a fresh scratch directory under the system temp dir.
    #[must_use]
    pub fn new(name: &str) -> Option<Self> {
        let dir = std::env::temp_dir().join(format!("manglepdf-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).ok()?;
        Some(Self(dir))
    }

    /// Path to the scratch directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Write `bytes` to `name` inside the scratch directory and return its path.
    #[must_use]
    pub fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        let _ = std::fs::write(&path, bytes);
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn workspace_helpers_locate_fixtures_and_liberation() {
        assert!(fixtures_dir().is_some(), "fixtures/MANIFEST.toml exists");
        let font = liberation().expect("LiberationSans-Regular.ttf exists");
        assert!(font.len() > 10_000);
    }

    #[test]
    fn hex_formats_lowercase_bytes() {
        assert_eq!(hex(&[0x00, 0x0a, 0xff]), "000aff");
    }

    #[test]
    fn pam_parses_and_flattens_onto_white() {
        let pam = b"P7\nWIDTH 1\nHEIGHT 1\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n\x00\x00\x00\x00";
        let (w, h, d, body) = read_pam(pam).expect("valid PAM");
        assert_eq!((w, h, d), (1, 1, 4));
        let rgba = pam_to_rgba(w, h, d, &body);
        assert_eq!(rgba, vec![0, 0, 0, 0]);
        assert_eq!(flatten_rgba_onto_paper(&rgba), vec![255, 255, 255, 255]);
    }

    #[test]
    fn insertions_are_removed_in_order() {
        let after = b"AA q 1 0 0 1 5 5 cm BB Q CC";
        let stripped =
            with_insertions_removed(after, &[b" q 1 0 0 1 5 5 cm".to_vec(), b" Q".to_vec()])
                .expect("insertions found");
        assert_eq!(stripped, b"AA BB CC");
    }

    #[test]
    fn a_single_page_pdf_has_an_xref_that_points_at_each_object() {
        let pdf = single_page_pdf(
            "/XObject << /Im1 5 0 R >>",
            b"q /Im1 Do Q",
            &[("/Subtype /Image", b"abc")],
        );
        let text = String::from_utf8(pdf).expect("ascii throughout");
        assert!(text.starts_with("%PDF-1.7\n"));
        let start: usize = text
            .rsplit("startxref\n")
            .next()
            .and_then(|tail| tail.lines().next())
            .and_then(|line| line.parse().ok())
            .expect("a startxref offset");
        let table = text.get(start..).expect("the offset is inside the file");
        assert!(
            table.starts_with("xref\n0 6\n"),
            "five objects and the free entry"
        );
        // Entry 0 is the free list head; entries 1 to 5 are the objects, in order.
        for (number, entry) in table
            .lines()
            .skip(3)
            .take(5)
            .enumerate()
            .map(|(i, e)| (i + 1, e))
        {
            let offset: usize = entry
                .get(..10)
                .and_then(|d| d.parse().ok())
                .expect("an offset");
            let at = text.get(offset..).expect("the offset is inside the file");
            assert!(
                at.starts_with(&format!("{number} 0 obj\n")),
                "object {number} is at {offset}"
            );
        }
        assert!(text.contains("/Length 3 >>\nstream\nabc\nendstream"));
    }

    #[test]
    fn scratch_creates_and_cleans_up_directory() {
        let path = {
            let s = Scratch::new("testkit-unit").expect("scratch created");
            let file = s.write("hello.bin", b"abc");
            assert!(file.is_file());
            s.path().to_path_buf()
        };
        assert!(!path.exists());
    }
}
