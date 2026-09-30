//! The application.
//!
//! Everything is painted from design tokens rather than from a toolkit's widgets, so
//! the interface is one drawing program with the look we intend (ADR-0004). The rule
//! from the charter that shapes this crate hardest: the UI thread never parses, decodes
//! or rasterizes.

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
