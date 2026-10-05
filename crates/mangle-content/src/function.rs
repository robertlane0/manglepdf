//! Functions: the little programs a PDF file uses to turn numbers into numbers.
//!
//! There are four kinds — a table of samples, an exponential ramp, a run of ramps stitched
//! end to end, and a PostScript calculator — and every one of them can fail. A function that
//! fails is **reported**, never replaced with a value that would look plausible: a gradient
//! that is slightly wrong is harder to notice than one that is missing, and a separation
//! painted in a tint the file did not ask for is a colour nobody can see the error in.
//!
//! # Why this is in `mangle-content` and not in the rasteriser
//!
//! A shading is a rasteriser's business: it is something to walk along an axis and fill. A
//! function is not — a function dictionary is an ordinary document object, and two things
//! that are not shadings need to read one. A `/Separation`'s `/TintTransform` is the
//! important one: it is the whole of what the space says a tint *means*, so converting a
//! separation is evaluating a function, and the conversion lives in
//! [`crate::state::Colour::to_rgba`] where the graphics state is. Keeping the evaluator
//! here and reaching upwards to it would invert the dependency direction; keeping a second
//! copy here would be the same arithmetic written twice, which is how two answers to one
//! question start to disagree. `mangle-render` re-exports this module as `shading`, because
//! a gradient's function is still how a gradient is defined.

// A calculator's `eq`, `ne` and `bitshift` compare and convert numbers exactly, because
// PostScript's own semantics are exact: `eq` is the question "are these two numbers the
// same number", not "are they close", and a tolerance would answer a different question
// from the one the file's program asked. Everything else in this crate still compares
// exactly rather than approximately, which is why the exemption is scoped to this module
// and not to the crate.
#![allow(clippy::float_cmp)]

use mangle_syntax::object::{Dict, Object, Stream};
use mangle_syntax::stream::decode_stream;

/// A PDF function: numbers in, numbers out.
#[derive(Debug, Clone, PartialEq)]
pub enum Function {
    /// Type 0: a table of samples, interpolated between entries.
    Sampled(Sampled),
    /// Type 2: `C0 + t^n · (C1 − C0)` over each output's own domain.
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
        // `domain` is already one `[min, max]` pair per input, so its length *is* the input
        // count. Halving it here reported every function as taking no inputs at all, which
        // a gradient never noticed — each kind reads `domain.first()` directly — and a
        // separation's tint transform did notice, because deciding how many tints to hand a
        // transform is exactly the question this answers.
        match self {
            Self::Sampled(s) => s.domain.len(),
            Self::Exponential(e) => e.domain.len(),
            Self::Stitching(s) => s.domain.len(),
            Self::Calculator(c) => c.domain.len(),
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

/// A type 2 function: `C0 + (x - x0)^n · (C1 - C0)`, which is `C0` at the bottom of the
/// domain and `C1` at the top of it.
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
            // ISO 32000-1 Table 42: `C0 + x^N · (C1 − C0)`. The two ends are the values at
            // `x = 0` and `x = 1`, so the exponent bends the ramp *between* them. Adding `C1`
            // instead of the difference gives the same answer only when `C0` is zero, which is
            // most hand-written gradients and none of Adobe's.
            out.push(c0 + power * (c1 - c0));
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
