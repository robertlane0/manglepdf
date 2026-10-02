//! Interoperability: a stream we write must be a stream another program can read.
//!
//! `/FlateDecode` names zlib (RFC 1950), not the bare deflate (RFC 1951) that sits inside
//! it. Writing the bare form produced a file our own reader accepted and no other program
//! could open, and no round trip in this repository could see it, because every one of them
//! inflated our output with our own decoder.
//!
//! Two kinds of check live here, deliberately:
//!
//! * byte-level assertions against streams another implementation *wrote*, which run
//!   everywhere and need nothing installed — they are the ones that must hold on a machine
//!   with no Python;
//! * a round trip through Python's `zlib`, which is the check that would actually have
//!   caught the bug, and which skips itself when `python3` is not present.
//!
//! Reading a file another program wrote is the mirror image of the bug and is checked the
//! same way, because a codec that cannot read its own format is not a codec.

#![forbid(unsafe_code)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::process::Command;

use mangle_filters::{DeflateLevel, deflate, deflate_raw, inflate};

/// A directory for the files handed to Python, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Option<Self> {
        let dir = std::env::temp_dir().join(format!("manglepdf-interop-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).ok()?;
        Some(Self(dir))
    }

    fn write(&self, name: &[u8]) -> PathBuf {
        let path = self.0.join("stream.zlib");
        std::fs::write(&path, name).expect("scratch file");
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

/// The payloads the tests round trip: nothing, one byte, text, every byte value, and
/// something long enough to need several deflate blocks.
fn payloads() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("empty", Vec::new()),
        ("one byte", b"a".to_vec()),
        (
            "text",
            b"Flate is a zlib stream, not a bare deflate stream.\n".repeat(40),
        ),
        ("every byte value", (0..=255u8).collect()),
        (
            "many blocks",
            (0..200_000u32).map(|i| (i % 251) as u8).collect(),
        ),
    ]
}

#[test]
fn a_stream_another_implementation_wrote_inflates() {
    // `zlib.compress(b"The quick brown fox jumps over the lazy dog. " * 20, 6)`. The
    // header here is 0x78 0x9c and the last four bytes are the Adler-32, which is what our
    // own reader now expects and did not expect before the wrapper was understood.
    let want = b"The quick brown fox jumps over the lazy dog. ".repeat(20);
    let packed: [u8; 61] = [
        0x78, 0x9c, 0x0b, 0xc9, 0x48, 0x55, 0x28, 0x2c, 0xcd, 0x4c, 0xce, 0x56, 0x48, 0x2a, 0xca,
        0x2f, 0xcf, 0x53, 0x48, 0xcb, 0xaf, 0x50, 0xc8, 0x2a, 0xcd, 0x2d, 0x28, 0x56, 0xc8, 0x2f,
        0x4b, 0x2d, 0x52, 0x28, 0x01, 0x4a, 0xe7, 0x24, 0x56, 0x55, 0x2a, 0xa4, 0xe4, 0xa7, 0xeb,
        0x29, 0x84, 0x8c, 0x2a, 0x1e, 0x55, 0x3c, 0xaa, 0x98, 0xda, 0x8a, 0x01, 0x47, 0xa5, 0x43,
        0x1c,
    ];
    let r = inflate(&packed, want.len());
    assert!(r.complete, "{r:?}");
    assert_eq!(r.data, want);
}

#[test]
fn a_stream_another_implementation_wrote_inflates_at_a_smaller_window() {
    // zlib lets a compressor declare a window smaller than 32 KiB, and the CMF/FLG pair
    // changes with it: here CINFO 1 declares a 512-byte window, and the check byte that
    // makes the pair a multiple of 31 follows it. A reader that recognised only the header
    // we write would fail here.
    let want = b"a smaller window is still a valid zlib stream.\n".repeat(20);
    // `zlib.compressobj(6, zlib.DEFLATED, 9)` — a 512-byte window, header 0x18 0x95.
    let packed: [u8; 64] = [
        0x18, 0x95, 0x4b, 0x54, 0x28, 0xce, 0x4d, 0xcc, 0xc9, 0x49, 0x2d, 0x52, 0x28, 0xcf, 0xcc,
        0x4b, 0xc9, 0x2f, 0x57, 0xc8, 0x2c, 0x56, 0x28, 0x2e, 0xc9, 0xcc, 0xc9, 0x51, 0x48, 0x54,
        0x28, 0x4b, 0xcc, 0xc9, 0x4c, 0x51, 0xa8, 0xca, 0xc9, 0x4c, 0x02, 0x0a, 0x15, 0xa5, 0x26,
        0xe6, 0xea, 0x71, 0x25, 0x8e, 0x2a, 0x1f, 0x55, 0x3e, 0xaa, 0x7c, 0x60, 0x94, 0x03, 0x00,
        0x52, 0xef, 0x50, 0x8c,
    ];
    let r = inflate(&packed, want.len());
    assert!(r.complete, "{r:?}");
    assert_eq!(r.data, want);
}

#[test]
fn python_agrees_that_what_we_write_is_zlib() {
    // The check that would have caught the bug: hand the bytes to an implementation that
    // is not us and see whether the original comes back. A bare deflate stream makes
    // `zlib.decompress` raise "unknown compression method" on the first header byte.
    if !have("python3") {
        eprintln!("skipped: python3 is not installed");
        return;
    }
    let Some(scratch) = Scratch::new("write") else {
        eprintln!("skipped: no scratch directory");
        return;
    };
    for (name, data) in payloads() {
        for level in [
            DeflateLevel::Fast,
            DeflateLevel::Default,
            DeflateLevel::Best,
        ] {
            let packed = deflate(&data, level);
            let path = scratch.write(&packed);
            let out = Command::new("python3")
                .arg("-c")
                .arg("import sys,zlib; sys.stdout.buffer.write(zlib.decompress(open(sys.argv[1],'rb').read()))")
                .arg(&path)
                .output()
                .expect("python3 runs");
            assert!(
                out.status.success(),
                "python3 rejected our {name} stream at {level:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(
                out.stdout, data,
                "python3 read our {name} stream at {level:?} as something else"
            );
        }
    }
}

#[test]
fn python_agrees_about_the_adler32_we_write() {
    // The trailing four bytes are a checksum of the *uncompressed* input, written
    // big-endian. Decompressing and recomputing the checksum in Python, then comparing it
    // with the integer the last four bytes encode, says the checksum covers the original
    // data and not, say, the deflate stream by accident.
    if !have("python3") {
        eprintln!("skipped: python3 is not installed");
        return;
    }
    let Some(scratch) = Scratch::new("adler") else {
        eprintln!("skipped: no scratch directory");
        return;
    };
    for (name, data) in payloads() {
        let packed = deflate(&data, DeflateLevel::Default);
        let path = scratch.write(&packed);
        let out = Command::new("python3")
            .arg("-c")
            .arg(
                "import sys,zlib; d=open(sys.argv[1],'rb').read(); \
                  print(int.from_bytes(d[-4:],'big') == zlib.adler32(zlib.decompress(d)))",
            )
            .arg(&path)
            .output()
            .expect("python3 runs");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "True",
            "the trailing four bytes of {name} are not its Adler-32"
        );
    }
}

/// A bare deflate stream is not a zlib stream, and Python says so. If this ever stops being
/// true the wrapper has become ambiguous; if it ever becomes true of our own output, the
/// other test is not testing what it claims to.
#[test]
fn python_rejects_the_raw_stream_that_flatedecode_does_not_name() {
    if !have("python3") {
        eprintln!("skipped: python3 is not installed");
        return;
    }
    let Some(scratch) = Scratch::new("raw") else {
        eprintln!("skipped: no scratch directory");
        return;
    };
    let data = b"the form /FlateDecode does not name.\n".repeat(20);
    let packed = deflate_raw(&data, DeflateLevel::Default);
    let path = scratch.write(&packed);
    let out = Command::new("python3")
        .arg("-c")
        .arg("import sys,zlib; zlib.decompress(open(sys.argv[1],'rb').read())")
        .arg(&path)
        .output()
        .expect("python3 runs");
    assert!(
        !out.status.success(),
        "a bare deflate stream was accepted as zlib, so the two are indistinguishable"
    );
    // And our own reader still takes it, because files in the wild carry one.
    let r = inflate(&packed, data.len());
    assert!(r.complete, "{r:?}");
    assert_eq!(r.data, data);
}
