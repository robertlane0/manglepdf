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

pub mod decrypt;
pub mod document;
pub mod error;
pub mod lexer;
pub mod object;
pub mod objstm;
pub mod parser;
pub mod recovery;
pub mod save;
pub mod stream;
pub mod writer;
pub mod xref;

pub use document::{Document, DocumentInfo, OpenOptions, PageRef};
pub use error::{Error, Result, Severity};
pub use lexer::{Lexer, Token};
pub use object::{Dict, Name, Object, Rect, Ref, Stream};
pub use objstm::ObjectStream;
pub use parser::Parser;
pub use save::{SaveMode, SaveOptions, SaveReport};
pub use writer::{IncrementalUpdate, WriteOptions, Writer};
pub use xref::{Xref, XrefEntry};

/// An indirect object number and generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjNum(pub u32);

/// A generation number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GenNum(pub u16);
