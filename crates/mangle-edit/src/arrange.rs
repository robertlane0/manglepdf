//! Arrange: changing the order things are drawn in, which is the one edit a wrapper cannot do.
//!
//! # Why this is the exception
//!
//! Every other edit here is a *wrapper*: the object's own operators stay exactly where they are
//! and the edit is two insertions around them. That works because a `q … Q` changes the space an
//! object is drawn **in**. Arrange cannot use it, because what Arrange changes is the *order* the
//! operators appear in — and in a content stream the order of operators **is** the z-order. So an
//! arrange is a **move**: the object's bytes come out of where they were and go in somewhere else.
//!
//! # The two halves, and which of them is hard
//!
//! Moving bytes is easy. The hard half is that an object's appearance is not in its own operators:
//! it is in its operators **plus the state that was in force where they sat**. A `Do` that drew a
//! photo under `q 2 0 0 2 0 0 cm` draws a different photo at a different size the moment it moves
//! out from under that `cm`. So the moved bytes are wrapped in `q` … `Q` with **every piece of
//! state the record carries re-materialised** — the CTM, the colours in the space the file used,
//! the stroke width, the caps, the joins, the dash.
//!
//! And what the record does *not* carry is where this stops, honestly:
//!
//! - **A clip cannot be re-established.** A record carries the *region* its clip resolved to, not
//!   the path operators that built it, and a region is not a path. An object clipped at its old
//!   position is refused rather than moved unclipped.
//! - **Alpha and blend cannot be re-established.** They live in a named `/ExtGState` dictionary,
//!   and inventing a name would either collide with one the file uses or draw attention to a
//!   resource nobody defined. A translucent object is refused.
//! - **Text cannot be re-established.** A run's appearance depends on the text state in force —
//!   the font, the size, `Tc`, `Tw`, `Tz`, `TL`, `Ts` — and none of that is on the record. The
//!   line-spacing and character-spacing panels in §4.4 are what would put it there. A run of text
//!   is refused.
//! - **An object with several spans is refused.** Arranging a line whose runs have another
//!   object's operators between them would move both and re-interleave them with it. That is a
//!   different operation from the one asked for.
//!
//! Each refusal names what is missing, which is GOAL.md §4.1's fifth law: a user is told what
//! cannot be done faithfully rather than shown a page that changed in a way they did not choose.

use std::fmt;
use std::fmt::Write as _;
use std::ops::Range;

use mangle_content::matrix::Matrix;
use mangle_content::state::{ColourSpace, Rgba};
use mangle_content::{Mark, Record, tokens::ContentStream};

use crate::edits::{Channel, write_in};
use crate::page_objects::{PageModel, PageObject};
use crate::surgery::{Applied, EditError, Patch};

/// The four arrange commands of GOAL.md §4.7.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arrange {
    /// Past the next object this one overlaps, which is what a designer sees as "in front of it".
    Forward,
    /// Back past the previous object this one overlaps.
    Backward,
    /// To the front of the containing scope, so nothing on this page draws over it.
    ToFront,
    /// To the back of the containing scope, so it draws under everything.
    ToBack,
}

impl fmt::Display for Arrange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Forward => "bring forward",
            Self::Backward => "send backward",
            Self::ToFront => "bring to front",
            Self::ToBack => "send to back",
        };
        f.write_str(name)
    }
}

/// Why an object cannot be arranged, named rather than approximated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArrangeRefusal {
    /// There is nothing in the direction asked for to go past.
    NoNeighbour {
        /// What was asked for.
        arrange: Arrange,
        /// How many objects were considered.
        considered: usize,
    },
    /// The object was drawn under a clip, which cannot be re-established at the destination.
    NeedsClip,
    /// The object was drawn with an alpha or a blend mode, which lives in a named `/ExtGState`.
    NeedsNamedState {
        /// What the state was.
        was: String,
    },
    /// The object is text, and the text state in force is not carried on the record.
    NeedsTextState,
    /// The object's operators are not contiguous, so arranging it would re-interleave it.
    Scattered {
        /// How many separate ranges it covers.
        spans: usize,
    },
    /// The object claims bytes this stream does not have.
    ForeignSpan {
        /// What was asked for.
        span: Range<usize>,
        /// How long the stream is.
        len: usize,
    },
}

impl fmt::Display for ArrangeRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoNeighbour {
                arrange,
                considered,
            } => write!(
                f,
                "nothing overlaps this object in that direction, so `{arrange}` would move it \
                 past nothing; of {considered} object(s) on the page, none qualifies"
            ),
            Self::NeedsClip => write!(
                f,
                "this object was drawn inside a clip, and a clip is a path this record does not \
                 carry — moving it would drop the clip and draw it somewhere it was never meant \
                 to be"
            ),
            Self::NeedsNamedState { was } => write!(
                f,
                "this object was drawn with {was}, which lives in a named /ExtGState dictionary \
                 and cannot be re-established without inventing a name"
            ),
            Self::NeedsTextState => write!(
                f,
                "a run of text is drawn by the text state in force where it sat — font, size, \
                 character and word spacing, horizontal scale, leading, rise — and none of that \
                 is carried on the record"
            ),
            Self::Scattered { spans } => write!(
                f,
                "this object is drawn by {spans} separate operations with other objects' \
                 operators between them, so moving it would re-interleave it with theirs"
            ),
            Self::ForeignSpan { span, len } => write!(
                f,
                "this object covers bytes {span:?} of a stream that is {len} bytes long, so it \
                 came from somewhere else"
            ),
        }
    }
}

impl std::error::Error for ArrangeRefusal {}

/// Why an arrange did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArrangeError {
    /// The object cannot be moved faithfully.
    Refused(ArrangeRefusal),
    /// The object's own bytes could not be written.
    Stream(EditError),
}

impl fmt::Display for ArrangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(r) => write!(f, "{r}"),
            Self::Stream(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ArrangeError {}

impl From<ArrangeRefusal> for ArrangeError {
    fn from(r: ArrangeRefusal) -> Self {
        Self::Refused(r)
    }
}

impl From<EditError> for ArrangeError {
    fn from(e: EditError) -> Self {
        Self::Stream(e)
    }
}

/// The object's byte extent: from the start of its first operation to the end of its last.
#[must_use]
pub fn extent(object: &PageObject) -> Range<usize> {
    let start = object.spans.first().map(|s| s.start).unwrap_or(0);
    let end = object.spans.last().map(|s| s.end).unwrap_or(start);
    start..end
}

/// Whether two boxes touch at all.
fn overlaps(a: &mangle_content::state::ClipBounds, b: &mangle_content::state::ClipBounds) -> bool {
    a.x0 <= b.x1 && b.x0 <= a.x1 && a.y0 <= b.y1 && b.y0 <= a.y1
}

/// The patches that carry out `arrange` on the object at `index` of `model`.
///
/// Nothing is written here, so a caller can see what an arrange *would* touch before letting it,
/// and so a test can assert the byte ranges directly.
pub fn arrange_patches(
    stream: &[u8],
    model: &PageModel,
    index: usize,
    arrange: Arrange,
) -> Result<Vec<Patch>, ArrangeRefusal> {
    let objects = model.objects();
    let Some(object) = objects.get(index) else {
        return Err(ArrangeRefusal::NoNeighbour {
            arrange,
            considered: objects.len(),
        });
    };
    if object.spans.len() > 1 {
        return Err(ArrangeRefusal::Scattered {
            spans: object.spans.len(),
        });
    }
    let from = extent(object);
    if from.end > stream.len() || from.start > from.end {
        return Err(ArrangeRefusal::ForeignSpan {
            span: from,
            len: stream.len(),
        });
    }
    state_of(object)?;

    // The neighbour, which is the nearest object in the direction asked for **that this one
    // overlaps** — not merely the nearest one. §4.7 says so explicitly: a designer who says
    // "in front" means in front of the thing under it, and moving past an object it does not
    // touch changes nothing on screen.
    let mine = object.bounds;
    let neighbour = match arrange {
        Arrange::Forward | Arrange::ToFront => objects
            .iter()
            .filter(|o| o.spans.first().map(|s| s.start).unwrap_or(0) >= from.end)
            .filter(|o| overlaps(&o.bounds, &mine))
            .min_by_key(|o| o.spans.first().map(|s| s.start).unwrap_or(0)),
        Arrange::Backward | Arrange::ToBack => objects
            .iter()
            .filter(|o| o.spans.last().map(|s| s.end).unwrap_or(0) <= from.start)
            .filter(|o| overlaps(&o.bounds, &mine))
            .max_by_key(|o| o.spans.last().map(|s| s.end).unwrap_or(0)),
    };

    let scope = scope_of(stream, &from, arrange);
    // **Later operators draw on top.** A move to the front is therefore a move to *after* the
    // neighbour, and a move to the back is a move to *before* it. Getting this the wrong way round
    // is an arrange that appears to work and moves the object the opposite way from the one asked
    // for.
    let at = match arrange {
        Arrange::Forward => neighbour.map(|o| extent(o).end),
        Arrange::Backward => neighbour.map(|o| extent(o).start),
        Arrange::ToFront | Arrange::ToBack => Some(scope),
    };
    let Some(at) = at else {
        return Err(ArrangeRefusal::NoNeighbour {
            arrange,
            considered: objects.len(),
        });
    };
    if at >= from.start && at <= from.end {
        return Err(ArrangeRefusal::NoNeighbour {
            arrange,
            considered: objects.len(),
        });
    }

    // The moved bytes come out and go back in at the destination, wrapped in the state they were
    // drawn under. The deletion keeps the bytes either side from joining into one token, exactly
    // as a delete does.
    let moved = stream
        .get(from.clone())
        .ok_or(ArrangeRefusal::ForeignSpan {
            span: from.clone(),
            len: stream.len(),
        })?
        .to_vec();
    let body = state_of(object)?;
    Ok(vec![
        Patch::delete(from.clone(), "the object leaves its old place"),
        Patch::insert(
            at,
            pad_for_insertion(
                stream,
                at,
                &format!("q {body}\n{} Q\n", String::from_utf8_lossy(&moved)),
            ),
            "and takes its place at the front",
        ),
    ])
}

/// Apply `arrange` to the object at `index`, and give back the stream it should become.
pub fn apply_arrange(
    stream: &[u8],
    model: &PageModel,
    index: usize,
    arrange: Arrange,
) -> Result<Applied, ArrangeError> {
    let patches = arrange_patches(stream, model, index, arrange)?;
    let applied = crate::surgery::apply(stream, &patches)?;
    debug_assert!(
        crate::surgery::is_balanced(&applied.bytes),
        "an arrange unbalanced the stream, which GOAL 4.3 says must never reach a save"
    );
    Ok(applied)
}

/// The `q … Q` or `BT … ET` range an object is drawn inside, at the end the arrange aims for.
///
/// Outermost first is the order the wrappers come back in, so the *first* match is the outermost
/// one and the containing scope is the whole stream when there is no wrapper at all. The position
/// returned is just inside the closer for a move to the front, and just after the opener for a
/// move to the back.
fn scope_of(stream: &[u8], from: &Range<usize>, arrange: Arrange) -> usize {
    let ops = ContentStream::parse(stream).operations();
    let mut stack: Vec<(bool, usize, usize)> = Vec::new();
    let mut enclosing: Option<(usize, usize)> = None;
    for op in ops {
        let name = op.operator.operator().unwrap_or_default();
        match name {
            b"BT" | b"q" | b"BMC" | b"BDC" => {
                stack.push((name == b"BT", op.operator.span.start, op.operator.span.end));
            }
            b"ET" | b"Q" | b"EMC" => {
                if let Some((_, start, _)) = stack.pop() {
                    let end = op.operator.span.end;
                    if start <= from.start
                        && end >= from.end
                        && enclosing.is_none_or(|(s, _)| s < start)
                    {
                        enclosing = Some((start, end));
                    }
                }
            }
            _ => {}
        }
    }
    match (enclosing, arrange) {
        (Some((_, end)), Arrange::ToFront) => end,
        (Some((start, _)), _) => start,
        (None, Arrange::ToFront) => stream.len(),
        (None, _) => 0,
    }
}

/// The operators that re-establish, at the destination, the state an object was drawn under.
///
/// Everything here comes off the record, and anything not on the record is a refusal rather than a
/// default: a default is a state the file did not ask for, and the object would draw differently
/// with nothing anywhere saying so.
fn state_of(object: &PageObject) -> Result<String, ArrangeRefusal> {
    let Some(record) = object.records.first() else {
        // The model never builds an object with no records; this is the honest answer to a model
        // that one day might, rather than a panic in a library.
        return Err(ArrangeRefusal::Scattered { spans: 0 });
    };
    match &record.mark {
        // A run of text depends on the text state in force where it sat, and none of that is on
        // the record.
        Mark::Glyphs { .. } => Err(ArrangeRefusal::NeedsTextState),
        // A shading is painted into the clip in force, and a clip cannot be re-established from
        // the region a record carries.
        Mark::Shading { .. } => Err(ArrangeRefusal::NeedsClip),
        Mark::Image { .. } | Mark::Path { .. } => Ok(state_of_record(record)?),
        // A clip marker is not an object and the model never makes one; if it ever does, this is
        // the answer that tells the truth about what is missing.
        Mark::ClipChanged(_) => Err(ArrangeRefusal::NeedsClip),
    }
}

/// The state operators for one record, in the order a content stream would carry them.
fn state_of_record(record: &Record) -> Result<String, ArrangeRefusal> {
    if record.clip.is_some() {
        return Err(ArrangeRefusal::NeedsClip);
    }
    let mut named = Vec::new();
    if (record.fill_alpha - 1.0).abs() > 1e-9 {
        named.push(format!("a fill alpha of {}", record.fill_alpha));
    }
    if (record.stroke_alpha - 1.0).abs() > 1e-9 {
        named.push(format!("a stroke alpha of {}", record.stroke_alpha));
    }
    if record.blend_mode != "Normal" {
        named.push(format!("a `{}` blend mode", record.blend_mode));
    }
    if !named.is_empty() {
        return Err(ArrangeRefusal::NeedsNamedState {
            was: named.join(", "),
        });
    }

    let mut out = String::new();
    // The CTM first, because everything after it is in the space it establishes.
    push_ctm(&mut out, record.ctm);
    // The colours, in the space the file used. A colour this cannot write into is a refusal, which
    // is `edits`'s own rule and is applied here rather than repeated.
    for colour in [colour_of(record, true), colour_of(record, false)] {
        let Some((space, components)) = colour else {
            continue;
        };
        match colour_operators(&space, components) {
            Some(ops) => out.push_str(&ops),
            None => {
                return Err(ArrangeRefusal::NeedsNamedState {
                    was: format!("a fill in {space:?}"),
                });
            }
        }
    }
    // The stroke style, which a path's own operators do not set.
    if matches!(record.mark, Mark::Path { .. }) {
        push_stroke_style(&mut out, record);
    }
    Ok(out)
}

/// A record's fill or stroking colour, whichever it has.
fn colour_of(record: &Record, want_fill: bool) -> Option<(ColourSpace, Vec<f64>)> {
    match &record.mark {
        Mark::Path {
            fill: f, stroke: s, ..
        } => {
            let c = if want_fill { f.as_ref() } else { s.as_ref() }?;
            Some((c.space.clone(), c.components.clone()))
        }
        // Glyphs and images paint in the colour in force, which is the non-stroking one.
        Mark::Glyphs { fill, .. } | Mark::Image { fill, .. } if want_fill => {
            Some((fill.space.clone(), fill.components.clone()))
        }
        _ => None,
    }
}

/// The operators that set a colour, in the space it is in. `None` when the space is not writable.
fn colour_operators(space: &ColourSpace, components: Vec<f64>) -> Option<String> {
    // The existing conversion is written for a target *sRGB* colour, so an existing colour is
    // turned back into one and through again. That is exact for the three device spaces, which is
    // what every file a designer arranges uses.
    let rgb = to_rgb(space, &components)?;
    let written = write_in(space, rgb, Channel::Fill).ok()?;
    Some(format!("{written}\n"))
}

/// The bytes to insert at `at`, padded so the stream still tokenises as it did.
fn pad_for_insertion(stream: &[u8], at: usize, text: &str) -> String {
    let before = at.checked_sub(1).and_then(|i| stream.get(i)).copied();
    let after = stream.get(at).copied();
    let regular = |b: Option<u8>| b.is_some_and(mangle_syntax::lexer::Token::is_regular);
    let first = text.as_bytes().first().copied();
    let last = text.as_bytes().last().copied();
    let mut out = String::with_capacity(text.len() + 2);
    if regular(before) && regular(first) {
        out.push(' ');
    }
    out.push_str(text);
    if regular(last) && regular(after) {
        out.push(' ');
    }
    out
}

/// The `cm` for the state an object was drawn under.
fn push_ctm(out: &mut String, ctm: Matrix) {
    if ctm.is_identity() {
        return;
    }
    let _ = writeln!(
        out,
        "{} {} {} {} {} {} cm",
        ctm.a, ctm.b, ctm.c, ctm.d, ctm.e, ctm.f
    );
}

/// The stroke style operators, from the values the record carries.
fn push_stroke_style(out: &mut String, record: &Record) {
    // The record's width is the *device* width, already scaled by the CTM, so writing it back as
    // `w` would scale it a second time. Dividing it back out is the only way to get the number
    // the file had, and a CTM with no mean scale leaves the width as it is.
    let scale = record.ctm.mean_scale();
    let width = if scale > 0.0 {
        record.device_line_width / scale
    } else {
        record.device_line_width
    };
    let _ = writeln!(out, "{width} w");
    let _ = writeln!(out, "{} J", cap_of(record.line_cap));
    let _ = writeln!(out, "{} j", join_of(record.line_join));
    let dash = &record.dash;
    if dash.array.is_empty() {
        out.push_str("[] 0 d\n");
    } else {
        let parts = dash
            .array
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        let _ = writeln!(out, "[{parts}] {} d", dash.phase);
    }
}

/// The number the file wrote for a line cap. `from_int` is the reader; this is the writer.
fn cap_of(cap: mangle_content::state::LineCap) -> u8 {
    match cap {
        mangle_content::state::LineCap::Butt => 0,
        mangle_content::state::LineCap::Round => 1,
        mangle_content::state::LineCap::Projecting | mangle_content::state::LineCap::Square => 2,
    }
}

/// The number the file wrote for a line join.
fn join_of(join: mangle_content::state::LineJoin) -> u8 {
    match join {
        mangle_content::state::LineJoin::Miter => 0,
        mangle_content::state::LineJoin::Round => 1,
        mangle_content::state::LineJoin::Bevel => 2,
    }
}

/// A colour as sRGB, for the round trip through the existing conversion.
fn to_rgb(space: &ColourSpace, components: &[f64]) -> Option<Rgba> {
    let colour = mangle_content::state::Colour {
        space: space.clone(),
        components: components.to_vec(),
        under: None,
    };
    let rgba = colour.to_rgba(None)?;
    Some(Rgba {
        r: rgba.r,
        g: rgba.g,
        b: rgba.b,
        a: rgba.a,
    })
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect` and `unwrap`, which is what a test is for;
    // the panic-free rule is about what the product does with a file, not about tests.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::float_cmp,
        clippy::indexing_slicing,
        // A single-`Tj` run genuinely has one text span, and the list is the field's subject.
        clippy::single_range_in_vec_init
    )]

    use mangle_content::interp::Record;
    use mangle_content::interp::run;
    use mangle_content::matrix::Matrix;
    use mangle_content::state::{ClipBounds, Colour, ColourSpace, Dash, LineCap, LineJoin};

    use mangle_content::tokens::ContentStream;

    use super::{Arrange, ArrangeRefusal, arrange_patches, extent, state_of};
    use crate::page_objects::{Kind, PageModel, PageObject};

    fn record(mark: mangle_content::Mark, span: std::ops::Range<usize>) -> Record {
        Record {
            span,
            mark,
            ctm: Matrix::IDENTITY,
            clip: None,
            fill_alpha: 1.0,
            stroke_alpha: 1.0,
            blend_mode: "Normal".into(),
            device_line_width: 1.0,
            line_cap: LineCap::Butt,
            line_join: LineJoin::Miter,
            dash: Dash::default(),
            tag: None,
            form: None,
        }
    }

    fn box_of(x0: f64, y0: f64, x1: f64, y1: f64) -> ClipBounds {
        ClipBounds { x0, y0, x1, y1 }
    }

    fn object(kind: Kind, records: Vec<Record>, bounds: ClipBounds) -> PageObject {
        let spans = records.iter().map(|r| r.span.clone()).collect::<Vec<_>>();
        PageObject {
            kind,
            bounds,
            records,
            spans,
            form: None,
            parent: None,
            line_break: None,
        }
    }

    fn image(at: usize, len: usize) -> Record {
        record(
            mangle_content::Mark::Image {
                name: Some("Im0".into()),
                matrix: Matrix::IDENTITY,
                inline: false,
                fill: Colour::black(),
            },
            at..at + len,
        )
    }

    fn path(at: usize, len: usize) -> Record {
        let mut fill = Colour::black();
        fill.space = ColourSpace::device_rgb();
        fill.components = vec![0.5, 0.5, 0.5];
        record(
            mangle_content::Mark::Path {
                segments: Vec::new(),
                fill: Some(fill),
                stroke: None,
                rule: mangle_content::interp::FillRule::NonZero,
            },
            at..at + len,
        )
    }

    /// An image under a scaled CTM, which is the case the state has to be re-materialised for.
    #[test]
    fn the_state_of_an_object_drawn_under_a_matrix_is_that_matrix() {
        let mut r = image(0, 4);
        r.ctm = Matrix::new(2.0, 0.0, 0.0, 3.0, 10.0, 20.0);
        let o = object(Kind::Image, vec![r], box_of(0.0, 0.0, 10.0, 10.0));
        let state = state_of(&o).expect("an image can be moved");
        assert!(
            state.contains("2 0 0 3 10 20 cm"),
            "the CTM it was drawn under is written back: {state}"
        );
        assert!(!state.contains("cm\n\n"), "and only once: {state}");
    }

    #[test]
    fn an_object_drawn_under_a_clip_is_refused_rather_than_moved_unclipped() {
        let mut r = image(0, 4);
        r.clip = Some(mangle_content::state::Clip {
            paths: std::sync::Arc::new(Vec::new()),
            bounds: box_of(0.0, 0.0, 10.0, 10.0),
        });
        let o = object(Kind::Image, vec![r], box_of(0.0, 0.0, 10.0, 10.0));
        let err = state_of(&o).expect_err("a clip cannot be re-established");
        assert!(matches!(err, ArrangeRefusal::NeedsClip), "{err}");
        assert!(err.to_string().contains("clip"), "{err}");
    }

    #[test]
    fn a_translucent_object_is_refused_because_alpha_lives_in_a_named_dictionary() {
        let mut r = image(0, 4);
        r.fill_alpha = 0.5;
        let o = object(Kind::Image, vec![r], box_of(0.0, 0.0, 10.0, 10.0));
        let err = state_of(&o).expect_err("an alpha is an /ExtGState entry");
        assert!(
            matches!(err, ArrangeRefusal::NeedsNamedState { .. }),
            "{err}"
        );
        assert!(err.to_string().contains("ExtGState"), "{err}");
    }

    /// A shading is refused for the right reason: it paints into the clip in force, so a clip is
    /// what would have to be re-established — not a text state.
    #[test]
    fn a_shading_is_refused_because_a_clip_cannot_be_re_established() {
        let r = record(
            mangle_content::Mark::Shading {
                name: "Sh0".into(),
                matrix: Matrix::IDENTITY,
            },
            0..4,
        );
        let o = object(Kind::Shading, vec![r], box_of(0.0, 0.0, 10.0, 10.0));
        let err = state_of(&o).expect_err("a shading paints into a clip");
        assert!(matches!(err, ArrangeRefusal::NeedsClip), "{err}");
        assert!(err.to_string().contains("clip"), "{err}");
    }

    #[test]
    fn text_is_refused_because_the_text_state_is_not_on_the_record() {
        let r = record(
            mangle_content::Mark::Glyphs {
                font: Some("F1".into()),
                size: 12.0,
                text: vec![b'a'],
                codes: vec![97],
                two_byte: false,
                fill: Colour::black(),
                text_spans: vec![0..3],
                placements: vec![Matrix::IDENTITY],
            },
            0..4,
        );
        let o = object(Kind::Line, vec![r], box_of(0.0, 0.0, 10.0, 10.0));
        let err = state_of(&o).expect_err("no text state is carried");
        assert!(matches!(err, ArrangeRefusal::NeedsTextState), "{err}");
    }

    /// A line whose runs have another operator between them is one object over two spans.
    ///
    /// The model has to be built from a real stream — `PageModel` has no public constructor,
    /// because a model that could be assembled by hand is a model that could disagree with the
    /// interpreter — so the fixture is two runs with a colour change between them, which is the
    /// shape a producer actually writes.
    #[test]
    fn a_scattered_object_is_refused_rather_than_re_interleaved() {
        let stream = b"BT (A) Tj 1 0 0 rg (B) Tj ET";
        let model = PageModel::build(&run(&ContentStream::parse(stream)).records);
        let line = model
            .objects()
            .iter()
            .position(|o| o.spans.len() > 1)
            .expect("the two runs are one line");
        let err = arrange_patches(stream, &model, line, Arrange::Forward)
            .expect_err("two spans with an operator between them");
        assert!(matches!(err, ArrangeRefusal::Scattered { .. }), "{err}");
        assert!(err.to_string().contains("interleave"), "{err}");
    }

    /// The shape of an arrange: the object's bytes move, and they move to the other side of the
    /// neighbour. Two images a screen apart do not overlap and arrange past nothing, so the
    /// fixture draws them both at the origin.
    #[test]
    fn an_arrange_moves_the_operators_to_the_other_side_of_the_neighbour() {
        let stream = b"q 1 0 0 1 0 0 cm /Im0 Do Q q 1 0 0 1 0 0 cm /Im1 Do Q";
        let model = PageModel::build(&run(&ContentStream::parse(stream)).records);
        assert_eq!(model.objects().len(), 2, "two images");
        let patches =
            arrange_patches(stream, &model, 0, Arrange::Forward).expect("there is a neighbour");
        let applied = crate::surgery::apply(stream, &patches).expect("valid");
        let text = String::from_utf8_lossy(&applied.bytes);
        assert!(
            text.contains("/Im1 Do"),
            "the neighbour is still there: {text}"
        );
        assert!(
            text.find("/Im0 Do").expect("the image moved")
                > text.find("/Im1 Do").expect("its neighbour"),
            "`forward` means it now draws after the object it goes in front of: {text}"
        );
        assert_eq!(
            text.matches("cm").count(),
            2,
            "the file's own two `cm`s, one of which is the moved image's: {text}"
        );
        assert!(
            crate::surgery::is_balanced(&applied.bytes),
            "and it stays balanced"
        );

        let patches =
            arrange_patches(stream, &model, 1, Arrange::Backward).expect("there is a neighbour");
        let applied = crate::surgery::apply(stream, &patches).expect("valid");
        let text = String::from_utf8_lossy(&applied.bytes);
        assert!(
            text.find("/Im1 Do").expect("it moved") < text.find("/Im0 Do").expect("its neighbour"),
            "`backward` means it draws before the object it goes behind: {text}"
        );
    }

    /// An object under no matrix needs no `cm` — but it does still need its colour, because the
    /// colour in force at the destination is not the one it was drawn in.
    #[test]
    fn the_state_of_an_object_under_no_matrix_is_its_colour_alone() {
        let o = object(Kind::Image, vec![image(0, 4)], box_of(0.0, 0.0, 10.0, 10.0));
        let state = state_of(&o).expect("writable");
        assert!(!state.contains("cm"), "no matrix, no `cm`: {state}");
        assert!(
            state.contains("0 g\n"),
            "and the black it was drawn in: {state}"
        );
    }

    /// The matrix an object was drawn under travels with it.
    #[test]
    fn an_arranged_object_keeps_the_matrix_it_was_drawn_under() {
        // Both at the same place, so they overlap and there is a neighbour; the matrices
        // differ, so one of them has to travel.
        let stream = b"q 2 0 0 3 10 20 cm /Im0 Do Q q 1 0 0 1 10 20 cm /Im1 Do Q";
        let model = PageModel::build(&run(&ContentStream::parse(stream)).records);
        let patches =
            arrange_patches(stream, &model, 0, Arrange::Forward).expect("there is a neighbour");
        let applied = crate::surgery::apply(stream, &patches).expect("valid");
        let text = String::from_utf8_lossy(&applied.bytes);
        assert!(
            text.contains("2 0 0 3 10 20 cm"),
            "the matrix goes with it: {text}"
        );
        assert_eq!(
            text.matches("cm").count(),
            3,
            "the file's two and the one re-materialised at the destination: {text}"
        );
    }

    #[test]
    fn extent_covers_every_span_the_object_claims() {
        let o = object(
            Kind::Line,
            vec![image(10, 4), image(30, 4)],
            box_of(0.0, 0.0, 10.0, 10.0),
        );
        assert_eq!(extent(&o), 10..34);
    }

    /// A path's stroke style is re-materialised too, and the width is the file's own number
    /// rather than the device one — dividing it back out is the whole of it.
    #[test]
    fn a_paths_stroke_style_is_written_back_unscaled() {
        let mut r = path(0, 4);
        r.ctm = Matrix::scale(2.0, 2.0);
        r.device_line_width = 4.0;
        r.line_cap = LineCap::Round;
        r.line_join = LineJoin::Bevel;
        r.dash = Dash {
            array: vec![3.0, 2.0],
            phase: 1.5,
        };
        let o = object(Kind::Path, vec![r], box_of(0.0, 0.0, 10.0, 10.0));
        let state = state_of(&o).expect("writable");
        assert!(state.contains("2 w\n"), "the file's own width: {state}");
        assert!(state.contains("1 J\n"), "a round cap: {state}");
        assert!(state.contains("2 j\n"), "a bevel join: {state}");
        assert!(state.contains("[3 2] 1.5 d\n"), "the dash: {state}");
    }
}
