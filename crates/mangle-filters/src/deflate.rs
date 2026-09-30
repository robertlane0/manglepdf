//! DEFLATE (RFC 1951) compression.
//!
//! In house for the same reason as `inflate`: we control the byte-for-byte output so
//! that saving the same document twice produces the same file, and so that a stream we
//! wrote is decoded by our own partial-tolerant reader.
//!
//! One block is emitted per ~16k tokens, choosing whichever of stored / fixed / dynamic
//! Huffman is smallest. The result is deterministic: no timing, no randomness.

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
            return lens.into_iter().map(|l| l as u8).collect();
        }
        // Too deep: flatten the distribution and retry. Converges in a few rounds.
        for f in work.iter_mut() {
            *f = (*f + 1) / 2;
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
            (None, Some((l, r))) => {
                if depth < 32 {
                    stack.push((l, depth + 1));
                    stack.push((r, depth + 1));
                }
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

    fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }
}

/// Fixed-Huffman code tables, built once.
fn fixed_tables() -> (Vec<(u32, u8)>, Vec<(u32, u8)>) {
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

/// Compress `data` into a raw DEFLATE stream (no zlib or gzip wrapper).
#[must_use]
pub fn deflate(data: &[u8], level: DeflateLevel) -> Vec<u8> {
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
                let dl = if fixed { 5 } else { usize::from(dist.get(di).map_or(0, |c| c.1)) };
                bits += ll + usize::from(LENGTH_EXTRA[li]) + dl + usize::from(DIST_EXTRA[di]);
            }
        }
    }
    bits + if fixed { 7 } else { usize::from(lit.get(256).map_or(0, |c| c.1)) }
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
    for f in dist_freq.iter_mut() {
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
        .max(4)
        .min(19);

    let hlit = lit_lengths.len();
    let hdist = dist_lengths.len();
    let cl_bits: usize = cl_seq
        .iter()
        .map(|s| usize::from(cl_codes.get(usize::from(s.sym)).map_or(0, |c| c.1)) + usize::from(s.extra_bits))
        .sum();
    let dynamic_bits = 3 + 5 + 5 + 4 + 3 * hclen + cl_bits + token_bits(block, &lit_codes, &dist_codes, false);
    let (fixed_lit, fixed_dist) = fixed_tables();
    let fixed_bits = 3 + token_bits(block, &fixed_lit, &fixed_dist, true);
    // A stored block is only usable when the block fits the 16-bit length field.
    let stored_bits = 3 + 7 + 32 + 8 * block.raw.len();
    let stored_ok = block.raw.len() <= usize::from(u16::MAX);

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
    use super::*;

    fn round_trip(data: &[u8]) {
        let packed = deflate(data, DeflateLevel::Default);
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
        assert!(packed.len() < text.len() / 4, "poor ratio: {}", packed.len());
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
        for level in [DeflateLevel::Fast, DeflateLevel::Default, DeflateLevel::Best] {
            let packed = deflate(&data, level);
            let back = crate::inflate_raw(&packed, data.len());
            assert!(back.complete, "{level:?}");
            assert_eq!(back.data, data, "{level:?}");
        }
    }
}

