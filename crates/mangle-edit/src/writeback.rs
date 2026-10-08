//! Saving an edited page back into the file: decode, patch, re-encode, append.
//!
//! # What a save is here
//!
//! GOAL.md §4.3 states the shape of a save in one paragraph: decode the streams, patch them,
//! **re-encode with the original filter kind**, and leave every other byte of the file alone.
//! FINISH.md's S2 exit criterion is the same instruction from the other end — edit, save,
//! reopen, and an untouched JPEG stream is still bit-identical.
//!
//! So this is an **incremental update**: the file's existing bytes are kept verbatim, and the
//! content streams that changed are written again as new objects at the end with a new xref
//! section pointing at them. Nothing is renumbered, nothing is re-serialised, and no object that
//! did not change is rewritten — which is what makes the "untouched image" claim checkable
//! rather than merely asserted.
//!
//! # The four decisions that make that true
//!
//! - **Only the parts that changed are written.** A page whose `/Contents` is an array of five
//!   streams, one of which was edited, gets one new object and four untouched ones. Their bytes
//!   are not re-encoded, because re-encoding is re-compressing and a re-compressed image is a
//!   different image.
//! - **The filter is the file's own.** `encode_stream` re-encodes with the filter the stream
//!   declared, and the parameters it declared with it. A stream that declared no filter stays
//!   unfiltered.
//! - **A patch that straddles a boundary is refused.** The byte offsets a `PageObject` carries
//!   are offsets into the *concatenation* of the page's content streams, because that is what the
//!   interpreter ran. A patch that covers the end of one stream and the start of the next is
//!   really two patches about two different objects, and writing it as one would put one
//!   object's bytes inside another.
//! - **An encrypted file is refused.** Writing a new revision of an encrypted document means
//!   re-encrypting it, and this does not do that. The user is told rather than handed a file
//!   that opens with a repair prompt.

use std::ops::Range;

use mangle_crypto::sha256;
use mangle_doc::Page;
use mangle_syntax::document::Document;
use mangle_syntax::object::{Dict, Object, Ref, Stream};
use mangle_syntax::stream::{decode_stream, encode_stream};
use mangle_syntax::writer::IncrementalUpdate;

use crate::surgery::{Patch, apply};

/// What a save produced, and what it had to touch to produce it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Save {
    /// The whole file, as it should now be written.
    pub bytes: Vec<u8>,
    /// The object numbers this revision rewrites. Everything else is the same bytes.
    pub rewritten: Vec<u32>,
    /// Whether any stream had to be re-encoded, which is the case that costs size.
    pub re_encoded: bool,
}

impl Save {
    /// A short hash of the untouched part of the file, for a check that nothing else moved.
    ///
    /// The saved file is the original with bytes appended, so the prefix is byte-identical and
    /// this is a check on the whole file being an *addition* rather than a rewrite.
    #[must_use]
    pub fn appended_only(&self, original: &[u8]) -> bool {
        self.bytes.starts_with(original)
    }
}

/// Why a page could not be saved, named rather than approximated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveError {
    /// The document is encrypted, and a revision would have to be encrypted too.
    Encrypted,
    /// `/Contents` is a direct stream, so there is no object number to replace and the page
    /// dictionary itself would have to be rewritten.
    ContentsNotIndirect,
    /// One of the content streams is not an indirect reference.
    PartNotIndirect {
        /// Which part, from zero.
        part: usize,
        /// What it was instead.
        found: String,
    },
    /// A content stream would not decode, so its bytes cannot be patched.
    Undecodable {
        /// Which part, from zero.
        part: usize,
        /// Why not.
        note: String,
    },
    /// The filter chain is one this does not re-encode, named so a caller can say so.
    FilterChain {
        /// The filters, in the order the file applies them.
        filters: Vec<String>,
    },
    /// A patch covers the end of one content stream and the start of the next.
    PatchStraddles {
        /// The patch that straddles.
        span: Range<usize>,
        /// Where the two streams join in the concatenation.
        boundary: usize,
    },
    /// The patch could not be applied to the stream it was meant for.
    Stream(crate::surgery::EditError),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Encrypted => write!(
                f,
                "the document is encrypted, and a new revision would have to be encrypted too"
            ),
            Self::ContentsNotIndirect => write!(
                f,
                "this page's /Contents is a direct stream, so there is no object to replace and \
                 the page dictionary itself would have to be rewritten"
            ),
            Self::PartNotIndirect { part, found } => write!(
                f,
                "content stream {part} is {found} rather than a reference, so it has no object \
                 number to write back to"
            ),
            Self::Undecodable { part, note } => {
                write!(f, "content stream {part} did not decode: {note}")
            }
            Self::FilterChain { filters } => write!(
                f,
                "the stream is filtered with {}, and this re-encodes one filter at a time",
                filters.join(" then ")
            ),
            Self::PatchStraddles { span, boundary } => write!(
                f,
                "the patch {span:?} covers byte {boundary}, where two content streams join; it \
                 is two edits about two different objects"
            ),
            Self::Stream(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SaveError {}

impl From<crate::surgery::EditError> for SaveError {
    fn from(e: crate::surgery::EditError) -> Self {
        Self::Stream(e)
    }
}

/// One content stream of a page, resolved to its object and its bytes.
#[derive(Debug, Clone)]
struct Part {
    /// The object it lives in, which is what a save replaces.
    r: Ref,
    /// The stream as it stands, still filtered.
    stream: Stream,
    /// The decoded bytes.
    decoded: Vec<u8>,
    /// Where this part starts in the concatenation the interpreter ran.
    at: usize,
}

/// Apply `patches` to a page's content streams and save the result as an incremental update.
///
/// The patches are the ones an [`crate::edits`] call produced, so their ranges are offsets into
/// the *decoded and concatenated* stream — the same stream `PageModel` was built from.
pub fn save_page(doc: &Document, page: &Page, patches: &[Patch]) -> Result<Save, SaveError> {
    if doc.info().encryption.encrypted {
        return Err(SaveError::Encrypted);
    }
    let parts = parts_of(doc, page)?;
    let boundaries = part_boundaries(&parts);
    for span in patches.iter().flat_map(|p| [p.range.start, p.range.end]) {
        if boundaries.contains(&span) && span != 0 {
            return Err(SaveError::PatchStraddles {
                span: 0..span,
                boundary: span,
            });
        }
    }

    // The stream the interpreter ran: the parts, with a newline between them, exactly as
    // `Page::decoded_contents` builds it. Patching that rather than the parts individually is
    // what lets one patch range be valid, and the split below puts each byte back where it came
    // from.
    let joined = joined(&parts);
    let applied = apply(&joined, patches)?;

    // Each part's new length is its old one plus the net change of the patches inside it.
    let mut lengths: Vec<usize> = parts.iter().map(|p| p.decoded.len()).collect();
    for (i, part) in parts.iter().enumerate() {
        let from = part.at;
        let to = from + part.decoded.len();
        for patch in patches {
            // Outside this part altogether is fine; inside it is what changes the length;
            // covering either edge is the straddle this refuses.
            if patch.range.start >= to || patch.range.end <= from {
                continue;
            }
            if patch.range.start < from || patch.range.end > to {
                return Err(SaveError::PatchStraddles {
                    span: patch.range.clone(),
                    boundary: from,
                });
            }
            let before = patch.range.end - patch.range.start;
            if let Some(slot) = lengths.get_mut(i) {
                *slot = *slot + patch.bytes.len() - before;
            }
        }
    }

    // Walk the result, taking each part's new length and stepping over the separator that was
    // synthetic in the first place.
    let mut rewritten: Vec<(Ref, Stream)> = Vec::new();
    let mut cursor = 0usize;
    let mut re_encoded = false;
    for (i, part) in parts.iter().enumerate() {
        let take = lengths.get(i).copied().unwrap_or(0);
        let new = applied
            .bytes
            .get(cursor..cursor + take)
            .ok_or(SaveError::PatchStraddles {
                span: cursor..cursor + take,
                boundary: cursor,
            })?
            .to_vec();
        if new != part.decoded {
            let stream = re_encode(part, new)?;
            re_encoded = re_encoded || stream.raw != part.stream.raw;
            rewritten.push((part.r, stream));
        }
        cursor += take;
        // Step over the newline that was between this part and the next.
        if i + 1 < parts.len() && applied.bytes.get(cursor) == Some(&b'\n') {
            cursor += 1;
        }
    }

    let mut update = IncrementalUpdate::new(doc.xref().size());
    for (r, stream) in &rewritten {
        update.set(r.num, Object::Stream(stream.clone()));
    }
    if let Some(info) = doc.info_ref() {
        update.info = Some(info);
    }
    let bytes = update.append_with_id(
        doc.bytes(),
        doc.xref().start_xref().unwrap_or(0),
        doc.catalog_ref(),
        &doc.file_id(),
    );
    Ok(Save {
        bytes,
        rewritten: rewritten.iter().map(|(r, _)| r.num).collect(),
        re_encoded,
    })
}

/// The page's content streams as indirect objects.
fn parts_of(doc: &Document, page: &Page) -> Result<Vec<Part>, SaveError> {
    let Some(contents) = page.dict.get("Contents") else {
        return Ok(Vec::new());
    };
    let refs: Vec<Ref> = match contents {
        Object::Ref(r) => vec![*r],
        Object::Array(parts) => {
            let mut out = Vec::with_capacity(parts.len());
            for (i, part) in parts.iter().enumerate() {
                match part {
                    Object::Ref(r) => out.push(*r),
                    other => {
                        return Err(SaveError::PartNotIndirect {
                            part: i,
                            found: other.type_name().to_string(),
                        });
                    }
                }
            }
            out
        }
        other => {
            return Err(SaveError::PartNotIndirect {
                part: 0,
                found: other.type_name().to_string(),
            });
        }
    };

    let mut out: Vec<Part> = Vec::with_capacity(refs.len());
    let mut at = 0usize;
    for (i, r) in refs.iter().enumerate() {
        let Some(Object::Stream(stream)) = doc.object(*r) else {
            return Err(SaveError::PartNotIndirect {
                part: i,
                found: "not a stream".to_string(),
            });
        };
        let d = decode_stream(&stream);
        if !d.notes.is_empty() && d.data.is_empty() {
            return Err(SaveError::Undecodable {
                part: i,
                note: d.notes.join("; "),
            });
        }
        out.push(Part {
            r: *r,
            stream,
            decoded: d.data,
            at,
        });
        at += out.last().map(|p| p.decoded.len()).unwrap_or(0);
        // The newline the concatenation puts between two parts.
        at += usize::from(i + 1 < refs.len());
    }
    Ok(out)
}

/// The byte offsets in the concatenation where one content stream ends.
///
/// These are the only positions a valid patch may not cover, because the byte after one is the
/// newline that was never in the file and the byte before it belongs to another object.
fn part_boundaries(parts: &[Part]) -> Vec<usize> {
    let mut out: Vec<usize> = parts
        .iter()
        .take(parts.len().saturating_sub(1))
        .map(|p| p.at + p.decoded.len())
        .collect();
    out.sort_unstable();
    out
}

/// The concatenation the interpreter ran.
fn joined(parts: &[Part]) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            out.push(b'\n');
        }
        out.extend_from_slice(&part.decoded);
    }
    out
}

/// A part's stream, rewritten with `decoded` inside it and the file's own filter kept.
fn re_encode(part: &Part, decoded: Vec<u8>) -> Result<Stream, SaveError> {
    let filters = part.stream.filters();
    if filters.len() > 1 {
        return Err(SaveError::FilterChain {
            filters: filters
                .iter()
                .map(|f| String::from_utf8_lossy(f).into_owned())
                .collect(),
        });
    }
    let params = part.stream.decode_parms().into_iter().next().flatten();
    let (raw, written) = match filters.first() {
        Some(filter) => encode_stream(&decoded, filter, params),
        None => (decoded, None),
    };
    let mut dict = Dict::new();
    for (key, value) in part.stream.dict.iter() {
        // `/Length` is recomputed on write and `/Filter` and `/DecodeParms` are the ones this
        // may have changed, so everything else the producer wrote is carried across untouched.
        if matches!(
            key.as_bytes(),
            b"Length" | b"Filter" | b"DecodeParms" | b"DP"
        ) {
            continue;
        }
        dict.insert(key.clone(), value.clone());
    }
    if let Some(filter) = filters.first() {
        dict.set(
            "Filter",
            Object::Name(mangle_syntax::object::Name::from_bytes(filter)),
        );
    }
    if let Some(p) = written {
        dict.set("DecodeParms", p);
    }
    let mut stream = Stream::new(dict, raw);
    stream.file_offset = part.stream.file_offset;
    Ok(stream)
}

/// The SHA-256 of a stream's bytes, which is how an untouched stream is proved untouched.
///
/// A hash rather than a comparison because the object is not in the same place in the saved
/// file: the bytes are what must not move, not their offset.
#[must_use]
pub fn stream_digest(stream: &Stream) -> [u8; 32] {
    sha256(&stream.raw)
}
