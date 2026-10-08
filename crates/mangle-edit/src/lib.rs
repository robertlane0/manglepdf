//! Editing.
//!
//! A command pattern over immutable snapshots, undo and redo, surgical write-back, the
//! annotation and appearance generators, forms, redaction, flatten, page operations and
//! the optimizer. Every edit is one command and one undo step, and every write is
//! checked against the model it came from before it is saved.

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

pub mod arrange;
pub mod edits;
pub mod history;
pub mod page_objects;
pub mod session;
pub mod surgery;
pub mod writeback;

pub use arrange::{Arrange, ArrangeError, ArrangeRefusal, apply_arrange, arrange_patches, extent};
pub use edits::{
    Change, ChangeError, Channel, Refusal, TextProperty, apply_change, map_point, verify,
};
pub use history::{Edit, History, HistoryError, Snapshot};
pub use page_objects::{Kind, LineBreak, PageModel, PageObject, Provenance, provenance_of};
pub use session::{Editor, OpenError, Reason, SessionError};
pub use surgery::{Applied, EditError, Patch};
pub use writeback::{Save, SaveError, save_decoded, save_page, stream_digest};
