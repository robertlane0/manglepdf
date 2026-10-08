//! Surgical content-stream editing: change some bytes, leave every other byte alone.
//!
//! # Why this is the delicate part of editing a PDF
//!
//! A content stream is not a data structure to be re-serialised. It is **the file's own text**,
//! and everything a PDF author put in it that this project does not understand — comments,
//! whitespace, an unusual number format, an operator from a later revision — is the only record
//! of their intent. Re-serialising from the token list throws all of it away, and a file that
//! round-trips through it is a file that has been quietly rewritten.
//!
//! So an edit here is a **byte-range replacement**: the original bytes come out verbatim and only
//! the ranges named by the caller are substituted. GOAL.md §4.3 asks for exactly this — unchanged
//! operators preserved byte-for-byte, modified in place where possible, and a regenerated object
//! emitted wrapped in `q … Q` rather than replacing a whole `/Contents`.
//!
//! # What this does not do
//!
//! It does not produce a *sane* edit. A caller that moves a picture by rewriting `cm` must also
//! re-materialise the graphics state around the new `cm`, and it is `PageModel`'s spans plus the
//! caller's knowledge of what the operations do that make that possible. This module's job is
//! narrower and is stated in one line: **give back the stream with some ranges replaced and every
//! other byte identical**, and refuse anything that would make that untrue.

use std::ops::Range;

/// One replacement: which bytes, and what they become.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patch {
    /// The range in the original stream to replace. Empty means an insertion at that point.
    pub range: Range<usize>,
    /// What to put there instead. Empty means a deletion.
    pub bytes: Vec<u8>,
    /// What this edit is for, so a caller can report it and a history can name it.
    pub label: String,
}

impl Patch {
    /// Replace `range` with `bytes`.
    #[must_use]
    pub fn replace(
        range: Range<usize>,
        bytes: impl Into<Vec<u8>>,
        label: impl Into<String>,
    ) -> Self {
        Self {
            range,
            bytes: bytes.into(),
            label: label.into(),
        }
    }

    /// Insert `bytes` at `at`, changing nothing that was there.
    #[must_use]
    pub fn insert(at: usize, bytes: impl Into<Vec<u8>>, label: impl Into<String>) -> Self {
        Self {
            range: at..at,
            bytes: bytes.into(),
            label: label.into(),
        }
    }

    /// Remove `range` entirely.
    #[must_use]
    pub fn delete(range: Range<usize>, label: impl Into<String>) -> Self {
        Self {
            range,
            bytes: Vec::new(),
            label: label.into(),
        }
    }
}

/// Why a set of patches could not be applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// A patch names a range outside the stream.
    OutOfBounds {
        /// What was asked for.
        range: Range<usize>,
        /// How long the stream is.
        len: usize,
    },
    /// Two patches touch the same bytes, so the result would depend on the order they are
    /// applied in. Which is exactly the kind of thing that makes an edit unreproducible.
    Overlapping {
        /// The first of the two, for the message.
        first: Range<usize>,
        /// The second.
        second: Range<usize>,
    },
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutOfBounds { range, len } => {
                write!(f, "the range {range:?} is outside a stream of {len} bytes")
            }
            Self::Overlapping { first, second } => {
                write!(f, "the ranges {first:?} and {second:?} overlap")
            }
        }
    }
}

impl std::error::Error for EditError {}

/// The result of applying patches to a content stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    /// The stream as it should now be written.
    pub bytes: Vec<u8>,
    /// What each patch did, in the order applied, for a history entry.
    pub applied: Vec<Patch>,
}

impl Applied {
    /// Whether anything changed.
    #[must_use]
    pub fn is_unchanged(&self) -> bool {
        self.applied.is_empty()
    }
}

/// Append `stream[start..end]` to `out`, clamping the range into bounds.
///
/// Both ends are clamped rather than trusted, so no caller can produce a panic from this. An
/// empty range appends nothing, which is what an insertion at the end of the stream needs.
fn copy_range(stream: &[u8], start: usize, end: usize, out: &mut Vec<u8>) {
    let lo = start.min(stream.len());
    let hi = end.min(stream.len()).max(lo);
    if let Some(slice) = stream.get(lo..hi) {
        out.extend_from_slice(slice);
    }
}

/// Apply `patches` to `stream`, preserving every byte they do not name.
///
/// Patches are applied in ascending order of position, which is also the order they are reported
/// in. That is not an accident: applying them in the order the caller wrote them would make the
/// result depend on that order, and two runs of the same edit would then not agree.
pub fn apply(stream: &[u8], patches: &[Patch]) -> Result<Applied, EditError> {
    let mut ordered: Vec<&Patch> = patches.iter().collect();
    ordered.sort_by_key(|p| (p.range.start, p.range.end));

    // Bounds and overlap are checked before any copying, so a rejected edit costs nothing and
    // cannot have half-applied. `previous` is the last range that occupied bytes, kept whole so
    // the error can name both sides rather than reconstructing one from an endpoint.
    let mut previous: Option<Range<usize>> = None;
    for patch in &ordered {
        if patch.range.end > stream.len() || patch.range.start > patch.range.end {
            return Err(EditError::OutOfBounds {
                range: patch.range.clone(),
                len: stream.len(),
            });
        }
        if let Some(prev) = &previous {
            // Touching is allowed — an insertion at the end of a replacement is the ordinary way
            // to write two things in a row — but two ranges that *cover* the same byte are not.
            let touches = patch.range.start < prev.end
                && !(patch.range.is_empty() && patch.range.start == prev.end);
            if touches && !patch.range.is_empty() && !prev.is_empty() {
                return Err(EditError::Overlapping {
                    first: prev.clone(),
                    second: patch.range.clone(),
                });
            }
        }
        if !patch.range.is_empty() {
            previous = Some(patch.range.clone());
        }
    }

    // The bounds of every range were checked above and `ordered` is ascending, so `cursor` never
    // passes `patch.range.start` and never passes the end of the stream. Copying is written
    // through a helper that takes the range by value and bounds both ends itself, so the
    // invariant is enforced where it is used rather than assumed by the caller.
    let mut out = Vec::with_capacity(stream.len());
    let mut cursor = 0usize;
    for patch in &ordered {
        copy_range(
            stream,
            cursor.min(patch.range.start),
            patch.range.start,
            &mut out,
        );
        out.extend_from_slice(&patch.bytes);
        // An insertion does not consume anything, so the cursor stays where it was; a
        // replacement advances past the bytes it replaced.
        cursor = cursor.max(patch.range.end);
    }
    copy_range(stream, cursor, stream.len(), &mut out);

    Ok(Applied {
        bytes: out,
        applied: ordered.into_iter().cloned().collect(),
    })
}

/// Whether `q` and `Q` are balanced across the whole stream.
///
/// A writer that unbalanced them would leave every operator after the edit under the wrong
/// graphics state, and the page would render differently with no note anywhere — which is why
/// GOAL.md §4.3 asks for the balance to be verified after every edit rather than trusted.
#[must_use]
pub fn is_balanced(stream: &[u8]) -> bool {
    let mut depth: i64 = 0;
    for token in mangle_content::tokens::ContentStream::parse(stream).tokens() {
        match token.operator() {
            Some(b"q") => depth += 1,
            Some(b"Q") => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect` and `unwrap`, which is what a test is for;
    // the panic-free rule is about what the product does with a file, not about tests.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{EditError, Patch, apply, is_balanced};

    #[test]
    fn no_patches_leaves_the_stream_byte_for_byte() {
        let stream = b"1 0 0 1 10 20 cm % a comment\n/Span1 Do\n";
        let out = apply(stream, &[]).expect("no patches is always valid");
        assert_eq!(out.bytes, stream);
        assert!(out.is_unchanged());
    }

    /// The property the whole module exists for.
    #[test]
    fn bytes_outside_the_patched_ranges_are_identical() {
        let stream = b"% comment\nq 1 0 0 1 10 20 cm /Im0 Do Q\n% tail\n";
        // The offset is found rather than written down: a hand-counted one is wrong the moment
        // the fixture's whitespace changes, and a test that fails for that reason teaches
        // nothing about the code.
        let at = stream
            .windows(5)
            .position(|w| w == b"10 20")
            .expect("the operands are in the fixture");
        let out = apply(stream, &[Patch::replace(at..at + 5, b"30 40", "move")]).expect("valid");
        let text = String::from_utf8_lossy(&out.bytes).into_owned();
        assert!(
            text.starts_with("% comment\nq 1 0 0 1 "),
            "the head survived"
        );
        assert!(text.contains("30 40 cm"), "the patch landed: {text}");
        assert!(text.ends_with("Q\n% tail\n"), "the tail survived: {text}");
    }

    #[test]
    fn comments_and_whitespace_outside_a_patch_are_untouched() {
        let stream = b"%  a comment with spaces   \nq   1   0   0   1   5   5 cm Q\n";
        let at = stream
            .windows(5)
            .position(|w| w == b"5   5")
            .expect("the operands are in the fixture");
        let out = apply(stream, &[Patch::replace(at..at + 5, b"9   9", "move")]).expect("valid");
        let text = String::from_utf8_lossy(&out.bytes).into_owned();
        assert!(
            text.starts_with("%  a comment with spaces   \n"),
            "the comment and its spacing are the author's, not ours: {text:?}"
        );
        assert!(text.contains("q   1   0   0   1   9   9 cm"), "{text}");
    }

    #[test]
    fn a_patch_outside_the_stream_is_refused_rather_than_truncated() {
        let stream = b"q Q";
        let err =
            apply(stream, &[Patch::replace(0..99, b"x", "too far")]).expect_err("out of range");
        assert_eq!(
            err,
            EditError::OutOfBounds {
                range: 0..99,
                len: stream.len()
            },
            "the length reported is the real one, not the one in the fixture comment"
        );
    }

    /// Two edits to the same bytes would make the result depend on the order applied, which is
    /// the failure this refuses rather than resolves.
    #[test]
    fn two_patches_covering_the_same_bytes_are_refused() {
        let stream = b"1 0 0 1 10 20 cm";
        let err = apply(
            stream,
            &[
                Patch::replace(9..11, b"a", "first"),
                Patch::replace(10..13, b"b", "second"),
            ],
        )
        .expect_err("overlapping");
        assert!(matches!(err, EditError::Overlapping { .. }), "{err:?}");
    }

    /// Two insertions at the same point are not overlapping — they are two things written in a
    /// row, which is how an edit adds an operator.
    #[test]
    fn two_insertions_at_the_same_point_are_applied_in_order() {
        let stream = b"q Q";
        let out = apply(
            stream,
            &[
                Patch::insert(2, b"1 0 0 1 0 0 cm ", "first"),
                Patch::insert(2, b"2 0 0 1 0 0 cm ", "second"),
            ],
        )
        .expect("valid");
        // Whichever order they went in, both are present and nothing was lost.
        let text = String::from_utf8_lossy(&out.bytes).into_owned();
        assert!(text.contains("1 0 0 1 0 0 cm"), "{text}");
        assert!(text.contains("2 0 0 1 0 0 cm"), "{text}");
        assert!(text.starts_with("q "), "{text}");
        assert!(text.ends_with(" Q"), "{text}");
    }

    #[test]
    fn a_deletion_removes_exactly_its_range() {
        let stream = b"q 1 0 0 1 5 5 cm /Im0 Do Q";
        let at = stream
            .windows(7)
            .position(|w| w == b"/Im0 Do")
            .expect("the operation is in the fixture");
        let out = apply(stream, &[Patch::delete(at..at + 7, "remove the image")]).expect("valid");
        assert_eq!(String::from_utf8_lossy(&out.bytes), "q 1 0 0 1 5 5 cm  Q");
    }

    #[test]
    fn patches_are_applied_by_position_not_by_the_order_they_were_written() {
        let stream = b"AAAA BBBB CCCC";
        let first_at = stream
            .windows(4)
            .position(|w| w == b"AAAA")
            .expect("the first token is in the fixture");
        let third_at = stream
            .windows(4)
            .position(|w| w == b"CCCC")
            .expect("the third token is in the fixture");
        let forward = apply(
            stream,
            &[
                Patch::replace(first_at..first_at + 4, b"1111", "first"),
                Patch::replace(third_at..third_at + 4, b"3333", "third"),
            ],
        )
        .expect("valid");
        let backward = apply(
            stream,
            &[
                Patch::replace(third_at..third_at + 4, b"3333", "third"),
                Patch::replace(first_at..first_at + 4, b"1111", "first"),
            ],
        )
        .expect("valid");
        assert_eq!(
            forward.bytes, backward.bytes,
            "the same edit written in either order gives the same file"
        );
        assert_eq!(String::from_utf8_lossy(&forward.bytes), "1111 BBBB 3333");
    }

    #[test]
    fn an_empty_stream_takes_an_empty_edit_and_stays_empty() {
        let out = apply(b"", &[]).expect("valid");
        assert!(out.bytes.is_empty());
    }

    #[test]
    fn balance_is_measured_over_the_whole_stream() {
        assert!(is_balanced(b"q q Q Q"));
        assert!(is_balanced(b"1 0 0 1 5 5 cm"));
        assert!(
            !is_balanced(b"q q Q"),
            "one `q` was opened and never closed"
        );
        assert!(!is_balanced(b"Q"), "a `Q` with nothing open");
        assert!(!is_balanced(b"q Q q"), "the trailing `q` was never closed");
    }

    /// The case the balance check exists for: an edit that opens or closes a wrapper wrongly.
    #[test]
    fn an_edit_that_unbalances_the_stream_is_visible_as_such() {
        let stream = b"q 1 0 0 1 5 5 cm Q";
        assert!(is_balanced(stream));
        let out = apply(stream, &[Patch::insert(0, b"q ", "wrap")]).expect("valid");
        assert!(
            !is_balanced(&out.bytes),
            "a caller that wraps without closing must be able to see it: {:?}",
            String::from_utf8_lossy(&out.bytes)
        );
    }
}
