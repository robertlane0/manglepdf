//! PDF stream filters and codecs.
//!
//! Everything here operates on bytes and is deliberately independent of the PDF object
//! model (see `mangle-syntax`, which interprets `/Filter` chains).
//!
//! Design rules:
//! * every decoder is *total* on hostile input — it returns partial output plus a
//!   diagnostic instead of failing, because a truncated stream must still show the
//!   content it does contain;
//! * every decoder is bounded (explicit output caps, ratio and absolute bomb guards);
//! * no `unsafe`.

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

mod ascii;
mod ccitt;
mod deflate;
mod error;
mod inflate;
mod lzw;
mod predict;
mod runlength;

pub use deflate::{DeflateLevel, deflate};
pub use error::{FilterError, FilterResult};
pub use inflate::{InflateOutcome, inflate, inflate_raw};

pub use ascii::{ascii_hex_decode, ascii85_decode};
pub use ccitt::{CcittParams, Variant as CcittVariant, ccitt_decode};
pub use lzw::{EarlyChange, lzw_decode, lzw_encode};
pub use predict::{PredictorParams, predict, unpredict};
pub use runlength::{run_length_decode, run_length_encode};

/// Result of a decode that is allowed to produce partial output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partial {
    /// The bytes decoded so far (always a prefix of what a healthy stream would give).
    pub data: Vec<u8>,
    /// `true` when the stream ended exactly as specified.
    pub complete: bool,
    /// Human-readable reason the stream stopped early, if it did.
    pub note: Option<String>,
}

impl Partial {
    /// Bytes only, discarding the diagnostic.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.data
    }
}

/// Absolute cap on any single decoded stream, independent of the compressed size.
///
/// Generous enough for a 20000x20000 1-bit image rendered in strips, small enough that a
/// declared `/Length` of 4 GB cannot exhaust memory.
pub const MAX_DECODED_BYTES: usize = 1 << 30;

/// How much a stream may expand before it is treated as a decompression bomb.
///
/// A deflate match returns at most 258 bytes for a few bits of code, so no valid
/// stream can exceed roughly 1032:1. Sitting above that means this ceiling can only
/// ever stop a crafted or corrupt stream, never a real one.
pub const MAX_EXPANSION_RATIO: usize = 1200;

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect`, which is what a test is for; the
    // panic-free rule is about what the product does with a file, not about tests.
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::*;

    #[test]
    fn round_trip_deflate() {
        let cases: Vec<Vec<u8>> = vec![
            Vec::new(),
            b"a".to_vec(),
            b"hello hello hello hello".to_vec(),
            (0..70_000u32).map(|i| (i % 251) as u8).collect(),
            {
                // Highly repetitive: exercises the match finder.
                let mut v = Vec::new();
                for _ in 0..2_000 {
                    v.extend_from_slice(b"the quick brown fox jumps over the lazy dog. ");
                }
                v
            },
        ];
        for case in cases {
            for level in [
                DeflateLevel::Fast,
                DeflateLevel::Default,
                DeflateLevel::Best,
            ] {
                let packed = deflate(&case, level);
                let back = inflate(&packed, case.len());
                assert!(back.complete, "level {level:?} did not complete");
                assert_eq!(back.data, case, "level {level:?} round trip mismatch");
            }
        }
    }

    #[test]
    fn truncated_deflate_returns_prefix() {
        let data: Vec<u8> = (0..50_000u32).map(|i| (i % 97) as u8).collect();
        let packed = deflate(&data, DeflateLevel::Default);
        let cut = packed.get(..packed.len() / 2).unwrap_or(&[]);
        let back = inflate(cut, data.len());
        assert!(!back.complete);
        assert!(!back.data.is_empty());
        // Everything produced must be a genuine prefix of the original.
        assert!(data.starts_with(&back.data));
    }

    #[test]
    fn garbage_is_partial_not_panic() {
        for seed in 0..64u32 {
            let mut v = Vec::new();
            let mut x = seed.wrapping_mul(2_654_435_761);
            for _ in 0..256 {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                v.push((x >> 16) as u8);
            }
            let _out = inflate(&v, 1 << 20);
        }
    }
}
