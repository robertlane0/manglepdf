//! ASCIIHexDecode and ASCII85Decode (PDF 1.7, 7.4.2 and 7.4.3).

use crate::error::FilterError;
use crate::Partial;

const fn hexval(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// `ASCIIHexDecode`: hex pairs, terminated by `>`. White space is ignored. An odd
/// trailing digit is padded with zero, as the specification requires.
#[must_use]
pub fn ascii_hex_decode(input: &[u8]) -> Partial {
    let mut out = Vec::with_capacity(input.len() / 2 + 1);
    let mut high: Option<u8> = None;
    let mut seen_terminator = false;
    let mut stopped_on_junk = false;
    for &b in input {
        if b == b'>' {
            seen_terminator = true;
            break;
        }
        if b.is_ascii_whitespace() {
            continue;
        }
        match hexval(b) {
            Some(v) => {
                if let Some(h) = high.take() {
                    out.push((h << 4) | v);
                } else {
                    high = Some(v);
                }
            }
            None => {
                // `EOD`-style markers and stray characters end the data.
                stopped_on_junk = true;
                break;
            }
        }
    }
    let complete = seen_terminator && high.is_none() && !stopped_on_junk;
    let mut note = None;
    if stopped_on_junk {
        note = Some("invalid hex digit".into());
    } else if !seen_terminator {
        note = Some("missing `>` terminator".into());
    } else if high.is_some() {
        note = Some("odd number of hex digits; last digit padded with 0".into());
        if let Some(h) = high {
            out.push(h << 4);
        }
    }
    Partial {
        data: out,
        complete,
        note,
    }
}

/// `ASCII85Decode`: base-85 with the PDF `z` shorthand and `~>` terminator.
#[must_use]
pub fn ascii85_decode(input: &[u8]) -> Partial {
    let mut out = Vec::with_capacity(input.len() * 4 / 5);
    let mut group = [0u8; 5];
    let mut n = 0usize;
    let mut seen_terminator = false;
    let mut note: Option<String> = None;
    let mut i = 0usize;
    let bytes = input;

    // Skip leading white space.
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if bytes.get(i) == Some(&b'<') {
        // A `<~` marker is tolerated but not required.
        i += 1;
    }

    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'~' => {
                seen_terminator = true;
                break;
            }
            b'z' if n == 0 => {
                out.extend_from_slice(&[0, 0, 0, 0]);
                i += 1;
            }
            b'!'..=b'u' => {
                group[n] = b - b'!';
                n += 1;
                if n == 5 {
                    let mut v = 0u32;
                    for g in group.iter() {
                        v = v.wrapping_mul(85).wrapping_add(u32::from(*g));
                    }
                    out.extend_from_slice(&v.to_be_bytes());
                    n = 0;
                }
                i += 1;
            }
            b' ' | b'\n' | b'\r' | b'\t' | b'\x0c' | 0 => {
                i += 1;
            }
            _ => {
                note = Some(format!("invalid ASCII85 byte 0x{b:02x} at offset {i}"));
                break;
            }
        }
    }

    let complete = seen_terminator;
    if n == 0 {
        // Clean terminator.
    } else if (1..=4).contains(&n) {
        // Final partial group: pad with 'u' and emit only the significant bytes.
        let keep = n - 1;
        for slot in group.iter_mut().skip(n) {
            *slot = 84;
        }
        let mut v = 0u32;
        for g in group.iter() {
            v = v.wrapping_mul(85).wrapping_add(u32::from(*g));
        }
        let full = v.to_be_bytes();
        out.extend_from_slice(&full[..keep]);
        if note.is_none() {
            note = Some("missing `~>` terminator; final group padded".into());
        }
    } else if note.is_none() {
        note = Some("missing `~>` terminator".into());
    }

    Partial {
        data: out,
        complete,
        note,
    }
}

/// `ASCII85Decode` strict wrapper for tests and round trips.
#[allow(dead_code)]
pub fn ascii85_decode_strict(input: &[u8]) -> Result<Vec<u8>, FilterError> {
    let p = ascii85_decode(input);
    match p.note {
        Some(_) => Err(FilterError::Damaged {
            filter: "ASCII85Decode",
            reason: p.note.unwrap_or_default(),
        }),
        None => Ok(p.data),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_basic() {
        let p = ascii_hex_decode(b"48656C6C6F>");
        assert!(p.complete);
        assert_eq!(p.data, b"Hello");
    }

    #[test]
    fn hex_ignores_whitespace_and_pads() {
        let p = ascii_hex_decode(b"48 65\n6C\r6C6F>");
        assert!(p.complete);
        assert_eq!(p.data, b"Hello");
        let p = ascii_hex_decode(b"4>"); // single digit
        assert_eq!(p.data, b"\x40");
    }

    #[test]
    fn hex_unterminated_is_partial() {
        let p = ascii_hex_decode(b"4865");
        assert!(!p.complete);
        assert_eq!(p.data, b"He");
    }

    #[test]
    fn hex_rejects_junk() {
        let p = ascii_hex_decode(b"48zz6C>");
        assert!(!p.complete);
        assert_eq!(p.data, b"H");
    }

    #[test]
    fn a85_round_trip() {
        // "Man " -> common test vector
        let p = ascii85_decode(b"9jqo^~>");
        assert!(p.complete, "{p:?}");
        assert_eq!(p.data, b"Man ");
    }

    #[test]
    fn a85_z_and_partial() {
        let p = ascii85_decode(b"z~>");
        assert!(p.complete);
        assert_eq!(p.data, [0, 0, 0, 0]);
        let p = ascii85_decode(b"87cURD]j7BEbo80~>");
        assert!(p.complete);
        assert_eq!(p.data, b"Hello world!");
    }

    #[test]
    fn a85_missing_terminator() {
        let p = ascii85_decode(b"9jqo");
        assert!(!p.complete);
        assert_eq!(p.data, b"Man");
    }
}
