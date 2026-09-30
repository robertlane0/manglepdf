//! PDF security: the standard security handlers, key derivation, permissions, and the
//! digest and signature primitives the signer needs.
//!
//! Everything here is byte-level and independent of the object model.

#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    clippy::indexing_slicing
)]
#![warn(missing_debug_implementations)]

mod aes;
mod error;
mod handler;
mod permissions;
mod rc4;
mod saslprep;
mod sha;

pub use aes::{AesDecryptor, AesEncryptor};
pub use error::{CryptoError, Result};
pub use handler::{
    Algorithm, Decryptor, EncryptionDict, Revision, compute_file_key, constant_time_eq,
    owner_password_key, prepare_owner_password, prepare_user_password, user_hash_legacy,
    validate_user_password,
};
pub use permissions::{Permissions, describe};
pub use rc4::{Rc4, rc4};
pub use sha::{Md5, Sha1, Sha256, Sha384, Sha512, md5, sha1, sha256, sha384, sha512};
