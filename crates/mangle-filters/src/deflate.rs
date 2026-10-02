//! DEFLATE (RFC 1951) compression.
//!
//! In house for the same reason as `inflate`: we control the byte-for-byte output so
//! that saving the same document twice produces the same file, and so that a stream we
//! wrote is decoded by our own partial-tolerant reader.
//!
//! One block is emitted per ~16k tokens, choosing whichever of stored / fixed / dynamic
//! Huffman is smallest. The result is deterministic: no timing, no randomness.
//!
//! Two entry points, because PDF's `/FlateDecode` names zlib and not deflate:
//! [`deflate`] writes the zlib stream a PDF wants and [`deflate_raw`] writes the bare
//! RFC 1951 data underneath it.

// Direct indexing is used throughout this file: every index is either masked to a
// table width or produced by a loop bounded by the length of the same buffer, so a
// checked access would add noise without adding safety. The surrounding code is
// still panic-free: see docs/PDF-QUIRKS.md for the callers' tolerance rules.
#![allow(clippy::indexing_slicing)]

use crate::zlib;

const MAX_BITS: u8 = 15;

/// Compression effort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeflateLevel {
    /// Short hash chain, no lazy matching. Fastest.
    Fast,
    /// Hash chain of 128 with lazy matching. The default for saved documents.
    Default,
    /// Long hash chain with deep lazy matching. Used by "Optimize".
    Best,
}

impl DeflateLevel {
    fn params(self) -> (usize, usize, bool) {
        match self {
            DeflateLevel::Fast => (8, 8, false),
            DeflateLevel::Default => (128, 64, true),
            DeflateLevel::Best => (1024, 258, true),
        }
    }
}

const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
const WINDOW: usize = 32 * 1024;
const BLOCK_TOKENS: usize = 1 << 14;
const HASH_BITS: usize = 15;
const HASH_SIZE: usize = 1 << HASH_BITS;

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
const CLEN_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// One LZ77 token: a literal byte, or a match of `len` bytes `dist` back.
#[derive(Debug, Clone, Copy)]
struct Token {
    len: Option<u16>,
    dist: u16,
    lit: u8,
}

#[derive(Debug, Default)]
struct BitWriter {
    out: Vec<u8>,
    acc: u32,
    nbits: u32,
}

impl BitWriter {
    fn with_capacity(n: usize) -> Self {
        Self {
            out: Vec::with_capacity(n),
            acc: 0,
            nbits: 0,
        }
    }

    /// Raw bits, least significant first (DEFLATE's native order).
    fn write_bits(&mut self, value: u32, n: u32) {
        if n == 0 {
            return;
        }
        self.acc |= (value & ((1u32 << n) - 1)) << self.nbits;
        self.nbits += n;
        while self.nbits >= 8 {
            self.out.push((self.acc & 0xff) as u8);
            self.acc >>= 8;
            self.nbits -= 8;
        }
    }

    /// Huffman codes go out most significant bit first.
    fn write_code(&mut self, code: u32, len: u8) {
        let mut v = 0u32;
        for i in 0..len {
            v |= ((code >> (len - 1 - i)) & 1) << i;
        }
        self.write_bits(v, u32::from(len));
    }

    fn align(&mut self) {
        if self.nbits > 0 {
            self.out.push((self.acc & 0xff) as u8);
            self.acc = 0;
            self.nbits = 0;
        }
    }

    fn finish(mut self) -> Vec<u8> {
        self.align();
        self.out
    }
}

/// Huffman code lengths for a frequency table, limited to `max_bits`.
///
/// The result may be an incomplete code, which DEFLATE allows for the distance tree.
fn build_lengths(freqs: &[u32], max_bits: u8) -> Vec<u8> {
    let mut work: Vec<u64> = freqs.iter().map(|&f| u64::from(f)).collect();
    for _ in 0..24 {
        let lens = tree_lengths(&work);
        if lens.iter().all(|&l| u32::from(l) <= u32::from(max_bits)) {
            return lens;
        }
        // Too deep: flatten the distribution and retry. Converges in a few rounds.
        for f in &mut work {
            *f = (*f).div_ceil(2);
        }
    }
    // Last resort: a flat, shallow table is always within the limit.
    let mut lens = vec![0u8; freqs.len()];
    for (i, f) in freqs.iter().enumerate() {
        if *f > 0 {
            lens[i] = max_bits.min(9);
        }
    }
    lens
}

/// Huffman code lengths by repeated two-smallest merging.
///
/// The alphabets here are tiny (288 and 30 symbols), so a sorted vector beats a heap and
/// keeps the tie-breaking — which the determinism requirement depends on — obvious.
fn tree_lengths(freqs: &[u64]) -> Vec<u8> {
    let mut lens = vec![0u8; freqs.len()];
    let used: Vec<usize> = (0..freqs.len()).filter(|&i| freqs[i] > 0).collect();
    match used.len() {
        0 => return lens,
        1 => {
            if let Some(&i) = used.first() {
                lens[i] = 1;
            }
            return lens;
        }
        _ => {}
    }
    #[derive(Debug, Clone, Copy)]
    struct Node {
        freq: u64,
        children: Option<(usize, usize)>,
        sym: Option<usize>,
    }
    // `nodes[k]` corresponds to `used[k]`; `active` holds node indices only.
    let mut nodes: Vec<Node> = used
        .iter()
        .map(|&s| Node {
            freq: freqs[s],
            children: None,
            sym: Some(s),
        })
        .collect();
    let mut active: Vec<usize> = (0..nodes.len()).collect();

    while active.len() > 1 {
        active.sort_by(|&a, &b| match (nodes.get(a), nodes.get(b)) {
            (Some(x), Some(y)) => x.freq.cmp(&y.freq).then(a.cmp(&b)),
            _ => a.cmp(&b),
        });
        let (left, right) = (active[1], active[0]);
        let freq = nodes.get(left).map_or(0, |n| n.freq) + nodes.get(right).map_or(0, |n| n.freq);
        nodes.push(Node {
            freq,
            children: Some((left, right)),
            sym: None,
        });
        let parent = nodes.len() - 1;
        active.drain(0..2);
        active.push(parent);
    }

    // Depth walk. Depth is bounded by the alphabet size in practice; the guard keeps a
    // pathological table from recursing deeply.
    let mut stack = vec![(active[0], 0u16)];
    let mut guard = 0usize;
    while let Some((node, depth)) = stack.pop() {
        guard += 1;
        if guard > 4 * nodes.len() + 16 {
            break;
        }
        let Some(nd) = nodes.get(node).copied() else {
            continue;
        };
        match (nd.sym, nd.children) {
            (Some(s), _) => {
                if let Some(l) = lens.get_mut(s) {
                    *l = u8::try_from(depth.max(1)).unwrap_or(0);
                }
            }
            (None, Some((l, r))) if depth < 32 => {
                stack.push((l, depth + 1));
                stack.push((r, depth + 1));
            }
            _ => {}
        }
    }
    lens
}

/// Canonical codes: symbol index -> `(code, bit length)`.
fn canonical(lengths: &[u8]) -> Vec<(u32, u8)> {
    let Some(&maxbits) = lengths.iter().max() else {
        return Vec::new();
    };
    if maxbits == 0 {
        return vec![(0, 0); lengths.len()];
    }
    let max = usize::from(maxbits);
    let mut bl_count = vec![0u32; max + 2];
    for &l in lengths {
        if l > 0 {
            if let Some(c) = bl_count.get_mut(usize::from(l)) {
                *c += 1;
            }
        }
    }
    let mut next_code = vec![0u32; max + 2];
    let mut code = 0u32;
    for bits in 1..=max {
        code = (code + bl_count[bits - 1]) << 1;
        next_code[bits] = code;
    }
    let mut out = vec![(0u32, 0u8); lengths.len()];
    for (i, &l) in lengths.iter().enumerate() {
        if l > 0 {
            let c = next_code[usize::from(l)];
            next_code[usize::from(l)] = c + 1;
            if let Some(slot) = out.get_mut(i) {
                *slot = (c, l);
            }
        }
    }
    out
}

fn length_code(len: usize) -> usize {
    let mut i = 0;
    while i + 1 < LENGTH_BASE.len() && usize::from(LENGTH_BASE[i + 1]) <= len {
        i += 1;
    }
    i
}

fn dist_code(dist: usize) -> usize {
    let mut i = 0;
    while i + 1 < DIST_BASE.len() && usize::from(DIST_BASE[i + 1]) <= dist {
        i += 1;
    }
    i
}

/// A run of symbols in the code-length (RLE) alphabet.
#[derive(Debug, Clone, Copy)]
struct ClSymbol {
    sym: u8,
    extra: u8,
    extra_bits: u8,
}

/// RLE-encode the concatenated literal and distance code lengths.
fn code_length_sequence(lengths: &[u8]) -> Vec<ClSymbol> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < lengths.len() {
        let v = lengths[i];
        let mut run = 1usize;
        while i + run < lengths.len() && lengths[i + run] == v && run < 138 {
            run += 1;
        }
        if v == 0 {
            while run >= 11 {
                let take = run.min(138);
                out.push(ClSymbol {
                    sym: 18,
                    extra: (take - 11) as u8,
                    extra_bits: 7,
                });
                i += take;
                run -= take;
            }
            while run >= 3 {
                let take = run.min(10);
                out.push(ClSymbol {
                    sym: 17,
                    extra: (take - 3) as u8,
                    extra_bits: 3,
                });
                i += take;
                run -= take;
            }
            for _ in 0..run {
                out.push(ClSymbol {
                    sym: 0,
                    extra: 0,
                    extra_bits: 0,
                });
                i += 1;
            }
        } else {
            out.push(ClSymbol {
                sym: v,
                extra: 0,
                extra_bits: 0,
            });
            i += 1;
            run -= 1;
            while run >= 3 {
                let take = run.min(6);
                out.push(ClSymbol {
                    sym: 16,
                    extra: (take - 3) as u8,
                    extra_bits: 2,
                });
                i += take;
                run -= take;
            }
            for _ in 0..run {
                out.push(ClSymbol {
                    sym: v,
                    extra: 0,
                    extra_bits: 0,
                });
                i += 1;
            }
        }
    }
    out
}

/// A tokenised block, ready to be coded three ways.
#[derive(Debug)]
struct Block {
    tokens: Vec<Token>,
    // 286, not 288: DEFLATE caps the literal/length alphabet there.
    lit_freq: [u32; 286],
    dist_freq: [u32; 30],
    /// The block's original bytes, kept so a stored block is a memcpy.
    raw: Vec<u8>,
}

impl Block {
    fn new() -> Self {
        Self {
            tokens: Vec::new(),
            lit_freq: [0; 286],
            dist_freq: [0; 30],
            raw: Vec::new(),
        }
    }

    fn push_literal(&mut self, b: u8) {
        self.lit_freq[usize::from(b)] += 1;
        self.tokens.push(Token {
            len: None,
            dist: 0,
            lit: b,
        });
        self.raw.push(b);
    }

    fn push_match(&mut self, len: u16, dist: u16) {
        self.lit_freq[257 + length_code(usize::from(len))] += 1;
        self.dist_freq[dist_code(usize::from(dist))] += 1;
        self.tokens.push(Token {
            len: Some(len),
            dist,
            lit: 0,
        });
    }
}

/// A Huffman code: `(code, bit length)` pairs indexed by symbol.
type Code = Vec<(u32, u8)>;

/// Fixed-Huffman code tables, built once.
fn fixed_tables() -> (Code, Code) {
    let mut lengths = [0u8; 288];
    for (i, slot) in lengths.iter_mut().enumerate() {
        *slot = match i {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    (canonical(&lengths), canonical(&[5u8; 30]))
}

fn hash3(data: &[u8], i: usize) -> usize {
    let a = data.get(i).copied().unwrap_or(0) as usize;
    let b = data.get(i + 1).copied().unwrap_or(0) as usize;
    let c = data.get(i + 2).copied().unwrap_or(0) as usize;
    ((a << 10) ^ (b << 5) ^ c) & (HASH_SIZE - 1)
}

/// Compress `data` into a **zlib** stream (RFC 1950): a two-byte header, the RFC 1951
/// deflate data, and a four-byte Adler-32 of `data`.
///
/// This is what PDF's `/FlateDecode` names, so it is what every stream we write must be. A
/// bare deflate stream reads back in our own decoder and in nothing else, which is a file
/// nobody else can open. [`deflate_raw`] is the form to reach for when raw really is what
/// is wanted.
///
/// The header arithmetic and the checksum cannot fail, so this returns a `Vec` rather than
/// a `Result`; there is no error here to report.
#[must_use]
pub fn deflate(data: &[u8], level: DeflateLevel) -> Vec<u8> {
    let body = deflate_raw(data, level);
    let mut out = Vec::with_capacity(2 + body.len() + 4);
    out.extend_from_slice(&zlib::header());
    out.extend_from_slice(&body);
    out.extend_from_slice(&zlib::adler32(data).to_be_bytes());
    out
}

/// Compress `data` into a raw DEFLATE stream (RFC 1951): Huffman-coded blocks and nothing
/// else, with no zlib header and no trailing checksum. This is the form `/FlateDecode` does
/// *not* name; [`deflate`] is.
#[must_use]
pub fn deflate_raw(data: &[u8], level: DeflateLevel) -> Vec<u8> {
    let (max_chain, nice_len, lazy) = level.params();
    let mut bw = BitWriter::with_capacity(data.len() / 2 + 64);

    if data.is_empty() {
        write_stored(&mut bw, &[], true);
        return bw.finish();
    }

    let mut head = vec![u32::MAX; HASH_SIZE];
    let mut prev = vec![u32::MAX; data.len()];
    let mut block = Block::new();
    let mut pending: Option<(u16, u16)> = None;
    let mut pos = 0usize;

    while pos < data.len() {
        let mut best_len = 0usize;
        let mut best_dist = 0usize;
        let has_candidate = pos + MIN_MATCH <= data.len();
        if has_candidate {
            let h = hash3(data, pos);
            let limit = pos.saturating_sub(WINDOW);
            let mut cand = head[h];
            let mut chain = 0usize;
            let ceiling = (data.len() - pos).min(MAX_MATCH);
            while cand != u32::MAX && chain < max_chain {
                let c = cand as usize;
                if c < limit {
                    break;
                }
                chain += 1;
                if ceiling <= best_len {
                    break;
                }
                let mut l = 0usize;
                while l < ceiling && data.get(c + l) == data.get(pos + l) {
                    l += 1;
                }
                if l > best_len {
                    best_len = l;
                    best_dist = pos - c;
                    if l >= nice_len {
                        break;
                    }
                }
                cand = prev[c];
            }
            if let Some(slot) = prev.get_mut(pos) {
                *slot = head[h];
            }
            head[h] = u32::try_from(pos).unwrap_or(u32::MAX);
        }

        if best_len < MIN_MATCH {
            best_len = 0;
        }

        match pending.take() {
            None => {
                if best_len > 0 {
                    let len = u16::try_from(best_len).unwrap_or(0);
                    let dist = u16::try_from(best_dist).unwrap_or(0);
                    if lazy {
                        pending = Some((len, dist));
                        pos += 1;
                    } else {
                        push_match_with_raw(&mut block, data, pos, len, dist);
                        pos += best_len;
                    }
                } else {
                    if let Some(&b) = data.get(pos) {
                        block.push_literal(b);
                    }
                    pos += 1;
                }
            }
            Some((plen, pdist)) => {
                if best_len > usize::from(plen) {
                    // The deferred match lost; emit the byte it started on.
                    if let Some(&b) = data.get(pos - 1) {
                        block.push_literal(b);
                    }
                    pending = Some((
                        u16::try_from(best_len).unwrap_or(0),
                        u16::try_from(best_dist).unwrap_or(0),
                    ));
                    pos += 1;
                } else {
                    push_match_with_raw(&mut block, data, pos - 1, plen, pdist);
                    pos = (pos - 1) + usize::from(plen);
                }
            }
        }

        if block.tokens.len() >= BLOCK_TOKENS && pos < data.len() {
            write_block(&mut bw, &block, false);
            block = Block::new();
        }
    }
    if let Some((len, dist)) = pending {
        push_match_with_raw(&mut block, data, pos - 1, len, dist);
    }
    // The stream must end with a block whose BFINAL is 1, even if it carries no tokens.
    write_block(&mut bw, &block, true);
    bw.finish()
}

fn push_match_with_raw(block: &mut Block, data: &[u8], at: usize, len: u16, dist: u16) {
    let l = usize::from(len);
    // Copy the matched bytes verbatim; the stored-block path needs them and they also
    // keep `raw` an exact image of the input.
    let start = at.saturating_sub(usize::from(dist));
    for k in 0..l {
        if let Some(b) = data.get(start + k).copied() {
            block.raw.push(b);
        }
    }
    block.push_match(len, dist);
}

fn write_stored(bw: &mut BitWriter, raw: &[u8], last: bool) {
    bw.write_bits(u32::from(last), 1);
    bw.write_bits(0, 2);
    bw.align();
    let len = u16::try_from(raw.len().min(usize::from(u16::MAX))).unwrap_or(0);
    let nlen = !len;
    bw.write_bits(u32::from(len), 16);
    bw.write_bits(u32::from(nlen), 16);
    for b in raw {
        bw.write_bits(u32::from(*b), 8);
    }
}

fn token_bits(block: &Block, lit: &[(u32, u8)], dist: &[(u32, u8)], fixed: bool) -> usize {
    let mut bits = 0usize;
    for t in &block.tokens {
        match t.len {
            None => {
                if fixed {
                    bits += match usize::from(t.lit) {
                        0..=143 => 8,
                        144..=255 => 9,
                        _ => 8,
                    };
                } else {
                    bits += usize::from(lit.get(usize::from(t.lit)).map_or(0, |c| c.1));
                }
            }
            Some(l) => {
                let li = length_code(usize::from(l));
                let di = dist_code(usize::from(t.dist));
                let ll = if fixed {
                    7
                } else {
                    usize::from(lit.get(257 + li).map_or(0, |c| c.1))
                };
                let dl = if fixed {
                    5
                } else {
                    usize::from(dist.get(di).map_or(0, |c| c.1))
                };
                bits += ll + usize::from(LENGTH_EXTRA[li]) + dl + usize::from(DIST_EXTRA[di]);
            }
        }
    }
    bits + if fixed {
        7
    } else {
        usize::from(lit.get(256).map_or(0, |c| c.1))
    }
}

fn write_tokens(
    bw: &mut BitWriter,
    block: &Block,
    lit: &[(u32, u8)],
    dist: &[(u32, u8)],
    fixed: bool,
) {
    for t in &block.tokens {
        match t.len {
            None => {
                let sym = usize::from(t.lit);
                let (c, l) = lit.get(sym).copied().unwrap_or((0, 0));
                bw.write_code(c, l);
            }
            Some(l) => {
                let li = length_code(usize::from(l));
                let (c, cl) = lit.get(257 + li).copied().unwrap_or((0, 0));
                bw.write_code(c, cl);
                bw.write_bits(
                    u32::from(l) - u32::from(LENGTH_BASE[li]),
                    u32::from(LENGTH_EXTRA[li]),
                );
                let di = dist_code(usize::from(t.dist));
                let (dc, dl) = dist.get(di).copied().unwrap_or((0, 0));
                bw.write_code(dc, dl);
                bw.write_bits(
                    u32::from(t.dist) - u32::from(DIST_BASE[di]),
                    u32::from(DIST_EXTRA[di]),
                );
            }
        }
    }
    let eob = lit.get(256).copied().unwrap_or((0, 0));
    let _ = fixed;
    bw.write_code(eob.0, eob.1);
}

/// Emit one block, choosing the cheapest of stored / fixed / dynamic Huffman.
fn write_block(bw: &mut BitWriter, block: &Block, last: bool) {
    let mut lit_freq = block.lit_freq;
    lit_freq[256] += 1; // end-of-block is always present
    let mut dist_freq = block.dist_freq;
    // The dynamic header needs at least one distance code. Give unused slots a token
    // frequency of one so the tree is complete and strict decoders are satisfied.
    for f in &mut dist_freq {
        if *f == 0 {
            *f = 1;
        }
    }

    let lit_lengths = build_lengths(&lit_freq, MAX_BITS);
    let dist_lengths = build_lengths(&dist_freq, MAX_BITS);
    let lit_codes = canonical(&lit_lengths);
    let dist_codes = canonical(&dist_lengths);

    let mut all_lengths = lit_lengths.clone();
    all_lengths.extend_from_slice(&dist_lengths);
    let cl_seq = code_length_sequence(&all_lengths);
    let mut cl_freq = [0u32; 19];
    for s in &cl_seq {
        cl_freq[usize::from(s.sym)] += 1;
    }
    let cl_lengths = build_lengths(&cl_freq, 7);
    let cl_codes = canonical(&cl_lengths);
    let hclen = CLEN_ORDER
        .iter()
        .rposition(|&s| cl_lengths.get(s).copied().unwrap_or(0) != 0)
        .map_or(4, |p| p + 1)
        .clamp(4, 19);

    let hlit = lit_lengths.len();
    let hdist = dist_lengths.len();
    let cl_bits: usize = cl_seq
        .iter()
        .map(|s| {
            usize::from(cl_codes.get(usize::from(s.sym)).map_or(0, |c| c.1))
                + usize::from(s.extra_bits)
        })
        .sum();
    let dynamic_bits =
        3 + 5 + 5 + 4 + 3 * hclen + cl_bits + token_bits(block, &lit_codes, &dist_codes, false);
    let (fixed_lit, fixed_dist) = fixed_tables();
    let fixed_bits = 3 + token_bits(block, &fixed_lit, &fixed_dist, true);
    // A stored block is only usable when the block fits the 16-bit length field.
    let stored_bits = 3 + 7 + 32 + 8 * block.raw.len();
    let stored_ok = u16::try_from(block.raw.len()).is_ok();

    if stored_ok && stored_bits <= dynamic_bits.min(fixed_bits) {
        write_stored(bw, &block.raw, last);
    } else if fixed_bits <= dynamic_bits {
        bw.write_bits(u32::from(last), 1);
        bw.write_bits(1, 2);
        write_tokens(bw, block, &fixed_lit, &fixed_dist, true);
    } else {
        bw.write_bits(u32::from(last), 1);
        bw.write_bits(2, 2);
        bw.write_bits(hlit as u32 - 257, 5);
        bw.write_bits(hdist as u32 - 1, 5);
        bw.write_bits(hclen as u32 - 4, 4);
        for i in 0..hclen {
            bw.write_bits(u32::from(cl_lengths[CLEN_ORDER[i]]), 3);
        }
        for s in &cl_seq {
            let (c, l) = cl_codes.get(usize::from(s.sym)).copied().unwrap_or((0, 0));
            bw.write_code(c, l);
            bw.write_bits(u32::from(s.extra), u32::from(s.extra_bits));
        }
        write_tokens(bw, block, &lit_codes, &dist_codes, false);
    }
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect`, which is what a test is for; the
    // panic-free rule is about what the product does with a file, not about tests.
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::*;

    /// `deflate` writes a zlib stream, so the raw round trip uses `deflate_raw`; the wrapper
    /// is what the other tests in this module are about.
    fn round_trip(data: &[u8]) {
        let packed = deflate_raw(data, DeflateLevel::Default);
        let back = crate::inflate_raw(&packed, data.len());
        assert!(back.complete, "incomplete stream for {} bytes", data.len());
        assert_eq!(back.data, data);
    }

    #[test]
    fn empty_and_tiny() {
        round_trip(b"");
        round_trip(b"a");
        round_trip(b"ab");
        round_trip(b"abc");
    }

    #[test]
    fn text_like() {
        let text = b"the quick brown fox jumps over the lazy dog. ".repeat(40);
        round_trip(&text);
        let packed = deflate(&text, DeflateLevel::Default);
        assert!(
            packed.len() < text.len() / 4,
            "poor ratio: {}",
            packed.len()
        );
    }

    #[test]
    fn incompressible() {
        let mut x = 0x1234_5678u32;
        let data: Vec<u8> = (0..50_000)
            .map(|_| {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (x >> 24) as u8
            })
            .collect();
        round_trip(&data);
    }

    #[test]
    fn long_runs() {
        round_trip(&vec![0xABu8; 200_000]);
    }

    #[test]
    fn many_blocks() {
        let data: Vec<u8> = (0..200_000u32).map(|i| (i % 7) as u8).collect();
        round_trip(&data);
    }

    #[test]
    fn deterministic() {
        let data: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
        let a = deflate(&data, DeflateLevel::Default);
        let b = deflate(&data, DeflateLevel::Default);
        assert_eq!(a, b);
    }

    #[test]
    fn all_levels_round_trip() {
        let data: Vec<u8> = (0..40_000u32).map(|i| ((i / 97) % 251) as u8).collect();
        for level in [
            DeflateLevel::Fast,
            DeflateLevel::Default,
            DeflateLevel::Best,
        ] {
            let packed = deflate(&data, level);
            let back = crate::inflate(&packed, data.len());
            assert!(back.complete, "{level:?}");
            assert_eq!(back.data, data, "{level:?}");
        }
    }

    // ---- the zlib wrapper, which is what /FlateDecode actually means ---------------

    /// Adler-32 computed from the definition rather than by calling the implementation:
    /// two sums modulo 65521, the second accumulating the first, as `b << 16 | a`. Stating
    /// the rule here rather than comparing a stored constant means the test says *why* the
    /// last four bytes are what they are.
    fn reference_adler32(data: &[u8]) -> u32 {
        let (mut a, mut b) = (1u32, 0u32);
        for &byte in data {
            a = (a + u32::from(byte)) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }

    /// The header derived from RFC 1950 rather than hard-coded: DEFLATE in the low nibble
    /// of CMF, CINFO 7 for the 32 KiB window PDF requires, and FCHECK chosen so the pair
    /// read big-endian is a multiple of 31.
    ///
    /// FLEVEL is 2, "the compressor chose", which occupies the top two bits of FLG, so it
    /// is part of what FCHECK has to cancel: `31 - (CMF*256 + FLEVEL<<6) mod 31`.
    fn expected_header() -> [u8; 2] {
        let cmf = (7u8 << 4) | 8;
        let flevel = 2u16;
        let flg = (31 - (u16::from(cmf) * 256 + (flevel << 6)) % 31) % 31;
        [cmf, (flevel << 6) as u8 | flg as u8]
    }

    /// The output is what zlib says it is. Our own inflate gets the input back, the first
    /// two bytes are the CMF/FLG pair for a 32 KiB window, and the last four are the
    /// Adler-32 of the original. This is the assertion that would have failed before the
    /// wrapper existed, and the bug it catches is the one no round trip in this crate
    /// could see, because every other test inflated our own output with our own reader.
    #[test]
    fn the_output_is_a_zlib_stream_and_not_a_bare_deflate_one() {
        let data = b"the quick brown fox jumps over the lazy dog. ".repeat(40);
        let packed = deflate(&data, DeflateLevel::Default);

        let back = crate::inflate(&packed, data.len());
        assert!(back.complete, "{back:?}");
        assert_eq!(back.data, data);

        assert_eq!(packed.get(..2), Some(expected_header().as_slice()));
        assert_eq!(packed.get(..2), Some(&[0x78, 0x9c][..]));
        assert_eq!(
            packed.get(packed.len() - 4..),
            Some(reference_adler32(&data).to_be_bytes().as_slice())
        );
        // Between the two sits exactly the raw stream: the wrapper changes nothing else.
        assert_eq!(
            packed.get(2..packed.len() - 4),
            Some(deflate_raw(&data, DeflateLevel::Default).as_slice())
        );
    }

    /// A zlib stream from another producer must inflate here. The bytes are what Python's
    /// `zlib` writes for the same input, so the reader is proved against a writer that is
    /// not us. Ours happens to be byte-identical for this input, which is a stronger
    /// statement about the writer than it is about the reader, and the reader is the part
    /// that has to keep working when the other producer differs.
    #[test]
    fn a_zlib_stream_from_another_producer_inflates() {
        let want = b"Flate zlib interoperability.\n";
        let packed: [u8; 37] = [
            0x78, 0x9c, 0x73, 0xcb, 0x49, 0x2c, 0x49, 0x55, 0xa8, 0xca, 0xc9, 0x4c, 0x52, 0xc8,
            0xcc, 0x2b, 0x49, 0x2d, 0xca, 0x2f, 0x48, 0x2d, 0x4a, 0x4c, 0xca, 0xcc, 0xc9, 0x2c,
            0xa9, 0xd4, 0xe3, 0x02, 0x00, 0xa4, 0xf1, 0x0a, 0xdc,
        ];
        let r = crate::inflate(&packed, want.len());
        assert!(r.complete, "{r:?}");
        assert_eq!(r.data, want);
    }

    /// A bare deflate stream still decodes. Files in the wild carry one where the
    /// specification says there should be a wrapper, and refusing them loses a page.
    #[test]
    fn a_bare_deflate_stream_still_inflates() {
        let data = b"no wrapper here, which is what a damaged file looks like.\n".repeat(50);
        let packed = deflate_raw(&data, DeflateLevel::Default);
        let r = crate::inflate(&packed, data.len());
        assert!(r.complete, "{r:?}");
        assert_eq!(r.data, data);
    }

    /// Empty input is a real case a compressor meets — a content stream can be empty — and
    /// it is the one where the wrapper is most of the output. The Adler-32 of nothing is
    /// the initial value of `a`, which is 1, and the deflate data is one empty stored
    /// block: a header byte, then LEN and NLEN as zero and its complement.
    #[test]
    fn empty_input_still_produces_a_valid_zlib_stream() {
        let packed = deflate(b"", DeflateLevel::Default);
        let r = crate::inflate(&packed, 0);
        assert!(r.complete, "{r:?}");
        assert!(r.data.is_empty());
        assert_eq!(packed.get(..2), Some(expected_header().as_slice()));
        assert_eq!(
            packed.get(2..),
            Some(&[0x01, 0x00, 0x00, 0xff, 0xff, 0, 0, 0, 1][..])
        );
    }

    /// Many blocks. `BLOCK_TOKENS` is 1 << 14, so this crosses dozens of block boundaries,
    /// each of which is an opportunity for the last block's BFINAL to go missing, and the
    /// checksum then covers a stream that never ends.
    #[test]
    fn a_large_input_spanning_many_blocks_round_trips() {
        let data: Vec<u8> = (0..400_000u32).map(|i| (i % 7) as u8).collect();
        let packed = deflate(&data, DeflateLevel::Default);
        let r = crate::inflate(&packed, data.len());
        assert!(r.complete, "{r:?}");
        assert_eq!(r.data, data);
        // The wrapper is six bytes whatever the size, and the header does not vary.
        assert_eq!(packed.get(..2), Some(expected_header().as_slice()));
        assert_eq!(
            packed.get(packed.len() - 4..),
            Some(reference_adler32(&data).to_be_bytes().as_slice())
        );
    }

    /// A corrupted payload is reported, not returned as if it were good. A reader that
    /// ignores the checksum would quietly hand back a wrong page, which is worse than
    /// handing back a right page with a note against it.
    #[test]
    fn a_damaged_payload_is_reported_rather_than_returned_as_good() {
        let data = b"checksums exist so this cannot pass unnoticed.\n".repeat(30);
        let mut packed = deflate(&data, DeflateLevel::Default);
        let last = packed.len() - 1;
        if let Some(b) = packed.get_mut(last) {
            *b ^= 0xff;
        }
        let r = crate::inflate(&packed, data.len());
        assert!(!r.complete, "a bad checksum must not read as complete");
        // The data is still there: a damaged stream shows what it does contain.
        assert_eq!(r.data, data);
        assert!(
            r.note.as_ref().is_some_and(|n| n.contains("checksum")),
            "{r:?}"
        );
    }
}
