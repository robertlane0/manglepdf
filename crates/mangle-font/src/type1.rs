//! Type 1 font programs: a PostScript program, encrypted twice, with Type 1 charstrings
//! inside the second layer.
//!
//! A Type 1 font is not a table format. It is a PostScript program that a PostScript
//! interpreter would run, and which happens to be stored in two halves: a **cleartext**
//! header naming the font's matrix and built-in encoding, and then an **encrypted** portion
//! holding the private DICT and the glyph programs. Reading a glyph therefore means three
//! things in order: find the `eexec` keyword, undo the outer cipher, and then undo a second,
//! inner cipher over the charstrings. **Swapping the two layers yields plausible garbage** —
//! a stream that decodes without complaint and a font whose glyphs are in the wrong places —
//! so the order is part of the format, not an implementation detail.
//!
//! ## The two ciphers
//!
//! Both are the same cipher, run under different seeds. For each ciphertext byte `c`:
//!
//! ```text
//! r = seed
//! p     = c XOR (r >> 8)          # the plaintext byte
//! r     = ((c + r) * 52845 + 22719) mod 65536
//! ```
//!
//! The update uses the **ciphertext** byte `c`, not the plaintext. That is the whole of the
//! `eexec` cipher, and getting it wrong is not detectable by inspection: the output is still
//! plausible bytes. The seeds are what make the two layers different.
//!
//! * The **cleartext header** is read as it stands; nothing is encrypted.
//! * The **`eexec` layer** is seeded with 55665, and its first four plaintext bytes are a
//!   random salt that must be discarded.
//! * The **charstrings and subroutines** are seeded with 4330, and the Private DICT's
//!   `/lenIV` advances that seed `lenIV - 4` further times — so the common `lenIV` of 4 (and
//!   a font that does not mention `lenIV` at all) leaves the seed at 4330.
//!
//! ## Encodings, and what a Type 1 glyph index is
//!
//! A Type 1 font has no `cmap`. A glyph is **named**, and its *number* is its position in the
//! `CharStrings` dictionary; a character code reaches a glyph by being turned into a name
//! through the font's **own** `/Encoding`, which sits in the cleartext header. That is the
//! lookup [`Type1::glyph_for_code`] performs, and it is the reason a Type 1 program can
//! resolve character codes at all where a bare CFF cannot.
//!
//! Only the font's own encoding is consulted. A page's `/Encoding` with `/Differences` names
//! glyphs the same way, but reading it is the content layer's business and nothing in this
//! renderer does it for any font type, so doing it for one type alone would make this the
//! only place a `/Differences` array is honoured.
//!
//! ## What is left out, on purpose
//!
//! * **The PostScript interpreter.** A Type 1 font may define `OtherSubrs` procedures in
//!   PostScript and call them with `callothersubr`; running PostScript is not this. The
//!   conventional meanings of the numbers `OtherSubrs` is addressed by *are* implemented —
//!   the flex is numbers 0, 1 and 2, hint replacement is 3 — because those are what every
//!   real font uses them for. A number with no conventional meaning is **refused with a
//!   reason** rather than ignored, because a subroutine whose result the charstring then
//!   consumes produces a shape that closes and does not match.
//! * **Hint replacement.** `callothersubr` 3 asks for the stem hints to be recomputed for a
//!   rasterizer's pixel grid. This renderer computes exact analytic coverage and has no grid
//!   to snap to, so the hint *replacement* is refused — and only that: the glyph's geometry
//!   is unaffected, which is why this costs nothing but a log line.
//! * **`seac`.** As in CFF: it builds an accented character out of two others *by name*.
//!   Refused, because drawing the unaccented one is a wrong answer rather than a missing one.
//! * **A PFB container.** A `.pfb` file wraps the same program in six-byte segment headers.
//!   The stripping is thirty lines and is not here; a PFB is reported as unsupported rather
//!   than mis-parsed, because mis-parsing it produces a font that is subtly and
//!   irrecoverably wrong rather than one that is visibly absent.
//! * **Multiple Master fonts.** `/WeightVector` and `blend`-shaped tricks live in
//!   `callothersubr` 14–28 and are refused with the number named.

use crate::outline::{Outline, Segment};

/// The most values a Type 1 charstring may hold on its operand stack.
///
/// The specification's limit for this dialect, half the Type 2 one, and it is the format's
/// limit rather than a convenient round number: a charstring that grows past it is damage,
/// and raising the bound to accommodate one would make the limit worth nothing.
pub const MAX_STACK: usize = 24;

/// The most subroutine calls a charstring may nest inside one another.
pub const MAX_DEPTH: usize = 10;

/// The `eexec` cipher's seed, which is what separates the outer layer from the inner one.
const EEXEC_SEED: u32 = 55665;

/// The charstring cipher's seed, before `/lenIV` is applied to it.
const CHARSTRING_SEED: u32 = 4330;

/// The multiplier both ciphers share.
const CIPHER_MULTIPLIER: u32 = 52845;

/// The increment both ciphers share.
const CIPHER_INCREMENT: u32 = 22719;

/// The `/lenIV` a font that does not mention one is taken to have.
const DEFAULT_LEN_IV: i32 = 4;

/// The most `/lenIV` rounds that will be applied to the charstring seed.
///
/// A font asking for more is not one whose seed can be worked out; every real font asks for
/// four, and the advance is only ever applied for a larger value.
const MAX_LEN_IV: i32 = 64;

/// The `FontMatrix` a Type 1 font has when it does not say: a thousandth of an em.
const DEFAULT_FONT_MATRIX: [f32; 6] = [0.001, 0.0, 0.0, 0.001, 0.0, 0.0];

/// The most glyphs a font may declare.
///
/// A Type 1 font has no table saying how many it has, so the count is whatever the
/// `CharStrings` dictionary turns out to hold, and a file that claims a million is damage
/// rather than a font.
const MAX_GLYPHS: usize = 65_536;

/// The most subroutines a font may declare, for the same reason.
const MAX_SUBRS: usize = 65_536;

/// The most bytes one `RD` body may claim.
const MAX_BODY: usize = 1 << 20;

// ── the cipher ─────────────────────────────────────────────────────────────────────

/// Undo the cipher, starting from `seed`.
///
/// Note which byte goes into the recurrence: the **ciphertext** one. Feeding the plaintext
/// back in instead produces a stream that is still mostly printable, which is why this is
/// worth saying twice.
fn decipher(data: &[u8], seed: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut r = seed;
    for &cipher in data {
        out.push((u32::from(cipher) ^ (r >> 8)) as u8);
        r = ((u32::from(cipher) + r) * CIPHER_MULTIPLIER + CIPHER_INCREMENT) & 0xFFFF;
    }
    out
}

/// The seed the charstrings and subroutines are decrypted under.
///
/// 4330, advanced `lenIV - 4` times by the cipher's own recurrence. The subtraction is the
/// part that matters: the seed 4330 *is* the `lenIV` 4 seed, which is why a font that writes
/// `lenIV 4` — or, like most of them, writes nothing at all — needs no advance. A font
/// asking for fewer than four rounds gets 4330, since there is no such seed to find.
fn charstring_seed(len_iv: i32) -> Result<u32, String> {
    if !(0..=MAX_LEN_IV).contains(&len_iv) {
        return Err(format!(
            "its Private DICT claims a `lenIV` of {len_iv}, and the charstrings can only be \
             decrypted for one between 0 and {MAX_LEN_IV}"
        ));
    }
    let mut r = CHARSTRING_SEED;
    for _ in 0..len_iv.saturating_sub(DEFAULT_LEN_IV) {
        r = (r * CIPHER_MULTIPLIER + CIPHER_INCREMENT) & 0xFFFF;
    }
    Ok(r)
}

// ── the decrypted portion ──────────────────────────────────────────────────────────

/// One token of the PostScript inside the encrypted portion.
///
/// The tokenizer covers only what a font program actually uses. A `(` string and a `{`
/// procedure are recognised so that their contents are stepped over rather than read as
/// tokens, which is the only thing that matters about them: the `OtherSubrs` array is full
/// of PostScript this does not run, and reading its words as font structure would find
/// `CharStrings` and `RD` inside a comment.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Token<'a> {
    /// `/name`, without the slash.
    Name(&'a [u8]),
    Number(f32),
    /// Anything else: an operator, a delimiter, or a `{`-procedure's body.
    Word(&'a [u8]),
    /// `(text)`, skipped whole.
    Text,
}

/// A cursor over the decrypted bytes.
struct Scanner<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Scanner<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }

    /// Step over whitespace, and over a `%` comment to the end of its line.
    ///
    /// A comment is not optional to skip: a Type 1 font's `OtherSubrs` array opens with a
    /// copyright comment, and a `;` in it would otherwise read as a dictionary entry.
    fn skip_filler(&mut self) {
        while let Some(&byte) = self.data.get(self.at) {
            if byte.is_ascii_whitespace() {
                self.at = self.at.saturating_add(1);
            } else if byte == b'%' {
                while let Some(&b) = self.data.get(self.at) {
                    self.at = self.at.saturating_add(1);
                    if b == b'\n' || b == b'\r' {
                        break;
                    }
                }
            } else {
                return;
            }
        }
    }

    /// The next token, or `None` at the end of the data.
    fn next(&mut self) -> Option<Token<'a>> {
        self.skip_filler();
        let &byte = self.data.get(self.at)?;
        // A name is a slash and then everything up to the next delimiter. A name may hold
        // any printable byte, so the delimiters are named rather than assumed.
        if byte == b'/' {
            self.at = self.at.saturating_add(1);
            let start = self.at;
            while let Some(&b) = self.data.get(self.at) {
                if is_delimiter(b) {
                    break;
                }
                self.at = self.at.saturating_add(1);
            }
            return Some(Token::Name(self.data.get(start..self.at)?));
        }
        // `(text)` — stepped over whole, escapes and all. The length is not bounded because
        // the data itself bounds it: the closing parenthesis has to be inside the file.
        if byte == b'(' {
            let mut depth = 1usize;
            self.at = self.at.saturating_add(1);
            while let Some(&b) = self.data.get(self.at) {
                self.at = self.at.saturating_add(1);
                match b {
                    b'\\' => {
                        self.at = self.at.saturating_add(1);
                    }
                    b'(' => depth = depth.saturating_add(1),
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            return Some(Token::Text);
        }
        if byte.is_ascii_digit() || byte == b'-' || byte == b'+' || byte == b'.' {
            let start = self.at;
            while let Some(&b) = self.data.get(self.at) {
                if b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'+' | b'E' | b'e') {
                    self.at = self.at.saturating_add(1);
                } else {
                    break;
                }
            }
            let text = std::str::from_utf8(self.data.get(start..self.at)?).ok()?;
            return Some(Token::Number(text.parse::<f32>().ok()?));
        }
        let start = self.at;
        while let Some(&b) = self.data.get(self.at) {
            if is_delimiter(b) {
                break;
            }
            self.at = self.at.saturating_add(1);
        }
        // A delimiter is its own one-byte word, which is how `[`, `]` and `}` come out.
        if self.at == start {
            self.at = self.at.saturating_add(1);
        }
        Some(Token::Word(self.data.get(start..self.at)?))
    }

    /// `len` bytes of `RD` body, which is binary and is therefore not tokenized.
    ///
    /// `RD` is followed by exactly one whitespace byte and then the data, per the format;
    /// a font that omits the space is tolerated by skipping one only when one is there.
    fn body(&mut self, len: usize) -> Option<&'a [u8]> {
        if self
            .data
            .get(self.at)
            .is_some_and(|b| b.is_ascii_whitespace())
        {
            self.at = self.at.saturating_add(1);
        }
        let end = self.at.checked_add(len)?;
        let body = self.data.get(self.at..end)?;
        self.at = end;
        Some(body)
    }
}

/// The bytes that end a token: whitespace, and the characters PostScript reserves.
fn is_delimiter(byte: u8) -> bool {
    byte.is_ascii_whitespace()
        || matches!(
            byte,
            b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
        )
}

/// One glyph: a name and the program that draws it.
#[derive(Debug, Clone, PartialEq)]
struct Glyph {
    name: String,
    code: Vec<u8>,
}

/// What a CID-keyed Type 1 font says about itself in its header.
///
/// A CID-keyed font is addressed by character identifier, and its glyph number *is* that
/// identifier — there is no encoding array to consult, which is why [`Type1::glyph_for_code`]
/// answers directly for one and has to look a name up for a name-keyed font.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CidInfo {
    /// The `/CIDFont` version, usually `0.001`.
    pub version: Option<f32>,
    /// The `/CIDFontInfo` `/registry` and `/ordering`, which name the character collection.
    pub registry: Option<String>,
    pub ordering: Option<String>,
}

/// A Type 1 font program, as much of one as a charstring walk needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Type1 {
    glyphs: Vec<Glyph>,
    subrs: Vec<Vec<u8>>,
    /// The font's own encoding as 256 glyph names, when it carries one as an array.
    ///
    /// `None` means it names a built-in encoding instead — `StandardEncoding`, or nothing at
    /// all, which the specification makes the same thing — and [`Type1::glyph_for_code`] then
    /// resolves through the standard encoding table.
    encoding: Option<Vec<String>>,
    /// `[a b c d e f]`, mapping a glyph's own units into ems.
    font_matrix: [f32; 6],
    /// `/UniqueID`, the number a font's name and version hash to.
    unique_id: Option<i64>,
    cid: Option<CidInfo>,
}

impl Type1 {
    /// The Type 1 font program in `data`, or the reason there is not one.
    pub fn parse(data: &[u8]) -> Result<Self, String> {
        if data.first() == Some(&0x80) {
            return Err(
                "it is a PFB container, whose program is wrapped in segment headers that are \
                 not stripped here, so nothing in it can be read as a font program"
                    .into(),
            );
        }
        if data.first() != Some(&b'%') {
            return Err(
                "it does not begin with `%!`, which every Type 1 font program's cleartext \
                 header does"
                    .into(),
            );
        }
        let split = eexec_split(data).ok_or(
            "its cleartext header never reaches an `eexec` keyword, so it is either not a \
             Type 1 font or a header with nothing encrypted behind it",
        )?;
        let (clear, cipher) = data.split_at(split);

        // The whole rest of the file is the ciphertext. A trailing `cleartomark` and `%%EOF`
        // decrypt to nothing useful, and looking for them would mean refusing a font whose
        // producer left them out — which is a real variation, so nothing is looked for.
        let plain = decipher(cipher, EEXEC_SEED);
        let body = plain
            .get(4..)
            .map(<[u8]>::to_vec)
            .ok_or("its encrypted portion is shorter than the four-byte salt that opens it")?;
        // The salt is followed by whitespace before the first real token, and how much of it
        // there is varies with the producer.
        let body = trim_leading(body.as_slice());

        let header = read_header(clear);

        // `/lenIV` decides the seed, and the seed is needed before the first body can be
        // read. `/lenIV` is declared before any body, so the first pass finds it and a
        // second pass with the seed that turns out to be right finishes the job. A font that
        // needs no advance — which is every font with the usual `lenIV` of 4, and every font
        // that does not mention one — is read once.
        let first = scan_bodies(&body, CHARSTRING_SEED)?;
        let seed = charstring_seed(first.len_iv.unwrap_or(DEFAULT_LEN_IV))?;
        let (glyphs, subrs) = if seed == CHARSTRING_SEED {
            (first.glyphs, first.subrs)
        } else {
            let second = scan_bodies(&body, seed)?;
            (second.glyphs, second.subrs)
        };

        if glyphs.is_empty() {
            return Err(
                "its encrypted portion has no glyphs in it, so there is nothing to draw".into(),
            );
        }

        let (encoding, cid) = read_encoding(clear, header.is_cid);
        Ok(Self {
            glyphs,
            subrs,
            encoding,
            font_matrix: header.font_matrix,
            unique_id: header.unique_id,
            cid,
        })
    }

    /// How many glyphs the font has.
    #[must_use]
    pub fn num_glyphs(&self) -> usize {
        self.glyphs.len()
    }

    /// The font's matrix, `[a b c d e f]`, mapping glyph units into ems.
    #[must_use]
    pub fn font_matrix(&self) -> [f32; 6] {
        self.font_matrix
    }

    /// The `/UniqueID` the header names, if it does.
    #[must_use]
    pub fn unique_id(&self) -> Option<i64> {
        self.unique_id
    }

    /// What a CID-keyed font says about itself, or `None` for a name-keyed one.
    #[must_use]
    pub fn cid_info(&self) -> Option<&CidInfo> {
        self.cid.as_ref()
    }

    /// Is this font CID-keyed, whose character identifiers are its glyph numbers?
    #[must_use]
    pub fn is_cid_keyed(&self) -> bool {
        self.cid.is_some()
    }

    /// The name one glyph is known by.
    #[must_use]
    pub fn glyph_name(&self, glyph: u32) -> Option<&str> {
        let index = usize::try_from(glyph).ok()?;
        self.glyphs.get(index).map(|g| g.name.as_str())
    }

    /// The glyph number one character code names.
    ///
    /// Three cases, and they are three different questions:
    ///
    /// * **A CID-keyed font**: the identifier *is* the glyph number. A Type 1 CID font is
    ///   addressed by `CID` and has no encoding array, so the code is taken as the number.
    /// * **A font carrying its own `/Encoding` array**: the code is an index into it, and the
    ///   name it holds is the glyph's name.
    /// * **A font naming a built-in encoding**, or naming none: the specification's answer
    ///   for a font with no built-in encoding is the standard encoding, so the standard
    ///   encoding's names are used.
    #[must_use]
    pub fn glyph_for_code(&self, code: u32) -> Option<u32> {
        if self.cid.is_some() {
            return (code < self.glyphs.len() as u32).then_some(code);
        }
        let name = match &self.encoding {
            Some(names) => names.get(usize::try_from(code).ok()?)?,
            None => crate::metrics::standard_glyph(code)?,
        };
        if name.is_empty() {
            return None;
        }
        self.glyphs
            .iter()
            .position(|g| g.name == name)
            .and_then(|found| u32::try_from(found).ok())
    }

    /// One glyph's outline, in ems, or the reason there is not one.
    pub fn outline(&self, glyph: u32) -> Result<Outline, String> {
        let Some(code) = self.charstring(glyph) else {
            let count = self.glyphs.len();
            return Err(format!(
                "glyph {glyph} is not one of its {count}, the highest being {}",
                count.saturating_sub(1)
            ));
        };
        Ok(Walker::new(self).walk(code)?.outline)
    }

    /// How wide one glyph is, in thousandths of an em, or the reason there is not one.
    ///
    /// The width comes from the glyph's own `hsbw`, in glyph units, and the font's matrix
    /// scales those to ems — so the answer means the same thing as the `/Widths` array it
    /// will be written into.
    pub fn advance(&self, glyph: u32) -> Result<u16, String> {
        let Some(code) = self.charstring(glyph) else {
            return Err(format!("glyph {glyph} is not one of this font's glyphs"));
        };
        let walked = Walker::new(self).walk(code)?;
        let width = walked
            .width
            .ok_or("that glyph's charstring names no width, having no `hsbw` in it")?;
        let scale = f64::from(self.font_matrix.first().copied().unwrap_or(0.0));
        let mils = f64::from(width) * 1000.0 * scale;
        Ok(mils.clamp(0.0, f64::from(u16::MAX)).round() as u16)
    }

    /// One glyph's charstring, or `None` if there is no such glyph.
    fn charstring(&self, glyph: u32) -> Option<&[u8]> {
        let index = usize::try_from(glyph).ok()?;
        self.glyphs.get(index).map(|g| g.code.as_slice())
    }
}

/// Trim the whitespace that follows the `eexec` salt.
fn trim_leading(data: &[u8]) -> Vec<u8> {
    let mut at = 0usize;
    while let Some(&byte) = data.get(at) {
        if byte.is_ascii_whitespace() {
            at = at.saturating_add(1);
        } else {
            break;
        }
    }
    data.get(at..).map(<[u8]>::to_vec).unwrap_or_default()
}

/// Where the ciphertext begins: past the `eexec` keyword and the whitespace after it.
///
/// The keyword is matched as a whole word, because the ciphertext is binary and contains the
/// five letters `eexec` by chance; the *first* match is taken, since a producer's own header
/// is the only one written in cleartext. Anything before it is the cleartext header.
fn eexec_split(data: &[u8]) -> Option<usize> {
    const KEY: &[u8] = b"eexec";
    let mut from = 0usize;
    while let Some(found) = find_from(data, KEY, from) {
        let before_ok = found == 0
            || data
                .get(found.wrapping_sub(1))
                .is_some_and(u8::is_ascii_whitespace);
        let after = found.saturating_add(KEY.len());
        let after_ok = data.get(after).is_none_or(u8::is_ascii_whitespace);
        if before_ok && after_ok {
            let mut at = after;
            while let Some(&byte) = data.get(at) {
                if byte.is_ascii_whitespace() {
                    at = at.saturating_add(1);
                } else {
                    break;
                }
            }
            return Some(at);
        }
        from = found.saturating_add(1);
    }
    None
}

/// `needle` in `haystack` at or after `from`, without pulling in a substring search.
fn find_from(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= haystack.len() {
        return None;
    }
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .and_then(|at| at.checked_add(from))
}

/// Walk the decrypted portion, collecting `/lenIV`, the glyphs and the subroutines.
///
/// `seed` is the cipher's starting point for the `RD` bodies. It cannot be *found* by this
/// function, because it is derived from `/lenIV` and `/lenIV` is in the middle of what this
/// function reads; the caller passes the default seed first, reads the `/lenIV` this
/// returns, and passes the seed that value implies.
fn scan_bodies(body: &[u8], seed: u32) -> Result<Bodies, String> {
    let mut scanner = Scanner::new(body);
    let mut numbers: Vec<f32> = Vec::new();
    let mut glyphs: Vec<Glyph> = Vec::new();
    let mut subrs: Vec<Vec<u8>> = Vec::new();
    let mut len_iv: Option<i32> = None;
    let mut pending: Option<String> = None;

    while let Some(token) = scanner.next() {
        match token {
            Token::Name(name) => {
                pending = Some(String::from_utf8_lossy(name).into_owned());
                continue;
            }
            Token::Number(value) => {
                if value.is_finite() {
                    numbers.push(value);
                    if numbers.len() > 2 {
                        numbers.remove(0);
                    }
                }
                continue;
            }
            // Any token other than a name or a number means the name before it was not a
            // glyph's, and a text string is never one.
            Token::Text => {
                pending = None;
                continue;
            }
            Token::Word(b"closefile" | b"cleartomark") => break,
            Token::Word(b"RD") => {
                let len = numbers.last().copied().unwrap_or(0.0);
                let index = numbers.get(numbers.len().wrapping_sub(2)).copied();
                numbers.clear();
                let len = if len.is_finite() && len > 0.0 {
                    len as usize
                } else {
                    0
                };
                if len > MAX_BODY {
                    return Err(format!(
                        "it declares a string of {len} bytes, which is past the limit this reads"
                    ));
                }
                let Some(bytes) = scanner.body(len) else {
                    // A body cut short is the end of the useful data rather than a reason to
                    // throw away the glyphs already read.
                    break;
                };
                let code = decipher(bytes, seed);
                match pending.take() {
                    Some(name) => {
                        if glyphs.len() >= MAX_GLYPHS {
                            return Err(format!(
                                "it declares more than {MAX_GLYPHS} glyphs, which is past the \
                                 limit this reads"
                            ));
                        }
                        glyphs.push(Glyph { name, code });
                    }
                    None => {
                        // A subroutine is `dup <n> <len> RD <bytes>`, and `<n>` is where it goes.
                        let Some(at) = index.map(|v| {
                            if v.is_finite() && v >= 0.0 {
                                v as usize
                            } else {
                                0
                            }
                        }) else {
                            continue;
                        };
                        if at >= MAX_SUBRS {
                            return Err(format!(
                                "its subroutines are indexed up to {at}, which is past the \
                                 limit this reads"
                            ));
                        }
                        if subrs.len() <= at {
                            subrs.resize(at.saturating_add(1), Vec::new());
                        }
                        if let Some(slot) = subrs.get_mut(at) {
                            *slot = code;
                        }
                    }
                }
                continue;
            }
            Token::Word(b"def") => {
                if pending.as_deref() == Some("lenIV")
                    && let Some(&value) = numbers.last()
                {
                    len_iv = Some(value as i32);
                }
            }
            Token::Word(_) => {}
        }
        pending = None;
    }
    Ok(Bodies {
        len_iv,
        glyphs,
        subrs,
    })
}

/// What one pass over the decrypted portion found.
#[derive(Debug, Clone, PartialEq)]
struct Bodies {
    len_iv: Option<i32>,
    glyphs: Vec<Glyph>,
    subrs: Vec<Vec<u8>>,
}

/// What the cleartext header says.
#[derive(Debug, Clone, PartialEq)]
struct Header {
    font_matrix: [f32; 6],
    unique_id: Option<i64>,
    is_cid: bool,
}

/// Read `/FontMatrix`, `/UniqueID` and whether this is a CID-keyed font, from the cleartext.
fn read_header(clear: &[u8]) -> Header {
    let text = String::from_utf8_lossy(clear);
    let mut header = Header {
        font_matrix: DEFAULT_FONT_MATRIX,
        unique_id: None,
        is_cid: text.contains("/CIDFontInfo"),
    };
    if let Some(matrix) = bracket_numbers(&text, "/FontMatrix")
        && let [a, b, c, d, e, f] = matrix.as_slice()
        && [a, b, c, d, e, f].iter().all(|v| v.is_finite())
        && a.abs() > f32::EPSILON
    {
        header.font_matrix = [*a, *b, *c, *d, *e, *f];
    }
    if let Some(id) = word_number(&text, "/UniqueID")
        && id.is_finite()
        && f64::from(id).abs() <= 9.2e18
    {
        header.unique_id = Some(id as i64);
    }
    // A font that says `/CIDFont` is CID-keyed whether or not it fills in `/CIDFontInfo`; a
    // CID font without an encoding array is addressed by number.
    header.is_cid |= text.contains("/CIDFont ");
    header.is_cid |= text.contains("/CIDFont\n");
    header
}

/// The six numbers of a `[ ... ]` array that follows `key`, if it has exactly six.
///
/// A `FontMatrix` written with braces rather than brackets — which half the fonts on a
/// machine do — is the same array, so both openings are accepted.
fn bracket_numbers(text: &str, key: &str) -> Option<Vec<f32>> {
    let at = text.find(key)?;
    let rest = text.get(at.saturating_add(key.len())..)?;
    let open = rest.find(['[', '{'])?;
    let close = if rest.as_bytes().get(open) == Some(&b'[') {
        rest.find(']')?
    } else {
        rest.find('}')?
    };
    let inner = rest.get(open.saturating_add(1)..close)?;
    inner
        .split_whitespace()
        .map(|word| word.parse::<f32>().ok())
        .collect()
}

/// The number that follows `key`, if it has one.
///
/// The key is matched as a whole word: `/CIDFont` is a prefix of `/CIDFontInfo`, and reading
/// the version out of the dictionary that holds the registry finds no number at all.
fn word_number(text: &str, key: &str) -> Option<f32> {
    let at = find_word(text, key)?;
    let rest = text.get(at.saturating_add(key.len())..)?.trim_start();
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '-' || c == '+' || c == '.'))
        .unwrap_or(rest.len());
    rest.get(..end)?.parse::<f32>().ok()
}

/// The font's built-in encoding, as 256 glyph names.
///
/// Three shapes are answered, and one of them is a refusal:
///
/// * `/Encoding 256 array dup 0 /A put …` — read, since that is the font naming every code's
///   glyph itself.
/// * `/Encoding StandardEncoding def` — no array, and the standard encoding table answers.
/// * No `/Encoding` at all — the specification makes that the same as the standard encoding.
///
/// Anything else names a built-in encoding this does not carry, and the standard encoding's
/// names are used for it: they are identical over the ASCII range, which is where prose
/// lives, and a code they do not assign simply has no glyph.
fn read_encoding(clear: &[u8], is_cid: bool) -> (Option<Vec<String>>, Option<CidInfo>) {
    let text = String::from_utf8_lossy(clear);
    let mut array = vec![String::new(); 256];
    let mut found = false;
    let mut at = 0usize;
    while let Some(hit) = text.get(at..).and_then(|rest| rest.find("dup ")) {
        let start = at.saturating_add(hit);
        let after = start.saturating_add(4);
        let Some(digits) = text.get(after..).map(|r| {
            let end = r.find(|c: char| !c.is_ascii_digit()).unwrap_or(r.len());
            &r[..end]
        }) else {
            break;
        };
        let Ok(code) = digits.parse::<usize>() else {
            at = after;
            continue;
        };
        let rest = text
            .get(after.saturating_add(digits.len())..)
            .unwrap_or_default();
        if let Some(name) = rest.trim_start().strip_prefix('/') {
            let end = name
                .find(|c: char| c.is_whitespace() || c == '/')
                .unwrap_or(name.len());
            if let Some(slot) = array.get_mut(code) {
                name.get(..end).unwrap_or_default().clone_into(slot);
                found = true;
            }
        }
        at = after.saturating_add(digits.len());
    }
    // The loop above finds `dup N /name put` anywhere in the cleartext, which for a font that
    // has no encoding array means finding one somewhere unrelated. Requiring the array's own
    // opening keeps a `dup` in the header from inventing an encoding.
    if found && !text.contains("/Encoding") {
        found = false;
        array = vec![String::new(); 256];
    }
    if is_cid {
        return (None, Some(read_cid_info(&text)));
    }
    (found.then_some(array), None)
}

/// A CID-keyed font's `/CIDFontInfo` and `/CIDFont` version.
fn read_cid_info(text: &str) -> CidInfo {
    CidInfo {
        version: word_number(text, "/CIDFont"),
        registry: word_name(text, "/registry"),
        ordering: word_name(text, "/ordering"),
    }
}

/// Where `key` appears as a whole word, and not as the start of a longer one.
fn find_word(text: &str, key: &str) -> Option<usize> {
    let mut from = 0usize;
    while let Some(found) = text.get(from..).and_then(|rest| rest.find(key)) {
        let at = from.saturating_add(found);
        let before_ok = at == 0
            || !text[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric());
        let after = at.saturating_add(key.len());
        let after_ok = !text
            .get(after..)
            .and_then(|rest| rest.chars().next())
            .is_some_and(|c| c.is_ascii_alphanumeric());
        if before_ok && after_ok {
            return Some(at);
        }
        from = at.saturating_add(1);
    }
    None
}

/// The value after `key`: either a `/name` or a `(string)`, which is how the two are both
/// written in a Type 1 header.
fn word_name(text: &str, key: &str) -> Option<String> {
    let at = find_word(text, key)?;
    let rest = text.get(at.saturating_add(key.len())..)?.trim_start();
    if let Some(name) = rest.strip_prefix('/') {
        let end = name
            .find(|c: char| c.is_whitespace() || c == '/')
            .unwrap_or(name.len());
        return Some(name.get(..end).unwrap_or_default().to_owned());
    }
    let inner = rest.strip_prefix('(')?;
    let end = inner.find(')')?;
    inner.get(..end).map(str::to_owned)
}

// ── the charstring interpreter ─────────────────────────────────────────────────────

/// One token of a Type 1 charstring.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Token1 {
    Number(f32),
    /// An operator in one byte.
    Op(u8),
    /// An operator in two bytes, the first of which was the escape `12`.
    Escaped(u8),
    /// The hint mask, which is followed by bytes of mask this walk does nothing with.
    Mask,
}

/// Read one token at `at`, and say how many bytes it took.
///
/// A truncated operand is an error rather than a number: the bytes after it are the next
/// token, and reading them as the tail of this one produces a program that runs.
fn token_at(code: &[u8], at: usize) -> Result<(Token1, usize), String> {
    let b0 = *code.get(at).ok_or("its charstring is cut short")?;
    let second = |at: usize| -> Result<u8, String> {
        code.get(at)
            .copied()
            .ok_or_else(|| "its charstring is cut short".to_owned())
    };
    let token = match b0 {
        32..=246 => (Token1::Number(f32::from(b0) - 139.0), 1),
        247..=250 => {
            let b1 = second(at.saturating_add(1))?;
            (
                Token1::Number(f32::from(b0 - 247) * 256.0 + f32::from(b1) + 108.0),
                2,
            )
        }
        251..=254 => {
            let b1 = second(at.saturating_add(1))?;
            (
                Token1::Number(-(f32::from(b0 - 251) * 256.0 + f32::from(b1) + 108.0)),
                2,
            )
        }
        // The sixteen-bit form. Type 1 has no four-byte integer and no binary real: a
        // 255 here is a five-byte 16.16 fixed that a few fonts in the wild carry.
        28 => {
            let raw = code
                .get(at.saturating_add(1)..at.saturating_add(3))
                .and_then(|r| r.get(..2))
                .ok_or("its charstring is cut short")?;
            let wide =
                i16::from_be_bytes([raw.first().copied().unwrap_or(0), *raw.get(1).unwrap_or(&0)]);
            (Token1::Number(f32::from(wide)), 3)
        }
        255 => {
            let raw = code
                .get(at.saturating_add(1)..at.saturating_add(5))
                .and_then(|r| r.get(..4))
                .ok_or("its charstring is cut short")?;
            let fixed = i32::from_be_bytes([
                raw.first().copied().unwrap_or(0),
                *raw.get(1).unwrap_or(&0),
                *raw.get(2).unwrap_or(&0),
                *raw.get(3).unwrap_or(&0),
            ]);
            (Token1::Number(fixed as f32 / 65536.0), 5)
        }
        0 => (Token1::Mask, 1),
        12 => (Token1::Escaped(second(at.saturating_add(1))?), 2),
        _ => (Token1::Op(b0), 1),
    };
    Ok(token)
}

/// A subroutine being executed, where in it the walk is, and how many stems *it* declared.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Frame<'c> {
    code: &'c [u8],
    at: usize,
    /// The stems declared in this code, which is what says how long its hint masks are.
    ///
    /// **Per frame, and not a single running total.** A charstring's mask covers the stems
    /// the charstring has declared, and a subroutine's covers the stems *the subroutine* has
    /// declared — so a mask inside a subroutine is empty unless that subroutine declares
    /// stems of its own. Reading it from the caller's total instead skips bytes that belong to
    /// the subroutine's operators: in a real font this turns the standard hint-replacement
    /// subroutine into a prefix of nonsense and every glyph that uses it loses its outline.
    stems: usize,
}

/// What a finished walk reports.
#[derive(Debug, PartialEq)]
struct Walked {
    outline: Outline,
    /// The width in glyph units, from the glyph's `hsbw`.
    width: Option<f32>,
}

/// The seven points of a flex in progress, before they are read as two curves.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Flex {
    points: [(f32, f32); 7],
    count: usize,
}

/// Executes one Type 1 charstring.
struct Walker<'a> {
    font: &'a Type1,
    segments: Vec<Segment>,
    stack: Vec<f32>,
    /// The pen, in glyph units rather than ems. The drawing operators are relative, and the
    /// matrix belongs where an absolute point becomes a segment.
    x: f32,
    y: f32,
    /// Where the contour being drawn began, and whether one is open.
    start: Option<(f32, f32)>,
    /// How many values a subroutine's results are still being read back.
    ///
    /// A `pop` after a `callothersubr` is not a pop: it is how the charstring says how many
    /// of the values the subroutine returned it is going to use, and the values themselves
    /// stay for the operator that does use them. A hint-replacement subroutine is
    /// `<count> 1 3 callothersubr pop callsubr` — the `pop` says "one value", and the
    /// `callsubr` then reads it as the subroutine number.
    results: usize,
    /// Whether the width has been taken off the stack yet.
    width: Option<f32>,
    /// The glyph's left sidebearing, in glyph units.
    ///
    /// **`hsbw`'s first operand says where the outline sits**, and a charstring's coordinates
    /// do not include it — the glyph is drawn at the pen plus this. Leaving it out puts every
    /// glyph a whole sidebearing to the left of where it belongs, which for a font whose
    /// sidebearings run to a tenth of an em is visible as loose, drifting lines.
    sidebearing: f32,
    /// A flex in progress, if `callothersubr` 1 started one.
    flex: Option<Flex>,
    done: bool,
}

impl<'a> Walker<'a> {
    fn new(font: &'a Type1) -> Self {
        Self {
            font,
            segments: Vec::new(),
            stack: Vec::new(),
            x: 0.0,
            y: 0.0,
            start: None,
            results: 0,
            width: None,
            sidebearing: 0.0,
            // A charstring's very first drawing operator follows the `hsbw` that put the pen
            // at the origin, so the pen has just moved even before any mover has run.
            flex: None,
            done: false,
        }
    }

    /// Run a charstring, and report the outline and the width.
    fn walk(mut self, code: &'a [u8]) -> Result<Walked, String> {
        let mut frames = vec![Frame {
            code,
            at: 0,
            stems: 0,
        }];
        while !self.done {
            let top = frames.len().saturating_sub(1);
            let Some(frame) = frames.get(top).copied() else {
                break;
            };
            // Running off the end of a charstring or of a subroutine ends it. A Type 1
            // charstring is supposed to finish with `endchar`, but a font whose last operator
            // is missing is a font whose shape is still knowable.
            if frame.code.get(frame.at).is_none() {
                frames.pop();
                continue;
            }
            let (token, len) = token_at(frame.code, frame.at)?;
            let mut next = frame.at.saturating_add(len);
            self.step(token, &mut frames)?;
            if token == Token1::Mask {
                let mask = frame.stems.div_ceil(8);
                if frame.code.len() < next.saturating_add(mask) {
                    return Err(format!(
                        "its hint mask claims {mask} bytes and only {} are left",
                        frame.code.len().saturating_sub(next)
                    ));
                }
                next = next.saturating_add(mask);
            }
            if let Some(frame) = frames.get_mut(top) {
                frame.at = next;
            }
        }
        self.close();
        // The sidebearing moves the whole glyph, so it is applied once at the end rather than
        // to every point as it is produced.
        let (dx, dy) = self.sidebearing_offset();
        let segments = self
            .segments
            .into_iter()
            .map(|s| shift(s, dx, dy))
            .collect();
        Ok(Walked {
            outline: Outline { segments },
            width: self.width,
        })
    }

    // ── the stack ────────────────────────────────────────────────────────────────

    fn take(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.stack)
    }

    fn push(&mut self, value: f32) -> Result<(), String> {
        if !value.is_finite() {
            return Err("one of its operands is not a number".into());
        }
        if self.stack.len() >= MAX_STACK {
            return Err(format!(
                "it leaves more than {MAX_STACK} values on the stack at once, which is past \
                 the limit this dialect sets"
            ));
        }
        self.stack.push(value);
        Ok(())
    }

    /// One value off the stack, or a reason there is none.
    fn pop(&mut self, name: &str) -> Result<f32, String> {
        self.stack
            .pop()
            .ok_or_else(|| format!("`{name}` has nothing on the stack to work on"))
    }

    // ── geometry ─────────────────────────────────────────────────────────────────

    /// A point in glyph units as a point in ems.
    #[allow(clippy::many_single_char_names)]
    fn ems(&self, x: f32, y: f32) -> (f64, f64) {
        let [a, b, c, d, e, f] = self.font.font_matrix;
        (f64::from(a * x + c * y + e), f64::from(b * x + d * y + f))
    }

    /// Where the sidebearing puts the glyph, in ems.
    #[allow(clippy::many_single_char_names)]
    fn sidebearing_offset(&self) -> (f64, f64) {
        let [a, b, ..] = self.font.font_matrix;
        (
            f64::from(a * self.sidebearing),
            f64::from(b * self.sidebearing),
        )
    }

    fn move_to(&mut self, x: f32, y: f32) {
        // **Inside a flex, a mover is not a mover.** The charstring walks the pen to each of
        // the flex's seven vertices with ordinary relative moves, and each is followed by the
        // othersubr number that claims it. Emitting a `Move` for those would cut the contour
        // into seven pieces and draw seven single-point contours instead of one curve.
        if self.flex.is_some() {
            self.x = x;
            self.y = y;
            return;
        }
        self.close();
        self.x = x;
        self.y = y;
        self.start = Some((x, y));
        let (ex, ey) = self.ems(x, y);
        self.segments.push(Segment::Move(ex, ey));
    }

    fn move_rel(&mut self, dx: f32, dy: f32) {
        self.move_to(self.x + dx, self.y + dy);
    }

    fn line_rel(&mut self, dx: f32, dy: f32) {
        self.begin_if_needed();
        self.x += dx;
        self.y += dy;
        let (ex, ey) = self.ems(self.x, self.y);
        self.segments.push(Segment::Line(ex, ey));
    }

    /// One curve, from three offsets, each from the point before it.
    #[allow(clippy::too_many_arguments)]
    fn curve_rel(&mut self, dx1: f32, dy1: f32, dx2: f32, dy2: f32, dx3: f32, dy3: f32) {
        self.begin_if_needed();
        let x1 = self.x + dx1;
        let y1 = self.y + dy1;
        let x2 = x1 + dx2;
        let y2 = y1 + dy2;
        self.x = x2 + dx3;
        self.y = y2 + dy3;
        let (ax, ay) = self.ems(x1, y1);
        let (bx, by) = self.ems(x2, y2);
        let (cx, cy) = self.ems(self.x, self.y);
        self.segments.push(Segment::Curve(ax, ay, bx, by, cx, cy));
    }

    /// A `lineto` or a curve before any `moveto` starts a contour where the pen is.
    fn begin_if_needed(&mut self) {
        if self.start.is_none() {
            self.start = Some((self.x, self.y));
            let (ex, ey) = self.ems(self.x, self.y);
            self.segments.push(Segment::Move(ex, ey));
        }
    }

    /// Close the contour being drawn, if one is open.
    fn close(&mut self) {
        let Some((sx, sy)) = self.start.take() else {
            return;
        };
        let (sx, sy) = self.ems(sx, sy);
        let (ex, ey) = self.ems(self.x, self.y);
        if (ex - sx).abs() > 1e-9 || (ey - sy).abs() > 1e-9 {
            self.segments.push(Segment::Line(sx, sy));
        }
    }

    // ── one token ────────────────────────────────────────────────────────────────

    fn step(&mut self, token: Token1, frames: &mut Vec<Frame<'a>>) -> Result<(), String> {
        match token {
            Token1::Number(v) => self.push(v),
            Token1::Mask => {
                // The mask's own operands are stem definitions, and they count like any
                // others: a font may leave out the declaration and let the mask claim it.
                let args = self.take();
                if let Some(frame) = frames.last_mut() {
                    frame.stems += args.len() / 2;
                }
                Ok(())
            }
            Token1::Op(op) => self.operator(op, frames),
            Token1::Escaped(op) => self.escaped(op, frames),
        }
    }

    fn operator(&mut self, op: u8, frames: &mut Vec<Frame<'a>>) -> Result<(), String> {
        match op {
            1 | 3 => {
                // hstem, vstem: pairs of stem positions.
                let args = self.take();
                let count = args.len() / 2;
                if let Some(frame) = frames.last_mut() {
                    frame.stems += count;
                }
                Ok(())
            }
            4 => {
                // vmoveto
                let args = self.take();
                let dy = one(&args, "vmoveto")?;
                self.move_rel(0.0, dy);
                Ok(())
            }
            5 => {
                // rlineto
                let args = self.take();
                whole(&args, 2, "rlineto")?;
                for chunk in args.chunks_exact(2) {
                    if let [a, b] = chunk {
                        self.line_rel(*a, *b);
                    }
                }
                Ok(())
            }
            6 | 7 => {
                // hlineto, vlineto: alternating, starting with whichever was named.
                let args = self.take();
                let horizontal = op == 6;
                for (i, v) in args.iter().enumerate() {
                    if horizontal == (i % 2 == 0) {
                        self.line_rel(*v, 0.0);
                    } else {
                        self.line_rel(0.0, *v);
                    }
                }
                Ok(())
            }
            8 => self.rrcurveto(),
            9 => {
                // closepath
                self.close();
                Ok(())
            }
            10 => self.callsubr(frames),
            11 => {
                // `return` ends the innermost subroutine. Returning from the charstring
                // itself ends it too, which the loop above notices.
                frames.pop();
                Ok(())
            }
            13 => {
                // hsbw: the side bearing and the width, which are the glyph's metrics. The
                // pen goes to the origin, which is what makes the first curve's absolute y
                // absolute rather than relative to wherever the last glyph left it.
                let args = self.take();
                let [sidebearing, width] = at(&args, "hsbw")? else {
                    return Err("`hsbw` was given too few operands".into());
                };
                self.width = Some(*width);
                self.sidebearing = *sidebearing;
                self.x = 0.0;
                self.y = 0.0;
                self.start = None;
                Ok(())
            }
            14 => {
                // endchar
                self.close();
                self.done = true;
                Ok(())
            }
            21 => {
                // rmoveto
                let args = self.take();
                let (dx, dy) = pair(&args, "rmoveto")?;
                self.move_rel(dx, dy);
                Ok(())
            }
            22 => {
                // hmoveto
                let args = self.take();
                let dx = one(&args, "hmoveto")?;
                self.move_rel(dx, 0.0);
                Ok(())
            }
            30 | 31 => self.alternating_curves(op == 30),
            // 15 is documented nowhere and reserved by the format; it appears in a handful of
            // fonts produced by tools that no longer exist, and nothing is known of what it
            // was meant to do, so it is refused rather than guessed at.
            15 => Err("it uses charstring operator 15, which is reserved and undocumented".into()),
            other => Err(format!(
                "it uses charstring operator {other}, which does not exist"
            )),
        }
    }

    /// `rrcurveto`: whole curves of six operands, all of them deltas.
    ///
    /// ## The absolute-y convention, and why it is not applied here
    ///
    /// The Type 1 specification records a convention: "the last dy value of the first
    /// rrcurveto following a moveto operator is an absolute value — it specifies the y value
    /// of the endpoint of the curve rather than a relative dy". Read naively as six deltas, a
    /// font written to that convention has every glyph offset by an amount that varies with
    /// its outline, which is exactly the kind of error nothing reports.
    ///
    /// **It is not applied here, and that is a measured decision rather than an oversight.**
    /// The 28 Type 1 faces installed on this machine (the URW and Nimbus families) are all
    /// written to the *other* convention — converted from a CFF outline, where every operand
    /// is a delta — and applying the rule moves the endpoint of the first curve in 18 of the
    /// 94 printable ASCII glyphs of `NimbusSans-Regular` alone. The same face is installed
    /// here as OpenType/CFF, and comparing glyph by glyph:
    ///
    /// * with the rule: 18 of 94 glyphs differ from the CFF outlines, by up to 0.72 em;
    /// * without it: 2 of 94 differ, both by less than 0.12 em, and both are the quotation
    ///   marks, where the two builds round the same design differently.
    ///
    /// `mutool`, the independent renderer the acceptance criteria name, draws these faces
    /// correctly, so the oracle agrees with the reading below. Reading the rule the other way
    /// costs 0.15 of SSIM on a page of ordinary text.
    fn rrcurveto(&mut self) -> Result<(), String> {
        let args = self.take();
        whole(&args, 6, "rrcurveto")?;
        for chunk in args.chunks_exact(6) {
            if let [a, b, c, d, e, f] = chunk {
                self.curve_rel(*a, *b, *c, *d, *e, *f);
            }
        }
        Ok(())
    }

    /// `vhcurveto` and `hvcurveto`: alternating runs of curves of four operands.
    ///
    /// Four operands, or five on the *last* curve of the run for the trailing coordinate that
    /// would otherwise be zero — so the count is not a multiple of four and must not be asked
    /// to be.
    fn alternating_curves(&mut self, vertical: bool) -> Result<(), String> {
        let mut args = self.take();
        let name = if vertical { "vhcurveto" } else { "hvcurveto" };
        if args.len() < 4 {
            return Err(format!(
                "`{name}` was given {} operands, and a curve takes four",
                args.len()
            ));
        }
        let mut vertical = vertical;
        while !args.is_empty() {
            if args.len() < 4 {
                return Err(format!(
                    "`{name}` was given {} operands, and a curve takes four",
                    args.len()
                ));
            }
            let last = args.len() == 5;
            let Some(chunk) = args.get(..if last { 5 } else { 4 }) else {
                return Err(format!("`{name}` was given fewer than four operands"));
            };
            let four: [f32; 4] = chunk
                .get(..4)
                .and_then(|c| <[f32; 4]>::try_from(c).ok())
                .ok_or_else(|| format!("`{name}` was given fewer than four operands"))?;
            let [a, b, c, d] = four;
            let extra = if last {
                chunk.get(4).copied().unwrap_or(0.0)
            } else {
                0.0
            };
            args.drain(..if last { 5 } else { 4 });
            if vertical {
                // The four operands are (dy1 dx2 dy2 dx3): the curve leaves the pen vertically
                // and arrives at it horizontally, so the first control point is at (0, dy1)
                // and the endpoint is at (dx3, 0).
                self.curve_rel(0.0, a, b, c, d, extra);
            } else {
                // The four are (dx1 dx2 dy2 dy3): the curve leaves the pen horizontally and
                // arrives vertically, so the first control point is at (dx1, 0) and the
                // endpoint is at (0, dy3).
                self.curve_rel(a, 0.0, b, c, extra, d);
            }
            vertical = !vertical;
        }
        Ok(())
    }

    /// `callsubr`: enter a subroutine, which is addressed by its number outright.
    ///
    /// **There is no subroutine-number bias in Type 1.** Adding CFF's bias lands on a
    /// *different, in-range* subroutine and runs the wrong program, which draws a plausible
    /// wrong shape rather than failing.
    fn callsubr(&mut self, frames: &mut Vec<Frame<'a>>) -> Result<(), String> {
        let number = self.pop("callsubr")?;
        if !number.is_finite() || number.fract() != 0.0 {
            return Err(format!(
                "`callsubr` was given {number}, which is not a subroutine number"
            ));
        }
        if frames.len() > MAX_DEPTH {
            return Err(format!(
                "it nests more than {MAX_DEPTH} subroutine calls, which is past the limit this \
                 dialect sets"
            ));
        }
        let wanted = number.max(0.0) as usize;
        let code = self
            .font
            .subrs
            .get(wanted)
            .map(Vec::as_slice)
            .ok_or_else(|| {
                format!(
                    "`callsubr` asks for subroutine {wanted} of {}, which it does not have",
                    self.font.subrs.len()
                )
            })?;
        frames.push(Frame {
            code,
            at: 0,
            stems: 0,
        });
        Ok(())
    }

    fn escaped(&mut self, op: u8, frames: &mut Vec<Frame<'a>>) -> Result<(), String> {
        match op {
            0 => {
                // dotsection: a hinting instruction, and there is nothing left to hint.
                let _ = self.take();
                Ok(())
            }
            1 | 2 => {
                // vstem3, hstem3: three stems rather than pairs.
                let args = self.take();
                let count = args.len() / 6;
                if let Some(frame) = frames.last_mut() {
                    frame.stems += count;
                }
                Ok(())
            }
            6 => Err(
                "it builds an accented character with `seac`, which names two glyphs this does \
                 not resolve"
                    .into(),
            ),
            7 => {
                // sbw: side bearing and width in four, which is `hsbw` with a y bearing.
                let args = self.take();
                let [sidebearing, _, _, width] = at(&args, "sbw")? else {
                    return Err("`sbw` was given too few operands".into());
                };
                self.width = Some(*width);
                self.sidebearing = *sidebearing;
                self.x = 0.0;
                self.y = 0.0;
                self.start = None;
                Ok(())
            }
            12 => {
                // div: divide the top two values. It exists so that a value too large for a
                // short operand can be brought back into range, which is arithmetic this
                // walk does not need — but it is honoured so that the stack stays where the
                // rest of the charstring expects it.
                let divisor = self.pop("div")?;
                let dividend = self.pop("div")?;
                if divisor == 0.0 {
                    return Err("`div` was asked to divide by zero".into());
                }
                self.push(dividend / divisor)
            }
            16 => self.call_other_subr(),
            17 => {
                // `pop` after a `callothersubr` says how many of the values that subroutine
                // returned the charstring is going to use, and leaves them there for the
                // operator that does use them. Anywhere else it is an ordinary discard.
                if self.results > 0 {
                    self.results -= 1;
                    return Ok(());
                }
                self.pop("pop").map(|_| ())
            }
            33 => self.set_current_point(),
            other => Err(format!("it uses escaped charstring operator 12 {other}")),
        }
    }

    /// `callothersubr`: hand control to a PostScript procedure this does not run.
    ///
    /// The stack holds the procedure's arguments, then how many of them there are, then the
    /// procedure's number — so the number is on top and the count is under it. Four numbers
    /// have conventional meanings that every real font uses them for, and those are
    /// implemented; the rest are PostScript this cannot run, and are **refused with the
    /// number named** rather than skipped, because the charstring goes on to consume whatever
    /// the procedure would have returned.
    fn call_other_subr(&mut self) -> Result<(), String> {
        self.results = 0;
        let number = self.pop("callothersubr")?;
        let count = self.pop("callothersubr")?;
        if !number.is_finite() || number.fract() != 0.0 {
            return Err(format!(
                "`callothersubr` was given {number} as the number to call, which is not one"
            ));
        }
        if !count.is_finite() || count < 0.0 {
            return Err(format!(
                "`callothersubr` claims {count} arguments, which is not a count"
            ));
        }
        let number = number as i32;
        let count = count as usize;
        match number {
            // Start a flex: remember where the pen is and wait for six more points.
            1 => {
                if count != 0 {
                    return Err(format!(
                        "`callothersubr` 1 starts a flex and takes no arguments, but it was \
                         given {count}"
                    ));
                }
                // Point zero is where the pen is when the flex starts, which is the point the
                // first curve begins at; the six that follow are added one at a time.
                let mut flex = Flex {
                    points: [(0.0, 0.0); 7],
                    count: 0,
                };
                if let Some(first) = flex.points.first_mut() {
                    *first = (self.x, self.y);
                }
                flex.count = 1;
                self.flex = Some(flex);
                Ok(())
            }
            // Add a flex point. The pen's position *is* the point: the charstring moves the
            // pen with ordinary operators and then says "this is one of the flex's vertices".
            2 => {
                let Some(flex) = self.flex.as_mut() else {
                    return Err(
                        "`callothersubr` 2 adds a flex point, but no flex was started".into(),
                    );
                };
                if count != 0 {
                    return Err(format!(
                        "`callothersubr` 2 adds a flex point and takes no arguments, but it \
                         was given {count}"
                    ));
                }
                let index = flex.count.min(6);
                if let Some(slot) = flex.points.get_mut(index) {
                    *slot = (self.x, self.y);
                }
                flex.count = flex.count.saturating_add(1);
                Ok(())
            }
            // End a flex: the seven points are two curves, and the pen ends where they do.
            0 => {
                let Some(flex) = self.flex.take() else {
                    return Err("`callothersubr` 0 ends a flex, but no flex was started".into());
                };
                if count != 3 {
                    return Err(format!(
                        "`callothersubr` 0 ends a flex and takes three arguments, but it was \
                         given {count}"
                    ));
                }
                if flex.count != 7 {
                    return Err(format!(
                        "its flex names {} points where a flex has seven",
                        flex.count
                    ));
                }
                // **The subroutine returns three numbers**, and the charstring reads them
                // back off the stack: the top two are where the flex ended and the third is
                // discarded. They are the pen's own position, so the `setcurrentpoint` that
                // reads them is a no-op — which is what makes it safe to have put the flex
                // itself in the right place rather than leaving it to that correction.
                let (x, y) = (self.x, self.y);
                self.results = 3;
                self.flex_curve(flex);
                self.push(0.0)?;
                self.push(x)?;
                self.push(y)
            }
            // Change the hints. This is refused, and *only* this: the glyph's geometry does
            // not depend on it, and there is no pixel grid here for a replacement to fit.
            3 => {
                if count != 1 {
                    return Err(format!(
                        "`callothersubr` 3 replaces hints and takes one argument, but it was \
                         given {count}"
                    ));
                }
                // The argument is how many hints were replaced, and it is put back: the
                // charstring goes on to use it, and dropping it would desynchronise the rest
                // of the program.
                let hints = self.pop("callothersubr")?;
                self.results = 1;
                self.push(hints)
            }
            // 12 and 13 are a pair of counter controls used by stem-building code: they
            // exist only to manage the hint counters, which are the thing being refused.
            12 | 13 => {
                self.stack.clear();
                Ok(())
            }
            other => Err(format!(
                "it calls PostScript subroutine {other} with {count} arguments, which is not \
                 one of the four this gives a meaning to, and a subroutine whose result the \
                 charstring goes on to use is not one to skip"
            )),
        }
    }

    /// Two curves through the seven points a flex collected.
    ///
    /// Points three and six are the curves' endpoints and the rest are their control points,
    /// so this is exactly an `hflex`: the first curve is flat, the second is flat, and the
    /// two in-between verticals are the bends.
    fn flex_curve(&mut self, flex: Flex) {
        let [p0, p1, p2, p3, p4, p5, p6] = flex.points;
        // The pen is where the flex started, whatever the six `rmoveto`s did to get there.
        // A contour that is not already open begins here, and `begin_if_needed` cannot do it:
        // it would use the pen's *current* position, which is the flex's last vertex.
        self.x = p0.0;
        self.y = p0.1;
        self.begin_if_needed();
        let (ax, ay) = self.ems(p1.0, p0.1);
        let (bx, by) = self.ems(p2.0, p2.1);
        let (cx, cy) = self.ems(p3.0, p3.1);
        self.segments.push(Segment::Curve(ax, ay, bx, by, cx, cy));
        let (ax, ay) = self.ems(p4.0, p3.1);
        let (bx, by) = self.ems(p5.0, p5.1);
        let (cx, cy) = self.ems(p6.0, p5.1);
        self.segments.push(Segment::Curve(ax, ay, bx, by, cx, cy));
        self.x = p6.0;
        self.y = p5.1;
    }

    /// `setcurrentpoint`: where the pen should be, given the values a subroutine returned.
    ///
    /// The values come off the stack — three of them by the convention the flex uses, the top
    /// two of which are the position and the third of which is discarded — and they *set* the
    /// pen rather than nudging it, which is the reading the specification supports: a
    /// `setcurrentpoint` follows a flex, and the flex has already put the pen exactly there.
    ///
    /// Fewer than two values is ignored rather than refused. Ghostscript and Adobe Distiller
    /// both ignore a `setcurrentpoint` that has nothing sensible to read, and they are the
    /// implementations real fonts were built against; refusing would lose a glyph over a
    /// bookkeeping instruction that cannot change its shape.
    fn set_current_point(&mut self) -> Result<(), String> {
        let args = self.take();
        let Some(&dx) = args.get(args.len().wrapping_sub(2)) else {
            return Ok(());
        };
        let Some(&dy) = args.get(args.len().wrapping_sub(1)) else {
            return Ok(());
        };
        if (dx - self.x).abs() <= 1e-9 && (dy - self.y).abs() <= 1e-9 {
            return Ok(());
        }
        self.close();
        self.x = dx;
        self.y = dy;
        self.start = Some((self.x, self.y));
        let (ex, ey) = self.ems(self.x, self.y);
        self.segments.push(Segment::Move(ex, ey));
        Ok(())
    }
}

/// One segment moved by `(dx, dy)`.
// A curve has six coordinates and the specification names them a, b, c, d, e and f; spelling
// them x1, x2, x3, y1, y2, y3 would be a different and worse notation.
#[allow(clippy::many_single_char_names)]
fn shift(segment: Segment, dx: f64, dy: f64) -> Segment {
    match segment {
        Segment::Move(x, y) => Segment::Move(x + dx, y + dy),
        Segment::Line(x, y) => Segment::Line(x + dx, y + dy),
        Segment::Curve(a, b, c, d, e, f) => {
            Segment::Curve(a + dx, b + dy, c + dx, d + dy, e + dx, f + dy)
        }
    }
}

/// An operator's operands as a slice, insisting there are some.
fn at<'f>(args: &'f [f32], name: &str) -> Result<&'f [f32], String> {
    if args.is_empty() {
        Err(format!("`{name}` was given no operands"))
    } else {
        Ok(args)
    }
}

/// An operator's operands, insisting there is at least one.
fn one(args: &[f32], name: &str) -> Result<f32, String> {
    args.first()
        .copied()
        .ok_or_else(|| format!("`{name}` was given no operands"))
}

/// An operator's operands, insisting there are at least two.
fn pair(args: &[f32], name: &str) -> Result<(f32, f32), String> {
    match args {
        [a, b, ..] => Ok((*a, *b)),
        _ => Err(format!("`{name}` needs two operands")),
    }
}

/// Insist the operands divide into whole groups, and that there are some.
fn whole(args: &[f32], size: usize, name: &str) -> Result<(), String> {
    if args.is_empty() {
        return Err(format!("`{name}` was given no operands"));
    }
    if args.len() % size != 0 {
        return Err(format!(
            "`{name}` was given {} operands, which is not a whole number of {size}-operand \
             groups",
            args.len()
        ));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect` and index the charstrings they have just
    // built. Both are what a test is for: the panic-free rule is about what the product does
    // with a font from the internet, not about how a test reads one it wrote two functions
    // above.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::float_cmp,
        // Six single-letter names is a curve. The specification names them a, b, c, d, e and
        // f, and spelling them x1, x2, x3, y1, y2, y3 would be a different and worse notation.
        clippy::many_single_char_names
    )]

    use std::io::Write as _;

    use super::*;

    // ── building a Type 1 font, here ────────────────────────────────────────────
    //
    // Every test below needs a font program, and a real one is somebody else's work that may
    // or may not be installed. So these tests build their own, byte by byte, from the format
    // rather than from a copy. That is also the only way to be sure what is being exercised:
    // an outline compared against a stored snapshot proves only that the snapshot has not
    // changed, and a cipher checked only against its own implementation proves only that the
    // two agree.

    /// A charstring, written the way a writer writes it.
    ///
    /// Each builder takes and returns, so a program reads as the list of things it says.
    #[derive(Debug, Default, Clone)]
    struct Cs {
        bytes: Vec<u8>,
    }

    impl Cs {
        fn new() -> Self {
            Self::default()
        }

        /// An operand, in whichever of the format's encodings is shortest.
        fn arg(mut self, value: i32) -> Self {
            match value {
                -107..=107 => self.bytes.push((value + 139) as u8),
                108..=1131 => {
                    let v = value - 108;
                    self.bytes.push((v / 256 + 247) as u8);
                    self.bytes.push((v % 256) as u8);
                }
                -1131..=-108 => {
                    let v = -value - 108;
                    self.bytes.push((v / 256 + 251) as u8);
                    self.bytes.push((v % 256) as u8);
                }
                _ => {
                    self.bytes.push(28);
                    self.bytes
                        .extend_from_slice(&i16::try_from(value).unwrap().to_be_bytes());
                }
            }
            self
        }

        /// An operator in one byte.
        fn op(mut self, op: u8) -> Self {
            self.bytes.push(op);
            self
        }

        /// An operator in two bytes, the first of which is the escape.
        fn escape(mut self, op: u8) -> Self {
            self.bytes.push(12);
            self.bytes.push(op);
            self
        }

        /// Bytes that follow an operator without being read as one, which is what a hint
        /// mask is.
        fn raw(mut self, bytes: &[u8]) -> Self {
            self.bytes.extend_from_slice(bytes);
            self
        }

        /// `return`.
        fn ret(mut self) -> Self {
            self.bytes.push(11);
            self
        }

        /// `n othersubr`, the two operands a `callothersubr` is preceded by.
        fn othersubr(self, arguments: i32, number: i32) -> Self {
            self.arg(arguments).arg(number).escape(16)
        }

        fn build(self) -> Vec<u8> {
            self.bytes
        }
    }

    /// The cleartext header every test font carries, plus whatever it needs said.
    fn header(extra: &str) -> String {
        format!(
            "%!PS-AdobeFont-1.0: TestFont 001.001\n\
             12 dict begin\n\
             /FontType 1 def\n\
             /FontMatrix [0.001 0 0 0.001 0 0] def\n\
             /FontName /TestFont def\n\
             /FontBBox {{0 0 1000 1000}} def\n\
             {extra}\
             currentdict end\n\
             currentfile eexec\n"
        )
    }

    /// A whole Type 1 font program: a header, then the encrypted portion.
    ///
    /// `len_iv` is written into the Private DICT only when it is `Some`, so that the common
    /// case — a font that does not mention it at all — is the default case of these tests
    /// rather than a special one.
    fn font(len_iv: Option<i32>, subrs: &[Vec<u8>], glyphs: &[(&str, Vec<u8>)]) -> Vec<u8> {
        font_with_header(
            &header("/Encoding StandardEncoding def\n"),
            len_iv,
            SALT,
            subrs,
            glyphs,
        )
    }

    fn font_with_header(
        head: &str,
        len_iv: Option<i32>,
        salt: &[u8],
        subrs: &[Vec<u8>],
        glyphs: &[(&str, Vec<u8>)],
    ) -> Vec<u8> {
        // A body's seed is the one the font's own `/lenIV` implies, so that a font declaring
        // an advance is encrypted the way a writer would encrypt it.
        let seed = charstring_seed(len_iv.unwrap_or(DEFAULT_LEN_IV)).expect("a seed");

        // The encrypted portion is assembled as bytes rather than as a string: an `RD` body is
        // binary, and a string would have to be built with something lossy to hold it.
        let mut body: Vec<u8> = b"dup\n/Private 8 dict dup begin\n".to_vec();
        if let Some(value) = len_iv {
            let _ = writeln!(body, "/lenIV {value} def");
        }
        if !subrs.is_empty() {
            let _ = writeln!(body, "/Subrs {} array", subrs.len());
            for (i, code) in subrs.iter().enumerate() {
                let _ = write!(body, "dup {i} {} RD ", code.len());
                body.extend_from_slice(&encipher(code, seed));
                body.extend_from_slice(b" NP\n");
            }
            body.extend_from_slice(b"ND\nput\n");
        }
        let _ = writeln!(
            body,
            "readonly put\n/CharStrings {} dict dup begin",
            glyphs.len()
        );
        for (name, code) in glyphs {
            let _ = write!(body, "/{name} {} RD ", code.len());
            body.extend_from_slice(&encipher(code, seed));
            body.extend_from_slice(b" ND\n");
        }
        body.extend_from_slice(b"end end\nreadonly put\nput\n");
        body.extend_from_slice(b"dup /FontName get exch definefont pop\n");
        body.extend_from_slice(b"mark currentfile closefile\n");

        // The salt is four *plaintext* bytes at the front of the encrypted stream, not four
        // bytes written before it: the cipher runs on through them, so they are part of what
        // is enciphered and a reader discarding them has already advanced its state.
        let mut plain = salt.to_vec();
        plain.push(b'\n');
        plain.extend_from_slice(&body);
        let mut out = head.as_bytes().to_vec();
        out.extend_from_slice(&encipher(&plain, EEXEC_SEED));
        out.extend_from_slice(b"\ncleartomark\n");
        out
    }

    /// The four random bytes that open the encrypted stream.
    const SALT: &[u8] = b"\x9c\x2f\x41\xd1";

    /// The cipher run forwards, which is what a font builder does to write a body.
    fn encipher(plain: &[u8], seed: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity(plain.len());
        let mut r = seed;
        for &p in plain {
            let c = (u32::from(p) ^ (r >> 8)) as u8;
            out.push(c);
            r = ((u32::from(c) + r) * CIPHER_MULTIPLIER + CIPHER_INCREMENT) & 0xFFFF;
        }
        out
    }

    /// Are two outlines the same, to within the precision a `f32` charstring is worked in?
    ///
    /// Exact equality would be the wrong assertion here: the coordinates are accumulated in
    /// `f32` and converted to `f64` at the last moment, so a quarter of an em in glyph units
    /// arrives as `0.10000000149011612`. The tolerance is far tighter than anything a glyph
    /// is wrong by, which is what makes the comparison a real one.
    fn same(outline: &Outline, want: &[Segment]) {
        assert_eq!(outline.segments.len(), want.len(), "segment count");
        for (got, wanted) in outline.segments.iter().zip(want) {
            let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
            let points = |s: &Segment| -> Vec<(f64, f64)> {
                match *s {
                    Segment::Move(x, y) | Segment::Line(x, y) => vec![(x, y)],
                    Segment::Curve(a, b, c, d, e, f) => vec![(a, b), (c, d), (e, f)],
                }
            };
            let (g, w) = (points(got), points(wanted));
            assert_eq!(g.len(), w.len());
            for ((gx, gy), (wx, wy)) in g.iter().zip(&w) {
                assert!(
                    close(*gx, *wx) && close(*gy, *wy),
                    "{got:?} is not {wanted:?}: ({gx}, {gy}) against ({wx}, {wy})"
                );
            }
        }
    }

    /// A one-glyph font drawing a square 100 by 200 glyph units with a sidebearing of zero,
    /// which is the baseline everything else is measured against.
    fn square() -> Vec<u8> {
        font(
            None,
            &[],
            &[(
                "square",
                Cs::new()
                    .arg(0) // sbx
                    .arg(500) // wx
                    .op(13) // hsbw
                    .arg(0)
                    .arg(0)
                    .op(21) // rmoveto
                    .arg(100)
                    .arg(0)
                    .op(5) // rlineto
                    .arg(0)
                    .arg(200)
                    .op(5)
                    .arg(-100)
                    .arg(0)
                    .op(5)
                    .arg(0)
                    .arg(-200)
                    .op(5)
                    .op(9) // closepath
                    .op(14) // endchar
                    .build(),
            )],
        )
    }

    // ── the ciphers ────────────────────────────────────────────────────────────

    /// `eexec` against a pair worked out by hand from the published rule.
    ///
    /// The plaintext is `%!FontType1\n`, and the ciphertext is `fc 2e 52 96 e7 f6 43 e8
    /// 7c cf a0 25`. Both are written out in the code so that this pins the *rule* rather
    /// than agreeing with whatever the implementation happens to do: the first byte is the
    /// whole argument, because `0x25` is `37 XOR 217` and `217` is `55665 >> 8`, so a
    /// reader that seeded `r` with anything else, or XORed with something other than `r >> 8`,
    /// gets a different first byte and fails here.
    ///
    /// The second half of the rule is which byte goes into the recurrence, and it is the one
    /// place the algorithm can be read two ways. From the third byte on, the two readings
    /// differ, so a thirteen-byte vector pins it: the recurrence here is fed the **ciphertext**
    /// byte. `0x52` (82) is the ciphertext for plaintext `0x46` (70) at `r = 5213`; feeding
    /// the plaintext back in gives `((70 + 5213) * 52845 + 22719) mod 65536 = 57207`, and the
    /// fourth byte would then be wrong.
    #[test]
    fn eexec_matches_a_pair_worked_out_from_the_rule() {
        let plain = b"%!FontType1\n";
        let expected = [
            0xfcu8, 0x2e, 0x52, 0x96, 0xe7, 0xf6, 0x43, 0xe8, 0x7c, 0xcf, 0xa0, 0x25,
        ];
        assert_eq!(decipher(&expected, EEXEC_SEED), plain);

        // The other direction, because a builder has to be able to write what a reader reads.
        assert_eq!(encipher(plain, EEXEC_SEED), expected);

        // Step by step, so a failure says which step went wrong.
        let mut r = EEXEC_SEED;
        for (i, (&cipher, &want)) in expected.iter().zip(plain.iter()).enumerate() {
            assert_eq!(u32::from(cipher) ^ (r >> 8), u32::from(want), "byte {i}");
            r = ((u32::from(cipher) + r) * CIPHER_MULTIPLIER + CIPHER_INCREMENT) & 0xFFFF;
        }
    }

    /// The charstring cipher is the same cipher under a different seed, and the seeds are what
    /// make the two layers different.
    ///
    /// `0 0 hsbw` — the first thing every Type 1 glyph says — encrypts under 4330 to
    /// `20 86 d2 8c 67 84 0c 8f`. Reading it under 55665 gives something else, which is the
    /// whole of why the two layers must not be swapped.
    #[test]
    fn the_charstring_cipher_is_the_same_cipher_under_its_own_seed() {
        let plain = b"0 0 hsbw";
        let expected = [0x20u8, 0x86, 0xd2, 0x8c, 0x67, 0x84, 0x0c, 0x8f];
        assert_eq!(decipher(&expected, CHARSTRING_SEED), plain);
        assert_ne!(
            decipher(&expected, EEXEC_SEED),
            plain,
            "the seeds are what tell the two layers apart, so they cannot agree"
        );
    }

    /// The four salt bytes come off, then the whitespace, and the two layers are undone in
    /// that order — outer first.
    ///
    /// The font below draws a square whose corners are at (0, 0), (200, 0), (200, 300) and
    /// (0, 300) in glyph units, which is (0, 0), (0.2, 0), (0.2, 0.3) and (0, 0.3) in ems at
    /// the font's own thousandth-of-an-em matrix. Those four numbers come out only if the
    /// `eexec` layer is undone first and the charstring layer second: a font whose charstring
    /// was encrypted under the outer seed would decrypt to bytes that are not a program, and
    /// the glyph would come back empty rather than square.
    #[test]
    fn both_layers_are_undone_before_anything_is_drawn() {
        let program = square();
        let font = Type1::parse(&program).expect("a Type 1 font");
        assert_eq!(font.num_glyphs(), 1);

        let outline = font.outline(0).expect("a square");
        // The four corners, in order.
        let corners: Vec<(f64, f64)> = outline
            .segments
            .iter()
            .filter_map(|s| match s {
                Segment::Move(x, y) | Segment::Line(x, y) => Some((*x, *y)),
                Segment::Curve(..) => None,
            })
            .collect();
        let want = [(0.0, 0.0), (0.1, 0.0), (0.1, 0.2), (0.0, 0.2)];
        for (got, wanted) in corners.iter().zip(&want) {
            assert!(
                (got.0 - wanted.0).abs() < 1e-6 && (got.1 - wanted.1).abs() < 1e-6,
                "four corners of the square in ems: {got:?} against {wanted:?}"
            );
        }

        // The salt really is four random bytes: their value is the font's choice and changes
        // nothing about what comes out.
        let other = font_with_header(
            &header("/Encoding StandardEncoding def\n"),
            None,
            b"\x00\x11\xfe\x5c",
            &[],
            &[(
                "square",
                Cs::new()
                    .arg(0)
                    .arg(500)
                    .op(13)
                    .arg(0)
                    .arg(0)
                    .op(21)
                    .arg(100)
                    .arg(0)
                    .op(5)
                    .arg(0)
                    .arg(200)
                    .op(5)
                    .arg(-100)
                    .arg(0)
                    .op(5)
                    .arg(0)
                    .arg(-200)
                    .op(5)
                    .op(9)
                    .op(14)
                    .build(),
            )],
        );
        let font = Type1::parse(&other).expect("the same font with a different salt");
        assert_eq!(
            font.outline(0).expect("the same square"),
            outline,
            "the four salt bytes are discarded, so their value is the font's choice"
        );
    }

    /// A `lenIV` of 4 — and, more to the point, no `lenIV` at all — leaves the charstring
    /// seed at 4330, and a font that advances it has its bodies read again.
    #[test]
    fn len_iv_advances_the_charstring_seed_and_nothing_else() {
        let plain = Cs::new()
            .arg(0)
            .arg(0)
            .op(13)
            .arg(0)
            .arg(0)
            .op(21)
            .arg(10)
            .arg(0)
            .op(5)
            .arg(0)
            .arg(10)
            .op(5)
            .arg(-10)
            .arg(0)
            .op(5)
            .arg(0)
            .arg(-10)
            .op(5)
            .op(14)
            .build();
        // `lenIV` 4 is the seed at 4330; `lenIV` 6 advances it twice, and the bodies have to
        // be read with the seed that comes out of the other side of that.
        for value in [None, Some(4), Some(6)] {
            let program = font(value, &[], &[("square", plain.clone())]);
            let font = Type1::parse(&program).expect("a Type 1 font");
            let outline = font.outline(0).expect("a square");
            assert!(
                !outline.is_empty(),
                "lenIV {value:?} should still draw the square it declares"
            );
        }
        assert_eq!(charstring_seed(4), Ok(CHARSTRING_SEED));
        assert_ne!(charstring_seed(6), Ok(CHARSTRING_SEED));
    }

    // ── the absolute-y rule ────────────────────────────────────────────────────

    /// An `rrcurveto`'s six operands are six deltas, including the first curve after a mover.
    ///
    /// The Type 1 specification records a convention in which the first `rrcurveto` after a
    /// mover reads its last *y* as an absolute position, and the reasoning for it is good:
    /// read naively, a font written to that convention has every glyph offset by an amount
    /// that varies with its outline. **The rule is not applied here**, and this test is the
    /// hand-computed case that says so — it is the `e` of `NimbusSans-Regular`, whose first
    /// curve is the one the convention would change:
    ///
    /// ```text
    /// hsbw 40 556            sidebearing 40, advance 556
    /// rmoveto 473 234        the pen is at (473, 234)
    /// rrcurveto 0 80 -6 48 -15 39
    /// ```
    ///
    /// The first control point is at (473, 314), the second at (467, 362), and the endpoint
    /// is either (467 − 15, 362 + 39) = (452, 401) as six deltas, or (452, 39) if the last y
    /// is absolute. **It is (452, 401).** The same face is installed as OpenType/CFF and its
    /// `e` ends that first curve at exactly (452 + 40, 401) — and `mutool` draws the face
    /// correctly, which the reading where it is (492, 39) does not: the glyph is then 0.36 em
    /// too short at the top and the `e` loses its bowl. The measurement across all 94
    /// printable ASCII glyphs of that face is 18 mismatches with the convention applied and 2
    /// without, and the same two faces in their CFF form are what those are measured against.
    #[test]
    fn a_curve_after_a_mover_is_six_deltas() {
        let program = font(
            None,
            &[],
            &[(
                "e",
                Cs::new()
                    .arg(40) // sbx
                    .arg(556) // wx
                    .op(13) // hsbw
                    .arg(473)
                    .arg(234)
                    .op(21) // rmoveto
                    .arg(0)
                    .arg(80)
                    .arg(-6)
                    .arg(48)
                    .arg(-15)
                    .arg(39)
                    .op(8) // rrcurveto
                    .op(14)
                    .build(),
            )],
        );
        let font = Type1::parse(&program).expect("a Type 1 font");
        let outline = font.outline(0).expect("a curve");
        // The sidebearing of 40 moves the whole glyph, so the endpoint is at 452 + 40.
        same(
            &outline,
            &[
                Segment::Move(0.513, 0.234),
                Segment::Curve(0.513, 0.314, 0.507, 0.362, 0.492, 0.401),
                Segment::Line(0.513, 0.234),
            ],
        );
    }

    /// A `rrcurveto` with a run of curves draws all of them, and a truncated group is a
    /// reason rather than a dropped corner.
    #[test]
    fn a_run_of_curves_is_drawn_whole() {
        let program = font(
            None,
            &[],
            &[(
                "two",
                Cs::new()
                    .arg(0)
                    .arg(500)
                    .op(13) // hsbw
                    .arg(0)
                    .arg(100)
                    .op(21) // rmoveto
                    .arg(0)
                    .arg(0)
                    .arg(0)
                    .arg(50)
                    .arg(0)
                    .arg(60) // y = 100 + 50 + 60 = 210
                    .arg(0)
                    .arg(0)
                    .arg(0)
                    .arg(20)
                    .arg(0)
                    .arg(25) // y = 210 + 20 + 25 = 255
                    .op(8) // rrcurveto, two curves
                    .op(14)
                    .build(),
            )],
        );
        let ends: Vec<f64> = Type1::parse(&program)
            .expect("a Type 1 font")
            .outline(0)
            .expect("two curves")
            .segments
            .iter()
            .filter_map(|s| match s {
                Segment::Curve(_, _, _, _, _, y) => Some(*y),
                _ => None,
            })
            .collect();
        assert_eq!(ends.len(), 2, "both curves of the run are drawn");
        assert!(
            (ends[0] - 0.21).abs() < 1e-6 && (ends[1] - 0.255).abs() < 1e-6,
            "each curve starts where the last one ended: {ends:?}"
        );

        // And a group of five is a reason, because drawing four and dropping the fifth is how
        // a glyph comes out missing a corner with nothing reporting it.
        let truncated = font(
            None,
            &[],
            &[(
                "short",
                Cs::new()
                    .arg(0)
                    .arg(500)
                    .op(13)
                    .arg(0)
                    .arg(0)
                    .arg(1)
                    .arg(2)
                    .arg(3)
                    .op(8)
                    .op(14)
                    .build(),
            )],
        );
        let reason = Type1::parse(&truncated)
            .expect("a Type 1 font")
            .outline(0)
            .expect_err("five operands");
        assert!(reason.contains("6-operand"), "{reason}");
    }

    // ── the flex ───────────────────────────────────────────────────────────────

    /// `callothersubr` 1, six of them numbered 2, and then 0: the flex.
    ///
    /// The pen is moved to each of the seven vertices in turn and each move is followed by
    /// the othersubr number that claims it as a flex point. The vertices are
    ///
    /// ```text
    /// p0 = (0, 0)   the point the flex started from
    /// p1 = (100, 0)  p2 = (200, 50)  p3 = (300, 0)
    /// p4 = (400, 0)  p5 = (500, 80)  p6 = (600, 80)
    /// ```
    ///
    /// which are two flat curves joined by a rise and a fall: the first from (0,0) through
    /// (100,0) and (200,50) to (300,0), the second from (300,0) through (400,0) and
    /// (500,80) to (600,80). That is an `hflex` with `dy2 = 50` and `dy5 = 80`.
    ///
    /// The three numbers the subroutine returns are the other half of the protocol: `callothersubr`
    /// 0 leaves the pen's position on the stack, and the `pop pop setcurrentpoint` that
    /// follows reads them — the top two move the pen, the third is discarded — which is why
    /// the flex ends where the vertices say rather than twice as far along.
    #[test]
    fn the_flex_path_draws_two_curves_and_returns_three_numbers() {
        let program = font(
            None,
            &[],
            &[(
                "flex",
                Cs::new()
                    .arg(0)
                    .arg(600)
                    .op(13) // hsbw
                    .othersubr(0, 1) // start the flex, at (0, 0)
                    .arg(100)
                    .arg(0)
                    .op(21)
                    .othersubr(0, 2)
                    .arg(100)
                    .arg(50)
                    .op(21)
                    .othersubr(0, 2)
                    .arg(100)
                    .arg(-50)
                    .op(21)
                    .othersubr(0, 2)
                    .arg(100)
                    .arg(0)
                    .op(21)
                    .othersubr(0, 2)
                    .arg(100)
                    .arg(80)
                    .op(21)
                    .othersubr(0, 2)
                    .arg(100)
                    .arg(0)
                    .op(21)
                    .othersubr(0, 2)
                    .othersubr(3, 0) // end the flex: three arguments, number 0
                    .escape(17) // pop
                    .escape(17) // pop
                    .escape(33) // setcurrentpoint, which reads the values back
                    .op(9) // closepath
                    .op(14) // endchar
                    .build(),
            )],
        );
        let font = Type1::parse(&program).expect("a Type 1 font");
        let outline = font.outline(0).expect("a flex");
        // One contour, two curves, and the closing edge. The seven `rmoveto`s that walked the
        // pen to the vertices emit no `Move` at all: inside a flex a mover is only the pen
        // moving, and a `Move` for each would be seven one-point contours instead of a curve.
        same(
            &outline,
            &[
                Segment::Move(0.0, 0.0),
                Segment::Curve(0.1, 0.0, 0.2, 0.05, 0.3, 0.0),
                Segment::Curve(0.4, 0.0, 0.5, 0.08, 0.6, 0.08),
                Segment::Line(0.0, 0.0),
            ],
        );
    }

    /// `callothersubr` 0 with no flex in progress is a reason, not a silently ignored
    /// operator: the charstring goes on to consume whatever the procedure would have
    /// returned, so a shape that closes is not the shape.
    #[test]
    fn a_flex_that_was_never_started_is_refused() {
        let program = font(
            None,
            &[],
            &[(
                "bad",
                Cs::new()
                    .arg(0)
                    .arg(500)
                    .op(13) // hsbw
                    .othersubr(3, 0) // end a flex that never began
                    .op(14)
                    .build(),
            )],
        );
        let font = Type1::parse(&program).expect("a Type 1 font");
        let reason = font.outline(0).expect_err("a flex with no start");
        assert!(reason.contains("flex"), "{reason}");
        assert!(reason.contains("started"), "{reason}");
    }

    /// `callothersubr` 3 asks for the stem hints to be recomputed for a rasterizer's grid,
    /// and this renderer has no grid. The *hints* are refused and the glyph is not.
    #[test]
    fn hint_replacement_is_refused_without_losing_the_glyph() {
        let code = Cs::new()
            .arg(0)
            .arg(500)
            .op(13) // hsbw
            .arg(0)
            .arg(0)
            .arg(200)
            .arg(200)
            .op(1) // hstem
            .raw(&[0b0000_0001]) // one byte of hint mask
            .arg(0)
            .arg(0)
            .op(21) // rmoveto
            .arg(200)
            .arg(0)
            .op(5) // rlineto
            .arg(0)
            .arg(200)
            .op(5)
            .arg(-200)
            .arg(0)
            .op(5)
            .arg(0)
            .arg(-200)
            .op(5)
            .op(9) // closepath
            .arg(1) // how many hints are being replaced
            .othersubr(1, 3)
            .op(14)
            .build();
        let program = font(None, &[], &[("sq", code)]);
        let font = Type1::parse(&program).expect("a Type 1 font");
        let outline = font.outline(0).expect("the glyph survives the refusal");
        assert!(!outline.is_empty(), "the square is still drawn");
        assert_eq!(
            font.advance(0),
            Ok(500),
            "and the glyph's own width is read out of the same walk"
        );
    }

    /// A `callothersubr` number with no conventional meaning is refused *with the number*,
    /// because the value the procedure returned goes on into the drawing.
    #[test]
    fn an_unknown_othersubr_is_refused_by_number() {
        for number in [4, 5, 14, 19, 27, 99] {
            let program = font(
                None,
                &[],
                &[(
                    "bad",
                    Cs::new()
                        .arg(0)
                        .arg(500)
                        .op(13) // hsbw
                        .othersubr(1, number)
                        .op(14)
                        .build(),
                )],
            );
            let font = Type1::parse(&program).expect("a Type 1 font");
            let reason = font.outline(0).expect_err("an othersubr this does not run");
            assert!(
                reason.contains(&number.to_string()),
                "the reason names {number}: {reason}"
            );
        }
    }

    // ── subroutines, and the two limits ─────────────────────────────────────────

    /// A subroutine is called by its number outright: there is no bias.
    ///
    /// Adding CFF's 107 would land on a different, in-range subroutine and run the wrong
    /// program — a plausible wrong shape rather than a failure. The subroutine here draws a
    /// line; if it were called with a bias the outline would have a different number of
    /// segments, which is what the assertion is for.
    #[test]
    fn a_subroutine_is_called_by_its_number_with_no_bias() {
        let subr = Cs::new()
            .arg(50)
            .arg(0)
            .op(5)
            .arg(-100)
            .arg(0)
            .op(5)
            .ret()
            .build();
        let program = font(
            None,
            &[subr],
            &[(
                "caller",
                Cs::new()
                    .arg(0)
                    .arg(500)
                    .op(13) // hsbw
                    .arg(0)
                    .arg(0)
                    .op(21) // rmoveto
                    .arg(0)
                    .op(10) // callsubr 0
                    .op(9) // closepath
                    .op(14)
                    .build(),
            )],
        );
        let font = Type1::parse(&program).expect("a Type 1 font");
        let outline = font.outline(0).expect("a line");
        assert_eq!(
            outline.segments.len(),
            4,
            "a move, two lines from the subroutine, and the closing line"
        );
    }

    /// Recursion past the nesting limit is a reason and never a panic.
    #[test]
    fn a_subroutine_that_calls_itself_past_the_limit_is_refused() {
        // Subroutine 0 calls subroutine 0. A font may do that, and a font that does it to
        // find the limit must get an answer rather than a crash.
        let subr = Cs::new().arg(0).op(10).op(11).build();
        let program = font(
            None,
            &[subr],
            &[(
                "deep",
                Cs::new()
                    .arg(0)
                    .arg(500)
                    .op(13) // hsbw
                    .arg(0)
                    .op(10) // callsubr 0
                    .op(14)
                    .build(),
            )],
        );
        let font = Type1::parse(&program).expect("a Type 1 font");
        let reason = font.outline(0).expect_err("a recursion with no end");
        assert!(reason.contains(&format!("{MAX_DEPTH}")), "{reason}");
    }

    /// Past the operand-stack limit is a reason and never a panic.
    #[test]
    fn more_than_the_stack_holds_is_refused() {
        let mut code = Cs::new().arg(0).arg(500).op(13);
        for _ in 0..=MAX_STACK {
            code = code.arg(1);
        }
        let code = code.op(14).build();
        let program = font(None, &[], &[("wide", code)]);
        let font = Type1::parse(&program).expect("a Type 1 font");
        let reason = font
            .outline(0)
            .expect_err("more operands than the stack holds");
        assert!(reason.contains(&format!("{MAX_STACK}")), "{reason}");
    }

    // ── things that are not Type 1 fonts ───────────────────────────────────────

    /// Bytes that are not a Type 1 font give a reason, and so does a truncated one.
    #[test]
    fn something_that_is_not_a_type_1_font_says_so() {
        assert!(Type1::parse(b"").is_err(), "no bytes at all");
        assert!(Type1::parse(b"not a font").is_err());
        // A PDF, which is what a `/FontFile` most often is when the file is damaged.
        let reason = Type1::parse(b"%PDF-1.7\n1 0 obj\n<< >>\nendobj\n%%EOF\n")
            .expect_err("a PDF is not a Type 1 font");
        assert!(reason.contains("eexec"), "{reason}");

        // A header with nothing encrypted behind it.
        let reason = Type1::parse(b"%!PS-AdobeFont-1.0: Test\n").expect_err("no encrypted half");
        assert!(reason.contains("eexec"), "{reason}");

        // The encrypted portion cut in half at every length, which is what a truncated
        // download looks like.
        let program = square();
        for cut in 1..program.len() {
            let _ = Type1::parse(&program[..cut]);
        }
        // And cut in the middle of the *second* layer, after the keyword.
        let at = program
            .windows(5)
            .position(|w| w == b"eexec")
            .expect("the keyword");
        for cut in at..program.len() {
            let _ = Type1::parse(&program[..cut]);
        }

        // A font whose cleartext decrypts to nothing usable.
        let mut empty = font_with_header(&header(""), None, SALT, &[], &[]);
        let keyword = empty.windows(5).position(|w| w == b"eexec").unwrap();
        empty.truncate(keyword.saturating_add(5).saturating_add(80));
        let reason = Type1::parse(&empty).expect_err("a font with no glyphs in it");
        assert!(reason.contains("glyphs"), "{reason}");
    }

    /// A PFB container is reported as unsupported rather than mis-parsed.
    ///
    /// A `.pfb` file is the same program wrapped in six-byte segment headers, so the bytes
    /// are not nonsense and a reader that did not check for `0x80` would go looking for an
    /// `eexec` in the middle of them. Producing outlines from a mis-parsed PFB is worse than
    /// producing none, which is why this is a refusal and not a best effort.
    #[test]
    fn a_pfb_container_is_refused_rather_than_mis_parsed() {
        let mut pfb = vec![0x80, 0x01, 0x00, 0x00];
        pfb.extend_from_slice(&square());
        let reason = Type1::parse(&pfb).expect_err("a PFB container");
        assert!(reason.contains("PFB"), "{reason}");

        // And a truncated one, which is what a damaged download of a `.pfb` looks like.
        let reason = Type1::parse(&pfb[..3]).expect_err("a truncated PFB container");
        assert!(reason.contains("PFB"), "{reason}");
    }

    // ── the header ─────────────────────────────────────────────────────────────

    /// The matrix, the `/UniqueID` and the built-in encoding all come out of the cleartext.
    #[test]
    fn the_cleartext_header_is_read() {
        let program = font_with_header(
            &header("/Encoding StandardEncoding def\n/UniqueID 5021339 def\n"),
            None,
            SALT,
            &[],
            &[
                ("A", Cs::new().arg(0).arg(667).op(13).op(14).build()),
                ("B", Cs::new().arg(0).arg(667).op(13).op(14).build()),
            ],
        );
        let font = Type1::parse(&program).expect("a Type 1 font");
        assert_eq!(font.font_matrix(), DEFAULT_FONT_MATRIX);
        assert_eq!(font.unique_id(), Some(5_021_339));
        assert_eq!(font.glyph_name(1), Some("B"));
        // The standard encoding gives code 65 the name `A`, which is glyph 0 here.
        assert_eq!(font.glyph_for_code(65), Some(0));
        assert_eq!(font.glyph_for_code(66), Some(1));
        // A code the standard encoding does not assign has no glyph, rather than glyph 0.
        assert_eq!(font.glyph_for_code(0), None);
    }

    /// A font that carries its own `/Encoding` array resolves codes through it.
    #[test]
    fn a_font_with_its_own_encoding_resolves_codes_through_it() {
        // Deliberately the other way round from the standard encoding, so an answer that
        // came from the standard-encoding table would be wrong.
        let program = font_with_header(
            &header("/Encoding 256 array\ndup 65 /B put\ndup 66 /A put\nreadonly def\n"),
            None,
            SALT,
            &[],
            &[
                ("A", Cs::new().arg(0).arg(667).op(13).op(14).build()),
                ("B", Cs::new().arg(0).arg(667).op(13).op(14).build()),
            ],
        );
        let font = Type1::parse(&program).expect("a Type 1 font");
        assert_eq!(font.glyph_for_code(65), Some(1), "code 65 is named B");
        assert_eq!(font.glyph_for_code(66), Some(0), "code 66 is named A");
        assert_eq!(font.glyph_for_code(67), None, "67 is not in the array");
    }

    /// A CID-keyed font is addressed by number, and says which collection it is for.
    #[test]
    fn a_cid_keyed_font_is_addressed_by_number() {
        let head = "%!PS-AdobeFont-1.0: TestCID 001.001\n\
                    12 dict begin\n\
                    /FontType 1 def\n\
                    /CIDFontInfo 3 dict dup begin\n\
                    /registry (Adobe) def\n\
                    /ordering (Japan1) def\n\
                    /supplement 2 def\n\
                    end def\n\
                    /CIDFontName /TestCID def\n\
                    /CIDFont 0.001 def\n\
                    /FontMatrix [0.001 0 0 0.001 0 0] def\n\
                    currentdict end\n\
                    currentfile eexec\n";
        let program = font_with_header(
            head,
            None,
            SALT,
            &[],
            &[
                ("cid00001", Cs::new().arg(0).arg(200).op(13).op(14).build()),
                ("cid00002", Cs::new().arg(0).arg(200).op(13).op(14).build()),
            ],
        );
        let font = Type1::parse(&program).expect("a CID-keyed Type 1 font");
        assert!(font.is_cid_keyed());
        assert_eq!(
            font.glyph_for_code(1),
            Some(1),
            "the identifier is the glyph number"
        );
        assert_eq!(
            font.glyph_for_code(9),
            None,
            "past the last glyph is nothing"
        );
        let cid = font.cid_info().expect("the CID font's own information");
        assert_eq!(cid.registry.as_deref(), Some("Adobe"));
        assert_eq!(cid.ordering.as_deref(), Some("Japan1"));
        assert_eq!(cid.version, Some(0.001));
    }

    /// A glyph's width is its own `hsbw`, in glyph units, scaled by the font's matrix.
    #[test]
    fn a_glyphs_width_comes_from_its_own_hsbw() {
        let program = font(
            None,
            &[],
            &[
                ("A", Cs::new().arg(20).arg(667).op(13).op(14).build()),
                ("B", Cs::new().arg(-5).arg(1000).op(13).op(14).build()),
            ],
        );
        let font = Type1::parse(&program).expect("a Type 1 font");
        assert_eq!(font.advance(0), Ok(667));
        assert_eq!(font.advance(1), Ok(1000));
        assert!(font.advance(2).is_err(), "there is no third glyph");
    }
}
