//! Running a content stream against a graphics state, and recording what it drew.
//!
//! Two things come out of this, and both are needed. The *state* tells a renderer what
//! colour, width and transformation each mark was drawn with. The *records* tell the
//! Inspector and a selection where each mark is, in both bytes and page space, which is
//! the only reason an edit can be surgical.
//!
//! Running the stream is not optional bookkeeping: the state after a page has been
//! executed is part of the document (a `/gs` in the next page, a form XObject's
//! inherited state), and the only way to know it is to run the stream.

use std::ops::Range;
use std::sync::Arc;

use mangle_syntax::object::{Dict, Object, Stream};
use mangle_syntax::stream::decode_stream;

use crate::matrix::Matrix;
use crate::ops;
use crate::state::{
    Clip, ClipBounds, ClipPath, Colour, ColourSpace, Dash, GraphicsState, LineCap, LineJoin,
    PathSegment, StateStack,
};
use crate::tokens::{ContentStream, ContentToken, Operation};

/// One thing the page drew.
#[derive(Debug, Clone, PartialEq)]
pub enum Mark {
    /// A path painted with the current fill and stroke.
    Path {
        /// The path in device space, as it will be drawn.
        segments: Vec<PathSegment>,
        fill: Option<Colour>,
        stroke: Option<Colour>,
        /// `EvenOdd` or `NonZero`.
        rule: FillRule,
    },
    /// A run of glyphs, with the matrix they were placed by.
    Glyphs {
        /// The font as it was named.
        font: Option<String>,
        size: f64,
        /// The raw string, before any encoding is applied.
        text: Vec<u8>,
        /// The character codes, one per glyph, as the font's encoding names them.
        ///
        /// This is what a renderer looks a glyph up by, and it is a separate list from
        /// `text` because a composite font's codes are two bytes each: four bytes of string
        /// are two glyphs. Carrying the codes means no consumer has to know the font's
        /// encoding to pair a glyph with its code, which is the only way to be right about
        /// it — a mark that said only how many bytes it had would leave every consumer to
        /// guess, and guessing one byte per code is the bug.
        ///
        /// `/ToUnicode` is a separate concern from drawing and is not read here. It is what
        /// text extraction will need, and this mark now carries what it will need to use it:
        /// the codes as the font's encoding names them, the font they were shown in, and the
        /// span of the bytes they came from.
        codes: Vec<u32>,
        /// Whether each code in `codes` is two bytes wide.
        ///
        /// The font dictionary's own claim, recorded rather than re-derived. A consumer
        /// needs it to know which `cmap` to read the code through: a composite font's codes
        /// are looked up in the (3,0) subtable, a simple font's through the subtables a
        /// simple font uses.
        two_byte: bool,
        /// The fill colour the text was shown in, which is the non-stroking colour that
        /// was current rather than a property of the glyphs.
        ///
        /// A path carries its own colours because `re`/`f` names them. Text does not: `Tj`
        /// takes no colour and paints in whatever `g`/`rg`/`k` last set, so without this a
        /// renderer would have to guess, and black is a guess that is wrong on every page
        /// with coloured text on it.
        fill: Colour,
        /// The bytes of each string operand, in order.
        ///
        /// Separate from the mark's own span, which runs from the first operand to the
        /// operator: to change the text, these are the bytes to rewrite, and the `Tj`
        /// and its operands are not among them. A `TJ` has several, which is why this is
        /// a list.
        text_spans: Vec<Range<usize>>,
        /// The matrix each glyph is placed by, one per glyph.
        ///
        /// Each is the full text rendering matrix — `ctm × text_matrix × [Tfs·Th 0 0 Tfs 0
        /// Ts]` — so a glyph's outline, which is in ems, is drawn through this and
        /// through nothing else but the page placement.
        placements: Vec<Matrix>,
    },
    /// An image placed by `Do` or an inline image.
    Image {
        /// The name, for an XObject; `None` for an inline image.
        name: Option<String>,
        /// The placement matrix.
        matrix: Matrix,
        /// True when the image was inline, so the bytes are in the stream.
        inline: bool,
        /// The fill colour current when the image was drawn, which is what paints an
        /// `/ImageMask`'s zero bits.
        ///
        /// It travels with the mark for the same reason a glyph run's does: `Do` names no
        /// colour, so a renderer that painted a mask in anything but this would paint every
        /// mask on every page black and be right only where the page was.
        fill: Colour,
    },
    /// A shading, filled into the current clip.
    Shading { name: String, matrix: Matrix },
    /// A clip was narrowed, or reset. The region is every clipping path in force, each with
    /// the rule that decides its inside, in the page's coordinate space as of the moment the
    /// clip was set, with the CTM then in force already applied. The box travels with it as a
    /// bound, not as the thing that decides visibility.
    ///
    /// A renderer does not need this mark: every mark carries the clip in force when it was
    /// created, and that is the clip it must draw under. The mark says *where* a clip changed,
    /// which is a fact about the page rather than about any one mark.
    ///
    /// `None` means the clip was reset to none. `Some` with no paths is a clip to nothing,
    /// which is a state rather than an error.
    ClipChanged(Option<Clip>),
}

/// How a path's interior is decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FillRule {
    #[default]
    NonZero,
    EvenOdd,
}

/// A mark together with where it came from and what drew it.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub mark: Mark,
    /// The bytes that produced it, from the first operand to the operator.
    pub span: Range<usize>,
    /// The transformation the page had when it was drawn.
    pub ctm: Matrix,
    /// The clip in force: the region — every clipping path, the rule each was set under, and
    /// their box — in the page's coordinate space, as of the moment the clip was set.
    ///
    /// This is on the record rather than in a renderer, and that is what makes a rendering
    /// reproducible: a mark's appearance is fixed by the state it was created in, so a
    /// renderer that installed the clip from here draws every mark the same way whatever it
    /// happened to draw before it. The CTM in force when the clip was set is already applied
    /// to the paths and to the bounds, so a renderer only has to apply the page placement on
    /// top. Applying the mark's CTM again would apply it twice.
    pub clip: Option<Clip>,
    /// The fill and stroke alphas, which a compositing renderer needs and a geometry
    /// one does not.
    pub fill_alpha: f64,
    pub stroke_alpha: f64,
    pub blend_mode: String,
    /// The stroke width, already scaled by the transformation: this is the width the
    /// user will see, not the number in the stream.
    pub device_line_width: f64,
    /// The rest of the stroke parameters, as they were when this mark was drawn.
    ///
    /// Each mark carries its own rather than the reader carrying the current ones,
    /// because a mark's appearance is fixed by the state it was drawn with: a dashed line
    /// that follows a solid one must be dashed, and a renderer reading one style for the
    /// whole page would draw them the same.
    pub line_cap: LineCap,
    pub line_join: LineJoin,
    pub dash: Dash,
    /// The marked-content tag, if the mark was inside a `/BMC` or `/BDC` group.
    pub tag: Option<String>,
    /// The form XObject this mark was drawn inside, if any.
    ///
    /// A form carries its own `/Resources`, and every name in it — `/F1`, `/Im0`, `/Sh0` —
    /// resolves against those rather than against the page's. This says which table, so a
    /// consumer can find it without guessing: two forms may each name `/F1` and mean
    /// different fonts, and a lookup that fell back to the page's table would draw the
    /// wrong glyphs with no way to see that it had.
    ///
    /// `None` is a mark on the page itself, and that is the only thing it means.
    pub form: Option<String>,
}

impl Record {
    /// The bounds of the mark in device space, from its geometry rather than from a
    /// glyph metric, which is all the renderer has not yet computed.
    #[must_use]
    pub fn bounds(&self) -> Option<ClipBounds> {
        bounds_of(&self.mark)
    }
}

/// The bounds of a mark, in device space.
#[must_use]
pub fn bounds_of(mark: &Mark) -> Option<ClipBounds> {
    {
        match mark {
            Mark::Path { segments, .. } => {
                let mut b: Option<ClipBounds> = None;
                for seg in segments {
                    let points: Vec<(f64, f64)> = match *seg {
                        PathSegment::Move(x, y) | PathSegment::Line(x, y) => vec![(x, y)],
                        PathSegment::Curve(x1, y1, x2, y2, x3, y3) => {
                            vec![(x1, y1), (x2, y2), (x3, y3)]
                        }
                        PathSegment::Close => continue,
                    };
                    for (x, y) in points {
                        b = Some(match b {
                            None => ClipBounds {
                                x0: x,
                                y0: y,
                                x1: x,
                                y1: y,
                            },
                            Some(r) => ClipBounds {
                                x0: r.x0.min(x),
                                y0: r.y0.min(y),
                                x1: r.x1.max(x),
                                y1: r.y1.max(y),
                            },
                        });
                    }
                }
                b
            }
            Mark::Glyphs { placements, .. } => {
                let mut b: Option<ClipBounds> = None;
                for m in placements {
                    let (x, y) = m.apply(0.0, 0.0);
                    b = Some(match b {
                        None => ClipBounds {
                            x0: x,
                            y0: y,
                            x1: x,
                            y1: y,
                        },
                        Some(r) => ClipBounds {
                            x0: r.x0.min(x),
                            y0: r.y0.min(y),
                            x1: r.x1.max(x),
                            y1: r.y1.max(y),
                        },
                    });
                }
                b
            }
            Mark::Image { matrix, .. } | Mark::Shading { matrix, .. } => {
                // The unit square, which is what an image's space is until its own
                // `/BBox` is read. A renderer replaces this with the real extent.
                let corners = [
                    matrix.apply(0.0, 0.0),
                    matrix.apply(1.0, 0.0),
                    matrix.apply(1.0, 1.0),
                    matrix.apply(0.0, 1.0),
                ];
                let x0 = corners.iter().map(|c| c.0).fold(f64::INFINITY, f64::min);
                let x1 = corners
                    .iter()
                    .map(|c| c.0)
                    .fold(f64::NEG_INFINITY, f64::max);
                let y0 = corners.iter().map(|c| c.1).fold(f64::INFINITY, f64::min);
                let y1 = corners
                    .iter()
                    .map(|c| c.1)
                    .fold(f64::NEG_INFINITY, f64::max);
                Some(ClipBounds { x0, y0, x1, y1 })
            }
            Mark::ClipChanged(clip) => clip.as_ref().map(|c| c.bounds),
        }
    }
}

/// Everything a content stream produced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageContent {
    /// The marks, in the order they were drawn.
    pub records: Vec<Record>,
    /// The state after the last operation, which a form XObject hands back.
    pub state: GraphicsState,
    /// The operators that were not in the table, which is a finding about the file.
    pub unknown_operators: Vec<(Vec<u8>, Range<usize>)>,
    /// The marked-content groups seen, innermost last.
    pub tags: Vec<String>,
    /// A note per operation that could not be carried out, so nothing is lost silently.
    pub notes: Vec<String>,
}

/// A limit on how many marks one stream may produce. A file can say `S` a million
/// times; the product needs a page, not a memory exhaustion.
pub const MAX_RECORDS: usize = 200_000;

/// The width used for a code the font says nothing about, in the font's own units.
///
/// The specification's default `/MissingWidth` behaviour amounts to half an em, and a
/// stated approximation is better than a guess that is not.
const DEFAULT_WIDTH: u16 = 500;

/// The one character code that word spacing applies to.
const WORD_SPACE: u32 = 32;

/// Run a content stream with no resources.
///
/// Everything works except `gs`, whose dictionaries are named in the page's resources
/// and so cannot be found here. Each such name becomes a note rather than a silent
/// no-op, because a page drawn with the wrong state is a finding the user should see.
#[must_use]
pub fn run(stream: &ContentStream) -> PageContent {
    run_with(stream, &crate::Resources::default())
}

/// Run a content stream against the page's resources.
///
/// The page is itself a nested run: a form XObject's content executes the same way, from a
/// state that has already had the form's `/Matrix` and `/BBox` applied to it and against
/// the form's own resources, so [`run_with_state`] is the entry point and this is the call
/// with the default state.
#[must_use]
pub fn run_with(stream: &ContentStream, resources: &crate::Resources) -> PageContent {
    run_with_state(stream, &GraphicsState::new(), resources)
}

/// Run a content stream from a given state, against a given set of resources.
///
/// A form XObject is executed as though its content were wrapped in `q` … `Q`: it starts in
/// the state its `/Matrix` and `/BBox` have already been composed into, and the state it
/// leaves behind is the one it started in. That is why this takes a state rather than
/// making one, and why the page's own run is the same call with the default.
///
/// Whatever the *lexer* had to leave out is already in the notes before the first operator
/// runs, and comes first: a collection the stream was truncated at is a finding about the
/// file that no amount of executing the rest of it will make smaller.
#[must_use]
pub fn run_with_state(
    stream: &ContentStream,
    state: &GraphicsState,
    resources: &crate::Resources,
) -> PageContent {
    let mut ctx = Context {
        state: state.clone(),
        stack: StateStack::new(),
        out: PageContent {
            notes: stream.notes().to_vec(),
            ..PageContent::default()
        },
        clip_pending: false,
        path_start: None,
        resources,
        depth: 0,
        form: None,
        prior: 0,
    };
    ctx.execute(stream);
    ctx.out.state = ctx.state;
    ctx.out
}

/// The deepest a form XObject may be executed, counting the page itself as depth zero.
///
/// A form that draws itself is a real thing in the wild — a producer that flattened a
/// template into the page and left the template behind does exactly that, and a chain of
/// two forms each naming the other is no rarer — and a reader that follows it without a
/// bound is a stack overflow, which is a crash rather than a note. Twelve is well past any
/// nesting a file *means*: office output routinely nests three deep and a template library
/// six.
pub const MAX_FORM_DEPTH: usize = 12;

struct Context<'a> {
    state: GraphicsState,
    stack: StateStack,
    out: PageContent,
    /// `W` and `W*` do nothing by themselves: they set a flag that the *next* painting
    /// operator reads, because the clip is the path that follows them.
    clip_pending: bool,
    /// Where the current path started, in bytes. A mark's span runs from here to the
    /// painting operator, because those are the bytes that have to change together for
    /// the mark to change at all.
    path_start: Option<usize>,
    /// The resource tables the names in this run resolve against: the page's, or a form's
    /// own while that form is executing.
    resources: &'a crate::Resources,
    /// How many form XObjects deep this run is. The page is zero.
    depth: usize,
    /// The form this run is executing, for the records it produces.
    form: Option<String>,
    /// How many records the enclosing runs had already produced, so that the mark limit is
    /// the page's rather than each run's own.
    prior: usize,
}

impl Context<'_> {
    /// Run the operations of one stream into this context.
    ///
    /// One loop for every run — the page's own and each form's — because a form *is* a
    /// nested content stream and the only things that differ are the state it starts in
    /// and the resources its names resolve against.
    fn execute(&mut self, stream: &ContentStream) {
        for op in stream.operations() {
            self.step(&op);
            // The limit is the page's, not this run's: `prior` is what the enclosing runs
            // had already recorded, so a page that draws one form a thousand times stops
            // in the same place a page with a thousand paths does.
            if self.prior + self.out.records.len() >= MAX_RECORDS {
                self.out.notes.push(format!(
                    "stopped after {MAX_RECORDS} marks; the rest of this page was not executed"
                ));
                break;
            }
        }
    }

    /// Execute one operation.
    fn step(&mut self, op: &Operation) {
        let name = op.operator.operator().unwrap_or_default().to_vec();
        if name.is_empty() {
            if !op.operands.is_empty() {
                self.out.notes.push(format!(
                    "{} operands with no operator at byte {}",
                    op.operands.len(),
                    op.span.start
                ));
            }
            return;
        }
        let Some(info) = ops::lookup(&name) else {
            self.out
                .unknown_operators
                .push((name.clone(), op.span.clone()));
            return;
        };
        // `W` and `W*` do nothing on their own; they set a flag the next paint reads.
        if matches!(name.as_slice(), b"W" | b"W*") {
            self.clip_pending = true;
            return;
        }

        let all: Vec<&Object> = op.operands.iter().map(|t| &t.value).collect();
        let operands = ops::take_last(info, &all);
        // A form is a nested content stream, not something to paint: executing it is what
        // this operator means, and the marks it draws are the page's marks. Every other
        // `Do` falls through to `record_mark`, which is the only thing that knows how to
        // record one.
        if name.as_slice() == b"Do"
            && let Some(named) = operands.first().and_then(|o| o.as_name())
            && self.do_xobject(named, &op.span)
        {
            return;
        }
        // A path begins at the first operator that puts a point down, not at the
        // operator that ends it.
        if self.path_start.is_none()
            && matches!(
                name.as_slice(),
                b"m" | b"l" | b"c" | b"v" | b"y" | b"re" | b"h"
            )
        {
            self.path_start = Some(op.span.start);
        }
        self.apply(name.as_slice(), &operands);

        if info.effect.emits || info.effect.paints {
            // A path painting operator with an empty path draws nothing, and a mark for
            // it would be a selectable thing on the page that is not there.
            let empty_path = info.effect.paints
                && !self.state.has_path()
                && !matches!(name.as_slice(), b"sh" | b"Do" | b"BI");
            if !empty_path {
                self.record_mark(op.clone(), &name, &operands);
            }
        }
        if info.effect.saves {
            self.stack.push(&self.state);
            self.state.depth = self.stack.depth();
        }
        if info.effect.restores {
            if let Some(saved) = self.stack.pop() {
                self.state = saved;
                self.state.depth = self.stack.depth();
            }
        }
    }

    /// Apply the operators that change state.
    fn apply(&mut self, name: &[u8], operands: &[&Object]) {
        let nums: Vec<f64> = operands.iter().filter_map(|o| o.as_f64()).collect();
        let num_at = |i: usize| nums.get(i).copied();
        let int_at = |i: usize| operands.get(i).and_then(|o| o.as_i64());
        let name_at = |i: usize| {
            operands
                .get(i)
                .and_then(|o| o.as_name())
                .map(<[u8]>::to_vec)
        };

        match name {
            // These are all handled by their effect on the state rather than by
            // anything they take: `q` and `Q` by the stack, `W` by the pending flag,
            // the marked-content operators by the tag stack, the compatibility
            // sections by doing nothing at all.
            b"q" | b"Q" | b"n" | b"W" | b"W*" | b"BX" | b"EX" | b"MP" | b"DP" | b"BMC" | b"BDC"
            | b"EMC" | b"OC" | b"ID" | b"EI" | b"BI" | b"gx" | b"hs" => {}
            b"cm" => {
                if let Some(m) = Matrix::from_slice(&nums) {
                    self.state.concat(m);
                }
            }
            b"w" => {
                if let Some(v) = num_at(0) {
                    self.state.stroke.width = v;
                }
            }
            b"J" => {
                if let Some(v) = int_at(0) {
                    self.state.stroke.cap = LineCap::from_int(v);
                }
            }
            b"j" => {
                if let Some(v) = int_at(0) {
                    self.state.stroke.join = LineJoin::from_int(v);
                }
            }
            b"M" => {
                if let Some(v) = num_at(0) {
                    self.state.stroke.miter_limit = v;
                }
            }
            b"i" => {
                if let Some(v) = num_at(0) {
                    self.state.stroke.intent = Some(intent_name(v as i64));
                }
            }
            b"d" => {
                let array = operands
                    .first()
                    .and_then(|o| o.as_array())
                    .map(|a| a.iter().filter_map(Object::as_f64).collect())
                    .unwrap_or_default();
                let phase = operands.get(1).and_then(|o| o.as_f64()).unwrap_or(0.0);
                self.state.stroke.dash = Dash { array, phase };
            }
            b"ri" => {
                self.state.stroke.intent = Some(intent_name(int_at(0).unwrap_or(0)));
            }
            b"gs" => {
                if let Some(n) = name_at(0) {
                    self.apply_extgstate(&n);
                }
            }
            b"CS" => self.set_colour_space(name_at(0), true),
            b"cs" => self.set_colour_space(name_at(0), false),
            b"SC" | b"SCN" => self.set_colour(&nums, name_at(0), true),
            b"sc" | b"scn" => self.set_colour(&nums, name_at(0), false),
            b"G" | b"g" => {
                if let Some(v) = num_at(0) {
                    let mut c = self.state.stroking.clone();
                    c.set(ColourSpace::device_gray(), &[v]);
                    if name == b"g" {
                        self.state.fill = c;
                    } else {
                        self.state.stroking = c;
                    }
                }
            }
            b"RG" | b"rg" => {
                if nums.len() >= 3 {
                    let mut c = self.state.stroking.clone();
                    c.set(ColourSpace::device_rgb(), &nums);
                    if name == b"rg" {
                        self.state.fill = c;
                    } else {
                        self.state.stroking = c;
                    }
                }
            }
            b"K" | b"k" => {
                if nums.len() >= 4 {
                    let mut c = self.state.stroking.clone();
                    c.set(
                        ColourSpace {
                            name: "DeviceCMYK".into(),
                            colorant: None,
                            icc: None,
                            tint: None,
                        },
                        &nums,
                    );
                    if name == b"k" {
                        self.state.fill = c;
                    } else {
                        self.state.stroking = c;
                    }
                }
            }
            // Path construction.
            b"m" => self
                .state
                .move_to(num_at(0).unwrap_or(0.0), num_at(1).unwrap_or(0.0)),
            b"l" => self
                .state
                .line_to(num_at(0).unwrap_or(0.0), num_at(1).unwrap_or(0.0)),
            b"c" => self.state.curve_to(
                num_at(0).unwrap_or(0.0),
                num_at(1).unwrap_or(0.0),
                num_at(2).unwrap_or(0.0),
                num_at(3).unwrap_or(0.0),
                num_at(4).unwrap_or(0.0),
                num_at(5).unwrap_or(0.0),
            ),
            b"v" => {
                let (x, y) = self.state.current_point.unwrap_or((0.0, 0.0));
                self.state.curve_to(
                    x,
                    y,
                    num_at(0).unwrap_or(0.0),
                    num_at(1).unwrap_or(0.0),
                    num_at(2).unwrap_or(0.0),
                    num_at(3).unwrap_or(0.0),
                );
            }
            // `y` is `c` with the first control point implied to be the current
            // point, so it takes two pairs: x2 y2 x3 y3.
            b"y" => {
                let (x, y) = self.state.current_point.unwrap_or((0.0, 0.0));
                self.state.curve_to(
                    x,
                    y,
                    num_at(0).unwrap_or(0.0),
                    num_at(1).unwrap_or(0.0),
                    num_at(2).unwrap_or(0.0),
                    num_at(3).unwrap_or(0.0),
                );
            }
            b"h" => self.state.close_path(),
            b"re" => {
                let (x, y, w, h) = (
                    num_at(0).unwrap_or(0.0),
                    num_at(1).unwrap_or(0.0),
                    num_at(2).unwrap_or(0.0),
                    num_at(3).unwrap_or(0.0),
                );
                self.state.move_to(x, y);
                self.state.line_to(x + w, y);
                self.state.line_to(x + w, y + h);
                self.state.line_to(x, y + h);
                self.state.close_path();
            }
            // Text state.
            b"Tc" => {
                if let Some(v) = num_at(0) {
                    self.state.text.char_spacing = v;
                }
            }
            b"Tw" => {
                if let Some(v) = num_at(0) {
                    self.state.text.word_spacing = v;
                }
            }
            b"Tz" => {
                if let Some(v) = num_at(0) {
                    self.state.text.horizontal_scale = v;
                }
            }
            b"TL" => {
                if let Some(v) = num_at(0) {
                    self.state.text.leading = v;
                }
            }
            b"Tf" => {
                if let Some(n) = name_at(0) {
                    let name = String::from_utf8_lossy(&n).into_owned();
                    // The font's declared widths are read once here, beside the name that
                    // selected them, rather than per glyph: `Tf` happens once per run of
                    // text, and a page has thousands of glyphs.
                    self.state.text.widths =
                        self.resources.font_widths(&name).cloned().map(Arc::new);
                    // Whether the codes are two bytes is the font dictionary's own claim,
                    // read beside the name that selected it, because it decides how the
                    // *next* `Tj` is split into codes and nothing else in the state says.
                    self.state.text.composite = self.resources.font_is_composite(&name);
                    self.state.text.font = Some(name);
                }
                // The size is the second operand, not the second number: the first
                // operand is a name and contributes no number to find.
                if let Some(v) = operands.get(1).and_then(|o| o.as_f64()) {
                    self.state.text.size = v;
                }
            }
            b"Tr" => {
                if let Some(v) = int_at(0).and_then(crate::state::RenderMode::from_int) {
                    self.state.text.render_mode = v;
                }
            }
            b"Ts" => {
                if let Some(v) = num_at(0) {
                    self.state.text.rise = v;
                }
            }
            b"Td" => self
                .state
                .move_text(num_at(0).unwrap_or(0.0), num_at(1).unwrap_or(0.0)),
            b"TD" => {
                self.state.text.leading = -num_at(1).unwrap_or(0.0);
                self.state
                    .move_text(num_at(0).unwrap_or(0.0), num_at(1).unwrap_or(0.0));
            }
            b"Tm" => {
                if let Some(m) = Matrix::from_slice(&nums) {
                    self.state.line_matrix = m;
                    self.state.text_matrix = m;
                }
            }
            b"T*" => self.state.next_line(),
            b"BT" => self.state.begin_text(),
            _ => {}
        }
    }

    fn set_colour_space(&mut self, name: Option<Vec<u8>>, stroking: bool) {
        let Some(n) = name else { return };
        let key = String::from_utf8_lossy(&n).into_owned();
        let tint = self.resources.tint_transform(&key).cloned();
        let space = ColourSpace {
            // The name as written is what a report leads with. Whether it names an
            // ICC-based resource is a fact about the resource table rather than about the
            // operator, so it is read here — the one place the table can be consulted —
            // and carried on the space from there to the conversion.
            name: key.clone(),
            // A tint space carries the colorant it names. A `/DeviceN` names several, and
            // one field holds one of them, so the first is what a single-colorant report
            // gets and the rest are in the [`Tint`] beside it.
            colorant: tint.as_ref().and_then(|t| t.names.first().cloned()),
            icc: self.resources.icc_profile(&key).cloned(),
            // A separation or device-N space, and the transform that says what its tints
            // mean. Read from the same table, at the same moment, for the same reason.
            tint,
        };
        let target = if stroking {
            &mut self.state.stroking
        } else {
            &mut self.state.fill
        };
        target.space = space;
    }

    fn set_colour(&mut self, nums: &[f64], pattern: Option<Vec<u8>>, stroking: bool) {
        if let Some(n) = pattern {
            let target = if stroking {
                &mut self.state.stroking
            } else {
                &mut self.state.fill
            };
            target.space = ColourSpace {
                name: "Pattern".into(),
                colorant: Some(String::from_utf8_lossy(&n).into_owned()),
                icc: None,
                // A pattern is not a tint: it names a painting rather than an ink, so it
                // carries no transform and is converted by the pattern's own path.
                tint: None,
            };
            return;
        }
        if nums.is_empty() {
            return;
        }
        let space = if stroking {
            self.state.stroking.space.clone()
        } else {
            self.state.fill.space.clone()
        };
        let target = if stroking {
            &mut self.state.stroking
        } else {
            &mut self.state.fill
        };
        target.set(space, nums);
    }

    /// `Do` of an XObject. `true` when this was something the interpreter carried out, so
    /// that no mark is recorded for it.
    ///
    /// The decision is the XObject dictionary's own `/Subtype`, read from the object the
    /// name resolved to. Three answers, and the third is why the other two cannot be
    /// guessed: a form is executed here, an image is recorded for a renderer to draw, and
    /// anything else is **reported by name**. A `/PS` XObject is a real thing in the
    /// format, and a producer that invents a subtype is rarer but real; executing either
    /// would draw PostScript as operators, and handing either to an image decoder asks it
    /// for something that is not an image. Reporting is the only answer that is not a
    /// blank or a half-drawn shape.
    ///
    /// A name the table does not define is not decided here: without the object there is
    /// no `/Subtype` to read, and the renderer — which holds the document and can name
    /// what it could not resolve — reports it.
    fn do_xobject(&mut self, name: &[u8], span: &Range<usize>) -> bool {
        let Ok(key) = std::str::from_utf8(name) else {
            return false;
        };
        let Some(object) = self.resources.xobjects.get(key) else {
            return false;
        };
        let Some(dict) = crate::xobject_dict(object) else {
            return false;
        };
        if crate::is_form(dict) {
            self.execute_form(key, object, dict, span);
            return true;
        }
        if let Some(subtype) = dict.get("Subtype").and_then(Object::as_name)
            && subtype != b"Image"
        {
            self.out.notes.push(format!(
                "the XObject `/{key}` is a `/Subtype /{}`, which is neither an image nor a \
                 form, so it was not executed",
                String::from_utf8_lossy(subtype)
            ));
            return true;
        }
        false
    }

    /// Execute a form XObject as though its content were wrapped in `q` … `Q`.
    ///
    /// Everything the form does is the page's: its marks go into the same records in the
    /// same order, at the point the `Do` appears, because that is where a painter's-algorithm
    /// renderer will draw them and no later mark may go underneath. Three things come from
    /// the form's own dictionary, and all three are applied to the state the form runs in:
    /// the `/Matrix` composes into the CTM, the `/BBox` clips, and the `/Resources` decide
    /// what its names resolve against. The state is put back afterwards whatever happened,
    /// because a form's operators are not allowed to change the page that drew it.
    ///
    /// **Depth is bounded, and the bound is where it belongs.** The recursion is here, in
    /// the run rather than in the stream, so a form that draws itself stops and says so
    /// instead of exhausting the stack. A name the table does not define is reported by the
    /// renderer; a form that *is* defined but cannot be executed is reported here, by name,
    /// because a form this could not run is a finding about the file and a blank patch of
    /// page is not.
    fn execute_form(&mut self, name: &str, object: &Object, dict: &Dict, span: &Range<usize>) {
        if self.depth >= MAX_FORM_DEPTH {
            self.out.notes.push(format!(
                "the form XObject `/{name}` is drawn from inside another form more than \
                 {MAX_FORM_DEPTH} deep, and was not executed"
            ));
            return;
        }
        let Object::Stream(stream) = object else {
            self.out.notes.push(format!(
                "the form XObject `/{name}` is a dictionary rather than a stream, so it has \
                 no content to execute"
            ));
            return;
        };
        // The raw bytes are never handed to the interpreter: a compressed stream read as
        // if it were operators is a page of nonsense, and a page of nonsense is a blank
        // patch with a plausible-looking reason attached to it.
        let decoded = decode_stream(stream);
        if !decoded.complete {
            if decoded.notes.is_empty() {
                self.out
                    .notes
                    .push(format!("the form XObject `/{name}` did not decode cleanly"));
            }
            for note in &decoded.notes {
                self.out
                    .notes
                    .push(format!("the form XObject `/{name}`: {note}"));
            }
        }
        if decoded.encoded {
            // The bytes are still encoded. Handing them to the operator table would draw
            // whatever the compressed stream happens to contain as a page, and the notes
            // above already name the filter and the form.
            return;
        }
        if decoded.data.is_empty() {
            // A form with no content draws nothing, and that is not a finding: it is what
            // the file said. The report is for a form this *could not* run.
            return;
        }
        if let Some(matrix) = dict.get("Matrix") {
            match form_matrix(matrix) {
                Some(m) => self.state.concat(m),
                None => self.out.notes.push(format!(
                    "the form XObject `/{name}` declares a `/Matrix` that is not six numbers, \
                     so its content was drawn without it"
                )),
            }
        }
        match form_bbox(dict.get("BBox")) {
            BBox::Absent => {}
            BBox::Unreadable => self.out.notes.push(format!(
                "the form XObject `/{name}` declares a `/BBox` that is not four numbers, so \
                 its content was not clipped"
            )),
            BBox::Inverted(ordered) => {
                // Reported, and then read as the same box with its corners in order. The
                // alternative reading — an empty box, and therefore an invisible form — is
                // a corner-ordering mistake in the file being allowed to delete a page.
                self.out.notes.push(format!(
                    "the form XObject `/{name}` declares its `/BBox` the wrong way round, so \
                     it is read as the same box with its corners in order"
                ));
                self.clip_to_box(ordered);
            }
            BBox::Box(ordered) => self.clip_to_box(ordered),
        }
        // A `W` in force at the `Do` clips the form's execution as well, so it is committed
        // to the state the form runs in rather than left pending for whatever follows. The
        // page is still told where its own clip changed, because that is a fact about the
        // page and not only about the form.
        if std::mem::take(&mut self.clip_pending) {
            let next = self.clip_for_path(FillRule::NonZero);
            self.commit_clip(Some(next));
            let changed = Mark::ClipChanged(self.state.clip.clone());
            let record = self.record_for(changed, span.clone());
            self.push_record(record);
        }
        // `Q`: the state as it was before the `/Matrix` and the `/BBox` went in. Copied rather
        // than moved, because the form's state is derived from it and this run has to get
        // its own back afterwards.
        let outer = self.state.clone();
        let prior = self.prior + self.out.records.len();
        let depth = self.depth + 1;
        // The form's own resources where it declares them, and the ones it was drawn
        // against where it does not: a form with no `/Resources` inherits, and what it
        // inherits is the table it was *named in*, so a form inside a form inherits that
        // form's table rather than the page's.
        let own = self.resources.form_resources(name);
        let mut inner = Context {
            state: std::mem::take(&mut self.state),
            stack: StateStack::new(),
            out: PageContent::default(),
            clip_pending: false,
            path_start: None,
            resources: own.as_deref().unwrap_or(self.resources),
            depth,
            form: Some(name.to_string()),
            prior,
        };
        // The form's execution is a `q`, so its `q`s start one level further in.
        inner.state.depth = outer.depth + 1;
        inner.execute(&ContentStream::parse(&decoded.data));
        self.out.records.append(&mut inner.out.records);
        self.out
            .unknown_operators
            .append(&mut inner.out.unknown_operators);
        self.out.notes.append(&mut inner.out.notes);
        self.state = outer;
        // A `Do` ends the path, as it did when it was a mark, and the bytes that built it
        // are not the bytes that drew whatever came next.
        self.state.clear_path();
        self.path_start = None;
    }

    /// The clip the path being built sets, and the empty region when it has no path.
    ///
    /// The rule is the operator's own: `W f*` clips by the even-odd rule exactly as it
    /// fills by it.
    fn clip_for_path(&self, rule: FillRule) -> Clip {
        if !self.state.has_path() {
            // A pending `W` with no path is not "no clip". The specification says an empty
            // path sets the region to the empty one, and until the clip is reset nothing is
            // drawn.
            return Clip::empty();
        }
        let segments = self.state.device_path();
        let bounds = bounds_of(&Mark::Path {
            segments: segments.clone(),
            fill: None,
            stroke: None,
            rule,
        })
        .unwrap_or(ClipBounds {
            x0: 0.0,
            y0: 0.0,
            x1: 0.0,
            y1: 0.0,
        });
        Clip::new(bounds, ClipPath { segments, rule })
    }

    /// Narrow the clip in force by another one.
    ///
    /// Two clips nest: the second is intersected with the first rather than replacing it.
    fn commit_clip(&mut self, next: Option<Clip>) {
        self.state.clip = match (&self.state.clip, &next) {
            (Some(existing), Some(next)) => Some(existing.intersect(next)),
            (Some(existing), None) => Some(existing.clone()),
            (None, Some(next)) => Some(next.clone()),
            (None, None) => None,
        };
    }

    /// Clip to a box in *this run's* own space, on top of the clip already in force.
    ///
    /// A form's `/BBox` is in the form's space, so the four corners go through the CTM the
    /// form is executing under — the same matrix the path of a `W` operator goes through,
    /// and for the same reason: a rotation does not map an axis-aligned box to an
    /// axis-aligned one, so the corners are placed and the result re-bounded.
    ///
    /// The clip is a region and not only a box, because that is what every consumer of a
    /// clip already expects to find, and a box drawn with a region behind it is narrower
    /// than the page asked for wherever the shape was smaller than its own bounds.
    fn clip_to_box(&mut self, bounds: ClipBounds) {
        let ctm = self.state.ctm;
        let corners = [
            ctm.apply(bounds.x0, bounds.y0),
            ctm.apply(bounds.x1, bounds.y0),
            ctm.apply(bounds.x1, bounds.y1),
            ctm.apply(bounds.x0, bounds.y1),
        ];
        let x0 = corners.iter().map(|c| c.0).fold(f64::INFINITY, f64::min);
        let x1 = corners
            .iter()
            .map(|c| c.0)
            .fold(f64::NEG_INFINITY, f64::max);
        let y0 = corners.iter().map(|c| c.1).fold(f64::INFINITY, f64::min);
        let y1 = corners
            .iter()
            .map(|c| c.1)
            .fold(f64::NEG_INFINITY, f64::max);
        let segments = vec![
            PathSegment::Move(corners[0].0, corners[0].1),
            PathSegment::Line(corners[1].0, corners[1].1),
            PathSegment::Line(corners[2].0, corners[2].1),
            PathSegment::Line(corners[3].0, corners[3].1),
            PathSegment::Close,
        ];
        let box_clip = Clip::new(
            ClipBounds { x0, y0, x1, y1 },
            ClipPath {
                segments,
                rule: FillRule::NonZero,
            },
        );
        self.commit_clip(Some(box_clip));
    }

    /// A record for `mark` at `span`, carrying the state it was drawn in.
    ///
    /// Every record is built here so that no mark can exist without the clip in force, the
    /// alphas and the stroke style that were current for it, and so that every mark knows
    /// which form it came from.
    fn record_for(&self, mark: Mark, span: Range<usize>) -> Record {
        Record {
            mark,
            span,
            ctm: self.state.ctm,
            clip: self.state.clip.clone(),
            fill_alpha: self.state.fill_alpha,
            stroke_alpha: self.state.stroke_alpha,
            blend_mode: self.state.blend_mode.clone(),
            device_line_width: self.state.stroke.width * self.state.ctm.mean_scale(),
            line_cap: self.state.stroke.cap,
            line_join: self.state.stroke.join,
            dash: self.state.stroke.dash.clone(),
            tag: self.out.tags.last().cloned(),
            form: self.form.clone(),
        }
    }

    /// Add a record, at the limit and no further.
    fn push_record(&mut self, record: Record) {
        if self.out.records.len() < MAX_RECORDS {
            self.out.records.push(record);
        }
    }

    /// `gs`: take the state from a named `/ExtGState` dictionary.
    ///
    /// A name the page's resources do not define is a note, not a silent no-op: the
    /// marks after it were drawn with a state we do not know, and the user is better
    /// told than shown a page that is subtly wrong.
    fn apply_extgstate(&mut self, name: &[u8]) {
        match self.resources.ext_gstate(name) {
            Some(gs) => gs.apply(&mut self.state),
            None => self.out.notes.push(format!(
                "`gs /{}` names a /ExtGState the page's resources do not define",
                String::from_utf8_lossy(name)
            )),
        }
    }

    /// Record what a painting operator drew, and end the path.
    fn record_mark(&mut self, op: Operation, name: &[u8], operands: &[&Object]) {
        let clip_pending = std::mem::take(&mut self.clip_pending);
        // The rule an operator carries in its name, and the same rule the clip it sets is
        // decided by: `W f*` clips by the even-odd rule exactly as it fills by it.
        let op_rule = if matches!(name, b"f*" | b"B*" | b"b*") {
            FillRule::EvenOdd
        } else {
            FillRule::NonZero
        };
        // The clip is the path that is being built, whatever the operator is going to
        // do with it: `W n` clips to it and `W f` clips to it and fills it.
        //
        // The region and its box are built from this one path, in one place, so that they
        // cannot disagree. A box computed from a different path than the one being clipped
        // to is worse than either of them being wrong alone: it is a claim about a shape
        // that is not on the page.
        let clip_path = clip_pending.then(|| self.clip_for_path(op_rule));
        let mut mark = match name {
            b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"S" | b"s" => Mark::Path {
                segments: self.state.device_path(),
                // Which of the two colours the operator uses is the operator's business,
                // not the state's: `S` strokes and does not fill, `f` fills and does not
                // stroke, and the four that do both are named for it. Carrying both
                // colours for every mark would have a fill-only operator draw an outline
                // in whatever colour happened to be set.
                fill: if name == b"S" {
                    None
                } else {
                    Some(self.state.fill.clone())
                },
                stroke: if matches!(name, b"f" | b"F" | b"f*") {
                    None
                } else {
                    Some(self.state.stroking.clone())
                },
                rule: op_rule,
            },
            // `n` paints nothing. It is here only because a pending `W` needs an
            // operation to attach itself to, and the attachment is the mark.
            b"n" => Mark::ClipChanged(self.state.clip.clone()),
            b"sh" => Mark::Shading {
                name: operands
                    .first()
                    .and_then(|o| o.as_name())
                    .map(|n| String::from_utf8_lossy(n).into_owned())
                    .unwrap_or_default(),
                matrix: self.state.ctm,
            },
            b"Do" => Mark::Image {
                name: operands
                    .first()
                    .and_then(|o| o.as_name())
                    .map(|n| String::from_utf8_lossy(n).into_owned()),
                matrix: self.state.ctm,
                inline: false,
                fill: self.state.fill.clone(),
            },
            b"Tj" | b"TJ" | b"'" | b"\"" => {
                self.record_text(op, operands, matches!(name, b"'" | b"\""));
                // `'` and `"` move to the next line first.
                if matches!(name, b"'" | b"\"") {
                    self.state.next_line();
                }
                return;
            }
            b"BI" => Mark::Image {
                name: None,
                matrix: self.state.ctm,
                inline: true,
                fill: self.state.fill.clone(),
            },
            _ => return,
        };

        // A pending `W` narrows the clip for this mark and every one after it.
        if clip_pending {
            self.commit_clip(clip_path);
            mark = Mark::ClipChanged(self.state.clip.clone());
        }

        let span = match self.path_start {
            // No path was built, so the mark is the operator and its operands alone.
            None => op.span.clone(),
            Some(start) => start..op.span.end,
        };
        let record = self.record_for(mark, span);
        self.push_record(record);
        // Every painting operator ends the path; `s`, `f` and `B*` also close it first,
        // which `apply` has already done.
        self.state.clear_path();
        self.path_start = None;
    }

    /// Record a text-showing operator.
    ///
    /// A shown string is a run of *character codes*, and how many bytes make one code is
    /// the current font's own claim: a simple font's codes are one byte each, a composite
    /// font's are two. The string is split accordingly, and everything after this point —
    /// the placements, the advances, the kerns — counts codes rather than bytes, because
    /// counting bytes is what puts every glyph after the first in the wrong place on a
    /// page set in a composite font.
    ///
    /// The raw bytes are still carried in the mark beside the codes, because they are what
    /// an edit rewrites: the codes are what the page means, the bytes are what the file
    /// says.
    fn record_text(&mut self, op: Operation, operands: &[&Object], _newline_first: bool) {
        let two_byte = self.state.text.composite;
        // The operand tokens, not the values, so a string's own bytes can be recorded.
        let strings: Vec<&ContentToken> = (0..operands.len())
            .filter_map(|i| op.operands.get(i))
            .filter(|t| matches!(t.value, Object::String(_)))
            .collect();
        let mut text: Vec<u8> = Vec::new();
        let mut text_spans: Vec<Range<usize>> = Vec::new();
        // The character codes, one per glyph. This is what the placements and the advances
        // are keyed by, and for a composite font it holds half as many entries as `text`
        // has bytes.
        let mut codes: Vec<u32> = Vec::new();
        // A `TJ` kern is paired with the glyphs of the string it *follows*, so it is
        // collected per glyph rather than as one number per glyph: the strings in a `TJ`
        // are of whatever lengths the file likes, and a kern displaces what comes after it
        // rather than the glyph at the same index. One kern per glyph would put a kern in
        // the wrong place for every string that is not exactly one character long.
        let mut kern_after: Vec<f64> = Vec::new();
        // Push a string's glyphs, leaving room for each one's own following kern. The
        // kerns are counted in codes, so this is where the two widths part company: a
        // two-byte string of four bytes is two glyphs and gets two kerns.
        let push =
            |bytes: &[u8], text: &mut Vec<u8>, codes: &mut Vec<u32>, kern_after: &mut Vec<f64>| {
                text.extend_from_slice(bytes);
                let added = codes_of(bytes, two_byte);
                kern_after.resize(kern_after.len() + added.len(), 0.0);
                codes.extend(added);
            };
        // A kern applies to the last glyph pushed, which is the one it follows.
        let kern = |value: f64, kern_after: &mut Vec<f64>| {
            if let Some(last) = kern_after.last_mut() {
                *last = value;
            }
        };
        for operand in operands {
            match operand {
                Object::String(s) => push(s, &mut text, &mut codes, &mut kern_after),
                Object::Array(a) => {
                    for item in a {
                        match item {
                            Object::String(s) => push(s, &mut text, &mut codes, &mut kern_after),
                            other => {
                                if let Some(v) = other.as_f64() {
                                    kern(v, &mut kern_after);
                                }
                            }
                        }
                    }
                }
                other => {
                    // A number where a string belongs is damage; record what we can.
                    if let Some(v) = other.as_f64() {
                        kern(v, &mut kern_after);
                    }
                }
            }
        }
        // A string with no glyph cannot carry a kern, and one left over applies to the
        // glyph before it, which is what the specification's own order says.
        let kerns = kern_after;
        // A string operand's own bytes; an array's are the array's span, which is the
        // tightest range that contains every string inside it.
        for token in &strings {
            text_spans.push(token.span.clone());
        }
        for (i, operand) in operands.iter().enumerate() {
            if operand.as_array().is_none() {
                continue;
            }
            if let Some(token) = op.operands.get(i) {
                text_spans.push(token.span.clone());
            }
        }
        if codes.is_empty() {
            return;
        }
        // Walk the *text matrix*, not the page. The cursor is in text space, whose units are
        // the pen's own: the text matrix is not scaled by the font size, so the size has to
        // be applied to each glyph's width instead, and the rendering matrix is composed
        // per glyph from the position it lands at. Stepping an already-scaled base by an
        // unscaled advance is how the placements and the advance come to disagree by a
        // factor of the size.
        let mut placements = Vec::with_capacity(codes.len());
        let mut cursor = self.state.text_matrix;
        for (index, code) in codes.iter().enumerate() {
            placements.push(self.state.text_rendering_matrix_for(&cursor));
            cursor = cursor.concat(Matrix::translate(self.glyph_advance(*code), 0.0));
            // A `TJ` number *follows* the string it displaces, so it moves what comes after
            // it: the kern at `index` applies to the next glyph, not to this one. It is a
            // displacement of the pen in thousandths of an em of unscaled text space and it
            // is subtracted, so a positive number pulls the following glyph closer, which
            // is the sign the specification means. It scales with the size and the
            // horizontal scale exactly as the glyph's own advance does.
            if let Some(kern) = kerns.get(index) {
                let kern = *kern / 1000.0 * self.state.text.size * self.state.text.horizontal_scale
                    / 100.0;
                cursor = cursor.concat(Matrix::translate(-kern, 0.0));
            }
        }
        let record = self.record_for(
            Mark::Glyphs {
                font: self.state.text.font.clone(),
                size: self.state.text.size,
                text,
                // The character codes, one per glyph: half as many entries as `text` has
                // bytes for a composite font, which is the whole point of carrying them.
                codes,
                two_byte,
                fill: self.state.fill.clone(),
                text_spans,
                placements,
            },
            op.span.clone(),
        );
        self.push_record(record);
        // The text matrix moves past what was shown, which is what makes a second `Tj`
        // continue rather than overlap, and by the sum of the glyphs' own advances rather
        // than one figure for all of them. It is left where the cursor walked it, which is
        // the same place the placements were composed from. The *line* matrix does not
        // move: a `Td` after this starts a new line from where the last one began, not from
        // the end of the text on it.
        self.state.text_matrix = cursor;
    }

    /// How far one glyph moves the pen, in text-space units.
    ///
    /// The specification's own formula, `tx = ((w0 − Tj/1000)·Tfs + Tc + Tw)·Th`: the
    /// glyph's declared width, which `/Widths` gives in thousandths of an em, times the
    /// size, plus the two spacing terms, all scaled by the horizontal scale. The size
    /// belongs here rather than in the text matrix because the text matrix's translation is
    /// *not* scaled by the size — a `Td` means the same distance at every font size — so
    /// the size has to be applied to the glyph's own width to land in the same units the
    /// pen is already walking in.
    ///
    /// Character and word spacing are stated in unscaled text-space units and are added as
    /// they stand, which is what makes them the one term here with no size in it.
    ///
    /// Word spacing applies to the one code the specification names, and to nothing else: a
    /// file that puts a wide space in its own text gets the glyph's width twice over if this
    /// is applied to every code that happens to be a space. It is a *single-byte* code, so a
    /// composite font's two-byte code 32 — which is a different thing entirely — does not
    /// get it, and a font whose space is CID 3 gets nothing.
    ///
    /// A code the font says nothing about falls back to the conventional 500-unit average,
    /// which is what the specification's default `/MissingWidth` amounts to for layout
    /// purposes. Nothing here divides by the size, so a missing or zero size costs nothing:
    /// it makes the glyph's own term zero, which is what a zero-sized glyph is.
    ///
    /// The code is the full character code, so for a composite font it is the two bytes read
    /// as one number — 0x0041 is a CID, not two codes 0 and 65. The width comes from the
    /// descendant font's run-length `/W` in that case, which is why the lookup goes through
    /// the declared-widths enum rather than through a `/Widths` array indexed from
    /// `/FirstChar`.
    fn glyph_advance(&self, code: u32) -> f64 {
        let text = &self.state.text;
        let declared = text
            .widths
            .as_ref()
            .and_then(|w| w.width_of(code))
            .unwrap_or(DEFAULT_WIDTH);
        let spacing = if !text.composite && code == WORD_SPACE {
            text.word_spacing
        } else {
            0.0
        };
        (f64::from(declared) / 1000.0 * text.size + text.char_spacing + spacing)
            * text.horizontal_scale
            / 100.0
    }
}

/// The character codes in one shown string, as the current font's encoding names them.
///
/// A simple font's code is one byte. A composite font's is two, big-endian, so the string
/// is read in pairs — the first byte is the high half of the CID, which is the order the
/// specification writes them in and the order every `Identity-H` string is written in.
///
/// A trailing odd byte is dropped rather than completed: it is half a code, and a code made
/// of half of one names no glyph in any font. Padding it with zero would place a glyph
/// nobody asked for, and a viewer that drew one would be inventing content, which is worse
/// than a character missing from a truncated string.
fn codes_of(bytes: &[u8], two_byte: bool) -> Vec<u32> {
    if two_byte {
        bytes
            .chunks_exact(2)
            .filter_map(|pair| {
                let high = pair.first()?;
                let low = pair.get(1)?;
                Some(u32::from(u16::from_be_bytes([*high, *low])))
            })
            .collect()
    } else {
        bytes.iter().map(|b| u32::from(*b)).collect()
    }
}

fn intent_name(v: i64) -> String {
    match v {
        0 => "AbsoluteColorimetric",
        1 => "RelativeColorimetric",
        2 => "Saturation",
        3 => "Perceptual",
        _ => "RelativeColorimetric",
    }
    .to_string()
}

/// A `/BBox`, which is what a form XObject and an image draw into.
#[must_use]
pub fn bbox_of(stream: &Stream) -> Option<ClipBounds> {
    let arr = stream.dict.get("BBox")?.as_array()?;
    let v: Vec<f64> = arr.iter().take(4).filter_map(Object::as_f64).collect();
    let [x0, y0, x1, y1] = v.as_slice() else {
        return None;
    };
    // A `/BBox` with the corners the other way round is wrong but not fatal, and
    // ordering them is what every consumer wants.
    Some(ClipBounds {
        x0: x0.min(*x1),
        y0: y0.min(*y1),
        x1: x0.max(*x1),
        y1: y0.max(*y1),
    })
}

/// A form's own `/Matrix`, or `None` when the entry is not one.
///
/// Optional, and the specification's default is the identity. A six-element array is
/// `[a b c d e f]`; anything else is damage, and is reported rather than turned into a
/// degenerate transformation that would collapse the form to a line.
#[must_use]
pub fn form_matrix(entry: &Object) -> Option<Matrix> {
    let values = entry.as_array()?;
    if values.len() != 6 {
        return None;
    }
    let at = |i: usize| values.get(i).and_then(Object::as_f64).unwrap_or(0.0);
    Some(Matrix::new(at(0), at(1), at(2), at(3), at(4), at(5)))
}

/// What a form's `/BBox` entry says.
///
/// Three answers and not two, because the third case is the one that would otherwise
/// delete a page from the middle of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BBox {
    /// No `/BBox` at all: the form clips nothing of its own, which is legal and is not the
    /// same as a box that clips everything away.
    Absent,
    /// Four corners, ordered so that `x0 <= x1` and `y0 <= y1`.
    Box(ClipBounds),
    /// Four corners the file gave the wrong way round. Reported, and then read as the same
    /// box ordered — see [`BBox::Inverted`].
    Inverted(ClipBounds),
    /// Present, and not four numbers. Reported, and no clip taken: a box this cannot read
    /// is not evidence that the form drew nothing.
    Unreadable,
}

/// Read a form's `/BBox`, with the three answers kept apart.
///
/// **`bbox_of` cannot be used here**: it answers `None` for a `/BBox` that is absent and
/// for one that cannot be read, and those two mean opposite things — a form with no box
/// clips nothing and is drawn whole, while a form whose box is damaged is a finding. Both
/// are told apart here.
///
/// **A box that is zero-width or zero-height is not reported.** The specification makes it
/// clip everything away, and that is what a producer that emitted `[0 0 0 0]` asked for.
/// An **inverted** box is a different matter: the same corners with the wrong order are a
/// slip, and reading them as an empty box would make the form invisible — so they are
/// reported *and* read as the box they obviously meant.
#[must_use]
pub fn form_bbox(entry: Option<&Object>) -> BBox {
    let Some(entry) = entry else {
        return BBox::Absent;
    };
    let Some(values) = entry.as_array() else {
        return BBox::Unreadable;
    };
    let numbers: Vec<f64> = values.iter().take(4).filter_map(Object::as_f64).collect();
    let [x0, y0, x1, y1] = numbers.as_slice() else {
        return BBox::Unreadable;
    };
    let ordered = ClipBounds {
        x0: x0.min(*x1),
        y0: y0.min(*y1),
        x1: x0.max(*x1),
        y1: y0.max(*y1),
    };
    if *x1 < *x0 || *y1 < *y0 {
        BBox::Inverted(ordered)
    } else {
        BBox::Box(ordered)
    }
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect` and index a slice whose length they
    // have already asserted. Both are what a test is for; the panic-free rule is about
    // what the product does with a file, not about how a test reads one.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use crate::Resources;
    use mangle_syntax::object::{Dict, Object as Obj, Stream};

    fn run_bytes(data: &[u8]) -> PageContent {
        run(&ContentStream::parse(data))
    }

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    /// Resources with one font named `F1`, declaring `widths` for the codes from
    /// `/FirstChar` upwards.
    fn font_resources(first: i64, widths: &[i64]) -> Resources {
        let mut font = Dict::new();
        font.set("Type", Obj::name("Font"));
        font.set("BaseFont", Obj::name("Helvetica"));
        font.set("FirstChar", Obj::Int(first));
        font.set(
            "Widths",
            Obj::Array(widths.iter().map(|w| Obj::Int(*w)).collect()),
        );
        let mut table = Dict::new();
        table.set("F1", Obj::Dict(font));
        let mut resources = Dict::new();
        resources.set("Font", Obj::Dict(table));
        Resources::from_dict(&resources, &|o| Some(o.clone()))
    }

    /// One width for every code from 32 up, all different, so that a test cannot pass
    /// with a single figure for all of them.
    fn distinct_widths() -> Vec<i64> {
        (0..95).map(|i| 200 + i * 7).collect()
    }

    /// Resources with one composite font named `F1`: `/Type0`, `/Identity-H`, and a
    /// descendant whose `/W` is `w`.
    ///
    /// `/DescendantFonts` holds the descendant dictionary directly rather than behind a
    /// reference, which is the only difference from what a real file writes and keeps the
    /// helper's resolver trivial.
    fn composite_resources(w: Vec<Obj>, dw: i64) -> Resources {
        let mut descendant = Dict::new();
        descendant.set("Type", Obj::name("Font"));
        descendant.set("Subtype", Obj::name("CIDFontType2"));
        descendant.set("W", Obj::Array(w));
        descendant.set("DW", Obj::Int(dw));
        let mut font = Dict::new();
        font.set("Type", Obj::name("Font"));
        font.set("Subtype", Obj::name("Type0"));
        font.set("BaseFont", Obj::name("Embedded"));
        font.set("Encoding", Obj::name("Identity-H"));
        font.set("DescendantFonts", Obj::Array(vec![Obj::Dict(descendant)]));
        let mut table = Dict::new();
        table.set("F1", Obj::Dict(font));
        let mut resources = Dict::new();
        resources.set("Font", Obj::Dict(table));
        Resources::from_dict(&resources, &|o| Some(o.clone()))
    }

    /// A composite font whose descendant declares no `/W` at all, only `/DW`.
    fn composite_default_only(dw: i64) -> Resources {
        let mut descendant = Dict::new();
        descendant.set("Subtype", Obj::name("CIDFontType2"));
        descendant.set("DW", Obj::Int(dw));
        let mut font = Dict::new();
        font.set("Type", Obj::name("Font"));
        font.set("Subtype", Obj::name("Type0"));
        font.set("Encoding", Obj::name("Identity-H"));
        font.set("DescendantFonts", Obj::Array(vec![Obj::Dict(descendant)]));
        let mut table = Dict::new();
        table.set("F1", Obj::Dict(font));
        let mut resources = Dict::new();
        resources.set("Font", Obj::Dict(table));
        Resources::from_dict(&resources, &|o| Some(o.clone()))
    }

    /// The glyph codes of the first text mark, in order.
    fn shown_codes(data: &[u8], resources: &Resources) -> Vec<u32> {
        let out = run_with(&ContentStream::parse(data), resources);
        out.records
            .iter()
            .filter_map(|r| match &r.mark {
                Mark::Glyphs { codes, .. } => Some(codes.clone()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    /// The placement count of the first text mark.
    fn placement_count(data: &[u8], resources: &Resources) -> usize {
        let out = run_with(&ContentStream::parse(data), resources);
        out.records
            .iter()
            .find_map(|r| match &r.mark {
                Mark::Glyphs { placements, .. } => Some(placements.len()),
                _ => None,
            })
            .expect("a text mark")
    }

    /// The same figure for every code, so that what moves a run is the spacing and
    /// nothing else.
    fn even_resources(width: i64) -> Resources {
        font_resources(32, &vec![width; 95])
    }

    /// The x of every glyph's placement, in order, across every show in the stream.
    fn glyph_x(data: &[u8], resources: &Resources) -> Vec<f64> {
        let out = run_with(&ContentStream::parse(data), resources);
        let mut xs = Vec::new();
        for record in &out.records {
            let Mark::Glyphs { placements, .. } = &record.mark else {
                continue;
            };
            xs.extend(placements.iter().map(|m| m.e));
        }
        xs
    }

    /// The advance between two consecutive glyphs, in text-space units.
    ///
    /// A placement is the text rendering matrix composed from the text matrix at that
    /// glyph, and the text matrix is unscaled by the font size, so the distance between two
    /// placements *is* the advance that was applied to it. Nothing needs dividing out.
    fn advance(from: f64, to: f64, _size: f64) -> f64 {
        to - from
    }

    /// The x the text matrix was left at, which is where a following show continues.
    fn final_x(data: &[u8], resources: &Resources) -> f64 {
        run_with(&ContentStream::parse(data), resources)
            .state
            .text_matrix
            .e
    }

    /// One show of `text` at `size`, after the text-state operators in `setup`.
    fn show_at(size: &str, setup: &str, text: &str) -> Vec<u8> {
        [
            b"BT /F1 ".as_slice(),
            size.as_bytes(),
            b" Tf ".as_slice(),
            setup.as_bytes(),
            b" 0 0 Td (".as_slice(),
            text.as_bytes(),
            b") Tj ET".as_slice(),
        ]
        .concat()
    }

    #[test]
    fn a_run_advances_by_the_sum_of_the_widths_its_font_declares() {
        // The expectation is summed from the same array the font dictionary is built
        // from, so the test states the rule rather than one set of numbers.
        let declared = distinct_widths();
        let width_of = |c: u8| f64::from(declared[usize::from(c) - 32] as u32);
        let text = b"Hamburgefonstiv0123456789";
        let size = 10.0;
        let expected: Vec<f64> = text.iter().map(|c| width_of(*c) / 1000.0 * size).collect();
        let sum: f64 = expected.iter().sum();
        let resources = font_resources(32, &declared);
        let stream = b"BT /F1 10 Tf 0 0 Td (Hamburgefonstiv0123456789) Tj ET";

        let xs = glyph_x(stream, &resources);
        assert_eq!(xs.len(), text.len(), "one placement per glyph");
        assert!(near(xs[0], 0.0), "the first glyph is where `Td` put it");
        for (i, (code, want)) in text.iter().zip(&expected).enumerate().take(text.len() - 1) {
            let got = advance(xs[i], xs[i + 1], size);
            assert!(
                near(got, *want),
                "glyph {} advanced by {got}, not its own width {want}",
                *code as char
            );
        }
        // The text matrix is left past the last glyph by the same sum, so a second show
        // continues rather than overlapping.
        assert!(
            near(final_x(stream, &resources), sum),
            "the run is as wide as its declared widths say: {sum}"
        );
    }

    #[test]
    fn two_glyphs_in_one_run_get_two_different_advances() {
        // No flat figure can pass this: the two glyphs are declared far apart.
        let declared = distinct_widths();
        let resources = font_resources(32, &declared);
        // Three glyphs, so that both advances are gaps between placements.
        let xs = glyph_x(b"BT /F1 1000 Tf 0 0 Td (WiW) Tj ET", &resources);
        assert_eq!(xs.len(), 3);
        let w = advance(xs[0], xs[1], 1000.0);
        let i = advance(xs[1], xs[2], 1000.0);
        assert!(near(w, f64::from(declared[87 - 32] as u32)), "`W`: {w}");
        assert!(near(i, f64::from(declared[105 - 32] as u32)), "`i`: {i}");
        assert!(
            (w - i).abs() > 1.0,
            "and they are different: {w} against {i}"
        );
    }

    #[test]
    fn a_tj_number_moves_the_following_glyph_and_the_sign_says_which_way() {
        // A `TJ` number is in thousandths of an em and is subtracted from the
        // displacement, so a positive number pulls the following glyph closer and a
        // negative one undoes part of the run's advance, moving the pen back the other
        // way. The sign is the whole content of the rule, so both are pinned.
        let declared = vec![500_i64; 95];
        let resources = font_resources(32, &declared);
        let second = |kern: &str| {
            let stream = [
                b"BT /F1 1000 Tf 0 0 Td [(A)".as_slice(),
                kern.as_bytes(),
                b" (B)] TJ ET",
            ]
            .concat();
            glyph_x(&stream, &resources)[1]
        };
        let plain = second("");
        let closer = second(" 500");
        let further = second(" -500");
        assert!(
            closer < plain,
            "a positive number pulls the glyph closer: {closer} against {plain}"
        );
        assert!(
            further > plain,
            "a negative one moves it the other way: {further} against {plain}"
        );
        assert!(
            near(further - plain, -(closer - plain)),
            "and the two are mirror images: {closer}, {plain}, {further}"
        );
    }

    #[test]
    fn a_font_with_no_widths_advances_by_the_old_half_em() {
        // The pinned fallback: with nothing declared and no table to consult, a glyph is half
        // an em wide, which is what this layer has always done. The expectation is written as
        // the rule rather than as a copied constant, so it says which behaviour is being held.
        //
        // The name is deliberately *not* a standard font. One of those now answers from its
        // built-in table — see `a_standard_font_with_no_widths_uses_the_built_in_table` — and a
        // test of the fallback that named Helvetica would be testing the other path.
        let mut font = Dict::new();
        font.set("BaseFont", Obj::name("NoSuchFont"));
        let mut table = Dict::new();
        table.set("F1", Obj::Dict(font));
        let mut resources_dict = Dict::new();
        resources_dict.set("Font", Obj::Dict(table));
        let resources = Resources::from_dict(&resources_dict, &|o| Some(o.clone()));
        assert!(
            resources.font_widths("F1").is_none(),
            "this font declares nothing and is not a standard one, which is the case under test"
        );
        let size = 24.0;
        let half = size * 0.5;
        let xs = glyph_x(b"BT /F1 24 Tf 0 0 Td (iiii) Tj ET", &resources);
        assert_eq!(xs.len(), 4);
        for gap in [advance(xs[0], xs[1], size), advance(xs[2], xs[3], size)] {
            assert!(near(gap, half), "each gap is half an em: {gap}");
        }
        assert!(
            near(advance(xs[1], xs[2], size), half),
            "and so is the one between them"
        );
    }

    #[test]
    fn a_standard_font_with_no_widths_uses_the_built_in_table() {
        // The standard fourteen have no `/Widths` by definition, so a document that names one
        // without embedding it is not a file with a gap in it. Falling back to one average
        // advance would put every character after the first in the wrong place on most of the
        // LaTeX and office output in the wild.
        let mut font = Dict::new();
        font.set("BaseFont", Obj::name("Times-Roman"));
        let mut table = Dict::new();
        table.set("F1", Obj::Dict(font));
        let mut resources_dict = Dict::new();
        resources_dict.set("Font", Obj::Dict(table));
        let resources = Resources::from_dict(&resources_dict, &|o| Some(o.clone()));
        assert!(
            resources.font_widths("F1").is_some(),
            "a standard font must answer for itself even with no /Widths"
        );

        let size = 24.0;
        // Three glyphs, so both widths are measured: a gap is the advance of the glyph
        // *before* it. `W` is 944 and `i` is 278 in Times-Roman, so the two differ by more
        // than a factor of three, which one fallback width cannot produce.
        let xs = glyph_x(b"BT /F1 24 Tf 0 0 Td (WiW) Tj ET", &resources);
        assert_eq!(xs.len(), 3);
        let wide = advance(xs[0], xs[1], size);
        let narrow = advance(xs[1], xs[2], size);
        assert!(
            near(wide, 944.0 / 1000.0 * size),
            "the wide glyph advances by its own width: {wide}"
        );
        assert!(
            near(narrow, 278.0 / 1000.0 * size),
            "and the narrow one by a different one: {narrow}"
        );
    }

    #[test]
    fn a_declared_width_array_still_wins_over_the_built_in_table() {
        // The fallback is for a font that declares nothing. A file that declares a width has
        // said what the width is, and second-guessing it with a table for a font of the same
        // name is how a document's own layout gets overridden.
        // `W` is 944 in the built-in Times-Roman table and 500 in the array, so the two
        // disagree about a code both of them cover.
        let mut font = Dict::new();
        font.set("BaseFont", Obj::name("Times-Roman"));
        font.set("FirstChar", Obj::Int(87));
        font.set("Widths", Obj::Array(vec![Obj::Int(500)]));
        let mut table = Dict::new();
        table.set("F1", Obj::Dict(font));
        let mut resources_dict = Dict::new();
        resources_dict.set("Font", Obj::Dict(table));
        let resources = Resources::from_dict(&resources_dict, &|o| Some(o.clone()));
        let widths = resources.font_widths("F1").expect("declared widths");
        assert_eq!(
            widths.width_of(87),
            Some(500),
            "the array is the file's answer and the built-in table must not replace it"
        );
    }

    #[test]
    fn invisible_text_advances_because_it_still_occupies_space() {
        // Rendering mode 3 draws nothing and moves the pen exactly as mode 0 does: text
        // that is not painted is not text that is not there.
        let declared = distinct_widths();
        let resources = font_resources(32, &declared);
        let shown = |mode: &str| {
            let stream = [
                b"BT /F1 10 Tf ".as_slice(),
                mode.as_bytes(),
                b" 0 0 Td (Wi) Tj (Wi) Tj ET",
            ]
            .concat();
            (glyph_x(&stream, &resources), final_x(&stream, &resources))
        };
        let (drawn, drawn_end) = shown("0 Tr");
        let (hidden, hidden_end) = shown("3 Tr");
        assert_eq!(drawn.len(), 4, "two runs of two glyphs");
        assert_eq!(hidden, drawn, "the same places, so also the same advances");
        assert!(near(hidden_end, drawn_end), "and the pen ends where it did");
    }

    #[test]
    fn a_code_outside_the_declared_run_falls_back_to_half_an_em() {
        // The run starts at `B`, so `A` is below it and the font says nothing about it.
        let resources = font_resources(66, &[400, 400]);
        let xs = glyph_x(b"BT /F1 1000 Tf 0 0 Td (ABA) Tj ET", &resources);
        assert_eq!(xs.len(), 3);
        assert!(
            near(advance(xs[0], xs[1], 1000.0), 500.0),
            "half an em for `A`, which is outside the run"
        );
        assert!(
            near(advance(xs[1], xs[2], 1000.0), 400.0),
            "and the 400 the run declares for `B`"
        );
    }

    #[test]
    fn word_spacing_moves_the_space_and_nothing_else() {
        let declared = distinct_widths();
        let resources = font_resources(32, &declared);
        let at = |tw: &str| {
            let stream = [
                b"BT /F1 10 Tf ".as_slice(),
                tw.as_bytes(),
                b" 0 0 Td (a a) Tj ET",
            ]
            .concat();
            glyph_x(&stream, &resources)
        };
        let plain = at("0 Tw");
        let spaced = at("20 Tw");
        assert_eq!(plain.len(), 3);
        assert!(
            near(spaced[0], plain[0]),
            "the first `a` is not a space, so it does not move"
        );
        assert!(
            near(spaced[1], plain[1]),
            "and the space's own place is where it was put: {} against {}",
            spaced[1],
            plain[1]
        );
        assert!(
            near(spaced[2] - plain[2], 20.0),
            "the glyph after the space moved by the word spacing, which is in unscaled \
             text-space units: {} against {}",
            spaced[2],
            plain[2]
        );
    }

    #[test]
    fn a_glyph_of_half_an_em_advances_by_half_an_em_at_every_size() {
        // The text matrix is *not* scaled by the font size, so the size has to be applied
        // to the glyph's own width for the pen to move the right distance: a glyph of width
        // 500 advances half an em at every size, and half an em of ten points is five.
        // Every number below is what two independent renderers produce for the same file.
        let resources = even_resources(500);
        let gap = |size: &str| {
            let xs = glyph_x(&show_at(size, "", "AA"), &resources);
            assert_eq!(xs.len(), 2, "one placement per glyph");
            xs[1] - xs[0]
        };
        for (size, want) in [("1", 0.5), ("10", 5.0), ("24", 12.0), ("100", 50.0)] {
            assert!(
                near(gap(size), want),
                "half an em at a size of {size} is {want}: {}",
                gap(size)
            );
        }
        // The text matrix is left where the pen walked, which is the same place the
        // placements were composed from. Two glyphs is two half ems.
        assert!(
            near(final_x(&show_at("10", "", "AA"), &resources), 10.0),
            "the pen ends a whole em along at a size of ten"
        );
    }

    #[test]
    fn the_font_size_scales_the_glyph_and_not_the_position() {
        // The two halves of the model, told apart. The *placement* grows with the size,
        // because the font scale is in the rendering matrix. A `Td`, which sets the text
        // matrix, does not: `Td 10 0` puts the text ten units along at every size. This is
        // the asymmetry that makes the composition order matter, and it is what a `Td` in a
        // real file means.
        let resources = even_resources(500);
        let origin = |size: &str| glyph_x(&show_at(size, "", "A"), &resources)[0];
        assert!(
            near(origin("1"), 0.0),
            "at the origin, whichever size it is"
        );
        let placed = |size: &str| {
            let stream = [
                b"BT /F1 ".as_slice(),
                size.as_bytes(),
                b" Tf 10 0 Td (A) Tj ET".as_slice(),
            ]
            .concat();
            glyph_x(&stream, &resources)[0]
        };
        for size in ["1", "10", "100"] {
            assert!(
                near(placed(size), 10.0),
                "`Td 10 0` is ten units along at a size of {size}: {}",
                placed(size)
            );
        }
    }

    #[test]
    fn character_spacing_is_added_unscaled() {
        // `Tc` is stated in unscaled text-space units and is added as it stands, which is
        // why it is the one term in the advance with no size in it. The glyph's own width
        // carries the size; the spacing does not.
        let resources = even_resources(500);
        let gap = |size: &str, setup: &str| {
            let xs = glyph_x(&show_at(size, setup, "AA"), &resources);
            xs[1] - xs[0]
        };
        for (size, want) in [("1000", 500.0 + 100.0), ("100", 50.0 + 100.0)] {
            assert!(
                near(gap(size, "100 Tc"), want),
                "a width of 500 at a size of {size} plus 100 of character spacing: {}",
                gap(size, "100 Tc")
            );
            assert!(
                near(gap(size, ""), want - 100.0),
                "and the same width without it, so the 100 is what moved the glyph"
            );
        }
    }

    #[test]
    fn the_horizontal_scale_scales_the_whole_advance() {
        // `Tz` multiplies the finished advance, spacing included, rather than each term:
        // the specification's formula is one factor over the sum.
        let resources = even_resources(500);
        let gap = |setup: &str| {
            let xs = glyph_x(&show_at("24", setup, "AA"), &resources);
            xs[1] - xs[0]
        };
        assert!(near(gap(""), 12.0), "half an em at 24pt: {}", gap(""));
        assert!(
            near(gap("50 Tz"), 6.0),
            "and half of it at 50%: {}",
            gap("50 Tz")
        );
        assert!(
            near(gap("200 Tz"), 24.0),
            "and twice it at 200%: {}",
            gap("200 Tz")
        );
        assert!(
            near(gap("50 Tz 100 Tc"), 6.0 + 50.0),
            "with the character spacing scaled too: {}",
            gap("50 Tz 100 Tc")
        );
    }

    #[test]
    fn word_spacing_applies_to_code_32_and_to_nothing_else() {
        // Two runs of the same three glyphs, one of `A B` and one of `AB `. The codes are
        // the same three and the widths are equal, so the only thing that can tell them
        // apart is the word spacing, and it is worth exactly the word spacing.
        let resources = even_resources(500);
        let end = |text: &str, setup: &str| final_x(&show_at("10", setup, text), &resources);
        let longer = |text: &str| end(text, "20 Tw") - end(text, "0 Tw");
        assert!(
            near(longer("A B"), 20.0),
            "the run with the space in it is a word spacing longer: {}",
            end("A B", "20 Tw")
        );
        // A run with no code 32 in it gets no such term, however it is spaced out. `A B`
        // and `ABA` are three glyphs of the same declared width, so the two differ by the
        // word spacing and by nothing else.
        assert!(
            near(longer("ABA"), 0.0),
            "a run with no code 32 is unaffected: {}",
            end("ABA", "20 Tw")
        );
        assert!(
            !near(longer("A B"), longer("ABA")),
            "so the term is the word spacing and not the space's own width"
        );
    }

    #[test]
    fn a_tj_kern_is_in_thousandths_of_an_em() {
        // A `TJ` number is a displacement of the pen in thousandths of an em of unscaled
        // text space, so it scales with the size and subtracts. `-500` at 24 points is half
        // an em of 12 units, and the second `A` lands at 24 rather than 12.
        let resources = even_resources(500);
        let stream = |size: &str| {
            [
                b"BT /F1 ".as_slice(),
                size.as_bytes(),
                b" Tf 0 0 Td [(A) -500 (A)] TJ ET".as_slice(),
            ]
            .concat()
        };
        let second = |size: &str| glyph_x(&stream(size), &resources)[1];
        assert!(
            near(second("24"), 24.0),
            "a width of 12 plus half an em of 12: {}",
            second("24")
        );
        assert!(
            near(second("100"), 100.0),
            "and fifty plus fifty at a size of 100: {}",
            second("100")
        );
        // And the sign is the whole content of the rule: positive pulls the glyph closer.
        let positive = [b"BT /F1 24 Tf 0 0 Td [(A) 500 (A)] TJ ET".as_slice()].concat();
        assert!(
            glyph_x(&positive, &resources)[1] < second("24"),
            "a positive number pulls the following glyph closer"
        );
    }

    #[test]
    fn a_zero_font_size_produces_no_infinities() {
        // Nothing in the advance divides by the size, so a missing or zero size costs
        // nothing and must not put an infinity into a matrix a renderer will read.
        let resources = even_resources(500);
        let stream = show_at("0", "100 Tc 100 Tw", "AB");
        let out = run_with(&ContentStream::parse(&stream), &resources);
        let Mark::Glyphs { placements, .. } = &out.records.first().expect("a mark").mark else {
            panic!("expected glyphs");
        };
        assert_eq!(placements.len(), 2);
        for placement in placements {
            for v in placement.to_array() {
                assert!(v.is_finite(), "a finite placement, not {v}");
            }
        }
        let x = final_x(&stream, &resources);
        assert!(x.is_finite(), "and a finite text matrix, not {x}");
        assert!(
            near(x, 200.0),
            "zero glyphs of width at a size of zero, plus the two spacing terms: {x}"
        );
    }

    #[test]
    fn a_second_show_continues_from_the_where_the_first_ended() {
        // The pen carries across `Tj` operators, which is the whole point of advancing the
        // text matrix rather than recomputing from the line matrix.
        let resources = even_resources(500);
        let first = show_at("24", "", "AB");
        // The run is 24 units wide, so the next show begins there — not where the last
        // glyph was *placed*, which is only 12, since the pen has moved on since.
        let run = final_x(&first, &resources);
        assert!(near(run, 24.0), "two half-em glyphs at a size of 24: {run}");
        let continued = glyph_x(b"BT /F1 24 Tf 0 0 Td (AB) Tj (CD) Tj ET", &resources);
        assert_eq!(continued.len(), 4, "two shows of two glyphs");
        assert!(
            near(continued[2], run),
            "the second show starts where the first ended: {} against {run}",
            continued[2]
        );
        // And the line matrix does not move, so a `Td` starts a new line from the beginning
        // of this one rather than from the end of the text on it.
        let new_line = glyph_x(
            b"BT /F1 24 Tf 0 0 Td (AB) Tj 0 -50 Td (CD) Tj ET",
            &resources,
        );
        assert!(
            near(new_line[2], 0.0),
            "a `Td` is relative to the line matrix: {}",
            new_line[2]
        );
    }

    #[test]
    fn a_transformation_applies_to_what_follows_it() {
        let out = run_bytes(b"q 2 0 0 2 0 0 cm 10 10 m 20 20 l S Q");
        let rec = out.records.first().expect("one mark");
        assert!(near(rec.ctm.a, 2.0));
        let Mark::Path { segments, .. } = &rec.mark else {
            panic!("expected a path");
        };
        assert!(
            matches!(segments.first(), Some(PathSegment::Move(x, y)) if near(*x, 20.0) && near(*y, 20.0))
        );
    }

    #[test]
    fn q_and_q_restore_the_transformation() {
        let out = run_bytes(b"q 5 0 0 5 0 0 cm Q 1 0 0 1 0 0 cm 1 1 m 2 2 l S");
        let rec = out.records.first().expect("one mark");
        assert!(
            rec.ctm.is_identity(),
            "the `Q` restored the matrix before the second `cm`"
        );
    }

    #[test]
    fn a_rectangle_becomes_a_closed_four_segment_path() {
        let out = run_bytes(b"10 20 30 40 re f");
        let Mark::Path { segments, .. } = &out.records.first().expect("a mark").mark else {
            panic!("expected a path");
        };
        assert_eq!(segments.len(), 5, "four sides and a close");
        assert!(segments.contains(&PathSegment::Close));
        // A rectangle at (10, 20) 30 by 40 is walked from its lower left corner:
        // (10, 20) -> (40, 20) -> (40, 60) -> (10, 60), then closed.
        let corners: Vec<(f64, f64)> = segments
            .iter()
            .filter_map(|s| match *s {
                PathSegment::Move(x, y) | PathSegment::Line(x, y) => Some((x, y)),
                _ => None,
            })
            .collect();
        assert_eq!(corners.len(), 4);
        for (got, want) in
            corners
                .iter()
                .zip([(10.0, 20.0), (40.0, 20.0), (40.0, 60.0), (10.0, 60.0)])
        {
            assert!(
                near(got.0, want.0) && near(got.1, want.1),
                "got {got:?}, wanted {want:?}"
            );
        }
    }

    #[test]
    fn v_repeats_the_current_point_as_the_first_control() {
        let out = run_bytes(b"0 0 m 10 10 20 0 v S");
        let Mark::Path { segments, .. } = &out.records.first().expect("a mark").mark else {
            panic!("expected a path");
        };
        let Some(PathSegment::Curve(x1, y1, ..)) = segments.get(1) else {
            panic!("expected a curve");
        };
        assert!(
            near(*x1, 0.0) && near(*y1, 0.0),
            "v repeats the current point"
        );
    }

    #[test]
    fn y_repeats_the_current_point_as_the_first_control() {
        // `y` is `c` without the first control point, and its four operands are
        // x2 y2 x3 y3.
        let out = run_bytes(b"0 0 m 10 10 20 0 y S");
        let Mark::Path { segments, .. } = &out.records.first().expect("a mark").mark else {
            panic!("expected a path");
        };
        let Some(PathSegment::Curve(x1, y1, x2, y2, x3, y3)) = segments.get(1) else {
            panic!("expected a curve");
        };
        assert!(
            near(*x1, 0.0) && near(*y1, 0.0),
            "y implies the current point"
        );
        assert!(near(*x2, 10.0) && near(*y2, 10.0), "got ({x2}, {y2})");
        assert!(near(*x3, 20.0) && near(*y3, 0.0));
    }

    #[test]
    fn painting_ends_the_path() {
        let out = run_bytes(b"0 0 m 10 10 l S S");
        assert_eq!(
            out.records.len(),
            1,
            "the second `S` has an empty path and draws nothing"
        );
    }

    #[test]
    fn the_fill_rule_follows_the_operator() {
        let even = run_bytes(b"0 0 m 1 0 l 1 1 l h f*");
        let Mark::Path { rule, .. } = &even.records.first().expect("a mark").mark else {
            panic!("expected a path");
        };
        assert_eq!(*rule, FillRule::EvenOdd);

        let non_zero = run_bytes(b"0 0 m 1 0 l 1 1 l h f");
        let Mark::Path { rule, .. } = &non_zero.records.first().expect("a mark").mark else {
            panic!("expected a path");
        };
        assert_eq!(*rule, FillRule::NonZero);
    }

    #[test]
    fn a_colour_operator_fills_the_right_slot() {
        // `B` fills and strokes, so both slots are set; `S` alone would leave the fill
        // at whatever it was, which is the specification's behaviour and worth stating.
        let out = run_bytes(b"1 0 0 rg 0 0 1 RG 0 0 m 1 1 l B");
        let rec = out.records.first().expect("a mark");
        let Mark::Path { fill, stroke, .. } = &rec.mark else {
            panic!("expected a path");
        };
        let f = fill.as_ref().expect("a fill");
        assert_eq!(f.components, vec![1.0, 0.0, 0.0], "rg set the fill");
        let s = stroke.as_ref().expect("a stroke");
        assert_eq!(s.components, vec![0.0, 0.0, 1.0], "RG set the stroke");
    }

    #[test]
    fn a_colour_operator_takes_the_count_its_space_needs() {
        // A stream that sets `1 0 0 rg` then `0.5 g` is switching spaces; the grey
        // operator must produce one component, not three.
        let out = run_bytes(b"0.5 g 0 0 m 1 1 l f");
        let Mark::Path { fill: Some(f), .. } = &out.records.first().expect("a mark").mark else {
            panic!("expected a filled path");
        };
        assert_eq!(f.space.name, "DeviceGray");
        assert_eq!(f.components, vec![0.5]);
    }

    /// Resources whose `/ColorSpace` table names one ICC-based space `CS0`, over a profile
    /// that declares `alternate` and `n`.
    ///
    /// The profile is a stream object rather than a reference because the resolver these
    /// tests use hands back what it is given; every other part of this is as a file writes
    /// it, and the page's `cs` sees a name and not a colour space.
    fn icc_resources(alternate: Option<&str>, n: i64) -> Resources {
        let mut profile = Dict::new();
        profile.set("N", Obj::Int(n));
        if let Some(alternate) = alternate {
            profile.set("Alternate", Obj::name(alternate));
        }
        let mut table = Dict::new();
        table.set(
            "CS0",
            Obj::Array(vec![
                Obj::name("ICCBased"),
                Obj::Stream(Stream::new(profile, vec![0u8; 4])),
            ]),
        );
        let mut resources = Dict::new();
        resources.set("ColorSpace", Obj::Dict(table));
        Resources::from_dict(&resources, &|o| Some(o.clone()))
    }

    #[test]
    fn cs_carries_the_icc_profile_with_the_colour() {
        // The interpreter holds no document, so `cs` is the only place the resource table can
        // be consulted. If what the profile declares is not read here and carried from here,
        // every later stage sees a name and nothing else — which is what made a whole file
        // blank.
        let out = run_with(
            &ContentStream::parse(b"/CS0 cs 0 1 0 sc 0 0 1 1 re f"),
            &icc_resources(Some("DeviceRGB"), 3),
        );
        let Mark::Path { fill: Some(f), .. } = &out.records.first().expect("a mark").mark else {
            panic!("expected a filled path");
        };
        assert_eq!(
            f.space.name, "CS0",
            "the name the stream used is what a report leads with"
        );
        let icc = f.space.icc.as_ref().expect("the profile came with it");
        assert_eq!(icc.alternate.as_deref(), Some("DeviceRGB"));
        assert_eq!(icc.components, Some(3));
        assert_eq!(
            f.to_rgba(None).map(|c| (c.r, c.g, c.b)),
            Some((0.0, 1.0, 0.0)),
            "and it converts, which is the whole point of carrying it"
        );
    }

    #[test]
    fn an_icc_space_with_no_alternate_reaches_the_conversion_as_a_refusal() {
        let out = run_with(
            &ContentStream::parse(b"/CS0 cs 0 1 0 sc 0 0 1 1 re f"),
            &icc_resources(None, 3),
        );
        let Mark::Path { fill: Some(f), .. } = &out.records.first().expect("a mark").mark else {
            panic!("expected a filled path");
        };
        assert!(
            f.space
                .icc
                .as_ref()
                .expect("it is still an ICC space")
                .alternate
                .is_none(),
            "the profile that names no alternate is recorded as one that names none"
        );
        assert!(
            f.to_rgba(None).is_none(),
            "so the conversion refuses rather than guessing a space"
        );
    }

    #[test]
    fn a_one_or_four_component_profile_reaches_the_conversion_at_that_width() {
        // The count comes from the profile's `/N`, and the components are read at that width:
        // a one-component stream set with `sc` has one operand, and a four-component one has
        // four, so a conversion that guessed the width would drop or pad the colour.
        for (n, operands, want) in [
            (1i64, "0.25", (0.25, 0.25, 0.25)),
            (4i64, "0 1 1 0", (1.0, 0.0, 0.0)),
        ] {
            let alternate = if n == 1 { "DeviceGray" } else { "DeviceCMYK" };
            let content = format!("/CS0 cs {operands} sc 0 0 1 1 re f");
            let out = run_with(
                &ContentStream::parse(content.as_bytes()),
                &icc_resources(Some(alternate), n),
            );
            let Mark::Path { fill: Some(f), .. } = &out.records.first().expect("a mark").mark
            else {
                panic!("expected a filled path");
            };
            assert_eq!(f.components.len(), n as usize, "{n} components are kept");
            let rgba = f.to_rgba(None).expect("the alternate converts");
            assert_eq!((rgba.r, rgba.g, rgba.b), want, "an /N {n} profile");
        }
    }

    #[test]
    fn a_name_that_is_not_an_icc_space_carries_no_profile() {
        let out = run_with(
            &ContentStream::parse(b"/DeviceGray cs 0.5 sc 0 0 1 1 re f"),
            &icc_resources(Some("DeviceRGB"), 3),
        );
        let Mark::Path { fill: Some(f), .. } = &out.records.first().expect("a mark").mark else {
            panic!("expected a filled path");
        };
        assert!(
            f.space.icc.is_none(),
            "an ordinary space is unchanged: it has no profile and needs none"
        );
        assert_eq!(f.space.name, "DeviceGray");
        assert!(f.to_rgba(None).is_some());
    }

    #[test]
    fn a_line_width_is_reported_as_the_user_sees_it() {
        let out = run_bytes(b"4 w 2 0 0 2 0 0 cm 0 0 m 1 1 l S");
        let rec = out.records.first().expect("a mark");
        assert!(
            near(rec.device_line_width, 8.0),
            "got {}",
            rec.device_line_width
        );
    }

    #[test]
    fn a_clip_narrows_and_the_narrowing_is_visible() {
        let out = run_bytes(b"0 0 100 100 re W n 0 0 m 10 10 l S");
        let rec = out.records.first().expect("a mark");
        let clip = rec.clip.as_ref().expect("a clip");
        assert!(near(clip.bounds.x0, 0.0) && near(clip.bounds.x1, 100.0));
        // The mark records that the clip changed rather than the path it changed it
        // with, because the path is not drawn.
        assert!(matches!(rec.mark, Mark::ClipChanged(_)));
    }

    #[test]
    fn two_clips_intersect() {
        let out = run_bytes(b"q 0 0 100 100 re W n 0 0 10 10 re W n 0 0 m 1 1 l S Q");
        // One record per clipping operator, then the stroke inside both.
        let clips: Vec<ClipBounds> = out
            .records
            .iter()
            .filter_map(|r| r.clip.as_ref().map(|c| c.bounds))
            .collect();
        assert_eq!(clips.len(), 3, "two clips and a stroke");
        let widths: Vec<f64> = clips.iter().map(|c| c.x1 - c.x0).collect();
        // The first clip is 100 wide; the second is the intersection with a 10-wide
        // rectangle, not a replacement; and the stroke inside both sees the narrower one.
        assert_eq!(widths, vec![100.0, 10.0, 10.0]);

        // And both paths are still on the record, not only the newer one. The box alone
        // cannot say where the region is: the older clip is a rectangle here, but a page may
        // clip to a shape whose box is most of the page, and a renderer handed only the newer
        // path would paint the older one wider than the page asked for.
        let depths: Vec<usize> = out
            .records
            .iter()
            .filter_map(|r| r.clip.as_ref().map(|c| c.paths.len()))
            .collect();
        assert_eq!(
            depths,
            vec![1, 2, 2],
            "the region carries every path in force, and the mark inside both carries both"
        );
        // The newest is the one that was set last, which is what a reader is asking about.
        let inside = out.records[2].clip.as_ref().expect("the stroke's clip");
        let newest = inside.newest().expect("the newer path");
        assert_eq!(newest.segments.first(), Some(&PathSegment::Move(0.0, 0.0)));
        assert_eq!(newest.segments.get(1), Some(&PathSegment::Line(10.0, 0.0)));
    }

    /// The region is the path, not only its box: a clip set from a rectangle carries that
    /// rectangle, so a renderer can draw to the region rather than to the bound around it.
    #[test]
    fn a_clip_from_a_rectangle_records_that_rectangle_as_its_path() {
        let out = run_bytes(b"0 0 100 60 re W n 0 0 m 1 1 l S");
        let clip = out
            .records
            .first()
            .and_then(|r| r.clip.as_ref())
            .expect("a clip");
        let newest = clip.newest().expect("the path the clip was set from");
        assert_eq!(newest.rule, FillRule::NonZero);
        assert!(!clip.is_empty(), "a rectangle is not an empty region");
        let corners: Vec<(f64, f64)> = newest
            .segments
            .iter()
            .filter_map(|s| match *s {
                PathSegment::Move(x, y) | PathSegment::Line(x, y) => Some((x, y)),
                _ => None,
            })
            .collect();
        // The four corners a `re` puts down, in the order it puts them down, and the box
        // they make. Both come from one path, so they cannot disagree.
        assert_eq!(
            corners,
            vec![(0.0, 0.0), (100.0, 0.0), (100.0, 60.0), (0.0, 60.0)]
        );
        assert!(near(clip.bounds.x0, 0.0) && near(clip.bounds.x1, 100.0));
        assert!(near(clip.bounds.y0, 0.0) && near(clip.bounds.y1, 60.0));
    }

    /// `W n` with no path is a clip to *nothing*, not no clip at all.
    ///
    /// This is where a clip that is a box and a clip that is a region come apart. If an
    /// empty path were read as "no clip", `None` would reach the renderer and the page would
    /// be drawn in full — the opposite of what the specification says, and a page that looks
    /// fine until someone looks for the box the page said it was clipping to.
    #[test]
    fn a_clip_to_an_empty_path_is_an_empty_clip_rather_than_no_clip() {
        let out = run_bytes(b"W n 0 0 100 100 re f 0 0 m 1 1 l S");
        let first = out.records.first().expect("the clip mark");
        let clip = first.clip.as_ref().expect("a clip, not the absence of one");
        assert!(clip.is_empty(), "an empty path clips everything away");
        assert!(clip.paths.is_empty(), "and it has no path");
        // The fill after it carries the same empty clip, because the clip is in force.
        let fill = out.records.get(1).expect("the fill");
        let Mark::Path { .. } = &fill.mark else {
            panic!("the second mark should be the fill");
        };
        assert!(
            fill.clip.as_ref().is_some_and(|c| c.is_empty()),
            "the empty clip is still in force for the fill"
        );
        // And so is it for the stroke, which is drawn against nothing.
        let stroke = out.records.get(2).expect("the stroke");
        assert!(
            stroke.clip.as_ref().is_some_and(|c| c.is_empty()),
            "and for the stroke"
        );
    }

    /// `Q` restores the clip that was in force before the `q`, so an empty clip does not
    /// outlive the state it was set in.
    #[test]
    fn an_empty_clip_is_restored_by_q() {
        let out = run_bytes(b"q W n 0 0 10 10 re f Q 0 0 10 10 re f");
        // Three marks: the clip, the fill inside `q`, and the fill after `Q`.
        assert_eq!(out.records.len(), 3);
        let inside = out.records.get(1).expect("the fill inside q");
        assert!(inside.clip.as_ref().is_some_and(|c| c.is_empty()));
        let after = out.records.get(2).expect("the fill after Q");
        assert!(after.clip.is_none(), "Q restores the clip, which was none");
    }

    #[test]
    fn text_marks_carry_the_font_and_the_placement() {
        let out = run_bytes(b"BT /F1 24 Tf 10 20 Td (Hi) Tj ET");
        let rec = out.records.first().expect("a mark");
        let Mark::Glyphs {
            font,
            size,
            text,
            placements,
            ..
        } = &rec.mark
        else {
            panic!("expected glyphs");
        };
        assert_eq!(font.as_deref(), Some("F1"));
        assert!(near(*size, 24.0));
        assert_eq!(text, b"Hi");
        assert_eq!(placements.len(), 2, "one placement per glyph");
        let (x, y) = placements[0].apply(0.0, 0.0);
        assert!(
            near(x, 10.0) && near(y, 20.0),
            "the first glyph is where `Td` put it"
        );
    }

    #[test]
    fn a_composite_font_places_one_glyph_per_two_byte_code() {
        // Four bytes, two codes, two glyphs. The count is the point: reading one glyph per
        // byte is what puts every glyph after the first in the wrong place on a page set
        // in a composite font, and it does it without any visible error to notice.
        let resources = composite_resources(
            vec![
                Obj::Int(0x0041),
                Obj::Array(vec![Obj::Int(500), Obj::Int(600)]),
            ],
            1000,
        );
        let stream = b"BT /F1 10 Tf 0 0 Td <00410042> Tj ET";
        assert_eq!(
            placement_count(stream, &resources),
            2,
            "two two-byte codes are two glyphs"
        );
        assert_eq!(
            shown_codes(stream, &resources),
            vec![0x0041, 0x0042],
            "each code is the pair of bytes read as one number, high byte first"
        );
    }

    #[test]
    fn a_composite_glyph_advances_by_its_own_entry_in_the_run_length_widths() {
        // Two codes of different declared widths. The pen must move by their sum, which is
        // only true if each glyph was looked up by its own code: a single figure for the
        // run, or one lookup per byte, would land somewhere else entirely.
        let resources = composite_resources(
            vec![
                Obj::Int(0x0041),
                Obj::Array(vec![Obj::Int(500), Obj::Int(600)]),
            ],
            1000,
        );
        let size = 10.0;
        let stream = b"BT /F1 10 Tf 0 0 Td <00410042> Tj ET";
        let xs = glyph_x(stream, &resources);
        assert_eq!(xs.len(), 2, "two glyphs");
        // The gap between two placements is the advance of the glyph *before* the second
        // one, so this is code 0x0041's own 500 and not a single figure for the pair.
        assert!(
            near(advance(xs[0], xs[1], size), 0.5 * size),
            "the second glyph starts one 500/1000 em after the first: {} against {}",
            advance(xs[0], xs[1], size),
            0.5 * size
        );
        let end = final_x(stream, &resources);
        let want = (0.5 + 0.6) * size;
        assert!(
            near(end, want),
            "and the pen ends at the sum of the two widths: {end} against {want}"
        );
    }

    #[test]
    fn a_composite_font_reads_both_shapes_of_run_length_widths() {
        // One listed run and one ranged run, in the same `/W`, with the two forms on either
        // side of the boundary between them.
        let resources = composite_resources(
            vec![
                // 0x0010..=0x0011 listed, 400 and 500.
                Obj::Int(0x0010),
                Obj::Array(vec![Obj::Int(400), Obj::Int(500)]),
                // 0x0020..=0x0022 ranged, all 700.
                Obj::Int(0x0020),
                Obj::Int(0x0022),
                Obj::Int(700),
            ],
            1000,
        );
        let size = 10.0;
        // Five codes: two from the listed run and three from the ranged one, so the
        // boundary between the two forms falls between the second and the third glyph.
        let stream = b"BT /F1 10 Tf 0 0 Td <00100011002000210022> Tj ET";
        let xs = glyph_x(stream, &resources);
        assert_eq!(xs.len(), 5, "five two-byte codes are five glyphs");
        let gaps = [
            advance(xs[0], xs[1], size),
            advance(xs[1], xs[2], size),
            advance(xs[2], xs[3], size),
            advance(xs[3], xs[4], size),
        ];
        for (i, (got, want)) in gaps.iter().zip([0.4, 0.5, 0.7, 0.7]).enumerate() {
            assert!(
                near(*got, want * size),
                "gap {i} is the width of the code before it: {got} against {}",
                want * size
            );
        }
        // A code past every run takes `/DW`, which is the specification's answer rather than
        // a fallback invented here.
        let past = final_x(b"BT /F1 10 Tf 0 0 Td <0099> Tj ET", &resources);
        assert!(
            near(past, size),
            "a code no run covers advances by one em, which is /DW: {past}"
        );
    }

    #[test]
    fn a_composite_font_with_only_a_default_width_advances_every_glyph_by_it() {
        // No `/W` at all, only `/DW`. The codes are still two bytes — the code width comes
        // from the font dictionary and not from the widths, which is exactly the case that
        // breaks if a caller infers one from the other.
        let resources = composite_default_only(750);
        let stream = b"BT /F1 10 Tf 0 0 Td <00410042> Tj ET";
        let xs = glyph_x(stream, &resources);
        assert_eq!(xs.len(), 2, "still two glyphs, with no /W to say otherwise");
        assert!(
            near(advance(xs[0], xs[1], 10.0), 0.75 * 10.0),
            "each by /DW: {}",
            advance(xs[0], xs[1], 10.0)
        );
    }

    #[test]
    fn a_truncated_two_byte_string_drops_its_last_byte_rather_than_inventing_a_glyph() {
        // Three bytes: one whole code and half of another. The half is not a code, so it
        // names no glyph, and padding it out would put a character on the page that the
        // file never asked for. A literal string is the way to get an odd byte count: a
        // hex string with an odd number of digits is padded to a whole byte by the
        // specification, so `<0041004>` is four bytes and two codes, not three.
        let resources = composite_resources(
            vec![
                Obj::Int(0x0041),
                Obj::Array(vec![Obj::Int(500), Obj::Int(600)]),
            ],
            1000,
        );
        let stream = b"BT /F1 10 Tf 0 0 Td (ABC) Tj ET";
        assert_eq!(
            placement_count(stream, &resources),
            1,
            "one whole code is one glyph, and the stray byte is not a second"
        );
        assert_eq!(
            shown_codes(stream, &resources),
            vec![0x4142],
            "the two whole bytes are one code, read high byte first"
        );
        // A string of a single byte is the same case with nothing whole in it: no glyph,
        // and no panic.
        let lone = b"BT /F1 10 Tf 0 0 Td (A) Tj ET";
        let out = run_with(&ContentStream::parse(lone), &resources);
        assert!(
            !out.records
                .iter()
                .any(|r| matches!(r.mark, Mark::Glyphs { .. })),
            "half a code is not a glyph, so there is no mark at all"
        );
        // The bytes themselves are still carried, because an edit rewrites the bytes and a
        // dropped one would be a dropped character as far as the file is concerned.
        let whole = b"BT /F1 10 Tf 0 0 Td (ABC) Tj ET";
        let out = run_with(&ContentStream::parse(whole), &resources);
        let Some(Mark::Glyphs { text, .. }) = out.records.first().map(|r| &r.mark) else {
            panic!("a text mark");
        };
        assert_eq!(text, b"ABC", "all three bytes, and one glyph");
    }

    #[test]
    fn a_single_byte_font_is_unchanged_by_any_of_this() {
        // The common case, pinned against the width computed from the same dictionary: a
        // Type 1 font declares `/FirstChar` and a flat `/Widths`, and each code's advance
        // is its own entry. If the two-byte path had leaked into this one, the gap between
        // the first two glyphs would be the width of the first code twice over.
        let declared = distinct_widths();
        let resources = font_resources(32, &declared);
        let width_of = |code: u8| {
            let offset = usize::try_from(i64::from(code) - 32).expect("in the declared run");
            declared.get(offset).copied().expect("a declared width")
        };
        let size = 12.0;
        let stream = b"BT /F1 12 Tf 0 0 Td (AB) Tj ET";
        let xs = glyph_x(stream, &resources);
        assert_eq!(xs.len(), 2, "one glyph per byte, as it always was");
        assert_eq!(
            shown_codes(stream, &resources),
            vec![65, 66],
            "and the codes are the bytes"
        );
        let want = width_of(b'A') as f64 / 1000.0 * size;
        assert!(
            near(advance(xs[0], xs[1], size), want),
            "the second glyph starts one A-width after the first: {} against {want}",
            advance(xs[0], xs[1], size)
        );
        let total = want + width_of(b'B') as f64 / 1000.0 * size;
        assert!(
            near(final_x(stream, &resources), total),
            "and the pen ends at the sum of both widths: {} against {total}",
            final_x(stream, &resources)
        );
    }

    #[test]
    fn word_spacing_does_not_apply_to_a_two_byte_code() {
        // The specification's word spacing is for the *single-byte* code 32. A composite
        // font's code 32 is 0x0020, a different thing, and adding word spacing to it would
        // widen a space the file never asked to be wide.
        let simple = even_resources(500);
        // Every code is a half-em glyph, 0x0020 included, so the only thing that could widen
        // the middle one is the word spacing.
        let composite = composite_resources(
            vec![Obj::Int(0x0000), Obj::Int(0xFFFF), Obj::Int(500)],
            1000,
        );
        let show = |data: &[u8], resources: &Resources| {
            let stream = [b"BT /F1 10 Tf 8 Tw ".as_slice(), data, b" ET"].concat();
            final_x(&stream, resources)
        };
        // Three glyphs of half an em each, plus the word spacing on the middle one.
        assert!(
            near(show(b"0 0 Td (A A) Tj", &simple), 0.5 * 10.0 * 3.0 + 8.0),
            "a simple font's space gets the word spacing"
        );
        assert!(
            near(
                show(b"0 0 Td <004100200041> Tj", &composite),
                0.5 * 10.0 * 3.0
            ),
            "a composite font's 0x0020 does not"
        );
    }

    #[test]
    fn the_text_matrix_advances_so_two_shows_do_not_overlap() {
        let out = run_bytes(b"BT /F1 24 Tf 0 0 Td (AB) Tj (CD) Tj ET");
        assert_eq!(
            out.records.len(),
            2,
            "each show is its own mark, with its own span"
        );
        let second = out.records.get(1).expect("the second mark");
        let Mark::Glyphs { placements, .. } = &second.mark else {
            panic!("expected glyphs");
        };
        let x0 = placements
            .first()
            .map(|m| m.apply(0.0, 0.0).0)
            .unwrap_or(0.0);
        assert!(x0 > 0.0, "the second show starts after the first, at {x0}");
    }

    #[test]
    fn a_quote_moves_to_the_next_line_first() {
        let out = run_bytes(b"BT /F1 12 Tf 0 100 Td 14 TL (a) ' (b) ' ET");
        let marks: Vec<&Record> = out
            .records
            .iter()
            .filter(|r| matches!(r.mark, Mark::Glyphs { .. }))
            .collect();
        assert_eq!(marks.len(), 2);
        let Mark::Glyphs {
            placements: first, ..
        } = &marks[0].mark
        else {
            panic!("expected glyphs");
        };
        let Mark::Glyphs {
            placements: second, ..
        } = &marks[1].mark
        else {
            panic!("expected glyphs");
        };
        let (_, y0) = first[0].apply(0.0, 0.0);
        let (_, y1) = second[0].apply(0.0, 0.0);
        assert!(y1 < y0, "the leading moved down: {y1} < {y0}");
    }

    #[test]
    fn td_moves_and_td_does_the_same_and_sets_the_leading() {
        let out = run_bytes(b"BT /F1 12 Tf 0 100 Td 5 -7 TD 0 -30 Td (x) Tj ET");
        let rec = out.records.first().expect("a mark");
        let Mark::Glyphs { placements, .. } = &rec.mark else {
            panic!("expected glyphs");
        };
        let (x, y) = placements.first().expect("a placement").apply(0.0, 0.0);
        assert!(near(x, 5.0), "got {x}");
        assert!(near(y, 100.0 - 7.0 - 30.0), "got {y}");
        assert!(
            near(out.state.text.leading, 7.0),
            "`TD` sets the leading to the negation of the vertical move"
        );
    }

    #[test]
    fn tm_replaces_both_matrices() {
        let out = run_bytes(b"BT 1 0 0 1 50 60 Tm (x) Tj ET");
        let rec = out.records.first().expect("a mark");
        let Mark::Glyphs { placements, .. } = &rec.mark else {
            panic!("expected glyphs");
        };
        let (x, y) = placements.first().expect("a placement").apply(0.0, 0.0);
        assert!(near(x, 50.0) && near(y, 60.0), "got ({x}, {y})");
    }

    #[test]
    fn an_xobject_placement_carries_the_transformation() {
        let out = run_bytes(b"q 10 0 0 10 100 200 cm /Im1 Do Q");
        let rec = out.records.first().expect("a mark");
        let Mark::Image {
            name,
            matrix,
            inline,
            ..
        } = &rec.mark
        else {
            panic!("expected an image");
        };
        assert_eq!(name.as_deref(), Some("Im1"));
        assert!(!inline);
        assert!(near(matrix.e, 100.0) && near(matrix.f, 200.0));
    }

    #[test]
    fn an_inline_image_is_a_mark() {
        let out = run_bytes(b"q 20 0 0 20 0 0 cm BI /W 1 /H 1 /BPC 8 /CS /G ID \x00 EI Q");
        let rec = out.records.first().expect("a mark");
        let Mark::Image { name, inline, .. } = &rec.mark else {
            panic!("expected an image");
        };
        assert!(name.is_none(), "an inline image has no name");
        assert!(inline);
    }

    #[test]
    fn a_shading_carries_its_name_and_matrix() {
        let out = run_bytes(b"q 2 0 0 2 5 5 cm /Sh0 sh Q");
        let rec = out.records.first().expect("a mark");
        let Mark::Shading { name, matrix } = &rec.mark else {
            panic!("expected a shading");
        };
        assert_eq!(name, "Sh0");
        assert!(near(matrix.e, 5.0));
    }

    #[test]
    fn an_unknown_operator_is_reported_rather_than_dropped() {
        // A bare word, not `/Flakey99`: a name is an operand, a bare word is an operator.
        let out = run_bytes(b"0 0 m 1 1 l S Flakey99 1 0 0 1 0 0 cm");
        assert_eq!(out.unknown_operators.len(), 1);
        assert_eq!(out.unknown_operators[0].0, b"Flakey99");
    }

    #[test]
    fn a_gs_without_a_resource_table_is_a_note_not_a_silent_no_op() {
        let out = run_bytes(b"/GS1 gs 0 0 m 1 1 l S");
        assert!(
            out.notes
                .iter()
                .any(|n| n.contains("GS1") && n.contains("ExtGState")),
            "the note must name the state: {:?}",
            out.notes
        );
    }

    #[test]
    fn each_mark_carries_the_stroke_it_was_drawn_with() {
        // A thick line with one pattern, then a thin round one with another. Each mark
        // must remember which it was, because a renderer that read one style for the whole
        // page would draw them the same.
        let out = run_bytes(b"3 w [9] 0 d 0 0 m 10 0 l S 1 J 1 w [3 1] 2 d 0 20 m 10 20 l S");
        assert_eq!(out.records.len(), 2);
        let solid = out.records.first().expect("the first mark");
        let dashed = out.records.get(1).expect("the second mark");
        assert_eq!(solid.dash.array, vec![9.0], "the first had its own pattern");
        assert_eq!(dashed.dash.array, vec![3.0, 1.0], "the second was dashed");
        assert_eq!(dashed.line_cap, LineCap::Round, "and round-ended");
        assert!(
            near(dashed.device_line_width, 1.0),
            "the width was changed between them"
        );
        assert!(
            near(solid.device_line_width, 3.0),
            "and the first kept the width it was drawn with"
        );
    }

    // ── Form XObjects ────────────────────────────────────────────────────────────

    /// A form XObject: `/Subtype /Form`, the entries a form may carry, and its content.
    ///
    /// Written here rather than taken from a file so that what each entry *means* is what
    /// is under test: the three answers `/BBox` can have, a `/Matrix` that is absent or
    /// present or damaged, and a `/Resources` of the form's own or none at all.
    fn form(
        content: &str,
        bbox: Option<&str>,
        matrix: Option<&str>,
        resources: Option<Dict>,
    ) -> Obj {
        let mut dict = Dict::new();
        dict.set("Type", Obj::name("XObject"));
        dict.set("Subtype", Obj::name("Form"));
        if let Some(bbox) = bbox {
            let numbers: Vec<Obj> = bbox
                .split_whitespace()
                .map(|n| Obj::Real(n.parse::<f64>().unwrap_or(0.0)))
                .collect();
            dict.set("BBox", Obj::Array(numbers));
        }
        if let Some(matrix) = matrix {
            let numbers: Vec<Obj> = matrix
                .split_whitespace()
                .map(|n| Obj::Real(n.parse::<f64>().unwrap_or(0.0)))
                .collect();
            dict.set("Matrix", Obj::Array(numbers));
        }
        if let Some(resources) = resources {
            dict.set("Resources", Obj::Dict(resources));
        }
        Obj::Stream(Stream {
            dict,
            raw: content.as_bytes().to_vec(),
            file_offset: None,
            synthetic: true,
        })
    }

    /// A `/Resources` dictionary with one font named `F1` and the given declared widths.
    fn font_resources_dict(widths: &[i64]) -> Dict {
        let mut font = Dict::new();
        font.set("Type", Obj::name("Font"));
        font.set("BaseFont", Obj::name("NoSuchFont"));
        font.set("FirstChar", Obj::Int(32));
        font.set(
            "Widths",
            Obj::Array(widths.iter().map(|w| Obj::Int(*w)).collect()),
        );
        let mut table = Dict::new();
        table.set("F1", Obj::Dict(font));
        let mut resources = Dict::new();
        resources.set("Font", Obj::Dict(table));
        resources
    }

    /// A resources dictionary whose `/XObject` table names the forms given.
    ///
    /// Forms and page resources share one writer because the difference between them is
    /// *where the table is read from*, not what it is: a page's table names a form, and a
    /// form's table names the forms it draws.
    fn xobject_resources(forms: &[(&str, Obj)], fonts: Option<&[i64]>) -> Resources {
        let mut table = Dict::new();
        for (name, object) in forms {
            table.set(name, object.clone());
        }
        let mut resources = Dict::new();
        resources.set("XObject", Obj::Dict(table));
        if let Some(widths) = fonts {
            let fonts = font_resources_dict(widths);
            resources.set("Font", fonts.get("Font").cloned().unwrap_or(Obj::Null));
        }
        Resources::from_dict(&resources, &|o| Some(o.clone()))
    }

    /// The corners of a path mark, in order, closing point included.
    fn corners(record: &Record) -> Vec<(f64, f64)> {
        let Mark::Path { segments, .. } = &record.mark else {
            panic!("expected a path");
        };
        segments
            .iter()
            .filter_map(|s| match *s {
                PathSegment::Move(x, y) | PathSegment::Line(x, y) => Some((x, y)),
                _ => None,
            })
            .collect()
    }

    /// The bounds of a path mark, as its own points put it.
    fn extent(points: &[(f64, f64)]) -> ClipBounds {
        let x0 = points.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
        let x1 = points.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
        let y0 = points.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
        let y1 = points.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
        ClipBounds { x0, y0, x1, y1 }
    }

    /// A form drawing a filled rectangle, and a page that puts it somewhere.
    ///
    /// The page is asymmetric on purpose: the form's square is drawn under a `cm` that
    /// both moves and scales it, so a reader that used the wrong matrix — the page's, the
    /// form's, or neither — puts a square of the wrong size in the wrong place, and none of
    /// the three mistakes lands where this says.
    fn rectangle_form_page(matrix: Option<&str>, bbox: Option<&str>) -> PageContent {
        let resources =
            xobject_resources(&[("Fm0", form("0 0 20 10 re f", bbox, matrix, None))], None);
        run_with(
            &ContentStream::parse(b"q 100 0 0 100 50 60 cm /Fm0 Do Q"),
            &resources,
        )
    }

    #[test]
    fn a_form_is_executed_as_a_nested_stream_rather_than_read_as_an_image() {
        // The whole defect in one assertion: a `Do` of a `/Subtype /Form` XObject used to
        // be recorded as `Mark::Image`, which is how a form became the note "an image
        // claims to be 0 by 0 pixels and was not drawn" — a finding about a picture where
        // there was a content stream.
        let out = rectangle_form_page(None, None);
        assert_eq!(
            out.records.len(),
            1,
            "the form's one fill is the page's one mark: {:?}",
            out.records.iter().map(|r| &r.mark).collect::<Vec<_>>()
        );
        let record = out.records.first().expect("the form's mark");
        assert!(
            matches!(record.mark, Mark::Path { .. }),
            "a path, not an image: {:?}",
            record.mark
        );
        assert_eq!(
            record.form.as_deref(),
            Some("Fm0"),
            "and it knows which form drew it, which is what its resources are looked up in"
        );
        // The page's own transformation, and only that: the form declares no `/Matrix`.
        // A 20 by 10 rectangle at the origin becomes 2000 by 1000 with its lower left at
        // (50, 60) — a hundred times the page's own scale, plus the page's own move.
        let points = corners(record);
        assert_eq!(points.len(), 4, "four sides");
        let b = extent(&points);
        assert!(
            near(b.x0, 50.0) && near(b.y0, 60.0) && near(b.x1, 2050.0) && near(b.y1, 1060.0),
            "got {b:?}"
        );
    }

    #[test]
    fn a_forms_matrix_composes_with_the_page_transformation() {
        // Each of the three things a `/Matrix` can do, checked against the **closed** path
        // rather than against a snapshot of a picture: the rectangle is still a rectangle,
        // and its sides are the page's `cm` and the form's `/Matrix` composed. A scale, a
        // quarter turn and a move in one page each, because a reader that dropped the
        // matrix entirely, or applied it in the wrong order, or treated it as the page's,
        // gets one of these three wrong while leaving the other two looking right.
        let scale = rectangle_form_page(Some("2 0 0 2 0 0"), None);
        let scaled = extent(&corners(scale.records.first().expect("a mark")));
        assert!(
            near(scaled.x0, 50.0)
                && near(scaled.y0, 60.0)
                && near(scaled.x1, 4050.0)
                && near(scaled.y1, 2060.0),
            "doubling in the form's own space, then the page's hundredfold: {scaled:?}"
        );

        // A quarter turn: `[0 1 -1 0 0 0]` sends (x, y) to (−y, x), so a rectangle from
        // (0,0) to (20,10) becomes one from (−10,0) to (0,20), and the page's move puts
        // its lower left at (50, 60) — so the box runs *left* of and *above* the origin,
        // which is the whole of what a rotation is and cannot be mistaken for a scale.
        let turn = rectangle_form_page(Some("0 1 -1 0 0 0"), None);
        let turned = extent(&corners(turn.records.first().expect("a mark")));
        assert!(
            near(turned.x0, -950.0)
                && near(turned.y0, 60.0)
                && near(turned.x1, 50.0)
                && near(turned.y1, 2060.0),
            "a quarter turn, in the form's space and then the page's: {turned:?}"
        );

        let moved = rectangle_form_page(Some("1 0 0 1 30 40"), None);
        let shifted = extent(&corners(moved.records.first().expect("a mark")));
        assert!(
            near(shifted.x0, 3050.0)
                && near(shifted.y0, 4060.0)
                && near(shifted.x1, 5050.0)
                && near(shifted.y1, 5060.0),
            "the form's own move, scaled by the page's hundredfold: {shifted:?}"
        );

        // And every one of them is still a closed four-sided path, which a wrong matrix
        // would leave as four unrelated points.
        for out in [&scale, &turn, &moved] {
            let points = corners(out.records.first().expect("a mark"));
            assert_eq!(points.len(), 4, "four corners, still a rectangle");
            let width = extent(&points);
            let sides = [0, 1, 2, 3].map(|i| {
                let (x, y) = points[i];
                let (x2, y2) = points[(i + 1) % 4];
                (x2 - x).hypot(y2 - y)
            });
            assert!(
                near(sides[0], sides[2]) && near(sides[1], sides[3]),
                "opposite sides equal, so the path is still closed: {sides:?}"
            );
            assert!(
                sides[0] > 0.0 && sides[1] > 0.0,
                "and it has an area, rather than being degenerate: {sides:?} for {width:?}"
            );
        }
    }

    #[test]
    fn a_forms_own_resources_name_a_font_the_page_does_not_have() {
        // The page has no `/Font` table at all and the form names `/F1` of its own. Every
        // name in a form resolves against the form's `/Resources`, and a reader that looked
        // in the page's would place every glyph with a fallback width — or draw nothing.
        let show = b"BT /F1 10 Tf 0 0 Td (AA) Tj ET";
        let with_own = xobject_resources(
            &[(
                "Fm0",
                form(
                    std::str::from_utf8(show).expect("ascii"),
                    None,
                    None,
                    Some(font_resources_dict(&[500; 95])),
                ),
            )],
            None,
        );
        assert!(
            with_own.fonts.is_empty(),
            "the page names no font at all, which is the case under test"
        );
        let out = run_with(
            &ContentStream::parse(b"q 2 0 0 2 10 10 cm /Fm0 Do Q"),
            &with_own,
        );
        let placements: Vec<Matrix> = out
            .records
            .iter()
            .filter_map(|r| match &r.mark {
                Mark::Glyphs { placements, .. } => Some(placements.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(placements.len(), 2, "the form's own font drew both glyphs");
        // The form's font declares 500 units per glyph, so at ten points each glyph is five
        // wide *in text space*, and the page's `2 0 0 2` doubles that. A fallback width
        // would be the same figure here, so the width is checked against the declared one
        // on both counts: the declaration and the page's transformation.
        let (x0, y0) = placements[0].apply(0.0, 0.0);
        let (x1, _) = placements[1].apply(0.0, 0.0);
        assert!(
            near(x0, 10.0) && near(y0, 10.0),
            "the first glyph is at the form's origin, which the page moved to (10, 10): ({x0}, {y0})"
        );
        assert!(
            near(x1 - x0, 10.0),
            "the second is one 500-unit glyph at ten points, doubled by the page: {}",
            x1 - x0
        );

        // A form with no `/Resources` of its own inherits the page's, which is the other
        // half of the rule: the same content, the same page transformation, and the *page's*
        // font — declared at a different width so that the two cannot be told apart by
        // accident.
        let inherited_page = xobject_resources(
            &[(
                "Fm0",
                form(std::str::from_utf8(show).expect("ascii"), None, None, None),
            )],
            Some(&[200; 95]),
        );
        let out = run_with(
            &ContentStream::parse(b"q 2 0 0 2 10 10 cm /Fm0 Do Q"),
            &inherited_page,
        );
        let placements: Vec<Matrix> = out
            .records
            .iter()
            .filter_map(|r| match &r.mark {
                Mark::Glyphs { placements, .. } => Some(placements.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(placements.len(), 2, "the inherited font drew both glyphs");
        let (x0, _) = placements[0].apply(0.0, 0.0);
        let (x1, _) = placements[1].apply(0.0, 0.0);
        assert!(near(x0, 10.0), "still at the form's origin: {x0}");
        assert!(
            near(x1 - x0, 4.0),
            "but a 200-unit glyph at ten points doubled: {}",
            x1 - x0
        );
        // And the marks say which table they came from, so a renderer can find the right
        // one: `/F1` in the form and `/F1` on the page are two different fonts.
        assert!(
            out.records.iter().all(|r| r.form.as_deref() == Some("Fm0")),
            "every mark inside the form names it"
        );
    }

    #[test]
    fn a_bbox_clips_the_forms_content() {
        // The form paints a rectangle far larger than its own box, so a reader that drew
        // the form without clipping paints a shape that is not on the page and one that
        // clips draws only the box.
        let out = rectangle_form_page(None, Some("10 10 20 20"));
        let record = out.records.first().expect("the form's mark");
        let clip = record.clip.as_ref().expect("the form's own clip");
        let b = clip.bounds;
        // The box is in the form's space, so it goes through the same hundredfold scale and the
        // same move as the mark does: (10,10) to (20,20) in the form becomes (1050, 1060)
        // to (2050, 2060) on the page.
        assert!(
            near(b.x0, 1050.0) && near(b.y0, 1060.0) && near(b.x1, 2050.0) && near(b.y1, 2060.0),
            "the box, through the transformation the form executes under: {b:?}"
        );
        assert_eq!(clip.paths.len(), 1, "and the region is the box, as a path");
        assert!(
            !clip.is_empty(),
            "a box with four corners is not the empty region"
        );
        // The mark's own geometry is still the whole rectangle — the clip is what cuts it
        // down — so a consumer that honours the clip and one that only reads the bounds
        // agree about what was drawn.
        let drawn = extent(&corners(record));
        assert!(
            near(drawn.x0, 50.0) && near(drawn.x1, 2050.0),
            "the mark itself is the whole rectangle, which the clip then cuts: {drawn:?}"
        );
        // The clip does not outlive the form: the `Q` in the page restored it.
        assert!(
            out.state.clip.is_none(),
            "the form's clip is not the page's"
        );

        // A form with no `/BBox` clips nothing of its own, which is legal and is not the
        // same thing as a box that clips everything away.
        let unclipped = rectangle_form_page(None, None);
        assert!(
            unclipped.records.first().expect("the mark").clip.is_none(),
            "no box means no clip of its own"
        );

        // A box with the corners the wrong way round is a slip in the file, and reading it
        // as the empty region would delete the form from the page. It is reported *and*
        // read as the box it meant.
        let inverted = rectangle_form_page(None, Some("20 20 10 10"));
        let record = inverted.records.first().expect("the form's mark");
        let clip = record.clip.as_ref().expect("a clip, not an invisible form");
        assert!(
            near(clip.bounds.x0, 1050.0) && near(clip.bounds.x1, 2050.0),
            "the same box with its corners in order: {:?}",
            clip.bounds
        );
        assert!(
            !clip.is_empty(),
            "which is the whole point: not the empty region"
        );
        assert!(
            inverted
                .notes
                .iter()
                .any(|n| n.contains("Fm0") && n.contains("wrong way round")),
            "and it is reported by name: {:?}",
            inverted.notes
        );

        // A `/BBox` that is not four numbers is damage, and damage is a finding rather than
        // a reason to draw nothing: the form is drawn, unclipped, and says so.
        let unreadable = rectangle_form_page(None, Some("0 0 10"));
        assert!(
            unreadable
                .notes
                .iter()
                .any(|n| n.contains("Fm0") && n.contains("/BBox")),
            "reported by name: {:?}",
            unreadable.notes
        );
        assert!(
            unreadable.records.first().expect("the mark").clip.is_none(),
            "and the form is still drawn"
        );
    }

    #[test]
    fn a_form_may_draw_another_form() {
        // Three deep, and the order is the assertion. The innermost form's mark comes
        // first, then the middle form's own, then the page's: a reader that executed a form
        // *after* the rest of the page would draw the page's rectangle underneath the form's
        // and get a page that looks right on paper and wrong in every overlap.
        let innermost = form("0 0 10 10 re f", None, None, None);
        let middle = form(
            "q 1 0 0 1 100 0 cm /Inner Do Q 0 0 10 10 re f",
            Some("0 0 200 200"),
            None,
            Some(xobject_resources_dict(&[("Inner", innermost)])),
        );
        let outer = xobject_resources(&[("Outer", middle)], None);
        let out = run_with(
            &ContentStream::parse(b"q 3 0 0 3 5 5 cm /Outer Do Q"),
            &outer,
        );
        assert_eq!(out.records.len(), 2, "the two rectangles, and nothing else");
        let (first, second) = (
            out.records.first().expect("the innermost form's mark"),
            out.records.get(1).expect("the middle form's own mark"),
        );
        assert_eq!(
            first.form.as_deref(),
            Some("Inner"),
            "the innermost form's mark comes first"
        );
        assert_eq!(
            second.form.as_deref(),
            Some("Outer"),
            "and the middle form's own mark after it"
        );
        // Each is placed by the whole chain of matrices: the page's threefold scale and its own
        // move, and the middle form's move to (100, 0) for its child.
        let inner_box = extent(&corners(first));
        assert!(
            near(inner_box.x0, 305.0)
                && near(inner_box.y0, 5.0)
                && near(inner_box.x1, 335.0)
                && near(inner_box.y1, 35.0),
            "three matrices deep: {inner_box:?}"
        );
        let outer_box = extent(&corners(second));
        assert!(
            near(outer_box.x0, 5.0)
                && near(outer_box.y0, 5.0)
                && near(outer_box.x1, 35.0)
                && near(outer_box.y1, 35.0),
            "the middle form's own rectangle, under the page's own `cm` and outside its \
             `q 1 0 0 1 100 0`: {outer_box:?}"
        );
        // The middle form's `/BBox` reaches both its own mark and its child's. The innermost
        // form declares no box of its own, so there is one path here and not two: a form
        // that inherits does not clip itself.
        let clip = first.clip.as_ref().expect("the middle form's clip");
        assert!(
            near(clip.bounds.x0, 5.0) && near(clip.bounds.x1, 605.0),
            "{:?}",
            clip.bounds
        );
        assert_eq!(
            clip.paths.len(),
            1,
            "one box, in force for the child's mark too"
        );
    }

    #[test]
    fn a_form_that_draws_itself_stops_and_reports() {
        // The recursion is in the run, not in the stream, so this is where the bound belongs.
        // A generator that flattened a template into the page and left the template behind
        // produces exactly this, and following it without a bound is a stack overflow — a
        // crash, not a note.
        let resources = xobject_resources(
            &[("Fm0", form("/Fm0 Do 0 0 1 1 re f", None, None, None))],
            None,
        );
        let out = run_with(&ContentStream::parse(b"/Fm0 Do"), &resources);
        assert_eq!(
            out.records.len(),
            MAX_FORM_DEPTH,
            "each level executed the same form once and then stopped"
        );
        let note = out
            .notes
            .iter()
            .find(|n| n.contains("Fm0"))
            .expect("a note naming the form");
        assert!(
            note.contains(&MAX_FORM_DEPTH.to_string()),
            "which says how deep it went: {note}"
        );
        // Nothing was drawn, and nothing was half-drawn either: there are no marks at all,
        // and the page is told why rather than being left with a hole.
        assert!(
            out.records
                .iter()
                .all(|r| !matches!(r.mark, Mark::Image { .. })),
            "no level of it reached the image path"
        );

        // Two forms each naming the other is the same shape one level round, and is bounded the
        // same way. A cycle of objects cannot be written down directly, so the chain is
        // built bottom-up and made one deeper than the bound: each form draws the next and
        // then a rectangle of its own, so the depth the run reached is countable in marks
        // rather than merely asserted to have stopped.
        let mut deepest = form("0 0 1 1 re f", None, None, None);
        for _ in 0..=MAX_FORM_DEPTH {
            deepest = form(
                "/Next Do 0 0 1 1 re f",
                None,
                None,
                Some(xobject_resources_dict(&[("Next", deepest)])),
            );
        }
        let chain = xobject_resources(&[("Fm0", deepest)], None);
        let out = run_with(&ContentStream::parse(b"/Fm0 Do"), &chain);
        assert_eq!(
            out.records.len(),
            MAX_FORM_DEPTH,
            "one rectangle per level, and not one level more"
        );
        assert!(
            out.notes.iter().any(|n| n.contains("deep")),
            "and the level that would have gone past it is reported: {:?}",
            out.notes
        );
        assert!(
            !out.notes.iter().any(|n| n.contains("do not define")),
            "every name in the chain resolved, so the bound is what stopped it: {:?}",
            out.notes
        );
    }

    #[test]
    fn a_truncated_array_reaches_the_page_notes() {
        // The lexer is where a collection bound bites, and the page is where it has to be
        // visible: a `TJ` cut short is a line of text that will not be drawn, and nobody
        // finds out by looking at the code that gathers it.
        let mut content = b"BT /F1 12 Tf [".to_vec();
        for i in 0..=crate::tokens::MAX_COLLECTION_ITEMS {
            content.extend_from_slice(format!("(x{i}) ").as_bytes());
        }
        content.extend_from_slice(b"] TJ ET");
        let out = run(&ContentStream::parse(&content));
        let note = out
            .notes
            .iter()
            .find(|n| n.contains("array"))
            .expect("a note naming the array");
        assert!(
            note.contains(&crate::tokens::MAX_COLLECTION_ITEMS.to_string()),
            "which says where it was cut: {note}"
        );
    }

    #[test]
    fn the_state_a_form_finds_is_the_state_it_leaves_behind() {
        // The form's content is executed as though wrapped in `q` … `Q`, so the state the
        // page set before the `Do` survives it and nothing the form set escapes.
        let noisy = form(
            "7 w 1 0 0 rg 2 0 0 2 1000 1000 cm 0 0 200 200 re W n 0 0 m 1 1 l S",
            None,
            None,
            None,
        );
        let resources = xobject_resources(&[("Fm0", noisy)], None);
        let out = run_with(
            &ContentStream::parse(b"q 3 w 0 0 1 rg 1 0 0 1 50 50 cm /Fm0 Do 0 0 m 10 0 l S Q"),
            &resources,
        );
        // The form's own mark is drawn under the form's state.
        let inside = out.records.first().expect("the form's mark");
        assert!(
            near(inside.ctm.e, 1050.0) && near(inside.ctm.f, 1050.0),
            "the form's own `cm` composed onto the page's: {:?}",
            inside.ctm
        );
        assert!(
            near(inside.device_line_width, 14.0),
            "and its own line width, doubled by its own scale: {}",
            inside.device_line_width
        );
        assert!(inside.clip.is_some(), "and its own clip");
        // The mark after the `Do` is the page's again: its own move, and neither the form's
        // twofold scale nor its thousand-unit translation. It is found by asking which
        // form drew it rather than by its position, because the form's own clip mark is in
        // the list too.
        let after = out
            .records
            .iter()
            .find(|r| r.form.is_none())
            .expect("the page's own mark");
        assert!(
            near(after.ctm.a, 1.0) && near(after.ctm.e, 50.0) && near(after.ctm.f, 50.0),
            "the transformation the form set did not escape: {:?}",
            after.ctm
        );
        assert!(
            near(after.device_line_width, 3.0),
            "nor its line width: {}",
            after.device_line_width
        );
        assert!(after.clip.is_none(), "nor its clip");
        let Mark::Path {
            stroke: Some(stroke),
            ..
        } = &after.mark
        else {
            panic!("a stroked path");
        };
        assert_eq!(
            stroke.components,
            vec![0.0],
            "nor its colour: the page set black and the form set red"
        );
        // And the state the page ends in is the one it had before the `q`, which is the default:
        // the form is two levels of `q` below it and neither leaked.
        assert!(
            near(out.state.stroke.width, 1.0),
            "the default width: {}",
            out.state.stroke.width
        );
        assert!(out.state.ctm.is_identity());
        assert_eq!(
            out.state.fill.components,
            vec![0.0],
            "and the fill the page set inside the `q` is gone with it"
        );
    }

    #[test]
    fn a_do_naming_an_xobject_that_is_neither_an_image_nor_a_form_is_reported_by_name() {
        // `/PS` is a real subtype: a PostScript XObject is in the format, and executing its
        // content as operators would draw whatever the PostScript happened to contain as
        // paths. Handing it to an image decoder asks for something that is not an image.
        // The only answer that is not a blank or a half-drawn shape is to say so.
        let mut dict = Dict::new();
        dict.set("Type", Obj::name("XObject"));
        dict.set("Subtype", Obj::name("PS"));
        let mut table = Dict::new();
        table.set("Weird", Obj::Dict(dict));
        let mut resources = Dict::new();
        resources.set("XObject", Obj::Dict(table));
        let resources = Resources::from_dict(&resources, &|o| Some(o.clone()));

        let out = run_with(&ContentStream::parse(b"/Weird Do"), &resources);
        assert!(
            out.records.is_empty(),
            "nothing was recorded for it: {:?}",
            out.records.iter().map(|r| &r.mark).collect::<Vec<_>>()
        );
        let note = out.notes.first().expect("a note");
        assert!(
            note.contains("Weird") && note.contains("PS"),
            "which names both the XObject and its subtype: {note}"
        );
    }

    /// The `/Resources` of a form, as a `/XObject` table of the forms it draws.
    fn xobject_resources_dict(forms: &[(&str, Obj)]) -> Dict {
        let mut table = Dict::new();
        for (name, object) in forms {
            table.set(name, object.clone());
        }
        let mut resources = Dict::new();
        resources.set("XObject", Obj::Dict(table));
        resources
    }

    #[test]
    fn a_record_carries_the_bytes_that_drew_it() {
        let source = b"1 0 0 1 5 5 cm 0 0 m 10 10 l S";
        let out = run_bytes(source);
        let rec = out.records.first().expect("a mark");
        let bytes = source.get(rec.span.clone()).expect("the span");
        assert_eq!(
            bytes, b"0 0 m 10 10 l S",
            "a mark's bytes run from the path's first point to the operator that painted it"
        );
    }

    #[test]
    fn a_zoomed_transform_gives_larger_bounds() {
        let plain = run_bytes(b"0 0 m 10 10 l S");
        let zoomed = run_bytes(b"2 0 0 2 0 0 cm 0 0 m 10 10 l S");
        let a = plain
            .records
            .first()
            .and_then(Record::bounds)
            .expect("bounds");
        let b = zoomed
            .records
            .first()
            .and_then(Record::bounds)
            .expect("bounds");
        assert!(near(a.x1, 10.0) && near(b.x1, 20.0));
    }

    #[test]
    fn a_runaway_stream_is_stopped_rather_than_run_forever() {
        // A megabyte of `S` with no path between them: one mark, then a note.
        let mut data = Vec::new();
        for _ in 0..=MAX_RECORDS {
            data.extend_from_slice(b"0 0 m 1 1 l S ");
        }
        let out = run_bytes(&data);
        assert!(out.records.len() <= MAX_RECORDS);
        assert!(out.notes.iter().any(|n| n.contains("marks")));
    }

    #[test]
    fn a_bbox_is_read_and_ordered() {
        let mut s = Stream {
            dict: Dict::new(),
            raw: Vec::new(),
            file_offset: None,
            synthetic: true,
        };
        s.dict.set(
            "BBox",
            Object::Array(vec![
                Object::Real(10.0),
                Object::Real(20.0),
                Object::Real(0.0),
                Object::Real(5.0),
            ]),
        );
        let b = bbox_of(&s).expect("a bbox");
        // The dictionary said (10, 20, 0, 5); the corners came the other way round, and
        // a reader must order them rather than produce a negative rectangle.
        assert_eq!(
            (b.x0, b.y0, b.x1, b.y1),
            (0.0, 5.0, 10.0, 20.0),
            "got {b:?}"
        );
    }

    #[test]
    fn a_short_or_missing_bbox_is_none() {
        let mut s = Stream {
            dict: Dict::new(),
            raw: Vec::new(),
            file_offset: None,
            synthetic: true,
        };
        assert!(bbox_of(&s).is_none());
        s.dict.set("BBox", Object::Array(vec![Object::Real(0.0)]));
        assert!(bbox_of(&s).is_none());
    }
}
