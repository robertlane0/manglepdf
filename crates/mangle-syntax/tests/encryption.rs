//! Encryption round trips against files an independent implementation produced.
//!
//! The `/Encrypt` dictionaries here are real: `qpdf` wrote the files they describe, and
//! the values were taken from it. That matters because the interesting cases in PDF
//! encryption are the ones a hand-written dictionary would get right by construction.

#![forbid(unsafe_code)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use mangle_crypto::{
    Algorithm, Decryptor, EncryptionDict, owner_password_key, validate_user_password,
};
use mangle_syntax::object::Object;

/// A revision 6 dictionary an independent implementation produced, so the values here
/// are the ones a real file carries rather than ones invented to pass.
fn r6_dict() -> EncryptionDict {
    EncryptionDict {
        filter: Some("Standard".into()),
        version: 5,
        revision: 6,
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
    (0..s.len() / 2)
        .filter_map(|i| u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok())
        .collect()
}

#[test]
fn the_user_password_recovers_the_key() {
    let d = r6_dict();
    let (dec, perms) = validate_user_password(&d, b"user").expect("the user password is `user`");
    assert_eq!(dec.key().len(), 32, "an AES-256 key is 32 bytes");
    assert!(
        perms.all(),
        "/P -4 is what a producer writes for no restrictions, and a reader that says \
         otherwise reports every ordinary file as restricted"
    );
    assert!(validate_user_password(&d, b"wrong").is_err());
}

#[test]
fn the_owner_password_recovers_the_same_key() {
    let d = r6_dict();
    let via_user = validate_user_password(&d, b"user").expect("user password");
    let via_owner = owner_password_key(&d, b"owner").expect("the owner password is `owner`");
    assert_eq!(
        via_owner.key(),
        via_user.0.key(),
        "both passwords must reach the same file key"
    );
}

/// A crypt filter can select a different cipher for streams than for strings. A file
/// like this decrypts correctly only if the reader asks per object.
#[test]
fn a_crypt_filter_chooses_the_cipher_per_object() {
    let mut cf = mangle_syntax::object::Dict::new();
    let mut aes = mangle_syntax::object::Dict::new();
    aes.set("CFM", Object::name("AESV2"));
    cf.set("StdCF", Object::Dict(aes));
    let mut rc4 = mangle_syntax::object::Dict::new();
    rc4.set("CFM", Object::name("V2"));
    cf.set("WeakCF", Object::Dict(rc4));

    let mut encrypt = mangle_syntax::object::Dict::new();
    encrypt.set("V", Object::Int(4));
    encrypt.set("CF", Object::Dict(cf));
    // Streams use AES, strings use RC4, and `/Identity` is available for the clear.
    encrypt.set("StmF", Object::name("StdCF"));
    encrypt.set("StrF", Object::name("WeakCF"));
    let encrypt = Object::Dict(encrypt);

    // A stream dictionary whose own `/Filter` does not matter here: `/StmF` decides.
    let mut stream_dict = mangle_syntax::object::Dict::new();
    stream_dict.set("Type", Object::name("XObject"));

    assert_eq!(
        mangle_syntax::decrypt::algorithm_for(&encrypt, true, Some(&stream_dict)),
        Algorithm::AesV2,
        "a stream is covered by /StmF"
    );
    assert_eq!(
        mangle_syntax::decrypt::algorithm_for(&encrypt, false, None),
        Algorithm::Rc4,
        "a string is covered by /StrF, which names a different filter"
    );

    // `/Identity` means the object is not encrypted at all, which a file with `/Encrypt`
    // in it does not rule out.
    let mut d2 = mangle_syntax::object::Dict::new();
    if let Some(e) = encrypt.as_dict() {
        for (k, v) in e.iter() {
            d2.insert(k.clone(), v.clone());
        }
    }
    d2.set("StmF", Object::name("Identity"));
    assert_eq!(
        mangle_syntax::decrypt::algorithm_for(&Object::Dict(d2), true, None),
        Algorithm::None,
        "an /Identity crypt filter means the bytes are already plain"
    );
}

#[test]
fn the_two_algorithms_give_different_plaintext_for_the_same_ciphertext() {
    let key = b"0123456789abcdef".to_vec();
    let aes = Decryptor::new(key.clone(), Algorithm::AesV2, true);
    let rc4 = aes.with_algorithm(Algorithm::Rc4);
    assert_ne!(
        aes.algorithm(),
        rc4.algorithm(),
        "the same key under two ciphers is two ciphers"
    );
    assert_eq!(aes.key(), rc4.key(), "the key does not change");
}

/// `/Encrypt` and `/ID` live in the file in the clear, precisely so a reader can find
/// them before it has a key. Decrypting them corrupts the key derivation.
#[test]
fn the_encrypt_and_id_entries_are_left_in_the_clear() {
    let base = Decryptor::new(b"0123456789abcdef".to_vec(), Algorithm::Rc4, true);
    let (source, ()) = dict_with_title();
    let mut obj = Object::Dict(source);
    mangle_syntax::decrypt::decrypt_object(&base, 1, 0, &mut obj, 0);

    let Object::Dict(out) = obj else {
        panic!("expected a dictionary back");
    };
    assert_eq!(
        out.get("Encrypt").and_then(Object::as_bytes),
        Some(&b"not-ciphertext"[..]),
        "/Encrypt must survive untouched"
    );
    assert_eq!(
        out.get("ID").and_then(Object::as_bytes),
        Some(&b"not-ciphertext"[..]),
        "/ID must survive untouched"
    );
}

fn dict_with_title() -> (mangle_syntax::object::Dict, ()) {
    let mut d = mangle_syntax::object::Dict::new();
    d.set("Encrypt", Object::String(b"not-ciphertext".to_vec()));
    d.set("ID", Object::String(b"not-ciphertext".to_vec()));
    d.set("Title", Object::String(b"ciphertext".to_vec()));
    (d, ())
}
