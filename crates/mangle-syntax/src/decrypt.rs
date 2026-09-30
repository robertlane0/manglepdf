//! Applying the document's encryption to parsed objects.

use mangle_crypto::{Algorithm, Decryptor};

use crate::object::{Dict, Name, Object, Ref, Stream};

/// Walk an object graph, decrypting strings and stream data in place.
///
/// The `/Encrypt` dictionary itself, the `/ID` strings and anything in the trailer of an
/// encrypted file are left alone, which is why the caller must not route them here.
pub fn decrypt_object(dec: &Decryptor, num: u32, generation: u16, obj: &mut Object, depth: usize) {
    if depth > 64 {
        return;
    }
    match obj {
        Object::String(s) => {
            *s = dec.decrypt(num, generation, s);
        }
        Object::Array(a) => {
            for item in a.iter_mut() {
                decrypt_object(dec, num, generation, item, depth + 1);
            }
        }
        Object::Dict(d) => {
            if !dec.encrypts_metadata()
                && d.get("Type").and_then(Object::as_name) == Some(&b"Metadata"[..])
            {
                return;
            }
            decrypt_dict(dec, num, generation, d, depth + 1);
        }
        Object::Stream(s) => {
            if !dec.encrypts_metadata()
                && s.dict.get("Type").and_then(Object::as_name) == Some(&b"Metadata"[..])
            {
                return;
            }
            s.raw = dec.decrypt(num, generation, &s.raw);
            decrypt_dict(dec, num, generation, &mut s.dict, depth + 1);
        }
        _ => {}
    }
}

fn decrypt_dict(dec: &Decryptor, num: u32, generation: u16, d: &mut Dict, depth: usize) {
    let skip: Vec<Name> = d
        .iter()
        .filter(|(k, _)| k.as_bytes() == b"Encrypt" || k.as_bytes() == b"ID")
        .map(|(k, _)| k.clone())
        .collect();
    for key in skip {
        if let Some(v) = d.get_mut(
            key.as_bytes()
                .iter()
                .map(|b| *b as char)
                .collect::<String>()
                .as_str(),
        ) {
            decrypt_object(dec, num, generation, v, depth + 1);
        }
    }
    for (_, v) in d.iter_mut() {
        decrypt_object(dec, num, generation, v, depth + 1);
    }
}

/// Encrypt an object graph, for writing an encrypted file.
pub fn encrypt_object(enc: &Decryptor, num: u32, generation: u16, obj: &mut Object, depth: usize) {
    if depth > 64 {
        return;
    }
    match obj {
        Object::String(s) => *s = enc.encrypt(num, generation, s),
        Object::Array(a) => {
            for item in a.iter_mut() {
                encrypt_object(enc, num, generation, item, depth + 1);
            }
        }
        Object::Dict(d) => encrypt_dict(enc, num, generation, d, depth + 1),
        Object::Stream(s) => {
            s.raw = enc.encrypt(num, generation, &s.raw);
            encrypt_dict(enc, num, generation, &mut s.dict, depth + 1);
        }
        _ => {}
    }
}

fn encrypt_dict(enc: &Decryptor, num: u32, generation: u16, d: &mut Dict, depth: usize) {
    for (_, v) in d.iter_mut() {
        encrypt_object(enc, num, generation, v, depth + 1);
    }
}

/// The crypt filter that applies to an object, honouring `/StmF` and `/StrF`.
#[must_use]
pub fn algorithm_for(encrypt: &Object, is_stream: bool, stream_dict: Option<&Dict>) -> Algorithm {
    let Some(d) = encrypt.as_dict() else {
        return Algorithm::None;
    };
    let v = d.get("V").and_then(Object::as_i64).unwrap_or(0);
    if v >= 4 {
        let name = if is_stream {
            d.get("StmF")
        } else {
            d.get("StrF")
        }
        .and_then(Object::as_name)
        .unwrap_or(b"Identity");
        if name == b"Identity" {
            return Algorithm::None;
        }
        // `/Identity` crypt filters, and filters that name a different `/CFM`.
        let Some(cf) = d.get("CF").and_then(Object::as_dict) else {
            return Algorithm::None;
        };
        let Some(entry) = cf.get(&String::from_utf8_lossy(name)) else {
            return Algorithm::None;
        };
        let Some(entry) = entry.as_dict() else {
            return Algorithm::None;
        };
        return match entry.get("CFM").and_then(Object::as_name) {
            Some(b"AESV2") => Algorithm::AesV2,
            Some(b"AESV3") => Algorithm::AesV3,
            Some(b"V2") => Algorithm::Rc4,
            _ => Algorithm::None,
        };
    }
    let r = d.get("R").and_then(Object::as_i64).unwrap_or(2);
    if r == 4 && d.get("CF").is_some() {
        return match d.get("StmF").and_then(Object::as_name) {
            Some(b"StdCF") => Algorithm::AesV2,
            _ => Algorithm::Rc4,
        };
    }
    let _ = stream_dict;
    Algorithm::Rc4
}

/// Build the `/Encrypt` dictionary that describes `dec`, for writing.
#[must_use]
pub fn build_encrypt_dict(algorithm: Algorithm, key_bits: i32, o: &[u8], u: &[u8], p: i32) -> Dict {
    let mut d = Dict::new();
    d.set("Filter", Object::name("Standard"));
    match algorithm {
        Algorithm::AesV3 => {
            d.set("V", Object::Int(5));
            d.set("R", Object::Int(6));
            d.set("Length", Object::Int(256));
        }
        Algorithm::AesV2 => {
            d.set("V", Object::Int(4));
            d.set("R", Object::Int(4));
            d.set("Length", Object::Int(i64::from(key_bits)));
            let mut std = Dict::new();
            std.set("CFM", Object::name("AESV2"));
            std.set("AuthEvent", Object::name("DocOpen"));
            std.set("Length", Object::Int(16));
            let mut cf = Dict::new();
            cf.set("StdCF", Object::Dict(std));
            d.set("CF", Object::Dict(cf));
            d.set("StmF", Object::name("StdCF"));
            d.set("StrF", Object::name("StdCF"));
        }
        _ => {
            d.set("V", Object::Int(if key_bits > 40 { 2 } else { 1 }));
            d.set("R", Object::Int(if key_bits > 40 { 3 } else { 2 }));
            d.set("Length", Object::Int(i64::from(key_bits)));
        }
    }
    d.set("O", Object::String(o.to_vec()));
    d.set("U", Object::String(u.to_vec()));
    d.set("P", Object::Int(i64::from(p)));
    d
}

/// Whether an object number is the `/Encrypt` dictionary itself.
#[must_use]
pub fn is_encrypt_ref(r: Ref, encrypt: Option<Ref>) -> bool {
    encrypt == Some(r)
}

/// Helper for callers that need the raw bytes of a stream after decryption.
#[must_use]
pub fn stream_bytes(s: &Stream) -> &[u8] {
    &s.raw
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect`, which is what a test is for; the
    // panic-free rule is about what the product does with a file, not about tests.
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::*;
    use mangle_crypto::Rc4;

    const KEY: &[u8] = b"0123456789abcdef";

    /// RC4-encrypt a test string the way object 1 generation 0 would be.
    fn enc(s: &[u8]) -> Vec<u8> {
        let key = Decryptor::new(KEY.to_vec(), Algorithm::Rc4, true).object_key(1, 0);
        let mut v = s.to_vec();
        Rc4::new(&key).apply(&mut v);
        v
    }

    fn test_decryptor() -> Decryptor {
        Decryptor::new(KEY.to_vec(), Algorithm::Rc4, true)
    }

    #[test]
    fn strings_are_decrypted() {
        let d = test_decryptor();
        let mut obj = Object::String(enc(b"hello"));
        decrypt_object(&d, 1, 0, &mut obj, 0);
        assert_eq!(obj, Object::String(b"hello".to_vec()));
    }

    #[test]
    fn nested_structures_are_decrypted() {
        let d = test_decryptor();
        let e = |s: &[u8]| Object::String(enc(s));
        let mut obj = Object::Dict(
            [
                (Name::new("A"), e(b"one")),
                (
                    Name::new("B"),
                    Object::Array(vec![e(b"two"), Object::Int(3)]),
                ),
            ]
            .into_iter()
            .collect(),
        );
        decrypt_object(&d, 1, 0, &mut obj, 0);
        let dict = obj.as_dict().expect("dict");
        assert_eq!(dict.get("A"), Some(&Object::String(b"one".to_vec())));
        assert_eq!(
            dict.get("B").and_then(|o| o.as_array()).map(|a| a.first()),
            Some(Some(&Object::String(b"two".to_vec())))
        );
    }

    #[test]
    fn metadata_can_be_left_alone() {
        let d = Decryptor::new(KEY.to_vec(), Algorithm::Rc4, false);
        let mut obj = Object::Dict(
            [
                (Name::new("Type"), Object::name("Metadata")),
                (Name::new("S"), Object::String(enc(b"raw"))),
            ]
            .into_iter()
            .collect(),
        );
        decrypt_object(&d, 1, 0, &mut obj, 0);
        let dict = obj.as_dict().expect("dict");
        assert_ne!(dict.get("S"), Some(&Object::String(b"raw".to_vec())));
    }

    #[test]
    fn crypt_filter_selects_the_algorithm() {
        let mut std = Dict::new();
        std.set("CFM", Object::name("AESV2"));
        let mut cf = Dict::new();
        cf.set("StdCF", Object::Dict(std));
        let enc = Object::Dict(
            [
                (Name::new("V"), Object::Int(4)),
                (Name::new("CF"), Object::Dict(cf)),
                (Name::new("StmF"), Object::name("StdCF")),
                (Name::new("StrF"), Object::name("StdCF")),
            ]
            .into_iter()
            .collect(),
        );
        assert_eq!(algorithm_for(&enc, true, None), Algorithm::AesV2);
        assert_eq!(algorithm_for(&enc, false, None), Algorithm::AesV2);
    }

    #[test]
    fn identity_filter_disables_encryption() {
        let enc = Object::Dict(
            [
                (Name::new("V"), Object::Int(4)),
                (Name::new("StmF"), Object::name("Identity")),
                (Name::new("StrF"), Object::name("Identity")),
            ]
            .into_iter()
            .collect(),
        );
        assert_eq!(algorithm_for(&enc, true, None), Algorithm::None);
    }
}
