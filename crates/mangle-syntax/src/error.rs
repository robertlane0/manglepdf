//! Typed errors for the syntax layer.

use thiserror::Error;

/// How serious a problem is. The UI shows a banner for warnings and a dialog for
/// errors; nothing is ever silently swallowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Something odd was found and worked around.
    Warning,
    /// The requested operation could not be completed.
    Error,
}

/// Anything that can go wrong below the document layer.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Error {
    /// Malformed syntax, with the byte offset where it was noticed.
    #[error("{0}")]
    Syntax(String),

    /// A structural problem that the repair path has to deal with.
    #[error("{0}")]
    Structure(String),

    /// The file is encrypted and cannot be read with the supplied password.
    #[error("the document is encrypted: {0}")]
    Encrypted(String),

    /// The password was wrong.
    #[error("incorrect password")]
    BadPassword,

    /// A stream could not be decoded, described well enough to show in the Inspector.
    #[error("stream {what} could not be decoded: {reason}")]
    Stream { what: String, reason: String },

    /// A page or object does not exist.
    #[error("object {0} {1} is not in the file")]
    NotFound(u32, u16),

    /// A cycle was found where one is not allowed.
    #[error("cycle in {0}")]
    Cycle(&'static str),

    /// A limit from hostile-input discipline was reached.
    #[error("{kind} exceeded the limit of {limit}")]
    Limit { kind: &'static str, limit: usize },

    /// Writing failed.
    #[error("could not write {0}")]
    Io(String),

    /// The document forbids this operation.
    #[error("not allowed: {0}")]
    NotAllowed(String),
}

impl Error {
    /// Build a syntax error with the offset that triggered it.
    #[must_use]
    pub fn at(offset: usize, message: &str) -> Self {
        Error::Syntax(format!("at byte {offset}: {message}"))
    }

    /// A message suitable for showing to a user, with the technical detail separated.
    #[must_use]
    pub fn user_message(&self) -> String {
        match self {
            Error::Encrypted(_) => "This document is password protected.".to_string(),
            Error::BadPassword => "That password did not work.".to_string(),
            Error::NotAllowed(what) => format!("This document does not allow {what}."),
            Error::Limit { kind, limit } => {
                format!("Stopped after {limit} {kind} to keep the document safe.")
            }
            other => other.to_string(),
        }
    }
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, Error>;
