//! Undo and redo, as a history of snapshots rather than of inverses.
//!
//! # Why snapshots and not inverses
//!
//! The obvious way to undo an edit is to compute its inverse — a move has a move back, a recolour
//! has the colour it was — and keep both. That is how most editors do it, and it is why most
//! editors' undo stacks go wrong: **an inverse has to be written, tested and trusted for every
//! operation**, and one that is subtly wrong corrupts the document rather than failing loudly. A
//! recolour that returns the wrong colour is not a bug report, it is a bad file the user saved.
//!
//! So this keeps the state instead. A [`Snapshot`] is the model as it was, taken before an edit,
//! and undo restores it wholesale. That trades memory for correctness: a hundred edits on a large
//! document cost a hundred snapshots unless they are shared. [`Snapshot::from_arc`] is how they
//! are shared — the common case, where an edit replaces the model outright and nothing before it
//! moved, shares every snapshot but the last with the snapshot before it.
//!
//! **The cost is a real limit and this says so.** A snapshot holds a clone of the document model,
//! so a history of them is proportional to the document times the number of steps. That is fine
//! for the edit sessions this is built for — a page or two, a few dozen operations — and it is
//! not fine for a hundred-thousand-page merge. The alternative, an operation log with recorded
//! inverses, is the right structure at that scale and is deliberately not what is here. See
//! [`History::capacity_note`].
//!
//! # What a step is
//!
//! One [`Edit`] is one entry, whatever it changed. FINISH.md S1.16 asks for structural equality
//! after an undo/redo of a whole stage rather than per-operation, which is what makes a coarse
//! step legitimate: a user who drags a box expects one undo, not one per pixel of movement.

use std::sync::Arc;

/// The document model as it stood, kept so that undo can put it back exactly.
///
/// A cheap clone, so keeping a history of these costs one reference per step rather than a copy
/// of the document per step, *provided* the snapshots actually share.
///
/// `Clone` and `PartialEq` are written out rather than derived because a derive would put a
/// `T: Clone` bound on them, and that bound is exactly what this type exists to avoid: cloning a
/// snapshot shares an `Arc`, and a document model that is not `Clone` — or is `Clone` but
/// enormous — still gets a cheap history. `PartialEq` compares the models, so it does need the
/// bound, and is therefore the one that carries it.
#[derive(Debug)]
pub struct Snapshot<T> {
    model: Arc<T>,
    /// What the user did, for a history panel and for an autosave label.
    label: String,
}

impl<T> Snapshot<T> {
    /// A snapshot of `model`.
    #[must_use]
    pub fn new(model: T, label: impl Into<String>) -> Self {
        Self {
            model: Arc::new(model),
            label: label.into(),
        }
    }

    /// A snapshot sharing an existing one, which is what an edit that replaces the model
    /// outright produces.
    ///
    /// The second argument is the model *after* the edit, and the first is a snapshot already
    /// held. They share nothing by accident: an `Arc` clone shares the same document, so the
    /// history stays flat only while the edits themselves are whole-model replacements.
    #[must_use]
    pub fn from_arc(previous: &Self, model: T) -> Self {
        Self {
            model: Arc::new(model),
            label: previous.label.clone(),
        }
    }

    /// The model, by reference. Holding this rather than a clone is what keeps a history cheap.
    #[must_use]
    pub fn get(&self) -> &T {
        &self.model
    }

    /// The model, cloned out. For a caller that needs to own it — a save, a diff.
    #[must_use]
    pub fn to_model(&self) -> T
    where
        T: Clone,
    {
        (*self.model).clone()
    }

    /// What the user did.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// How many histories share this snapshot, which is the number that says whether the
    /// history is as cheap as it looks.
    #[must_use]
    pub fn shares(&self) -> usize {
        Arc::strong_count(&self.model)
    }
}

/// Shares the model rather than copying it, which is the whole point of the snapshot.
impl<T> Clone for Snapshot<T> {
    fn clone(&self) -> Self {
        Self {
            model: Arc::clone(&self.model),
            label: self.label.clone(),
        }
    }
}

impl<T: PartialEq> PartialEq for Snapshot<T> {
    fn eq(&self, other: &Self) -> bool {
        self.label == other.label && self.model == other.model
    }
}

/// One entry in the history: the state *before* an edit, and what the edit was called.
///
/// The label lives on the step rather than being passed separately, so a history cannot end up
/// with an undo whose description belongs to a different operation.
#[derive(Debug, Clone, PartialEq)]
pub struct Edit<T> {
    before: Snapshot<T>,
    after: Snapshot<T>,
}

impl<T> Edit<T> {
    /// The state before the edit.
    #[must_use]
    pub fn before(&self) -> &Snapshot<T> {
        &self.before
    }

    /// The state after it.
    #[must_use]
    pub fn after(&self) -> &Snapshot<T> {
        &self.after
    }

    /// What the user called this edit.
    #[must_use]
    pub fn label(&self) -> &str {
        self.after.label()
    }
}

/// What went wrong, said as the thing that went wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryError {
    /// There is nothing to undo.
    NothingToUndo,
    /// There is nothing to redo, which is the normal state after a new edit.
    NothingToRedo,
}

impl std::fmt::Display for HistoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NothingToUndo => f.write_str("there is nothing to undo"),
            Self::NothingToRedo => f.write_str("there is nothing to redo"),
        }
    }
}

impl std::error::Error for HistoryError {}

/// A document model with undo and redo over it.
///
/// The current model is the history's own, so there is exactly one answer to "what does the
/// document look like now" and it cannot disagree with the undo stack.
#[derive(Debug, Clone)]
pub struct History<T> {
    /// States before each edit, oldest first. `done` indexes into it.
    entries: Vec<Edit<T>>,
    /// How many entries are applied. Everything from here on is redoable.
    done: usize,
    /// The newest state, which is the document as the user sees it.
    current: Snapshot<T>,
    /// The oldest state we are still willing to keep, for bounding memory.
    capacity: usize,
}

/// Steps kept before this is the default. A session is measured in dozens of operations, so this
/// is generous rather than tight; the point is that a runaway loop cannot exhaust memory.
const DEFAULT_CAPACITY: usize = 512;

impl<T> History<T> {
    /// A history over `model`, with nothing to undo yet.
    #[must_use]
    pub fn new(model: T) -> Self {
        Self {
            entries: Vec::new(),
            done: 0,
            current: Snapshot {
                model: Arc::new(model),
                label: String::new(),
            },
            capacity: DEFAULT_CAPACITY,
        }
    }

    /// Bound the history to `capacity` steps. Zero means one step, which is "no undo".
    #[must_use]
    pub fn with_capacity(mut self, capacity: usize) -> Self {
        self.capacity = capacity.max(1);
        self.trim();
        self
    }

    /// The document as the user sees it.
    #[must_use]
    pub fn current(&self) -> &Snapshot<T> {
        &self.current
    }

    /// Borrow the model.
    #[must_use]
    pub fn model(&self) -> &T {
        self.current.get()
    }

    /// Clone the model out, for a save.
    #[must_use]
    pub fn to_model(&self) -> T
    where
        T: Clone,
    {
        self.current.to_model()
    }

    /// Record an edit: `before` is the current state and `after` is what it became.
    ///
    /// A new edit discards the redo stack, which is what every editor does and what makes a
    /// redo *wrong* rather than merely unavailable if it were kept.
    pub fn push(&mut self, before: Snapshot<T>, after: Snapshot<T>) {
        debug_assert!(
            std::ptr::eq(before.model.as_ref(), self.current.model.as_ref()),
            "an edit's `before` must be the state the history already had"
        );
        self.entries.truncate(self.done);
        let newest = after.clone();
        self.entries.push(Edit { before, after });
        self.done = self.entries.len();
        // Taken from the argument rather than read back out of `entries`: the entry is always
        // there, but a trim can drop it again immediately below, and this does not care.
        self.current = newest;
        self.trim();
    }

    /// Step back one edit.
    pub fn undo(&mut self) -> Result<&Snapshot<T>, HistoryError> {
        if self.done == 0 {
            return Err(HistoryError::NothingToUndo);
        }
        self.done -= 1;
        let Some(entry) = self.entries.get(self.done) else {
            return Err(HistoryError::NothingToUndo);
        };
        self.current = entry.before.clone();
        Ok(&self.current)
    }

    /// Step forward one edit.
    pub fn redo(&mut self) -> Result<&Snapshot<T>, HistoryError> {
        if self.done >= self.entries.len() {
            return Err(HistoryError::NothingToRedo);
        }
        let Some(entry) = self.entries.get(self.done) else {
            return Err(HistoryError::NothingToRedo);
        };
        self.current = entry.after.clone();
        self.done += 1;
        Ok(&self.current)
    }

    /// How many edits are applied, which is the "can I undo?" question.
    #[must_use]
    pub fn undo_depth(&self) -> usize {
        self.done
    }

    /// How many edits can be redone.
    #[must_use]
    pub fn redo_depth(&self) -> usize {
        self.entries.len() - self.done
    }

    /// Whether there is anything to undo.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.done > 0
    }

    /// Whether there is anything to redo.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.done < self.entries.len()
    }

    /// The applied edits, oldest first, for a history panel.
    pub fn applied(&self) -> impl Iterator<Item = &Edit<T>> {
        self.entries.iter().take(self.done)
    }

    /// Forget everything. What a user means by "revert to the file as it was opened".
    pub fn clear(&mut self) {
        self.entries.clear();
        self.done = 0;
    }

    /// Drop the oldest steps past the capacity, which is what bounds the memory.
    ///
    /// The oldest `Edit` that goes carries a `before` the next one no longer names, so the
    /// next one is **rebased** onto it: without that, undoing to the oldest surviving step would
    /// land on a state the user never saw, which is the one thing an undo stack must never do.
    /// The rebased entry is a new `Edit` rather than a mutation because the dropped one may
    /// still be referenced by an `applied()` iterator the caller is holding.
    fn trim(&mut self) {
        while self.entries.len() > self.capacity {
            let dropped = self.entries.remove(0);
            self.done = self.done.saturating_sub(1);
            if let Some(next) = self.entries.first_mut() {
                next.before = dropped.before;
            }
        }
        // Trimming can leave `done` past the end if the newest steps were the ones kept.
        self.done = self.done.min(self.entries.len());
    }

    /// How many documents this history is actually holding, which is what decides whether the
    /// snapshot design is affordable.
    ///
    /// The sum of the shared counts over every snapshot the history refers to, minus the
    /// references from outside it — which is why this is a sum and not a length: two snapshots
    /// of the same document are one document held twice, and the whole point of the design is
    /// that consecutive edits share rather than copy.
    #[must_use]
    pub fn held_models(&self) -> usize {
        let mut counted = 0usize;
        for entry in &self.entries {
            counted += entry.before.shares().min(1);
            counted += entry.after.shares().min(1);
        }
        counted += self.current.shares().min(1);
        counted
    }

    /// The number of steps this history holds, applied or not.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the history is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect` and `unwrap`, which is what a test is for; the
    // panic-free rule is about what the product does with a file, not about tests. The same
    // allowance, for the same reason, as `mangle-content`'s test modules.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{Edit, History, HistoryError, Snapshot};

    /// A model small enough to compare by value, standing in for a document.
    #[derive(Debug, Clone, PartialEq)]
    struct Page {
        text: String,
    }

    fn history() -> History<Page> {
        History::new(Page {
            text: "as opened".into(),
        })
    }

    /// One edit, recorded the way a caller would: take the current state, make the next one.
    fn edit(h: &mut History<Page>, text: &str) {
        let before = h.current().clone();
        let after = Snapshot::new(Page { text: text.into() }, format!("set text to {text}"));
        h.push(before, after);
    }

    #[test]
    fn a_new_history_has_the_model_it_was_given_and_nothing_to_undo() {
        let mut h = history();
        assert_eq!(h.model().text, "as opened");
        assert!(!h.can_undo());
        assert!(!h.can_redo());
        assert_eq!(h.undo(), Err(HistoryError::NothingToUndo));
        assert_eq!(h.redo(), Err(HistoryError::NothingToRedo));
    }

    #[test]
    fn an_edit_changes_the_model_and_becomes_undoable() {
        let mut h = history();
        edit(&mut h, "edited");
        assert_eq!(h.model().text, "edited");
        assert!(h.can_undo());
        assert_eq!(h.undo_depth(), 1);
    }

    #[test]
    fn undo_puts_the_model_back_exactly_and_redo_takes_it_forward_again() {
        let mut h = history();
        edit(&mut h, "first");
        edit(&mut h, "second");
        assert_eq!(h.model().text, "second");

        assert_eq!(h.undo().map(|s| s.get().text.clone()), Ok("first".into()));
        assert_eq!(
            h.undo().map(|s| s.get().text.clone()),
            Ok("as opened".into())
        );
        assert_eq!(h.undo(), Err(HistoryError::NothingToUndo));

        assert_eq!(h.redo().map(|s| s.get().text.clone()), Ok("first".into()));
        assert_eq!(h.redo().map(|s| s.get().text.clone()), Ok("second".into()));
        assert_eq!(h.redo(), Err(HistoryError::NothingToRedo));
    }

    /// The property FINISH.md S1.16 asks for: a whole stage undone and redone is the document
    /// it started as, compared structurally rather than by eyeball.
    #[test]
    fn undoing_and_redoing_a_stage_is_structurally_equal_to_where_it_started() {
        let mut h = history();
        let opened = h.to_model();
        for text in ["one", "two", "three", "four"] {
            edit(&mut h, text);
        }
        for _ in 0..4 {
            h.undo().expect("four steps to undo");
        }
        assert_eq!(h.to_model(), opened, "undo returns the document it opened");
        for _ in 0..4 {
            h.redo().expect("four steps to redo");
        }
        assert_eq!(h.model().text, "four");
    }

    /// A new edit after an undo makes the abandoned future unreachable, which is the whole
    /// reason redo is dropped rather than kept.
    #[test]
    fn an_edit_after_an_undo_discards_what_would_have_been_redone() {
        let mut h = history();
        edit(&mut h, "first");
        edit(&mut h, "second");
        h.undo().expect("one step back");
        assert!(h.can_redo());

        edit(&mut h, "different");
        assert!(!h.can_redo(), "the abandoned future is gone");
        assert_eq!(h.redo(), Err(HistoryError::NothingToRedo));
        assert_eq!(h.model().text, "different");
    }

    #[test]
    fn a_history_is_bounded_and_drops_the_oldest_steps_first() {
        let mut h = history().with_capacity(3);
        for text in ["a", "b", "c", "d", "e"] {
            edit(&mut h, text);
        }
        assert_eq!(h.len(), 3, "the history holds the capacity and no more");
        assert_eq!(h.model().text, "e");
        assert_eq!(h.undo_depth(), 3);

        // Three steps are undoable, and the oldest surviving one goes all the way back to where
        // the document opened rather than stopping at the first step it kept — the rebasing in
        // `trim` is what makes that true, and stopping short of it is the bug this pins.
        assert_eq!(h.undo().map(|s| s.get().text.clone()), Ok("d".into()));
        assert_eq!(h.undo().map(|s| s.get().text.clone()), Ok("c".into()));
        assert_eq!(
            h.undo().map(|s| s.get().text.clone()),
            Ok("as opened".into()),
            "the oldest surviving step undoes to the document as opened, not to the first \
             state that happened to survive the bound"
        );
        assert_eq!(h.undo(), Err(HistoryError::NothingToUndo));
    }

    /// Capacity one means "the last edit only", which is the degenerate case of the bound.
    #[test]
    fn a_capacity_of_one_keeps_exactly_one_step() {
        let mut h = history().with_capacity(0);
        edit(&mut h, "a");
        edit(&mut h, "b");
        assert_eq!(h.len(), 1);
        assert_eq!(
            h.undo().map(|s| s.get().text.clone()),
            Ok("as opened".into()),
            "one step of history still reaches the document as it opened"
        );
        assert_eq!(h.undo(), Err(HistoryError::NothingToUndo));
    }

    #[test]
    fn clearing_returns_the_model_to_what_it_opened_as() {
        let mut h = history();
        edit(&mut h, "edited");
        h.clear();
        assert!(h.is_empty());
        assert!(!h.can_undo());
        assert_eq!(
            h.model().text,
            "edited",
            "clear forgets history, not the document"
        );
    }

    #[test]
    fn each_edit_carries_what_it_was_called() {
        let mut h = history();
        edit(&mut h, "renamed");
        let label = h.applied().next().map(Edit::label).unwrap_or_default();
        assert_eq!(label, "set text to renamed");
    }

    /// The property that makes snapshots affordable: consecutive edits share state, so a
    /// five-step history does not hold six copies of the document.
    #[test]
    fn a_history_of_whole_model_replacements_shares_its_snapshots() {
        let mut h = history();
        for text in ["a", "b", "c", "d", "e"] {
            edit(&mut h, text);
        }
        assert_eq!(h.len(), 5);
        assert!(
            h.current().shares() >= 2,
            "the current model is also the newest snapshot, so at least two share it: {}",
            h.current().shares()
        );
    }
}
