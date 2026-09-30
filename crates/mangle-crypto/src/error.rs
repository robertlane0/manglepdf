//! Errors from the security layer.

use thiserror::Error;

/// Anything that can go wrong in the crypto layer.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CryptoError {
    /// The password is wrong.
    #[error("incorrect password")]
    BadPassword,

    /// The `/Encrypt` dictionary is not one we can use.
    #[error("unsupported encryption: {0}")]
    Unsupported(String),

    /// A key or block was the wrong size.
    #[error("malformed encryption data: {0}")]
    Malformed(&'static str),

    /// A limit was hit while deriving a key.
    #[error("key derivation exceeded its limit")]
    Limit,
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, CryptoError>;
