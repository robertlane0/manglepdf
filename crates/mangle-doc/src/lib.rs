//! The document model above the syntax layer: catalogue, page tree, name trees,
//! outlines, destinations, page labels, optional-content groups, attachments and
//! metadata.
//!
//! Everything here answers questions about the *document* rather than about a single
//! object. The page tree in particular is the one place where a hostile or sloppy file
//! can send us into a cycle, an infinite inheritance chain or an unbounded walk, so
//! every traversal is bounded and every failure is reported rather than guessed at.

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

pub mod attachments;
pub mod catalog;
pub mod error;
pub mod labels;
pub mod layers;
pub mod metadata;
pub mod names;
pub mod outlines;
pub mod pages;

pub use attachments::{EmbeddedFiles, FileSpec};
pub use catalog::Catalog;
pub use error::{Error, Result};
pub use labels::{LabelRange, PageLabel, Style};
pub use layers::{Layer, LayerCommand, LayerTree};
pub use metadata::{DocumentMetadata, Info};
pub use names::NameTree;
pub use outlines::{Destination, Outline, OutlineItem, decode_pdf_text, resolve_destination};
pub use pages::{Inheritable, Page, PageTree};

use mangle_syntax::{Dict, Object, Ref};

/// Anything that can hand back a PDF object by number.
///
/// `mangle_syntax::Document` implements this. Tests and the fixture harness implement
/// it over a plain map, which keeps this crate free of any I/O.
pub trait Resolver {
    /// The object with this number, if it exists and is not a dangling reference.
    fn resolve(&self, r: Ref) -> Option<Object>;
}

impl Resolver for mangle_syntax::Document {
    fn resolve(&self, r: Ref) -> Option<Object> {
        self.object(r)
    }
}

/// A resolver over a plain map, for tests and for callers that already hold the
/// objects.
#[derive(Debug, Clone, Default)]
pub struct MapResolver {
    objects: std::collections::BTreeMap<Ref, Object>,
}

impl MapResolver {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an object, replacing any object with the same number and generation.
    pub fn set(&mut self, num: u32, obj: Object) {
        self.objects.insert(Ref::new(num, 0), obj);
    }

    #[must_use]
    pub fn with(mut self, num: u32, obj: Object) -> Self {
        self.set(num, obj);
        self
    }
}

impl Resolver for MapResolver {
    fn resolve(&self, r: Ref) -> Option<Object> {
        self.objects.get(&r).cloned()
    }
}

/// How deep a document structure may nest before we call it damaged.
pub const MAX_TREE_DEPTH: usize = 64;
/// The most entries one name tree or outline may hold.
pub const MAX_TREE_ENTRIES: usize = 500_000;

/// Follow a chain of indirect references to the object it finally reaches.
///
/// A self-referential or cyclic chain resolves to `None` rather than looping.
#[must_use]
pub fn follow(resolver: &dyn Resolver, mut obj: Object) -> Option<Object> {
    for _ in 0..MAX_TREE_DEPTH {
        match obj.as_ref_id() {
            Some(r) => obj = resolver.resolve(r)?,
            None => return Some(obj),
        }
    }
    None
}

/// Follow a chain of indirect references starting from a dictionary entry.
#[must_use]
pub fn follow_from(resolver: &dyn Resolver, d: &Dict, key: &str) -> Option<Object> {
    follow(resolver, d.get(key)?.clone())
}

/// The `/Kids` array of a tree node, dereferenced.
#[must_use]
pub fn kids_of(resolver: &dyn Resolver, d: &Dict) -> Vec<Object> {
    let Some(kids) = d.get("Kids").and_then(|k| follow(resolver, k.clone())) else {
        return Vec::new();
    };
    kids.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|k| follow(resolver, k.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// Is this dictionary a page rather than an interior node?
#[must_use]
pub fn is_page(d: &Dict) -> bool {
    // `/Type /Page` is definitive when present. When it is missing, the absence of
    // `/Kids` is the discriminator, which is what broken files actually rely on.
    match d.get("Type").and_then(Object::as_name) {
        Some(b"Page") => true,
        Some(_) => false,
        None => d.get("Kids").is_none(),
    }
}

/// Round a `/Rotate` value to 0, 90, 180 or 270, as the specification requires.
#[must_use]
pub fn normalise_rotate(value: i64) -> i32 {
    let r = value % 360;
    let r = if r < 0 { r + 360 } else { r };
    // Anything that is not a right angle rounds to the nearest one.
    match r {
        0..=44 | 315..=359 => 0,
        45..=134 => 90,
        135..=224 => 180,
        _ => 270,
    }
}
