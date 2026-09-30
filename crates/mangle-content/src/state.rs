//! The graphics state: everything `q` saves and `Q` restores.
//!
//! The state is a value, not a place. That is what makes `q`/`Q` trivial, what makes
//! `gs` a single field assignment, and what lets an interpreter answer "what was the
//! line width when this was painted?" without having kept a history.

use std::collections::BTreeMap;

use mangle_syntax::object::{Dict, Object};

use crate::matrix::Matrix;

/// How a line's ends are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineCap {
    #[default]
    Butt,
    Round,
    Projecting,
    Square,
}

impl LineCap {
    #[must_use]
    pub fn from_int(v: i64) -> Self {
        match v {
            1 => Self::Round,
            2 => Self::Square,
            _ => Self::Butt,
        }
    }
}

/// How a line's corners are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineJoin {
    #[default]
    Miter,
    Round,
    Bevel,
}

impl LineJoin {
    #[must_use]
    pub fn from_int(v: i64) -> Self {
        match v {
            1 => Self::Round,
            2 => Self::Bevel,
            _ => Self::Miter,
        }
    }
}

/// A dash pattern: alternating on and off lengths, and where in the pattern to start.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Dash {
    pub array: Vec<f64>,
    pub phase: f64,
}

impl Dash {
    /// Does this pattern mean "solid"?
    ///
    /// An empty array does, an array of all zeros does, and an array with any negative
    /// entry does: the specification makes such an array invalid, and the defensive
    /// reading of an invalid dash is to draw the line solid rather than to stop drawing
    /// it. All three appear in real files.
    #[must_use]
    pub fn is_solid(&self) -> bool {
        self.array.is_empty() || self.array.iter().any(|v| *v <= 0.0)
    }
}

/// How glyphs are painted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RenderMode {
    #[default]
    Fill,
    Stroke,
    FillThenStroke,
    Invisible,
    FillAndClip,
    StrokeAndClip,
    FillThenStrokeAndClip,
    Clip,
    ClipStroke,
}

impl RenderMode {
    #[must_use]
    pub fn from_int(v: i64) -> Option<Self> {
        Some(match v {
            0 => Self::Fill,
            1 => Self::Stroke,
            2 => Self::FillThenStroke,
            3 => Self::Invisible,
            4 => Self::FillAndClip,
            5 => Self::StrokeAndClip,
            6 => Self::FillThenStrokeAndClip,
            7 => Self::Clip,
            _ => return None,
        })
    }

    #[must_use]
    pub fn paints(self) -> bool {
        !matches!(self, Self::Invisible | Self::Clip)
    }
}

/// A colour space, as far as the graphics state needs to know.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ColourSpace {
    /// `/DeviceGray`, `/DeviceRGB`, `/DeviceCMYK`, `/Pattern`, or a name to resolve.
    pub name: String,
    /// `/Separation`, `/DeviceN` and the rest carry a colorant name.
    pub colorant: Option<String>,
}

impl ColourSpace {
    #[must_use]
    pub fn device_gray() -> Self {
        Self {
            name: "DeviceGray".into(),
            colorant: None,
        }
    }

    #[must_use]
    pub fn device_rgb() -> Self {
        Self {
            name: "DeviceRGB".into(),
            colorant: None,
        }
    }

    /// How many components a colour in this space has.
    #[must_use]
    pub fn components(&self) -> usize {
        match self.name.as_str() {
            "DeviceGray" | "CalGray" | "Separation" | "Indexed" => 1,
            "DeviceRGB" | "CalRGB" | "Lab" => 3,
            "DeviceCMYK" => 4,
            "Pattern" => 1,
            // An unknown space: assume the common case rather than guessing wildly.
            _ => 3,
        }
    }
}

/// A colour: components in its own space, clamped to the range the space allows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Colour {
    pub space: ColourSpace,
    pub components: Vec<f64>,
}

impl Colour {
    /// Black, which is what a stream that sets no colour draws in.
    #[must_use]
    pub fn black() -> Self {
        Self {
            space: ColourSpace::device_gray(),
            components: vec![0.0],
        }
    }

    /// Set the components, padding or trimming to what the space needs. A stream that
    /// gives the wrong count is damaged, and the nearest sensible value is better than
    /// refusing to draw.
    pub fn set(&mut self, space: ColourSpace, components: &[f64]) {
        let want = space.components();
        self.space = space;
        self.components = components.iter().copied().take(want).collect();
        while self.components.len() < want {
            self.components.push(0.0);
        }
        // Every device space is nominally 0 to 1, including CMYK, so one clamp serves
        // all of them. A value outside the range is a damaged stream, and clamping
        // draws it rather than dropping it.
        for c in &mut self.components {
            *c = c.clamp(0.0, 1.0);
        }
    }

    /// The components as grey, for a state that has to know one colour from another.
    #[must_use]
    pub fn as_gray(&self) -> f64 {
        match self.components.as_slice() {
            [g] => *g,
            [r, g, b, ..] => (0.2126 * r + 0.7152 * g + 0.0722 * b).clamp(0.0, 1.0),
            _ => 0.0,
        }
    }
}

/// The line parameters.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StrokeStyle {
    pub width: f64,
    pub cap: LineCap,
    pub join: LineJoin,
    pub miter_limit: f64,
    pub dash: Dash,
    /// `/RenderingIntent`, which a colour-management-aware renderer needs and a
    /// geometry-only one ignores.
    pub intent: Option<String>,
}

impl StrokeStyle {
    fn defaults() -> Self {
        Self {
            width: 1.0,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            miter_limit: 10.0,
            dash: Dash::default(),
            intent: None,
        }
    }
}

/// The text parameters.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TextState {
    /// `/F1`, as written. Resolution to a font is the font layer's job.
    pub font: Option<String>,
    pub size: f64,
    /// `Tc`: added to every glyph's displacement.
    pub char_spacing: f64,
    /// `Tw`: added to every space's displacement. Not in every font.
    pub word_spacing: f64,
    /// `Tz`, as a percentage: 100 is normal.
    pub horizontal_scale: f64,
    /// `TL`: the leading `T*` moves by.
    pub leading: f64,
    /// `Ts`: how far the baseline rises.
    pub rise: f64,
    /// `Tr`.
    pub render_mode: RenderMode,
}

impl TextState {
    fn defaults() -> Self {
        Self {
            font: None,
            size: 0.0,
            char_spacing: 0.0,
            word_spacing: 0.0,
            horizontal_scale: 100.0,
            leading: 0.0,
            rise: 0.0,
            render_mode: RenderMode::Fill,
        }
    }
}

/// The state a `/gs` parameter dictionary can set.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExtGState {
    pub line_width: Option<f64>,
    pub line_cap: Option<LineCap>,
    pub line_join: Option<LineJoin>,
    pub miter_limit: Option<f64>,
    pub dash: Option<Dash>,
    pub font: Option<(String, f64)>,
    pub char_spacing: Option<f64>,
    pub word_spacing: Option<f64>,
    pub horizontal_scale: Option<f64>,
    pub leading: Option<f64>,
    pub rise: Option<f64>,
    pub render_mode: Option<RenderMode>,
    pub font_size: Option<f64>,
    /// `/ca` and `/CA`.
    pub fill_alpha: Option<f64>,
    pub stroke_alpha: Option<f64>,
    /// `/LW`, `/LC` and `/LJ` are the abbreviations a `/gs` dictionary uses.
    pub blend_mode: Option<String>,
    pub soft_mask: Option<Object>,
    /// `/SMask`: a name to resolve, or a dictionary to read.
    pub smask: Option<Object>,
    /// `/Font`: an array `[font size]`.
    pub font_array: Option<Object>,
}

impl ExtGState {
    /// Read the keys a `/gs` dictionary may carry.
    #[must_use]
    pub fn from_dict(d: &Dict) -> Self {
        let num = |k: &str| d.get(k).and_then(Object::as_f64);
        let int = |k: &str| d.get(k).and_then(Object::as_i64);
        Self {
            // The specification's names are all short. The long forms are a courtesy to
            // producers that write them out, and only where the name is unambiguous.
            line_width: num("LW").or_else(|| num("LineWidth")),
            line_cap: int("LC").or_else(|| int("LineCap")).map(LineCap::from_int),
            line_join: int("LJ")
                .or_else(|| int("LineJoin"))
                .map(LineJoin::from_int),
            miter_limit: num("ML").or_else(|| num("MiterLimit")),
            dash: read_dash(d),
            font: d
                .get("Font")
                .and_then(Object::as_dict)
                .and_then(|f| f.get("F1"))
                .and_then(Object::as_name)
                .map(|n| (String::from_utf8_lossy(n).into_owned(), 0.0)),
            font_array: d.get("Font").cloned(),
            font_size: num("FontSize"),
            char_spacing: num("Tc").or_else(|| num("CharSpacing")),
            word_spacing: num("Tw").or_else(|| num("WordSpacing")),
            horizontal_scale: num("Tz").or_else(|| num("HScale")),
            leading: num("TL").or_else(|| num("Leading")),
            rise: num("Ts").or_else(|| num("Rise")),
            render_mode: int("Tr").and_then(RenderMode::from_int).or_else(|| {
                d.get("TR").and_then(Object::as_name).map(|n| match n {
                    b"Fill" => RenderMode::Fill,
                    b"Stroke" => RenderMode::Stroke,
                    b"FillThenStroke" => RenderMode::FillThenStroke,
                    b"Invisible" => RenderMode::Invisible,
                    b"FillAndClip" => RenderMode::FillAndClip,
                    b"StrokeAndClip" => RenderMode::StrokeAndClip,
                    b"FillThenStrokeAndClip" => RenderMode::FillThenStrokeAndClip,
                    b"Clip" => RenderMode::Clip,
                    b"ClipStroke" => RenderMode::ClipStroke,
                    _ => RenderMode::Fill,
                })
            }),
            fill_alpha: num("ca"),
            stroke_alpha: num("CA"),
            blend_mode: d
                .get("BM")
                .and_then(Object::as_name)
                .map(|n| String::from_utf8_lossy(n).into_owned()),
            soft_mask: d.get("SMask").cloned(),
            smask: d.get("SMask").cloned(),
        }
    }

    /// Apply this to a state. Only the keys the dictionary actually carries are touched.
    pub fn apply(&self, state: &mut GraphicsState) {
        if let Some(w) = self.line_width {
            state.stroke.width = w;
        }
        if let Some(c) = self.line_cap {
            state.stroke.cap = c;
        }
        if let Some(j) = self.line_join {
            state.stroke.join = j;
        }
        if let Some(m) = self.miter_limit {
            state.stroke.miter_limit = m;
        }
        if let Some(d) = &self.dash {
            state.stroke.dash = d.clone();
        }
        if let Some((f, size)) = &self.font {
            state.text.font = Some(f.clone());
            if *size != 0.0 {
                state.text.size = *size;
            }
        }
        if let Some(size) = self.font_size {
            state.text.size = size;
        }
        if let Some(v) = self.char_spacing {
            state.text.char_spacing = v;
        }
        if let Some(v) = self.word_spacing {
            state.text.word_spacing = v;
        }
        if let Some(v) = self.horizontal_scale {
            state.text.horizontal_scale = v;
        }
        if let Some(v) = self.leading {
            state.text.leading = v;
        }
        if let Some(v) = self.rise {
            state.text.rise = v;
        }
        if let Some(v) = self.render_mode {
            state.text.render_mode = v;
        }
        if let Some(v) = self.fill_alpha {
            state.fill_alpha = v.clamp(0.0, 1.0);
        }
        if let Some(v) = self.stroke_alpha {
            state.stroke_alpha = v.clamp(0.0, 1.0);
        }
        if let Some(v) = &self.blend_mode {
            state.blend_mode.clone_from(v);
        }
    }
}

fn read_dash(d: &Dict) -> Option<Dash> {
    let array = d.get("D").or_else(|| d.get("Dash"))?.as_array()?;
    // A `/D` is one array of numbers. A file that nests the numbers in an array of its
    // own is handled too, because that shape also occurs.
    let mut numbers = Vec::new();
    for item in array {
        if let Object::Array(inner) = item {
            numbers.extend(inner.iter().filter_map(Object::as_f64));
        }
    }
    let phase = d
        .get("Phase")
        .or_else(|| d.get("DP"))
        .and_then(|o| match o {
            Object::Array(a) => a.first().and_then(Object::as_f64),
            other => other.as_f64(),
        })
        .unwrap_or(0.0);
    Some(Dash {
        array: numbers,
        phase,
    })
}

/// The state `q` and `Q` act on.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphicsState {
    /// The current transformation.
    pub ctm: Matrix,
    pub stroke: StrokeStyle,
    pub fill: Colour,
    pub stroking: Colour,
    pub text: TextState,
    /// The clip, as a rectangle in device space.
    ///
    /// A rectangle is not the whole truth — a clip can be any path — but it is the
    /// part that decides what is visible for the overwhelming majority of pages, and
    /// the renderer computes the real region from the path itself. Recording a
    /// *bounding* box here would be a claim this type cannot keep, so it is called what
    /// it is.
    pub clip: Option<ClipBounds>,
    pub fill_alpha: f64,
    pub stroke_alpha: f64,
    pub blend_mode: String,
    /// Where the current path is, which the interpreter owns but the state reports.
    pub current_point: Option<(f64, f64)>,
    /// The starting point of the current subpath, for `h`.
    pub subpath_start: Option<(f64, f64)>,
    /// The path being built, in user space.
    pub path: Vec<PathSegment>,
    /// Text: the matrix and the line matrix, plus whether each is set.
    pub text_matrix: Matrix,
    pub line_matrix: Matrix,
    /// `/TK`: whether a glyph's knockout is true.
    pub text_knockout: bool,
    /// The number of `q` calls not yet matched by a `Q`. Bounded, because a file can
    /// push without ever popping.
    pub depth: usize,
}

/// A clip rectangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipBounds {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl ClipBounds {
    /// The intersection, or `None` when the two do not overlap: an empty clip is not a
    /// smaller clip, it is nothing drawn at all.
    #[must_use]
    pub fn intersect(self, other: Self) -> Option<Self> {
        let r = Self {
            x0: self.x0.max(other.x0),
            y0: self.y0.max(other.y0),
            x1: self.x1.min(other.x1),
            y1: self.y1.min(other.y1),
        };
        (r.x0 < r.x1 && r.y0 < r.y1).then_some(r)
    }
}

/// One piece of a path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathSegment {
    /// A new subpath at a point.
    Move(f64, f64),
    Line(f64, f64),
    /// A cubic with two control points.
    Curve(f64, f64, f64, f64, f64, f64),
    /// The path is closed here.
    Close,
}

/// How many `q` calls may be outstanding before we stop saving.
pub const MAX_STATE_DEPTH: usize = 64;

impl Default for GraphicsState {
    fn default() -> Self {
        Self {
            ctm: Matrix::IDENTITY,
            stroke: StrokeStyle::defaults(),
            fill: Colour::black(),
            stroking: Colour::black(),
            text: TextState::defaults(),
            clip: None,
            fill_alpha: 1.0,
            stroke_alpha: 1.0,
            blend_mode: "Normal".to_string(),
            current_point: None,
            subpath_start: None,
            path: Vec::new(),
            text_matrix: Matrix::IDENTITY,
            line_matrix: Matrix::IDENTITY,
            text_knockout: true,
            depth: 0,
        }
    }
}

impl GraphicsState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Begin `BT`: the text and line matrices become the identity, as the specification
    /// says, so text starts from the current transformation and nowhere else.
    pub fn begin_text(&mut self) {
        self.text_matrix = Matrix::IDENTITY;
        self.line_matrix = Matrix::IDENTITY;
    }

    /// `Td`: move to the start of the next line, offset from the line matrix.
    pub fn move_text(&mut self, tx: f64, ty: f64) {
        self.line_matrix = self.line_matrix.concat(Matrix::translate(tx, ty));
        self.text_matrix = self.line_matrix;
    }

    /// `T*`: move down by the leading.
    pub fn next_line(&mut self) {
        let leading = self.text.leading;
        self.move_text(0.0, -leading);
    }

    /// The text rendering matrix: the transformation, the text matrix, and the size and
    /// rise. This is the matrix a glyph is drawn through.
    ///
    /// The *text* matrix, not the line matrix: `Tj` advances the text matrix only, so
    /// two shows in a row continue while a `Td` starts a new line from the line matrix.
    #[must_use]
    pub fn text_rendering_matrix(&self) -> Matrix {
        let size = self.text.size;
        let rise = self.text.rise;
        self.ctm
            .concat(self.text_matrix)
            .concat(Matrix::new(size, 0.0, 0.0, size, 0.0, rise))
    }

    /// `cm`.
    pub fn concat(&mut self, m: Matrix) {
        self.ctm = self.ctm.concat(m);
    }

    /// Start a subpath.
    pub fn move_to(&mut self, x: f64, y: f64) {
        self.current_point = Some((x, y));
        self.subpath_start = Some((x, y));
        self.path.push(PathSegment::Move(x, y));
    }

    /// A straight segment. A line with no current point starts a subpath here, which is
    /// what the specification says and what damaged files rely on.
    pub fn line_to(&mut self, x: f64, y: f64) {
        if self.current_point.is_none() {
            self.move_to(x, y);
            return;
        }
        self.current_point = Some((x, y));
        self.path.push(PathSegment::Line(x, y));
    }

    /// A cubic segment.
    #[allow(clippy::too_many_arguments)]
    pub fn curve_to(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, x3: f64, y3: f64) {
        if self.current_point.is_none() {
            self.move_to(x1, y1);
        }
        self.current_point = Some((x3, y3));
        self.path.push(PathSegment::Curve(x1, y1, x2, y2, x3, y3));
    }

    /// `h`: close the subpath, and set the current point to where it started.
    pub fn close_path(&mut self) {
        if self.subpath_start.is_some() {
            self.current_point = self.subpath_start;
            self.path.push(PathSegment::Close);
        }
    }

    /// Empty the current path, which every painting operator does.
    pub fn clear_path(&mut self) {
        self.path.clear();
        self.current_point = None;
        self.subpath_start = None;
    }

    /// Does the current path have anything to paint?
    #[must_use]
    pub fn has_path(&self) -> bool {
        self.path
            .iter()
            .any(|s| !matches!(s, PathSegment::Move(_, _)))
    }

    /// The bounds of the current path in user space, as a control-point hull.
    ///
    /// A curve's bounds are not its control points' bounds; this is a hull, and the
    /// renderer computes the real extent. It is used for hit testing and for a
    /// conservative cull, both of which want an over-estimate rather than a wrong one.
    #[must_use]
    pub fn path_bounds(&self) -> Option<ClipBounds> {
        let mut b: Option<ClipBounds> = None;
        for seg in &self.path {
            let points: Vec<(f64, f64)> = match *seg {
                PathSegment::Move(x, y) | PathSegment::Line(x, y) => vec![(x, y)],
                PathSegment::Curve(x1, y1, x2, y2, x3, y3) => vec![(x1, y1), (x2, y2), (x3, y3)],
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

    /// The current path, moved into device space by the transformation. This is what a
    /// renderer consumes, and it is a copy: the state keeps the user-space path, which
    /// is the version that survives a further `cm`.
    #[must_use]
    pub fn device_path(&self) -> Vec<PathSegment> {
        self.path
            .iter()
            .map(|seg| match *seg {
                PathSegment::Move(x, y) => {
                    let (x, y) = self.ctm.apply(x, y);
                    PathSegment::Move(x, y)
                }
                PathSegment::Line(x, y) => {
                    let (x, y) = self.ctm.apply(x, y);
                    PathSegment::Line(x, y)
                }
                PathSegment::Curve(x1, y1, x2, y2, x3, y3) => {
                    let (x1, y1) = self.ctm.apply(x1, y1);
                    let (x2, y2) = self.ctm.apply(x2, y2);
                    let (x3, y3) = self.ctm.apply(x3, y3);
                    PathSegment::Curve(x1, y1, x2, y2, x3, y3)
                }
                PathSegment::Close => PathSegment::Close,
            })
            .collect()
    }
}

/// The stack `q` writes to and `Q` reads from.
#[derive(Debug, Clone, Default)]
pub struct StateStack {
    saved: Vec<GraphicsState>,
}

impl StateStack {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `q`. At the limit the state is not saved, because a stream that pushes forever
    /// must not be able to exhaust memory; the drawing continues, just without the
    /// ability to go back further.
    pub fn push(&mut self, state: &GraphicsState) {
        if self.saved.len() < MAX_STATE_DEPTH {
            self.saved.push(state.clone());
        }
    }

    /// `Q`. An unbalanced `Q` is ignored rather than fatal.
    pub fn pop(&mut self) -> Option<GraphicsState> {
        self.saved.pop()
    }

    #[must_use]
    pub fn depth(&self) -> usize {
        self.saved.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.saved.is_empty()
    }
}

/// Every `/ExtGState` dictionary a page's resources name, keyed by name.
///
/// A resource table can name a dictionary that does not exist; that is reported by the
/// interpreter when it looks the name up and finds nothing, rather than here, where
/// there is no page to report it against.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExtGStates {
    /// The dictionaries, by name.
    pub entries: BTreeMap<String, ExtGState>,
    /// The names that were named but not defined. A page that used one was drawn with a
    /// state we do not know, which is a finding rather than a reason to draw nothing.
    pub missing: Vec<String>,
}

impl ExtGStates {
    /// Read a `/ExtGState` resource dictionary.
    #[must_use]
    pub fn from_dict(d: &Dict, resolve: &dyn Fn(&[u8]) -> Option<Object>) -> Self {
        let mut entries = BTreeMap::new();
        let mut missing = Vec::new();
        for (key, _) in d.iter() {
            let Some(obj) = resolve(key.as_bytes()) else {
                missing.push(String::from_utf8_lossy(key.as_bytes()).into_owned());
                continue;
            };
            let obj = match obj {
                Object::Ref(r) => match resolve_object_number(r.num) {
                    Some(o) => o,
                    None => {
                        missing.push(String::from_utf8_lossy(key.as_bytes()).into_owned());
                        continue;
                    }
                },
                other => other,
            };
            let Some(inner) = obj.as_dict() else {
                missing.push(String::from_utf8_lossy(key.as_bytes()).into_owned());
                continue;
            };
            entries.insert(
                String::from_utf8_lossy(key.as_bytes()).into_owned(),
                ExtGState::from_dict(inner),
            );
        }
        Self { entries, missing }
    }

    #[must_use]
    pub fn get(&self, name: &[u8]) -> Option<&ExtGState> {
        self.entries.get(std::str::from_utf8(name).ok()?)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many dictionaries the table holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.entries.keys()
    }
}

/// A resolver stand-in for the case where a value is already a direct dictionary. Kept
/// separate from the closure so a caller does not have to model references to reach
/// their own dictionaries.
fn resolve_object_number(_num: u32) -> Option<Object> {
    None
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect`, which is what a test is for; the
    // panic-free rule is about what the product does with a file, not about tests.
    //
    // The defaults are compared exactly because they are assigned from literals and no
    // arithmetic touches them, so a tolerance would only hide a real change.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::float_cmp,
        clippy::indexing_slicing
    )]

    use super::*;

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn the_default_state_is_the_specification_default() {
        let s = GraphicsState::new();
        assert!(s.ctm.is_identity());
        assert_eq!(s.stroke.width, 1.0, "the default line width is one");
        assert_eq!(s.stroke.cap, LineCap::Butt);
        assert_eq!(s.stroke.join, LineJoin::Miter);
        assert_eq!(s.stroke.miter_limit, 10.0);
        assert_eq!(s.text.horizontal_scale, 100.0);
        assert_eq!(s.text.render_mode, RenderMode::Fill);
        assert_eq!((s.fill_alpha, s.stroke_alpha), (1.0, 1.0));
        assert_eq!(s.blend_mode, "Normal");
        assert!(s.stroke.dash.is_solid(), "no dash array means solid");
    }

    #[test]
    fn q_and_q_restore_everything() {
        let mut s = GraphicsState::new();
        let mut stack = StateStack::new();
        s.stroke.width = 4.0;
        stack.push(&s);

        s.stroke.width = 9.0;
        s.concat(Matrix::translate(5.0, 5.0));
        s.text.size = 72.0;
        s.fill_alpha = 0.25;
        let path_len = s.path.len();
        let _ = path_len;

        let restored = stack.pop().expect("a saved state");
        assert_eq!(restored.stroke.width, 4.0);
        assert!(restored.ctm.is_identity());
        assert_eq!(restored.text.size, 0.0);
        assert_eq!(restored.fill_alpha, 1.0);
    }

    #[test]
    fn an_unbalanced_q_is_ignored() {
        let mut stack = StateStack::new();
        assert!(stack.pop().is_none());
        assert!(stack.is_empty());
    }

    #[test]
    fn the_state_stack_is_bounded() {
        let mut stack = StateStack::new();
        let s = GraphicsState::new();
        for _ in 0..(MAX_STATE_DEPTH * 3) {
            stack.push(&s);
        }
        assert_eq!(
            stack.depth(),
            MAX_STATE_DEPTH,
            "a stream that pushes forever must not exhaust memory"
        );
    }

    #[test]
    fn bt_resets_the_text_matrices_to_the_identity() {
        let mut s = GraphicsState::new();
        s.move_text(10.0, 20.0);
        s.begin_text();
        assert!(s.text_matrix.is_identity());
        assert!(s.line_matrix.is_identity());
    }

    #[test]
    fn td_offsets_from_the_line_matrix_and_t_star_uses_the_leading() {
        let mut s = GraphicsState::new();
        s.begin_text();
        s.move_text(10.0, 20.0);
        assert_eq!(s.text_matrix.apply(0.0, 0.0), (10.0, 20.0));

        s.move_text(5.0, 5.0);
        assert_eq!(s.text_matrix.apply(0.0, 0.0), (15.0, 25.0));

        s.text.leading = 14.0;
        s.next_line();
        assert_eq!(s.text_matrix.apply(0.0, 0.0), (15.0, 11.0));
    }

    #[test]
    fn the_text_rendering_matrix_includes_the_size_and_the_rise() {
        let mut s = GraphicsState::new();
        s.concat(Matrix::scale(2.0, 2.0));
        s.begin_text();
        s.move_text(3.0, 4.0);
        s.text.size = 10.0;
        s.text.rise = 1.0;
        let m = s.text_rendering_matrix();
        // The point (0, 1) is one unit above the baseline, in glyph space. It comes out
        // as: the text origin at (3, 4) doubled by the transformation to (6, 8), plus
        // one em of ten doubled to twenty, plus a rise of one doubled to two. 8 + 20 +
        // 2 is 30.
        let (x, y) = m.apply(0.0, 1.0);
        assert!(near(x, 6.0) && near(y, 30.0), "got ({x}, {y})");
    }

    #[test]
    fn a_path_starts_at_the_first_segment_and_paints_from_the_second() {
        let mut s = GraphicsState::new();
        assert!(!s.has_path(), "an empty path paints nothing");
        s.move_to(0.0, 0.0);
        assert!(!s.has_path(), "a move alone paints nothing");
        s.line_to(10.0, 0.0);
        assert!(s.has_path());
    }

    #[test]
    fn a_line_with_no_current_point_becomes_a_move() {
        let mut s = GraphicsState::new();
        s.line_to(5.0, 5.0);
        assert!(matches!(s.path.first(), Some(PathSegment::Move(5.0, 5.0))));
        assert!(
            !s.has_path(),
            "which is still a move, so still nothing to paint"
        );
    }

    #[test]
    fn close_returns_to_the_subpath_start() {
        let mut s = GraphicsState::new();
        s.move_to(3.0, 4.0);
        s.line_to(10.0, 10.0);
        s.close_path();
        assert_eq!(s.current_point, Some((3.0, 4.0)));
        assert!(s.path.contains(&PathSegment::Close));
    }

    #[test]
    fn the_device_path_follows_the_transformation() {
        let mut s = GraphicsState::new();
        s.concat(Matrix::translate(100.0, 0.0));
        s.move_to(1.0, 2.0);
        s.line_to(3.0, 4.0);
        let device = s.device_path();
        assert!(
            matches!(device.first(), Some(PathSegment::Move(x, y)) if near(*x, 101.0) && near(*y, 2.0))
        );
        assert!(
            matches!(device.get(1), Some(PathSegment::Line(x, y)) if near(*x, 103.0) && near(*y, 4.0))
        );
    }

    #[test]
    fn path_bounds_cover_every_point_of_the_path() {
        let mut s = GraphicsState::new();
        assert!(s.path_bounds().is_none());
        s.move_to(-5.0, 10.0);
        s.line_to(20.0, -2.0);
        let b = s.path_bounds().expect("bounds");
        assert!(b.x0 == -5.0 && b.x1 == 20.0 && b.y0 == -2.0 && b.y1 == 10.0);
    }

    #[test]
    fn two_clips_intersect_rather_than_overwrite() {
        let a = ClipBounds {
            x0: 0.0,
            y0: 0.0,
            x1: 100.0,
            y1: 100.0,
        };
        let b = ClipBounds {
            x0: 50.0,
            y0: 50.0,
            x1: 200.0,
            y1: 200.0,
        };
        let i = a.intersect(b).expect("an overlap");
        assert!(i.x0 == 50.0 && i.y0 == 50.0 && i.x1 == 100.0 && i.y1 == 100.0);

        let outside = ClipBounds {
            x0: 500.0,
            y0: 500.0,
            x1: 600.0,
            y1: 600.0,
        };
        assert!(
            a.intersect(outside).is_none(),
            "clips that do not overlap mean nothing is drawn, not everything"
        );
    }

    #[test]
    fn a_colour_takes_the_count_its_space_needs() {
        let mut c = Colour::black();
        c.set(ColourSpace::device_rgb(), &[0.2, 0.4]);
        assert_eq!(
            c.components.len(),
            3,
            "a missing component is drawn as zero"
        );
        assert_eq!(c.components[2], 0.0);

        c.set(ColourSpace::device_gray(), &[0.5, 0.5, 0.5]);
        assert_eq!(c.components.len(), 1, "extra components are dropped");
    }

    #[test]
    fn a_colour_outside_its_range_is_clamped() {
        let mut c = Colour::black();
        c.set(ColourSpace::device_rgb(), &[-1.0, 2.0, 0.5]);
        assert_eq!(c.components[0], 0.0);
        assert_eq!(c.components[1], 1.0);
    }

    #[test]
    fn grey_and_rgb_read_the_same_when_they_are_the_same_colour() {
        let mut g = Colour::black();
        g.set(ColourSpace::device_gray(), &[0.5]);
        let mut rgb = Colour::black();
        rgb.set(ColourSpace::device_rgb(), &[0.5, 0.5, 0.5]);
        assert!(near(g.as_gray(), rgb.as_gray()));
    }

    #[test]
    fn a_dash_of_zeros_means_solid() {
        assert!(Dash::default().is_solid());
        assert!(
            Dash {
                array: vec![0.0, 0.0],
                phase: 0.0
            }
            .is_solid()
        );
        assert!(
            Dash {
                array: vec![-1.0, 2.0],
                phase: 0.0
            }
            .is_solid()
        );
        assert!(
            !Dash {
                array: vec![3.0, 2.0],
                phase: 0.0
            }
            .is_solid()
        );
    }

    #[test]
    fn an_extgstate_sets_only_what_it_carries() {
        let mut s = GraphicsState::new();
        s.text.size = 24.0;
        let mut d = Dict::new();
        d.set("LW", Object::Real(3.0));
        ExtGState::from_dict(&d).apply(&mut s);
        assert_eq!(s.stroke.width, 3.0);
        assert_eq!(
            s.text.size, 24.0,
            "a /gs that does not mention the font must not reset it"
        );
    }

    #[test]
    fn an_extgstate_carries_both_spellings_of_its_keys() {
        // The specification's own names are the short ones.
        let mut s = GraphicsState::new();
        let mut short = Dict::new();
        short.set("LW", Object::Real(2.0));
        short.set("LC", Object::Int(1));
        short.set("LJ", Object::Int(2));
        short.set("ML", Object::Real(4.0));
        short.set("CA", Object::Real(0.5));
        short.set("ca", Object::Real(0.75));
        ExtGState::from_dict(&short).apply(&mut s);
        assert_eq!(s.stroke.width, 2.0);
        assert_eq!(s.stroke.cap, LineCap::Round);
        assert_eq!(s.stroke.join, LineJoin::Bevel);
        assert_eq!(s.stroke.miter_limit, 4.0);
        assert!(near(s.stroke_alpha, 0.5));
        assert!(near(s.fill_alpha, 0.75));

        // A producer that spells them out is understood too, where the name is clear.
        let mut long = Dict::new();
        long.set("LineWidth", Object::Real(7.0));
        long.set("LineCap", Object::Int(2));
        long.set("MiterLimit", Object::Real(9.0));
        ExtGState::from_dict(&long).apply(&mut s);
        assert_eq!(s.stroke.width, 7.0);
        assert_eq!(s.stroke.cap, LineCap::Square);
        assert_eq!(s.stroke.miter_limit, 9.0);
    }

    #[test]
    fn an_alpha_outside_zero_to_one_is_clamped() {
        let mut s = GraphicsState::new();
        let mut d = Dict::new();
        d.set("ca", Object::Real(5.0));
        d.set("CA", Object::Real(-1.0));
        ExtGState::from_dict(&d).apply(&mut s);
        assert_eq!(s.fill_alpha, 1.0);
        assert_eq!(s.stroke_alpha, 0.0);
    }

    #[test]
    fn a_dash_dictionary_is_read() {
        let mut d = Dict::new();
        d.set(
            "D",
            Object::Array(vec![Object::Array(vec![
                Object::Real(3.0),
                Object::Real(2.0),
            ])]),
        );
        d.set("Phase", Object::Real(1.0));
        let dash = read_dash(&d).expect("a dash");
        assert_eq!(dash.array, vec![3.0, 2.0]);
        assert!(near(dash.phase, 1.0));
        assert!(!dash.is_solid());
    }

    #[test]
    fn an_unbalanced_restore_does_not_walk_off_the_stack() {
        let mut stack = StateStack::new();
        let s = GraphicsState::new();
        stack.push(&s);
        assert!(stack.pop().is_some());
        for _ in 0..10 {
            assert!(stack.pop().is_none());
        }
    }
}
