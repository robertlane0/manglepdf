//! AES-128 and AES-256 in CBC mode with PKCS#7 padding, as PDF uses them.
//!
//! In house so that the R5/R6 key wrap, the CBC IV handling, and the tolerant
//! decrypt-then-ignore-padding behaviour are all under our control.

const SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab, 0x76,
    0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4, 0x72, 0xc0,
    0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71, 0xd8, 0x31, 0x15,
    0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2, 0xeb, 0x27, 0xb2, 0x75,
    0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6, 0xb3, 0x29, 0xe3, 0x2f, 0x84,
    0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb, 0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf,
    0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45, 0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8,
    0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5, 0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2,
    0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44, 0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73,
    0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a, 0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb,
    0xe0, 0x32, 0x3a, 0x0a, 0x49, 0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79,
    0xe7, 0xc8, 0x37, 0x6d, 0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08,
    0xba, 0x78, 0x25, 0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a,
    0x70, 0x3e, 0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e,
    0xe1, 0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb, 0x16,
];

fn inv_sbox() -> &'static [u8; 256] {
    use std::sync::OnceLock;
    static INV: OnceLock<[u8; 256]> = OnceLock::new();
    INV.get_or_init(|| {
        let mut inv = [0u8; 256];
        for (i, &v) in SBOX.iter().enumerate() {
            inv[usize::from(v)] = u8::try_from(i).unwrap_or(0);
        }
        inv
    })
}

const RCON: [u8; 11] = [
    0x00, 0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36,
];

fn xtime(b: u8) -> u8 {
    (b << 1) ^ if b & 0x80 != 0 { 0x1b } else { 0x00 }
}

fn gmul(a: u8, b: u8) -> u8 {
    let mut a = a;
    let mut b = b;
    let mut p = 0u8;
    for _ in 0..8 {
        if b & 1 != 0 {
            p ^= a;
        }
        a = xtime(a);
        b >>= 1;
    }
    p
}

/// AES-128 or AES-256, encrypt only. Decryption shares the key schedule.
#[derive(Debug, Clone)]
struct Aes {
    round_keys: Vec<[u8; 16]>,
    rounds: usize,
}

impl Aes {
    fn new(key: &[u8]) -> Option<Self> {
        let (nk, rounds) = match key.len() {
            16 => (4usize, 10usize),
            24 => (6, 12),
            32 => (8, 14),
            _ => return None,
        };
        let total_words = 4 * (rounds + 1);
        let mut w = vec![[0u8; 4]; total_words];
        for i in 0..nk {
            w[i].copy_from_slice(key.get(i * 4..i * 4 + 4)?);
        }
        for i in nk..total_words {
            let mut temp = w[i - 1];
            if i % nk == 0 {
                temp.rotate_left(1);
                for b in temp.iter_mut() {
                    *b = SBOX[usize::from(*b)];
                }
                temp[0] ^= RCON[i / nk];
            } else if nk > 6 && i % nk == 4 {
                for b in temp.iter_mut() {
                    *b = SBOX[usize::from(*b)];
                }
            }
            for j in 0..4 {
                w[i][j] = w[i - nk][j] ^ temp[j];
            }
        }
        let mut round_keys = Vec::with_capacity(rounds + 1);
        for r in 0..=rounds {
            let mut rk = [0u8; 16];
            for c in 0..4 {
                rk[c * 4..c * 4 + 4].copy_from_slice(&w[r * 4 + c]);
            }
            round_keys.push(rk);
        }
        Some(Self { round_keys, rounds })
    }

    fn encrypt_block(&self, block: &mut [u8; 16]) {
        add_round_key(block, &self.round_keys[0]);
        for r in 1..self.rounds {
            sub_bytes(block);
            shift_rows(block);
            mix_columns(block);
            add_round_key(block, &self.round_keys[r]);
        }
        sub_bytes(block);
        shift_rows(block);
        add_round_key(block, &self.round_keys[self.rounds]);
    }

    fn decrypt_block(&self, block: &mut [u8; 16]) {
        add_round_key(block, &self.round_keys[self.rounds]);
        for r in (1..self.rounds).rev() {
            inv_shift_rows(block);
            inv_sub_bytes(block);
            add_round_key(block, &self.round_keys[r]);
            inv_mix_columns(block);
        }
        inv_shift_rows(block);
        inv_sub_bytes(block);
        add_round_key(block, &self.round_keys[0]);
    }
}

fn add_round_key(state: &mut [u8; 16], rk: &[u8; 16]) {
    for i in 0..16 {
        state[i] ^= rk[i];
    }
}

fn sub_bytes(state: &mut [u8; 16]) {
    for b in state.iter_mut() {
        *b = SBOX[usize::from(*b)];
    }
}

fn inv_sub_bytes(state: &mut [u8; 16]) {
    let inv = inv_sbox();
    for b in state.iter_mut() {
        *b = inv[usize::from(*b)];
    }
}

fn shift_rows(state: &mut [u8; 16]) {
    let s = *state;
    for r in 1..4 {
        for c in 0..4 {
            state[c * 4 + r] = s[((c + r) % 4) * 4 + r];
        }
    }
}

fn inv_shift_rows(state: &mut [u8; 16]) {
    let s = *state;
    for r in 1..4 {
        for c in 0..4 {
            state[((c + r) % 4) * 4 + r] = s[c * 4 + r];
        }
    }
}

fn mix_columns(state: &mut [u8; 16]) {
    for c in 0..4 {
        let a = [
            state[c * 4],
            state[c * 4 + 1],
            state[c * 4 + 2],
            state[c * 4 + 3],
        ];
        state[c * 4] = gmul(a[0], 2) ^ gmul(a[1], 3) ^ a[2] ^ a[3];
        state[c * 4 + 1] = a[0] ^ gmul(a[1], 2) ^ gmul(a[2], 3) ^ a[3];
        state[c * 4 + 2] = a[0] ^ a[1] ^ gmul(a[2], 2) ^ gmul(a[3], 3);
        state[c * 4 + 3] = gmul(a[0], 3) ^ a[1] ^ a[2] ^ gmul(a[3], 2);
    }
}

fn inv_mix_columns(state: &mut [u8; 16]) {
    for c in 0..4 {
        let a = [
            state[c * 4],
            state[c * 4 + 1],
            state[c * 4 + 2],
            state[c * 4 + 3],
        ];
        state[c * 4] = gmul(a[0], 14) ^ gmul(a[1], 11) ^ gmul(a[2], 13) ^ gmul(a[3], 9);
        state[c * 4 + 1] = gmul(a[0], 9) ^ gmul(a[1], 14) ^ gmul(a[2], 11) ^ gmul(a[3], 13);
        state[c * 4 + 2] = gmul(a[0], 13) ^ gmul(a[1], 9) ^ gmul(a[2], 14) ^ gmul(a[3], 11);
        state[c * 4 + 3] = gmul(a[0], 11) ^ gmul(a[1], 13) ^ gmul(a[2], 9) ^ gmul(a[3], 14);
    }
}

/// AES-CBC encryption with PKCS#7 padding and a random-looking IV prefix.
#[derive(Debug, Clone)]
pub struct AesEncryptor {
    aes: Aes,
    /// The last ciphertext block, carried into the next call.
    chain: [u8; 16],
}

impl AesEncryptor {
    #[must_use]
    pub fn new(key: &[u8], iv: &[u8; 16]) -> Option<Self> {
        Some(Self {
            aes: Aes::new(key)?,
            chain: *iv,
        })
    }

    /// Encrypt `data` (already a whole number of blocks; padding is added by the caller).
    pub fn apply(&mut self, data: &mut [u8]) {
        for chunk in data.chunks_mut(16) {
            if chunk.len() < 16 {
                break;
            }
            for i in 0..16 {
                chunk[i] ^= self.chain[i];
            }
            let mut block = [0u8; 16];
            block.copy_from_slice(chunk);
            self.aes.encrypt_block(&mut block);
            chunk.copy_from_slice(&block);
            self.chain = block;
        }
    }
}

/// AES-CBC decryption. Padding is *not* stripped automatically: a corrupt final block
/// must still yield its earlier plaintext, which is what a damaged PDF needs.
#[derive(Debug, Clone)]
pub struct AesDecryptor {
    aes: Aes,
    chain: [u8; 16],
}

impl AesDecryptor {
    #[must_use]
    pub fn new(key: &[u8], iv: &[u8; 16]) -> Option<Self> {
        Some(Self {
            aes: Aes::new(key)?,
            chain: *iv,
        })
    }

    /// Decrypt with an explicit key.
    #[must_use]
    pub fn with_key(key: &[u8], data: &[u8]) -> Option<Vec<u8>> {
        if data.len() < 32 {
            return None;
        }
        let mut iv = [0u8; 16];
        iv.copy_from_slice(data.get(..16)?);
        let mut d = AesDecryptor::new(key, &iv)?;
        let body = data.get(16..)?;
        let mut out = vec![0u8; body.len()];
        for (i, chunk) in body.chunks(16).enumerate() {
            if chunk.len() < 16 {
                // A partial final block: keep what we can rather than failing.
                for (j, &b) in chunk.iter().enumerate() {
                    let c = out.get(i * 16 + j).copied().unwrap_or(0);
                    if let Some(o) = out.get_mut(i * 16 + j) {
                        *o = c ^ b ^ d.chain[j];
                    }
                }
                break;
            }
            let mut block = [0u8; 16];
            block.copy_from_slice(chunk);
            let cipher = block;
            d.aes.decrypt_block(&mut block);
            for j in 0..16 {
                block[j] ^= d.chain[j];
            }
            d.chain = cipher;
            if let Some(slot) = out.get_mut(i * 16..i * 16 + 16) {
                slot.copy_from_slice(&block);
            }
        }
        Some(strip_pkcs7(&out))
    }

    /// Decrypt into a caller-provided buffer, used by the tolerant stream reader.
    pub fn decrypt_into(&mut self, data: &[u8], out: &mut [u8]) {
        for (i, chunk) in data.chunks(16).enumerate() {
            let n = chunk.len().min(16);
            let mut block = [0u8; 16];
            block[..n].copy_from_slice(&chunk[..n]);
            let cipher = block;
            self.aes.decrypt_block(&mut block);
            for j in 0..n {
                block[j] ^= self.chain[j];
            }
            self.chain = cipher;
            let end = (i * 16 + n).min(out.len());
            let start = (i * 16).min(end);
            if let Some(slot) = out.get_mut(start..end) {
                slot.copy_from_slice(&block[..end - start]);
            }
        }
    }
}

/// Remove PKCS#7 padding when it is well formed; otherwise leave the data alone.
#[must_use]
pub fn strip_pkcs7(data: &[u8]) -> Vec<u8> {
    let Some(&pad) = data.last() else {
        return data.to_vec();
    };
    let pad = usize::from(pad);
    if pad == 0 || pad > 16 || pad > data.len() {
        return data.to_vec();
    }
    if data[data.len() - pad..].iter().all(|&b| usize::from(b) == pad) {
        return data[..data.len() - pad].to_vec();
    }
    data.to_vec()
}

/// Add PKCS#7 padding.
#[must_use]
pub fn add_pkcs7(data: &[u8]) -> Vec<u8> {
    let pad = 16 - (data.len() % 16);
    let mut out = data.to_vec();
    out.extend(std::iter::repeat_n(pad as u8, pad));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(d: &[u8]) -> String {
        d.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .filter_map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
            .collect()
    }

    #[test]
    fn fips197_aes128() {
        let key = unhex("000102030405060708090a0b0c0d0e0f");
        let mut block = [0u8; 16];
        block.copy_from_slice(&unhex("00112233445566778899aabbccddeeff"));
        let aes = Aes::new(&key).expect("key");
        aes.encrypt_block(&mut block);
        assert_eq!(hex(&block), "69c4e0d86a7b0430d8cdb78070b4c55a");
        aes.decrypt_block(&mut block);
        assert_eq!(hex(&block), "00112233445566778899aabbccddeeff");
    }

    #[test]
    fn fips197_aes256() {
        let key = unhex("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f");
        let mut block = [0u8; 16];
        block.copy_from_slice(&unhex("00112233445566778899aabbccddeeff"));
        let aes = Aes::new(&key).expect("key");
        aes.encrypt_block(&mut block);
        assert_eq!(hex(&block), "8ea2b7ca516745bfeafc49904b496089");
        aes.decrypt_block(&mut block);
        assert_eq!(hex(&block), "00112233445566778899aabbccddeeff");
    }

    #[test]
    fn nist_cbc_vector() {
        // SP 800-38A, F.2.1, AES-128-CBC.
        let key = unhex("2b7e151628aed2a6abf7158809cf4f3c");
        let iv = unhex("000102030405060708090a0b0c0d0e0f");
        let plain = unhex("6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e51");
        let cipher = unhex("7649abac8119b246cee98e9b12e9197d5086cb9b507219ee95db113a917678b2");
        let iv_a: [u8; 16] = iv.clone().try_into().unwrap_or([0; 16]);
        let mut enc = AesEncryptor::new(&key, &iv_a).expect("key");
        let mut buf = plain.clone();
        enc.apply(&mut buf);
        assert_eq!(hex(&buf), hex(&cipher));
        let dec = AesDecryptor::with_key(&key, &{
            let mut v = iv.clone();
            v.extend_from_slice(&cipher);
            v
        })
        .expect("decrypt");
        assert_eq!(dec, plain);
    }

    #[test]
    fn padding_round_trip() {
        for n in 0..40usize {
            let data: Vec<u8> = (0..n).map(|i| i as u8).collect();
            let padded = add_pkcs7(&data);
            assert_eq!(padded.len() % 16, 0);
            assert_eq!(strip_pkcs7(&padded), data);
        }
    }

    #[test]
    fn damaged_padding_is_left_alone() {
        let mut data = vec![1u8, 2, 3, 4, 5];
        data.push(9); // invalid padding
        assert_eq!(strip_pkcs7(&data), data);
    }
}
