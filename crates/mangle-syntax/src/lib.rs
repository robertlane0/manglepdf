//! PDF syntax: the object model, lexer, parser, cross-reference tables with repair,
//! object streams, and the writer.
//!
//! The organising principle is **losslessness**. Every object is kept exactly as it was
//! found — raw stream bytes, original filter chain, key order, name spelling — until
//! something deliberately changes it. That is what makes the seven laws in the charter
//! achievable: an edit touches the bytes of its own object and nothing else.

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

mod decrypt;
mod document;
mod error;
mod lexer;
mod object;
mod objstm;
mod parser;
mod recovery;
mod writer;
mod xref;

pub use document::{Document, DocumentInfo, OpenOptions, PageRef};
pub use error::{Error, Result, Severity};
pub use lexer::{Lexer, Token};
pub use object::{Dict, Name, Object, Rect, Ref, Stream};
pub use objstm::ObjectStream;
pub use parser::Parser;
pub use writer::{IncrementalWriter, WriteOptions, Writer};
pub use xref::{Xref, XrefEntry};

/// An indirect object number and generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjNum(pub u32);

/// A generation number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GenNum(pub u16);
