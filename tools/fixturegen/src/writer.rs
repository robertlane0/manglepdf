//! A minimal PDF writer, written from the specification.
//!
//! `fixturegen` deliberately shares nothing with the product: a fixture built by the
//! same misunderstanding as the parser proves nothing. That means the cross-reference
//! tables, object streams, incremental revisions and damaged variants are all
//! assembled here by hand, from the same description ISO 32000-1 gives.
//!
//! Everything is deterministic. No clock, no random source, no hash-order iteration:
//! the same seed and the same version produce the same bytes, which is what
//! `cargo xtask fixtures --check` relies on.
//!
//! Indexing is used throughout, into buffers this file itself just built and whose
//! lengths every loop condition checks. A generator that cannot index its own output
//! would be harder to read than one that can.
#![allow(clippy::indexing_slicing)]

use std::fmt::Write as _;

/// Append formatted text to a byte buffer.
macro_rules! put {
    ($out:expr, $($arg:tt)*) => {
        $out.extend_from_slice(format!($($arg)*).as_bytes())
    };
}

/// One indirect object under construction.
pub(crate) struct Obj {
    num: u32,
    body: Vec<u8>,
}

impl Obj {
    /// A dictionary-only object.
    pub(crate) fn dict(num: u32, entries: &[(&str, &str)]) -> Self {
        let mut body = format!("{num} 0 obj\n<< ");
        for (i, (k, v)) in entries.iter().enumerate() {
            if i > 0 {
                body.push(' ');
            }
            let _ = write!(body, "/{k} {v}");
        }
        body.push_str(" >>\nendobj\n");
        Self {
            num,
            body: body.into_bytes(),
        }
    }

    /// An object with a free-form body, for the damaged variants.
    pub(crate) fn raw(num: u32, body: impl Into<Vec<u8>>) -> Self {
        Self {
            num,
            body: body.into(),
        }
    }

    /// A stream object. `dict_entries` may add `/Length` itself; if it does not, the
    /// writer computes the correct one.
    pub(crate) fn stream(num: u32, dict_entries: &[(&str, &str)], data: &[u8]) -> Self {
        let mut body = format!("{num} 0 obj\n<< ");
        for (i, (k, v)) in dict_entries.iter().enumerate() {
            if i > 0 {
                body.push(' ');
            }
            let _ = write!(body, "/{k} {v}");
        }
        if !dict_entries.iter().any(|(k, _)| *k == "Length") {
            let _ = write!(body, "/Length {} >>", data.len());
        } else {
            body.push_str(" >>");
        }
        body.push_str("\nstream\n");
        let mut out = body.into_bytes();
        out.extend_from_slice(data);
        out.extend_from_slice(b"\nendstream\nendobj\n");
        Self { num, body: out }
    }

    /// A deliberately wrong `/Length`, which is what `F08` needs.
    pub(crate) fn stream_with_length(num: u32, declared: usize, data: &[u8]) -> Self {
        let mut body = format!("{num} 0 obj\n<< /Length {declared} >>\nstream\n").into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(b"\nendstream\nendobj\n");
        Self { num, body }
    }

    fn num(&self) -> u32 {
        self.num
    }
}

/// A document being assembled: the objects, plus where they landed.
pub(crate) struct Builder {
    header: String,
    objects: Vec<Obj>,
    /// Bytes that go before the first object, for the "700 bytes of garbage" fixture.
    preamble: Vec<u8>,
    /// Bytes that go after `%%EOF`.
    trailer_junk: Vec<u8>,
    /// Force the first revision, for a linearised-looking file.
    linearised: bool,
}

impl Builder {
    pub(crate) fn new(version: &str) -> Self {
        Self {
            header: format!("%PDF-{version}\n"),
            objects: Vec::new(),
            preamble: Vec::new(),
            trailer_junk: Vec::new(),
            linearised: false,
        }
    }

    pub(crate) fn with_preamble(mut self, bytes: &[u8]) -> Self {
        self.preamble = bytes.to_vec();
        self
    }

    pub(crate) fn with_trailer_junk(mut self, bytes: &[u8]) -> Self {
        self.trailer_junk = bytes.to_vec();
        self
    }

    pub(crate) fn linearised(mut self) -> Self {
        self.linearised = true;
        self
    }

    pub(crate) fn push(&mut self, obj: Obj) -> &mut Self {
        self.objects.push(obj);
        self
    }

    /// Write the file with a classic cross-reference table.
    pub(crate) fn build(&self, root: u32, extra_trailer: &str) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(self.header.as_bytes());
        if self.linearised {
            // A linearised file's first cross-reference is not the one at the end.
            out.extend_from_slice(b"%Linearized\n");
        }
        out.extend_from_slice(&self.preamble);
        let mut offsets: Vec<(u32, usize)> = Vec::new();
        for obj in &self.objects {
            offsets.push((obj.num(), out.len()));
            out.extend_from_slice(&obj.body);
        }
        let size = offsets.iter().map(|(n, _)| *n + 1).max().unwrap_or(1);

        let xref = out.len();
        out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n");
        // One subsection per contiguous run, which is what every producer writes.
        offsets.sort_by_key(|(n, _)| *n);
        let mut i = 0;
        while i < offsets.len() {
            let start_num = offsets[i].0;
            let mut j = i;
            while j + 1 < offsets.len() && offsets[j + 1].0 == offsets[j].0 + 1 {
                j += 1;
            }
            put!(out, "{} {}\n", start_num, j - i + 1);
            for (_, off) in &offsets[i..=j] {
                put!(out, "{off:010} 00000 n \n");
            }
            i = j + 1;
        }
        put!(
            out,
            "trailer\n<< /Size {size} /Root {root} 0 R {extra_trailer} >>\n"
        );
        put!(out, "startxref\n{xref}\n%%EOF\n");
        out.extend_from_slice(&self.trailer_junk);
        out
    }

    /// Write a hybrid file: a classic table, then an object and a cross-reference
    /// stream that the table does not mention, and one trailer pointing at both.
    ///
    /// The order matters and is the whole point of the layout: a reader that only
    /// understands the table still finds the catalogue, and a reader that reads
    /// `/XRefStm` finds the object the table omits.
    pub(crate) fn build_hybrid(&self, root: u32, extra_trailer: &str) -> Vec<u8> {
        // The two objects the classic table will not mention, numbered straight on from
        // the ones it has. A hybrid file is contiguous: an object described by neither
        // the table nor the stream is missing, not hybrid.
        let extra_num = self.objects.iter().map(|o| o.num() + 1).max().unwrap_or(1);
        let payload = b"<< /Nums [0 << /S /r >>] >>";
        let label_offset = self.total_len();

        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(self.header.as_bytes());
        out.extend_from_slice(&self.preamble);
        let mut offsets: Vec<(u32, usize)> = Vec::new();
        for obj in &self.objects {
            offsets.push((obj.num(), out.len()));
            out.extend_from_slice(&obj.body);
        }
        out.extend_from_slice(
            format!(
                "{extra_num} 0 obj\n<< /Type /PageLabels /Length {} >>\nstream\n",
                payload.len()
            )
            .as_bytes(),
        );
        out.extend_from_slice(payload);
        out.extend_from_slice(b"\nendstream\nendobj\n");

        // A cross-reference stream is a flat array with /W giving each field's width,
        // and /Index selecting which object numbers are present. The stream describes
        // itself too, which is the convention every producer follows.
        let xref_num = extra_num + 1;
        let xref_offset = out.len();
        let mut body: Vec<u8> = Vec::new();
        for offset in [label_offset as u64, xref_offset as u64] {
            body.push(1);
            body.extend_from_slice(&offset.to_be_bytes());
            body.extend_from_slice(&0u16.to_be_bytes());
        }

        out.extend_from_slice(
            format!(
                "{xref_num} 0 obj\n<< /Type /XRef /Size {} /W [1 8 2] \
                 /Index [{extra_num} 2] /Length {} >>\nstream\n",
                xref_num + 1,
                body.len()
            )
            .as_bytes(),
        );
        out.extend_from_slice(&body);
        out.extend_from_slice(b"\nendstream\nendobj\n");

        // `/Size` must cover every object the file contains, including the two the
        // stream added, or a reader counts objects and disagrees.
        let size = offsets
            .iter()
            .map(|(n, _)| *n + 1)
            .max()
            .unwrap_or(1)
            .max(extra_num + 2);
        let xref = out.len();
        out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n");
        offsets.sort_by_key(|(n, _)| *n);
        let mut i = 0;
        while i < offsets.len() {
            let start_num = offsets[i].0;
            let mut j = i;
            while j + 1 < offsets.len() && offsets[j + 1].0 == offsets[j].0 + 1 {
                j += 1;
            }
            put!(out, "{} {}\n", start_num, j - i + 1);
            for (_, off) in &offsets[i..=j] {
                put!(out, "{off:010} 00000 n \n");
            }
            i = j + 1;
        }
        put!(
            out,
            "trailer\n<< /Size {size} /Root {root} 0 R {extra_trailer} /XRefStm {xref_offset} >>\n"
        );
        put!(out, "startxref\n{xref}\n%%EOF\n");
        out.extend_from_slice(&self.trailer_junk);
        out
    }

    /// How long the file will be before anything is appended.
    pub(crate) fn total_len(&self) -> usize {
        let mut pos = self.header.len() + self.preamble.len();
        if self.linearised {
            pos += "%Linearized\n".len();
        }
        for obj in &self.objects {
            pos += obj.body.len();
        }
        pos
    }

    /// Write the file with offsets that are deliberately wrong, for `F06`.
    pub(crate) fn build_with_bad_offsets(&self, root: u32, extra_trailer: &str) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(self.header.as_bytes());
        out.extend_from_slice(&self.preamble);
        for obj in &self.objects {
            out.extend_from_slice(&obj.body);
        }
        let size = self.objects.iter().map(|o| o.num() + 1).max().unwrap_or(1);
        let xref = out.len();
        out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n");
        let mut sorted: Vec<&Obj> = self.objects.iter().collect();
        sorted.sort_by_key(|o| o.num());
        let mut i = 0;
        while i < sorted.len() {
            let start_num = sorted[i].num();
            let mut j = i;
            while j + 1 < sorted.len() && sorted[j + 1].num() == sorted[j].num() + 1 {
                j += 1;
            }
            put!(out, "{} {}\n", start_num, j - i + 1);
            for obj in sorted.get(i..=j).unwrap_or_default() {
                // Wrong by a few bytes, which is what a producer with a line-ending
                // mismatch produces.
                let wrong = obj.num() as usize * 3;
                put!(out, "{wrong:010} 00000 n \n");
            }
            i = j + 1;
        }
        put!(
            out,
            "trailer\n<< /Size {size} /Root {root} 0 R {extra_trailer} >>\n"
        );
        put!(out, "startxref\n{xref}\n%%EOF\n");
        out
    }

    /// The object bodies without any cross-reference, for `F07`.
    pub(crate) fn build_without_trailer(&self, keep_percent: usize) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(self.header.as_bytes());
        out.extend_from_slice(&self.preamble);
        for obj in &self.objects {
            out.extend_from_slice(&obj.body);
        }
        let keep = out.len() * keep_percent / 100;
        out.truncate(keep);
        out
    }

    /// Where each object lands in the finished file, for a fixture that has to point
    /// at its own objects afterwards, and as a self-check on the builder.
    pub(crate) fn offsets(&self) -> Vec<(u32, usize)> {
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(self.header.as_bytes());
        if self.linearised {
            out.extend_from_slice(b"%Linearized\n");
        }
        out.extend_from_slice(&self.preamble);
        let mut offsets = Vec::new();
        for obj in &self.objects {
            offsets.push((obj.num(), out.len()));
            out.extend_from_slice(&obj.body);
        }
        offsets
    }
}

/// SHA-256, so the manifest can record a hash without pulling in a dependency.
pub(crate) fn sha256_hex(data: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(64);
    for byte in sha256(data) {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// The compression function's working variables are named as the specification names
/// them, so this reads like FIPS 180-4.
#[allow(clippy::many_single_char_names)]
fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for block in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, value) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(value);
        }
    }

    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}
