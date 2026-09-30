//! Typed errors for the document model.

/// What went wrong reading the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The structure required by the specification is missing or malformed.
    Structure(String),
    /// A cycle, a runaway walk, or a count no file could contain.
    Limit { kind: &'static str, limit: usize },
    /// A reference points at nothing.
    Dangling(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Structure(m) => write!(f, "malformed document: {m}"),
            Self::Limit { kind, limit } => write!(f, "{kind} exceeds the limit of {limit}"),
            Self::Dangling(m) => write!(f, "dangling reference: {m}"),
        }
    }
}

impl std::error::Error for Error {}

/// Shorthand for this crate's results.
pub type Result<T> = std::result::Result<T, Error>;
