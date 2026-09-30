//! RC4, used by the R2 and R3 standard security handlers.

// Direct indexing is used throughout this file: every index is either masked to a
// table width or produced by a loop bounded by the length of the same buffer, so a
// checked access would add noise without adding safety. The surrounding code is
// still panic-free: see docs/PDF-QUIRKS.md for the callers' tolerance rules.
#![allow(clippy::indexing_slicing)]

/// The RC4 stream cipher. Twenty lines, and no maintained crate should be trusted with
/// PDF's use of it.
#[derive(Debug, Clone)]
pub struct Rc4 {
    s: [u8; 256],
    i: u8,
    j: u8,
}

impl Rc4 {
    #[must_use]
    pub fn new(key: &[u8]) -> Self {
        let mut s = [0u8; 256];
        for (i, slot) in s.iter_mut().enumerate() {
            *slot = u8::try_from(i).unwrap_or(0);
        }
        if !key.is_empty() {
            let mut j: u8 = 0;
            for i in 0..256usize {
                j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
                s.swap(i, usize::from(j));
            }
        }
        Self { s, i: 0, j: 0 }
    }

    /// Encrypt or decrypt: RC4 is its own inverse.
    pub fn apply(&mut self, data: &mut [u8]) {
        for byte in data.iter_mut() {
            self.i = self.i.wrapping_add(1);
            self.j = self.j.wrapping_add(self.s[usize::from(self.i)]);
            self.s.swap(usize::from(self.i), usize::from(self.j));
            let idx =
                usize::from(self.s[usize::from(self.i)]) + usize::from(self.s[usize::from(self.j)]);
            let k = self.s.get(idx & 0xff).copied().unwrap_or(0);
            *byte ^= k;
        }
    }
}

/// One-shot convenience: encrypt or decrypt `data` in one call.
#[must_use]
pub fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut out = data.to_vec();
    Rc4::new(key).apply(&mut out);
    out
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect`, which is what a test is for; the
    // panic-free rule is about what the product does with a file, not about tests.
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::*;

    /// Hex-encode, upper case, for the test vectors below.
    fn hex(d: &[u8]) -> String {
        use std::fmt::Write as _;
        let mut s = String::with_capacity(d.len() * 2);
        for b in d {
            let _ = write!(s, "{b:02X}");
        }
        s
    }

    #[test]
    fn rfc6229_key_key() {
        // The classic test vector: key "Key", plaintext "Plaintext" -> "BBF316E8D940AF0AD3".
        let out = rc4(b"Key", b"Plaintext");
        assert_eq!(hex(&out), "BBF316E8D940AF0AD3");
    }

    #[test]
    fn rfc6229_wiki_pea() {
        let out = rc4(b"Wiki", b"pedia");
        assert_eq!(hex(&out), "1021BF0420");
    }

    #[test]
    fn secret() {
        let out = rc4(b"Secret", b"Attack at dawn");
        assert_eq!(hex(&out), "45A01F645FC35B383552544B9BF5");
    }

    #[test]
    fn round_trip() {
        let key = b"a moderately long file encryption key";
        let data: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        let enc = rc4(key, &data);
        let dec = rc4(key, &enc);
        assert_eq!(dec, data);
    }

    #[test]
    fn empty_key_still_round_trips() {
        let data = b"unchanged".to_vec();
        assert_eq!(rc4(b"", &rc4(b"", &data)), data);
    }
}
