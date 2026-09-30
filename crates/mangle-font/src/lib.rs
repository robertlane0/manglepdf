//! PDF fonts.
//!
//! Everything PDF-shaped about a font is ours: the font programs (TrueType, CFF, Type
//! 1), the encodings, the CMaps, the glyph-name tables, the metrics a page depends on,
//! and subsetting. Shaping a run of text is a different problem and uses a shaping
//! library; deciding which glyphs a character code maps to in *this* document is ours.

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
