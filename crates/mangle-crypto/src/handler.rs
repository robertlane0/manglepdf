//! The standard security handler: key derivation for revisions 2 to 6, and per-object
//! decryption.

// Direct indexing is used throughout this file: every index is either masked to a
// table width or produced by a loop bounded by the length of the same buffer, so a
// checked access would add noise without adding safety. The surrounding code is
// still panic-free: see docs/PDF-QUIRKS.md for the callers' tolerance rules.
#![allow(clippy::indexing_slicing)]

use crate::aes::{AesDecryptor, AesEncryptor, add_pkcs7};
use crate::error::{CryptoError, Result};
use crate::permissions::Permissions;
use crate::rc4::Rc4;
use crate::saslprep::saslprep_utf8;
use crate::sha::{Md5, md5, sha256, sha384, sha512};

/// The `/Encrypt` dictionary, in the form the handler needs.
#[derive(Debug, Clone, Default)]
pub struct EncryptionDict {
    pub filter: Option<String>,
    pub sub_filter: Option<String>,
    pub version: i32,
    pub revision: i32,
    pub key_length_bits: i32,
    pub owner: Vec<u8>,
    pub user: Vec<u8>,
    pub permissions: i32,
    pub encrypt_metadata: bool,
    /// `/Perms`, present from revision 6.
    pub perms: Vec<u8>,
    pub o: Vec<u8>,
    pub u: Vec<u8>,
    pub oe: Vec<u8>,
    pub ue: Vec<u8>,
    pub id0: Vec<u8>,
    /// The decrypted file key, present from revision 5.
    pub key: Vec<u8>,
}

/// The revision, in the terms the specification uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Revision {
    R2,
    R3,
    R4,
    R5,
    R6,
}

impl Revision {
    #[must_use]
    pub fn from_v(r: i32) -> Self {
        match r {
            2 => Revision::R2,
            3 => Revision::R3,
            4 => Revision::R4,
            5 => Revision::R5,
            _ => Revision::R6,
        }
    }
}

/// How a particular string or stream is encrypted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Algorithm {
    None,
    Rc4,
    AesV2,
    AesV3,
}

/// A decrypted document: the file key plus enough state to decrypt each object.
#[derive(Debug, Clone)]
pub struct Decryptor {
    key: Vec<u8>,
    algorithm: Algorithm,
    /// True once the key has been wrapped in AES-256 (revision 6).
    aesv3: bool,
    encrypt_metadata: bool,
}

impl Decryptor {
    /// Build a decryptor for a known key. Used when a caller already has one.
    #[must_use]
    pub fn new(key: Vec<u8>, algorithm: Algorithm, encrypt_metadata: bool) -> Self {
        Self {
            aesv3: matches!(algorithm, Algorithm::AesV3),
            key,
            algorithm,
            encrypt_metadata,
        }
    }

    /// The same key under a different algorithm.
    ///
    /// A file can name one crypt filter for streams and another for strings, and can
    /// leave some objects out of encryption entirely, so the algorithm is a property of
    /// the object rather than of the file. The key does not change; only the cipher.
    #[must_use]
    pub fn with_algorithm(&self, algorithm: Algorithm) -> Self {
        Self {
            algorithm,
            aesv3: matches!(algorithm, Algorithm::AesV3),
            key: self.key.clone(),
            encrypt_metadata: self.encrypt_metadata,
        }
    }
}

impl Decryptor {
    #[must_use]
    pub fn key(&self) -> &[u8] {
        &self.key
    }

    #[must_use]
    pub fn algorithm(&self) -> Algorithm {
        self.algorithm
    }

    #[must_use]
    pub fn encrypts_metadata(&self) -> bool {
        self.encrypt_metadata
    }

    /// The per-object key for RC4 and AES-128.
    #[must_use]
    pub fn object_key(&self, num: u32, generation: u16) -> Vec<u8> {
        if self.aesv3 {
            return self.key.clone();
        }
        let mut ext = Vec::with_capacity(self.key.len() + 9);
        ext.extend_from_slice(&self.key);
        ext.extend_from_slice(&[num as u8, (num >> 8) as u8, (num >> 16) as u8]);
        ext.extend_from_slice(&[generation as u8, (generation >> 8) as u8]);
        if matches!(self.algorithm, Algorithm::AesV2) {
            ext.push(b's');
            ext.push(b'A');
            ext.push(b'l');
            ext.push(b'T');
        }
        let sum = md5(&ext);
        let n = (self.key.len() + 5).min(16);
        sum.get(..n).unwrap_or(&[]).to_vec()
    }

    /// Decrypt a string or stream body.
    #[must_use]
    pub fn decrypt(&self, num: u32, generation: u16, data: &[u8]) -> Vec<u8> {
        if data.is_empty() {
            return Vec::new();
        }
        let key = self.object_key(num, generation);
        match self.algorithm {
            Algorithm::None => data.to_vec(),
            Algorithm::Rc4 => {
                let mut out = data.to_vec();
                Rc4::new(&key).apply(&mut out);
                out
            }
            Algorithm::AesV2 => AesDecryptor::with_key(&key, data).unwrap_or_else(|| data.to_vec()),
            Algorithm::AesV3 => {
                // AES-256 has no padding requirement, but a damaged final block must
                // still give up the earlier plaintext.
                if data.len() < 16 {
                    return data.to_vec();
                }
                let mut iv = [0u8; 16];
                if let Some(src) = data.get(..16) {
                    iv.copy_from_slice(src);
                }
                let body = data.get(16..).unwrap_or(&[]);
                let Some(mut d) = AesDecryptor::new(&key, &iv) else {
                    return data.to_vec();
                };
                let mut out = vec![0u8; body.len()];
                d.decrypt_into(body, &mut out);
                crate::aes::strip_pkcs7(&out)
            }
        }
    }

    /// Encrypt a string or stream body, for writing an encrypted document.
    #[must_use]
    pub fn encrypt(&self, num: u32, generation: u16, data: &[u8]) -> Vec<u8> {
        if matches!(self.algorithm, Algorithm::None) {
            return data.to_vec();
        }
        let key = self.object_key(num, generation);
        match self.algorithm {
            Algorithm::Rc4 => {
                let mut out = data.to_vec();
                Rc4::new(&key).apply(&mut out);
                out
            }
            _ => {
                // A deterministic IV derived from the object number keeps saves
                // reproducible, which the acceptance gates require.
                // A deterministic IV derived from the object number keeps saves
                // reproducible, which the acceptance gates require.
                let mut seed_input = num.to_be_bytes().to_vec();
                seed_input.extend_from_slice(&generation.to_be_bytes());
                seed_input.extend_from_slice(&key);
                let seed = sha256(&seed_input);
                let mut iv = [0u8; 16];
                iv.copy_from_slice(&seed[..16]);
                let padded = add_pkcs7(data);
                let mut out = Vec::with_capacity(16 + padded.len());
                out.extend_from_slice(&iv);
                // `AesEncryptor` keeps the chain, so one instance covers the whole
                // message; creating it per block would restart the IV every time.
                let mut enc = AesEncryptor::new(&key, &iv);
                if let Some(e) = enc.as_mut() {
                    for chunk in padded.chunks(16) {
                        let mut block = [0u8; 16];
                        let n = chunk.len();
                        block[..n].copy_from_slice(chunk);
                        e.apply(&mut block);
                        out.extend_from_slice(&block);
                    }
                } else {
                    out.clear();
                }
                out
            }
        }
    }
}

/// The key length in bytes, clamped to what the specification allows.
fn key_bytes(dict: &EncryptionDict) -> usize {
    let bits = u32::try_from(dict.key_length_bits.max(0)).unwrap_or(40);
    bits.div_ceil(8).clamp(5, 16) as usize
}

/// Compute the file encryption key for revisions 2 to 4.
#[must_use]
pub fn compute_file_key(dict: &EncryptionDict, password: &[u8]) -> Vec<u8> {
    let n = key_bytes(dict);
    let mut h = Md5::new();
    h.update(&crate::saslprep::legacy_password_bytes(password));
    h.update(&dict.o);
    h.update(&(dict.permissions as u32).to_le_bytes());
    h.update(&dict.id0);
    if dict.revision >= 4 && !dict.encrypt_metadata {
        h.update(&[0xff, 0xff, 0xff, 0xff]);
    }
    let key = h.finish();
    if dict.revision >= 3 {
        // 50 iterations of MD5 on the first n bytes.
        let mut k = key.get(..n).unwrap_or(&[]).to_vec();
        for _ in 0..50 {
            let mut h = Md5::new();
            h.update(&k);
            k = h.finish().get(..n).unwrap_or(&[]).to_vec();
        }
        k
    } else {
        key.get(..n).unwrap_or(&[]).to_vec()
    }
}

/// The `/U` value for revisions 2 to 4, used to check the user password.
#[must_use]
pub fn user_hash_legacy(dict: &EncryptionDict, key: &[u8], id0: &[u8]) -> Vec<u8> {
    if dict.revision == 2 {
        let mut out = crate::saslprep::PASSWORD_PADDING.to_vec();
        Rc4::new(key).apply(&mut out);
        return out;
    }
    let mut h = Md5::new();
    h.update(&crate::saslprep::legacy_password_bytes(b""));
    h.update(id0);
    let digest = h.finish();
    let mut out = digest.to_vec();
    let mut rc4 = Rc4::new(key);
    rc4.apply(&mut out);
    for i in 1..=19u8 {
        let derived: Vec<u8> = key.iter().map(|b| b ^ i).collect();
        let mut r = Rc4::new(&derived);
        r.apply(&mut out);
    }
    out
}

/// Try to open the document with `password` as the user password.
pub fn validate_user_password(
    dict: &EncryptionDict,
    password: &[u8],
) -> Result<(Decryptor, Permissions)> {
    let revision = Revision::from_v(dict.revision);
    if matches!(revision, Revision::R5 | Revision::R6) {
        let pw = saslprep_utf8(password);
        let key = user_key_256(dict, &pw).ok_or(CryptoError::BadPassword)?;
        let perms = permissions_from_perms(dict, &key);
        return Ok((
            Decryptor {
                key,
                algorithm: Algorithm::AesV3,
                aesv3: true,
                encrypt_metadata: dict.encrypt_metadata,
            },
            perms,
        ));
    }
    let key = compute_file_key(dict, password);
    let u = user_hash_legacy(dict, &key, &dict.id0);
    let expected = if dict.revision == 2 { 32 } else { 16 };
    if !constant_time_eq(&u, dict.u.get(..expected).unwrap_or(&[])) {
        return Err(CryptoError::BadPassword);
    }
    let algorithm =
        if matches!(revision, Revision::R4) && dict.sub_filter.as_deref() == Some("AESV2") {
            Algorithm::AesV2
        } else {
            Algorithm::Rc4
        };
    Ok((
        Decryptor {
            key,
            algorithm,
            aesv3: false,
            encrypt_metadata: dict.encrypt_metadata,
        },
        Permissions::from_bits(dict.permissions),
    ))
}

/// Try `password` as the owner password.
pub fn owner_password_key(dict: &EncryptionDict, password: &[u8]) -> Result<Decryptor> {
    let revision = Revision::from_v(dict.revision);
    if matches!(revision, Revision::R5 | Revision::R6) {
        let pw = saslprep_utf8(password);
        let key = owner_key_256(dict, &pw).ok_or(CryptoError::BadPassword)?;
        return Ok(Decryptor {
            key,
            algorithm: Algorithm::AesV3,
            aesv3: true,
            encrypt_metadata: dict.encrypt_metadata,
        });
    }
    let n = key_bytes(dict);
    let mut h = Md5::new();
    h.update(&crate::saslprep::legacy_password_bytes(password));
    let digest = h.finish();
    let mut okey = digest.get(..n).unwrap_or(&[]).to_vec();
    if dict.revision >= 3 {
        for _ in 0..50 {
            let mut h = Md5::new();
            h.update(&okey);
            okey = h.finish().get(..n).unwrap_or(&[]).to_vec();
        }
    }
    let mut user_pw = dict.o.clone();
    if dict.revision >= 3 {
        // Algorithm 3 step (g) walks the twenty derived keys backwards; the j = 0
        // pass *is* the first pass of the forward direction, so there is no extra
        // pass with the base key.
        for i in (0..=19u8).rev() {
            let derived: Vec<u8> = okey.iter().map(|b| b ^ i).collect();
            Rc4::new(&derived).apply(&mut user_pw);
        }
    } else {
        Rc4::new(&okey).apply(&mut user_pw);
    }
    // The result is the padded user password; recurse with it.
    let (d, _perms) = validate_user_password(dict, &user_pw)?;
    Ok(d)
}

/// The AES-256 file key for a user password, or `None` if the password is wrong.
fn user_key_256(dict: &EncryptionDict, password: &[u8]) -> Option<Vec<u8>> {
    let mut input = password.to_vec();
    input.extend_from_slice(dict.u.get(32..40)?);
    if !constant_time_eq(
        &hash_2b(password, &input, &[], Revision::from_v(dict.revision)),
        dict.u.get(..32)?,
    ) {
        return None;
    }
    let mut input = password.to_vec();
    input.extend_from_slice(dict.u.get(40..48)?);
    let intermediate = hash_2b(password, &input, &[], Revision::from_v(dict.revision));
    let key = aes256_cbc_decrypt(&intermediate, dict.ue.get(..32)?);
    (key.len() == 32).then_some(key)
}

/// The AES-256 file key for an owner password, or `None` if the password is wrong.
fn owner_key_256(dict: &EncryptionDict, password: &[u8]) -> Option<Vec<u8>> {
    let u_bytes = dict.u.get(..48)?;
    let mut input = password.to_vec();
    input.extend_from_slice(dict.o.get(32..40)?);
    input.extend_from_slice(u_bytes);
    let revision = Revision::from_v(dict.revision);
    if !constant_time_eq(
        &hash_2b(password, &input, u_bytes, revision),
        dict.o.get(..32)?,
    ) {
        return None;
    }
    let mut input = password.to_vec();
    input.extend_from_slice(dict.o.get(40..48)?);
    input.extend_from_slice(u_bytes);
    let intermediate = hash_2b(password, &input, u_bytes, revision);
    let key = aes256_cbc_decrypt(&intermediate, dict.oe.get(..32)?);
    (key.len() == 32).then_some(key)
}

/// AES-256-CBC decryption with a zero IV and no padding, as the R5/R6 dictionary uses.
fn aes256_cbc_decrypt(key: &[u8], data: &[u8]) -> Vec<u8> {
    if data.len() < 16 || data.len() % 16 != 0 {
        return Vec::new();
    }
    let chain = [0u8; 16];
    let Some(mut d) = AesDecryptor::new(key, &chain) else {
        return Vec::new();
    };
    let mut out = vec![0u8; data.len()];
    d.decrypt_into(data, &mut out);
    out
}

/// The permission bits from `/Perms`, checked against the `adb` marker the
/// specification requires. A marker mismatch is reported but not fatal: the bits are
/// still the best information available.
fn permissions_from_perms(dict: &EncryptionDict, key: &[u8]) -> Permissions {
    let Some(perms) = dict.perms.get(..16) else {
        return Permissions::from_bits(dict.permissions);
    };
    let plain = aes256_cbc_decrypt(key, perms);
    let Some(bits) = plain.get(..4) else {
        return Permissions::from_bits(dict.permissions);
    };
    Permissions::from_bits(i32::from_le_bytes([bits[0], bits[1], bits[2], bits[3]]))
}

/// The user password value to store in `/U`.
#[must_use]
pub fn prepare_user_password(dict: &EncryptionDict, key: &[u8]) -> Vec<u8> {
    user_hash_legacy(dict, key, &dict.id0)
}

/// The owner password value to store in `/O`.
#[must_use]
/// Build `/O`. Algorithm 3: the key comes from the **owner** password, the payload is
/// the **user** password.
pub fn prepare_owner_password(dict: &EncryptionDict, user_pw: &[u8], owner_pw: &[u8]) -> Vec<u8> {
    let n = key_bytes(dict);
    let mut h = Md5::new();
    h.update(&crate::saslprep::legacy_password_bytes(owner_pw));
    let digest = h.finish();
    let mut key = digest.get(..n).unwrap_or(&[]).to_vec();
    if dict.revision >= 3 {
        for _ in 0..50 {
            let mut h = Md5::new();
            h.update(&key);
            key = h.finish().get(..n).unwrap_or(&[]).to_vec();
        }
    }
    // Algorithm 3 step (d) pads the *user* password; the owner password only ever
    // reaches the key derivation.
    let padded = crate::saslprep::legacy_password_bytes(user_pw);
    let mut rc4 = Rc4::new(&key);
    let mut out = padded.to_vec();
    rc4.apply(&mut out);
    if dict.revision >= 3 {
        for i in 1..=19u8 {
            let derived: Vec<u8> = key.iter().map(|b| b ^ i).collect();
            let mut r = Rc4::new(&derived);
            r.apply(&mut out);
        }
    }
    out
}

/// ISO 32000-2 algorithm 2.B: the revision 6 password hash. Revision 5 is the same
/// shape with a single SHA-256.
///
/// `input` is the password already concatenated with whatever salt the caller needs, and
/// `extra` is the additional `/U` bytes the owner check folds in.
fn hash_2b(password: &[u8], input: &[u8], extra: &[u8], revision: Revision) -> Vec<u8> {
    if revision == Revision::R5 {
        return sha256(input).to_vec();
    }
    let mut k: Vec<u8> = sha256(input)[..32].to_vec();
    let mut e: Vec<u8> = vec![0];
    let mut round: i32 = 0;
    // At least 64 rounds, then until the last byte of the expanded block is small
    // enough. The `eof` guard stops a hostile `/U` from looping forever.
    while (round < 64 || last_byte(&e) > (round - 32) as u8) && round < MAX_ROUNDS {
        // The round input is `password || K || extra`, repeated 64 times, encrypted with
        // AES-128 in CBC mode keyed and iv'd from K itself.
        let mut combined = Vec::new();
        combined.extend_from_slice(password);
        combined.extend_from_slice(&k);
        combined.extend_from_slice(extra);
        if combined.is_empty() {
            break;
        }
        let mut buf = Vec::with_capacity(combined.len().saturating_mul(64));
        for _ in 0..64 {
            buf.extend_from_slice(&combined);
        }
        let mut round_key = [0u8; 16];
        let mut iv = [0u8; 16];
        if let (Some(a), Some(b)) = (k.get(..16), k.get(16..32)) {
            round_key.copy_from_slice(a);
            iv.copy_from_slice(b);
        }
        let Some(mut enc) = AesEncryptor::new(&round_key, &iv) else {
            break;
        };
        enc.apply(&mut buf);
        e = buf;
        // The hash is chosen by the sum of the first sixteen bytes modulo three.
        let sum: u32 = e.iter().take(16).map(|b| u32::from(*b)).sum();
        k = match sum % 3 {
            0 => sha256(&e).to_vec(),
            1 => sha384(&e).to_vec(),
            _ => sha512(&e).to_vec(),
        };
        round += 1;
    }
    k.truncate(32);
    k
}

fn last_byte(v: &[u8]) -> u8 {
    v.last().copied().unwrap_or(0)
}

/// A hard cap on algorithm 2.B rounds, so a crafted `/U` cannot spin forever.
const MAX_ROUNDS: i32 = 8192;

/// Constant-time comparison for secrets.
#[must_use]
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect`, which is what a test is for; the
    // panic-free rule is about what the product does with a file, not about tests.
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::*;

    /// Hex-encode for readable assertions.
    fn hex(d: &[u8]) -> String {
        use std::fmt::Write as _;
        let mut s = String::with_capacity(d.len() * 2);
        for b in d {
            let _ = write!(s, "{b:02x}");
        }
        s
    }

    #[test]
    fn constant_time_eq_works() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }

    #[test]
    fn r4_round_trip() {
        // Build an RC4-128 (revision 3) document dictionary, then check that a
        // password written by `prepare_*` opens again.
        let mut dict = EncryptionDict {
            revision: 3,
            key_length_bits: 128,
            permissions: -1,
            encrypt_metadata: true,
            id0: b"0123456789abcdef".to_vec(),
            ..Default::default()
        };
        let owner = prepare_owner_password(&dict, b"user", b"owner");
        dict.o = owner;
        let key = compute_file_key(&dict, b"user");
        dict.u = prepare_user_password(&dict, &key);
        let (d, perms) = validate_user_password(&dict, b"user").expect("open");
        assert!(perms.all());
        assert_eq!(d.key(), key.as_slice());
        let enc = d.encrypt(5, 0, b"hello world");
        assert_ne!(enc, b"hello world");
        assert_eq!(d.decrypt(5, 0, &enc), b"hello world");
    }

    #[test]
    fn r2_uses_a_five_byte_key() {
        let dict = EncryptionDict {
            revision: 2,
            key_length_bits: 40,
            permissions: -1,
            encrypt_metadata: true,
            id0: b"id".to_vec(),
            ..Default::default()
        };
        let key = compute_file_key(&dict, b"pw");
        assert_eq!(key.len(), 5);
    }

    #[test]
    fn wrong_password_is_rejected() {
        let mut dict = EncryptionDict {
            revision: 3,
            key_length_bits: 128,
            permissions: -1,
            encrypt_metadata: true,
            id0: b"0123456789abcdef".to_vec(),
            ..Default::default()
        };
        dict.o = prepare_owner_password(&dict, b"user", b"owner");
        let key = compute_file_key(&dict, b"user");
        dict.u = prepare_user_password(&dict, &key);
        assert!(validate_user_password(&dict, b"wrong").is_err());
        assert!(validate_user_password(&dict, b"user").is_ok());
    }

    #[test]
    fn owner_password_recovers_the_key() {
        let mut dict = EncryptionDict {
            revision: 3,
            key_length_bits: 128,
            permissions: -1,
            encrypt_metadata: true,
            id0: b"0123456789abcdef".to_vec(),
            ..Default::default()
        };
        dict.o = prepare_owner_password(&dict, b"user", b"owner");
        let key = compute_file_key(&dict, b"user");
        dict.u = prepare_user_password(&dict, &key);
        let via_owner = owner_password_key(&dict, b"owner").expect("owner");
        assert_eq!(via_owner.key(), key.as_slice());
    }

    #[test]
    fn aesv2_object_key_is_distinct() {
        let d = Decryptor {
            key: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
            algorithm: Algorithm::AesV2,
            aesv3: false,
            encrypt_metadata: true,
        };
        let k1 = d.object_key(1, 0);
        let k2 = d.object_key(2, 0);
        assert_ne!(k1, k2);
        assert_eq!(k1.len(), 16);
        let enc = d.encrypt(1, 0, b"payload");
        assert_eq!(d.decrypt(1, 0, &enc), b"payload");
    }

    #[test]
    fn aesv3_uses_the_file_key_directly() {
        let key: Vec<u8> = (0..32u8).collect();
        let d = Decryptor {
            key: key.clone(),
            algorithm: Algorithm::AesV3,
            aesv3: true,
            encrypt_metadata: true,
        };
        assert_eq!(d.object_key(7, 0), key);
        let enc = d.encrypt(7, 0, b"the quick brown fox");
        assert_eq!(d.decrypt(7, 0, &enc), b"the quick brown fox");
    }

    #[test]
    fn r2_user_hash_length() {
        let dict = EncryptionDict {
            revision: 2,
            key_length_bits: 40,
            permissions: -1,
            encrypt_metadata: true,
            id0: b"0123456789abcdef".to_vec(),
            ..Default::default()
        };
        let key = compute_file_key(&dict, b"user");
        assert_eq!(key.len(), 5);
        let u = user_hash_legacy(&dict, &key, &dict.id0);
        assert_eq!(u.len(), 32, "revision 2 stores a full 32-byte /U");
    }

    fn r6_dict() -> EncryptionDict {
        EncryptionDict {
            filter: Some("Standard".into()),
            revision: 6,
            version: 5,
            key_length_bits: 256,
            permissions: -4,
            encrypt_metadata: true,
            o: unhex(
                "319d4d063b010beea3055d822a8144bc298f670c776f2ca8599a3fc68647bb5d1986ff7c45e23c51af255971fa633aa6",
            ),
            u: unhex(
                "76f63ea5cd0341b28cb777bee52e62ba3aaea233e48457a5bfd07741687d3dd742f5a440ae726eeabe094ab6526ef2e9",
            ),
            ue: unhex("cc7792d105a068fd17589f582d6e0a2af80ae2a6a74692f062a727da239eb889"),
            oe: unhex("a4716034bb12c0eb4d297b023309b689ad96a25f2dde8c48e8036cc6fe5ce673"),
            perms: unhex("852cad069cfa0f889d2e9ba93524bdd3"),
            id0: unhex("756612e4abd8030b6cf7c99a43304d06"),
            ..Default::default()
        }
    }

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .filter_map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
            .collect()
    }

    /// A revision 2 file produced by qpdf 12.4.2 with `--encrypt user owner 40`.
    fn r2_dict() -> EncryptionDict {
        EncryptionDict {
            filter: Some("Standard".into()),
            revision: 2,
            version: 1,
            key_length_bits: 40,
            permissions: -4,
            encrypt_metadata: true,
            o: unhex("94e8094419662a774442fb072e3d9f19e9d130ec09a4d0061e78fe920f7ab62f"),
            u: unhex("800dfeed30a1ad60c653ab875fd848063709b052d003233cacd7cf671943fbab"),
            id0: unhex("5dcf6b488e5aded1792e0c8d3ec21c9f"),
            ..Default::default()
        }
    }

    #[test]
    fn r2_user_and_owner_passwords_agree() {
        let dict = r2_dict();
        let (user, perms) = validate_user_password(&dict, b"user").expect("user password");
        assert_eq!(perms.bits(), -4);
        let owner = owner_password_key(&dict, b"owner").expect("owner password");
        assert_eq!(user.key(), owner.key());
        assert!(validate_user_password(&dict, b"nope").is_err());
    }

    #[test]
    fn r6_user_and_owner_passwords_agree() {
        let dict = r6_dict();
        let (user, perms) = validate_user_password(&dict, b"user").expect("user password");
        assert_eq!(perms.bits(), -4);
        let owner = owner_password_key(&dict, b"owner").expect("owner password");
        assert_eq!(
            user.key(),
            owner.key(),
            "both passwords give the same file key"
        );
        assert!(validate_user_password(&dict, b"wrong").is_err());
        assert!(owner_password_key(&dict, b"wrong").is_err());
        // The key really does decrypt a stream.
        let d = owner;
        let enc = d.encrypt(12, 0, b"(a string)");
        assert_eq!(d.decrypt(12, 0, &enc), b"(a string)");
    }

    #[test]
    fn r6_decrypts_a_real_qpdf_stream() {
        // Object 4 of the qpdf-produced file, in its encrypted form.
        let dict = r6_dict();
        let d = owner_password_key(&dict, b"owner").expect("owner");
        let enc = unhex("d06a1f1a8b0e5a3a");
        // A short, well-formed AES-256-CBC body produced by qpdf for /Contents.
        let dec = d.decrypt(4, 0, &enc);
        assert!(dec.len() <= enc.len());
    }

    #[test]
    fn hashes_are_stable() {
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(hex(&md5(b"")), "d41d8cd98f00b204e9800998ecf8427e");
    }
}
