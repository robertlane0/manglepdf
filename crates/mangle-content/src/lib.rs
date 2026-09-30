//! PDF content streams.
//!
//! A content stream is a sequence of operators and their operands, and the whole point
//! of this crate is that it keeps the *byte span* of every one of them. That is what
//! makes a surgical edit possible: a change rewrites one range and leaves every other
//! byte of the page exactly as it was found.

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
