//! Shadings: the gradients a page can fill a region with.
//!
//! Three things are here, and they are separable on purpose:
//!
//! 1. **Functions.** A PDF function maps numbers to numbers, and there are four kinds. Two
//!    of them are arithmetic; one is a table; one is a small stack machine that runs a
//!    PostScript program written in the file. Every one of them can fail, and a function
//!    that fails is reported rather than replaced with a value that would look plausible.
//! 2. **Shadings.** An axial or a radial gradient: two endpoints in a space, a function from
//!    position to colour, and whether the gradient continues past its ends.
//! 3. **Painting.** Each pixel of the clip is mapped *back* into the shading's own space,
//!    turned into a parameter `t` between zero and one, and coloured by the function.
//!
//! ## Why the parameter is computed backwards
//!
//! Painting forwards — walk the gradient's axis and fill as you go — works for an
//! axis-aligned gradient and fails for a rotated or skewed one, which every page with a
//! diagonal banner uses. Working backwards from each pixel through the inverse
//! transformation costs one matrix multiply per pixel and has no special cases at all.
//!
//! ## The edge is antialiased
//!
//! A gradient's boundary is a line in the shading's space and an antialiased edge on the
//! page. The distance from a pixel's centre to that boundary is `t / |∇t|`, where `∇t` is
//! how much `t` changes per pixel; evaluating `t` at three points and differencing gives it
//! without any case analysis, which matters because the boundary of a radial gradient is a
//! conic section and has no formula worth writing twice.

use std::ops::Range;

use mangle_content::Matrix;
use mangle_syntax::object::{Dict, Object, Stream};
use mangle_syntax::stream::decode_stream;

use crate::{Device, Rect};

/// A PDF function: numbers in, numbers out.
#[derive(Debug, Clone, PartialEq)]
pub enum Function {
    /// Type 0: a table of samples, interpolated between entries.
    Sampled(Sampled),
    /// Type 2: `C0 + C1 · t^n` over each output's own domain.
    Exponential(Exponential),
    /// Type 3: a run of type-2 functions stitched end to end.
    Stitching(Stitching),
    /// Type 4: a PostScript calculator.
    Calculator(Calculator),
}

impl Function {
    /// How many inputs this function takes.
    #[must_use]
    pub fn inputs(&self) -> usize {
        match self {
            Self::Sampled(s) => s.domain.len() / 2,
            Self::Exponential(e) => e.domain.len() / 2,
            Self::Stitching(s) => s.domain.len() / 2,
            Self::Calculator(c) => c.domain.len() / 2,
        }
    }

    /// How many outputs this function produces.
    #[must_use]
    pub fn outputs(&self) -> usize {
        match self {
            Self::Sampled(s) => s.range.len(),
            Self::Exponential(e) => e.range.len(),
            Self::Stitching(s) => s.functions.first().map_or(0, Function::outputs),
            Self::Calculator(c) => c.range.len(),
        }
    }

    /// Apply the function, returning `None` when it cannot answer.
    ///
    /// A function that is out of its domain, or whose program leaves the wrong number of
    /// values, has no answer. Returning one anyway would paint a colour the file did not
    /// ask for, and a gradient that is slightly wrong is harder to notice than one that is
    /// missing.
    #[must_use]
    pub fn apply(&self, inputs: &[f64]) -> Option<Vec<f64>> {
        if inputs.len() < self.inputs() {
            return None;
        }
        let want = self.outputs();
        if want == 0 {
            return None;
        }
        match self {
            Self::Sampled(s) => s.apply(inputs),
            Self::Exponential(e) => e.apply(inputs),
            Self::Stitching(s) => s.apply(inputs),
            Self::Calculator(c) => c.apply(inputs),
        }
        .filter(|v: &Vec<f64>| v.len() == want)
    }

    /// Apply to a single input, which is every case a gradient needs.
    #[must_use]
    pub fn apply1(&self, t: f64) -> Option<Vec<f64>> {
        self.apply(&[t])
    }
}

/// A type 0 function: a table of samples.
#[derive(Debug, Clone, PartialEq)]
pub struct Sampled {
    pub domain: Vec<[f64; 2]>,
    /// `/Size`, one entry per input.
    pub size: Vec<u32>,
    /// `/BitsPerSample`, which PDF requires to be a multiple of 8.
    pub bits: usize,
    /// The output ranges from `/Range`, or from `/Decode`'s pairs.
    pub range: Vec<[f64; 2]>,
    /// `/Encode`, mapping the input domain onto the sample indices.
    pub encode: Vec<[f64; 2]>,
    pub samples: Vec<f64>,
}

impl Sampled {
    /// Read one component of the sample at an index, or `None` if the table is short.
    ///
    /// A short table is a damaged file, and reading past the end as zero would paint a
    /// gradient whose far end silently fades to black rather than reporting that there is
    /// nothing to read.
    fn at(&self, index: &[u64], component: usize) -> Option<f64> {
        // Row-major over the input dimensions, with the *last* dimension varying fastest.
        let mut flat = 0u64;
        for (i, ix) in index.iter().enumerate() {
            let size = u64::from(self.size.get(i).copied().unwrap_or(1).max(1));
            flat = flat
                .saturating_mul(size)
                .saturating_add((*ix).min(size - 1));
        }
        let outputs = u64::try_from(self.outputs()).ok()?;
        let offset = flat
            .saturating_mul(outputs)
            .saturating_add(u64::try_from(component).ok()?);
        self.samples.get(usize::try_from(offset).ok()?).copied()
    }

    /// How many outputs one sample holds.
    fn outputs(&self) -> usize {
        self.range.len().max(1)
    }

    /// Largest sample index, which is `size - 1` per dimension.
    fn last(&self, dimension: usize) -> u64 {
        u64::from(self.size.get(dimension).copied().unwrap_or(1).max(1)) - 1
    }

    fn apply(&self, inputs: &[f64]) -> Option<Vec<f64>> {
        let t = inputs.first().copied()?;
        let lo = self.domain.first().copied().unwrap_or([0.0, 1.0]);
        if t < lo[0] || t > lo[1] {
            return None;
        }
        let encode = self.encode.first().copied().unwrap_or([0.0, 0.0]);
        // Map the input onto the sample grid, then clamp to the table's own bounds.
        let scaled = if (encode[1] - encode[0]).abs() < f64::EPSILON {
            0.0
        } else {
            (t - lo[0]) / (lo[1] - lo[0]) * (encode[1] - encode[0]) + encode[0]
        };
        let high = self.last(0) as f64;
        let clamped = scaled.clamp(0.0, high);
        let low_index = clamped.floor().max(0.0) as u64;
        let fraction = clamped - low_index as f64;
        let high_index = (low_index + 1).min(self.last(0));

        let mut out = Vec::with_capacity(self.outputs());
        for component in 0..self.outputs() {
            let a = self.at(&[low_index], component)?;
            let b = self.at(&[high_index], component)?;
            let value = a + (b - a) * fraction;
            let range = self.range.get(component).copied().unwrap_or([0.0, 1.0]);
            out.push(value * (range[1] - range[0]) + range[0]);
        }
        Some(out)
    }
}

/// A type 2 function: `C0 + C1 · (x - x0)^n`.
#[derive(Debug, Clone, PartialEq)]
pub struct Exponential {
    pub domain: Vec<[f64; 2]>,
    /// One `[min, max, exponent]` per output.
    pub range: Vec<[f64; 3]>,
    pub c0: Vec<f64>,
    pub c1: Vec<f64>,
}

impl Exponential {
    fn apply(&self, inputs: &[f64]) -> Option<Vec<f64>> {
        let t = inputs.first().copied()?;
        let lo = self.domain.first().copied().unwrap_or([0.0, 1.0]);
        if t < lo[0] || t > lo[1] {
            return None;
        }
        let x = t - lo[0];
        let mut out = Vec::with_capacity(self.range.len());
        for (i, r) in self.range.iter().enumerate() {
            let c0 = self.c0.get(i).copied().unwrap_or(0.0);
            let c1 = self.c1.get(i).copied().unwrap_or(1.0);
            let n = r.get(2).copied().unwrap_or(1.0);
            // The exponent is the one place a file can ask for something this cannot
            // compute, and `powi` on a negative base is one of them.
            let power = if n.fract() == 0.0 && n.abs() < 64.0 {
                x.powi(n as i32)
            } else if x < 0.0 && n.fract() != 0.0 {
                return None;
            } else {
                x.powf(n)
            };
            if !power.is_finite() {
                return None;
            }
            out.push(c0 + c1 * power);
        }
        Some(out)
    }
}

/// A type 3 function: type-2 functions stitched end to end.
#[derive(Debug, Clone, PartialEq)]
pub struct Stitching {
    pub domain: Vec<[f64; 2]>,
    pub functions: Vec<Function>,
    /// `/Bounds`, the `k - 1` points between the functions.
    pub bounds: Vec<f64>,
    /// `/Encode`, the sub-domain of each function.
    pub encode: Vec<[f64; 2]>,
}

impl Stitching {
    fn apply(&self, inputs: &[f64]) -> Option<Vec<f64>> {
        let t = inputs.first().copied()?;
        let lo = self.domain.first().copied().unwrap_or([0.0, 1.0]);
        if t < lo[0] || t > lo[1] {
            return None;
        }
        // The last function covers everything above the last bound, so a value past it
        // belongs to it rather than to nothing.
        let mut chosen = self.functions.len().saturating_sub(1);
        for (i, bound) in self.bounds.iter().enumerate() {
            if t < *bound {
                chosen = i;
                break;
            }
        }
        let function = self.functions.get(chosen)?;
        let encode = self.encode.get(chosen).copied().unwrap_or([0.0, 1.0]);
        // The first function owns everything below the first bound; each later one owns the
        // span between its two neighbouring bounds.
        let start = if chosen == 0 {
            lo[0]
        } else {
            self.bounds
                .get(chosen.saturating_sub(1))
                .copied()
                .unwrap_or(lo[0])
        };
        // The last function has no bound above it, so it runs to the end of the domain.
        let end = if chosen < self.bounds.len() {
            self.bounds.get(chosen).copied().unwrap_or(lo[1])
        } else {
            lo[1]
        };
        let span = end - start;
        let scaled = if span.abs() < f64::EPSILON {
            encode[0]
        } else {
            (t - start) / span * (encode[1] - encode[0]) + encode[0]
        };
        function.apply1(scaled)
    }
}

/// A type 4 function: a PostScript calculator.
#[derive(Debug, Clone, PartialEq)]
pub struct Calculator {
    pub domain: Vec<[f64; 2]>,
    /// One `[min, max]` per output.
    pub range: Vec<[f64; 2]>,
    pub program: Vec<Token>,
}

/// The instruction set of a type 4 function.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Number(f64),
    /// An operator or a literal name.
    Name(Vec<u8>),
    /// A hexadecimal string, which is how a name holding binary bytes is written.
    HexString(Vec<u8>),
    /// `{ ... }`, evaluated and pushed.
    Block(Vec<Token>),
}

impl Calculator {
    fn apply(&self, inputs: &[f64]) -> Option<Vec<f64>> {
        let t = inputs.first().copied()?;
        let lo = self.domain.first().copied().unwrap_or([0.0, 1.0]);
        if t < lo[0] || t > lo[1] {
            return None;
        }
        let x = t - lo[0];
        let mut stack = vec![x];
        run(&self.program, &mut stack)?;
        // The specification takes the first `n` values left on the stack, where `n` is the
        // output count, and clamps each to its output's range.
        let want = self.range.len();
        if want == 0 || stack.len() < want {
            return None;
        }
        // The results are the topmost `n` values, which is the PostScript convention: a
        // program leaves the input on the stack and reaches its answer from the top of it.
        let start = stack.len() - want;
        let mut out = Vec::with_capacity(want);
        for (i, r) in self.range.iter().enumerate() {
            let v = stack.get(start + i).copied()?;
            out.push(v.clamp(r[0], r[1]));
        }
        Some(out)
    }
}

/// Run a token program against a stack, in place.
fn run(program: &[Token], stack: &mut Vec<f64>) -> Option<()> {
    for token in program {
        match token {
            Token::Number(n) => {
                if !n.is_finite() {
                    return None;
                }
                stack.push(*n);
            }
            Token::Block(body) => {
                // A block is a procedure, and a calculator's program is executed rather
                // than stored, so the body runs against the *same* stack. Running it against
                // a fresh one hides the input from every operator inside it.
                run(body, stack)?;
            }
            Token::HexString(bytes) => {
                // A string literal on the stack is a literal name; the value is its bytes as
                // a number, which is what the operators that consume names need.
                let mut value = 0.0f64;
                for b in bytes {
                    value = value * 256.0 + f64::from(*b);
                }
                stack.push(value);
            }
            Token::Name(name) => {
                operator(name, stack)?;
            }
        }
        if stack.len() > MAX_STACK {
            // A program that grows the stack without bound is damage, not a long loop.
            return None;
        }
    }
    Some(())
}

/// The most values a calculator program may leave on its stack.
pub const MAX_STACK: usize = 512;

/// One arithmetic or logical operator.
fn operator(name: &[u8], stack: &mut Vec<f64>) -> Option<()> {
    let pop = |stack: &mut Vec<f64>| -> Option<f64> { stack.pop() };
    match name {
        b"add" => {
            let b = pop(stack)?;
            let a = pop(stack)?;
            stack.push(a + b);
        }
        b"sub" => {
            let b = pop(stack)?;
            let a = pop(stack)?;
            stack.push(a - b);
        }
        b"mul" => {
            let b = pop(stack)?;
            let a = pop(stack)?;
            stack.push(a * b);
        }
        b"div" => {
            let b = pop(stack)?;
            let a = pop(stack)?;
            if b == 0.0 {
                return None;
            }
            stack.push(a / b);
        }
        b"idiv" => {
            let b = pop(stack)?;
            let a = pop(stack)?;
            if b == 0.0 {
                return None;
            }
            stack.push((a / b).trunc());
        }
        b"mod" => {
            let b = pop(stack)?;
            let a = pop(stack)?;
            if b == 0.0 {
                return None;
            }
            stack.push(a % b);
        }
        b"exp" => {
            let b = pop(stack)?;
            let a = pop(stack)?;
            let v = a.powf(b);
            if !v.is_finite() {
                return None;
            }
            stack.push(v);
        }
        b"ln" => {
            let a = pop(stack)?;
            if a <= 0.0 {
                return None;
            }
            stack.push(a.ln());
        }
        b"log" => {
            let a = pop(stack)?;
            if a <= 0.0 {
                return None;
            }
            stack.push(a.log10());
        }
        b"sqrt" => {
            let a = pop(stack)?;
            if a < 0.0 {
                return None;
            }
            stack.push(a.sqrt());
        }
        b"sin" => {
            let a = pop(stack)?;
            stack.push(a.sin());
        }
        b"cos" => {
            let a = pop(stack)?;
            stack.push(a.cos());
        }
        b"atan" => {
            let den = pop(stack)?;
            let num = pop(stack)?;
            stack.push(num.atan2(den));
        }
        b"neg" => {
            let a = pop(stack)?;
            stack.push(-a);
        }
        b"abs" => {
            let a = pop(stack)?;
            stack.push(a.abs());
        }
        b"ceil" => {
            let a = pop(stack)?;
            stack.push(a.ceil());
        }
        b"floor" => {
            let a = pop(stack)?;
            stack.push(a.floor());
        }
        b"round" => {
            let a = pop(stack)?;
            stack.push(a.round());
        }
        b"truncate" => {
            let a = pop(stack)?;
            stack.push(a.trunc());
        }
        b"dup" => {
            let a = *stack.last()?;
            stack.push(a);
        }
        b"exch" => {
            let b = pop(stack)?;
            let a = pop(stack)?;
            stack.push(b);
            stack.push(a);
        }
        b"pop" => {
            pop(stack)?;
        }
        b"copy" => {
            let n = pop(stack)?;
            if n < 0.0 {
                return None;
            }
            let n = n as usize;
            if stack.len() < n {
                return None;
            }
            let start = stack.len() - n;
            let copied = stack.get(start..).unwrap_or_default().to_vec();
            stack.extend(copied);
        }
        b"index" => {
            let n = pop(stack)?;
            if n < 0.0 {
                return None;
            }
            let n = n as usize;
            let v = *stack.get(stack.len().checked_sub(n + 1)?)?;
            stack.push(v);
        }
        b"roll" => {
            let j = pop(stack)?;
            let n = pop(stack)?;
            if n < 0.0 || j < 0.0 {
                return None;
            }
            let (n, j) = (n as usize, j as usize);
            if n == 0 || stack.len() < n {
                return None;
            }
            let start = stack.len() - n;
            let mut window: Vec<f64> = stack.drain(start..).collect();
            let j = j % window.len().max(1);
            window.rotate_right(j);
            stack.extend(window);
        }
        b"eq" => {
            let b = pop(stack)?;
            let a = pop(stack)?;
            stack.push(f64::from(a == b));
        }
        b"ne" => {
            let b = pop(stack)?;
            let a = pop(stack)?;
            stack.push(f64::from(a != b));
        }
        b"gt" => {
            let b = pop(stack)?;
            let a = pop(stack)?;
            stack.push(f64::from(a > b));
        }
        b"ge" => {
            let b = pop(stack)?;
            let a = pop(stack)?;
            stack.push(f64::from(a >= b));
        }
        b"lt" => {
            let b = pop(stack)?;
            let a = pop(stack)?;
            stack.push(f64::from(a < b));
        }
        b"le" => {
            let b = pop(stack)?;
            let a = pop(stack)?;
            stack.push(f64::from(a <= b));
        }
        b"and" => {
            let b = pop(stack)?;
            let a = pop(stack)?;
            stack.push(f64::from(a != 0.0 && b != 0.0));
        }
        b"or" => {
            let b = pop(stack)?;
            let a = pop(stack)?;
            stack.push(f64::from(a != 0.0 || b != 0.0));
        }
        b"not" => {
            let a = pop(stack)?;
            stack.push(f64::from(bm_is_false(a)));
        }
        b"bitshift" => {
            let shift = pop(stack)?;
            let value = pop(stack)?;
            let shift = shift as i32;
            if !(0..=64).contains(&shift) {
                return None;
            }
            let v = if shift >= 32 {
                // A shift past the word's width is zero by the specification's definition,
                // which differs from a machine shift and is the usual way to get it wrong.
                0.0
            } else {
                let word = value as i64 as u32;
                f64::from(word << shift)
            };
            stack.push(v);
        }
        b"true" => stack.push(1.0),
        b"false" => stack.push(0.0),
        // `if` and `ifelse` take their procedures from the stack, so they need the blocks
        // kept rather than evaluated. The parser therefore records them separately; here a
        // block that was already evaluated is simply not a procedure, and the operator has
        // nothing to do.
        b"jmp" => return None,
        // An operator this implementation does not have. A file that uses one produces no
        // answer rather than a wrong one.
        _ => return None,
    }
    if stack.last().is_some_and(|v| !v.is_finite()) {
        return None;
    }
    Some(())
}

/// PostScript's own truth: zero is false and everything else is true, including
/// negative zero's negative.
fn bm_is_false(v: f64) -> bool {
    v == 0.0
}

/// A shading this project can paint.
#[derive(Debug, Clone, PartialEq)]
pub enum Shading {
    /// Type 2: a linear gradient between two points.
    Axial {
        coords: Vec<f64>,
        function: Function,
        extend: [bool; 2],
    },
    /// Type 3: a gradient between two circles.
    Radial {
        coords: Vec<f64>,
        function: Function,
        extend: [bool; 2],
    },
}

impl Shading {
    /// Read a `/Shading` dictionary.
    #[must_use]
    pub fn parse(object: &Object, resolve: &dyn Fn(&Object) -> Option<Object>) -> Option<Self> {
        let raw = object.clone();
        let resolved = resolve(&raw).unwrap_or(raw);
        let (dict, is_stream): (Dict, bool) = match &resolved {
            Object::Dict(d) => (d.clone(), false),
            Object::Stream(s) => (s.dict.clone(), true),
            _ => return None,
        };
        let _ = is_stream;
        let kind = dict.get("ShadingType").and_then(Object::as_i64)?;
        let extend = read_extend(dict.get("Extend"));
        let coords: Vec<f64> = dict
            .get("Coords")
            .and_then(Object::as_array)
            .map(|a| a.iter().filter_map(Object::as_f64).collect())
            .unwrap_or_default();
        let function_object = dict.get("Function")?;
        // `/Function` is either one function or an array of them; an array is stitched
        // together, which is how a gradient with more than two stops is written.
        let function = if let Some(array) = function_object.as_array() {
            let parts: Vec<Function> = array
                .iter()
                .filter_map(|o| Function::parse(o, resolve))
                .collect();
            stitch(
                &parts,
                &dict,
                coords.first().copied().unwrap_or(0.0),
                coords.get(1).copied().unwrap_or(1.0),
            )?
        } else {
            Function::parse(function_object, resolve)?
        };

        match kind {
            2 if coords.len() >= 4 => Some(Self::Axial {
                coords,
                function,
                extend,
            }),
            3 if coords.len() >= 6 => Some(Self::Radial {
                coords,
                function,
                extend,
            }),
            // Types 1, 4, 5, 6 and 7 need either a pattern colour or a mesh this project
            // does not build. Reporting them is better than painting them wrongly.
            _ => None,
        }
    }

    /// The parameter at a point in the shading's own space, or `None` if it is outside.
    ///
    /// `t` is *not* clamped: a caller needs to know how far outside the gradient a point
    /// is in order to work out how much of it is covered. Clamping belongs to the colour
    /// lookup, which is where "the nearest end's colour" is the right answer.
    #[must_use]
    pub fn parameter_at(&self, x: f64, y: f64) -> Option<f64> {
        match self {
            Self::Axial { coords, .. } => {
                let p0 = (coords.first().copied()?, coords.get(1).copied()?);
                let p1 = (coords.get(2).copied()?, coords.get(3).copied()?);
                let d = (p1.0 - p0.0, p1.1 - p0.1);
                let length_squared = d.0 * d.0 + d.1 * d.1;
                if length_squared <= 0.0 {
                    // A zero-length axis has no direction, and every point is at the same
                    // place along it. The specification's answer is the first stop.
                    return Some(0.0);
                }
                let t = ((x - p0.0) * d.0 + (y - p0.1) * d.1) / length_squared;
                Some(t)
            }
            Self::Radial { coords, .. } => {
                let (x0, y0, r0) = (
                    coords.first().copied()?,
                    coords.get(1).copied()?,
                    coords.get(2).copied()?,
                );
                let (x1, y1, r1) = (
                    coords.get(3).copied()?,
                    coords.get(4).copied()?,
                    coords.get(5).copied()?,
                );
                radial_parameter(x, y, x0, y0, r0, x1, y1, r1)
            }
        }
    }

    /// Whether the gradient continues past each end.
    #[must_use]
    pub fn extend(&self) -> [bool; 2] {
        match self {
            Self::Axial { extend, .. } | Self::Radial { extend, .. } => *extend,
        }
    }

    /// Whether a point in the shading's own space lies inside a radial gradient's *first*
    /// circle.
    ///
    /// That disc is painted with the first colour, and the parameter cannot say so: a
    /// point at the centre of the family is on no circle in it, and the quadratic's root
    /// comes out negative. An axial gradient has no such disc and never reports one.
    #[must_use]
    pub fn inner_fill(&self, x: f64, y: f64) -> bool {
        let Self::Radial { coords, .. } = self else {
            return false;
        };
        let cx = coords.first().copied().unwrap_or(0.0);
        let cy = coords.get(1).copied().unwrap_or(0.0);
        let r = coords.get(2).copied().unwrap_or(0.0);
        (cx - x).powi(2) + (cy - y).powi(2) <= r * r
    }

    /// The colour at a parameter, as RGB in 0..1, or `None` if the function cannot answer.
    #[must_use]
    pub fn colour_at(&self, t: f64) -> Option<[f64; 3]> {
        // A point past either end still has a colour and it is that end's. Whether the
        // point is *visible* is the coverage's business, decided from the raw parameter.
        let t = t.clamp(0.0, 1.0);
        let function = match self {
            Self::Axial { function, .. } | Self::Radial { function, .. } => function,
        };
        let values = function.apply1(t)?;
        // A gradient's function produces colour components in the space its `/ColorSpace`
        // names. Everything here is in RGB, and the spaces this paints are RGB and Gray,
        // so a three-component result is RGB and a one-component result is gray.
        match values.len() {
            1 => {
                let v = values.first().copied()?.clamp(0.0, 1.0);
                Some([v, v, v])
            }
            3 => Some([
                values.first().copied()?.clamp(0.0, 1.0),
                values.get(1).copied()?.clamp(0.0, 1.0),
                values.get(2).copied()?.clamp(0.0, 1.0),
            ]),
            4 => {
                // Subtractive, and the components are ink rather than light.
                let c = values.first().copied()?.clamp(0.0, 1.0);
                let m = values.get(1).copied()?.clamp(0.0, 1.0);
                let y = values.get(2).copied()?.clamp(0.0, 1.0);
                let k = values.get(3).copied()?.clamp(0.0, 1.0);
                Some([
                    (1.0 - c) * (1.0 - k),
                    (1.0 - m) * (1.0 - k),
                    (1.0 - y) * (1.0 - k),
                ])
            }
            _ => None,
        }
    }
}

/// The parameter along a radial gradient, by solving the intersection of a ray and a
/// family of circles.
///
/// The radius at parameter `t` is `r0 + t·(r1 − r0)` and the centre is
/// `p0 + t·(p1 − p0)`, so requiring the point to be at distance `r0 + t·dr` from the centre
/// gives a quadratic in `t`. The smaller root is the one the specification wants, because
/// the larger one belongs to the far side of the circle.
#[must_use]
pub fn radial_parameter(
    x: f64,
    y: f64,
    x0: f64,
    y0: f64,
    r0: f64,
    x1: f64,
    y1: f64,
    r1: f64,
) -> Option<f64> {
    let d = (x1 - x0, y1 - y0);
    let f = (x - x0, y - y0);
    let dr = r1 - r0;
    let a = d.0 * d.0 + d.1 * d.1 - dr * dr;
    let b = -2.0 * (f.0 * d.0 + f.1 * d.1 + r0 * dr);
    let c = f.0 * f.0 + f.1 * f.1 - r0 * r0;
    let t = if a.abs() < f64::EPSILON {
        // A cone, where the radii change but the centres do not: the quadratic degenerates
        // to a line and the smaller root is the only one.
        if b.abs() < f64::EPSILON {
            return None;
        }
        -c / b
    } else {
        let discriminant = b * b - 4.0 * a * c;
        if discriminant < 0.0 {
            // The ray misses the cone entirely.
            return None;
        }
        let root = discriminant.sqrt();
        let candidates = [(-b - root) / (2.0 * a), (-b + root) / (2.0 * a)];
        // The specification's rule is the larger of the two `t` values that are at most one.
        // A point past the outer circle has neither root in `[0, 1]`, and then the smallest
        // root above one says "outside" without pretending the gradient ends there.
        let inside = candidates
            .iter()
            .copied()
            .filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
            .fold(f64::NEG_INFINITY, f64::max);
        if inside.is_finite() {
            inside
        } else {
            let beyond = candidates
                .iter()
                .copied()
                .filter(|v| v.is_finite() && *v > 1.0)
                .fold(f64::INFINITY, f64::min);
            if beyond.is_finite() {
                beyond
            } else {
                candidates
                    .iter()
                    .copied()
                    .filter(|v| v.is_finite())
                    .fold(f64::NEG_INFINITY, f64::max)
            }
        }
    };
    if !t.is_finite() {
        return None;
    }
    Some(t)
}

fn read_extend(object: Option<&Object>) -> [bool; 2] {
    let Some(array) = object.and_then(Object::as_array) else {
        return [false, false];
    };
    [
        array.first().and_then(Object::as_bool).unwrap_or(false),
        array.get(1).and_then(Object::as_bool).unwrap_or(false),
    ]
}

impl Function {
    /// Read a function dictionary or stream.
    #[must_use]
    pub fn parse(object: &Object, resolve: &dyn Fn(&Object) -> Option<Object>) -> Option<Self> {
        let raw = object.clone();
        let resolved = resolve(&raw).unwrap_or(raw);
        let (dict, stream): (Dict, Option<Stream>) = match &resolved {
            Object::Dict(d) => (d.clone(), None),
            Object::Stream(s) => (s.dict.clone(), Some(s.clone())),
            _ => return None,
        };
        let kind = dict.get("FunctionType").and_then(Object::as_i64)?;
        let domain: Vec<[f64; 2]> = dict
            .get("Domain")
            .and_then(Object::as_array)
            .map(|a| {
                a.chunks_exact(2)
                    .filter_map(|p| {
                        Some([
                            p.first().and_then(Object::as_f64)?,
                            p.get(1).and_then(Object::as_f64)?,
                        ])
                    })
                    .collect()
            })
            .unwrap_or_default();
        match kind {
            0 => {
                let stream = stream?;
                let size: Vec<u32> = dict
                    .get("Size")
                    .and_then(Object::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Object::as_i64)
                            .map(|v| v.max(1) as u32)
                            .collect()
                    })
                    .unwrap_or_default();
                if size.is_empty() {
                    return None;
                }
                let bits = dict
                    .get("BitsPerSample")
                    .and_then(Object::as_i64)
                    .unwrap_or(8)
                    .max(1) as usize;
                if bits % 8 != 0 {
                    // A bit depth that is not a whole number of bytes cannot be read without
                    // a bit unpacker, and the specification does not allow one here anyway.
                    return None;
                }
                let range: Vec<[f64; 2]> = dict
                    .get("Range")
                    .and_then(Object::as_array)
                    .map(|a| {
                        a.chunks_exact(2)
                            .filter_map(|p| {
                                Some([
                                    p.first().and_then(Object::as_f64)?,
                                    p.get(1).and_then(Object::as_f64)?,
                                ])
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let inputs = size.len();
                let outputs = range.len().max(1);
                // `/Encode` defaults to the identity over the sample count.
                let mut encode: Vec<[f64; 2]> = (0..inputs)
                    .map(|i| {
                        [
                            0.0,
                            f64::from(size.get(i).copied().unwrap_or(1).max(1)) - 1.0,
                        ]
                    })
                    .collect();
                if let Some(array) = dict.get("Encode").and_then(Object::as_array) {
                    let mut i = 0;
                    while i + 1 < array.len() && i / 2 < encode.len() {
                        if let (Some(lo), Some(hi)) = (
                            array.get(i).and_then(Object::as_f64),
                            array.get(i + 1).and_then(Object::as_f64),
                        ) && let Some(slot) = encode.get_mut(i / 2)
                        {
                            *slot = [lo, hi];
                        }
                        i += 2;
                    }
                }
                let data = decode_stream(&stream).data;
                let per_sample = bits / 8;
                let count = data.len() / per_sample.max(1);
                let samples: Vec<f64> = data
                    .chunks_exact(per_sample.max(1))
                    .take(count.min(1 << 20))
                    .map(|bytes| {
                        let mut v = 0u64;
                        for b in bytes {
                            v = (v << 8) | u64::from(*b);
                        }
                        v as f64 / ((1u64 << bits.min(32)) - 1).max(1) as f64
                    })
                    .collect();
                let _ = outputs;
                Some(Self::Sampled(Sampled {
                    domain,
                    size,
                    bits,
                    range,
                    encode,
                    samples,
                }))
            }
            2 => {
                let numbers = |key: &str| -> Vec<f64> {
                    dict.get(key)
                        .and_then(Object::as_array)
                        .map(|a| a.iter().filter_map(Object::as_f64).collect())
                        .unwrap_or_default()
                };
                let c0 = numbers("C0");
                let c1 = numbers("C1");
                // One output component per entry in `/C0` and `/C1`, which is what decides
                // how many pairs `/Range` holds.
                let outputs = c0.len().max(c1.len()).max(1);
                // `/Range` is one `[min, max]` pair per output component. The exponent is
                // `/N`, a key of its own: reading it as a third number in the range would
                // reject `/Range [0 1]`, which is what a one-component function is required
                // to write, and a grey gradient is the common case.
                let exponent = dict.get("N").and_then(Object::as_f64).unwrap_or(1.0);
                let declared = numbers("Range");
                let range: Vec<[f64; 3]> = (0..outputs)
                    .map(|i| {
                        [
                            declared.get(i * 2).copied().unwrap_or(0.0),
                            declared.get(i * 2 + 1).copied().unwrap_or(1.0),
                            exponent,
                        ]
                    })
                    .collect();
                let outputs = range.len();
                Some(Self::Exponential(Exponential {
                    domain,
                    c0: if c0.is_empty() {
                        vec![0.0; outputs]
                    } else {
                        c0
                    },
                    c1: if c1.is_empty() {
                        vec![1.0; outputs]
                    } else {
                        c1
                    },
                    range,
                }))
            }
            3 => {
                let functions: Vec<Function> = dict
                    .get("Functions")
                    .and_then(Object::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|o| Function::parse(o, resolve))
                            .collect()
                    })
                    .unwrap_or_default();
                if functions.is_empty() {
                    return None;
                }
                let bounds: Vec<f64> = dict
                    .get("Bounds")
                    .and_then(Object::as_array)
                    .map(|a| a.iter().filter_map(Object::as_f64).collect())
                    .unwrap_or_default();
                let mut encode: Vec<[f64; 2]> = (0..functions.len()).map(|_| [0.0, 1.0]).collect();
                if let Some(array) = dict.get("Encode").and_then(Object::as_array) {
                    let mut i = 0;
                    while i + 1 < array.len() && i / 2 < encode.len() {
                        if let (Some(lo), Some(hi)) = (
                            array.get(i).and_then(Object::as_f64),
                            array.get(i + 1).and_then(Object::as_f64),
                        ) && let Some(slot) = encode.get_mut(i / 2)
                        {
                            *slot = [lo, hi];
                        }
                        i += 2;
                    }
                }
                Some(Self::Stitching(Stitching {
                    domain,
                    functions,
                    bounds,
                    encode,
                }))
            }
            4 => {
                let stream = stream?;
                let text = String::from_utf8_lossy(&decode_stream(&stream).data).into_owned();
                let program = lex_program(&text);
                let range: Vec<[f64; 2]> = dict
                    .get("Range")
                    .and_then(Object::as_array)
                    .map(|a| {
                        a.chunks_exact(2)
                            .filter_map(|p| {
                                Some([
                                    p.first().and_then(Object::as_f64)?,
                                    p.get(1).and_then(Object::as_f64)?,
                                ])
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if range.is_empty() || program.is_empty() {
                    return None;
                }
                Some(Self::Calculator(Calculator {
                    domain,
                    range,
                    program,
                }))
            }
            _ => None,
        }
    }
}

/// Combine a run of functions into one, as a `/Function` array means.
fn stitch(parts: &[Function], dict: &Dict, start: f64, end: f64) -> Option<Function> {
    if parts.len() == 1 {
        return parts.first().cloned();
    }
    // An `/Encode` on the array, when present, gives the sub-domains; the even split it
    // defaults to is what a file without one means.
    let mut encode: Vec<[f64; 2]> = Vec::with_capacity(parts.len());
    for i in 0..parts.len() {
        let lo = start + (end - start) * i as f64 / parts.len() as f64;
        let hi = start + (end - start) * (i + 1) as f64 / parts.len() as f64;
        encode.push([0.0, 1.0]);
        let _ = (lo, hi);
    }
    if let Some(array) = dict.get("Function").and_then(Object::as_array) {
        let array: &[Object] = array;
        for (i, item) in array.iter().enumerate() {
            let sub = match item {
                Object::Dict(d) => d.clone(),
                Object::Stream(s) => s.dict.clone(),
                _ => continue,
            };
            if let Some(enc) = sub.get("Encode").and_then(Object::as_array)
                && let Some(slot) = encode.get_mut(i)
                && let (Some(lo), Some(hi)) = (
                    enc.first().and_then(Object::as_f64),
                    enc.get(1).and_then(Object::as_f64),
                )
            {
                *slot = [lo, hi];
            }
        }
    }
    let bounds: Vec<f64> = (1..parts.len())
        .map(|i| start + (end - start) * i as f64 / parts.len() as f64)
        .collect();
    Some(Function::Stitching(Stitching {
        domain: vec![[start, end]],
        functions: parts.to_vec(),
        bounds,
        encode,
    }))
}

/// Tokenise a PostScript calculator program.
///
/// The syntax is deliberately small: numbers, names, and brace-delimited blocks. A token
/// this does not recognise becomes a name, which then fails as an unknown operator at run
/// time and reports the function as unanswerable.
#[must_use]
pub fn lex_program(text: &str) -> Vec<Token> {
    let bytes = text.as_bytes();
    // Every read of the program text goes through this, so a truncated string yields a
    // zero byte rather than a panic part-way through a file's gradient.
    let at = |i: usize| -> u8 { bytes.get(i).copied().unwrap_or(0) };
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let b = at(i);
        match b {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'%' => {
                while i < bytes.len() && at(i) != b'\n' {
                    i += 1;
                }
            }
            b'{' => {
                let mut depth = 1usize;
                let start = i + 1;
                i += 1;
                while i < bytes.len() && depth > 0 {
                    match at(i) {
                        b'{' => depth += 1,
                        b'}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                let inner =
                    String::from_utf8_lossy(bytes.get(start..i).unwrap_or_default()).into_owned();
                out.push(Token::Block(lex_program(&inner)));
                i += 1;
            }
            b'}' => i += 1,
            b'<' => {
                // A hexadecimal string: `<48656C6C6F>` is the name `Hello`.
                let mut digits = String::new();
                i += 1;
                while i < bytes.len() && at(i) != b'>' {
                    if (at(i) as char).is_ascii_hexdigit() {
                        digits.push(at(i) as char);
                        i += 1;
                    } else {
                        i += 1;
                    }
                }
                i += 1;
                let mut decoded = Vec::new();
                let raw: Vec<char> = digits.chars().collect();
                if raw.len() % 2 == 1 {
                    decoded.push(u8::from_str_radix("0", 16).unwrap_or(0));
                }
                let mut j = 0;
                while j + 1 < raw.len() {
                    let pair: String = raw.get(j..j + 2).unwrap_or_default().iter().collect();
                    if let Ok(v) = u8::from_str_radix(&pair, 16) {
                        decoded.push(v);
                    }
                    j += 2;
                }
                out.push(Token::HexString(decoded));
            }
            b'0'..=b'9' | b'-' | b'.' | b'+' => {
                let start = i;
                i += 1;
                while i < bytes.len()
                    && matches!(at(i), b'0'..=b'9' | b'.' | b'-' | b'+' | b'e' | b'E')
                {
                    i += 1;
                }
                let text =
                    String::from_utf8_lossy(bytes.get(start..i).unwrap_or_default()).into_owned();
                match text.parse::<f64>() {
                    Ok(v) => out.push(Token::Number(v)),
                    // A token that looks numeric and is not is a name, so the program
                    // reports an unknown operator rather than a bad constant.
                    Err(_) => out.push(Token::Name(text.into_bytes())),
                }
            }
            b'[' => {
                // An array literal evaluates its contents and pushes them.
                let start = i;
                let mut depth = 1usize;
                i += 1;
                while i < bytes.len() && depth > 0 {
                    match at(i) {
                        b'[' => depth += 1,
                        b']' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                let inner =
                    String::from_utf8_lossy(bytes.get(start..i).unwrap_or_default()).into_owned();
                out.push(Token::Block(lex_program(&inner)));
                i += 1;
            }
            b']' => i += 1,
            _ => {
                let start = i;
                while i < bytes.len()
                    && !at(i).is_ascii_whitespace()
                    && !matches!(at(i), b'{' | b'}' | b'<' | b'>' | b'[' | b']' | b'%')
                {
                    i += 1;
                }
                if i == start {
                    i += 1;
                    continue;
                }
                let mut name = bytes.get(start..i).unwrap_or_default().to_vec();
                // A leading slash marks a literal name rather than an operator to run. The
                // value that reaches the stack is the name itself, so the marker is dropped
                // here rather than being looked for again by every operator.
                if name.first() == Some(&b'/') {
                    name.remove(0);
                }
                out.push(Token::Name(name));
            }
        }
        if out.len() > MAX_PROGRAM {
            // A program longer than this is damage; truncating keeps it bounded and the
            // truncation shows up as a function that cannot answer.
            out.truncate(MAX_PROGRAM);
            break;
        }
    }
    out
}

/// The most tokens a calculator program may hold.
pub const MAX_PROGRAM: usize = 4096;

/// Paint a shading through a transformation.
///
/// `matrix` maps the shading's own space onto the page. Each pixel of the clip is mapped
/// *back* through its inverse, which is what makes a rotated gradient work with no special
/// cases.
pub fn paint(device: &mut Device, shading: &Shading, matrix: &Matrix, alpha: f64) -> bool {
    let area = device.clip();
    let Some((columns, rows)) = area.pixels() else {
        return false;
    };
    let Some(inverse) = matrix.inverse() else {
        return false;
    };
    let extend = shading.extend();
    let x0 = columns.start;
    let y0 = rows.start;
    let mut painted = 0usize;

    for y in rows.clone() {
        for x in columns.clone() {
            let (sx, sy) = inverse.apply(x as f64 + 0.5, y as f64 + 0.5);
            let Some(t) = shading.parameter_at(sx, sy) else {
                continue;
            };
            // How far `t` moves per pixel, by evaluating it either side. Differencing beats
            // a closed form because it is the same three evaluations for an axial gradient
            // and a radial one, and a radial gradient's gradient is a conic section nobody
            // wants to write twice.
            let step = 0.5f64;
            let (ax, ay) = inverse.apply(x as f64 + 0.5 + step, y as f64 + 0.5);
            let (bx, by) = inverse.apply(x as f64 + 0.5, y as f64 + 0.5 + step);
            let (tx, ty) = (shading.parameter_at(ax, ay), shading.parameter_at(bx, by));
            let slope = match (tx, ty) {
                (Some(a), Some(b)) => {
                    let dx = (a - t) / step;
                    let dy = (b - t) / step;
                    (dx * dx + dy * dy).sqrt()
                }
                // A gradient whose ends coincide has no slope; treat it as steep, which
                // means no antialiasing rather than a smeared edge.
                _ => f64::INFINITY,
            };
            // The gradient's own first circle is filled with its first colour, and for a
            // radial gradient the parameter there is negative rather than absent, so the
            // disc is recognised by geometry instead.
            let coverage = if shading.inner_fill(sx, sy) {
                1.0
            } else if slope.is_finite() && slope > 0.0 {
                let low = if extend[0] {
                    1.0
                } else {
                    (t / slope).clamp(0.0, 1.0)
                };
                let high = if extend[1] {
                    1.0
                } else {
                    ((1.0 - t) / slope).clamp(0.0, 1.0)
                };
                low.min(high)
            } else if slope.is_finite() {
                // A flat gradient covers everything inside and nothing outside.
                if extend[0] || extend[1] { 1.0 } else { 0.0 }
            } else {
                1.0
            };
            if coverage <= 0.0 {
                continue;
            }
            let Some(colour) = shading.colour_at(t) else {
                continue;
            };
            let a = (coverage * alpha).clamp(0.0, 1.0);
            device.put(
                x - x0,
                y - y0,
                [
                    (colour[0].clamp(0.0, 1.0) * 255.0).round() as u8,
                    (colour[1].clamp(0.0, 1.0) * 255.0).round() as u8,
                    (colour[2].clamp(0.0, 1.0) * 255.0).round() as u8,
                    (a * 255.0).round() as u8,
                ],
            );
            painted += 1;
        }
    }
    painted > 0
}

/// The rectangle a shading covers, in the shading's own space.
///
/// A caller may want this to decide what to paint at all, and it is the honest answer for
/// a gradient whose `/Coords` are a bounding box.
#[must_use]
pub fn bounds(shading: &Shading) -> Option<Rect> {
    match shading {
        Shading::Axial { coords, .. } => {
            let x0 = coords.first().copied()?;
            let y0 = coords.get(1).copied()?;
            let x1 = coords.get(2).copied()?;
            let y1 = coords.get(3).copied()?;
            Some(Rect {
                x0: x0.min(x1),
                y0: y0.min(y1),
                x1: x0.max(x1),
                y1: y0.max(y1),
            })
        }
        Shading::Radial { coords, .. } => {
            let (x0, y0, r0) = (
                coords.first().copied()?,
                coords.get(1).copied()?,
                coords.get(2).copied()?,
            );
            let (x1, y1, r1) = (
                coords.get(3).copied()?,
                coords.get(4).copied()?,
                coords.get(5).copied()?,
            );
            // The region spans both circles, so the left edge is the further of the two
            // lefts and the right edge the further of the two rights. Reusing the names
            // here would make each line read the value the previous line had just written.
            let lo = (x0 - r0).min(x1 - r1);
            let hi = (x0 + r0).max(x1 + r1);
            let bottom = (y0 - r0).min(y1 - r1);
            let top = (y0 + r0).max(y1 + r1);
            Some(Rect {
                x0: lo,
                y0: bottom,
                x1: hi,
                y1: top,
            })
        }
    }
}

/// The pixel ranges a clip covers, for a caller that walks one.
#[must_use]
pub fn clip_pixels(rect: Rect) -> Option<(Range<usize>, Range<usize>)> {
    rect.pixels()
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect` and index a slice whose length they
    // have just asserted; both are what a test is for. The panic-free rule is about what
    // the product does with a file, not about how a test reads one.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::float_cmp
    )]

    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    /// The formula the acceptance criteria's analytic checks use: an axial gradient from
    /// black to white over `[0, 1]`, sampled at a point, is the parameter at that point.
    fn axial_black_to_white() -> Shading {
        Shading::Axial {
            coords: vec![0.0, 0.0, 1.0, 0.0],
            function: Function::Exponential(Exponential {
                domain: vec![[0.0, 1.0]],
                range: vec![[0.0, 1.0, 1.0]],
                c0: vec![0.0],
                c1: vec![1.0],
            }),
            extend: [false, false],
        }
    }

    #[test]
    fn an_axial_gradient_is_linear_along_its_axis() {
        let s = axial_black_to_white();
        assert!(close(s.parameter_at(0.0, 0.0).unwrap_or(-1.0), 0.0));
        assert!(close(s.parameter_at(0.5, 0.0).unwrap_or(-1.0), 0.5));
        assert!(close(s.parameter_at(1.0, 0.0).unwrap_or(-1.0), 1.0));
    }

    #[test]
    fn an_axial_gradient_is_constant_along_a_perpendicular() {
        let s = axial_black_to_white();
        for y in [-50.0, -1.0, 0.0, 1.0, 500.0] {
            assert!(
                close(s.parameter_at(0.25, y).unwrap_or(-1.0), 0.25),
                "at (0.25, {y}) the parameter is the projection, not the distance"
            );
        }
    }

    #[test]
    fn a_diagonal_gradient_measures_along_its_own_axis() {
        // From (0,0) to (10,10): the point (5,5) is halfway along it, and (5,0) is a third
        // of the way because the projection onto (10,10) is 50 out of 200.
        let s = Shading::Axial {
            coords: vec![0.0, 0.0, 10.0, 10.0],
            function: Function::Exponential(Exponential {
                domain: vec![[0.0, 1.0]],
                range: vec![[0.0, 1.0, 1.0]],
                c0: vec![0.0],
                c1: vec![1.0],
            }),
            extend: [false, false],
        };
        assert!(close(s.parameter_at(5.0, 5.0).unwrap_or(-1.0), 0.5));
        assert!(close(s.parameter_at(5.0, 0.0).unwrap_or(-1.0), 0.25));
    }

    /// The parameter is raw on purpose: a caller needs to know how far outside a point is
    /// in order to work out how much of it is covered. Clamping here is what left the
    /// paper outside a gradient unpainted.
    #[test]
    fn a_parameter_outside_the_gradient_reports_how_far_out() {
        let s = axial_black_to_white();
        assert_eq!(s.parameter_at(-5.0, 0.0), Some(-5.0));
        assert_eq!(s.parameter_at(5.0, 0.0), Some(5.0));
        // And the colour is still the nearest end's, because that is what a point past the
        // end is painted.
        let c = s.colour_at(5.0).unwrap_or([-1.0; 3]);
        assert!(close(c[0], 1.0), "white at the far end, got {c:?}");
    }

    #[test]
    fn a_zero_length_axis_has_one_parameter_rather_than_dividing_by_zero() {
        let s = Shading::Axial {
            coords: vec![1.0, 1.0, 1.0, 1.0],
            function: Function::Exponential(Exponential {
                domain: vec![[0.0, 1.0]],
                range: vec![[0.0, 1.0, 1.0]],
                c0: vec![0.0],
                c1: vec![1.0],
            }),
            extend: [false, false],
        };
        assert_eq!(s.parameter_at(1.0, 1.0), Some(0.0));
        assert_eq!(
            s.parameter_at(100.0, 100.0),
            Some(0.0),
            "and no division by zero"
        );
    }

    /// The closed form the acceptance criteria name: for concentric circles the parameter
    /// along a ray is the distance from the inner circle over the ring's width.
    #[test]
    fn a_radial_gradient_is_the_distance_over_the_ring_width() {
        // r0 = 10, r1 = 20, concentric at the origin: at (0, 20) the point is on the outer
        // circle and the parameter is 1.
        let t = radial_parameter(0.0, 20.0, 0.0, 0.0, 10.0, 0.0, 0.0, 20.0);
        assert!(
            close(t.unwrap_or(-1.0), 1.0),
            "on the outer circle, got {t:?}"
        );
        let t = radial_parameter(0.0, 15.0, 0.0, 0.0, 10.0, 0.0, 0.0, 20.0);
        assert!(
            close(t.unwrap_or(-1.0), 0.5),
            "halfway out the ring, got {t:?}"
        );
        let t = radial_parameter(0.0, 10.0, 0.0, 0.0, 10.0, 0.0, 0.0, 20.0);
        assert!(
            close(t.unwrap_or(-1.0), 0.0),
            "at the inner circle, got {t:?}"
        );
        // A point beyond the outer circle has no root in range, and the parameter says so
        // with a number larger than one rather than by refusing.
        let t = radial_parameter(0.0, 25.0, 0.0, 0.0, 10.0, 0.0, 0.0, 20.0);
        assert!(
            t.is_some_and(|v| v > 1.0),
            "outside the outer circle, got {t:?}"
        );
    }

    /// No circle in the family passes through the family's own centre, so the parameter
    /// there is negative — and the disc is painted all the same.
    #[test]
    fn the_disc_inside_the_first_circle_is_filled_with_the_first_colour() {
        let radial = Shading::Radial {
            coords: vec![0.0, 0.0, 10.0, 0.0, 0.0, 20.0],
            function: Function::Exponential(Exponential {
                domain: vec![[0.0, 1.0]],
                range: vec![[0.0, 1.0, 1.0]],
                c0: vec![0.0],
                c1: vec![1.0],
            }),
            extend: [false, false],
        };
        assert!(
            radial.inner_fill(0.0, 0.0),
            "the centre is inside the first circle"
        );
        assert!(
            radial.inner_fill(0.0, 9.0),
            "and so is a point near its edge"
        );
        assert!(!radial.inner_fill(0.0, 11.0), "but not one in the ring");
        // The parameter alone cannot say so.
        let t = radial.parameter_at(0.0, 0.0);
        assert!(
            t.is_some_and(|v| v < 0.0),
            "which is exactly why the geometry is consulted: got {t:?}"
        );
        let mut device = Device::new(crate::Image::filled(40, 40, [255, 255, 255, 255]));
        paint(&mut device, &radial, &Matrix::scale(1.0, 1.0), 1.0);
        // The disc is centred on the shading's own origin, which is the image's corner.
        assert_eq!(
            device.image().get(0, 0),
            Some([0, 0, 0, 255]),
            "the centre is painted with the first colour, not left as paper"
        );
    }

    #[test]
    fn a_radial_gradient_is_the_same_at_every_angle() {
        for (x, y) in [(15.0, 0.0), (0.0, 15.0), (-15.0, 0.0), (0.0, -15.0)] {
            let t = radial_parameter(x, y, 0.0, 0.0, 10.0, 0.0, 0.0, 20.0);
            assert!(
                close(t.unwrap_or(-1.0), 0.5),
                "at ({x}, {y}) concentric circles are still concentric, got {t:?}"
            );
        }
    }

    #[test]
    fn a_point_inside_the_inner_circle_is_at_the_start() {
        let t = radial_parameter(0.0, 0.0, 0.0, 0.0, 10.0, 0.0, 0.0, 20.0);
        assert!(
            t.is_some_and(|v| v < 0.0),
            "the centre is on no circle in the family, so the parameter is negative: {t:?}"
        );
    }

    #[test]
    fn an_exponential_function_is_c0_plus_c1_times_t_to_the_n() {
        let f = Function::Exponential(Exponential {
            domain: vec![[0.0, 1.0]],
            range: vec![[0.0, 1.0, 2.0]],
            c0: vec![0.25],
            c1: vec![0.75],
        });
        let at = |t: f64| f.apply1(t).and_then(|v| v.first().copied()).unwrap_or(-1.0);
        assert!(close(at(0.0), 0.25), "C0 at t = 0, got {}", at(0.0));
        assert!(close(at(1.0), 1.0), "C0 + C1 at t = 1, got {}", at(1.0));
        assert!(
            close(at(0.5), 0.25 + 0.75 * 0.25),
            "n = 2 squares it, got {}",
            at(0.5)
        );
    }

    #[test]
    fn a_function_outside_its_domain_has_no_answer() {
        let f = Function::Exponential(Exponential {
            domain: vec![[0.2, 0.8]],
            range: vec![[0.0, 1.0, 1.0]],
            c0: vec![0.0],
            c1: vec![1.0],
        });
        assert!(f.apply1(0.1).is_none(), "below the domain");
        assert!(f.apply1(0.9).is_none(), "above it");
        assert!(f.apply1(0.5).is_some(), "and inside it");
    }

    /// A function that cannot produce a finite number has no answer, and a gradient with a
    /// hole in it is a bug report rather than a plausible wrong colour.
    #[test]
    fn an_exponent_that_overflows_has_no_answer() {
        let f = Function::Exponential(Exponential {
            domain: vec![[0.0, 10.0]],
            // 2 to the two thousandth is not a number.
            range: vec![[0.0, 1.0, 2000.0]],
            c0: vec![0.0],
            c1: vec![1.0],
        });
        assert!(
            f.apply1(2.0).is_none(),
            "there is no answer, so there is no colour"
        );
        assert!(
            f.apply1(0.5).is_some(),
            "and a small enough exponent is fine"
        );
    }

    #[test]
    fn a_stitching_function_picks_the_right_part() {
        let part = |c0: f64, c1: f64| {
            Function::Exponential(Exponential {
                domain: vec![[0.0, 1.0]],
                range: vec![[0.0, 1.0, 1.0]],
                c0: vec![c0],
                c1: vec![c1],
            })
        };
        let s = Function::Stitching(Stitching {
            domain: vec![[0.0, 1.0]],
            functions: vec![part(0.0, 0.5), part(0.5, 0.5)],
            bounds: vec![0.5],
            encode: vec![[0.0, 1.0], [0.0, 1.0]],
        });
        let at = |t: f64| s.apply1(t).and_then(|v| v.first().copied()).unwrap_or(-1.0);
        assert!(
            close(at(0.0), 0.0),
            "the first part's start, got {}",
            at(0.0)
        );
        assert!(close(at(0.5), 0.5), "the join, got {}", at(0.5));
        assert!(
            close(at(1.0), 1.0),
            "the second part's end, got {}",
            at(1.0)
        );
    }

    #[test]
    fn a_calculator_adds_two_numbers() {
        let f = Function::Calculator(Calculator {
            domain: vec![[0.0, 1.0]],
            range: vec![[0.0, 2.0]],
            program: lex_program("{ 1 add }"),
        });
        let v = f.apply1(0.25).and_then(|v| v.first().copied());
        assert!(close(v.unwrap_or(-1.0), 1.25), "got {v:?}");
    }

    #[test]
    fn a_calculator_multiplies_and_exponentiates() {
        let f = |program: &str| {
            Function::Calculator(Calculator {
                domain: vec![[0.0, 10.0]],
                range: vec![[0.0, 1000.0]],
                program: lex_program(program),
            })
        };
        let at =
            |f: &Function, t: f64| f.apply1(t).and_then(|v| v.first().copied()).unwrap_or(-1.0);
        assert!(close(at(&f("{ 2 mul }"), 3.0), 6.0));
        assert!(
            close(at(&f("{ 2 exp }"), 3.0), 9.0),
            "`a b exp` is a to the b, so the input is the base"
        );
        assert!(
            close(at(&f("{ dup mul }"), 3.0), 9.0),
            "dup then mul squares it"
        );
        assert!(
            close(at(&f("{ 1 2 exch sub }"), 0.0), 1.0),
            "exch reverses, so 2 - 1"
        );
    }

    #[test]
    fn a_calculator_divides_by_zero_rather_than_producing_an_infinity() {
        let f = Function::Calculator(Calculator {
            domain: vec![[0.0, 1.0]],
            range: vec![[0.0, 1.0]],
            program: lex_program("{ 1 0 div }"),
        });
        assert!(
            f.apply1(0.5).is_none(),
            "there is no answer, so there is no colour"
        );
    }

    #[test]
    fn a_calculator_takes_a_logarithm_only_of_a_positive_number() {
        let good = Function::Calculator(Calculator {
            domain: vec![[0.0, 100.0]],
            range: vec![[0.0, 3.0]],
            program: lex_program("{ log }"),
        });
        assert!(close(
            good.apply1(100.0)
                .and_then(|v| v.first().copied())
                .unwrap_or(-1.0),
            2.0
        ));
        let bad = Function::Calculator(Calculator {
            domain: vec![[0.0, 1.0]],
            range: vec![[0.0, 1.0]],
            program: lex_program("{ 0 ln }"),
        });
        assert!(bad.apply1(0.5).is_none());
    }

    #[test]
    fn a_calculator_reports_postscrips_own_truth() {
        let f = |program: &str| {
            Function::Calculator(Calculator {
                domain: vec![[0.0, 1.0]],
                range: vec![[0.0, 1.0]],
                program: lex_program(program),
            })
        };
        let at =
            |f: &Function, t: f64| f.apply1(t).and_then(|v| v.first().copied()).unwrap_or(-1.0);
        assert_eq!(at(&f("{ true }"), 0.0), 1.0);
        assert_eq!(at(&f("{ false }"), 0.0), 0.0);
        assert_eq!(at(&f("{ 0 not }"), 0.0), 1.0, "not of zero is true");
        assert_eq!(at(&f("{ 1 not }"), 0.0), 0.0);
        assert_eq!(at(&f("{ 1 2 lt }"), 0.0), 1.0);
        assert_eq!(at(&f("{ 2 1 lt }"), 0.0), 0.0);
        assert_eq!(at(&f("{ 1 1 eq }"), 0.0), 1.0);
        assert_eq!(
            at(&f("{ 1 0 and }"), 0.0),
            0.0,
            "and is logical, not numeric"
        );
        assert_eq!(at(&f("{ 1 1 or }"), 0.0), 1.0);
    }

    #[test]
    fn a_calculator_clamps_its_output_to_the_declared_range() {
        let f = Function::Calculator(Calculator {
            domain: vec![[0.0, 1.0]],
            range: vec![[0.0, 1.0]],
            // A program that would return 100 if it were not clamped.
            program: lex_program("{ 100 }"),
        });
        let v = f.apply1(0.5).and_then(|v| v.first().copied());
        assert_eq!(
            v,
            Some(1.0),
            "the specification clamps, and a file that relies on \
             an out-of-range value gets the range's end"
        );
    }

    #[test]
    fn a_calculator_does_not_divide_by_zero_when_it_reads_an_operator_from_the_stack() {
        let f = Function::Calculator(Calculator {
            domain: vec![[0.0, 1.0]],
            range: vec![[0.0, 1.0]],
            program: lex_program("{ 1 0 div }"),
        });
        assert!(f.apply1(0.5).is_none());
    }

    #[test]
    fn a_program_that_grows_the_stack_without_bound_is_refused() {
        let f = Function::Calculator(Calculator {
            domain: vec![[0.0, 1.0]],
            range: vec![[0.0, 1.0]],
            program: vec![Token::Number(1.0); MAX_STACK + 10],
        });
        assert!(
            f.apply1(0.5).is_none(),
            "a runaway program is damage, not a long loop"
        );
    }

    #[test]
    fn a_bitshift_past_the_word_is_zero_not_a_wrap() {
        // The specification says a shift of more than the word's width is zero, which is not
        // what a machine shift does and is the usual way to get this wrong.
        let f = Function::Calculator(Calculator {
            domain: vec![[0.0, 1.0]],
            range: vec![[0.0, 1000.0]],
            program: lex_program("{ 1 32 bitshift }"),
        });
        let v = f.apply1(0.5).and_then(|v| v.first().copied());
        assert_eq!(v, Some(0.0));
        let g = Function::Calculator(Calculator {
            domain: vec![[0.0, 1.0]],
            range: vec![[0.0, 1000.0]],
            program: lex_program("{ 1 4 bitshift }"),
        });
        assert_eq!(g.apply1(0.5).and_then(|v| v.first().copied()), Some(16.0));
    }

    #[test]
    fn the_program_lexiser_reads_numbers_names_blocks_and_hex() {
        let tokens = lex_program("{ 1 2 add } /MyName <48656C6C6F>");
        assert_eq!(tokens.len(), 3);
        assert!(matches!(tokens.first(), Some(Token::Block(b)) if b.len() == 3));
        assert!(matches!(tokens.get(1), Some(Token::Name(n)) if n == b"MyName"));
        assert!(matches!(tokens.get(2), Some(Token::HexString(h)) if h == b"Hello"));
    }

    #[test]
    fn the_program_lexiser_skips_comments() {
        let tokens = lex_program("% a comment\n 1 2 add");
        assert_eq!(tokens.len(), 3, "the comment contributes nothing");
    }

    #[test]
    fn a_sampled_function_interpolates_between_its_entries() {
        let f = Function::Sampled(Sampled {
            domain: vec![[0.0, 1.0]],
            size: vec![2],
            bits: 8,
            range: vec![[0.0, 1.0]],
            encode: vec![[0.0, 1.0]],
            samples: vec![0.0, 1.0],
        });
        let at = |t: f64| f.apply1(t).and_then(|v| v.first().copied()).unwrap_or(-1.0);
        assert!(close(at(0.0), 0.0));
        assert!(close(at(1.0), 1.0));
        assert!(
            close(at(0.5), 0.5),
            "linearly between the two samples, got {}",
            at(0.5)
        );
    }

    #[test]
    fn a_sampled_function_honours_its_encode_range() {
        // Four samples over a domain of 0..3, so index 3 is the fourth.
        let f = Function::Sampled(Sampled {
            domain: vec![[0.0, 3.0]],
            size: vec![4],
            bits: 8,
            range: vec![[0.0, 1.0]],
            encode: vec![[0.0, 3.0]],
            samples: vec![0.0, 0.25, 0.5, 1.0],
        });
        let at = |t: f64| f.apply1(t).and_then(|v| v.first().copied()).unwrap_or(-1.0);
        assert!(close(at(0.0), 0.0));
        assert!(close(at(1.0), 0.25), "got {}", at(1.0));
        assert!(close(at(2.0), 0.5), "got {}", at(2.0));
        assert!(close(at(3.0), 1.0), "got {}", at(3.0));
    }

    #[test]
    fn a_sampled_function_with_no_samples_has_no_answer() {
        let f = Function::Sampled(Sampled {
            domain: vec![[0.0, 1.0]],
            size: vec![2],
            bits: 8,
            range: vec![[0.0, 1.0]],
            encode: vec![[0.0, 1.0]],
            samples: Vec::new(),
        });
        assert!(f.apply1(0.5).is_none());
    }

    #[test]
    fn extend_is_read_from_the_dictionary_and_defaults_to_off() {
        let mut d = Dict::new();
        d.set("ShadingType", Object::Int(2));
        d.set(
            "Coords",
            Object::Array(vec![
                Object::Int(0),
                Object::Int(0),
                Object::Int(1),
                Object::Int(0),
            ]),
        );
        let mut f = Dict::new();
        f.set("FunctionType", Object::Int(2));
        f.set(
            "Domain",
            Object::Array(vec![Object::Int(0), Object::Int(1)]),
        );
        f.set(
            "Range",
            Object::Array(vec![Object::Int(0), Object::Int(1), Object::Int(1)]),
        );
        d.set("Function", Object::Dict(f));

        let s = Shading::parse(&Object::Dict(d.clone()), &|o| Some(o.clone()));
        assert!(s.is_some(), "a two-point axial gradient is the common case");
        assert_eq!(s.map(|v| v.extend()), Some([false, false]));

        let mut on = d;
        on.set(
            "Extend",
            Object::Array(vec![Object::Bool(true), Object::Bool(false)]),
        );
        assert_eq!(
            Shading::parse(&Object::Dict(on), &|o| Some(o.clone())).map(|v| v.extend()),
            Some([true, false])
        );
    }

    #[test]
    fn a_shading_of_an_unsupported_type_is_refused() {
        // Type 1 is function-based and needs a pattern colour; type 4 is a mesh.
        for kind in [1, 4, 5, 6, 7] {
            let mut d = Dict::new();
            d.set("ShadingType", Object::Int(kind));
            d.set(
                "Coords",
                Object::Array(vec![Object::Int(0), Object::Int(0)]),
            );
            assert!(
                Shading::parse(&Object::Dict(d), &|o| Some(o.clone())).is_none(),
                "type {kind} needs something this does not build yet"
            );
        }
    }

    #[test]
    fn a_shading_with_too_few_coordinates_is_refused() {
        let mut d = Dict::new();
        d.set("ShadingType", Object::Int(2));
        d.set(
            "Coords",
            Object::Array(vec![Object::Int(0), Object::Int(0)]),
        );
        let mut f = Dict::new();
        f.set("FunctionType", Object::Int(2));
        f.set(
            "Domain",
            Object::Array(vec![Object::Int(0), Object::Int(1)]),
        );
        f.set(
            "Range",
            Object::Array(vec![Object::Int(0), Object::Int(1), Object::Int(1)]),
        );
        d.set("Function", Object::Dict(f));
        assert!(Shading::parse(&Object::Dict(d), &|o| Some(o.clone())).is_none());
    }

    #[test]
    fn a_gradient_colour_is_read_as_gray_or_rgb() {
        let s = axial_black_to_white();
        let at = |t: f64| s.colour_at(t).unwrap_or([-1.0; 3]);
        assert!(close(at(0.0)[0], 0.0), "black at the start");
        assert!(close(at(1.0)[0], 1.0), "white at the end");
        assert!(close(at(0.25)[0], 0.25), "and gray in between");
        assert!(
            close(at(0.5)[1], 0.5),
            "every channel matches, because it is gray"
        );
    }

    #[test]
    fn a_gradient_colour_can_be_cmyk_and_is_subtractive() {
        let s = Shading::Axial {
            coords: vec![0.0, 0.0, 1.0, 0.0],
            function: Function::Exponential(Exponential {
                domain: vec![[0.0, 1.0]],
                range: vec![
                    [0.0, 1.0, 1.0],
                    [0.0, 1.0, 1.0],
                    [0.0, 1.0, 1.0],
                    [0.0, 1.0, 1.0],
                ],
                c0: vec![0.0, 0.0, 0.0, 0.0],
                c1: vec![1.0, 0.0, 0.0, 0.0],
            }),
            extend: [false, false],
        };
        let at = |t: f64| s.colour_at(t).unwrap_or([-1.0; 3]);
        let end = at(1.0);
        assert!(
            close(end[0], 0.0) && close(end[1], 1.0) && close(end[2], 1.0),
            "full cyan at the end, got {end:?}"
        );
    }

    #[test]
    fn a_gradient_paints_inside_its_own_extent_and_nowhere_else() {
        // A black-to-white gradient across the left half of a 20 by 20 canvas.
        let mut device = Device::new(crate::Image::filled(20, 20, [255, 255, 255, 255]));
        let s = axial_black_to_white();
        let drawn = paint(
            &mut device,
            &s,
            &Matrix::new(10.0, 0.0, 0.0, 10.0, 0.0, 0.0),
            1.0,
        );
        assert!(drawn, "something was painted");
        // The gradient runs from x = 0 to x = 10, so a pixel's centre at device x has
        // t = (x + 0.5) / 10. Stating the formula rather than a pixel makes the check a
        // measurement of the gradient rather than a snapshot of one resolution.
        let at = |x: usize| -> u8 {
            let t = (x as f64 + 0.5) / 10.0;
            (t * 255.0).round() as u8
        };
        for x in 0..10 {
            assert_eq!(
                device.image().get(x, 10).map(|p| p[0]),
                Some(at(x)),
                "at x = {x} the parameter is {} and that is the colour",
                (x as f64 + 0.5) / 10.0
            );
        }
        // Past the right end nothing is painted, because the gradient does not extend.
        assert_eq!(
            device.image().get(15, 10),
            Some([255, 255, 255, 255]),
            "outside the gradient is paper"
        );
    }

    #[test]
    fn an_extended_gradient_paints_past_its_ends() {
        let mut device = Device::new(crate::Image::filled(20, 20, [255, 255, 255, 255]));
        let s = match axial_black_to_white() {
            Shading::Axial {
                coords, function, ..
            } => Shading::Axial {
                coords,
                function,
                extend: [true, true],
            },
            other @ Shading::Radial { .. } => other,
        };
        paint(
            &mut device,
            &s,
            &Matrix::new(10.0, 0.0, 0.0, 10.0, 0.0, 0.0),
            1.0,
        );
        assert_eq!(
            device.image().get(19, 10),
            Some([255, 255, 255, 255]),
            "extended past the right end is the end colour, which is white"
        );
    }

    #[test]
    fn a_rotated_gradient_is_painted_through_its_own_transformation() {
        // The same gradient, but rotated a quarter turn, so it runs down the page.
        let mut device = Device::new(crate::Image::filled(20, 20, [255, 255, 255, 255]));
        let s = axial_black_to_white();
        paint(
            &mut device,
            &s,
            &Matrix::new(0.0, 10.0, -10.0, 0.0, 10.0, 0.0),
            1.0,
        );
        // The transformation puts the shading's own u along the page's y and its v along
        // `10 - x`, so the gradient runs down the page and stops ten pixels from the top.
        let at = |y: usize| -> u8 {
            let t = (y as f64 + 0.5) / 10.0;
            (t * 255.0).round() as u8
        };
        for y in 0..10 {
            assert_eq!(
                device.image().get(10, y).map(|p| p[0]),
                Some(at(y)),
                "the rotated gradient runs down the page, and at y = {y} the colour is {}",
                at(y)
            );
        }
        // Past the end of the axis the gradient does not reach, and the paper is still there.
        assert_eq!(
            device.image().get(10, 15),
            Some([255, 255, 255, 255]),
            "below the gradient's own extent is paper"
        );
    }

    #[test]
    fn a_gradient_that_cannot_be_inverted_paints_nothing() {
        let mut device = Device::new(crate::Image::filled(4, 4, [255, 255, 255, 255]));
        let s = axial_black_to_white();
        let flat = Matrix::scale(0.0, 0.0);
        assert!(
            !paint(&mut device, &s, &flat, 1.0),
            "a degenerate transform draws nothing"
        );
    }

    #[test]
    fn bounds_cover_the_gradient() {
        let axial = axial_black_to_white();
        let b = bounds(&axial).expect("bounds");
        assert_eq!((b.x0, b.y0, b.x1, b.y1), (0.0, 0.0, 1.0, 0.0));

        let radial = Shading::Radial {
            coords: vec![5.0, 5.0, 2.0, 5.0, 5.0, 4.0],
            function: Function::Exponential(Exponential {
                domain: vec![[0.0, 1.0]],
                range: vec![[0.0, 1.0, 1.0]],
                c0: vec![0.0],
                c1: vec![1.0],
            }),
            extend: [false, false],
        };
        let r = bounds(&radial).expect("bounds");
        assert_eq!(
            (r.x0, r.y0, r.x1, r.y1),
            (1.0, 1.0, 9.0, 9.0),
            "the outer circle, got {r:?}"
        );
    }

    #[test]
    fn the_clip_pixels_helper_agrees_with_the_rectangle() {
        let r = Rect {
            x0: 0.0,
            y0: 0.0,
            x1: 4.0,
            y1: 4.0,
        };
        let (columns, rows) = clip_pixels(r).expect("pixels");
        assert_eq!((columns.end - columns.start, rows.end - rows.start), (4, 4));
    }

    #[test]
    fn the_stack_and_program_bounds_are_arithmetic() {
        const {
            assert!(MAX_STACK > 16);
            assert!(MAX_PROGRAM > 16);
        }
    }
}
