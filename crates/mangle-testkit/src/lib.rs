//! Shared test fixtures, comparisons and tolerances.
//!
//! Lives above the product crates so that a test can compare a render, measure a
//! similarity, or build a document without any of them depending on each other.

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
