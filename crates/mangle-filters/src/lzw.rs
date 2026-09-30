//! `LZWDecode` (ISO 32000-1 7.4.4) and the `EarlyChange` parameter that different
//! producers disagree about by exactly one code width.

use std::collections::BTreeMap;

use crate::Partial;

/// When the code width grows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EarlyChange {
    /// Grow when the next free code reaches `2^width - 1`. This is what ISO 32000
    /// describes and what PDF producers emit.
    Standard,
    /// Grow when it reaches `2^width`, one code later. Seen in the wild from some
    /// TIFF-flavoured producers.
    Late,
}

impl EarlyChange {
    /// How far *before* the encoder's threshold the decoder switches width. The
    /// decoder's table always trails the encoder's by one entry, so it has to switch
    /// one code early to stay aligned; `Late` omits that correction.
    const fn decoder_bias(self) -> u32 {
        match self {
            EarlyChange::Standard => 1,
            EarlyChange::Late => 0,
        }
    }
}

const CLEAR: u16 = 256;
const EOD: u16 = 257;
const FIRST_CODE: u16 = 258;
const MAX_CODES: usize = 4096;

/// Decode an LZW stream starting at 9-bit codes.
#[must_use]
pub fn lzw_decode(input: &[u8], early: EarlyChange) -> Partial {
    let early_v = early.decoder_bias();
    let mut out: Vec<u8> = Vec::with_capacity(input.len() * 3 + 16);
    // Pre-sized so the KwKwK case can reference the entry that is about to be created.
    let mut table: Vec<Vec<u8>> = vec![Vec::new(); MAX_CODES];
    let mut filled: usize = 0;
    let mut width: u32 = 9;
    let mut prev: Option<u16> = None;
    let mut bitpos: usize = 0;
    let nbits = input.len() * 8;
    let mut complete = false;
    let mut note: Option<String> = None;

    let reset = |table: &mut [Vec<u8>], filled: &mut usize| {
        for slot in table.iter_mut() {
            slot.clear();
        }
        for (i, slot) in table.iter_mut().take(256).enumerate() {
            *slot = vec![i as u8];
        }
        // 256 = clear, 257 = eod: present but empty.
        *filled = 258;
    };
    reset(&mut table, &mut filled);

    while bitpos + width as usize <= nbits {
        let mut code: u32 = 0;
        for _ in 0..width {
            let byte = bitpos >> 3;
            let shift = 7 - (bitpos & 7);
            let b = input.get(byte).copied().unwrap_or(0);
            code = (code << 1) | u32::from((b >> shift) & 1);
            bitpos += 1;
        }
        let code = u16::try_from(code).unwrap_or(0xFFFF);

        if code == EOD {
            complete = true;
            break;
        }
        if code == CLEAR {
            reset(&mut table, &mut filled);
            width = 9;
            prev = None;
            continue;
        }

        let idx = usize::from(code);
        let entry: Vec<u8> = if idx < 256 {
            vec![code as u8]
        } else if idx < filled {
            table.get(idx).cloned().unwrap_or_default()
        } else if idx == filled {
            // KwKwK: the code being read is the entry this step is about to create.
            match prev.and_then(|p| table.get(usize::from(p)).filter(|v| !v.is_empty())) {
                Some(pv) => {
                    let mut v = pv.clone();
                    if let Some(&f) = pv.first() {
                        v.push(f);
                    }
                    v
                }
                None => {
                    note = Some(format!("LZW code {code} with no prefix"));
                    break;
                }
            }
        } else {
            note = Some(format!("LZW code {code} outside the {filled}-entry table"));
            break;
        };
        out.extend_from_slice(&entry);

        if let Some(p) = prev
            && filled < MAX_CODES
        {
            let mut new_entry = table.get(usize::from(p)).cloned().unwrap_or_default();
            if let Some(&f) = entry.first() {
                new_entry.push(f);
            }
            if let Some(slot) = table.get_mut(filled) {
                *slot = new_entry;
            }
            filled += 1;
            if filled as u32 + early_v >= (1u32 << width) && width < 12 {
                width += 1;
            }
        }
        prev = Some(code);
    }

    if !complete && note.is_none() {
        note = Some(if bitpos + width as usize > nbits {
            "input ended before EOD".to_string()
        } else {
            "LZW table filled without EOD".to_string()
        });
    }

    Partial {
        data: out,
        complete,
        note,
    }
}

struct BitOut {
    bytes: Vec<u8>,
    acc: u64,
    n: u32,
}

impl BitOut {
    fn put(&mut self, code: u16, width: u32) {
        self.acc = (self.acc << width) | u64::from(code);
        self.n += width;
        while self.n >= 8 {
            self.n -= 8;
            self.bytes.push(((self.acc >> self.n) & 0xff) as u8);
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.bytes.push(((self.acc << (8 - self.n)) & 0xff) as u8);
        }
        self.bytes
    }
}

/// LZW encode, used for round-trip tests and for writing small image streams.
#[must_use]
pub fn lzw_encode(input: &[u8], early: EarlyChange) -> Vec<u8> {
    // The encoder always uses the plain rule; the decoder's `EarlyChange` accounts
    // for the one-entry lag (see `decoder_bias`).
    let early_v = 0u32;
    let mut out = BitOut {
        bytes: Vec::with_capacity(input.len() * 4 / 3 + 16),
        acc: 0,
        n: 0,
    };
    let mut table: BTreeMap<(u16, u8), u16> = BTreeMap::new();
    let mut next: u16 = FIRST_CODE;
    let mut width: u32 = 9;
    let mut w: Option<u16> = None;

    for &b in input {
        match w {
            None => w = Some(u16::from(b)),
            Some(prev_w) => {
                if let Some(&code) = table.get(&(prev_w, b)) {
                    w = Some(code);
                    continue;
                }
                out.put(prev_w, width);
                if next < MAX_CODES as u16 {
                    table.insert((prev_w, b), next);
                    next += 1;
                    if u32::from(next) + early_v >= (1u32 << width) && width < 12 {
                        width += 1;
                    }
                } else {
                    out.put(CLEAR, width);
                    table.clear();
                    next = FIRST_CODE;
                    width = 9;
                }
                w = Some(u16::from(b));
            }
        }
    }
    if let Some(code) = w {
        out.put(code, width);
    }
    out.put(EOD, width);
    out.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(data: &[u8]) {
        let enc = lzw_encode(data, EarlyChange::Standard);
        let dec = lzw_decode(&enc, EarlyChange::Standard);
        assert!(dec.complete, "{:?}", dec.note);
        assert_eq!(dec.data, data);
        // The two widths must both terminate rather than run away.
        let alt = lzw_decode(&enc, EarlyChange::Late);
        assert!(alt.data.len() < data.len() * 4 + 1024);
    }

    #[test]
    fn round_trip() {
        check(b"");
        check(b"a");
        check(b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        check(b"abcabcabcabcabcabc");
    }

    #[test]
    fn long_stream_spans_all_widths() {
        let data: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
        check(&data);
        let data: Vec<u8> = (0..30_000u32).map(|i| ((i / 13) % 7) as u8).collect();
        check(&data);
    }

    #[test]
    fn garbage_is_bounded() {
        let dec = lzw_decode(&[0xff; 64], EarlyChange::Standard);
        assert!(dec.data.len() < 1 << 20);
    }
}
