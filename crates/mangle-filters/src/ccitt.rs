//! CCITT Group 3 and Group 4 (ITU-T T.4 and T.6) facsimile decoding, as PDF uses it
//! for bilevel scans.

// Direct indexing is used throughout this file: every index is either masked to a
// table width or produced by a loop bounded by the length of the same buffer, so a
// checked access would add noise without adding safety. The surrounding code is
// still panic-free: see docs/PDF-QUIRKS.md for the callers' tolerance rules.
#![allow(clippy::indexing_slicing)]

use std::sync::OnceLock;

use crate::FilterResult;
use crate::error::FilterError;

/// The compression variant in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// Pure 1D: every run is coded with its own colour table.
    G3_1D,
    /// 2D: runs are coded relative to the previous line, 1D runs for short changes.
    G3_2D,
    /// Pure 2D (T.6): no EOLs, no 1D runs.
    G4,
}

/// Decode parameters from `/DecodeParms`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CcittParams {
    pub variant: Variant,
    /// `BlackIs1`: when false the decoded samples are inverted.
    pub black_is_1: bool,
    /// `EncodedByteAlign`: each line starts on a byte boundary.
    pub byte_align: bool,
    /// `EndOfBlock`: a trailing EOL terminates the data.
    pub end_of_block: bool,
    /// `DamagedRowsBeforeError`: 0 means "never give up", the PDF default.
    pub damaged_rows_before_error: i32,
    pub columns: usize,
    pub rows: usize,
}

impl Default for CcittParams {
    fn default() -> Self {
        Self {
            variant: Variant::G4,
            black_is_1: false,
            byte_align: false,
            end_of_block: false,
            damaged_rows_before_error: 0,
            columns: 1728,
            rows: 0,
        }
    }
}

// -------------------------------------------------------------------------------------
// Code tables (ITU-T T.4 tables 1, 2 and 3).
// -------------------------------------------------------------------------------------

const WHITE_TERMINATING: [&str; 64] = [
    "00110101", "000111", "0111", "1000", "1011", "1100", "1110", "1111", "10011", "10100",
    "00111", "01000", "001000", "000011", "110100", "110101", "101010", "101011", "0100111",
    "0001100", "0001000", "0010111", "0000011", "0000100", "0101000", "0101011", "0010011",
    "0100100", "0011000", "00000010", "00000011", "00011010", "00011011", "00010010", "00010011",
    "00010100", "00010101", "00010110", "00010111", "00101000", "00101001", "00101010", "00101011",
    "00101100", "00101101", "00000100", "00000101", "00001010", "00001011", "01010010", "01010011",
    "01010100", "01010101", "00100100", "00100101", "01011000", "01011001", "01011010", "01011011",
    "01001010", "01001011", "00110010", "00110011", "00110100",
];

const WHITE_MAKEUP: [(&str, u16); 27] = [
    ("11011", 64),
    ("10010", 128),
    ("010111", 192),
    ("0110111", 256),
    ("00110110", 320),
    ("00110111", 384),
    ("01100100", 448),
    ("01100101", 512),
    ("01101000", 576),
    ("01100111", 640),
    ("011001100", 704),
    ("011001101", 768),
    ("011010010", 832),
    ("011010011", 896),
    ("011010100", 960),
    ("011010101", 1024),
    ("011010110", 1088),
    ("011010111", 1152),
    ("011011000", 1216),
    ("011011001", 1280),
    ("011011010", 1344),
    ("011011011", 1408),
    ("010011000", 1472),
    ("010011001", 1536),
    ("010011010", 1600),
    ("011000", 1664),
    ("010011011", 1728),
];

const BLACK_TERMINATING: [&str; 64] = [
    "0000110111",
    "010",
    "11",
    "10",
    "011",
    "0011",
    "0010",
    "00011",
    "000101",
    "000100",
    "0000100",
    "0000101",
    "0000111",
    "00000100",
    "00000111",
    "000011000",
    "0000010111",
    "0000011000",
    "0000001000",
    "00001100111",
    "00001101000",
    "00001101100",
    "00000110111",
    "00000101000",
    "00000010111",
    "00000011000",
    "000011001010",
    "000011001011",
    "000011001100",
    "000011001101",
    "000001101000",
    "000001101001",
    "000001101010",
    "000001101011",
    "000011010010",
    "000011010011",
    "000011010100",
    "000011010101",
    "000011010110",
    "000011010111",
    "000001101100",
    "000001101101",
    "000011011010",
    "000011011011",
    "000001010100",
    "000001010101",
    "000001010110",
    "000001010111",
    "000001100100",
    "000001100101",
    "000001010010",
    "000001010011",
    "000000100100",
    "000000110111",
    "000000111000",
    "000000100111",
    "000000101000",
    "000001011000",
    "000001011001",
    "000000101011",
    "000000101100",
    "000001011010",
    "000001100110",
    "000001100111",
];

const BLACK_MAKEUP: [(&str, u16); 27] = [
    ("0000001111", 64),
    ("000011001000", 128),
    ("000011001001", 192),
    ("000001011011", 256),
    ("000000110011", 320),
    ("000000110100", 384),
    ("000000110101", 448),
    ("0000001101100", 512),
    ("0000001101101", 576),
    ("0000001001010", 640),
    ("0000001001011", 704),
    ("0000001001100", 768),
    ("0000001001101", 832),
    ("0000001110010", 896),
    ("0000001110011", 960),
    ("0000001110100", 1024),
    ("0000001110101", 1088),
    ("0000001110110", 1152),
    ("0000001110111", 1216),
    ("0000001010010", 1280),
    ("0000001010011", 1344),
    ("0000001010100", 1408),
    ("0000001010101", 1472),
    ("0000001011010", 1536),
    ("0000001011011", 1600),
    ("0000001100100", 1664),
    ("0000001100101", 1728),
];

/// Extended make-up codes, shared by both colours (1792..=2560).
const EXTENDED_MAKEUP: [(&str, u16); 13] = [
    ("00000001000", 1792),
    ("00000001100", 1856),
    ("00000001101", 1920),
    ("000000010010", 1984),
    ("000000010011", 2048),
    ("000000010100", 2112),
    ("000000010101", 2176),
    ("000000010110", 2240),
    ("000000010111", 2304),
    ("000000011100", 2368),
    ("000000011101", 2432),
    ("000000011110", 2496),
    ("000000011111", 2560),
];

/// One node of a prefix-code trie. `leaf` holds the run length when this node ends a
/// code; `-1` means interior.
#[derive(Debug, Clone, Copy)]
struct Node {
    children: [i32; 2],
    leaf: i32,
}

impl Node {
    const fn interior() -> Self {
        Self {
            children: [-1, -1],
            leaf: -1,
        }
    }
}

#[derive(Debug)]
struct Tables {
    white: Vec<Node>,
    black: Vec<Node>,
}

fn insert_into(trie: &mut Vec<Node>, bits: &str, value: u16) {
    let mut node = 0usize;
    for b in bits.bytes() {
        let bit = usize::from(b == b'1');
        let child = trie.get(node).map_or(-1, |n| n.children[bit]);
        if child < 0 {
            let new = i32::try_from(trie.len()).unwrap_or(-1);
            if let Some(slot) = trie.get_mut(node) {
                slot.children[bit] = new;
            }
            trie.push(Node::interior());
            node = usize::try_from(new).unwrap_or(0);
        } else {
            node = usize::try_from(child).unwrap_or(0);
        }
    }
    if let Some(slot) = trie.get_mut(node) {
        slot.leaf = i32::from(value);
    }
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let mut white = vec![Node::interior()];
        let mut black = vec![Node::interior()];
        for (i, bits) in WHITE_TERMINATING.iter().enumerate() {
            insert_into(&mut white, bits, i as u16);
        }
        for (bits, v) in WHITE_MAKEUP {
            insert_into(&mut white, bits, v);
        }
        for (i, bits) in BLACK_TERMINATING.iter().enumerate() {
            insert_into(&mut black, bits, i as u16);
        }
        for (bits, v) in BLACK_MAKEUP {
            insert_into(&mut black, bits, v);
        }
        // Extended make-up codes live in both colour tables.
        for (bits, v) in EXTENDED_MAKEUP {
            insert_into(&mut white, bits, v);
            insert_into(&mut black, bits, v);
        }
        Tables { white, black }
    })
}

/// MSB-first bit reader.
#[derive(Debug)]
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn bit(&mut self) -> Option<u8> {
        let byte = *self.data.get(self.pos >> 3)?;
        let b = (byte >> (7 - (self.pos & 7))) & 1;
        self.pos += 1;
        Some(b)
    }

    fn peek(&self, n: usize) -> Option<u32> {
        let end = self.pos.checked_add(n)?;
        if end > self.data.len() * 8 {
            return None;
        }
        let mut v = 0u32;
        for i in 0..n {
            let byte = *self.data.get((self.pos + i) >> 3)?;
            v = (v << 1) | u32::from((byte >> (7 - ((self.pos + i) & 7))) & 1);
        }
        Some(v)
    }

    fn align(&mut self) {
        self.pos = (self.pos + 7) & !7;
    }

    /// A T.4 EOL is twelve zero bits followed by a one.
    fn at_eol(&self) -> bool {
        self.peek(12) == Some(0b0000_0000_0001)
    }

    /// EOFB is two consecutive EOLs.
    fn at_eofb(&self) -> bool {
        self.at_eol() && self.peek_at(self.pos + 12, 12) == Some(0b0000_0000_0001)
    }

    fn peek_at(&self, from: usize, n: usize) -> Option<u32> {
        let end = from.checked_add(n)?;
        if end > self.data.len() * 8 {
            return None;
        }
        let mut v = 0u32;
        for i in 0..n {
            let byte = *self.data.get((from + i) >> 3)?;
            v = (v << 1) | u32::from((byte >> (7 - ((from + i) & 7))) & 1);
        }
        Some(v)
    }

    fn eod(&self) -> bool {
        self.pos >= self.data.len() * 8
    }
}

/// One step of the 2D mode code (T.4 table 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Pass,
    Horizontal,
    Vertical(i8),
}

/// The eight 2D mode codes of ITU-T T.4 table 4: the code's bits, its width, and what
/// it means. Listed longest last so the widths are visible in one place.
const MODE_CODES: [(&str, Mode); 8] = [
    ("1", Mode::Vertical(0)),
    ("011", Mode::Vertical(1)),
    ("010", Mode::Vertical(-1)),
    ("001", Mode::Horizontal),
    ("0001", Mode::Vertical(-2)),
    ("000011", Mode::Vertical(2)),
    ("0000011", Mode::Vertical(-3)),
    ("0000001", Mode::Pass),
];

fn parse_bits(bits: &str) -> u32 {
    bits.bytes()
        .fold(0u32, |acc, b| (acc << 1) | u32::from(b == b'1'))
}

/// Read a 2D mode code. `None` means the bits are not a valid mode.
fn read_mode(bits: &mut Bits<'_>) -> Option<Mode> {
    let v = bits.peek(7)?;
    for (pattern, mode) in MODE_CODES {
        let width = u32::try_from(pattern.len()).unwrap_or(0);
        // `peek` returns the first bits in the high positions, so the code to match
        // is the top `width` bits of the window.
        if (v >> (7 - width)) == parse_bits(pattern) {
            bits.pos += width as usize;
            return Some(mode);
        }
    }
    None
}

/// Decode one run of the given colour: make-up codes then a terminating code.
fn read_run(bits: &mut Bits<'_>, black: bool) -> Option<u32> {
    let t = tables();
    let main = if black { &t.black } else { &t.white };
    let mut total: u32 = 0;
    loop {
        let mut node = 0usize;
        let leaf = loop {
            let b = bits.bit()?;
            let child = main.get(node).map_or(-1, |n| n.children[usize::from(b)]);
            if child < 0 {
                return None;
            }
            node = usize::try_from(child).ok()?;
            let leaf = main.get(node).map_or(-1, |n| n.leaf);
            if leaf >= 0 {
                break leaf;
            }
        };
        total = total.saturating_add(u32::from(leaf as u16));
        if leaf < 64 {
            return Some(total);
        }
        if total > 2560 {
            return None;
        }
    }
}

/// A decoded line: the positions where colour changes, strictly increasing.
#[derive(Debug, Default, Clone)]
struct Line {
    changes: Vec<i64>,
}

impl Line {
    /// Colour of the run containing `at`: `true` is black.
    fn colour_at(&self, at: i64) -> bool {
        self.changes.iter().take_while(|c| **c < at).count() % 2 == 1
    }

    /// `b1` and `b2` from T.4: the first and second changing elements to the right of
    /// `a0` whose colour differs from the colour at `a0`.
    fn b1_b2(&self, a0: i64) -> (i64, i64) {
        let base = self.colour_at(a0);
        let mut found = false;
        let mut b1 = -1i64;
        for (j, &p) in self.changes.iter().enumerate() {
            if p <= a0 {
                continue;
            }
            let colour = j % 2 == 1;
            if colour != base {
                b1 = p;
                found = true;
                break;
            }
        }
        if !found {
            return (-1, -1);
        }
        // b2 is the element after b1 on the reference line.
        let idx = self.changes.iter().position(|c| *c == b1).unwrap_or(0);
        let b2 = self.changes.get(idx + 1).copied().unwrap_or(-1);
        (b1, b2)
    }

    /// Expand to `width` samples, 1 = black.
    ///
    /// The list holds *change points* — where one run ends and the next begins — so it
    /// says nothing about the end of the last run: a row coded `white 3, black 5` has one
    /// change point, at three. Every sample up to a change point is filled from it, and
    /// filling only to them leaves the whole tail at the row's opening colour. A line
    /// whose last run is white comes out right by coincidence, which is why a corpus of
    /// such lines hid this: the runs after the final change point have to be emitted from
    /// the colour already in force when the list runs out.
    fn samples(&self, width: usize) -> Vec<u8> {
        let mut row = vec![0u8; width];
        let mut colour = false;
        let mut pos = 0usize;
        for &c in &self.changes {
            let at = usize::try_from(c).unwrap_or(width).min(width);
            if at <= pos {
                colour = !colour;
                continue;
            }
            for slot in row.iter_mut().take(at).skip(pos) {
                *slot = u8::from(colour);
            }
            pos = at;
            colour = !colour;
            if pos >= width {
                break;
            }
        }
        for slot in row.iter_mut().skip(pos) {
            *slot = u8::from(colour);
        }
        row
    }
}

/// Decode CCITT data to one byte per pixel, 0 = white and 1 = black before inversion.
pub fn ccitt_decode(data: &[u8], p: &CcittParams) -> FilterResult<Vec<u8>> {
    if p.columns == 0 {
        return Err(FilterError::BadParameters {
            filter: "CCITTFaxDecode",
            reason: "/Columns is zero".into(),
        });
    }
    let width = p.columns;
    // A hostile /Columns x /Rows pair must not be able to ask for a terabyte.
    let max_pixels = 1usize << 30;
    let row_cap: usize = if p.rows > 0 { p.rows } else { usize::MAX };

    let mut out: Vec<u8> = Vec::new();
    let mut bits = Bits::new(data);
    let mut reference = Line::default();
    let mut damaged = 0i32;
    let max_damage = if p.damaged_rows_before_error > 0 {
        p.damaged_rows_before_error
    } else {
        i32::MAX
    };

    // Leading fill and EOLs.
    while bits.peek(12) == Some(0) && !bits.eod() {
        bits.pos += 12;
    }
    if bits.at_eol() {
        bits.pos += 12;
    }

    'rows: while out.len() / width < row_cap && out.len() < max_pixels {
        if p.byte_align {
            bits.align();
        }
        if p.end_of_block && bits.at_eofb() {
            break;
        }
        if bits.eod() {
            break;
        }
        // Group 3 lines may be preceded by an EOL; Group 4 never is.
        if p.variant != Variant::G4 {
            while bits.at_eol() {
                bits.pos += 12;
                if bits.eod() {
                    break 'rows;
                }
            }
        }
        if bits.eod() {
            break;
        }

        let line = match p.variant {
            Variant::G3_1D => decode_line_1d(&mut bits, width, max_damage, &mut damaged),
            Variant::G3_2D => decode_line_2d(
                &mut bits,
                width,
                &reference,
                false,
                max_damage,
                &mut damaged,
            ),
            Variant::G4 => {
                decode_line_2d(&mut bits, width, &reference, true, max_damage, &mut damaged)
            }
        };
        let Some(line) = line else {
            damaged += 1;
            if damaged > max_damage {
                break 'rows;
            }
            // Skip to the next line boundary and carry on with a blank line.
            if bits.eod() {
                break 'rows;
            }
            bits.align();
            if bits.eod() {
                break 'rows;
            }
            out.extend(std::iter::repeat_n(0u8, width));
            continue;
        };
        let mut row = line.samples(width);
        if !p.black_is_1 {
            for b in &mut row {
                *b = u8::from(*b == 0);
            }
        }
        out.extend_from_slice(&row);
        reference = line;
    }

    Ok(out)
}

fn decode_line_1d(
    bits: &mut Bits<'_>,
    width: usize,
    max_damage: i32,
    damaged: &mut i32,
) -> Option<Line> {
    let mut line = Line::default();
    let mut x: i64 = 0;
    let w = i64::try_from(width).ok()?;
    let mut guard = 0usize;
    // Runs alternate white, black, white... so the colour is implicit: an even number
    // of changes means white. Zero-length runs are legal and must be recorded, because
    // they are what lets a line start on black.
    let black = false;
    loop {
        guard += 1;
        if guard > width * 4 + 64 {
            return None;
        }
        if x >= w {
            break;
        }
        if bits.eod() {
            break;
        }
        let Some(run) = read_run(bits, black) else {
            *damaged += 1;
            if *damaged > max_damage {
                return None;
            }
            return Some(line);
        };
        let next = x + i64::from(run);
        if next < w {
            line.changes.push(next);
        }
        x = next;
        if x >= w {
            break;
        }
        if bits.eod() {
            break;
        }
        let Some(run) = read_run(bits, !black) else {
            return Some(line);
        };
        let next = x + i64::from(run);
        if next < w {
            line.changes.push(next);
        }
        x = next;
        // Two runs were consumed, so the colour is back to where it started.
    }
    Some(line)
}

/// Decode one line of 2D data. `pure_2d` selects T.6, where 1D runs are not allowed.
fn decode_line_2d(
    bits: &mut Bits<'_>,
    width: usize,
    reference: &Line,
    pure_2d: bool,
    max_damage: i32,
    damaged: &mut i32,
) -> Option<Line> {
    let w = i64::try_from(width).ok()?;
    let mut line = Line::default();
    let mut a0: i64 = -1;
    let mut colour = false; // colour of the run starting at a0
    let mut guard = 0usize;

    while a0 < w {
        guard += 1;
        if guard > width * 4 + 64 {
            return None;
        }
        if bits.eod() {
            break;
        }
        let (mut b1, mut b2) = reference.b1_b2(a0);
        if b1 < 0 {
            b1 = w;
            b2 = w;
        }
        if b2 < a0 {
            b2 = b1;
        }

        let use_1d = !pure_2d && b1 - a0 <= 3;
        if use_1d {
            let Some(r1) = read_run(bits, colour) else {
                *damaged += 1;
                if *damaged > max_damage {
                    return None;
                }
                return Some(line);
            };
            let Some(r2) = read_run(bits, !colour) else {
                return Some(line);
            };
            let next = a0 + i64::from(r1) + i64::from(r2);
            if next < w {
                line.changes.push(next);
            }
            a0 = next;
            colour = !colour;
            continue;
        }

        let Some(mode) = read_mode(bits) else {
            *damaged += 1;
            if *damaged > max_damage {
                return None;
            }
            return Some(line);
        };
        match mode {
            Mode::Pass => {
                a0 = b2;
            }
            Mode::Horizontal => {
                let Some(r1) = read_run(bits, colour) else {
                    return Some(line);
                };
                let Some(r2) = read_run(bits, !colour) else {
                    return Some(line);
                };
                let mid = b1 + i64::from(r1);
                let next = mid + i64::from(r2);
                if next < w {
                    line.changes.push(next);
                }
                a0 = next;
                colour = !colour;
            }
            Mode::Vertical(d) => {
                let next = b1 + i64::from(d);
                if next > 0 && next < w {
                    line.changes.push(next);
                }
                a0 = next;
                colour = !colour;
            }
        }
    }
    line.changes.sort_unstable();
    line.changes.dedup();
    Some(line)
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect`, which is what a test is for; the
    // panic-free rule is about what the product does with a file, not about tests.
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::*;

    /// Pack a bit string MSB-first into bytes, zero padding the final byte.
    fn pack(bits: &str) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::with_capacity(bits.len().div_ceil(8));
        let mut acc = 0u8;
        let mut n = 0u32;
        for b in bits.bytes() {
            acc = (acc << 1) | u32::from(b == b'1') as u8;
            n += 1;
            if n == 8 {
                out.push(acc);
                acc = 0;
                n = 0;
            }
        }
        if n > 0 {
            out.push(acc << (8 - n));
        }
        out
    }

    #[test]
    fn tables_build() {
        let t = tables();
        assert!(t.white.len() > 100);
        assert!(t.black.len() > 100);
    }

    #[test]
    fn terminating_codes_round_trip_through_the_trie() {
        for (i, bits) in WHITE_TERMINATING.iter().enumerate() {
            // Two bytes of slack so `bit()` never runs dry mid-code.
            let mut data = pack(bits);
            data.extend_from_slice(&[0b1000_0000, 0]);
            let mut b = Bits::new(&data);
            assert_eq!(read_run(&mut b, false), Some(i as u32), "white {bits}");
        }
        for (i, bits) in BLACK_TERMINATING.iter().enumerate() {
            let mut data = pack(bits);
            data.extend_from_slice(&[0b1000_0000, 0]);
            let mut b = Bits::new(&data);
            assert_eq!(read_run(&mut b, true), Some(i as u32), "black {bits}");
        }
    }

    #[test]
    fn makeup_plus_terminating_sums() {
        // make-up 64 (11011) + white terminating 3 (1000) = 67
        let data = pack("110111000");
        let mut b = Bits::new(&data);
        assert_eq!(read_run(&mut b, false), Some(67));
    }

    #[test]
    fn mode_codes_are_distinct() {
        let cases = [
            ("1", Mode::Vertical(0)),
            ("011", Mode::Vertical(1)),
            ("010", Mode::Vertical(-1)),
            ("001", Mode::Horizontal),
            ("0001", Mode::Vertical(-2)),
            ("000011", Mode::Vertical(2)),
            ("0000011", Mode::Vertical(-3)),
            ("0000001", Mode::Pass),
        ];
        for (bits, expect) in cases {
            let mut data = pack(bits);
            data.push(0);
            let mut b = Bits::new(&data);
            assert_eq!(read_mode(&mut b), Some(expect), "{bits}");
        }
    }

    #[test]
    fn empty_input_is_empty_output() {
        let p = CcittParams {
            columns: 16,
            ..Default::default()
        };
        assert!(ccitt_decode(&[], &p).expect("decode").is_empty());
    }

    #[test]
    fn all_white_rows_1d() {
        // Two all-white 1728-pixel rows, each terminated by an EOL.
        let p = CcittParams {
            variant: Variant::G3_1D,
            columns: 1728,
            rows: 2,
            ..Default::default()
        };
        // A run is make-up codes followed by a terminating code, so 1600 + 128 + 0.
        let line = "010011010".to_string() + "10010" + "00110101";
        let data = pack(&format!("{line}000000000001{line}000000000001"));
        let out = ccitt_decode(&data, &p).expect("decode");
        assert_eq!(out.len(), 2 * 1728, "row count");
        // BlackIs1 default false, so white is 1.
        assert!(out.iter().all(|b| *b == 1), "expected all white");
    }

    #[test]
    fn black_is_one_marks_black_pixels() {
        // A line starts white, so a leading zero-length white run is coded first.
        let bits = "00110101".to_string() + "0011" + "10100";
        let p = CcittParams {
            variant: Variant::G3_1D,
            columns: 14,
            rows: 1,
            black_is_1: true,
            ..Default::default()
        };
        let out = ccitt_decode(&pack(&bits), &p).expect("decode");
        assert_eq!(out.len(), 14);
        assert_eq!(&out[..5], &[1, 1, 1, 1, 1]);
        assert_eq!(&out[5..], &[0; 9]);
    }

    // ── A line's last run ────────────────────────────────────────────────────────────
    //
    // Built from change points rather than from a stream, because a change point list is
    // what `samples` is given and the point is what it does with one that ends early. Every
    // fax fixture in the project ends on a white run, which is the one case where leaving
    // the tail alone looks correct.

    /// The defect: a change point says where a run *ends*, so a list whose last entry is
    /// not at the row's edge describes a tail that `samples` used to leave at the row's
    /// opening colour.
    #[test]
    fn a_line_that_ends_on_black_emits_its_last_run() {
        // White three then black five, out of eight: one change point, at three.
        let line = Line { changes: vec![3] };
        assert_eq!(
            line.samples(8),
            vec![0, 0, 0, 1, 1, 1, 1, 1],
            "the five black pixels after the last change point are the last run"
        );
    }

    #[test]
    fn a_line_of_one_run_is_entirely_that_run() {
        // No change points at all: the whole row is the opening colour.
        let white = Line::default();
        assert_eq!(white.samples(6), vec![0; 6], "an all-white row");
        // A change point at zero is the row opening on the other colour, so the single run
        // is the whole of it in the other direction.
        let black = Line { changes: vec![0] };
        assert_eq!(black.samples(6), vec![1; 6], "an all-black row");
    }

    /// The case the existing fixtures cover, which must not move.
    #[test]
    fn a_line_that_ends_on_white_is_unchanged() {
        // White three, black two, white three.
        let line = Line {
            changes: vec![3, 5],
        };
        assert_eq!(line.samples(8), vec![0, 0, 0, 1, 1, 0, 0, 0]);
    }

    /// A row is exactly as wide as it was asked to be, whatever its change points say.
    #[test]
    fn every_row_is_exactly_as_wide_as_it_was_asked_to_be() {
        let lines = [
            Line::default(),
            Line { changes: vec![0] },
            Line { changes: vec![1] },
            Line {
                changes: vec![0, 1],
            },
            Line {
                changes: vec![3, 5],
            },
            // Past the edge: a change point beyond the row is clamped rather than ignored,
            // and must not shorten the row.
            Line {
                changes: vec![3, 99],
            },
        ];
        for width in [1usize, 2, 5, 8, 13, 344] {
            for line in &lines {
                let row = line.samples(width);
                assert_eq!(row.len(), width, "{width} columns from {line:?} is {row:?}");
            }
        }
    }

    /// A change point at or beyond the row's width ends the row rather than wrapping it.
    #[test]
    fn a_change_point_at_the_edge_fills_the_whole_row() {
        let line = Line {
            changes: vec![0, 4],
        };
        assert_eq!(line.samples(4), vec![1, 1, 1, 1], "black to the edge");
        let wider = Line {
            changes: vec![0, 4],
        };
        assert_eq!(
            wider.samples(9),
            vec![1, 1, 1, 1, 0, 0, 0, 0, 0],
            "and the tail after it is the colour now in force"
        );
    }

    /// The same defect seen end to end: a Group 3 row whose black run reaches the right
    /// margin, which the decoder reports as one change point and must still draw.
    #[test]
    fn a_decoded_line_that_ends_on_black_fills_its_last_run() {
        // White three (`1000`) then black five (`0011`), with BlackIs1 so white is 0.
        let bits = "1000".to_string() + "0011";
        let p = CcittParams {
            variant: Variant::G3_1D,
            columns: 8,
            rows: 1,
            black_is_1: true,
            ..Default::default()
        };
        let out = ccitt_decode(&pack(&bits), &p).expect("decode");
        assert_eq!(
            out,
            vec![0, 0, 0, 1, 1, 1, 1, 1],
            "three white then five black, the black reaching the right margin"
        );
    }
}
