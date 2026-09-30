//! SHA-1 and SHA-256, plus the SHA-384/512 truncation and iteration helpers the R6
//! handler needs. These are thin wrappers over the `sha2` crate's compression function
//! usage, kept separate so the rest of the crate does not care.

use sha2::Digest as _;

/// SHA-256.
#[derive(Debug, Clone, Default)]
pub struct Sha256 {
    inner: sha2::Sha256,
}

impl Sha256 {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: sha2::Sha256::new(),
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }

    #[must_use]
    pub fn finish(self) -> [u8; 32] {
        self.inner.finalize().into()
    }
}

/// One-shot SHA-256.
#[must_use]
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(data);
    h.finish()
}

/// SHA-384.
#[derive(Debug, Clone, Default)]
pub struct Sha384 {
    inner: sha2::Sha384,
}

impl Sha384 {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: sha2::Sha384::new(),
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }

    #[must_use]
    pub fn finish(self) -> [u8; 48] {
        self.inner.finalize().into()
    }
}

/// One-shot SHA-384.
#[must_use]
pub fn sha384(data: &[u8]) -> [u8; 48] {
    let mut h = Sha384::new();
    h.update(data);
    h.finish()
}

/// SHA-512.
#[derive(Debug, Clone, Default)]
pub struct Sha512 {
    inner: sha2::Sha512,
}

impl Sha512 {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: sha2::Sha512::new(),
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }

    #[must_use]
    pub fn finish(self) -> [u8; 64] {
        self.inner.finalize().into()
    }
}

/// One-shot SHA-512.
#[must_use]
pub fn sha512(data: &[u8]) -> [u8; 64] {
    let mut h = Sha512::new();
    h.update(data);
    h.finish()
}

/// SHA-1 (FIPS 180-4). Only needed to read legacy signatures and `/Perms` digests.
#[derive(Debug, Clone)]
pub struct Sha1 {
    state: [u32; 5],
    buffer: [u8; 64],
    buffered: usize,
    length: u64,
}

impl Default for Sha1 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha1 {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0],
            buffer: [0; 64],
            buffered: 0,
            length: 0,
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u64);
        let mut i = 0usize;
        if self.buffered > 0 {
            let take = (64 - self.buffered).min(data.len());
            if let Some(dst) = self.buffer.get_mut(self.buffered..self.buffered + take) {
                dst.copy_from_slice(&data[..take]);
            }
            self.buffered += take;
            i = take;
            if self.buffered == 64 {
                let block = self.buffer;
                self.compress(&block);
                self.buffered = 0;
            }
        }
        while i + 64 <= data.len() {
            if let Some(block) = data.get(i..i + 64) {
                let mut b = [0u8; 64];
                b.copy_from_slice(block);
                self.compress(&b);
            }
            i += 64;
        }
        if i < data.len() {
            let rest = data.len() - i;
            if let Some(dst) = self.buffer.get_mut(self.buffered..self.buffered + rest) {
                dst.copy_from_slice(&data[i..]);
            }
            self.buffered += rest;
        }
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = self.state;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i / 20 {
                0 => ((b & c) | (!b & d), 0x5A827999u32),
                1 => (b ^ c ^ d, 0x6ED9EBA1),
                2 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let tmp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = tmp;
        }
        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
    }

    #[must_use]
    pub fn finish(mut self) -> [u8; 20] {
        let bit_len = self.length.wrapping_mul(8);
        let mut block = self.buffer;
        let mut n = self.buffered;
        block[n] = 0x80;
        n += 1;
        if n > 56 {
            for slot in block.iter_mut().skip(n) {
                *slot = 0;
            }
            self.compress(&block);
            n = 0;
            block = [0; 64];
        }
        for slot in block.iter_mut().take(56).skip(n) {
            *slot = 0;
        }
        block[56..64].copy_from_slice(&bit_len.to_be_bytes());
        self.compress(&block);
        let mut out = [0u8; 20];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = (self.state[i / 4] >> (24 - 8 * (i % 4))) as u8;
        }
        out
    }
}

/// One-shot SHA-1.
#[must_use]
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h = Sha1::new();
    h.update(data);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(d: &[u8]) -> String {
        d.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn sha1_vectors() {
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(hex(&sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            hex(&sha1(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }

    #[test]
    fn sha2_vectors() {
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(&sha384(b"abc")),
            "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7"
        );
        assert_eq!(hex(&sha512(b"abc")).len(), 128);
    }

    #[test]
    fn sha1_incremental() {
        let data: Vec<u8> = (0..3000u32).map(|i| (i % 256) as u8).collect();
        let mut h = Sha1::new();
        for part in data.chunks(13) {
            h.update(part);
        }
        assert_eq!(h.finish(), sha1(&data));
    }
}

// -------------------------------------------------------------------------------------
// MD5 (RFC 1321), needed by the revision 2 to 4 handlers.
// -------------------------------------------------------------------------------------

const MD5_S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

const MD5_K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];

/// MD5, as specified in RFC 1321. Only used because the PDF specification requires it.
#[derive(Debug, Clone)]
pub struct Md5 {
    state: [u32; 4],
    buffer: [u8; 64],
    buffered: usize,
    length: u64,
}

impl Default for Md5 {
    fn default() -> Self {
        Self::new()
    }
}

impl Md5 {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476],
            buffer: [0; 64],
            buffered: 0,
            length: 0,
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u64);
        let mut i = 0usize;
        if self.buffered > 0 {
            let take = (64 - self.buffered).min(data.len());
            if let Some(dst) = self.buffer.get_mut(self.buffered..self.buffered + take) {
                dst.copy_from_slice(&data[..take]);
            }
            self.buffered += take;
            i = take;
            if self.buffered == 64 {
                let block = self.buffer;
                self.compress(&block);
                self.buffered = 0;
            }
        }
        while i + 64 <= data.len() {
            if let Some(block) = data.get(i..i + 64) {
                let mut b = [0u8; 64];
                b.copy_from_slice(block);
                self.compress(&b);
            }
            i += 64;
        }
        if i < data.len() {
            let rest = data.len() - i;
            if let Some(dst) = self.buffer.get_mut(self.buffered..self.buffered + rest) {
                dst.copy_from_slice(&data[i..]);
            }
            self.buffered += rest;
        }
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut m = [0u32; 16];
        for (i, slot) in m.iter_mut().enumerate() {
            *slot = u32::from_le_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        let [mut a, mut b, mut c, mut d] = self.state;
        for i in 0..64usize {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let tmp = d;
            d = c;
            c = b;
            let sum = a
                .wrapping_add(f)
                .wrapping_add(MD5_K[i])
                .wrapping_add(m[g]);
            b = b.wrapping_add(sum.rotate_left(MD5_S[i]));
            a = tmp;
        }
        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
    }

    #[must_use]
    pub fn finish(mut self) -> [u8; 16] {
        let bit_len = self.length.wrapping_mul(8);
        // Pad by hand: routing the padding through `update` would compress the block
        // that fills the buffer and then try to fill it again.
        let mut block = self.buffer;
        let mut n = self.buffered;
        block[n] = 0x80;
        n += 1;
        if n > 56 {
            for slot in block.iter_mut().skip(n) {
                *slot = 0;
            }
            self.compress(&block);
            n = 0;
            block = [0; 64];
        }
        for slot in block.iter_mut().take(56).skip(n) {
            *slot = 0;
        }
        block[56..64].copy_from_slice(&bit_len.to_le_bytes());
        self.compress(&block);
        let mut out = [0u8; 16];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = (self.state[i / 4] >> (8 * (i % 4))) as u8;
        }
        out
    }
}

/// One-shot MD5.
#[must_use]
pub fn md5(data: &[u8]) -> [u8; 16] {
    let mut h = Md5::new();
    h.update(data);
    h.finish()
}

#[cfg(test)]
mod md5_tests {
    use super::md5;

    fn hex(d: &[u8]) -> String {
        d.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn rfc1321_vectors() {
        assert_eq!(hex(&md5(b"")), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hex(&md5(b"abc")), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            hex(&md5(b"message digest")),
            "f96b697d7cb7938d525a2f31aaf161d0"
        );
    }
}

