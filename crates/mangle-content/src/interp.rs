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

use mangle_syntax::object::{Object, Stream};

use crate::matrix::Matrix;
use crate::ops;
use crate::state::{
    ClipBounds, Colour, ColourSpace, Dash, GraphicsState, LineCap, LineJoin, PathSegment,
    StateStack,
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
        /// The bytes of each string operand, in order.
        ///
        /// Separate from the mark's own span, which runs from the first operand to the
        /// operator: to change the text, these are the bytes to rewrite, and the `Tj`
        /// and its operands are not among them. A `TJ` has several, which is why this is
        /// a list.
        text_spans: Vec<Range<usize>>,
        /// The matrix each glyph is placed by, one per glyph.
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
    },
    /// A shading, filled into the current clip.
    Shading { name: String, matrix: Matrix },
    /// A clip was narrowed.
    ClipChanged(Option<ClipBounds>),
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
    /// The clip in force, in device space.
    pub clip: Option<ClipBounds>,
    /// The fill and stroke alphas, which a compositing renderer needs and a geometry
    /// one does not.
    pub fill_alpha: f64,
    pub stroke_alpha: f64,
    pub blend_mode: String,
    /// The stroke width, already scaled by the transformation: this is the width the
    /// user will see, not the number in the stream.
    pub device_line_width: f64,
    /// The marked-content tag, if the mark was inside a `/BMC` or `/BDC` group.
    pub tag: Option<String>,
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
            Mark::ClipChanged(bounds) => *bounds,
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
/// A form XObject is executed with the *form's* resources and the *page's* graphics
/// state, which is the same call with different arguments, so this is the only entry
/// point.
#[must_use]
pub fn run_with(stream: &ContentStream, resources: &crate::Resources) -> PageContent {
    let mut ctx = Context {
        state: GraphicsState::new(),
        stack: StateStack::new(),
        out: PageContent::default(),
        clip_pending: false,
        path_start: None,
        resources,
    };
    for op in stream.operations() {
        ctx.step(&op);
        if ctx.out.records.len() >= MAX_RECORDS {
            ctx.out.notes.push(format!(
                "stopped after {MAX_RECORDS} marks; the rest of this page was not executed"
            ));
            break;
        }
    }
    ctx.out.state = ctx.state;
    ctx.out
}

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
    /// The page's resource tables, which is where `gs` names are looked up.
    resources: &'a crate::Resources,
}

impl Context<'_> {
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
                    self.state.text.font = Some(String::from_utf8_lossy(&n).into_owned());
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
        let space = ColourSpace {
            name: String::from_utf8_lossy(&n).into_owned(),
            colorant: None,
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
        // The clip is the path that is being built, whatever the operator is going to
        // do with it: `W n` clips to it and `W f` clips to it and fills it.
        let clip_path = if clip_pending && self.state.has_path() {
            Some(bounds_of(&Mark::Path {
                segments: self.state.device_path(),
                fill: None,
                stroke: None,
                rule: FillRule::NonZero,
            }))
        } else {
            None
        };
        let mut mark = match name {
            b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"S" | b"s" => Mark::Path {
                segments: self.state.device_path(),
                fill: match name {
                    b"S" => None,
                    b"s" => Some(self.state.fill.clone()),
                    b"b" | b"b*" => Some(self.state.fill.clone()),
                    _ => Some(self.state.fill.clone()),
                },
                stroke: Some(self.state.stroking.clone()),
                rule: if matches!(name, b"f*" | b"B*" | b"b*") {
                    FillRule::EvenOdd
                } else {
                    FillRule::NonZero
                },
            },
            // `n` paints nothing. It is here only because a pending `W` needs an
            // operation to attach itself to, and the attachment is the mark.
            b"n" => Mark::ClipChanged(self.state.clip),
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
            },
            _ => return,
        };

        // A pending `W` narrows the clip for this mark and every one after it.
        if clip_pending {
            // `n` with an empty path says nothing about a clip: there is no path to
            // clip to, and a reader must not conclude the page is now unclipped.
            let device = clip_path.unwrap_or(self.state.clip);
            self.state.clip = match (self.state.clip, device) {
                (Some(existing), Some(b)) => existing.intersect(b),
                (None, Some(b)) => Some(b),
                // A clip to nothing means nothing is drawn from here on, which is a
                // state rather than an error.
                (Some(_), None) => None,
                (None, None) => None,
            };
            mark = Mark::ClipChanged(self.state.clip);
        }

        let span = match self.path_start {
            // No path was built, so the mark is the operator and its operands alone.
            None => op.span.clone(),
            Some(start) => start..op.span.end,
        };
        let record = Record {
            mark,
            span,
            ctm: self.state.ctm,
            clip: self.state.clip,
            fill_alpha: self.state.fill_alpha,
            stroke_alpha: self.state.stroke_alpha,
            blend_mode: self.state.blend_mode.clone(),
            device_line_width: self.state.stroke.width * self.state.ctm.mean_scale(),
            tag: self.out.tags.last().cloned(),
        };
        if self.out.records.len() < MAX_RECORDS {
            self.out.records.push(record);
        }
        // Every painting operator ends the path; `s`, `f` and `B*` also close it first,
        // which `apply` has already done.
        self.state.clear_path();
        self.path_start = None;
    }

    /// Record a text-showing operator. Each byte is treated as one glyph, which is
    /// wrong for every multi-byte encoding and right for none of them — but it is the
    /// honest unit here, because deciding which bytes are glyphs needs the font and
    /// encoding, which is the font layer's job and not this one's.
    fn record_text(&mut self, op: Operation, operands: &[&Object], _newline_first: bool) {
        // The operand tokens, not the values, so a string's own bytes can be recorded.
        let strings: Vec<&ContentToken> = (0..operands.len())
            .filter_map(|i| op.operands.get(i))
            .filter(|t| matches!(t.value, Object::String(_)))
            .collect();
        let mut text: Vec<u8> = Vec::new();
        let mut text_spans: Vec<Range<usize>> = Vec::new();
        let mut kerns: Vec<f64> = Vec::new();
        for operand in operands {
            match operand {
                Object::String(s) => text.extend_from_slice(s),
                Object::Array(a) => {
                    for item in a {
                        match item {
                            Object::String(s) => text.extend_from_slice(s),
                            other => kerns.push(other.as_f64().unwrap_or(0.0)),
                        }
                    }
                }
                other => {
                    // A number where a string belongs is damage; record what we can.
                    if let Some(v) = other.as_f64() {
                        kerns.push(v);
                    }
                }
            }
        }
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
        if text.is_empty() {
            return;
        }
        let base = self.state.text_rendering_matrix();
        let width = self.glyph_advance();
        // Place every glyph, applying the kerns between the strings.
        let mut placements = Vec::with_capacity(text.len());
        let mut cursor = base;
        for (index, _) in text.iter().enumerate() {
            if let Some(kern) = kerns.get(index) {
                // A `TJ` number is in thousandths of an em, and positive moves the next
                // glyph *closer*, which is the sign the specification means.
                let kern = *kern / 1000.0 * self.state.text.size;
                cursor = cursor.concat(Matrix::translate(-kern, 0.0));
            }
            placements.push(cursor);
            cursor = cursor.concat(Matrix::translate(width, 0.0));
        }
        let shown = text.len();
        let record = Record {
            mark: Mark::Glyphs {
                font: self.state.text.font.clone(),
                size: self.state.text.size,
                text,
                text_spans,
                placements,
            },
            span: op.span.clone(),
            ctm: self.state.ctm,
            clip: self.state.clip,
            fill_alpha: self.state.fill_alpha,
            stroke_alpha: self.state.stroke_alpha,
            blend_mode: self.state.blend_mode.clone(),
            device_line_width: self.state.stroke.width * self.state.ctm.mean_scale(),
            tag: self.out.tags.last().cloned(),
        };
        if self.out.records.len() < MAX_RECORDS {
            self.out.records.push(record);
        }
        // The text matrix moves past what was shown, which is what makes a second `Tj`
        // continue rather than overlap. The *line* matrix does not move: a `Td` after
        // this starts a new line from where the last one began, not from the end of the
        // text on it.
        let advance = self.glyph_advance() * f64::from(u32::try_from(shown).unwrap_or(0));
        self.state.text_matrix = self
            .state
            .text_matrix
            .concat(Matrix::translate(advance, 0.0));
    }

    /// How far one glyph moves the pen, without a font to ask.
    ///
    /// The spacing terms are known exactly; the glyph's own width is not, so this uses
    /// the conventional 500-unit average, which is what the specification's default
    /// `/MissingWidth` behaviour amounts to for layout purposes. The font layer
    /// replaces this with real metrics; until then, an approximation that is stated is
    /// better than a guess that is not.
    fn glyph_advance(&self) -> f64 {
        let scale = self.state.text.horizontal_scale / 100.0;
        let nominal = self.state.text.size * 0.5;
        (nominal + self.state.text.char_spacing) * scale
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

    fn run_bytes(data: &[u8]) -> PageContent {
        run(&ContentStream::parse(data))
    }

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
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
        let clip = rec.clip.expect("a clip");
        assert!(near(clip.x0, 0.0) && near(clip.x1, 100.0));
        // The mark records that the clip changed rather than the path it changed it
        // with, because the path is not drawn.
        assert!(matches!(rec.mark, Mark::ClipChanged(_)));
    }

    #[test]
    fn two_clips_intersect() {
        let out = run_bytes(b"q 0 0 100 100 re W n 0 0 10 10 re W n 0 0 m 1 1 l S Q");
        // One record per clipping operator, then the stroke inside both.
        let clips: Vec<ClipBounds> = out.records.iter().filter_map(|r| r.clip).collect();
        assert_eq!(clips.len(), 3, "two clips and a stroke");
        let widths: Vec<f64> = clips.iter().map(|c| c.x1 - c.x0).collect();
        // The first clip is 100 wide; the second is the intersection with a 10-wide
        // rectangle, not a replacement; and the stroke inside both sees the narrower one.
        assert_eq!(widths, vec![100.0, 10.0, 10.0]);
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
            dict: mangle_syntax::object::Dict::new(),
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
            dict: mangle_syntax::object::Dict::new(),
            raw: Vec::new(),
            file_offset: None,
            synthetic: true,
        };
        assert!(bbox_of(&s).is_none());
        s.dict.set("BBox", Object::Array(vec![Object::Real(0.0)]));
        assert!(bbox_of(&s).is_none());
    }
}
