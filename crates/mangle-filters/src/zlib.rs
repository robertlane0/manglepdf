//! The zlib wrapper (RFC 1950) that PDF's `/FlateDecode` actually names.
//!
//! `/FlateDecode` does not mean "raw deflate". It means a two-byte header, the RFC 1951
//! stream inside, and a four-byte Adler-32 of the *uncompressed* input. A writer that
//! omits the wrapper produces a stream its own reader accepts and no other program can,
//! which is the one failure a PDF writer must not have.
//!
//! The two halves of the codec share this file so the header constants and the checksum
//! have exactly one definition: `deflate` writes them, `inflate` recognises them.

/// Window size exponent. `CINFO` is the base-two logarithm of the window minus eight, so
/// 7 declares 2^15 = 32 KiB — the largest window a PDF reader has to accept.
const CINFO: u8 = 7;

/// Compression method 8: what is inside the wrapper is DEFLATE.
const CM_DEFLATE: u8 = 8;

/// Compression level, 0 (fastest) to 3 (maximum). 2 means "the compressor chose", which
/// is the honest answer for us: our level is a chain length, not a zlib one.
const FLEVEL: u8 = 2;

/// The two header bytes of a zlib stream.
///
/// The first byte is `CMF = CINFO << 4 | CM`. The second is `FLG = FLEVEL << 6 | FDICT <<
/// 5 | FCHECK`, where `FCHECK` is the only part that has to be computed: it is chosen so
/// that the two bytes read as one big-endian 16-bit number are divisible by 31.
///
/// The 31 is fixed by RFC 1950 so a reader can test the whole header with one modulo and
/// no parsing. 31 is prime and 256 = 8·31 + 8, so shifting either byte by one moves the
/// value by a non-zero amount modulo 31: a byte-swapped, truncated or misaligned header
/// fails, while a correct one passes. It is a guard against losing track of the stream, not
/// a checksum of anything.
///
/// The arithmetic is on constants, so it cannot fail and needs no error type.
#[must_use]
pub(crate) fn header() -> [u8; 2] {
    let cmf = (CINFO << 4) | CM_DEFLATE;
    // FLEVEL is already in the byte, so FCHECK only has to cancel this much.
    let partial = u16::from(cmf) * 256 + (u16::from(FLEVEL) << 6);
    let fcheck = (31 - partial % 31) % 31;
    [cmf, (FLEVEL << 6) | fcheck as u8]
}

/// How many bytes of zlib header `input` begins with: two, or zero when it begins with the
/// bare deflate stream instead.
///
/// A damaged file and a hand-made stream can both be bare deflate, and a file we wrote
/// before this wrapper existed certainly is, so this recognises a header rather than
/// assuming one. The two tests are the ones zlib itself makes before it dispatches, and
/// the consequence is the same: a bare deflate stream passes them by accident about once in
/// eight thousand, which is the price of telling the two apart at all.
pub(crate) fn header_len(input: &[u8]) -> usize {
    match (input.first(), input.get(1)) {
        (Some(&cmf), Some(&flg))
            if cmf & 0x0f == CM_DEFLATE && (u16::from(cmf) * 256 + u16::from(flg)) % 31 == 0 =>
        {
            2
        }
        _ => 0,
    }
}

/// Adler-32 of `data` (RFC 1950): the checksum a zlib stream ends with, over the bytes
/// *before* compression.
///
/// Two sums modulo 65521, the largest prime below 2^16: `a` accumulates the bytes and `b`
/// accumulates `a`. `b` therefore counts each byte once for every byte after it, so the
/// pair changes when bytes are reordered as well as when they are dropped or replaced — a
/// plain running sum cannot tell those apart. The result is `b << 16 | a`, written out
/// big-endian.
pub(crate) fn adler32(data: &[u8]) -> u32 {
    /// The modulus. Prime, and close to 2^16, so the two halves barely wrap.
    const MODULUS: u32 = 65521;
    /// The most bytes that can be absorbed before `b` could overflow 32 bits. 255·5552
    /// plus the running total is just under 2^32, hence the chunking.
    const RUN: usize = 5552;
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(RUN) {
        for &byte in chunk {
            a += u32::from(byte);
            b += a;
        }
        a %= MODULUS;
        b %= MODULUS;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect`, which is what a test is for; the
    // panic-free rule is about what the product does with a file, not about tests.
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::*;

    /// Adler-32 written the slow way — reduce after every byte, no chunking — so the test
    /// states the definition rather than re-running the implementation's own shortcuts.
    fn reference_adler32(data: &[u8]) -> u32 {
        let mut a = 1u32;
        let mut b = 0u32;
        for &byte in data {
            a = (a + u32::from(byte)) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }

    #[test]
    fn header_is_the_pair_for_a_32_kib_window() {
        let [cmf, flg] = header();
        assert_eq!(cmf & 0x0f, CM_DEFLATE, "CM must name deflate");
        assert_eq!(cmf >> 4, 7, "CINFO 7 is the 32 KiB window");
        assert_eq!(
            (u16::from(cmf) * 256 + u16::from(flg)) % 31,
            0,
            "CMF*256 + FLG must be a multiple of 31"
        );
        // This is the pair every zlib implementation writes at its default level.
        assert_eq!([cmf, flg], [0x78, 0x9c]);
    }

    #[test]
    fn header_len_recognises_our_own_header_and_rejects_noise() {
        assert_eq!(header_len(&header()), 2);
        assert_eq!(header_len(&[0x78]), 0, "one byte cannot be a header");
        assert_eq!(header_len(&[]), 0);
        // Right method, wrong check byte.
        assert_eq!(header_len(&[0x78, 0x9d]), 0);
        // Valid check byte, wrong method.
        assert_eq!(header_len(&[0x79, 0x4b]), 0);
        assert_eq!(header_len(b"Q"), 0);
    }

    #[test]
    fn adler32_matches_the_definition() {
        for case in [
            Vec::new(),
            b"a".to_vec(),
            b"the quick brown fox jumps over the lazy dog. ".repeat(40),
            (0..300_000u32).map(|i| (i % 251) as u8).collect(),
        ] {
            assert_eq!(
                adler32(&case),
                reference_adler32(&case),
                "{} bytes",
                case.len()
            );
        }
    }

    #[test]
    fn adler32_of_nothing_is_one() {
        // The initial value of `a` is 1, and that is what a stream of no bytes checksums to.
        assert_eq!(adler32(b""), 1);
    }
}
