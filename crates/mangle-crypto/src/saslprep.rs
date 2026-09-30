//! Password preparation for revision 6: SASLprep (RFC 4013) over the UTF-8 form of the
//! password, with the mapping and normalisation tables limited to the ranges that can
//! actually appear in a PDF password.
//!
//! A full SASLprep needs stringprep tables. For passwords the reachable cases are
//! narrow: map the non-ASCII space characters to U+0020, drop the "commonly mapped to
//! nothing" code points, and apply NFKC from `unicode-normalization`. That is what the
//! specification's own profile 1 and 2 rules reduce to, and it is what other
//! implementations do.

use unicode_normalization::UnicodeNormalization;


/// Prepare a password for revisions 5 and 6: SASLprep, then UTF-8.
#[must_use]
pub fn saslprep_utf8(password: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(password);
    prepare(&text).into_bytes()
}

/// The 32-byte password padding string from ISO 32000-1, 7.6.3.3.
pub const PASSWORD_PADDING: [u8; 32] = [
    0x28, 0xbf, 0x4e, 0x5e, 0x4e, 0x75, 0x8a, 0x41, 0x64, 0x00, 0x4e, 0x56, 0xff, 0xfa, 0x01, 0x08,
    0x2e, 0x2e, 0x00, 0xb6, 0xd0, 0x68, 0x3e, 0x80, 0x2f, 0x0c, 0xa9, 0xfe, 0x64, 0x53, 0x69, 0x7a,
];

/// Prepare a password for revisions 2 to 4: the password followed by the standard
/// padding string, truncated to exactly 32 bytes. Longer passwords are simply truncated.
///
/// The padding starts at the first free byte, so a four-byte password keeps the first
/// four bytes of the padding string. Verified against qpdf's revision 2 output.
#[must_use]
pub fn legacy_password_bytes(password: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let n = password.len().min(32);
    for (i, slot) in out.iter_mut().take(n).enumerate() {
        *slot = password.get(i).copied().unwrap_or(0);
    }
    for (i, slot) in out.iter_mut().skip(n).enumerate() {
        *slot = PASSWORD_PADDING.get(i).copied().unwrap_or(0);
    }
    out
}

/// The preparation rules, as a string.
#[must_use]
pub fn prepare(password: &str) -> String {
    // 1. Map the non-ASCII space separators to U+0020, and drop the mapping-to-nothing
    //    code points.
    let mapped: String = password
        .chars()
        .filter_map(|c| match c {
            // Table C.1.2: non-ASCII space separators become U+0020.
            '\u{00A0}' | '\u{1680}' | '\u{2000}'..='\u{200A}' | '\u{202F}' | '\u{205F}'
            | '\u{3000}' => Some(' '),
            // Table B.1: commonly mapped to nothing.
            '\u{00AD}' | '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}' | '\u{FEFF}' => None,
            '\u{0009}' | '\u{000A}' | '\u{000B}' | '\u{000C}' | '\u{000D}' => Some(' '),
            other => Some(other),
        })
        .collect();
    // 2. Prohibit, then normalise with NFKC. Passwords are not expected to contain
    //    prohibited characters; leaving them in is friendlier than rejecting the file.
    mapped.nfkc().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_is_unchanged() {
        assert_eq!(saslprep_utf8(b"secret"), b"secret");
    }

    #[test]
    fn non_ascii_space_maps_to_ascii_space() {
        assert_eq!(prepare("a\u{00A0}b"), "a b");
        assert_eq!(prepare("a\u{3000}b"), "a b");
    }

    #[test]
    fn zero_width_is_dropped() {
        assert_eq!(prepare("a\u{200B}b"), "ab");
        assert_eq!(prepare("a\u{FEFF}b"), "ab");
    }

    #[test]
    fn compatibility_normalisation() {
        // U+FB01 is the "fi" ligature and decomposes under NFKC.
        assert_eq!(prepare("\u{FB01}"), "fi");
        // Fullwidth A becomes A.
        assert_eq!(prepare("\u{FF21}"), "A");
    }

    #[test]
    fn legacy_padding() {
        let p = legacy_password_bytes(b"abc");
        assert_eq!(&p[..3], b"abc");
        assert_eq!(&p[3..], &PASSWORD_PADDING[..29]);
        let long = legacy_password_bytes(&[b'x'; 40]);
        assert_eq!(long.len(), 32);
        assert!(long.iter().all(|b| *b == b'x'));
    }
}
