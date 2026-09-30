//! The rasterizer.
//!
//! Analytic coverage, so a fill is exact rather than sampled, and the same rasterizer
//! serves both the page and the interface's own vector artwork. Everything driven by a
//! file is bounded: a page may not allocate without limit, and a worker may not run
//! without a deadline.

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
