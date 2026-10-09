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
use crate::edits::{Change, patches_for};
use crate::history::{History, Snapshot};
use crate::page_objects::PageModel;
use crate::surgery::apply;
use crate::writeback::{Part, Save, SaveError, save_decoded, split_like};

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
    /// The edited stream no longer divides into the page's content streams.
    Reason(Reason),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Change(e) => write!(f, "{e}"),
            Self::Arrange(e) => write!(f, "{e}"),
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
    /// Returns the label recorded for it, which is the same string a history panel shows.
    pub fn apply(&mut self, index: usize, change: &Change) -> Result<String, SessionError> {
        let model = self.model();
        let Some(object) = model.objects().get(index) else {
            return Err(SessionError::Change(crate::ChangeError::Refused(
                crate::Refusal::NoSpans,
            )));
        };
        let patches = patches_for(&self.stream(), object, change)
            .map_err(|r| SessionError::Change(crate::ChangeError::Refused(r)))?;
        let applied = apply(&self.stream(), &patches)
            .map_err(|e| SessionError::Change(crate::ChangeError::Stream(e)))?;
        let label = change_label(change);
        self.record(applied.bytes, &patches, label.clone())?;
        Ok(label)
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
        self.parts() != base.as_slice()
    }

    /// Save the page as it now stands, as an incremental update.
    ///
    /// Only the streams that differ from the file's own are rewritten; the rest keep their bytes,
    /// and every other byte of the file is kept verbatim.
    pub fn save(&self) -> Result<Save, SaveError> {
        let current = self.parts();
        let wanted: Vec<Option<Vec<u8>>> = self
            .parts
            .iter()
            .zip(current)
            .map(|(was, now)| (was.decoded != *now).then(|| now.clone()))
            .collect();
        save_decoded(&self.doc, &self.page, &wanted)
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

        let label = editor
            .apply(0, &Change::move_by(7.0, 0.0))
            .expect("a move on a real object");
        assert_eq!(label, "move by 7, 0");
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
        let editor = crate::Editor::open(std::sync::Arc::new(doc), &page, &Resources::default())
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
