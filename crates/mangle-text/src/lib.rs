//! Text extraction, reading order and search.
//!
//! The result is built from the page object model rather than from the raw content
//! stream, so a selection across two columns comes out in the order a reader would say
//! it is in, not in the order the operators happen to appear.

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
