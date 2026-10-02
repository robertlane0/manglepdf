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

pub mod cff;
pub mod metrics;
pub mod outline;
pub mod type1;

pub use cff::{Cff, MAX_DEPTH as CFF_MAX_DEPTH, MAX_STACK as CFF_MAX_STACK};
pub use metrics::{
    ASCII_START, CidWidths, Declared, DeclaredWidths, STANDARD_14, Widths, ascii_glyphs, by_name,
    standard_glyph, widths,
};
pub use outline::{Outline, Program, Segment, em_scale, from_cff, from_true_type, from_type1};
pub use type1::{MAX_DEPTH as TYPE1_MAX_DEPTH, MAX_STACK as TYPE1_MAX_STACK, Type1};
