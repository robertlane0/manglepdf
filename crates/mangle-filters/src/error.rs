//! Typed errors for the filter layer.

use thiserror::Error;

/// Something a filter can report that is not fatal.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FilterError {
    /// The filter is not one we implement, or a name we do not recognise.
    #[error("unsupported filter `{0}`")]
    UnsupportedFilter(String),
    /// A decode parameter was outside the range the specification allows.
    #[error("invalid decode parameter for `{filter}`: {reason}")]
    BadParameters {
        filter: &'static str,
        reason: String,
    },
    /// Decoding stopped early. The bytes produced so far are still valid.
    #[error("`{filter}` data is damaged: {reason}")]
    Damaged {
        filter: &'static str,
        reason: String,
    },
    /// The output would exceed the configured cap.
    #[error("`{filter}` output exceeded the {limit} byte limit")]
    OutputTooLarge { filter: &'static str, limit: usize },
    /// The caller asked for something structurally impossible (bad base, bad table).
    #[error("malformed {0} input")]
    Malformed(&'static str),
}

/// Convenience alias.
pub type FilterResult<T> = Result<T, FilterError>;
