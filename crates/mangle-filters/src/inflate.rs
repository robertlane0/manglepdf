//! DEFLATE (RFC 1951) decompression.
//!
//! Written in house rather than taken from a crate because a PDF decoder must be
//! *partial-tolerant*: when a stream is truncated or damaged we return everything that
//! decoded cleanly plus a note, instead of discarding the whole object. Off-the-shelf
//! inflate implementations abort on the first error, which loses whole pages.

// Direct indexing is used throughout this file: every index is either masked to a
// table width or produced by a loop bounded by the length of the same buffer, so a
// checked access would add noise without adding safety. The surrounding code is
// still panic-free: see docs/PDF-QUIRKS.md for the callers' tolerance rules.
#![allow(clippy::indexing_slicing)]

use crate::error::FilterError;
use crate::{MAX_DECODED_BYTES, MAX_EXPANSION_RATIO, Partial};

const MAX_BITS: usize = 15;
const FAST_BITS: u32 = 9;

/// Reverse the low `n` bits of `v`.
fn reverse_bits(v: u32, n: u32) -> u32 {
    let mut out = 0u32;
    for i in 0..n {
        out |= ((v >> i) & 1) << (n - 1 - i);
    }
    out
}

/// Outcome of an inflate run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InflateOutcome {
    /// Decoded bytes; a prefix of what a healthy stream would produce.
    pub data: Vec<u8>,
    /// `true` only if the final block ended and the input was fully consumed.
    pub complete: bool,
    /// Why we stopped, when we did not finish.
    pub note: Option<String>,
    /// Bytes of input actually read.
    pub consumed: usize,
}

/// A canonical Huffman decoding table (counts + symbol order + a 9-bit fast lookup).
#[derive(Debug, Clone)]
struct Huffman {
    counts: [u16; MAX_BITS + 1],
    symbols: Vec<u16>,
    /// `len << 12 | symbol`, or 0 when the code is longer than `FAST_BITS`.
    fast: Vec<u16>,
}

impl Huffman {
    fn new(lengths: &[u8]) -> Result<Self, FilterError> {
        let mut counts = [0u16; MAX_BITS + 1];
        for &l in lengths {
            if l as usize > MAX_BITS {
                return Err(FilterError::Malformed("deflate huffman"));
            }
            counts[l as usize] = counts[l as usize].saturating_add(1);
        }
        if counts[0] as usize == lengths.len() {
            return Ok(Self {
                counts,
                symbols: Vec::new(),
                fast: vec![0; 1 << FAST_BITS],
            });
        }
        // Reject over-subscribed sets; incomplete sets are legal for the distance tree.
        let mut left: i32 = 1;
        for count in counts.iter().copied().take(MAX_BITS + 1).skip(1) {
            left <<= 1;
            left -= i32::from(count);
            if left < 0 {
                return Err(FilterError::Malformed("deflate huffman (over-subscribed)"));
            }
        }

        let mut offsets = [0u16; MAX_BITS + 2];
        for len in 1..MAX_BITS {
            offsets[len + 1] = offsets[len] + counts[len];
        }
        let mut symbols = vec![0u16; lengths.len()];
        for (sym, &l) in lengths.iter().enumerate() {
            if l != 0 {
                let slot = &mut offsets[l as usize];
                if let Some(dst) = symbols.get_mut(*slot as usize) {
                    *dst = sym as u16;
                }
                *slot = slot.saturating_add(1);
            }
        }

        // Fast table: every code of length <= FAST_BITS covers 2^(FAST_BITS-len) entries.
        // `peek` returns the next bits with the *earliest* bit in the low position, so
        // the index is the bit-reversed canonical code.
        let mut fast = vec![0u16; 1 << FAST_BITS];
        // Canonical code assignment uses only the *used* codes: a zero-length code
        // contributes nothing to the count for its length.
        let mut bl_count = [0u32; MAX_BITS + 1];
        for &l in lengths {
            if l > 0 {
                bl_count[usize::from(l)] += 1;
            }
        }
        let mut next_code = [0u32; MAX_BITS + 1];
        let mut code: u32 = 0;
        for len in 1..=MAX_BITS {
            code = (code + bl_count[len - 1]) << 1;
            next_code[len] = code;
        }
        for (sym, &l) in lengths.iter().enumerate() {
            if l == 0 {
                continue;
            }
            let len = l as usize;
            let c = next_code[len];
            next_code[len] = c.saturating_add(1);
            if len as u32 <= FAST_BITS {
                let shift = FAST_BITS - len as u32;
                let base = (c << shift) as usize;
                let run = 1usize << shift;
                let entry = ((len as u16) << 12) | (sym as u16);
                if base + run <= fast.len() {
                    for slot in fast.iter_mut().skip(base).take(run) {
                        *slot = entry;
                    }
                }
            }
        }
        Ok(Self {
            counts,
            symbols,
            fast,
        })
    }

    /// Decode one symbol. `None` means the input ran out.
    fn decode(&self, br: &mut BitReader<'_>) -> Option<u16> {
        let peeked = br.peek(FAST_BITS as usize);
        if let Some(entry) = self.fast.get(peeked as usize).copied()
            && entry != 0
        {
            let len = entry >> 12;
            br.consume(len as usize);
            return Some(entry & 0x0fff);
        }
        // Walk the code one bit at a time; `bit` already advances the reader, so the
        // loop must not consume again on the winning length.
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for len in 1..=MAX_BITS {
            code |= i32::from(br.bit()?);
            let count = i32::from(self.counts[len]);
            if code - first < count {
                let idx = (index + (code - first)) as usize;
                return self.symbols.get(idx).copied();
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        None
    }
}

/// LSB-first bit reader over a byte slice.
#[derive(Debug)]
struct BitReader<'a> {
    data: &'a [u8],
    /// Index of the next real byte to feed into the accumulator.
    pos: usize,
    bitbuf: u64,
    bitcnt: u32,
    /// Set once the reader has had to invent bits past the end of the input.
    eof: bool,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            bitbuf: 0,
            bitcnt: 0,
            eof: false,
        }
    }

    /// Peek up to `n` bits without consuming, with the *earliest* bit in the high
    /// position — the convention prefix codes are decoded with. Past the end of the
    /// input the missing bits read as zero, which keeps the loop bounds finite; `bit`
    /// reports the end.
    fn peek(&mut self, n: usize) -> u32 {
        while self.bitcnt < n as u32 {
            match self.data.get(self.pos) {
                Some(&byte) => {
                    self.pos += 1;
                    self.bitbuf |= u64::from(byte) << self.bitcnt;
                    self.bitcnt += 8;
                }
                None => {
                    self.eof = true;
                    self.bitcnt += 8;
                }
            }
        }
        reverse_bits((self.bitbuf & ((1u64 << n) - 1)) as u32, n as u32)
    }

    /// A numeric field of `n` bits, earliest bit first in the *low* position. This is
    /// how DEFLATE stores every non-prefix value (block types, counts, extra bits).
    fn field(&mut self, n: usize) -> u32 {
        let v = reverse_bits(self.peek(n), n as u32);
        self.consume(n);
        v
    }

    fn consume(&mut self, n: usize) {
        let n = n.min(self.bitcnt as usize);
        self.bitbuf >>= n;
        self.bitcnt -= n as u32;
    }

    /// One bit, or `None` at the end of input.
    fn bit(&mut self) -> Option<u8> {
        if self.eof {
            return None;
        }
        // `peek(1)` loads if needed and, for a single bit, is the identity reversal.
        let b = self.peek(1) as u8;
        self.consume(1);
        Some(b)
    }

    fn align(&mut self) {
        let drop = self.bitcnt % 8;
        self.consume(drop as usize);
    }

    fn read_bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        // Push back whole buffered bytes first.
        while self.bitcnt >= 8 {
            self.bitbuf >>= 8;
            self.bitcnt -= 8;
            self.pos = self.pos.saturating_sub(1);
        }
        self.bitbuf = 0;
        self.bitcnt = 0;
        let start = self.pos.min(self.data.len());
        let end = start.checked_add(n)?;
        if end > self.data.len() {
            return None;
        }
        self.pos = end;
        self.data.get(start..end)
    }

    /// Bytes still physically available, used to salvage a truncated stored block.
    fn remaining(&self) -> usize {
        self.data
            .len()
            .saturating_sub(self.pos.min(self.data.len()))
    }
}

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
struct FixedTables {
    lit: Huffman,
    dist: Huffman,
}

fn fixed_tables() -> FixedTables {
    let mut lengths = [0u8; 288];
    for (i, slot) in lengths.iter_mut().enumerate() {
        *slot = match i {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    let lit = Huffman::new(&lengths);
    let dist = Huffman::new(&[5u8; 30]);
    FixedTables {
        // The fixed tables are constants known to be well formed.
        lit: lit.unwrap_or_else(|_| unreachable_fixed()),
        dist: dist.unwrap_or_else(|_| unreachable_fixed()),
    }
}

fn unreachable_fixed() -> Huffman {
    // Only reached if a compile-time constant were rejected, which cannot happen.
    Huffman {
        counts: [0; MAX_BITS + 1],
        symbols: Vec::new(),
        fast: vec![0; 1 << FAST_BITS],
    }
}

/// Inflate a raw DEFLATE stream. `hint` is the expected output size when known.
pub fn inflate_raw(input: &[u8], hint: usize) -> InflateOutcome {
    let cap = output_cap(input.len(), hint);
    let mut out: Vec<u8> = Vec::with_capacity(hint.min(cap).min(1 << 22));
    let mut br = BitReader::new(input);
    let fixed = fixed_tables();

    let mut note: Option<String> = None;
    let mut complete = false;

    'blocks: loop {
        let Some(bfinal) = br.bit() else {
            note = Some("input ended before a block header".into());
            break 'blocks;
        };
        let btype = br.field(2);
        match btype {
            0 => {
                br.align();
                let Some(hdr) = br.read_bytes(4) else {
                    note = Some("truncated stored block header".into());
                    break 'blocks;
                };
                let len = u16::from_le_bytes([hdr[0], hdr[1]]) as usize;
                let nlen = u16::from_le_bytes([hdr[2], hdr[3]]) as usize;
                if len != (!nlen & 0xffff) {
                    note = Some("stored block length check failed".into());
                    break 'blocks;
                }
                let Some(chunk) = br.read_bytes(len) else {
                    // Copy what we can; the rest of the file is gone.
                    let have = br.remaining();
                    if let Some(tail) = br.read_bytes(have) {
                        out.extend_from_slice(tail);
                    }
                    note = Some("truncated stored block".into());
                    break 'blocks;
                };
                if out.len() + chunk.len() > cap {
                    out.extend_from_slice(&chunk[..cap.saturating_sub(out.len())]);
                    note = Some(format!("output capped at {cap} bytes"));
                    break 'blocks;
                }
                out.extend_from_slice(chunk);
            }
            1 | 2 => {
                let (lit, dist) = if btype == 1 {
                    (fixed.lit.clone(), fixed.dist.clone())
                } else {
                    match read_dynamic_tables(&mut br) {
                        Ok(t) => t,
                        Err(e) => {
                            note = Some(e.to_string());
                            break 'blocks;
                        }
                    }
                };
                loop {
                    let Some(sym) = lit.decode(&mut br) else {
                        note = Some("input ended inside a compressed block".into());
                        break 'blocks;
                    };
                    if sym < 256 {
                        if out.len() >= cap {
                            note = Some(format!("output capped at {cap} bytes"));
                            break 'blocks;
                        }
                        out.push(sym as u8);
                        continue;
                    }
                    if sym == 256 {
                        // End of block.
                        break;
                    }
                    let Some((length, distance)) =
                        read_match(&mut br, &dist, sym, out.len(), &mut note)
                    else {
                        break 'blocks;
                    };
                    if out.len() + length > cap {
                        let room = cap.saturating_sub(out.len());
                        copy_from_history(&mut out, distance, room);
                        note = Some(format!("output capped at {cap} bytes"));
                        break 'blocks;
                    }
                    copy_from_history(&mut out, distance, length);
                }
            }
            _ => {
                note = Some("reserved block type 3".into());
                break 'blocks;
            }
        }
        if bfinal == 1 {
            complete = true;
            break 'blocks;
        }
    }

    InflateOutcome {
        data: out,
        complete,
        note,
        consumed: br.pos.min(input.len()),
    }
}

/// Read one length/distance pair. Returns `None` and fills in `note` when the stream is
/// damaged, which is the caller's signal to stop and keep what it already has.
fn read_match(
    br: &mut BitReader<'_>,
    dist: &Huffman,
    sym: u16,
    out_len: usize,
    note: &mut Option<String>,
) -> Option<(usize, usize)> {
    let li = usize::from(sym - 257);
    let Some(&base) = LENGTH_BASE.get(li) else {
        *note = Some("invalid length symbol".into());
        return None;
    };
    if br.eof {
        *note = Some("truncated length extra bits".into());
        return None;
    }
    let extra = *LENGTH_EXTRA.get(li).unwrap_or(&0);
    let length = base as usize + br.field(extra as usize) as usize;

    let Some(dsym) = dist.decode(br) else {
        *note = Some("input ended before a distance".into());
        return None;
    };
    let di = usize::from(dsym);
    let Some(&dbase) = DIST_BASE.get(di) else {
        *note = Some("invalid distance symbol".into());
        return None;
    };
    if br.eof {
        *note = Some("truncated distance extra bits".into());
        return None;
    }
    let dextra = *DIST_EXTRA.get(di).unwrap_or(&0);
    let distance = dbase as usize + br.field(dextra as usize) as usize;
    if distance == 0 || distance > out_len {
        *note = Some("distance points before the output start".into());
        return None;
    }
    Some((length, distance))
}

/// Copy `len` bytes from `distance` back in the output, byte at a time so overlapping
/// runs (the common `abcabcabc` case) expand correctly.
fn copy_from_history(out: &mut Vec<u8>, distance: usize, len: usize) {
    let start = out.len() - distance;
    for i in 0..len {
        let Some(b) = out.get(start + i).copied() else {
            break;
        };
        out.push(b);
    }
}

fn read_dynamic_tables(br: &mut BitReader<'_>) -> Result<(Huffman, Huffman), FilterError> {
    let hlit = br.field(5) as usize + 257;
    let hdist = br.field(5) as usize + 1;
    let hclen = br.field(4) as usize + 4;
    if hlit > 286 || hdist > 30 {
        return Err(FilterError::Malformed("deflate dynamic header"));
    }
    let mut clen = [0u8; 19];
    for i in 0..hclen {
        let v = br.field(3) as u8;
        if let Some(slot) = CLEN_ORDER.get(i) {
            clen[*slot] = v;
        }
    }
    const CLEN_ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];
    let clen_huff = Huffman::new(&clen)?;

    let total = hlit + hdist;
    let mut lengths = vec![0u8; total];
    let mut i = 0usize;
    while i < total {
        let Some(sym) = clen_huff.decode(br) else {
            return Err(FilterError::Malformed("deflate code length code"));
        };
        match sym {
            0..=15 => {
                if let Some(slot) = lengths.get_mut(i) {
                    *slot = sym as u8;
                }
                i += 1;
            }
            16 => {
                if i == 0 {
                    return Err(FilterError::Malformed("deflate repeat at start"));
                }
                let prev = lengths.get(i - 1).copied().unwrap_or(0);
                let n = br.field(2) as usize;
                let n = 3 + n;
                for _ in 0..n {
                    if i >= total {
                        break;
                    }
                    if let Some(slot) = lengths.get_mut(i) {
                        *slot = prev;
                    }
                    i += 1;
                }
            }
            17 => {
                let n = br.field(3) as usize;
                i += 3 + n;
            }
            18 => {
                let n = br.field(7) as usize;
                i += 11 + n;
            }
            _ => return Err(FilterError::Malformed("deflate code length symbol")),
        }
        if i > total {
            return Err(FilterError::Malformed("deflate code length overflow"));
        }
    }
    let lit = Huffman::new(&lengths[..hlit])?;
    let dist = Huffman::new(&lengths[hlit..])?;
    Ok((lit, dist))
}

/// The largest output a stream may produce, from its compressed size alone.
///
/// The `hint` is deliberately *not* allowed to raise this: hints come from `/Width`,
/// `/Height` and friends, which an attacker controls, and a declared 20000x20000 image
/// would otherwise turn a 1 KiB bomb into a 1 GiB allocation. Real Flate data tops out
/// around 1000:1, so the ratio ceiling only ever bites on a bomb.
fn output_cap(compressed: usize, _hint: usize) -> usize {
    let ratio_cap = compressed.saturating_mul(MAX_EXPANSION_RATIO).max(4096);
    ratio_cap.min(MAX_DECODED_BYTES)
}

/// Inflate, guessing the output size from `hint` (a `/Length` or an image's row stride).
pub fn inflate(input: &[u8], hint: usize) -> Partial {
    let r = inflate_raw(input, hint);
    Partial {
        data: r.data,
        complete: r.complete,
        note: r.note,
    }
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect`, which is what a test is for; the
    // panic-free rule is about what the product does with a file, not about tests.
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::*;

    #[test]
    fn stored_block() {
        // BFINAL=1, BTYPE=00, LEN=3, NLEN=~3, "abc"
        let data = [0x01, 0x03, 0x00, 0xfc, 0xff, b'a', b'b', b'c'];
        let r = inflate_raw(&data, 3);
        assert!(r.complete, "{r:?}");
        assert_eq!(r.data, b"abc");
    }

    #[test]
    fn dynamic_block_with_distance() {
        // "aaaaaaaaaa" compressed by zlib, raw deflate portion extracted.
        let packed = crate::deflate::deflate(
            &b"aaaaaaaaaa".repeat(30),
            crate::deflate::DeflateLevel::Default,
        );
        let r = inflate_raw(&packed, 300);
        assert!(r.complete, "{r:?}");
        assert_eq!(r.data, b"aaaaaaaaaa".repeat(30));
    }

    #[test]
    fn overlapping_match_expands() {
        let src = b"ab".repeat(200);
        let packed = crate::deflate::deflate(&src, crate::deflate::DeflateLevel::Default);
        let r = inflate_raw(&packed, src.len());
        assert_eq!(r.data, src);
    }

    #[test]
    fn bomb_is_capped() {
        // A stored block that claims 60000 bytes but is cut short.
        let mut data = vec![0x01];
        data.extend_from_slice(&60000u16.to_le_bytes());
        data.extend_from_slice(&(!60000u16).to_le_bytes());
        let r = inflate_raw(&data, 0);
        assert!(!r.complete);
        assert!(r.data.len() <= MAX_DECODED_BYTES);
    }

    #[test]
    fn an_attacker_supplied_hint_cannot_raise_the_bomb_limit() {
        // Regression: the cap was `hint.max(ratio_cap)`, and the hint comes from
        // `/Width` and `/Height`, which the file controls. A 1 KiB image stream that
        // declared 20000x20000 would have been allowed to produce 1 GiB.
        assert_eq!(output_cap(1000, 0), 1000 * MAX_EXPANSION_RATIO);
        assert_eq!(output_cap(1000, usize::MAX), 1000 * MAX_EXPANSION_RATIO);
        assert_eq!(output_cap(0, 0), 4096);
        assert_eq!(output_cap(usize::MAX, usize::MAX), MAX_DECODED_BYTES);
    }

    #[test]
    fn ordinary_data_still_decodes_whole() {
        // The ratio ceiling is above what any valid deflate stream can reach (a match
        // returns at most 258 bytes for a couple of bytes of code, so roughly 1032:1),
        // so real content is never truncated by the bomb guard.
        let src = b"the quick brown fox jumps over the lazy dog. ".repeat(20_000);
        let packed = crate::deflate::deflate(&src, crate::deflate::DeflateLevel::Default);
        let hinted = inflate_raw(&packed, usize::MAX);
        assert!(hinted.complete, "{hinted:?}");
        assert_eq!(hinted.data, src);
        assert_eq!(inflate_raw(&packed, 0).data, hinted.data);
    }
}
