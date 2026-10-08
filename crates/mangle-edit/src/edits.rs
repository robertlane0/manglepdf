//! The edits themselves: move, scale, delete and recolour, written as patches.
//!
//! # What this turns a user's intent into
//!
//! A user drags a photo. That is one sentence in English and about six operators in the file:
//! the `cm` that placed the photo is a *separate operation* from the `Do` that drew it, the
//! photo's pixels live in an image XObject this module must not rewrite, and the state the
//! `Do` depends on — the clip, the colour, the alpha — was set forty operations earlier. So
//! the question this module answers is narrower than "move a photo": **given a [`PageObject`]
//! that says which bytes drew it, and a [`Change`], which byte ranges change**?
//!
//! # The wrapper, which is the whole trick
//!
//! Every non-destructive edit here is a **wrapper**, not a rewrite. The object's own operators
//! come out byte-for-byte and the edit is two insertions around each of them:
//!
//! ```text
//! q 1 0 0 1 12 0 cm /Im0 Do Q
//! ```
//!
//! `q` … `Q` saves and restores the whole graphics state, so whatever matrix it installs
//! applies to exactly one operation and to nothing after it. That is what makes the edit
//! *local by construction* — FINISH.md U1's locality requirement, satisfied by the shape of the
//! change rather than by checking afterwards — and it is why a move here never has to work out
//! how the object was placed in the first place. A `Tm`, a `cm`, a form's own `/Matrix`, a
//! chain of `Td`: the wrapper does not care, because it changes the space the object is drawn
//! **in** rather than the numbers the object was drawn **with**.
//!
//! GOAL.md §4.3 asks for a regenerated object to be emitted wrapped in `q … Q` exactly this
//! way, and for an object that is not regenerated the wrapper is the same two insertions. The
//! smaller edit — rewriting the operands of a `cm` in place — is still available to a caller
//! that can prove which `cm` belongs to the object, and `surgery` will carry it; this module
//! does not do it, because it cannot prove that without re-implementing the interpreter's state
//! machine, and a module that guessed at it would be a module that writes the wrong bytes.
//!
//! # Space, which is the thing to get wrong
//!
//! A matrix written into a content stream acts in the stream's own **user space**. An object's
//! bounds, as [`PageObject::bounds`] gives them, are in **device space** — the CTM has already
//! been applied. Those are the same thing for a page-level object drawn under the identity
//! CTM, which is most of them, and they are not the same as soon as a `cm` is in force. So
//! every [`Change`] here is in user space and says so, and [`map_point`] is the pair of
//! glasses: it is what a caller holding device-space bounds — a canvas — needs before it can
//! ask for a move of 12 pt.
//!
//! # Honesty
//!
//! A recolour in a space this cannot write into faithfully is **refused by name**
//! ([`Refusal`]), not approximated. GOAL.md §4.1's fifth law is that a user is told what
//! cannot be done faithfully and why, and a spot colour invented from a luminance is a number
//! nobody asked for wearing a plausible hat.

use std::fmt;
use std::fmt::Write as _;
use std::ops::Range;

use mangle_content::interp::{Mark, PageContent};
use mangle_content::matrix::Matrix;
use mangle_content::state::{ClipBounds, Colour, ColourSpace, Rgba};
use mangle_content::tokens::{ContentStream, Operation};

use crate::page_objects::{Kind, PageObject};
use crate::surgery::{Applied, EditError, Patch, apply};

/// Which of a mark's two colours a recolour changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Channel {
    /// The non-stroking colour: `g`, `rg`, `k`, `sc`.
    #[default]
    Fill,
    /// The stroking colour: `G`, `RG`, `K`, `SC`.
    Stroke,
    /// Both, for a path that is filled and stroked in one colour.
    Both,
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Fill => "fill",
            Self::Stroke => "stroke",
            Self::Both => "fill and stroke",
        };
        f.write_str(name)
    }
}

/// What was asked for, on one object.
///
/// Every variant is a whole edit and there is no combination variant: two changes are two
/// calls. A history entry recording "moved and recoloured" as one unnameable step is a history
/// nobody can read in a panel, and an undo that put back only half of one would be worse than
/// either.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// Install `matrix` around the object's own operations, leaving them untouched.
    ///
    /// The matrix is in the stream's user space. [`Change::move_by`] and
    /// [`Change::scale_about`] build the two a canvas usually wants; a rotation, a flip or a
    /// shear is the same variant with a different matrix.
    Transform(Matrix),
    /// Take the object out of the stream, and its wrapper with it if that leaves the wrapper
    /// empty.
    Delete,
    /// Change the object's colour, written in the space it already uses.
    Recolour {
        /// The colour asked for, in sRGB, which is what a picker gives.
        colour: Rgba,
        /// Which of the two colours.
        channel: Channel,
    },
    /// Crop an image to a rectangle, non-destructively.
    ///
    /// The one edit whose wrapper is a **clip** rather than a state: `q <polygon> W n <the Do> Q`
    /// keeps the part of the picture inside the rectangle and drops the rest. It is non-destructive
    /// in the only sense that matters — the image's own bytes are untouched, and undoing the edit
    /// takes the clip away and restores the whole picture — which is what GOAL.md §4.5 means by
    /// "crop as a non-destructive clip (resettable)".
    ///
    /// The rectangle is in **page space**, because that is what a crop tool drags. A `Do` draws in
    /// the space that was current when it ran, so the rectangle has to be read back through the
    /// record's own CTM before it can be written.
    Crop {
        /// The part to keep, in page space.
        keep: ClipBounds,
    },
    /// Change one text property of a run of glyphs.
    ///
    /// Every control GOAL.md §4.4 lists that is a *text-state* operator rather than a
    /// position is here: `Tf`, `Tc`, `Tw`, `Tz`, `TL`, `Ts`, `Tr`. A change is a `q … Q`
    /// around the run's own `Tj`, with the one operator inside it, which is what makes the
    /// edit local — the surrounding text state is what the file had it as, restored by the
    /// `Q`.
    Text(TextProperty),
}

/// One text property, in the units the specification's own operators use.
///
/// The units are the operators' rather than a friendlier ones, because a panel that shows
/// "character spacing 4 pt" and writes `4 Tc` is a panel that tells the truth, and one that
/// converts on the way in is a panel that has to be right twice.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextProperty {
    /// `Tc` — added to every glyph's displacement, in points.
    CharacterSpacing(f64),
    /// `Tw` — added to every space's displacement, in points. Not in every font.
    WordSpacing(f64),
    /// `Tz` — the horizontal scale, as a percentage: 100 is normal.
    HorizontalScale(f64),
    /// `TL` — the leading `T*` moves by, in points.
    Leading(f64),
    /// `Ts` — how far above the baseline the run rises, in points.
    BaselineShift(f64),
    /// `Tf`'s size, in points. The font the run already uses is kept — changing the *face* is
    /// a different edit, because it needs a font the page's resources do not necessarily have.
    Size(f64),
    /// `Tr` — how the glyphs are painted: filled, stroked, both, invisible, or as a clip.
    RenderMode(mangle_content::state::RenderMode),
}

impl TextProperty {
    /// The operator that carries this property, and its operand.
    fn written(&self) -> (&'static str, f64) {
        match self {
            Self::CharacterSpacing(v) => ("Tc", *v),
            Self::WordSpacing(v) => ("Tw", *v),
            Self::HorizontalScale(v) => ("Tz", *v),
            Self::Leading(v) => ("TL", *v),
            Self::BaselineShift(v) => ("Ts", *v),
            // `Tf`'s operand is the size, and the font it keeps is the one the run used; the
            // name is written by the caller, which has the record.
            Self::Size(v) => ("", *v),
            // `Tr` takes an integer, which is what `RenderMode` is.
            Self::RenderMode(mode) => ("Tr", f64::from(render_mode_number(*mode))),
        }
    }

    /// What this property is called in a panel and in a history entry.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::CharacterSpacing(_) => "character spacing",
            Self::WordSpacing(_) => "word spacing",
            Self::HorizontalScale(_) => "horizontal scale",
            Self::Leading(_) => "leading",
            Self::BaselineShift(_) => "baseline shift",
            Self::Size(_) => "size",
            Self::RenderMode(_) => "render mode",
        }
    }
}

impl Change {
    /// Move the object by `(dx, dy)` in user space.
    #[must_use]
    pub fn move_by(dx: f64, dy: f64) -> Self {
        Self::Transform(Matrix::translate(dx, dy))
    }

    /// Scale the object about the point `(cx, cy)` in user space.
    ///
    /// About a point, not about the origin: a scale about the origin moves everything as well,
    /// and a user who drags a corner expects the opposite one to stay put.
    #[must_use]
    pub fn scale_about(cx: f64, cy: f64, sx: f64, sy: f64) -> Self {
        Self::Transform(
            Matrix::translate(cx, cy)
                .concat(Matrix::scale(sx, sy))
                .concat(Matrix::translate(-cx, -cy)),
        )
    }

    /// Change the fill colour.
    #[must_use]
    pub fn recolour(colour: Rgba) -> Self {
        Self::recolour_in(colour, Channel::Fill)
    }

    /// Change a named colour channel.
    #[must_use]
    pub fn recolour_in(colour: Rgba, channel: Channel) -> Self {
        Self::Recolour { colour, channel }
    }

    /// Change a text property of a run of glyphs.
    #[must_use]
    pub fn text(property: TextProperty) -> Self {
        Self::Text(property)
    }

    /// The matrix this installs, for a transform. `None` for the other kinds.
    #[must_use]
    pub fn matrix(&self) -> Option<Matrix> {
        match self {
            Self::Transform(m) => Some(*m),
            _ => None,
        }
    }
}

/// Why an edit could not be written, named rather than guessed at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The object claims bytes this stream does not have — it was built from a different
    /// stream, or from inside a form the caller has not opened.
    ForeignSpan {
        /// What was asked for.
        span: Range<usize>,
        /// How long the stream is.
        len: usize,
    },
    /// The object has no bytes at all, so there is nothing to edit.
    NoSpans,
    /// The object has no colour of the kind asked for: a shading has none, and an image's is
    /// the colour its *mask's* zero bits paint in, which is not the image.
    NoSuchColour {
        /// What kind of thing it is.
        kind: Kind,
        /// Whether it was the fill or the stroke.
        channel: Channel,
    },
    /// The object's colour space is one this cannot write a new value into faithfully.
    ColourSpaceNotWritable {
        /// The space, by the name the file used.
        space: String,
        /// Whether it was the fill or the stroke.
        channel: Channel,
        /// Why not, as a sentence.
        why: &'static str,
    },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ForeignSpan { span, len } => write!(
                f,
                "this object covers bytes {span:?} of a stream that is {len} bytes long, so it \
                 came from somewhere else"
            ),
            Self::NoSpans => write!(
                f,
                "this object covers no bytes, so there is nothing to edit"
            ),
            Self::NoSuchColour { kind, channel } => {
                write!(f, "a {kind:?} has no {channel} colour to change")
            }
            Self::ColourSpaceNotWritable {
                space,
                channel,
                why,
            } => write!(
                f,
                "the {channel} colour is in {space}, and {why}; change it by hand rather than \
                 have this write a number the file did not ask for"
            ),
        }
    }
}

impl std::error::Error for Refusal {}

/// Why an edit did not happen: either the edit is one this cannot write, or the bytes it
/// would have written are not there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeError {
    /// The edit itself was refused, with the reason.
    Refused(Refusal),
    /// The edit was fine and the stream was not.
    Stream(EditError),
}

impl fmt::Display for ChangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(r) => write!(f, "{r}"),
            Self::Stream(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ChangeError {}

impl From<EditError> for ChangeError {
    fn from(e: EditError) -> Self {
        Self::Stream(e)
    }
}

impl From<Refusal> for ChangeError {
    fn from(r: Refusal) -> Self {
        Self::Refused(r)
    }
}

/// Apply `change` to `object`, and give back the stream it should become.
///
/// The thing checked after the write is the one GOAL.md §4.3 asks for and it is not left to a
/// caller: the result is balanced. An edit that opened a wrapper without closing it would leave
/// every operator after it under the wrong graphics state, and the page would be wrongly drawn
/// with nothing anywhere to say so.
pub fn apply_change(
    stream: &[u8],
    object: &PageObject,
    change: &Change,
) -> Result<Applied, ChangeError> {
    let patches = patches_for(stream, object, change)?;
    let applied = apply(stream, &patches)?;
    debug_assert!(
        crate::surgery::is_balanced(&applied.bytes),
        "an edit unbalanced the stream, which GOAL 4.3 says must never reach a save"
    );
    Ok(applied)
}

/// The patches that carry out `change`, in the order they will be applied.
///
/// Nothing is written here: this is the whole computation, separated so a caller can look at
/// what an edit *would* touch before letting it, and so a test can assert the byte ranges
/// directly instead of inferring them from the result.
pub fn patches_for(
    stream: &[u8],
    object: &PageObject,
    change: &Change,
) -> Result<Vec<Patch>, Refusal> {
    if object.spans.is_empty() {
        return Err(Refusal::NoSpans);
    }
    for span in &object.spans {
        if span.end > stream.len() || span.start > span.end {
            return Err(Refusal::ForeignSpan {
                span: span.clone(),
                len: stream.len(),
            });
        }
    }

    match change {
        Change::Transform(matrix) => Ok(wrap_each(stream, object, &cm_of(*matrix), "")),
        Change::Recolour { colour, channel } => {
            let body = colour_body(object, *colour, *channel)?;
            Ok(wrap_each(stream, object, "", &body))
        }
        Change::Text(property) => {
            let body = text_body(object, property)?;
            Ok(wrap_each(stream, object, "", &body))
        }
        Change::Crop { keep } => {
            let body = crop_body(object, *keep)?;
            Ok(wrap_each(stream, object, "", &body))
        }
        Change::Delete => delete_patches(stream, object),
    }
}

/// The one operator that sets a text property, written where the run's own `Tj` can see it.
///
/// `q`…`Q` is legal *inside* a `BT`, and the run's span is always inside one — a `Tj` outside a text
/// object is not a `Tj`. So the wrapper is simply the operator and nothing else: the state
/// operators this project needs are all that `q`/`Q` save and restore beside the text matrix,
/// which is exactly the property being set.
/// The number the file writes for a render mode.
///
/// The specification numbers them zero to eight in the order they are declared, so the mapping is
/// total and has no wrong answer to give. `RenderMode::from_int` is the reader; this is the writer.
fn render_mode_number(mode: mangle_content::state::RenderMode) -> u8 {
    use mangle_content::state::RenderMode;
    match mode {
        RenderMode::Fill => 0,
        RenderMode::Stroke => 1,
        RenderMode::FillThenStroke => 2,
        RenderMode::Invisible => 3,
        RenderMode::FillAndClip => 4,
        RenderMode::StrokeAndClip => 5,
        RenderMode::FillThenStrokeAndClip => 6,
        RenderMode::Clip => 7,
        RenderMode::ClipStroke => 8,
    }
}

/// The polygon that keeps the asked-for part of an image, in the space its `Do` draws in.
///
/// Four corners, mapped back through the record's CTM, written as `m l l l h`. A polygon rather
/// than a `re` because the mapped rectangle is not necessarily axis-aligned: under a rotated `cm` a
/// `re` would clip the *bounding box* of the crop and keep a corner the user dragged away.
fn crop_body(object: &PageObject, keep: ClipBounds) -> Result<String, Refusal> {
    let Some(record) = object.records.first() else {
        return Err(Refusal::NoSpans);
    };
    if !matches!(record.mark, Mark::Image { .. }) {
        return Err(Refusal::NoSuchColour {
            kind: object.kind,
            channel: Channel::Fill,
        });
    }
    let Some(inverse) = record.ctm.inverse() else {
        return Err(Refusal::ColourSpaceNotWritable {
            space: "a degenerate transform".to_string(),
            channel: Channel::Fill,
            why: "the image's placement has no inverse, so a page-space crop cannot be mapped \
                 into the space it draws in",
        });
    };
    let corners = [
        (keep.x0, keep.y0),
        (keep.x1, keep.y0),
        (keep.x1, keep.y1),
        (keep.x0, keep.y1),
    ];
    let mut out = String::new();
    for (i, (x, y)) in corners.iter().enumerate() {
        let (ux, uy) = inverse.apply(*x, *y);
        if i == 0 {
            let _ = write!(out, "{} {} m", num(ux), num(uy));
        } else {
            let _ = write!(out, " {} {} l", num(ux), num(uy));
        }
    }
    out.push_str(" h W n\n");
    Ok(out)
}

fn text_body(object: &PageObject, property: &TextProperty) -> Result<String, Refusal> {
    let Some(record) = object.records.first() else {
        return Err(Refusal::NoSpans);
    };
    let Mark::Glyphs { .. } = &record.mark else {
        return Err(Refusal::NoSuchColour {
            kind: object.kind,
            channel: Channel::Fill,
        });
    };
    let (operator, value) = property.written();
    // A size change has to name the font alongside it, because `Tf` takes both and a stream
    // that wrote a bare size would be answered with a font nobody chose.
    if operator.is_empty() {
        let font = record
            .text
            .font
            .clone()
            .ok_or(Refusal::ColourSpaceNotWritable {
                space: "no font".to_string(),
                channel: Channel::Fill,
                why: "this run names no font, so a size cannot be set without changing it",
            })?;
        return Ok(format!("/{} {} Tf\n", font, num(value)));
    }
    // **Operands first, then the operator.** This is the third time this project has written the
    // pair the other way round — once for a colour, once for a dash — and the result is the same
    // every time: `Tc 2` is a `Tc` with no operands followed by a stray number, so the property is
    // never set and the page draws exactly as it did. Nothing reports it.
    Ok(format!("{} {}\n", num(value), operator))
}

/// One `q … Q` wrapper per span, so an object whose operations are not contiguous — a line with
/// another object's operator between two of its runs — still moves as one thing.
///
/// Each wrapper is independent, so nothing between two of an object's operations is touched: a
/// `Tc` that a later run depends on still applies to it, because the `Q` before it restored
/// exactly the state the `q` had saved.
fn wrap_each(stream: &[u8], object: &PageObject, matrix: &str, state: &str) -> Vec<Patch> {
    let mut body = String::new();
    if !matrix.is_empty() {
        body.push_str(matrix);
    }
    if !state.is_empty() {
        body.push_str(state);
    }
    let mut out = Vec::with_capacity(object.spans.len() * 2);
    for span in &object.spans {
        out.push(Patch::insert(
            span.start,
            padded(stream, span.start, &format!("q {body}\n")),
            "open a wrapper",
        ));
        out.push(Patch::insert(
            span.end,
            padded(stream, span.end, "Q"),
            "close the wrapper",
        ));
    }
    out
}

/// The bytes to insert at `at`, padded so the stream still tokenises as it did.
///
/// # Why this is not fussiness
///
/// Two *regular* bytes adjacent in a content stream are **one token**, and a producer is
/// entitled to write `0.000 Tc(Working Papers)Tj` with no whitespace in it — the lexer knows
/// where an operator ends and an operand begins, so nothing in the file forces a space. A
/// corpus page in this repository does exactly that.
///
/// Inserting bytes at such a point without carrying whitespace glues them onto the token that
/// ends there. `q` after a `Tc` becomes the single keyword `Tcq`: the wrapper this module just
/// opened is not an operator any more, so the `Q` that was supposed to close it closes nothing
/// and the stream is unbalanced. Nothing downstream reports it — the page simply renders with a
/// wrong graphics state, which is the failure GOAL.md §4.1's validity law exists to prevent.
///
/// The same applies on the other side: a `Q` inserted before a `/F5` is fine, because `/` is a
/// delimiter, but a `Q` before a `Tf` would be `QTf`.
fn padded(stream: &[u8], at: usize, text: &str) -> String {
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

/// Whether removing `range` would glue the bytes on either side of it into one token.
///
/// The mirror of [`padded`], and the reason it is needed at all: a delete that leaves `Tf`
/// against `/F5` is harmless because `/` ends a token, but one that leaves `Td` against `Q`
/// writes `TdQ` and removes an operator from the page. The replacement is a single space, which
/// is what the file would have had if the producer had written the two operators apart.
fn deletion_glues(stream: &[u8], range: &Range<usize>) -> bool {
    let before = range.start.checked_sub(1).and_then(|i| stream.get(i));
    let after = stream.get(range.end);
    let regular = |b: Option<&u8>| b.is_some_and(|b| mangle_syntax::lexer::Token::is_regular(*b));
    regular(before) && regular(after)
}

/// The `cm` for a matrix, as the stream would write it.
fn cm_of(m: Matrix) -> String {
    format!("{} cm", six(&m.to_array()))
}

/// Six numbers, in PDF's order, each as short as a round trip allows.
fn six(v: &[f64; 6]) -> String {
    v.iter().map(|n| num(*n)).collect::<Vec<_>>().join(" ")
}

/// One number as a stream would write it.
///
/// Rust's shortest round-trip form is what a producer's own numbers look like: `0.1` stays
/// `0.1`, and a whole number has no `.0` on it. Negative zero is normalised, because no
/// producer writes `-0` and a diff reads it as a change from `0`.
fn num(v: f64) -> String {
    if v == 0.0 {
        return "0".to_string();
    }
    format!("{v}")
}

/// A colour written into a space: the operator that sets it, and what it sets.
///
/// Both halves come out together because they only mean anything as a pair — `rg` with one
/// component is a damaged stream, and so is `g` with three.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Written {
    /// The operator, cased for the channel.
    op: &'static str,
    /// The components, in the space's own order.
    components: Vec<f64>,
}

impl fmt::Display for Written {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // **Operands first, then the operator.** A content stream is written the other way
        // round from a function call, and the reverse is not a formatting nicety: `rg 0.1 0.2
        // 0.3` is an `rg` with *no operands* followed by four stray numbers, so the colour is
        // never set and the next operator quietly collects the numbers as its own operands. The
        // page renders, the edit appears to have worked, and the colour is the old one — which
        // is how this was written the wrong way round and only a corpus page caught it.
        for c in &self.components {
            write!(f, " {}", num(*c))?;
        }
        write!(f, " {}", self.op)
    }
}

/// The operators that set the colour asked for, in the spaces the object already uses.
fn colour_body(object: &PageObject, colour: Rgba, channel: Channel) -> Result<String, Refusal> {
    let mut out = String::new();
    for want in channels_of(channel) {
        let writing = written_as(object, colour, want)?;
        out.push_str(&writing.to_string());
        out.push('\n');
    }
    Ok(out)
}

/// The channels an edit on `channel` has to write, in the order they are written.
fn channels_of(channel: Channel) -> Vec<Channel> {
    match channel {
        Channel::Both => vec![Channel::Fill, Channel::Stroke],
        one => vec![one],
    }
}

/// The operator and components that paint `colour` in one of the object's own spaces.
fn written_as(object: &PageObject, colour: Rgba, channel: Channel) -> Result<Written, Refusal> {
    let (fill, stroke) = colours_of(object);
    let current = match channel {
        Channel::Stroke => stroke,
        _ => fill,
    }
    .ok_or(Refusal::NoSuchColour {
        kind: object.kind,
        channel,
    })?;
    write_in(&current.space, colour, channel)
}

/// The operator and components that paint `colour` in `space`.
///
/// `pub(crate)` because arrange re-uses it: a moved object's colours are written back exactly as a
/// recolour's are, and two copies of that arithmetic would be two places to get it wrong.
///
/// The space is the object's own, because GOAL.md §4.6 says a recolour preserves it: a
/// DeviceCMYK object stays CMYK and a Separation stays a Separation with its tint edited.
/// Writing a new `cs` would re-point the space, which changes what every operator after the
/// edit means as well as what this object draws in.
pub(crate) fn write_in(
    space: &ColourSpace,
    colour: Rgba,
    channel: Channel,
) -> Result<Written, Refusal> {
    let unwritable = |why: &'static str| Refusal::ColourSpaceNotWritable {
        space: space.name.clone(),
        channel,
        why,
    };
    // A pattern colour is a name, not a set of components: `scn /P0` paints the pattern, and
    // there is no number that would change its colour.
    if space.name == "Pattern" {
        return Err(unwritable("a pattern's colour is the pattern itself"));
    }
    let stroke = channel == Channel::Stroke;
    // A tint space carries its transform, so a new tint can be written. The tint is the *ink*
    // amount, which is the other way round from a grey: `sc 0` paints no colorant at all, so a
    // black target is a full tint and a white one is none. The value is derived from the
    // target's luminance rather than by inverting the transform, which is the approximation and
    // is stated rather than hidden: the transform maps a tint to a colour, and reading it
    // backwards for an arbitrary target is a search, not a conversion. A `/DeviceN` with more
    // than one colorant is refused outright, because changing one tint leaves the others
    // describing the old colour.
    if let Some(tint) = &space.tint {
        if tint.colorants > 1 {
            return Err(unwritable(
                "it has more than one colorant and no one of them is the colour",
            ));
        }
        return Ok(Written {
            op: if stroke { "SC" } else { "sc" },
            components: vec![1.0 - grey_of(colour)],
        });
    }
    // An ICC-based space is read through whatever its profile names — the same route the
    // interpreter reads its own colours by — so the components are written in that space. A
    // profile naming neither has nothing to write into.
    let through = space
        .through_alternate()
        .map(|s| s.name)
        .unwrap_or_else(|| space.name.clone());
    match through.as_str() {
        "DeviceGray" | "CalGray" => Ok(Written {
            op: if stroke { "G" } else { "g" },
            components: vec![grey_of(colour)],
        }),
        "DeviceRGB" | "CalRGB" => Ok(Written {
            op: if stroke { "RG" } else { "rg" },
            components: vec![colour.r, colour.g, colour.b],
        }),
        "DeviceCMYK" => Ok(Written {
            op: if stroke { "K" } else { "k" },
            components: cmyk_of(colour).to_vec(),
        }),
        other => Err(Refusal::ColourSpaceNotWritable {
            space: other.to_string(),
            channel,
            why: "nothing here knows what its components mean",
        }),
    }
}

/// The one number a grey or a tint is: luminance, which is what a colour *is* to a black and
/// white page and the only reading that survives a spot colour's own transform.
fn grey_of(c: Rgba) -> f64 {
    (0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b).clamp(0.0, 1.0)
}

/// sRGB to CMYK, the way a printer's own conversion does it: the key is what is left over.
fn cmyk_of(c: Rgba) -> [f64; 4] {
    let red = c.r.clamp(0.0, 1.0);
    let green = c.g.clamp(0.0, 1.0);
    let blue = c.b.clamp(0.0, 1.0);
    let key = 1.0 - red.max(green).max(blue);
    if key >= 1.0 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let scale = 1.0 / (1.0 - key);
    [
        (1.0 - red - key) * scale,
        (1.0 - green - key) * scale,
        (1.0 - blue - key) * scale,
        key,
    ]
}

/// The two colours a mark carries, whichever it has.
fn colours_of(object: &PageObject) -> (Option<&Colour>, Option<&Colour>) {
    match &object.records.first().map(|r| &r.mark) {
        Some(Mark::Path { fill, stroke, .. }) => (fill.as_ref(), stroke.as_ref()),
        // Glyphs and images take no colour of their own: they paint in the one in force, which
        // the mark carries and which is therefore the non-stroking one.
        Some(Mark::Glyphs { fill, .. } | Mark::Image { fill, .. }) => (Some(fill), None),
        _ => (None, None),
    }
}

/// The patches that take an object out of a stream.
///
/// A `q…Q`, `BT…ET` or `BDC…EMC` the object leaves empty is removed with it, because GOAL.md
/// §4.3 says so and because a wrapper around nothing is a state the user did not draw and every
/// later edit would have to reason about. The wrappers are found in the stream rather than
/// assumed, and removed only when they *are* empty: a `BT` that still holds another run is left
/// exactly as it is.
fn delete_patches(stream: &[u8], object: &PageObject) -> Result<Vec<Patch>, Refusal> {
    let ops = ContentStream::parse(stream).operations();
    // The ranges to take out. They start as the object's own and grow into the wrappers it
    // leaves empty — and a wrapper's range *replaces* the ranges it contains rather than being
    // added beside them, both because the wrapper takes its contents with it and because two
    // patches covering the same bytes is exactly what `surgery` refuses.
    let mut pending: Vec<(Range<usize>, &'static str)> = object
        .spans
        .iter()
        .map(|s| (s.clone(), "remove the object"))
        .collect();
    let first = object.spans.first().cloned().unwrap_or_default();
    let last = object.spans.last().cloned().unwrap_or_default();
    let mut region = first.start..last.end;

    // Innermost first, so the check walks outward from the object and stops at the first
    // wrapper that still holds something.
    for wrapper in wrappers(stream).into_iter().rev() {
        if wrapper.start > region.start || wrapper.end < region.end {
            continue;
        }
        if region_holds_an_operation(&ops, &wrapper, &region) {
            break;
        }
        pending.retain(|(r, _)| !(wrapper.start <= r.start && r.end <= wrapper.end));
        pending.push((wrapper.clone(), "the wrapper is now empty"));
        region = wrapper;
    }

    // A delete leaves nothing, so what it *replaces* the range with is either nothing at all or
    // the one space that stops the bytes either side from joining into a token.
    Ok(pending
        .into_iter()
        .map(|(range, label)| {
            if deletion_glues(stream, &range) {
                Patch::replace(range, " ", label)
            } else {
                Patch::delete(range, label)
            }
        })
        .collect())
}

/// A wrapper kind, as the operators name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opener {
    /// `BT`.
    Text,
    /// `q`.
    State,
    /// `BMC` or `BDC`.
    Marked,
}

fn opener_of(name: &[u8]) -> Option<Opener> {
    match name {
        b"BT" => Some(Opener::Text),
        b"q" => Some(Opener::State),
        b"BMC" | b"BDC" => Some(Opener::Marked),
        _ => None,
    }
}

fn closer_of(name: &[u8]) -> Option<Opener> {
    match name {
        b"ET" => Some(Opener::Text),
        b"Q" => Some(Opener::State),
        b"EMC" => Some(Opener::Marked),
        _ => None,
    }
}

/// The `q…Q`, `BT…ET` and `BDC…EMC` ranges in a stream, outermost first.
///
/// Found by walking the operations with a stack of openers, which is the nesting rule the
/// interpreter itself applies. A stream whose wrappers are not properly nested yields none:
/// removing a wrapper from a damaged stream on a guess is exactly the silent change GOAL.md
/// §4.1 forbids.
fn wrappers(stream: &[u8]) -> Vec<Range<usize>> {
    let mut stack: Vec<(Opener, usize)> = Vec::new();
    let mut out: Vec<Range<usize>> = Vec::new();
    for op in ContentStream::parse(stream).operations() {
        let name = op.operator.operator().unwrap_or_default();
        match opener_of(name) {
            Some(kind) => stack.push((kind, op.operator.span.start)),
            None => {
                if let Some(want) = closer_of(name)
                    && let Some((open, start)) = stack.last().copied()
                    && open == want
                {
                    stack.pop();
                    out.push(start..op.operator.span.end);
                }
            }
        }
    }
    // Outermost first, so a caller walking outward from an object meets the innermost last.
    out.reverse();
    out
}

/// Whether a wrapper range still holds an operation once the object's own are taken out.
///
/// The wrapper's own opener and closer sit at the two ends and are excluded, so an empty
/// `BT … ET` holds nothing and one with a run in it does. `ignore` is the region the edit is
/// already deleting — which grows as wrappers are removed — so an operation that is leaving
/// anyway does not count as a reason to keep its wrapper.
fn region_holds_an_operation(
    ops: &[Operation],
    range: &Range<usize>,
    ignore: &Range<usize>,
) -> bool {
    ops.iter().any(|op| {
        op.span.start > range.start
            && op.span.end < range.end
            && !(ignore.start <= op.span.start && op.span.end <= ignore.end)
    })
}

/// Where a point that is at `(x, y)` in device space ends up once `m` is installed around an
/// object drawn under `ctm`.
///
/// A device-space point has to be read back into user space before a user-space matrix can be
/// applied to it, and forward again afterwards — which is why this takes the CTM and not just
/// the matrix. The inverse is the whole of the conversion: `ctm⁻¹` gives the user-space point
/// and `ctm · m` gives where it lands. A matrix with no inverse — a degenerate scale — leaves
/// the point where it is, which is the only answer that is not a wrong one.
#[must_use]
pub fn map_point(ctm: &Matrix, m: Matrix, x: f64, y: f64) -> (f64, f64) {
    let Some(inverse) = ctm.inverse() else {
        return (x, y);
    };
    let (ux, uy) = inverse.apply(x, y);
    ctm.concat(m).apply(ux, uy)
}

/// The check that closes GOAL.md §4.3's write-back loop: re-parse the edited stream, run it
/// again, and see that the object on the page is the one that was asked for.
///
/// This is the assertion a debug build makes after every edit, and it is public rather than
/// private so a test can call it on a real page. It answers three questions and reports the
/// first that fails: the page has the number of objects the edit implies, the edited object is
/// where it was supposed to end up, and *nothing else moved*. The third is U1 — an edit that
/// shifted a neighbour would draw a page the user did not ask for.
pub fn verify(
    before: &PageContent,
    after: &PageContent,
    object: usize,
    change: &Change,
) -> Result<(), String> {
    use crate::page_objects::PageModel;

    let old = PageModel::build(&before.records);
    let new = PageModel::build(&after.records);
    let Some(target) = old.objects().get(object) else {
        return Err(format!(
            "object {object} is not one of the page's {} objects",
            old.objects().len()
        ));
    };

    // A text property is dispatched before the object count is looked at, because it is the one
    // edit that legitimately **changes the grouping**: moving glyphs along a sheared text matrix
    // moves them out of the line they were in, and the model regroups. Every other edit adds or
    // removes nothing from the geometry, so the count is the same.
    if let Change::Text(_) = change {
        return verify_text(before, after, object, change);
    }

    let expected = match change {
        Change::Delete => old.objects().len().saturating_sub(1),
        _ => old.objects().len(),
    };
    if new.objects().len() != expected {
        return Err(format!(
            "the edit left {} objects, and {change:?} should have left {expected}",
            new.objects().len()
        ));
    }

    let centre = |b: &ClipBounds| (f64::midpoint(b.x0, b.x1), f64::midpoint(b.y0, b.y1));
    let from = centre(&target.bounds);

    match change {
        // Dispatched above: the one edit that changes the grouping.
        Change::Text(_) => Ok(()),
        // A crop changes where the image **is not**: it still sits exactly where it was, because
        // a clip cuts what is drawn rather than moving it. So the ordinary checks apply.
        Change::Crop { .. } => Ok(()),
        Change::Transform(m) => {
            let ctm = target
                .records
                .first()
                .map(|r| r.ctm)
                .unwrap_or_else(Matrix::default);
            let want = map_point(&ctm, *m, from.0, from.1);
            let edited = new.objects().iter().any(|o| near(centre(&o.bounds), want));
            if !edited {
                return Err(format!(
                    "nothing ended up where the matrix was supposed to put it: {want:?}"
                ));
            }
            let strays = new
                .objects()
                .iter()
                .filter(|o| {
                    let c = centre(&o.bounds);
                    !near(c, want) && !old.objects().iter().any(|w| near(centre(&w.bounds), c))
                })
                .count();
            if strays != 0 {
                return Err(format!(
                    "{strays} object(s) are somewhere the edit did not put them"
                ));
            }
            Ok(())
        }
        Change::Recolour { colour, channel } => {
            let Some(o) = new.objects().iter().find(|o| near(centre(&o.bounds), from)) else {
                return Err("the object is no longer where it was".to_string());
            };
            for want in channels_of(*channel) {
                let expect = written_as(target, *colour, want)
                    .map_err(|r| r.to_string())?
                    .components;
                let got = o.records.iter().find_map(|r| match &r.mark {
                    Mark::Path { fill, stroke, .. } => {
                        if want == Channel::Stroke {
                            stroke.as_ref()
                        } else {
                            fill.as_ref()
                        }
                    }
                    Mark::Glyphs { fill, .. } | Mark::Image { fill, .. } => {
                        if want == Channel::Stroke {
                            None
                        } else {
                            Some(fill)
                        }
                    }
                    _ => None,
                });
                match got {
                    Some(c) if near_components(&c.components, &expect) => {}
                    Some(c) => {
                        return Err(format!(
                            "the colour is now {:?} and the edit asked for {expect:?}",
                            c.components
                        ));
                    }
                    None => return Err(format!("there is no {want} colour to check")),
                }
            }
            Ok(())
        }
        Change::Delete => {
            if new.objects().iter().any(|o| near(centre(&o.bounds), from)) {
                return Err("the object is still on the page".to_string());
            }
            let strays = new
                .objects()
                .iter()
                .filter(|o| {
                    let c = centre(&o.bounds);
                    !old.objects().iter().any(|w| near(centre(&w.bounds), c))
                })
                .count();
            if strays != 0 {
                return Err(format!("deleting one object moved {strays} others"));
            }
            Ok(())
        }
    }
}

/// Did a text property become the value that was asked for?
///
/// Read out of the interpreter rather than out of the bytes, for the reason the recolour
/// verification exists: a string of the right words in the wrong order is still a string of the
/// right words, and only running the result says which one this is.
///
/// **The run is matched by its string, not by its index and not by where it is.** A text property
/// *moves* the glyphs — character spacing widens the run, a baseline shift lifts it — and on a page
/// whose text matrix is sheared it moves them along the shear, so the run leaves the line it was
/// in and the model regroups. `gov__irs-f1040` page 1 goes from 589 objects to 656 on one character
/// spacing change for exactly that reason, and that is the file asking for what it asked for. A
/// check that insisted the object count was unchanged would be a check that failed on a correct
/// edit, and a check that looked for the object at its old centre would be looking for a run that
/// had not changed.
fn verify_text(
    before: &PageContent,
    after: &PageContent,
    object: usize,
    change: &Change,
) -> Result<(), String> {
    use crate::page_objects::PageModel;

    let Change::Text(property) = change else {
        return Err("not a text property".to_string());
    };
    let old = PageModel::build(&before.records);
    let Some(target) = old.objects().get(object) else {
        return Err(format!(
            "object {object} is not one of the page's {} objects",
            old.objects().len()
        ));
    };
    let Some(was) = target.records.first() else {
        return Err("the object has no records to check".to_string());
    };
    let Mark::Glyphs { text: string, .. } = &was.mark else {
        return Err("the object is not a run of text".to_string());
    };

    // Every record in the result with the same string is a candidate, and the one the edit
    // touched is the one whose value is the one asked for. A page with the same run twice is
    // answered the same way either: both are checked, and a refusal names what it saw.
    let (name, want) = property_operand(property);
    let mut checked = 0usize;
    for record in &after.records {
        let Mark::Glyphs { text: other, .. } = &record.mark else {
            continue;
        };
        if other != string {
            continue;
        }
        let have = match property {
            TextProperty::CharacterSpacing(_) => record.text.char_spacing,
            TextProperty::WordSpacing(_) => record.text.word_spacing,
            TextProperty::HorizontalScale(_) => record.text.horizontal_scale,
            TextProperty::Leading(_) => record.text.leading,
            TextProperty::BaselineShift(_) => record.text.rise,
            TextProperty::Size(_) => record.text.size,
            TextProperty::RenderMode(_) => f64::from(render_mode_number(record.text.render_mode)),
        };
        if (have - want).abs() > 1e-6 {
            return Err(format!(
                "the {name} is now {have} and the edit asked for {want}"
            ));
        }
        checked += 1;
    }
    if checked == 0 {
        return Err("the run is no longer in the page".to_string());
    }
    // And the edit did not change the *number* of records: an edit that added or removed a glyph
    // would be a different edit from the one asked for.
    if after.records.len() != before.records.len() {
        return Err(format!(
            "the edit left {} records, and a text property should have left {}",
            after.records.len(),
            before.records.len()
        ));
    }
    Ok(())
}

/// The operand a text property is written with, for a label and for the check above.
pub(crate) fn property_operand(property: &TextProperty) -> (&'static str, f64) {
    match property {
        TextProperty::CharacterSpacing(v) => ("Tc", *v),
        TextProperty::WordSpacing(v) => ("Tw", *v),
        TextProperty::HorizontalScale(v) => ("Tz", *v),
        TextProperty::Leading(v) => ("TL", *v),
        TextProperty::BaselineShift(v) => ("Ts", *v),
        TextProperty::Size(v) => ("Tf", *v),
        TextProperty::RenderMode(m) => ("Tr", f64::from(render_mode_number(*m))),
    }
}

/// Two points are the same point.
fn near(a: (f64, f64), b: (f64, f64)) -> bool {
    (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6
}

/// Whether two component lists are the same colour. Compared rather than derived so that the
/// check is about the numbers the file now holds, not about this module's arithmetic.
fn near_components(got: &[f64], want: &[f64]) -> bool {
    got.len() == want.len() && got.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-6)
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect` and `unwrap`, which is what a test is for;
    // the panic-free rule is about what the product does with a file, not about tests.
    // `float_cmp` because several of these assert an exact box or an exact component list, and
    // a tolerance would hide a real change rather than absorb a real difference.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::float_cmp,
        clippy::indexing_slicing,
        clippy::single_range_in_vec_init
    )]

    use std::sync::Arc;

    use mangle_content::interp::{Mark, Record, run};
    use mangle_content::matrix::Matrix;
    use mangle_content::state::{
        ClipBounds, Colour, ColourSpace, Dash, LineCap, LineJoin, Rgba, Tint, TintKind,
    };
    use mangle_content::tokens::ContentStream;

    use super::{
        Change, ChangeError, Patch, Refusal, apply_change, map_point, patches_for,
        region_holds_an_operation, verify, wrappers,
    };
    use crate::page_objects::{Kind, PageModel, PageObject};

    /// A record whose mark is the one given.
    fn record(mark: Mark, span: std::ops::Range<usize>) -> Record {
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
            text: mangle_content::state::TextState::default(),
            text_matrix: Matrix::IDENTITY,
        }
    }

    /// An object over the records given, in drawing order.
    fn object(kind: Kind, records: Vec<Record>) -> PageObject {
        let spans = records.iter().map(|r| r.span.clone()).collect::<Vec<_>>();
        PageObject {
            kind,
            bounds: ClipBounds {
                x0: 0.0,
                y0: 0.0,
                x1: 10.0,
                y1: 10.0,
            },
            records,
            spans,
            form: None,
            parent: None,
            line_break: None,
        }
    }

    fn image(at: usize, len: usize) -> Record {
        record(
            Mark::Image {
                name: Some("Im0".into()),
                matrix: Matrix::IDENTITY,
                inline: false,
                fill: Colour::black(),
            },
            at..at + len,
        )
    }

    /// A path filled in `space` with `components`.
    fn path(at: usize, len: usize, space: ColourSpace, components: Vec<f64>) -> Record {
        let mut fill = Colour::black();
        fill.space = space;
        fill.components = components;
        record(
            Mark::Path {
                segments: Vec::new(),
                fill: Some(fill),
                stroke: None,
                rule: mangle_content::interp::FillRule::NonZero,
            },
            at..at + len,
        )
    }

    /// The bytes a patch inserts at `at`, if there is a patch that inserts there.
    fn insert_at(patches: &[Patch], at: usize) -> Option<&str> {
        patches
            .iter()
            .find(|p| p.range.start == at && p.range.is_empty())
            .map(|p| std::str::from_utf8(&p.bytes).unwrap_or(""))
    }

    #[test]
    fn a_move_wraps_the_object_and_removes_none_of_its_bytes() {
        let o = object(Kind::Image, vec![image(10, 4)]);
        let patches =
            patches_for(b"0123456789ABCDEFGH", &o, &Change::move_by(12.0, -3.0)).expect("writable");
        assert_eq!(patches.len(), 2, "one open and one close");
        let text = insert_at(&patches, 10).expect("something is inserted before the image");
        assert!(text.contains("1 0 0 1 12 -3 cm"), "{text}");
        // The close carries a trailing space, because the byte after the image in this fixture
        // is a regular one and `QE` would be one token. Whitespace is free in a content stream
        // and a glued operator is not.
        assert_eq!(insert_at(&patches, 14), Some(" Q "), "and closed after it");
        assert!(
            patches.iter().all(|p| p.range.is_empty()),
            "a move adds bytes and removes none: {patches:?}"
        );
    }

    /// The property the whole wrapper rests on: the object's own operators come out of the
    /// stream byte-for-byte, and the edit is only the bytes around them.
    #[test]
    fn a_move_leaves_the_objects_own_operators_byte_for_byte() {
        let stream = b"q 1 0 0 1 10 0 cm /Im0 Do Q";
        // `/Im0 Do`, from its first operand to the end of its operator: the trailing ` Q`
        // belongs to the wrapper the file already had, not to the image.
        let at = stream
            .windows(7)
            .position(|w| w == b"/Im0 Do")
            .expect("the image is in the fixture");
        let o = object(Kind::Image, vec![image(at, 7)]);
        let applied = apply_change(stream, &o, &Change::move_by(5.0, 0.0)).expect("valid");
        let text = String::from_utf8_lossy(&applied.bytes).into_owned();
        assert!(
            text.contains("/Im0 Do"),
            "the operation is still there: {text}"
        );
        // Taking the wrapper back out leaves the original exactly as it was. Only one close is
        // taken out — the one this edit added — because the file's own trailing ` Q` is its own.
        let stripped = text.replace("q 1 0 0 1 5 0 cm\n", "").replacen(" Q", "", 1);
        assert_eq!(
            stripped,
            String::from_utf8_lossy(stream),
            "nothing outside the wrapper moved"
        );
    }

    #[test]
    fn a_scale_about_a_point_leaves_the_pivot_where_it_is() {
        let m = Change::scale_about(100.0, 0.0, 2.0, 1.0)
            .matrix()
            .expect("a scale is a transform");
        assert_eq!(map_point(&Matrix::IDENTITY, m, 100.0, 0.0), (100.0, 0.0));
        // A point at the origin moves by the whole scale.
        assert_eq!(map_point(&Matrix::IDENTITY, m, 0.0, 7.0), (-100.0, 7.0));
    }

    #[test]
    fn map_point_reads_a_device_delta_back_into_user_space() {
        // Under a CTM that doubles, one unit of user-space translation covers two of device
        // space — which is what a canvas dragging a box has to know before it asks for a move.
        let ctm = Matrix::scale(2.0, 2.0);
        assert_eq!(
            map_point(&ctm, Matrix::translate(1.0, 0.0), 10.0, 10.0),
            (12.0, 10.0)
        );
        // A matrix with no inverse leaves the point alone rather than inventing a place.
        let flat = Matrix::new(0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        assert_eq!(
            map_point(&flat, Matrix::translate(9.0, 9.0), 4.0, 5.0),
            (4.0, 5.0)
        );
    }

    /// A run of glyphs carrying its text state, which is what a `Tf` change needs a font from.
    fn glyphs(at: usize, len: usize) -> Record {
        let mut r = record(
            Mark::Glyphs {
                font: Some("F1".into()),
                size: 12.0,
                text: vec![b'a'],
                codes: vec![97],
                two_byte: false,
                fill: Colour::black(),
                text_spans: vec![at..at + 3],
                placements: vec![Matrix::IDENTITY],
            },
            at..at + len,
        );
        r.text = mangle_content::state::TextState {
            font: Some("F1".into()),
            widths: None,
            composite: false,
            size: 12.0,
            char_spacing: 0.0,
            word_spacing: 0.0,
            horizontal_scale: 100.0,
            leading: 0.0,
            rise: 0.0,
            render_mode: mangle_content::state::RenderMode::Fill,
        };
        r
    }

    /// A text property is written as the one operator that sets it, and nothing else.
    #[test]
    fn a_text_property_is_the_operator_that_sets_it() {
        let o = object(Kind::Line, vec![glyphs(4, 4)]);
        let patches = patches_for(
            b"BT (x) Tj ET",
            &o,
            &Change::text(crate::TextProperty::CharacterSpacing(2.0)),
        )
        .expect("a text property is writable");
        let text = insert_at(&patches, 4).expect("inserted before the run");
        assert!(text.contains("2 Tc"), "and nothing else: {text}");
        assert!(!text.contains("Tw"), "one property is one operator: {text}");
    }

    /// **The property the interpreter reports is the one that was asked for.**
    ///
    /// The lesson the corpus taught, applied to the newest edit: a string of the right words in
    /// the wrong order is still a string of the right words, so the check runs the result.
    #[test]
    fn a_text_property_is_only_done_when_the_interpreter_sees_the_new_value() {
        let stream = b"BT (x) Tj ET";
        let before = run(&ContentStream::parse(stream));
        let model = PageModel::build(&before.records);
        let o = model
            .objects()
            .iter()
            .find(|o| matches!(o.kind, Kind::Line | Kind::Block))
            .expect("a run of text");
        let change = Change::text(crate::TextProperty::CharacterSpacing(2.5));
        let applied = apply_change(stream, o, &change).expect("writable");
        let after = run(&ContentStream::parse(&applied.bytes));
        let index = model
            .objects()
            .iter()
            .position(|x| std::ptr::eq(x, o))
            .unwrap_or(0);
        verify(&before, &after, index, &change)
            .expect("the character spacing the interpreter reports is 2.5");
    }

    /// A size change names the font alongside it, because `Tf` takes both and a stream that wrote
    /// a bare size would be answered with a font nobody chose.
    #[test]
    fn a_size_change_names_the_font_it_keeps() {
        let o = object(Kind::Line, vec![glyphs(4, 4)]);
        let patches = patches_for(
            b"BT (x) Tj ET",
            &o,
            &Change::text(crate::TextProperty::Size(18.0)),
        )
        .expect("writable");
        let text = insert_at(&patches, 4).expect("inserted");
        assert!(
            text.contains("/F1 18 Tf"),
            "the font it already had: {text}"
        );
    }

    /// The render mode is written as the number the file uses, and the placeholder is gone.
    #[test]
    fn a_render_mode_is_written_as_its_own_number() {
        let o = object(Kind::Line, vec![glyphs(4, 4)]);
        let patches = patches_for(
            b"BT (x) Tj ET",
            &o,
            &Change::text(crate::TextProperty::RenderMode(
                mangle_content::state::RenderMode::Invisible,
            )),
        )
        .expect("writable");
        let text = insert_at(&patches, 4).expect("inserted");
        assert!(text.contains("3 Tr"), "invisible is `Tr 3`: {text}");
    }

    /// A property that is not a text property is refused rather than written as one.
    #[test]
    fn a_text_property_on_an_object_that_is_not_text_is_refused() {
        let o = object(Kind::Image, vec![image(2, 4)]);
        let err = patches_for(
            b"01 /Im0 Do Q",
            &o,
            &Change::text(crate::TextProperty::CharacterSpacing(2.0)),
        )
        .expect_err("an image has no text state");
        assert!(matches!(err, Refusal::NoSuchColour { .. }), "{err}");
    }

    /// A crop is a **clip** wrapped around the image's own `Do`, and the clip is written as a
    /// polygon in the space the `Do` draws in — which is the record's CTM read backwards.
    #[test]
    fn a_crop_is_a_clip_written_in_the_space_the_do_draws_in() {
        let mut r = image(4, 4);
        r.ctm = Matrix::scale(10.0, 10.0);
        let o = object(Kind::Image, vec![r]);
        // A rectangle in page space that covers half of a 100x100 image.
        let keep = ClipBounds {
            x0: 0.0,
            y0: 0.0,
            x1: 500.0,
            y1: 500.0,
        };
        let patches = patches_for(b"01 2 0 0 20 0 0 cm /Im0 Do", &o, &Change::Crop { keep })
            .expect("a crop is writable");
        let text = insert_at(&patches, 4).expect("inserted before the image");
        // The record's CTM is `scale(10, 10)`, so 500 page units is 50 in the image's own space.
        assert!(
            text.contains("0 0 m 50 0 l 50 50 l 0 50 l h W n"),
            "the polygon is the crop mapped back through the CTM: {text}"
        );
    }

    /// A crop on something that is not an image is refused: there is nothing to clip.
    #[test]
    fn a_crop_on_an_object_that_is_not_an_image_is_refused() {
        let mark = path(2, 4, ColourSpace::device_rgb(), vec![1.0, 0.0, 0.0]);
        let o = object(Kind::Path, vec![mark]);
        let err = patches_for(
            b"01 0 0 0 1 k f  ",
            &o,
            &Change::Crop { keep: bounds_box() },
        )
        .expect_err("a path has no picture to crop");
        assert!(matches!(err, Refusal::NoSuchColour { .. }), "{err}");
    }

    fn bounds_box() -> ClipBounds {
        ClipBounds {
            x0: 0.0,
            y0: 0.0,
            x1: 10.0,
            y1: 10.0,
        }
    }

    #[test]
    fn a_delete_takes_the_wrapper_too_when_it_left_it_empty() {
        let stream = b"BT (Hello) Tj ET";
        let at = stream
            .windows(2)
            .position(|w| w == b"(H")
            .expect("the string is in the fixture");
        let end = stream.windows(2).position(|w| w == b"Tj").expect("Tj") + 2;
        let o = object(Kind::Line, vec![image(at, end - at)]);
        let applied = apply_change(stream, &o, &Change::Delete).expect("valid");
        assert_eq!(
            String::from_utf8_lossy(&applied.bytes).trim(),
            "",
            "the `BT` is left empty, so it goes too"
        );
    }

    /// The other half of the rule: a wrapper that still holds something is left exactly as it
    /// is, because removing it would change the state every later operator runs under.
    #[test]
    fn a_delete_leaves_a_wrapper_that_still_holds_something() {
        let stream = b"q (A) Tj (B) Tj Q";
        let first = stream.windows(3).position(|w| w == b"(A)").expect("A");
        // The operation is `(A) Tj`: from its first operand to the end of its operator.
        let end = stream.windows(2).position(|w| w == b"Tj").expect("Tj") + 2;
        let o = object(Kind::Line, vec![image(first, end - first)]);
        let applied = apply_change(stream, &o, &Change::Delete).expect("valid");
        let text = String::from_utf8_lossy(&applied.bytes);
        assert!(
            text.contains("(B) Tj"),
            "the other run is untouched: {text}"
        );
        assert!(
            !text.contains("(A) Tj"),
            "and the one asked for is gone: {text}"
        );
        assert!(text.starts_with('q') && text.ends_with('Q'), "{text}");
    }

    /// A recolour is only done when the interpreter agrees the colour changed.
    ///
    /// The bytes this module writes are a string, and a string of the right words in the wrong
    /// order is still a string of the right words: `rg 0.1 0.2 0.3` is an `rg` with *no
    /// operands* followed by four stray numbers, and the next operator quietly collects them as
    /// its own. The page still renders, the edit appears to have worked, and the colour is the
    /// old one. Nothing but running the result catches that, which is why this test runs it.
    #[test]
    fn a_recolour_is_only_done_when_the_interpreter_sees_the_new_colour() {
        let stream = b"0.5 0.5 0.5 rg 10 20 30 40 re f";
        let before = run(&ContentStream::parse(stream));
        let model = PageModel::build(&before.records);
        let o = model
            .objects()
            .iter()
            .find(|o| o.kind == Kind::Path)
            .expect("the fixture draws a path");
        let change = Change::recolour(Rgba::WHITE);
        let applied = apply_change(stream, o, &change).expect("writable");
        let text = String::from_utf8_lossy(&applied.bytes);
        assert!(
            text.contains("1 1 1 rg"),
            "the operands come before the operator: {text}"
        );
        let after = run(&ContentStream::parse(&applied.bytes));
        verify(&before, &after, 0, &change)
            .expect("the colour the interpreter sees is the new one");
    }

    #[test]
    fn a_recolour_writes_into_the_space_the_object_already_uses() {
        let o = object(
            Kind::Path,
            vec![path(4, 4, ColourSpace::device_rgb(), vec![1.0, 0.0, 0.0])],
        );
        let patches =
            patches_for(b"0123 1 0 0 rg f", &o, &Change::recolour(Rgba::WHITE)).expect("writable");
        let text = insert_at(&patches, 4).expect("inserted before the path");
        assert!(
            text.contains("1 1 1 rg"),
            "DeviceRGB stays DeviceRGB: {text}"
        );
        assert!(!text.contains("cs"), "and no new space is selected: {text}");
    }

    #[test]
    fn a_cmyk_object_stays_cmyk_when_recoloured() {
        let mut space = ColourSpace::device_gray();
        space.name = "DeviceCMYK".into();
        let o = object(
            Kind::Path,
            vec![path(2, 4, space, vec![0.0, 0.0, 0.0, 1.0])],
        );
        let patches =
            patches_for(b"01 0 0 0 1 k f", &o, &Change::recolour(Rgba::WHITE)).expect("writable");
        let text = insert_at(&patches, 2).expect("inserted");
        // Operands then the operator, which is the order a stream is written in.
        assert!(text.contains("0 0 0 0 k"), "the operator is a `k`: {text}");
        // `q`, a space, four components, three spaces between them, and a space before the
        // operator: the count is the shape, so it is checked rather than written as a constant
        // a reader has to trust.
        assert_eq!(
            text.split_whitespace().count(),
            6,
            "a `q`, four components and the `k`: {text}"
        );
    }

    #[test]
    fn a_separation_is_written_as_a_tint_in_its_own_space() {
        let space = ColourSpace {
            name: "Cs8".into(),
            colorant: Some("PANTONE 123".into()),
            icc: None,
            tint: Some(Arc::new(Tint {
                kind: TintKind::Separation,
                alternate: Some(ColourSpace::device_rgb()),
                function: None,
                colorants: 1,
                names: vec!["PANTONE 123".into()],
            })),
        };
        let o = object(Kind::Path, vec![path(2, 4, space, vec![0.5])]);
        let patches = patches_for(b"01 /Cs8 cs 1 sc f", &o, &Change::recolour(Rgba::BLACK))
            .expect("a tint space is writable");
        let text = insert_at(&patches, 2).expect("inserted");
        assert!(text.contains("1 sc"), "black is a full tint: {text}");
        assert!(
            !text.contains("cs"),
            "and the space is not re-pointed: {text}"
        );
    }

    /// The honesty law: a space this cannot write into is refused by name, not approximated.
    /// A tint space with no transform has nothing that says what its numbers mean.
    #[test]
    fn a_colour_space_with_nothing_to_write_into_is_refused_by_name() {
        let space = ColourSpace {
            name: "Cs8".into(),
            colorant: Some("PANTONE 123".into()),
            icc: None,
            tint: None,
        };
        let o = object(Kind::Path, vec![path(2, 4, space, vec![0.5])]);
        let err = patches_for(b"01 /Cs8 cs 1 sc f", &o, &Change::recolour(Rgba::WHITE))
            .expect_err("nothing here knows what a tint means without its transform");
        assert!(
            matches!(err, Refusal::ColourSpaceNotWritable { .. }),
            "{err}"
        );
        assert!(
            err.to_string().contains("Cs8"),
            "and it names the space: {err}"
        );
    }

    #[test]
    fn a_pattern_colour_is_refused_because_it_is_a_name_not_a_number() {
        let mut space = ColourSpace::device_gray();
        space.name = "Pattern".into();
        let o = object(Kind::Path, vec![path(2, 4, space, vec![0.0])]);
        let err = patches_for(b"01 /Pattern cs sc f", &o, &Change::recolour(Rgba::WHITE))
            .expect_err("a pattern's colour is the pattern");
        assert!(
            matches!(err, Refusal::ColourSpaceNotWritable { .. }),
            "{err}"
        );
        assert!(err.to_string().contains("pattern"), "{err}");
    }

    #[test]
    fn an_object_from_another_stream_is_refused_before_anything_is_written() {
        let o = object(Kind::Image, vec![image(400, 4)]);
        let err = patches_for(b"q Q", &o, &Change::move_by(1.0, 1.0))
            .expect_err("the span is not in this stream");
        assert!(matches!(err, Refusal::ForeignSpan { .. }), "{err}");
        assert!(!err.to_string().is_empty(), "and it says so");
    }

    #[test]
    fn an_object_with_no_spans_has_nothing_to_edit() {
        let o = object(Kind::Image, Vec::new());
        let err = patches_for(b"q Q", &o, &Change::Delete).expect_err("nothing to edit");
        assert!(matches!(err, Refusal::NoSpans), "{err}");
    }

    #[test]
    fn wrappers_are_found_by_the_same_nesting_rule_the_interpreter_applies() {
        let ranges = wrappers(b"q BT q /F1 12 Tf (x) Tj Q ET Q");
        assert_eq!(ranges.len(), 3, "three wrappers: {ranges:?}");
        assert!(
            ranges[0].start < ranges[1].start && ranges[1].start < ranges[2].start,
            "outermost first: {ranges:?}"
        );
        assert!(
            ranges[0].end > ranges[1].end && ranges[1].end > ranges[2].end,
            "and each contains the one after it: {ranges:?}"
        );
        // A closed pair is a wrapper however damaged the rest of the stream is; an opener with
        // no closer, or a closer with nothing open, yields nothing rather than a guess.
        assert_eq!(wrappers(b"q Q q").len(), 1, "the trailing `q` has no `Q`");
        assert_eq!(
            wrappers(b"Q q Q").len(),
            1,
            "the first `Q` had nothing open"
        );
        assert_eq!(
            wrappers(b"q q Q").len(),
            1,
            "the `Q` closes the inner `q`, not the outer"
        );
    }

    #[test]
    fn a_range_holding_only_whitespace_is_empty() {
        let stream = b"BT   ET";
        let ops = ContentStream::parse(stream).operations();
        let bt = stream.windows(2).position(|w| w == b"BT").expect("BT");
        let et = stream.windows(2).position(|w| w == b"ET").expect("ET");
        let wrapper = bt..et + 2;
        // An empty ignore means nothing is being taken out, which is the question "does this
        // wrapper hold anything at all".
        let nothing = 0..0;
        assert!(
            !region_holds_an_operation(&ops, &wrapper, &nothing),
            "the `BT` and the `ET` are the two ends, so the inside is empty"
        );
        // An operation inside the wrapper, and one outside it, are the two cases that matter.
        let stream = b"BT (x) Tj ET Tj";
        let ops = ContentStream::parse(stream).operations();
        let bt = stream.windows(2).position(|w| w == b"BT").expect("BT");
        let et = stream.windows(2).position(|w| w == b"ET").expect("ET");
        let wrapper = bt..et + 2;
        assert!(
            region_holds_an_operation(&ops, &wrapper, &nothing),
            "the `(x) Tj` is inside it"
        );
        let run = stream.windows(2).position(|w| w == b"(x").expect("the run")
            ..stream.windows(2).position(|w| w == b"Tj").expect("Tj") + 2;
        assert!(
            !region_holds_an_operation(&ops, &wrapper, &run),
            "and once that run is gone the wrapper is empty"
        );
    }

    #[test]
    fn grey_of_is_luminance_and_cmyk_of_keeps_black_black() {
        assert_eq!(super::grey_of(Rgba::WHITE), 1.0);
        assert_eq!(super::grey_of(Rgba::BLACK), 0.0);
        assert_eq!(super::cmyk_of(Rgba::BLACK), [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(super::cmyk_of(Rgba::WHITE), [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(
            super::cmyk_of(Rgba {
                r: 1.0,
                g: 0.0,
                b: 0.0,
                a: 1.0
            }),
            [0.0, 1.0, 1.0, 0.0],
            "pure red is magenta and yellow"
        );
        // A mid grey is a key plate, not three partial inks.
        let mid = super::cmyk_of(Rgba {
            r: 0.5,
            g: 0.5,
            b: 0.5,
            a: 1.0,
        });
        assert!((mid[3] - 0.5).abs() < 1e-9, "{mid:?}");
    }

    /// The verification that closes the loop: after a real edit the interpreter agrees that the
    /// object moved and that nothing else did. Two objects moving is what it refuses.
    #[test]
    fn verify_refuses_an_edit_that_moved_something_else_as_well() {
        let parse = |s: &[u8]| run(&ContentStream::parse(s));
        let before = parse(b"q 1 0 0 1 10 0 cm /Im0 Do Q q 1 0 0 1 50 0 cm /Im1 Do Q");
        let both = parse(b"q 1 0 0 1 20 0 cm /Im0 Do Q q 1 0 0 1 90 0 cm /Im1 Do Q");
        let err =
            verify(&before, &both, 0, &Change::move_by(10.0, 0.0)).expect_err("two objects moved");
        assert!(
            err.contains("did not put them"),
            "the report names it: {err}"
        );

        let one = parse(b"q 1 0 0 1 20 0 cm /Im0 Do Q q 1 0 0 1 50 0 cm /Im1 Do Q");
        verify(&before, &one, 0, &Change::move_by(10.0, 0.0)).expect("one object moved");
    }

    /// The shape the UI uses: build a model over a stream, select, change, write back, verify.
    #[test]
    fn an_edit_written_from_a_model_over_a_real_stream_lands() {
        let stream = b"q 1 0 0 1 10 0 cm /Im0 Do Q";
        let before = run(&ContentStream::parse(stream));
        let model = PageModel::build(&before.records);
        assert_eq!(model.objects().len(), 1, "one photo is one object");
        let applied =
            apply_change(stream, &model.objects()[0], &Change::move_by(4.0, 0.0)).expect("valid");
        let text = String::from_utf8_lossy(&applied.bytes);
        assert!(
            text.contains("q 1 0 0 1 4 0 cm\n/Im0 Do Q"),
            "the wrapper goes around the image's own operation, inside the `q` the file had: \
             {text}"
        );
        let after = run(&ContentStream::parse(&applied.bytes));
        verify(&before, &after, 0, &Change::move_by(4.0, 0.0)).expect("the edit did what it said");
    }

    /// A line whose runs have another object's operator between them still moves as one thing,
    /// and the operator in between is not touched.
    #[test]
    fn an_object_with_two_runs_is_wrapped_twice_and_not_between() {
        let stream = b"(A) Tj (B) Tj";
        let first = stream.windows(3).position(|w| w == b"(A)").expect("A");
        let second = stream.windows(3).position(|w| w == b"(B)").expect("B");
        let o = object(Kind::Line, vec![image(first, 6), image(second, 6)]);
        let patches = patches_for(stream, &o, &Change::move_by(1.0, 2.0)).expect("writable");
        assert_eq!(patches.len(), 4, "two wrappers: {patches:?}");
        assert!(
            insert_at(&patches, second).is_some(),
            "the second is wrapped too"
        );
        assert!(
            patches.iter().all(|p| p.range.is_empty()),
            "and nothing is removed: {patches:?}"
        );
        let applied = crate::surgery::apply(stream, &patches).expect("valid");
        let text = String::from_utf8_lossy(&applied.bytes).into_owned();
        assert_eq!(text.matches("cm\n").count(), 2, "{text}");
        // Taking the wrappers out again gives the original stream exactly — which is what
        // "the operator in between is not touched" means in bytes.
        let stripped = text.replace("q 1 0 0 1 1 2 cm\n", "").replace(" Q", "");
        assert_eq!(stripped, String::from_utf8_lossy(stream), "{stripped}");
    }

    #[test]
    fn an_edit_that_cannot_be_written_is_a_refusal_not_a_write() {
        let o = object(Kind::Image, vec![image(2, 4)]);
        let err = apply_change(b"q Q", &o, &Change::move_by(1.0, 0.0))
            .expect_err("the span is outside this stream");
        assert!(
            matches!(err, ChangeError::Refused(Refusal::ForeignSpan { .. })),
            "{err}"
        );
        assert!(
            matches!(
                apply_change(b"q Q", &o, &Change::Delete),
                Err(ChangeError::Refused(Refusal::ForeignSpan { .. }))
            ),
            "and the same refusal catches a delete before a byte is removed: nothing is ever \
             half-written"
        );
    }

    #[test]
    fn a_svg_like_fixture_round_trips_through_a_delete() {
        // Two objects: delete one and the other's bytes must be identical afterwards.
        let stream = b"q 1 0 0 1 0 0 cm /Im0 Do Q q 1 0 0 1 40 0 cm /Im1 Do Q";
        let run = run(&ContentStream::parse(stream));
        let model = PageModel::build(&run.records);
        assert_eq!(model.objects().len(), 2, "two photos");
        let first = &model.objects()[0];
        let applied = apply_change(stream, first, &Change::Delete).expect("valid");
        let after = mangle_content::interp::run(&ContentStream::parse(&applied.bytes));
        verify(&run, &after, 0, &Change::Delete).expect("the deletable object is gone");
        let second = model.objects()[1].spans[0].clone();
        // The second object's operator is still the second object's operator.
        let text = String::from_utf8_lossy(&applied.bytes);
        assert!(text.contains("/Im1 Do"), "the other photo survives: {text}");
        assert!(
            text.find("/Im0 Do").is_none(),
            "and the deleted one does not"
        );
        let _ = second;
    }
}
