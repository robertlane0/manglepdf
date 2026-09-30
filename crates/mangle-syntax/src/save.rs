//! Writing a document back out.
//!
//! There are two ways to save, and the difference matters to the user:
//!
//! * **Full save** rewrites the file. Every object is re-emitted, keeping its number.
//!   Nothing about the document changes, so this is what a repaired file, a file that
//!   cannot take an incremental update, and Save As all use.
//! * **Incremental save** appends a revision containing only what changed. The
//!   previous bytes stay in the file as an exact prefix, which is what a signed
//!   document needs and what makes "nothing else changed" provable.
//!
//! What a save must *not* do is lose an object the file still needs. Everything here
//! is written from the resolved object graph, so an object living in an object stream
//! is materialised as a direct object, and a reference that cannot be resolved is a
//! reportable finding rather than a silently broken file.

use std::collections::BTreeMap;

use crate::document::Document;
use crate::error::{Error, Result};
use crate::object::{Dict, Object, Ref};
use crate::writer::{IncrementalUpdate, WriteOptions, Writer};
use crate::xref::{XrefEntry, find_startxref};

/// How a document should be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveOptions {
    /// Which layout to produce.
    pub mode: SaveMode,
    /// Declare this version in the header. The document's own, when `None`.
    pub header_version: Option<String>,
    /// Write a cross-reference stream instead of a table. Not every reader, and not
    /// every signed-document validator, accepts one.
    pub xref_streams: bool,
}

impl Default for SaveOptions {
    fn default() -> Self {
        Self {
            mode: SaveMode::Full,
            header_version: None,
            xref_streams: false,
        }
    }
}

/// Which save to perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveMode {
    /// Rewrite the whole file.
    Full,
    /// Append a revision with only the changes.
    Incremental,
}

/// What a save produced, and anything the caller should tell the user about.
#[derive(Debug, Clone, Default)]
pub struct SaveReport {
    /// The bytes written.
    pub bytes: Vec<u8>,
    /// Objects that were dropped because nothing could resolve them.
    pub dropped: Vec<Ref>,
    /// Whether a full rewrite was forced rather than requested.
    pub forced_full: bool,
    /// Why, in one sentence, when a full rewrite was forced.
    pub forced_reason: Option<String>,
}

impl Document {
    /// Write the document.
    pub fn save(&self, options: &SaveOptions) -> Result<SaveReport> {
        if self.info().encryption.encrypted {
            return Err(Error::Encrypted(
                "this document is encrypted; decrypt it before saving, or save a copy \
                 without the protection you were given"
                    .into(),
            ));
        }
        // A repaired file must not be appended to: its original bytes are not a valid
        // history, and a viewer would read the broken revision first.
        let forces_full = self.info().recovery.forces_full_rewrite();
        let mode = if forces_full {
            SaveMode::Full
        } else {
            options.mode
        };

        let objects = self.all_objects()?;
        let mut report = match mode {
            SaveMode::Full => {
                let wopts = WriteOptions {
                    file_id: self.file_id(),
                    use_xref_streams: options.xref_streams,
                    header_version: options
                        .header_version
                        .clone()
                        .or_else(|| Some(self.info().version.clone())),
                    // /Size must keep covering every number the file ever used.
                    minimum_size: self.with_xref(|x| x.size()),
                    ..WriteOptions::default()
                };
                let mut trailer = Dict::new();
                if let Some(info) = self.info_ref() {
                    trailer.set("Info", Object::Ref(info));
                }
                let roots: Vec<Ref> = self.catalog_ref().into_iter().collect();
                // An object that is neither in the overlay nor removed was never
                // touched, so its original bytes are still the truth. Copying them is
                // the difference between a lossless save and a re-serialisation that
                // happens to parse.
                let touched: std::collections::BTreeSet<u32> = self
                    .overlay()
                    .into_iter()
                    .map(|(r, _)| r.num)
                    .chain(self.removed_refs().into_iter().map(|r| r.num))
                    .collect();
                let spans: BTreeMap<u32, (usize, usize)> = self
                    .object_spans()
                    .into_iter()
                    .filter(|(num, _)| !touched.contains(num))
                    .collect();
                let writer =
                    Writer::new(&objects.0, &objects.1, wopts).copying_from(self.bytes(), &spans);
                let bytes = writer.write(&roots, &trailer);
                SaveReport {
                    bytes,
                    dropped: Vec::new(),
                    forced_full: forces_full,
                    forced_reason: forces_full.then(|| {
                        "the file was repaired on open, so its original bytes are not a \
                         valid history"
                            .to_string()
                    }),
                }
            }
            SaveMode::Incremental => {
                let changes = self.overlay();
                if changes.is_empty() && self.removed_refs().is_empty() {
                    // A no-op incremental save must leave the file exactly as it was, so
                    // that its bytes remain an exact prefix of any later revision.
                    return Ok(SaveReport {
                        bytes: self.bytes().to_vec(),
                        forced_full: forces_full,
                        dropped: Vec::new(),
                        forced_reason: None,
                    });
                }
                let mut update = IncrementalUpdate::new(self.with_xref(|x| x.size()));
                for (r, obj) in changes {
                    update.set_with_generation(r.num, r.generation, obj);
                }
                for r in self.removed_refs() {
                    update.free(r.num, r.generation);
                }
                let prev = find_startxref(self.bytes()).unwrap_or(0);
                let root = self.catalog_ref();
                let bytes = update.append_with_id(self.bytes(), prev as u64, root, &self.file_id());
                SaveReport {
                    bytes,
                    forced_full: false,
                    dropped: Vec::new(),
                    forced_reason: None,
                }
            }
        };
        report.dropped = self.unresolvable_references(&objects.0);
        Ok(report)
    }

    /// Every object in the document, keyed by number, with its generation.
    ///
    /// The range comes from `/Size` rather than from the cross-reference, so a number
    /// that is only reachable from an object stream is still written out.
    fn all_objects(&self) -> Result<(BTreeMap<u32, Object>, BTreeMap<u32, u16>)> {
        let size = self.with_xref(|x| x.size());
        let mut objects = BTreeMap::new();
        let mut generations = BTreeMap::new();
        let removed = self.removed_refs();
        for num in 0..size {
            if removed.contains(&Ref::new(num, 0)) {
                continue;
            }
            let entry = self.with_xref(|x| x.get(num));
            let generation = match entry {
                Some(XrefEntry::InFile { generation, .. }) => generation,
                // An object inside an object stream always has generation 0.
                Some(XrefEntry::InStream { .. }) => 0,
                // A free object stays free: its number is a legal hole.
                Some(XrefEntry::Free { generation, .. }) => {
                    generations.insert(num, generation);
                    continue;
                }
                _ => 0,
            };
            let Some(obj) = self.object(Ref::new(num, generation)) else {
                continue;
            };
            generations.insert(num, generation);
            objects.insert(num, obj);
        }
        Ok((objects, generations))
    }

    /// References that no longer resolve, which a save would turn into dangling ones.
    fn unresolvable_references(&self, objects: &BTreeMap<u32, Object>) -> Vec<Ref> {
        let mut missing = std::collections::BTreeSet::new();
        let removed = self.removed_refs();
        for (num, obj) in objects {
            let mut refs = Vec::new();
            collect_refs(obj, &mut refs, 0);
            for r in refs {
                // A reference to a number past /Size, to a removed object, or to
                // nothing at all is a reference the file cannot honour.
                if removed.contains(&r) || (!objects.contains_key(&r.num) && r.num > *num) {
                    missing.insert(r);
                }
            }
        }
        missing.into_iter().collect()
    }
}

fn collect_refs(obj: &Object, out: &mut Vec<Ref>, depth: usize) {
    if depth > 64 {
        return;
    }
    match obj {
        Object::Ref(r) | Object::Missing(r) => out.push(*r),
        Object::Array(a) => a.iter().for_each(|o| collect_refs(o, out, depth + 1)),
        Object::Dict(d) => d.iter().for_each(|(_, v)| collect_refs(v, out, depth + 1)),
        Object::Stream(s) => s
            .dict
            .iter()
            .for_each(|(_, v)| collect_refs(v, out, depth + 1)),
        _ => {}
    }
}
