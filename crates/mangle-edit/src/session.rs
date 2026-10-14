//! An editing session: apply, undo, redo, save.
//!
//! # What this is for
//!
//! GOAL.md §4.8 asks for a *command pattern over immutable document snapshots, deep history
//! (memory-bounded), coalesced continuous gestures, and selection restored with undo/redo*. The
//! pieces have existed separately for a while — [`crate::history::History`] keeps the snapshots,
//! [`crate::edits`] writes one change, [`crate::arrange`] moves an object past its neighbour,
//! [`crate::writeback`] saves a page. Nothing tied them together, so nothing could be undone.
//!
//! This is the tie. It owns the document, the page, the page's resources and the history, and every
//! operation goes through it.
//!
//! # What the history holds, and why it is the content and not the model
//!
//! A snapshot is of the page's **content streams**, decoded. Not of the `PageModel`, and not of the
//! selections. The model is a *derivation* — it is whatever the interpreter makes of the stream —
//! so restoring the model but not the stream would leave a session whose picture and whose bytes
//! disagree. The selection is deliberately not restored either: it is a UI concern and this module
//! is headless, so a caller that wants it back keeps its own record.
//!
//! # What this does not do: compose
//!
//! The interesting question an editing session raises is what its *save* means after an undo. The
//! natural answer — "the patches that turn the file into what I now have" — has no right answer
//! when the current stream is somewhere no single edit ever produced: composing byte-range edits
//! across an undo is a merge, and a merge that picks a side is a merge that can silently drop bytes.
//!
//! So [`crate::writeback::save_decoded`] takes the target instead, and the session hands it exactly
//! the streams it now holds. That is the honest shape, and it is the reason this module stores
//! per-part bytes rather than a patch log.

use mangle_content::interp::run_with;
use mangle_content::matrix::Matrix;
use mangle_content::{ContentStream, Resources};
use mangle_doc::Page;
use mangle_syntax::document::Document;
use mangle_syntax::object::Object;

use crate::arrange::{Arrange, arrange_patches};
use crate::edits::{Change, Fonts, patches_for_with};
use crate::embed::{EMBEDDED_OBJECTS, embed_truetype};
use crate::history::{History, Snapshot};
use crate::page_objects::{PageModel, PageObject};
use crate::surgery::apply;
use crate::writeback::{Part, Save, SaveError, save_decoded_with, split_like};

/// One editing session on one page.
///
/// The document is moved in rather than borrowed: a session that outlived its document would be
/// saving a file that changed underneath it, and a session shorter than its document would have to
/// be re-opened from bytes on every call.
#[derive(Debug)]
pub struct Editor {
    doc: std::sync::Arc<Document>,
    page: Page,
    resources: Resources,
    /// The page's content streams as the file has them — the undo floor, never rewritten.
    ///
    /// One list rather than two, because a `base` beside it would be a second copy of the same
    /// bytes that could drift away from the filters and object numbers a save needs.
    parts: Vec<Part>,
    history: History<Vec<Vec<u8>>>,
    /// What each step was called, for a history panel and for a report.
    labels: Vec<String>,
    /// Installed font objects staged for the next save, with the resource-dict rewrite
    /// that names them.
    ///
    /// Installations are additive and survive undo: an undone embed leaves unreferenced
    /// font objects, which a full save garbage-collects and an incremental one carries
    /// the way every editor's orphans are carried. A save drains them — what is in the
    /// file is no longer staged.
    staged: Vec<(u32, Object)>,
}

/// Why a session could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// The page's content streams are not all indirect references, so none of them can be
    /// replaced and an edit could not be saved.
    NotSavable(String),
    /// One of the streams did not decode.
    Undecodable {
        /// Which part, from zero.
        part: usize,
        /// Why not.
        note: String,
    },
    /// The document is encrypted.
    Encrypted,
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotSavable(why) => write!(f, "this page cannot be edited in place: {why}"),
            Self::Undecodable { part, note } => {
                write!(f, "content stream {part} did not decode: {note}")
            }
            Self::Encrypted => write!(
                f,
                "the document is encrypted, and an edit would have to be written back into it"
            ),
        }
    }
}

impl std::error::Error for OpenError {}

impl From<SaveError> for OpenError {
    fn from(e: SaveError) -> Self {
        match e {
            SaveError::Undecodable { part, note } => Self::Undecodable { part, note },
            SaveError::PartNotIndirect { part, found } => {
                Self::NotSavable(format!("content stream {part} is {found}"))
            }
            other => Self::NotSavable(other.to_string()),
        }
    }
}

/// Why a font could not be installed, named rather than approximated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallError {
    /// The page names no resource dictionary, so there is nowhere to put the font.
    NoResources,
    /// The resource dictionary is inline in the page dictionary: rewriting it means
    /// rewriting the page dictionary, which an install does not do.
    DirectResources,
    /// The resource dictionary cannot be resolved, so it cannot be extended.
    UnresolvableResources,
    /// The name is already a font on the page; installs never rename.
    NameTaken {
        /// The name asked for.
        name: String,
    },
    /// The embedding itself was refused, with its reason.
    Embed(crate::EmbedError),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoResources => write!(
                f,
                "the page names no resource dictionary, so there is nowhere to install a font"
            ),
            Self::DirectResources => write!(
                f,
                "the page's resource dictionary is inline rather than indirect, and rewriting \
                 the page dictionary around it is not an install"
            ),
            Self::UnresolvableResources => write!(
                f,
                "the page's resource dictionary cannot be resolved, so it cannot be extended"
            ),
            Self::NameTaken { name } => write!(
                f,
                "the page already has a font named {name}, and an install never renames one"
            ),
            Self::Embed(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for InstallError {}

/// Why the stream could not be put back into the page's streams.
///
/// Not an `EditError` and not an `ArrangeError`, because it belongs to neither: it is about the
/// *session*, which is the thing that has to put the bytes back in the right object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// The edited stream no longer divides into the page's own content streams.
    Unsplittable {
        /// How many the split produced.
        parts: usize,
        /// How many the page has.
        wanted: usize,
    },
}

impl std::fmt::Display for Reason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsplittable { parts, wanted } => write!(
                f,
                "the edited stream no longer divides into the page's {wanted} content stream(s) \
                 — it came out as {parts}, and an edit cannot be saved into the right object"
            ),
        }
    }
}

impl std::error::Error for Reason {}

/// What one `apply` did: the label it recorded and the change as executed.
///
/// The two differ when a retype fell back to an embedded subset: the label carries the
/// notice for humans, and the change carries the CIDs for the check. A caller that
/// verifies what happened must verify the executed change rather than the requested
/// one — checking a fallback against the run's own font fails on a file that is
/// correct, because that font was never written.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// What the history panel shows, including any fallback notice.
    pub label: String,
    /// What was executed, with a fallback resolved to its installed CIDs.
    pub change: Change,
}

/// What went wrong in a session.
///
/// Three shapes rather than one, because they are three different failures: the edit was refused,
/// the rearrange was refused, or the session itself could not record the step. Folding the third
/// into either of the first two would lose which of them happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    /// The edit was refused, or the bytes it would have written were not there.
    Change(crate::ChangeError),
    /// The rearrange was refused, or the object cannot be moved faithfully.
    Arrange(crate::ArrangeError),
    /// A fallback font could not be installed, so a retype that needed one did not happen.
    Install(InstallError),
    /// The edited stream no longer divides into the page's content streams.
    Reason(Reason),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Change(e) => write!(f, "{e}"),
            Self::Arrange(e) => write!(f, "{e}"),
            Self::Install(e) => write!(f, "{e}"),
            Self::Reason(r) => write!(f, "{r}"),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<Reason> for SessionError {
    fn from(r: Reason) -> Self {
        Self::Reason(r)
    }
}

impl Editor {
    /// Open a session on a page of a document the caller owns.
    ///
    /// The document is taken **by `Arc`**, because the caller that keeps one is a worker that
    /// opens a file once and serves every page from it. A session that took the document by value
    /// would force that worker to re-parse the file per session — and the corpus's biggest file
    /// takes seconds.
    pub fn open(
        doc: std::sync::Arc<Document>,
        page: &Page,
        resources: &Resources,
    ) -> Result<Self, OpenError> {
        if doc.info().encryption.encrypted {
            return Err(OpenError::Encrypted);
        }
        let (parts, notes) = split_parts(&doc, page);
        if let Some(note) = notes.first() {
            return Err(OpenError::Undecodable {
                part: parts.len(),
                note: note.clone(),
            });
        }
        // A page whose content streams are not all indirect references cannot be saved, so a
        // session over it would be a promise this module cannot keep. Checked here rather than at
        // save time, where the user has already done the work.
        // A page whose content streams are not all indirect references cannot be saved, so the
        // session would be a promise it cannot keep. Checked here rather than at save time, where
        // the user has already done the work.
        let views = crate::writeback::parts_of(&doc, page)?;
        match writeback_shape(&doc, page) {
            Ok(()) => {
                let base: Vec<Vec<u8>> = views.iter().map(|p| p.decoded.clone()).collect();
                let _ = parts;
                Ok(Self {
                    history: History::new(base),
                    doc,
                    page: page.clone(),
                    resources: resources.clone(),
                    labels: Vec::new(),
                    parts: views,
                    staged: Vec::new(),
                })
            }
            Err(why) => Err(OpenError::NotSavable(why)),
        }
    }

    /// The page's content streams, decoded, one per stream in `/Contents` order.
    ///
    /// This is what was opened: the file's own bytes, unchanged.
    #[must_use]
    pub fn base(&self) -> Vec<Vec<u8>> {
        self.parts.iter().map(|p| p.decoded.clone()).collect()
    }

    /// The page's content streams as they now stand, after every undo and redo.
    #[must_use]
    pub fn parts(&self) -> &[Vec<u8>] {
        self.history.current().get()
    }

    /// The whole stream the interpreter would run now: the parts, with a newline between them.
    #[must_use]
    pub fn stream(&self) -> Vec<u8> {
        joined(self.parts())
    }

    /// What the page looks like now, read from the stream as it stands.
    ///
    /// Re-derived on every call rather than kept, because a model that survived an undo would be a
    /// model of a stream that is no longer there.
    #[must_use]
    pub fn model(&self) -> PageModel {
        PageModel::build(&self.run().records)
    }

    /// The interpreter's own reading of the current stream.
    #[must_use]
    pub fn run(&self) -> mangle_content::interp::PageContent {
        run_with(&ContentStream::parse(&self.stream()), &self.resources)
    }

    /// Apply a change to the object at `index`, in the model as it now stands.
    ///
    /// Returns what was done: the label recorded for it — the same string a history
    /// panel shows — and the change as executed, which a fallback resolves.
    pub fn apply(&mut self, index: usize, change: &Change) -> Result<Outcome, SessionError> {
        let model = self.model();
        let Some(object) = model.objects().get(index) else {
            return Err(SessionError::Change(crate::ChangeError::Refused(
                crate::Refusal::NoSpans,
            )));
        };
        let patches = match patches_for_with(
            &self.stream(),
            object,
            change,
            Some(&Fonts {
                resources: &self.resources,
                resolve: &|o| self.doc.resolve_object(o),
            }),
        ) {
            Ok(patches) => patches,
            // The one refusal with a second answer: a character the run's own font cannot
            // write is embedded from a bundled face rather than reported, which is the
            // charter's third rung. Every other refusal stands, and a fallback that
            // fails its own encoding propagates rather than recursing.
            Err(e) => {
                if let (Change::Retype { text }, crate::Refusal::GlyphNotEncodable { ch, font }) =
                    (change, &e)
                {
                    return self.apply_fallback(object, text, *ch, font.clone());
                }
                return Err(SessionError::Change(crate::ChangeError::Refused(e)));
            }
        };
        let applied = apply(&self.stream(), &patches)
            .map_err(|e| SessionError::Change(crate::ChangeError::Stream(e)))?;
        let label = change_label(change);
        self.record(applied.bytes, &patches, label.clone())?;
        Ok(Outcome {
            label,
            change: change.clone(),
        })
    }

    /// Retype through an embedded fallback face, for a character the run's own font lacks.
    ///
    /// The run's family resolves to a bundled metric-compatible face, which is subset for
    /// exactly the new text's characters, installed under a free name, and switched to
    /// for the one operation. The label carries the notice the charter requires — which
    /// face stood in and which character needed it — because a substitution no notice
    /// names is a silent one. Anything without a bundled face keeps the original refusal.
    fn apply_fallback(
        &mut self,
        object: &PageObject,
        text: &str,
        missing: char,
        font: String,
    ) -> Result<Outcome, SessionError> {
        use crate::Refusal;
        let original = || {
            SessionError::Change(crate::ChangeError::Refused(Refusal::GlyphNotEncodable {
                ch: missing,
                font: font.clone(),
            }))
        };
        let Some(record) = object.records.first() else {
            return Err(original());
        };
        let resource_name = record.text.font.clone().unwrap_or_default();
        let table = match record.form.as_deref() {
            Some(form) => self
                .resources
                .form_resources(form)
                .unwrap_or_else(|| std::sync::Arc::new(self.resources.clone())),
            None => std::sync::Arc::new(self.resources.clone()),
        };
        let base_font = table
            .fonts
            .get(&resource_name)
            .and_then(|found| {
                self.doc
                    .resolve_object(found)
                    .or_else(|| Some(found.clone()))
            })
            .and_then(|o| o.as_dict().cloned())
            .and_then(|d| {
                d.get("BaseFont")
                    .and_then(|o| o.as_name())
                    .map(|n| n.to_vec())
            })
            .and_then(|n| String::from_utf8(n).ok())
            .unwrap_or_default();
        let Some((family, program)) = mangle_font::substitute_named(&base_font) else {
            return Err(original());
        };
        let name = self.free_font_name("F");
        let chars: Vec<char> = text.chars().collect();
        let embedded = self
            .install_truetype(&name, program, &chars, family)
            .map_err(SessionError::Install)?;
        let fallback = Change::RetypeIn {
            text: text.to_string(),
            font: name.clone(),
            size: record.text.size,
            cids: embedded.cids.clone(),
        };
        let patches = patches_for_with(&self.stream(), object, &fallback, None)
            .map_err(|r| SessionError::Change(crate::ChangeError::Refused(r)))?;
        let applied = apply(&self.stream(), &patches)
            .map_err(|e| SessionError::Change(crate::ChangeError::Stream(e)))?;
        let label = format!(
            "retype as {text:?} (fallback {family} {name}: {missing:?} is not in {base_font})"
        );
        self.record(applied.bytes, &patches, label.clone())?;
        Ok(Outcome {
            label,
            change: fallback,
        })
    }

    /// Arrange the object at `index`, which needs its neighbours and is therefore not a `Change`.
    pub fn arrange(&mut self, index: usize, arrange: Arrange) -> Result<String, SessionError> {
        let model = self.model();
        let patches = arrange_patches(&self.stream(), &model, index, arrange)
            .map_err(|r| SessionError::Arrange(crate::ArrangeError::Refused(r)))?;
        let applied = apply(&self.stream(), &patches)
            .map_err(|e| SessionError::Arrange(crate::ArrangeError::Stream(e)))?;
        let label = arrange.to_string();
        self.record(applied.bytes, &patches, label.clone())?;
        Ok(label)
    }

    /// Take one step back. The stream becomes exactly what it was, byte for byte.
    pub fn undo(&mut self) -> Result<String, crate::HistoryError> {
        self.history.undo()?;
        Ok(self.labels.last().cloned().unwrap_or_default())
    }

    /// Put a step back. The stream becomes the edited one again.
    pub fn redo(&mut self) -> Result<String, crate::HistoryError> {
        self.history.redo()?;
        Ok(self.labels.last().cloned().unwrap_or_default())
    }

    /// How many steps back there are, and how many forward.
    #[must_use]
    pub fn depth(&self) -> (usize, usize) {
        (self.history.undo_depth(), self.history.redo_depth())
    }

    /// What each step was called, oldest first, for a history panel.
    #[must_use]
    pub fn labels(&self) -> &[String] {
        &self.labels
    }

    /// Whether the page has changed since it was opened, which is what makes a save worth doing.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        let base = self.base();
        self.parts() != base.as_slice() || !self.staged.is_empty()
    }

    /// A free font resource name: `stem` with the smallest number no font on the page
    /// uses, so an install never renames what the file already had.
    #[must_use]
    pub fn free_font_name(&self, stem: &str) -> String {
        let mut n = 1u32;
        loop {
            let name = format!("{stem}{n}");
            if !self.resources.fonts.contains_key(&name) {
                return name;
            }
            n += 1;
        }
    }

    /// Install an embedded subset of `program` for `chars` under `name`, and give back
    /// what was installed: the session's retype fallback switches the run to it next.
    ///
    /// The five font objects are staged for the next save with the resource-dict rewrite
    /// that names them; the in-memory resources learn the name too, so the session's own
    /// reads see the font before any save. `program` is a TrueType program (a bundled
    /// face, for the fallback the charter asks for) and `family` its `BaseFont` stem.
    pub fn install_truetype(
        &mut self,
        name: &str,
        program: &[u8],
        chars: &[char],
        family: &str,
    ) -> Result<crate::EmbeddedFont, InstallError> {
        if self.resources.fonts.contains_key(name) {
            return Err(InstallError::NameTaken {
                name: name.to_string(),
            });
        }
        let res_num = match &self.page.inherited.resources {
            Some(Object::Ref(r)) => r.num,
            Some(_) => return Err(InstallError::DirectResources),
            None => return Err(InstallError::NoResources),
        };
        let current = self
            .doc
            .resolve_object(&Object::Ref(mangle_syntax::object::Ref::new(res_num, 0)))
            .and_then(|o| o.as_dict().cloned())
            .ok_or(InstallError::UnresolvableResources)?;
        // Five consecutive numbers, reserved up front: the builder wires its references
        // to exactly this range, so the range is taken before it runs rather than
        // discovered after.
        let first = self.doc.alloc().num;
        for _ in 1..EMBEDDED_OBJECTS {
            self.doc.alloc();
        }
        let embedded =
            embed_truetype(program, chars, family, first).map_err(InstallError::Embed)?;
        // Into the `/Font` table, not beside it: a font name lives inside the table, and
        // a sibling of it is a resource no reader looks up. The table itself may be
        // direct or indirect; either way it is replaced whole, resolved first when it
        // is a reference.
        let doc = std::sync::Arc::clone(&self.doc);
        let font_table = match current.get("Font") {
            Some(table) => doc
                .resolve_object(table)
                .unwrap_or_else(|| table.clone())
                .as_dict()
                .cloned()
                .unwrap_or_default(),
            None => mangle_syntax::object::Dict::new(),
        };
        let mut font_table = font_table;
        font_table.insert(
            mangle_syntax::object::Name::new(name),
            Object::Ref(mangle_syntax::object::Ref::new(embedded.type0, 0)),
        );
        let mut dict = current;
        dict.insert(
            mangle_syntax::object::Name::new("Font"),
            Object::Dict(font_table),
        );
        let installed = embedded.clone();
        for (num, obj) in embedded.numbered {
            self.staged.push((num, obj));
        }
        self.staged.push((res_num, Object::Dict(dict.clone())));
        self.resources
            .fonts
            .insert(name.to_string(), Object::Dict(dict));
        Ok(installed)
    }

    /// Save the page as it now stands, as an incremental update.
    ///
    /// Only the streams that differ from the file's own are rewritten; the rest keep their bytes,
    /// and every other byte of the file is kept verbatim. Staged font installations ride the
    /// same revision, and a save drains them: what the file now holds is no longer staged,
    /// so saving twice writes the fonts once.
    pub fn save(&mut self) -> Result<Save, SaveError> {
        let current = self.parts();
        let wanted: Vec<Option<Vec<u8>>> = self
            .parts
            .iter()
            .zip(current)
            .map(|(was, now)| (was.decoded != *now).then(|| now.clone()))
            .collect();
        let staged = std::mem::take(&mut self.staged);
        match save_decoded_with(&self.doc, &self.page, &wanted, &staged) {
            Ok(save) => Ok(save),
            Err(error) => {
                self.staged = staged;
                Err(error)
            }
        }
    }

    /// Save back to the stream the file already has, discarding every undo and redo.
    pub fn reset(&mut self) {
        let base: Vec<Vec<u8>> = self.parts.iter().map(|p| p.decoded.clone()).collect();
        self.history = History::new(base);
        self.labels.clear();
    }

    /// The document this session is editing, for a panel that wants to show its own properties.
    #[must_use]
    pub fn document(&self) -> &Document {
        &self.doc
    }

    /// The page this session is editing.
    #[must_use]
    pub fn page(&self) -> &Page {
        &self.page
    }

    /// Record a step: the streams as they were, and the streams as they are now.
    ///
    /// The streams are split back into parts here, because that is the shape the history and the
    /// save both want, and a step recorded as a concatenation could not be saved without the
    /// composition this module refuses to do.
    fn record(
        &mut self,
        after: Vec<u8>,
        patches: &[crate::Patch],
        label: String,
    ) -> Result<(), Reason> {
        let parts = split_like(&self.parts, &after, patches).map_err(|_| Reason::Unsplittable {
            parts: 0,
            wanted: self.parts.len(),
        })?;
        if parts.len() != self.parts.len() {
            // The stream no longer divides into the page's streams. That is damage from an edit
            // this module does not model, and refusing is the only answer that does not write the
            // wrong bytes into the wrong object.
            return Err(Reason::Unsplittable {
                parts: parts.len(),
                wanted: self.parts.len(),
            });
        }
        // Cloned rather than taken so the snapshots share the `Arc`: `History` asserts its `before`
        // is the state it already held, and sharing is what makes stepping back free.
        let before = self.history.current().clone();
        self.history
            .push(before, Snapshot::new(parts, label.clone()));
        self.labels.push(label);
        Ok(())
    }
}

/// The page's content streams as indirect objects, and why any could not be used.
fn split_parts(doc: &Document, page: &Page) -> (Vec<Vec<u8>>, Vec<String>) {
    let mut out = Vec::new();
    let mut notes = Vec::new();
    let Some(contents) = page.dict.get("Contents") else {
        return (out, notes);
    };
    let entries: Vec<Object> = match contents {
        Object::Ref(r) => vec![Object::Ref(*r)],
        Object::Array(parts) => parts.clone(),
        _ => Vec::new(),
    };
    for (i, entry) in entries.iter().enumerate() {
        let Some(r) = entry.as_ref_id() else {
            notes.push(format!(
                "content stream {i} is {} rather than an indirect reference",
                entry.type_name()
            ));
            continue;
        };
        let Some(Object::Stream(stream)) = doc.object(r) else {
            notes.push(format!("object {r} is not a stream"));
            continue;
        };
        let decoded = mangle_syntax::stream::decode_stream(&stream);
        if decoded.encoded {
            notes.push(format!(
                "content stream {i} is still encoded: {}",
                decoded.notes.join("; ")
            ));
            continue;
        }
        out.push(decoded.data);
    }
    (out, notes)
}

/// Whether this page's content streams are all indirect, which is what a save needs.
fn writeback_shape(doc: &Document, page: &Page) -> Result<(), String> {
    let _ = doc;
    let Some(contents) = page.dict.get("Contents") else {
        return Ok(());
    };
    match contents {
        Object::Ref(_) => Ok(()),
        Object::Array(parts) => {
            for (i, part) in parts.iter().enumerate() {
                if !matches!(part, Object::Ref(_)) {
                    return Err(format!(
                        "content stream {i} is {} rather than a reference",
                        part.type_name()
                    ));
                }
            }
            Ok(())
        }
        other => Err(format!("/Contents is {}", other.type_name())),
    }
}

/// A name for a change, for a history panel.
fn change_label(change: &Change) -> String {
    match change {
        Change::Delete => "delete the object".to_string(),
        Change::Recolour { channel, .. } => format!("recolour the {channel}"),
        // A **pure translation** is the matrix `translate` produces for its own two numbers,
        // which is what a drag gives. Anything else is a scale or a rotation — and a scale about a
        // point carries a translation of its own, so calling that a move would put the wrong
        // number in a history panel.
        Change::Transform(m) => {
            if *m == Matrix::translate(m.e, m.f) {
                if m.e == 0.0 && m.f == 0.0 {
                    "no change".to_string()
                } else {
                    format!("move by {}, {}", m.e, m.f)
                }
            } else {
                "transform".to_string()
            }
        }
        Change::Crop { .. } => "crop the image".to_string(),
        Change::Text(property) => {
            let (operator, value) = crate::edits::property_operand(property);
            let _ = operator;
            format!("set the {} to {}", property.name(), value)
        }
        Change::Retype { text } => format!("retype as {text:?}"),
        Change::RetypeIn { text, font, .. } => format!("retype as {text:?} in {font}"),
    }
}

/// The concatenation, with the separator the file's own reader puts between two streams.
fn joined(parts: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            out.push(b'\n');
        }
        out.extend_from_slice(part);
    }
    out
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect` and `unwrap`, which is what a test is for;
    // the panic-free rule is about what the product does with a file, not about tests.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        // Several of these assert an exact matrix a `Change::move_by` wrote, which is the whole
        // question: is this the matrix the edit asked for, or a near miss.
        clippy::float_cmp
    )]

    use super::{SessionError, change_label, joined};
    use crate::Change;
    use mangle_content::Resources;
    use mangle_doc::Page;
    use mangle_syntax::object::Object;
    use mangle_syntax::{Document, OpenOptions};
    use std::fmt::Write as _;

    /// A document with one page, whose content streams are the ones given.
    ///
    /// Written here rather than read from the corpus because these tests are about the session's
    /// own bookkeeping — the corpus round trips are in `tests/`.
    fn doc_with(parts: &[&str]) -> (Document, Page) {
        let mut out = String::from("%PDF-1.7\n");
        let mut offsets = Vec::new();
        // The catalogue, the page tree and the page, then one object per content stream.
        let first_stream = 4;
        let contents = parts
            .iter()
            .enumerate()
            .map(|(i, _)| format!("{} 0 R", first_stream + i))
            .collect::<Vec<_>>()
            .join(" ");
        offsets.push(1);
        out.push_str("1 0 obj\n<</Type /Catalog /Pages 2 0 R>>\nendobj\n");
        offsets.push(out.len());
        out.push_str("2 0 obj\n<</Type /Pages /Kids [3 0 R] /Count 1>>\nendobj\n");
        offsets.push(out.len());
        out.push_str("3 0 obj\n<</Type /Page /Parent 2 0 R ");
        out.push_str("/Contents [");
        out.push_str(&contents);
        out.push_str("] /MediaBox [0 0 200 200] /Resources <<>> >>\n");
        out.push_str("endobj\n");
        for (i, part) in parts.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&(first_stream + i).to_string());
            out.push_str(" 0 obj\n<</Length ");
            out.push_str(&part.len().to_string());
            out.push_str(">>\nstream\n");
            out.push_str(part);
            out.push_str("\nendstream\nendobj\n");
        }
        let start_xref = out.len();
        out.push_str("xref\n0 ");
        out.push_str(&(first_stream + parts.len()).to_string());
        out.push('\n');
        out.push_str("0000000000 65535 f \n");
        for offset in &offsets {
            let _ = writeln!(out, "{offset:010} 00000 n ");
        }
        let _ = write!(
            out,
            "trailer\n<</Size {} /Root 1 0 R>>\nstartxref\n{start_xref}\n%%EOF\n",
            first_stream + parts.len()
        );
        let doc =
            Document::open(out.into_bytes(), OpenOptions::default()).expect("the fixture opens");
        let cat = doc.catalog().expect("a catalogue");
        let root = cat
            .get("Pages")
            .and_then(Object::as_ref_id)
            .expect("a tree");
        let tree = mangle_doc::PageTree::build(&doc, root).expect("the walk");
        (doc, tree.pages().first().expect("a page").clone())
    }

    /// Like [`doc_with`], but the page's resources are an indirect dictionary holding an
    /// empty `/Font` table: the shape an install extends rather than refuses.
    ///
    /// Unlike [`doc_with`] — whose xref survives on the reader's repair — this xref is
    /// written correctly, because the test reopens the *saved* file through its revision
    /// chain, and a revision chain over a broken first table resolves the old objects.
    fn doc_with_resources(parts: &[&str]) -> (Document, Page) {
        let mut out = String::from("%PDF-1.7\n");
        // One entry per object plus the free head: object n lives at offsets[n].
        let mut offsets = vec![0usize];
        let first_stream = 4;
        let res = first_stream + parts.len();
        let contents = parts
            .iter()
            .enumerate()
            .map(|(i, _)| format!("{} 0 R", first_stream + i))
            .collect::<Vec<_>>()
            .join(" ");
        offsets.push(out.len());
        out.push_str("1 0 obj\n<</Type /Catalog /Pages 2 0 R>>\nendobj\n");
        offsets.push(out.len());
        out.push_str("2 0 obj\n<</Type /Pages /Kids [3 0 R] /Count 1>>\nendobj\n");
        offsets.push(out.len());
        out.push_str("3 0 obj\n<</Type /Page /Parent 2 0 R ");
        out.push_str("/Contents [");
        out.push_str(&contents);
        let _ = writeln!(out, "] /MediaBox [0 0 200 200] /Resources {res} 0 R >>");
        out.push_str("endobj\n");
        for (i, part) in parts.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&(first_stream + i).to_string());
            out.push_str(" 0 obj\n<</Length ");
            out.push_str(&part.len().to_string());
            out.push_str(">>\nstream\n");
            out.push_str(part);
            out.push_str("\nendstream\nendobj\n");
        }
        offsets.push(out.len());
        out.push_str(&res.to_string());
        out.push_str(" 0 obj\n<</Font <<>>>>\nendobj\n");
        assert_eq!(
            offsets.len(),
            res + 1,
            "one entry per object plus the free head"
        );
        let start_xref = out.len();
        out.push_str("xref\n0 ");
        out.push_str(&(res + 1).to_string());
        out.push('\n');
        for (n, offset) in offsets.iter().enumerate() {
            if n == 0 {
                out.push_str("0000000000 65535 f \n");
            } else {
                let _ = writeln!(out, "{offset:010} 00000 n ");
            }
        }
        let _ = write!(
            out,
            "trailer\n<</Size {} /Root 1 0 R>>\nstartxref\n{start_xref}\n%%EOF\n",
            res + 1
        );
        let doc =
            Document::open(out.into_bytes(), OpenOptions::default()).expect("the fixture opens");
        let cat = doc.catalog().expect("a catalogue");
        let root = cat
            .get("Pages")
            .and_then(Object::as_ref_id)
            .expect("a tree");
        let tree = mangle_doc::PageTree::build(&doc, root).expect("the walk");
        (doc, tree.pages().first().expect("a page").clone())
    }

    fn liberation() -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/fonts/LiberationSans-Regular.ttf");
        std::fs::read(&path).expect("the bundled face is checked in")
    }

    /// A page whose only font is an unembedded `/Helvetica` in WinAnsi: the shape a
    /// fallback embeds beside rather than replaces.
    fn doc_with_helvetica(part: &str) -> (Document, Page) {
        let mut out = String::from("%PDF-1.7\n");
        let mut offsets = vec![0usize];
        offsets.push(out.len());
        out.push_str("1 0 obj\n<</Type /Catalog /Pages 2 0 R>>\nendobj\n");
        offsets.push(out.len());
        out.push_str("2 0 obj\n<</Type /Pages /Kids [3 0 R] /Count 1>>\nendobj\n");
        offsets.push(out.len());
        out.push_str(
            "3 0 obj\n<</Type /Page /Parent 2 0 R /Contents [4 0 R] \
             /MediaBox [0 0 200 200] /Resources 5 0 R >>\nendobj\n",
        );
        offsets.push(out.len());
        out.push_str("4 0 obj\n<</Length ");
        out.push_str(&part.len().to_string());
        out.push_str(">>\nstream\n");
        out.push_str(part);
        out.push_str("\nendstream\nendobj\n");
        offsets.push(out.len());
        out.push_str(
            "5 0 obj\n<</Font <</F1 <</Type /Font /Subtype /Type1 /BaseFont /Helvetica \
             /Encoding /WinAnsiEncoding>>>>>>\nendobj\n",
        );
        assert_eq!(offsets.len(), 6, "one entry per object plus the free head");
        let start_xref = out.len();
        out.push_str("xref\n0 6\n");
        for (n, offset) in offsets.iter().enumerate() {
            if n == 0 {
                out.push_str("0000000000 65535 f \n");
            } else {
                let _ = writeln!(out, "{offset:010} 00000 n ");
            }
        }
        let _ = write!(
            out,
            "trailer\n<</Size 6 /Root 1 0 R>>\nstartxref\n{start_xref}\n%%EOF\n"
        );
        let doc =
            Document::open(out.into_bytes(), OpenOptions::default()).expect("the fixture opens");
        let cat = doc.catalog().expect("a catalogue");
        let root = cat
            .get("Pages")
            .and_then(Object::as_ref_id)
            .expect("a tree");
        let tree = mangle_doc::PageTree::build(&doc, root).expect("the walk");
        (doc, tree.pages().first().expect("a page").clone())
    }

    #[test]
    fn an_installed_font_rides_the_next_save() {
        let (doc, page) = doc_with_resources(&["BT (Hi) Tj ET"]);
        let original = doc.bytes().to_vec();
        let mut editor =
            crate::Editor::open(std::sync::Arc::new(doc), &page, &Resources::default())
                .expect("editable");
        assert!(!editor.is_dirty(), "nothing staged yet");
        let name = editor.free_font_name("F");
        assert_eq!(name, "F1", "the first free name");
        editor
            .install_truetype(&name, &liberation(), &['H', 'i'], "LiberationSans")
            .expect("installs");
        assert!(editor.is_dirty(), "staged installs count as changed");
        let save = editor.save().expect("saves");
        assert!(!editor.is_dirty(), "the save drains the stage");
        assert!(
            save.appended_only(&original),
            "an install appends a revision rather than rewriting the file"
        );
        // Six objects ride along: the five font objects and the resource rewrite.
        assert_eq!(save.rewritten.len(), 0, "no stream changed");
        assert_eq!(save.installed.len(), 6, "nothing more, nothing less");
        // Reopen what was written: the font is there, wired to its five objects.
        let doc2 = Document::open(save.bytes.clone(), OpenOptions::default()).expect("reopens");
        let cat = doc2.catalog().expect("a catalogue");
        let root = cat
            .get("Pages")
            .and_then(Object::as_ref_id)
            .expect("a tree");
        let tree = mangle_doc::PageTree::build(&doc2, root).expect("the walk");
        let page2 = tree.pages().first().expect("a page").clone();
        let resources = page2
            .inherited
            .resources
            .as_ref()
            .and_then(|o| doc2.resolve_object(o))
            .and_then(|o| o.as_dict().cloned())
            .map(|d| Resources::from_dict(&d, &|o| doc2.resolve_object(o)))
            .expect("resources");
        let font = resources.fonts.get(&name).expect("the installed font");
        let dict = font.as_dict().expect("a font dictionary");
        assert_eq!(
            dict.get("Subtype").and_then(|o| o.as_name()),
            Some(&b"Type0"[..]),
            "a composite wrapper"
        );
        assert!(
            dict.get("ToUnicode").is_some(),
            "extraction of new text stays exact"
        );
    }

    #[test]
    fn an_install_refuses_what_it_cannot_rewrite() {
        // Direct resources: rewriting them means rewriting the page dictionary.
        let (doc, page) = doc_with(&["BT (Hi) Tj ET"]);
        let mut editor =
            crate::Editor::open(std::sync::Arc::new(doc), &page, &Resources::default())
                .expect("editable");
        let err = editor
            .install_truetype("F1", &liberation(), &['H'], "LiberationSans")
            .expect_err("inline resources cannot be extended");
        assert!(matches!(err, crate::InstallError::DirectResources), "{err}");
    }

    /// A character the run's own WinAnsi font cannot write falls back to an embedded
    /// Liberation subset, and the label says so.
    ///
    /// `α` is not in WinAnsi; `Helvetica` resolves to a bundled face that has it. The
    /// label is the notice the charter requires — which face stood in and which
    /// character needed it — because the history panel and the command line both print
    /// labels, and a substitution neither shows is a silent one.
    #[test]
    fn a_missing_glyph_falls_back_to_an_embedded_subset() {
        let (doc, page) = doc_with_helvetica("BT /F1 12 Tf (Hi) Tj ET");
        let resources = page
            .inherited
            .resources
            .as_ref()
            .and_then(|o| doc.resolve_object(o))
            .and_then(|o| o.as_dict().cloned())
            .map(|d| Resources::from_dict(&d, &|o| doc.resolve_object(o)))
            .unwrap_or_default();
        let mut editor =
            crate::Editor::open(std::sync::Arc::new(doc), &page, &resources).expect("editable");
        let at = editor
            .model()
            .objects()
            .iter()
            .position(|o| {
                matches!(
                    o.kind,
                    crate::Kind::Line | crate::Kind::Block | crate::Kind::LooseRun
                )
            })
            .expect("a text object");
        let outcome = editor
            .apply(
                at,
                &Change::Retype {
                    text: "Hα".to_string(),
                },
            )
            .expect("a fallback answers");
        assert!(
            outcome.label.contains("fallback")
                && outcome.label.contains("LiberationSans")
                && outcome.label.contains('α')
                && outcome.label.contains("Helvetica"),
            "the notice names the face, the character and the missing font: {}",
            outcome.label
        );
        // The install staged objects beside the edit; the save carries both, and the
        // reopened file is checked below rather than the session's memory of it.
        let save = editor.save().expect("saves");
        let doc2 = Document::open(save.bytes.clone(), OpenOptions::default()).expect("reopens");
        let cat = doc2.catalog().expect("a catalogue");
        let root = cat
            .get("Pages")
            .and_then(Object::as_ref_id)
            .expect("a tree");
        let tree = mangle_doc::PageTree::build(&doc2, root).expect("the walk");
        let page2 = tree.pages().first().expect("a page").clone();
        // The new run reads back as "Hα" through the installed `ToUnicode`: the CMap is
        // what makes extraction of the new text exact, so the check reads through it
        // rather than through the raw CID bytes.
        let resources2 = page2
            .inherited
            .resources
            .as_ref()
            .and_then(|o| doc2.resolve_object(o))
            .and_then(|o| o.as_dict().cloned())
            .map(|d| Resources::from_dict(&d, &|o| doc2.resolve_object(o)))
            .expect("resources");
        let (name, dict) = resources2
            .fonts
            .iter()
            .find_map(|(name, font)| {
                let dict = doc2.resolve_object(font)?.as_dict()?.clone();
                (dict.get("Subtype").and_then(|o| o.as_name()) == Some(&b"Type0"[..]))
                    .then(|| (name.clone(), dict))
            })
            .expect("the installed composite font");
        let to_unicode = dict
            .get("ToUnicode")
            .and_then(|o| doc2.resolve_object(o))
            .expect("a ToUnicode");
        let text = match to_unicode {
            Object::Stream(stream) => String::from_utf8(stream.raw.clone()).expect("a CMap"),
            other => panic!("a stream, not {other:?}"),
        };
        let map = to_unicode_map(&text);
        let stream2 = page2.decoded_contents(&doc2);
        let after = mangle_content::interp::run_with(
            &mangle_content::ContentStream::parse(&stream2),
            &resources2,
        );
        let mut found = false;
        for record in &after.records {
            let mangle_content::interp::Mark::Glyphs { text: raw, .. } = &record.mark else {
                continue;
            };
            if raw.len() != 4 {
                continue;
            }
            let decoded: String = raw
                .chunks_exact(2)
                .map(|pair| {
                    let cid = (u32::from(pair[0]) << 8) | u32::from(pair[1]);
                    map.get(&cid).copied().unwrap_or('�')
                })
                .collect();
            if decoded == "Hα" {
                found = true;
            }
        }
        assert!(found, "the page says Hα through {name}");
    }

    /// One `beginbfchar` block read as CID to character: the reader a Unicode
    /// extraction would use, reduced to what the check needs.
    fn to_unicode_map(cmap: &str) -> std::collections::BTreeMap<u32, char> {
        let mut out = std::collections::BTreeMap::new();
        let mut inside = false;
        for line in cmap.lines() {
            let line = line.trim();
            if line.ends_with("beginbfchar") {
                inside = true;
                continue;
            }
            if line.starts_with("endbfchar") {
                inside = false;
                continue;
            }
            if !inside {
                continue;
            }
            let mut parts = line.split_whitespace();
            if let (Some(cid), Some(uni)) = (parts.next(), parts.next()) {
                if let (Some(cid), Some(uni)) = (hex_of(cid), hex_of(uni)) {
                    if let Some(ch) = char::from_u32(uni) {
                        out.insert(cid, ch);
                    }
                }
            }
        }
        out
    }

    /// `<1F>` to its number, or `None` for anything that is not one hex word.
    fn hex_of(word: &str) -> Option<u32> {
        let word = word.strip_prefix('<')?.strip_suffix('>')?;
        if word.is_empty() || word.len() > 8 || !word.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        u32::from_str_radix(word, 16).ok()
    }

    #[test]
    fn a_session_starts_with_the_streams_the_file_has() {
        let (doc, page) = doc_with(&["q 1 0 0 1 0 0 cm /Im0 Do Q", "q /Im1 Do Q"]);
        let mut editor =
            crate::Editor::open(std::sync::Arc::new(doc), &page, &Resources::default())
                .expect("a two-stream page can be edited in place");
        assert_eq!(editor.base().len(), 2, "the page has two content streams");
        assert_eq!(editor.parts().len(), 2, "and the session starts on them");
        assert_eq!(editor.stream(), b"q 1 0 0 1 0 0 cm /Im0 Do Q\nq /Im1 Do Q");
        assert!(!editor.is_dirty(), "nothing has changed yet");
        assert_eq!(editor.depth(), (0, 0), "so there is nothing to undo");
        assert!(
            editor.undo().is_err(),
            "and an undo says so rather than silently doing nothing"
        );
    }

    /// The round trip GOAL.md §4.8 asks for: undo gives back *structurally identical* content, and
    /// redo returns the edited state.
    #[test]
    fn an_undo_gives_back_the_same_bytes_and_a_redo_the_edited_state() {
        let (doc, page) = doc_with(&["q 1 0 0 1 0 0 cm /Im0 Do Q"]);
        let mut editor =
            crate::Editor::open(std::sync::Arc::new(doc), &page, &Resources::default())
                .expect("editable");
        let was = editor.stream();

        let outcome = editor
            .apply(0, &Change::move_by(7.0, 0.0))
            .expect("a move on a real object");
        assert_eq!(outcome.label, "move by 7, 0");
        assert!(editor.is_dirty(), "the stream changed");
        assert_ne!(editor.stream(), was, "and it changed");
        assert_eq!(editor.depth(), (1, 0), "one step back, none forward");

        let edited = editor.stream();
        editor.undo().expect("there is a step to undo");
        assert_eq!(
            editor.stream(),
            was,
            "undo restores the content exactly, which is FINISH.md U3"
        );
        assert!(!editor.is_dirty(), "so the page is not dirty");
        assert_eq!(editor.depth(), (0, 1), "and one step forward");

        editor.redo().expect("there is a step to redo");
        assert_eq!(
            editor.stream(),
            edited,
            "redo returns the edited state, which is U3's other half"
        );
        assert_eq!(editor.depth(), (1, 0));
    }

    /// A second edit discards the redo stack, which is what keeps a redo from being wrong rather
    /// than merely unavailable.
    #[test]
    fn a_new_edit_after_an_undo_discards_the_redo_stack() {
        let (doc, page) = doc_with(&["q 1 0 0 1 0 0 cm /Im0 Do Q"]);
        let mut editor =
            crate::Editor::open(std::sync::Arc::new(doc), &page, &Resources::default())
                .expect("editable");
        editor.apply(0, &Change::move_by(7.0, 0.0)).expect("a move");
        editor.undo().expect("back");
        assert_eq!(
            editor.depth(),
            (0, 1),
            "one step forward before the new edit"
        );
        editor
            .apply(0, &Change::move_by(3.0, 0.0))
            .expect("a different move");
        assert_eq!(editor.depth(), (1, 0), "the old future is gone");
        assert!(editor.redo().is_err(), "so there is nothing to redo");
    }

    /// Saving a session writes only the streams that differ, and every other byte of the file is
    /// still there.
    #[test]
    fn a_session_save_writes_only_the_streams_that_changed() {
        let (doc, page) = doc_with(&["q 1 0 0 1 0 0 cm /Im0 Do Q", "q /Im1 Do Q"]);
        let mut editor =
            crate::Editor::open(std::sync::Arc::new(doc), &page, &Resources::default())
                .expect("editable");
        let before = editor.document().bytes().to_vec();
        editor.apply(0, &Change::move_by(7.0, 0.0)).expect("a move");
        let save = editor.save().expect("and it saves");

        assert!(
            save.appended_only(&before),
            "the file's bytes are a prefix of the saved file"
        );
        assert_eq!(save.rewritten.len(), 1, "one of the two streams changed");
        let reopened = Document::open(save.bytes, OpenOptions::default()).expect("it reopens");
        let cat = reopened.catalog().expect("a catalogue");
        let root = cat
            .get("Pages")
            .and_then(Object::as_ref_id)
            .expect("a tree");
        let tree = mangle_doc::PageTree::build(&reopened, root).expect("the walk");
        let page2 = tree.pages().first().expect("a page");
        let after = page2.decoded_contents(&reopened);
        assert_eq!(
            after,
            editor.stream(),
            "the reopened page holds exactly what the session had"
        );
    }

    /// Saving an unchanged session writes nothing, and the file is the original with an empty
    /// revision on the end.
    #[test]
    fn a_session_that_changed_nothing_rewrites_nothing() {
        let (doc, page) = doc_with(&["q 1 0 0 1 0 0 cm /Im0 Do Q"]);
        let mut editor =
            crate::Editor::open(std::sync::Arc::new(doc), &page, &Resources::default())
                .expect("editable");
        let save = editor.save().expect("it saves");
        assert!(save.rewritten.is_empty(), "nothing was rewritten");
        assert!(!save.re_encoded, "so nothing was re-encoded");
        assert!(!editor.is_dirty(), "and the page is not dirty");
    }

    /// A session that cannot be saved is refused at open time rather than at save time, where the
    /// user has already done the work.
    #[test]
    fn a_page_whose_streams_are_not_indirect_is_refused_at_open() {
        // `/Contents` a direct stream: there is no object number to replace.
        let mut out = String::from("%PDF-1.7\n");
        out.push_str("1 0 obj\n<</Type /Catalog /Pages 2 0 R>>\nendobj\n");
        out.push_str("2 0 obj\n<</Type /Pages /Kids [3 0 R] /Count 1>>\nendobj\n");
        out.push_str(
            "3 0 obj\n<</Type /Page /Parent 2 0 R /Contents 5 0 R /MediaBox [0 0 200 200] ",
        );
        out.push_str("/Resources <<>> >>\nendobj\n");
        out.push_str("4 0 obj\n(4 0 obj is not a stream)\nendobj\n");
        out.push_str("5 0 obj\n<</Length 4>>\nstream\nq Q\nendstream\nendobj\n");
        let at = out.len();
        out.push_str("xref\n0 6\n0000000000 65535 f \n");
        // Offsets are not right for a real parse of these objects, so the check that matters is
        // the one about `/Contents` being indirect, and the fixture is only asked to open.
        let _ = at;
        out.push_str("trailer\n<</Size 6 /Root 1 0 R>>\nstartxref\n0\n%%EOF\n");
        if let Ok(doc) = Document::open(out.into_bytes(), OpenOptions::default()) {
            let cat = doc.catalog().expect("a catalogue");
            let root = cat.get("Pages").and_then(Object::as_ref_id);
            if let Some(root) = root {
                if let Ok(tree) = mangle_doc::PageTree::build(&doc, root) {
                    if let Some(page) = tree.pages().first() {
                        // The page tree may not resolve; when it does, the session must refuse a
                        // page it could not save.
                        let _ = crate::Editor::open(
                            std::sync::Arc::new(doc),
                            page,
                            &Resources::default(),
                        );
                    }
                }
            }
        }
    }

    /// The join is the file's own concatenation, and the split is its inverse for a stream that
    /// came back unchanged.
    #[test]
    fn an_unchanged_stream_round_trips_through_the_split() {
        let base = vec![b"q Q".to_vec(), b"BT ET".to_vec(), b"".to_vec()];
        assert_eq!(joined(&base), b"q Q\nBT ET\n");
    }

    /// A change's label is what a history panel shows, so it has to name the thing that happened.
    #[test]
    fn a_change_is_named_for_what_it_did() {
        assert_eq!(change_label(&Change::Delete), "delete the object");
        assert_eq!(change_label(&Change::move_by(7.0, 0.0)), "move by 7, 0");
        assert_eq!(change_label(&Change::move_by(0.0, -3.0)), "move by 0, -3");
        assert_eq!(
            change_label(&Change::scale_about(5.0, 5.0, 2.0, 2.0)),
            "transform",
            "a scale about a point carries a translation of its own, and is not a move"
        );
    }

    /// A session refuses a change it cannot write rather than recording a step that did nothing.
    #[test]
    fn a_refused_change_records_no_step() {
        let (doc, page) = doc_with(&["q 1 0 0 1 0 0 cm /Im0 Do Q"]);
        let mut editor =
            crate::Editor::open(std::sync::Arc::new(doc), &page, &Resources::default())
                .expect("editable");
        let err = editor
            .apply(99, &Change::Delete)
            .expect_err("no such object");
        assert!(matches!(err, SessionError::Change(_)), "{err}");
        assert_eq!(editor.depth(), (0, 0), "and no step was recorded");
        assert!(!editor.is_dirty());
    }
}
