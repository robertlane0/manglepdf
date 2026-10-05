//! Image XObjects: samples, and the placement of samples on a page.
//!
//! A PDF image is a rectangle of samples plus a great deal of metadata about how those
//! samples were meant to be read: how many bits each has, what colour space they are in,
//! what range of that space they span, how they were compressed, and whether a second
//! image says which of them to keep. Getting any of it wrong does not produce an error —
//! it produces a plausible picture of the wrong thing, which is the failure mode this file
//! is most careful about.
//!
//! ## The parts that are easy to get wrong
//!
//! * **`/Decode`** is not a brightness curve. It maps the stored integer range onto the
//!   colour space's range, and an inverted one is a legitimate and common way to write a
//!   negative. Applying it as if it were nothing gives a photograph of its own negative.
//! * **Packed samples are padded per row.** A row of 1-, 2- or 4-bit samples is padded to a
//!   whole number of bytes, so the byte holding sample *n* is not `n / samples_per_byte`;
//!   it depends on the row. Getting this wrong works on the first row and produces noise
//!   on every other one.
//! * **`/ImageMask`** is a stencil, not a picture. One bit per sample, zero paints and one
//!   does not, and the colour comes from the graphics state rather than from the data.
//! * **`/SMask`** is a *separate* image whose luminance is this one's alpha. It is not
//!   blended with the colour, and it is **required to have the same `/Width` and `/Height`**
//!   as the image it masks — one mask sample per image sample. "Usually a different size" is
//!   the wrong reading and it is a dangerous one: a mask much larger than its image samples to
//!   a constant, and a constant alpha paints the picture solid, which looks like a rendering
//!   and is a lie about transparency. See [`decode_soft_mask`].
//! * **`/Interpolate`** decides what happens when the image is scaled up. Without it a
//!   photograph becomes a mosaic of whole pixels, which is what the file asked for.
//!
//! ## What is not here
//!
//! JBIG2 and JPEG 2000 have no decoder in this project yet. An image that needs one is
//! reported rather than drawn as a blank rectangle, because a page with a conspicuous hole
//! is a bug report and a page with a missing photograph is a wrong answer.
//!
//! ## Placement
//!
//! An image's space is the unit square and the transformation maps it onto the page. The
//! destination is therefore the bounding box of the transformed square's four corners, and
//! each pixel in it is mapped *back* through the inverse transformation to find which
//! sample it wants. Going backwards rather than forwards is what makes a rotated image
//! work: nothing has to be resampled into an axis-aligned buffer first, and a square at
//! forty-five degrees comes out the right shape rather than a staircase.
//!
//! ## How big an image is allowed to be
//!
//! [`MAX_IMAGE_PIXELS`] is asked of the **dictionary**, before anything is decoded, because a
//! declared size above the bound is a refusal rather than a warning — and because asking it
//! afterwards is asking it too late. The size a file *declares* is what its samples will be
//! read at, and a file that declares an enormous image can attach a filter chain to it: a
//! 408 kB `/FlateDecode` stream that inflates to 400 MB, or a JPEG whose `/SOF` marker names
//! 16000 by 16000 and makes a codec reserve 768 MB before it has read a single scan. Under an
//! image codec the dictionary may be lying, so the **header** is read on its own there and the
//! bound asked of *it*, which costs a few hundred bytes of parsing and no pixels.
//!
//! A soft mask is an image and is asked the same question, one step earlier — before its stream
//! exists — because a mask nobody can check is a claim about transparency that must be reported
//! rather than believed. See [`decode_soft_mask`].

use mangle_content::{IccBased, Matrix};
use mangle_syntax::object::{Dict, Object, Stream};
use mangle_syntax::stream::decode_stream;

use crate::fill::FillColour;
use crate::{Device, Rect};

/// The most pixels an image may have. A page can ask for an image larger than memory, and
/// the answer has to be a refusal rather than an allocation failure.
pub const MAX_IMAGE_PIXELS: usize = 64 * 1024 * 1024;

/// Does a picture of this shape exceed the bound?
///
/// Multiplication saturates rather than wrapping: a `/Width` of 2⁶⁴ next to a `/Height` of 2
/// is over the bound whatever the machine's arithmetic says, and a wrapped product could
/// come out small enough to be drawn. Every call is a *question*, so it can be asked before
/// anything has been allocated — which is the point of having it apart from the refusal
/// that follows it.
#[must_use]
fn exceeds_bound(width: usize, height: usize) -> bool {
    width.saturating_mul(height) > MAX_IMAGE_PIXELS
}

/// A decoded image, ready to place.
#[derive(Debug, Clone, PartialEq)]
pub struct Raster {
    /// The width in samples, which is what `/Width` said or, for a JPEG, what the JPEG's
    /// own header said when the two disagreed.
    pub width: usize,
    pub height: usize,
    /// RGBA, four bytes per sample, straight (not premultiplied) alpha.
    pub pixels: Vec<u8>,
    /// Whether to interpolate when scaled up, which is what `/Interpolate true` asks for
    /// and what a photograph without it would not get.
    pub interpolate: bool,
    /// A separate image whose luminance is this one's alpha.
    pub soft_mask: Option<Box<Raster>>,
    /// A colour-key: paint only where the samples fall outside this range.
    pub key_range: Option<[f64; 2]>,
    /// Whether these samples are a stencil, in which case the fill colour paints.
    pub is_stencil: bool,
}

/// How an image's stored samples become colours.
#[derive(Debug, Clone, PartialEq)]
enum Space {
    /// One component, as luminance.
    Gray { decode: [f64; 2] },
    /// Three components.
    Rgb { decode: [f64; 6] },
    /// Four components, subtractive.
    Cmyk { decode: [f64; 8] },
    /// A palette: one component indexing a table of colours.
    Indexed {
        palette: Vec<[u8; 3]>,
        /// The largest index the palette holds.
        high: usize,
        decode: [f64; 2],
    },
    /// One bit per sample, where zero paints. The colour comes from the graphics state.
    Stencil,
}

impl Space {
    /// How many stored components make up one sample in this space.
    fn components(&self) -> usize {
        match self {
            Self::Gray { .. } | Self::Indexed { .. } | Self::Stencil => 1,
            Self::Rgb { .. } => 3,
            Self::Cmyk { .. } => 4,
        }
    }

    /// The name a report would use, for a note about an image that could not be drawn.
    fn name(&self) -> &'static str {
        match self {
            Self::Gray { .. } => "DeviceGray",
            Self::Rgb { .. } => "DeviceRGB",
            Self::Cmyk { .. } => "DeviceCMYK",
            Self::Indexed { .. } => "Indexed",
            Self::Stencil => "an image mask",
        }
    }
}

/// Read one sample out of packed data.
///
/// Rows of sub-byte samples are padded to a byte boundary, so the byte holding a sample
/// depends on which row it is in as well as on its position within that row. Sixteen-bit
/// samples are two bytes, most significant first.
#[must_use]
pub fn sample_at(
    data: &[u8],
    x: usize,
    y: usize,
    width: usize,
    components: usize,
    bits: usize,
    component: usize,
) -> u16 {
    let ordinal = y
        .saturating_mul(width)
        .saturating_add(x)
        .saturating_mul(components)
        .saturating_add(component);
    match bits {
        16 => {
            let hi = data.get(ordinal.saturating_mul(2)).copied().unwrap_or(0);
            let lo = data
                .get(ordinal.saturating_mul(2).saturating_add(1))
                .copied()
                .unwrap_or(0);
            u16::from(hi) << 8 | u16::from(lo)
        }
        8 => u16::from(data.get(ordinal).copied().unwrap_or(0)),
        1 | 2 | 4 => {
            // The bit position within the *row*, and so within its byte. It has to be
            // derived from the column alone: the ordinal above already includes the row, and
            // adding `y * row_bytes` on top of that counts each row twice and makes every
            // row after the first read the wrong byte.
            let row_bits = width.saturating_mul(components).saturating_mul(bits);
            let row_bytes = row_bits.div_ceil(8);
            let within_row = x.saturating_mul(components).saturating_add(component);
            let bit = within_row.saturating_mul(bits);
            let byte_index = y.saturating_mul(row_bytes).saturating_add(bit / 8);
            let shift = 8usize.saturating_sub(bits).saturating_sub(bit % 8);
            let byte = data.get(byte_index).copied().unwrap_or(0);
            // The byte is narrowed first and the mask applied in the wider type, because the
            // two have different widths and shifting a `u8` by seven is already its top bit.
            u16::from(byte >> shift) & ((1u16 << bits) - 1)
        }
        // An unsupported depth reads as zero rather than being misread, because a file with
        // one is damaged and a note about it beats a picture of noise.
        _ => 0,
    }
}

/// Apply one component's `/Decode` pair: the stored range mapped onto 0..1.
#[must_use]
pub fn decode_component(value: f64, pair: [f64; 2]) -> f64 {
    let [lo, hi] = pair;
    if (hi - lo).abs() < f64::EPSILON {
        // A degenerate range is a damaged file; leaving the component at its minimum is
        // black rather than an arbitrary guess.
        return 0.0;
    }
    let t = (value - lo) / (hi - lo);
    t.clamp(0.0, 1.0)
}

/// The `/Decode` array for `components` components, defaulting each pair to 0..1.
#[must_use]
pub fn read_decode(object: Option<&Object>, components: usize) -> Vec<f64> {
    let mut out = vec![0.0f64; components.saturating_mul(2)];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = if i % 2 == 0 { 0.0 } else { 1.0 };
    }
    let Some(array) = object.and_then(Object::as_array) else {
        return out;
    };
    for (i, value) in array.iter().take(out.len()).enumerate() {
        if let Some(v) = value.as_f64()
            && let Some(slot) = out.get_mut(i)
        {
            *slot = v;
        }
    }
    out
}

/// Pair up a `/Decode` array, two components at a time.
fn pairs(values: &[f64], components: usize) -> Vec<[f64; 2]> {
    (0..components)
        .map(|i| {
            [
                values.get(i * 2).copied().unwrap_or(0.0),
                values.get(i * 2 + 1).copied().unwrap_or(1.0),
            ]
        })
        .collect()
}

/// Fixed-size `/Decode` arrays, so a space can carry its own.
#[must_use]
pub fn flat<const N: usize>(values: &[f64]) -> [f64; N] {
    let mut out = [0.0f64; N];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = values
            .get(i)
            .copied()
            .unwrap_or(if i % 2 == 0 { 0.0 } else { 1.0 });
    }
    out
}

/// Read an image's colour space, following an indirect reference with `resolve`.
///
/// `None` means the space cannot be read at all, which is different from reading it and
/// producing the wrong colour: Lab and Separation need an alternate space or a white point
/// that this does not have, and refusing is better than a plausible wrong answer.
fn read_space(
    object: Option<&Object>,
    resolve: &dyn Fn(&Object) -> Option<Object>,
) -> Option<Space> {
    let raw = object?.clone();
    let resolved = resolve(&raw).unwrap_or(raw);
    let Some(array) = resolved.as_array() else {
        return simple_space(resolved.as_name()?);
    };
    match array.first()?.as_name()? {
        b"ICCBased" => {
            // The same rule `Colour::to_rgba` follows, and for the same reason: `/Alternate`
            // is the file's own statement of what to do with these samples without
            // applying the profile, and no profile is applied here. It is also the answer
            // that is right where `/N` is wrong — a CMYK profile whose `/N` reads 3 is
            // telling the truth about the profile and not about the file, whereas the
            // alternate is what the producer said the data should be read as.
            let profile = array
                .get(1)
                .and_then(resolve)
                .or_else(|| array.get(1).cloned());
            let declared = IccBased::from_profile(profile.as_ref());
            if let Some(space) = declared
                .alternate
                .as_deref()
                .and_then(|name| simple_space(name.as_bytes()))
            {
                return Some(space);
            }
            // No alternate, or one this does not read. The count is still enough to read
            // the samples and show them approximately, which beats showing nothing — and
            // it is what an image does where a colour cannot: a wrong picture on the page
            // is recoverable by looking at the page, and a colour invented from a
            // component count is not.
            match declared.components {
                Some(1) => Some(Space::Gray { decode: [0.0, 1.0] }),
                Some(4) => Some(Space::Cmyk {
                    decode: [0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0],
                }),
                _ => Some(Space::Rgb {
                    decode: [0.0, 1.0, 0.0, 1.0, 0.0, 1.0],
                }),
            }
        }
        b"Indexed" | b"I" => {
            // `[/Indexed base hival lookup]`, where the lookup is a string of three bytes
            // per entry or a stream holding the same.
            let high = array.get(2).and_then(Object::as_i64).unwrap_or(0).max(0) as usize;
            let bytes: Vec<u8> = match array.get(3).and_then(resolve) {
                Some(Object::String(s)) => s,
                Some(Object::Stream(s)) => decode_stream(&s).data,
                _ => Vec::new(),
            };
            let count = high.saturating_add(1).min(MAX_IMAGE_PIXELS);
            let mut palette = Vec::with_capacity(count);
            for i in 0..count {
                let at = i.saturating_mul(3);
                palette.push([
                    bytes.get(at).copied().unwrap_or(0),
                    bytes.get(at.saturating_add(1)).copied().unwrap_or(0),
                    bytes.get(at.saturating_add(2)).copied().unwrap_or(0),
                ]);
            }
            Some(Space::Indexed {
                palette,
                high,
                // The default `/Decode` for an indexed image maps the *index* rather than a
                // colour, so its range is 0 to hival and not 0 to 1. With one entry in the
                // palette a stored byte of 255 is index 1 and a byte of 1 is index 0.
                decode: [0.0, high as f64],
            })
        }
        b"CalRGB" => Some(Space::Rgb {
            decode: [0.0, 1.0, 0.0, 1.0, 0.0, 1.0],
        }),
        b"CalGray" => Some(Space::Gray { decode: [0.0, 1.0] }),
        other => simple_space(other),
    }
}

/// The device spaces, which need no parameters.
fn simple_space(name: &[u8]) -> Option<Space> {
    match name {
        b"DeviceGray" | b"G" | b"CalGray" => Some(Space::Gray { decode: [0.0, 1.0] }),
        b"DeviceRGB" | b"RGB" | b"CalRGB" => Some(Space::Rgb {
            decode: [0.0, 1.0, 0.0, 1.0, 0.0, 1.0],
        }),
        b"DeviceCMYK" | b"CMYK" => Some(Space::Cmyk {
            decode: [0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0],
        }),
        _ => None,
    }
}

/// Fold a `/Decode` array into the space it belongs to.
fn apply_decode(space: Space, values: &[f64]) -> Space {
    match space {
        Space::Gray { .. } => Space::Gray {
            decode: flat::<2>(values),
        },
        Space::Rgb { .. } => Space::Rgb {
            decode: flat::<6>(values),
        },
        Space::Cmyk { .. } => Space::Cmyk {
            decode: flat::<8>(values),
        },
        Space::Indexed { palette, high, .. } => Space::Indexed {
            palette,
            high,
            decode: flat::<2>(values),
        },
        other @ Space::Stencil => other,
    }
}

/// Where an image's samples come from, once the outer filters have been undone.
enum Source {
    /// Already-decoded samples, with the two facts a consumer needs kept apart.
    Samples { data: Vec<u8>, range: SampleRange },
}

/// How many bits a sample occupies, and how much it is worth.
///
/// These are two different questions and a codec can make them disagree, which is why
/// they are kept apart rather than collapsed into one "bits per sample" number:
///
/// * **layout** is how many bits of each byte a sample takes, so it is what says which
///   byte a pixel lives in. A codec that expands runs knows this: a fax decoder writes
///   one byte per pixel whatever the stream said, and its output has to be read as eight
///   bits per sample or eight pixels come out of every byte.
/// * **value** is what a sample is worth, so it is what a sample is divided by to become
///   a colour. That is `/BitsPerComponent`, because it is the file's statement of the
///   range its samples span, and a decoder has no opinion about it.
///
/// Reading a fax decoder's output as both is what the two halves of the failure were.
/// Read as the declared one bit per sample it is a barcode — the image smears sideways
/// by a factor of eight. Read as eight bits and divided by 255 it is a black page — a
/// fax scan is mostly white, its white sample is the byte 1, and 1/255 is black. Layout
/// has to come from the decoder and value from the dictionary, and neither can stand in
/// for the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SampleRange {
    layout: usize,
    value: usize,
}

impl SampleRange {
    /// What a stream no codec expanded needs: the same answer to both questions.
    fn declared(declared_bits: usize) -> Self {
        Self {
            layout: declared_bits,
            value: declared_bits,
        }
    }

    /// What a codec that wrote one byte per sample needs: laid out as eight bits, and
    /// still worth whatever the dictionary said.
    fn expanded(declared_bits: usize) -> Self {
        Self {
            layout: 8,
            value: declared_bits,
        }
    }

    /// A stencil's sample *is* a bit — zero paints, one does not — whatever its
    /// dictionary says, so both answers are one unless a codec widened the layout.
    fn stencil(expanded: bool) -> Self {
        Self {
            layout: if expanded { 8 } else { 1 },
            value: 1,
        }
    }
}

/// What a stream is being read as.
///
/// A soft mask is an image with one obligation a picture does not have: it has to be whole.
/// The part of a mask that is missing becomes an alpha nothing can be drawn with, and a
/// renderer that fills the gap with zero alpha does not draw a damaged picture, it hides
/// content — which is the one answer a note exists to rule out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    /// A picture, drawn for its own sake.
    Picture,
    /// An `/SMask`: the alpha of another image.
    SoftMask,
}

/// Decode an image XObject into samples ready to place.
///
/// `None` means the image could not be decoded at all, which the caller reports; a note is
/// added for every reason, so a page that loses an image says why.
#[must_use]
pub fn decode(
    stream: &Stream,
    resolve: &dyn Fn(&Object) -> Option<Object>,
    notes: &mut Vec<String>,
) -> Option<Raster> {
    decode_role(stream, resolve, notes, Role::Picture)
}

/// Decode an `/SMask`: the separate image whose luminance is another one's alpha.
///
/// A mask is required to have the **same** width and height as the image it masks, and both
/// facts are checked here — before the mask's stream is decoded at all, so a file cannot name
/// a mask of any size it likes and have us allocate for it.
///
/// Four things are refused, each with its own note, because they are four different faults
/// and a page that loses its transparency deserves to be told which one it was:
///
/// * a mask above [`MAX_IMAGE_PIXELS`], which is the one that is about memory rather than
///   about meaning — this file's mask is 34862 by 4332 pixels, and asking its filter chain for
///   the bytes of a mask that size is how a page from the internet asks for a quarter of a
///   gigabyte;
/// * a mask whose size disagrees with the image's, which is damaged or hostile and cannot be
///   sampled per image sample at all;
/// * a mask that decoded short, whose missing samples would become an invented alpha;
/// * and a mask carrying a `/SMask` of its own, which is not a thing and is where an unbounded
///   recursion would start.
///
/// Every one of them leaves the image drawn with its own colours at full alpha. The colours
/// are real and worth showing; the mask is a claim about transparency that could not be
/// checked, and it is reported rather than believed.
#[must_use]
pub fn decode_soft_mask(
    stream: &Stream,
    image_width: usize,
    image_height: usize,
    resolve: &dyn Fn(&Object) -> Option<Object>,
    notes: &mut Vec<String>,
) -> Option<Raster> {
    let declared = |key: &str| -> usize {
        stream
            .dict
            .get(key)
            .and_then(Object::as_i64)
            .unwrap_or(0)
            .max(0) as usize
    };
    let (mask_w, mask_h) = (declared("Width"), declared("Height"));
    // A mask that says nothing about its own size is left to `decode_role`, which refuses it
    // with the note it uses for every picture with no size.
    if mask_w > 0 && mask_h > 0 {
        if exceeds_bound(mask_w, mask_h) {
            notes.push(format!(
                "a soft mask of {mask_w} by {mask_h} pixels is above the {MAX_IMAGE_PIXELS} \
                 this draws, and was not decoded"
            ));
            return None;
        }
        if mask_w != image_width || mask_h != image_height {
            notes.push(format!(
                "a soft mask of {mask_w} by {mask_h} pixels does not match the {image_width} by \
                 {image_height} image it masks, and was not used"
            ));
            return None;
        }
    }
    decode_role(stream, resolve, notes, Role::SoftMask)
}

fn decode_role(
    stream: &Stream,
    resolve: &dyn Fn(&Object) -> Option<Object>,
    notes: &mut Vec<String>,
    role: Role,
) -> Option<Raster> {
    let Some(raster) = decode_samples(stream, resolve, notes, role) else {
        // Whatever the refusal was, the image still has an `/SMask` and a `/Mask` that
        // were never looked at, and saying so is the difference between a gap we can see
        // and a gap we cannot.
        note_skipped(&stream.dict, resolve, role, notes);
        return None;
    };
    Some(raster)
}

/// Report the parts of an image that a refusal left unexamined.
///
/// An image is refused in about a dozen ways, and nearly all of them happen before the
/// `/SMask` and `/Mask` keys are read — a codec this cannot read returns about a hundred
/// lines above them. So a picture whose samples are JPEG 2000 and whose `/Mask` is JBIG2
/// says only that the JPEG 2000 was not decoded: the mask is not mentioned, and a page
/// missing two things reports one.
///
/// Every one of those refusals reports what else was skipped, so the note counts what the
/// page lost rather than where the reading stopped. Naming the codec of the skipped part
/// is the same thing one level down: a `/Mask` whose own filter is unreadable is two gaps,
/// and saying so is what tells a reader whether the JPEG 2000 work will fix this page.
fn note_skipped(
    dict: &Dict,
    resolve: &dyn Fn(&Object) -> Option<Object>,
    role: Role,
    notes: &mut Vec<String>,
) {
    // A mask under a mask is not a thing this follows, and it says so in its own words
    // wherever it is met. Here it is only ever a fact about what was skipped.
    if role == Role::SoftMask && dict.get("SMask").is_some() {
        notes.push("a soft mask carries a soft mask of its own, which is not read".to_owned());
    } else if dict.get("SMask").is_some() {
        notes.push(
            "the image's `/SMask` was not read either, because the image it belongs to was not \
             decoded"
                .into(),
        );
    }
    let mask = dict
        .get("Mask")
        .and_then(resolve)
        .or_else(|| dict.get("Mask").cloned());
    let Some(mask) = mask else {
        return;
    };
    let Object::Stream(mask) = mask else {
        // A `/Mask` that is not an image is nothing this can say anything useful about:
        // the shape of the key is itself the finding, and the report below stands.
        notes.push(
            "the image's `/Mask` is not an image this can read, and was not read because the \
             image it masks was not decoded"
                .into(),
        );
        return;
    };
    let codec = codec_name(&mask);
    notes.push(match codec {
        Some(codec) => format!(
            "the image's `/Mask` is a {codec} image and no decoder exists for one either, so \
             it was not read"
        ),
        None => "the image's `/Mask` was not read either, because the image it masks was not \
                 decoded"
            .into(),
    });
}

/// The image codec a stream is under, named as a reader would name it.
///
/// Only the two that are not decoded here: a `/Mask` under `/FlateDecode` is not a second
/// gap, and calling it one would bury the first.
fn codec_name(stream: &Stream) -> Option<&'static str> {
    stream.filters().iter().rev().find_map(|f| match &**f {
        b"JPXDecode" => Some("JPEG 2000"),
        b"JBIG2Decode" => Some("JBIG2"),
        _ => None,
    })
}

/// The body of [`decode_role`], which refuses without saying what else it skipped.
fn decode_samples(
    stream: &Stream,
    resolve: &dyn Fn(&Object) -> Option<Object>,
    notes: &mut Vec<String>,
    role: Role,
) -> Option<Raster> {
    let dict: &Dict = &stream.dict;
    let declared_w = dict.get("Width").and_then(Object::as_i64).unwrap_or(0);
    let declared_h = dict.get("Height").and_then(Object::as_i64).unwrap_or(0);
    if declared_w <= 0 || declared_h <= 0 {
        notes.push(format!(
            "an image claims to be {declared_w} by {declared_h} pixels and was not drawn"
        ));
        return None;
    }

    let is_stencil = dict
        .get("ImageMask")
        .and_then(Object::as_bool)
        .unwrap_or(false);
    let declared_bits = dict
        .get("BitsPerComponent")
        .and_then(Object::as_i64)
        .unwrap_or(8)
        .max(0) as usize;

    // Read off the dictionary, which costs nothing, and asked before anything is decoded: a
    // file's declared size is what its samples are read at, so a size above the bound is a
    // refusal and not a warning. The exception is a stream under an image codec, where the
    // codec's own header carries the size and the dictionary may be lying — there the header
    // is asked instead, below, and before the codec has allocated anything either.
    let terminal = stream
        .filters()
        .iter()
        .rev()
        .find(|f| matches!(**f, b"DCTDecode" | b"DCT" | b"JPXDecode" | b"JBIG2Decode"))
        .copied();
    if terminal.is_none() && exceeds_bound(declared_w.max(0) as usize, declared_h.max(0) as usize) {
        notes.push(format!(
            "an image of {declared_w} by {declared_h} pixels is above the {MAX_IMAGE_PIXELS} \
             this draws and was not drawn"
        ));
        return None;
    }

    // The filters below an image codec are deliberately left alone, so what comes back is
    // either samples or the bytes the codec itself wants.
    let decoded = decode_stream(stream);
    for note in &decoded.notes {
        notes.push(format!("image: {note}"));
    }

    let (source, space, width, height) = match terminal {
        Some(b"DCTDecode" | b"DCT") => {
            let (pixels, jw, jh) = jpeg(&decoded.data, notes)?;
            let space = Space::Rgb {
                decode: [0.0, 1.0, 0.0, 1.0, 0.0, 1.0],
            };
            let (w, h) = (jw, jh);
            // A JPEG's own header is authoritative: the two disagree often enough that
            // trusting the dictionary produces a torn image rather than a slightly wrong
            // one. The discrepancy is worth saying out loud.
            let dw = declared_w.max(0) as usize;
            let dh = declared_h.max(0) as usize;
            if dw != 0 && dh != 0 && (dw != w || dh != h) {
                notes.push(format!(
                    "a JPEG image is {w} by {h} pixels but the dictionary says {dw} by {dh}; \
                     the JPEG's own size was used"
                ));
            }
            (
                Source::Samples {
                    data: pixels,
                    range: SampleRange::declared(8),
                },
                space,
                w,
                h,
            )
        }
        Some(b"JPXDecode") => {
            notes.push("a JPEG 2000 image was found but no decoder exists yet".into());
            return None;
        }
        Some(b"JBIG2Decode") => {
            notes.push("a JBIG2 image was found but no decoder exists yet".into());
            return None;
        }
        _ => {
            let space = if is_stencil {
                Space::Stencil
            } else {
                match read_space(dict.get("ColorSpace"), resolve) {
                    Some(s) => s,
                    None => {
                        notes.push(
                            "an image is in a colour space this cannot read and was not drawn"
                                .into(),
                        );
                        return None;
                    }
                }
            };
            let range = if decoded.one_byte_per_sample {
                if is_stencil {
                    SampleRange::stencil(true)
                } else {
                    SampleRange::expanded(declared_bits)
                }
            } else if is_stencil {
                SampleRange::stencil(false)
            } else {
                SampleRange::declared(declared_bits)
            };
            let w = declared_w as usize;
            let h = declared_h as usize;
            (
                Source::Samples {
                    data: decoded.data,
                    range,
                },
                space,
                w,
                h,
            )
        }
    };

    let (samples, range) = match source {
        Source::Samples { data, range } => (data, range),
    };

    // The bound, asked again for the size that is actually about to be allocated. The check
    // above it is the one that matters — it runs before a single byte is allocated — and this
    // one is here because a codec's header can name a size the dictionary did not, so the
    // answer to "how big is this" is not yet final at the earlier point.
    if exceeds_bound(width, height) {
        notes.push(format!(
            "an image of {width} by {height} pixels is above the {MAX_IMAGE_PIXELS} this \
             draws and was not drawn"
        ));
        return None;
    }
    // Both numbers are checked because they can come apart, and `/BitsPerComponent 7`
    // under a codec that widened the layout is laid out as eight and worth seven.
    if !matches!(range.layout, 1 | 2 | 4 | 8 | 16) {
        notes.push(format!(
            "an image lays its samples out {} bits at a time, which is not a thing, and \
             was not drawn",
            range.layout
        ));
        return None;
    }
    if !matches!(range.value, 1 | 2 | 4 | 8 | 16) {
        notes.push(format!(
            "an image claims {} bits per component, which is not a thing, and was not drawn",
            range.value
        ));
        return None;
    }

    let components = space.components();
    let space = apply_decode(space, &read_decode(dict.get("Decode"), components));

    // A short stream is a fact about the mask's *alpha* rather than about its colour, so it
    // is checked here and only here. `sample_at` past the end of the buffer answers zero, and
    // for a picture zero is a colour the file may well have meant; for a mask it is a hole in
    // the transparency, and drawing the picture as though that hole were transparent is not a
    // rendering of the file — it is the removal of part of it, done quietly.
    if role == Role::SoftMask {
        let wanted = sample_bytes(width, height, components, range);
        if samples.len() < wanted {
            notes.push(format!(
                "a soft mask holds {} bytes of the {wanted} its {width} by {height} pixels \
                 need, and was not used",
                samples.len()
            ));
            return None;
        }
    }

    let pixels = to_rgba(&samples, &space, width, height, range, components);
    let _ = space.name();

    // A mask's own `/SMask` is not followed. A soft mask's luminance *is* its alpha, so a mask
    // under a mask is a claim the specification does not make, and following it would make the
    // depth of a chain of them the only thing standing between a file and the stack: each link
    // is a legal image reference, and a few hundred of them is a few hundred nested calls. It
    // is reported rather than dropped silently, because a file that writes one is telling us
    // something about itself.
    let soft_mask = if role == Role::SoftMask {
        if dict.get("SMask").is_some() {
            notes.push("a soft mask carries a soft mask of its own, which is not read".to_owned());
        }
        None
    } else {
        dict.get("SMask")
            .and_then(resolve)
            .and_then(|o| match o {
                Object::Stream(s) => Some(s),
                _ => None,
            })
            .and_then(|s| decode_soft_mask(&s, width, height, resolve, notes))
            .map(Box::new)
    };

    let key_range = dict
        .get("Mask")
        .filter(|o| !matches!(o, Object::Ref(_)))
        .and_then(Object::as_array)
        .filter(|a| a.len() >= 2)
        .map(|a| {
            [
                a.first().and_then(Object::as_f64).unwrap_or(0.0),
                a.get(1).and_then(Object::as_f64).unwrap_or(1.0),
            ]
        });

    Some(Raster {
        width,
        height,
        pixels,
        interpolate: dict
            .get("Interpolate")
            .and_then(Object::as_bool)
            .unwrap_or(false),
        soft_mask,
        key_range,
        is_stencil,
    })
}

/// How many bytes a picture of this shape needs, packed the way its samples are.
///
/// A codec that wrote one byte per sample made that width for us, so the layout is 8 bits
/// whatever the dictionary said. Sub-byte layouts are padded to a byte per *row*, which is
/// the same rule `sample_at` reads by and the reason this cannot be `width × height × …`.
fn sample_bytes(width: usize, height: usize, components: usize, range: SampleRange) -> usize {
    let per_row = width
        .saturating_mul(components)
        .saturating_mul(range.layout)
        .div_ceil(8);
    per_row.saturating_mul(height)
}

/// Decode a JPEG's samples into RGB.
///
/// The bound is asked of the header, before the decode, and that is the whole reason this is
/// not one call: `decode` allocates `width × height × 3` up front from a `/SOF` marker, so a
/// file is a request for that much memory with a header and nothing behind it. The header is
/// read on its own first, which costs a few hundred bytes of parsing and no pixels, and a
/// picture above the bound is refused there.
fn jpeg(data: &[u8], notes: &mut Vec<String>) -> Option<(Vec<u8>, usize, usize)> {
    use zune_jpeg::JpegDecoder;
    use zune_jpeg::zune_core::bytestream::ZCursor;

    let mut decoder = JpegDecoder::new(ZCursor::new(data));
    if decoder.decode_headers().is_ok()
        && let Some(info) = decoder.info()
    {
        let (w, h) = (usize::from(info.width), usize::from(info.height));
        if exceeds_bound(w, h) {
            notes.push(format!(
                "a JPEG image is {w} by {h} pixels, above the {MAX_IMAGE_PIXELS} this draws, \
                 and was not decoded"
            ));
            return None;
        }
    }
    let pixels = match decoder.decode() {
        Ok(p) => p,
        Err(e) => {
            notes.push(format!("a JPEG image could not be decoded: {e}"));
            return None;
        }
    };
    let info = decoder.info()?;
    Some((pixels, usize::from(info.width), usize::from(info.height)))
}

/// Turn packed samples into RGBA.
///
/// `range` answers the two questions separately: `layout` says which byte a sample is
/// in, and `value` says what that sample is worth. A stencil asks neither — see below.
fn to_rgba(
    samples: &[u8],
    space: &Space,
    width: usize,
    height: usize,
    range: SampleRange,
    components: usize,
) -> Vec<u8> {
    let max = f64::from((1u32 << range.value.min(16)) - 1);
    let mut out = Vec::with_capacity(width.saturating_mul(height).saturating_mul(4));
    for y in 0..height {
        for x in 0..width {
            let raw = |c: usize| -> f64 {
                let v = sample_at(samples, x, y, width, components, range.layout, c);
                if max > 0.0 { f64::from(v) / max } else { 0.0 }
            };
            let (r, g, b, a): (f64, f64, f64, f64) = match space {
                Space::Stencil => {
                    // Zero paints and one does not, which is a stencil and not a picture.
                    // The test is on the *stored bit*, not on the value it normalises to,
                    // because a stencil's sample is a bit however many bits of byte the
                    // codec that expanded it chose to write that bit into: a fax decoder
                    // writes the byte 1 for "do not paint", and comparing 1/255 against
                    // one half decides that byte 0 paints and byte 1 paints too.
                    let painted = sample_at(samples, x, y, width, components, range.layout, 0) == 0;
                    if painted {
                        (0.0, 0.0, 0.0, 1.0)
                    } else {
                        (0.0, 0.0, 0.0, 0.0)
                    }
                }
                Space::Gray { decode } => {
                    let v = decode_component(raw(0), *decode);
                    (v, v, v, 1.0)
                }
                Space::Rgb { decode } => {
                    let d = pairs(decode, 3);
                    let pair = |i: usize| d.get(i).copied().unwrap_or([0.0, 1.0]);
                    (
                        decode_component(raw(0), pair(0)),
                        decode_component(raw(1), pair(1)),
                        decode_component(raw(2), pair(2)),
                        1.0,
                    )
                }
                Space::Cmyk { decode } => {
                    let d = pairs(decode, 4);
                    let pair = |i: usize| d.get(i).copied().unwrap_or([0.0, 1.0]);
                    // Subtractive: each component removes light, and black removes all of it.
                    let (c, m, y, k) = (
                        decode_component(raw(0), pair(0)),
                        decode_component(raw(1), pair(1)),
                        decode_component(raw(2), pair(2)),
                        decode_component(raw(3), pair(3)),
                    );
                    (
                        (1.0 - c) * (1.0 - k),
                        (1.0 - m) * (1.0 - k),
                        (1.0 - y) * (1.0 - k),
                        1.0,
                    )
                }
                Space::Indexed {
                    palette,
                    high,
                    decode,
                } => {
                    let t = decode_component(raw(0), *decode);
                    let last = f64::from(u32::try_from(*high).unwrap_or(0));
                    let index = (t * last).round();
                    if index < 0.0 || index > last {
                        // Outside the palette is a damaged index; black is the documented
                        // outcome and is better than an arbitrary wrap.
                        (0.0, 0.0, 0.0, 1.0)
                    } else {
                        match palette.get(index as usize) {
                            Some(entry) => {
                                let c = |i: usize| {
                                    f64::from(entry.get(i).copied().unwrap_or(0)) / 255.0
                                };
                                (c(0), c(1), c(2), 1.0)
                            }
                            None => (0.0, 0.0, 0.0, 1.0),
                        }
                    }
                }
            };
            out.push((r.clamp(0.0, 1.0) * 255.0).round() as u8);
            out.push((g.clamp(0.0, 1.0) * 255.0).round() as u8);
            out.push((b.clamp(0.0, 1.0) * 255.0).round() as u8);
            out.push((a.clamp(0.0, 1.0) * 255.0).round() as u8);
        }
    }
    out
}

impl Raster {
    /// One sample at fractional coordinates, both in 0..1.
    ///
    /// Nearest or bilinear: bilinear when the file asked to interpolate, and otherwise the
    /// whole pixel that contains the point, which is what a file without `/Interpolate`
    /// means and what a photograph upscaled without it looks like.
    #[must_use]
    pub fn sample(&self, u: f64, v: f64) -> [u8; 4] {
        if self.width == 0 || self.height == 0 {
            return [0, 0, 0, 0];
        }
        let (u, v) = (u.clamp(0.0, 1.0), v.clamp(0.0, 1.0));
        if !self.interpolate {
            let x = (u * self.width as f64).floor().max(0.0) as usize;
            let y = (v * self.height as f64).floor().max(0.0) as usize;
            return self.at(x, y);
        }
        // The sample grid sits at pixel centres, so the first and last pixels are half a
        // pixel from the edge rather than on it.
        let fx = u * self.width as f64 - 0.5;
        let fy = v * self.height as f64 - 0.5;
        let x0 = fx.floor();
        let y0 = fy.floor();
        let tx = fx - x0;
        let ty = fy - y0;
        let ix = |v: f64| (v.max(0.0) as usize).min(self.width - 1);
        let iy = |v: f64| (v.max(0.0) as usize).min(self.height - 1);
        let (x0i, y0i) = (ix(x0), iy(y0));
        let (x1i, y1i) = (ix(x0 + 1.0), iy(y0 + 1.0));
        let p00 = self.at(x0i, y0i);
        let p10 = self.at(x1i, y0i);
        let p01 = self.at(x0i, y1i);
        let p11 = self.at(x1i, y1i);
        let mut out = [0u8; 4];
        // Zipping the four samples keeps the channels together; an index into a four-element
        // array is an off-by-one waiting to happen.
        for (slot, (((a, b), c), d)) in out
            .iter_mut()
            .zip(p00.iter().zip(p10.iter()).zip(p01.iter()).zip(p11.iter()))
        {
            let top = f64::from(*a) * (1.0 - tx) + f64::from(*b) * tx;
            let bottom = f64::from(*c) * (1.0 - tx) + f64::from(*d) * tx;
            *slot = (top * (1.0 - ty) + bottom * ty).clamp(0.0, 255.0).round() as u8;
        }
        out
    }

    /// One sample by pixel index, clamped to the image.
    #[must_use]
    pub fn at(&self, x: usize, y: usize) -> [u8; 4] {
        let x = x.min(self.width.saturating_sub(1));
        let y = y.min(self.height.saturating_sub(1));
        let at = y
            .saturating_mul(self.width)
            .saturating_add(x)
            .saturating_mul(4);
        [
            self.pixels.get(at).copied().unwrap_or(0),
            self.pixels.get(at.saturating_add(1)).copied().unwrap_or(0),
            self.pixels.get(at.saturating_add(2)).copied().unwrap_or(0),
            self.pixels
                .get(at.saturating_add(3))
                .copied()
                .unwrap_or(255),
        ]
    }

    /// The alpha the soft mask gives this fractional sample, or 1.0.
    fn mask_alpha(&self, u: f64, v: f64) -> f64 {
        let Some(mask) = &self.soft_mask else {
            return 1.0;
        };
        let [r, g, b, _] = mask.sample(u, v);
        // A soft mask's luminance is its alpha. A mask in DeviceGray has equal channels, so
        // a weighted sum and the single channel agree; the sum is right for the rest.
        (f64::from(r) * 0.2126 + f64::from(g) * 0.7152 + f64::from(b) * 0.0722) / 255.0
    }
}

/// Draw an image through a transformation.
///
/// `matrix` maps the unit square onto the device. `fill` is the graphics state's fill colour,
/// which is what paints an image mask's *zero* bits; `None` means there is no colour to paint
/// in, which for a stencil draws nothing rather than guessing one — an image that is not a
/// stencil paints its own samples and ignores it. Returns whether anything was drawn.
///
/// A stencil's colour is a [`FillColour`] rather than a plain `Rgba` because a mask painted in
/// a pattern takes a different colour at every pixel: `scn` with a `/Pattern` colour space
/// names a pattern, and a shading pattern's colour varies with where the mask lands. The
/// pattern is evaluated at each painted pixel, through the same evaluator `sh` uses.
///
/// Every write is at the device coordinate it was computed for, and every sample is read
/// through the transformation with its vertical axis the way an image's is: row zero at the
/// top. Both are stated because both were once wrong, and each put an image in the wrong
/// place on its own — see D5b in `docs/known-diffs.md`.
pub fn draw(
    device: &mut Device,
    raster: &Raster,
    matrix: &Matrix,
    alpha: f64,
    fill: Option<&FillColour>,
) -> bool {
    let corners = [
        matrix.apply(0.0, 0.0),
        matrix.apply(1.0, 0.0),
        matrix.apply(1.0, 1.0),
        matrix.apply(0.0, 1.0),
    ];
    let min_x = corners.iter().map(|c| c.0).fold(f64::INFINITY, f64::min);
    let max_x = corners
        .iter()
        .map(|c| c.0)
        .fold(f64::NEG_INFINITY, f64::max);
    let min_y = corners.iter().map(|c| c.1).fold(f64::INFINITY, f64::min);
    let max_y = corners
        .iter()
        .map(|c| c.1)
        .fold(f64::NEG_INFINITY, f64::max);
    let bounds = Rect {
        x0: min_x,
        y0: min_y,
        x1: max_x,
        y1: max_y,
    };
    let area = bounds.intersect(device.clip());
    let Some((columns, rows)) = area.pixels() else {
        return false;
    };
    // Each pixel is mapped back through the inverse to find its sample. Going backwards
    // rather than forwards is what makes a rotated image come out the right shape.
    //
    // The vertical axis is flipped in the mapping rather than in the buffer, because the two
    // are the same answer only until the image is turned. A raster's row zero is its *top*
    // row, and an image's unit square puts row zero at `v = 1` — the matrix that reaches
    // here has already turned the page's y axis over, because a canvas counts down and a
    // page counts up, so `v = 0` is the placement's bottom edge and its *last* raster row.
    // Reading `v` straight off the inverse therefore draws every image upside down. Folding
    // the flip into the transformation is what puts `v = 0` at the placement's top, and it
    // carries a quarter turn with it: the image's top edge ends up on the left or the right
    // according to the direction of the turn, where a flipped buffer would put it at the
    // bottom in both.
    let to_image = matrix.concat(Matrix::new(1.0, 0.0, 0.0, -1.0, 0.0, 1.0));
    let Some(inverse) = to_image.inverse() else {
        return false;
    };

    let mut drawn = 0usize;
    // A stencil's colour is asked per painted pixel, so a pattern is prepared once here
    // rather than rebuilt at every bit the mask paints.
    let stencil = if raster.is_stencil {
        match fill.and_then(FillColour::sampler) {
            Some(sampler) => Some(sampler),
            // No colour to paint in — a colour in a space this cannot convert, or a pattern
            // whose transformation collapses. Nothing is drawn rather than something
            // arbitrary, and the caller has already said why.
            None => return false,
        }
    } else {
        None
    };
    // `rows` is consumed by the outer loop and `columns` by the inner one, and a range is
    // not `Copy`; the clone is two words and is clearer than restructuring the walk.
    for y in rows.clone() {
        for x in columns.clone() {
            let (u, v) = inverse.apply(x as f64 + 0.5, y as f64 + 0.5);
            if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
                continue;
            }
            let mut px = raster.sample(u, v);
            // A stencil's painting bits take the graphics state's colour and its other bits
            // leave the paper alone. `to_rgba` gave the first an alpha of 255 and the second
            // an alpha of 0, so the alpha is what says which is which — painting the bits
            // *without* colour turns a mask into a solid block, which is the opposite of what
            // a stencil is for.
            if let Some(sampler) = stencil.as_ref() {
                if px[3] == 0 {
                    continue;
                }
                // A pattern that does not reach this pixel leaves the paper there, which is
                // the same answer a path fill gives a gradient that stops short.
                let Some(colour) = sampler.at(x, y, alpha) else {
                    continue;
                };
                px = colour;
            }
            // The soft mask multiplies whatever alpha the samples had.
            let masked = raster.mask_alpha(u, v);
            if let Some(a) = px.get_mut(3) {
                *a = (f64::from(*a) * masked).clamp(0.0, 255.0).round() as u8;
            }
            // And the clip does the same, for the same reason and with the same result at
            // the edge: a clip that is a region antialiases rather than steps.
            if let Some(share) = device.clip_coverage(x, y) {
                if let Some(a) = px.get_mut(3) {
                    *a = (f64::from(*a) * share).clamp(0.0, 255.0).round() as u8;
                }
            }
            if px.get(3).copied().unwrap_or(0) == 0 {
                continue;
            }
            // The device position is the one the pixel was computed for. `area` is in
            // absolute device coordinates, so there is nothing to subtract: the clipped
            // area's origin is where the placement happens to start, not where the device
            // starts, and subtracting it drew every image not at the page origin at the
            // page origin.
            device.put(x, y, px);
            drawn += 1;
        }
    }
    drawn > 0
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
    use mangle_content::Rgba;

    fn grey_image(w: usize, h: usize, data: Vec<u8>, bits: usize) -> Stream {
        let mut dict = Dict::new();
        dict.set("Width", Object::Int(w as i64));
        dict.set("Height", Object::Int(h as i64));
        dict.set("BitsPerComponent", Object::Int(bits as i64));
        dict.set("ColorSpace", Object::name("DeviceGray"));
        Stream::new(dict, data)
    }

    fn rgb_image(w: usize, h: usize, data: Vec<u8>) -> Stream {
        let mut dict = Dict::new();
        dict.set("Width", Object::Int(w as i64));
        dict.set("Height", Object::Int(h as i64));
        dict.set("BitsPerComponent", Object::Int(8));
        dict.set("ColorSpace", Object::name("DeviceRGB"));
        Stream::new(dict, data)
    }

    /// A CCITT Group 3 fax image, one row of which is `bits`.
    ///
    /// Group 3 1D rather than Group 4 because its codes are written out by hand in the tests
    /// below and there is nothing to generate them. `/K 0` is what selects it; `/Columns` is
    /// what the runs are counted against, and the sample width follows it.
    fn fax_image(w: usize, bits: &str) -> Stream {
        let mut dict = Dict::new();
        dict.set("Width", Object::Int(w as i64));
        dict.set("Height", Object::Int(1));
        // A fax stream's encoded depth is one bit, whatever its decoded samples look like.
        dict.set("BitsPerComponent", Object::Int(1));
        dict.set("ColorSpace", Object::name("DeviceGray"));
        dict.set("Filter", Object::name("CCITTFaxDecode"));
        dict.set(
            "DecodeParms",
            Object::Dict({
                let mut p = Dict::new();
                p.set("K", Object::Int(0));
                p.set("Columns", Object::Int(w as i64));
                p.set("Rows", Object::Int(1));
                p
            }),
        );
        Stream::new(dict, pack(bits))
    }

    /// A one-byte-per-pixel `/ImageMask` whose row is a fax stream of `bits`.
    ///
    /// `/BlackIs1 true` so the decoder's samples come through unchanged — a zero stays a
    /// zero — which is what makes the mask's two kinds of bit distinguishable. `/K 0` is
    /// Group 3 1D, whose codes are written out by hand in the tests that use this.
    fn fax_mask(w: usize, bits: String) -> Stream {
        let mut dict = Dict::new();
        dict.set("Width", Object::Int(w as i64));
        dict.set("Height", Object::Int(1));
        dict.set("ImageMask", Object::Bool(true));
        dict.set("BitsPerComponent", Object::Int(1));
        dict.set("Filter", Object::name("CCITTFaxDecode"));
        dict.set(
            "DecodeParms",
            Object::Dict({
                let mut p = Dict::new();
                p.set("K", Object::Int(0));
                p.set("BlackIs1", Object::Bool(true));
                p.set("Columns", Object::Int(w as i64));
                p.set("Rows", Object::Int(1));
                p
            }),
        );
        Stream::new(dict, pack(&bits))
    }

    /// An `/SMask` of this size in `DeviceGray`, carrying `data`.
    ///
    /// The point of the fixture is that `/Width` and `/Height` can be written freely: the
    /// common case is a mask the same size as its image, and every refusal here is a claim
    /// about a mask that is *not*.
    fn soft_mask(w: usize, h: usize, data: Vec<u8>, bits: usize) -> Stream {
        let mut dict = Dict::new();
        dict.set("Width", Object::Int(w as i64));
        dict.set("Height", Object::Int(h as i64));
        dict.set("BitsPerComponent", Object::Int(bits as i64));
        dict.set("ColorSpace", Object::name("DeviceGray"));
        Stream::new(dict, data)
    }

    /// An image of this size whose `/SMask` is `mask`.
    ///
    /// An `/SMask` here is the stream itself rather than a reference, because the resolver
    /// these tests use hands back what it is given and a reference would come back as a
    /// reference.
    fn with_mask(w: usize, h: usize, data: Vec<u8>, bits: usize, mask: &Stream) -> Stream {
        let mut stream = match bits {
            8 => rgb_image(w, h, data),
            1 => {
                // One bit per sample in `DeviceGray`, packed per row, which is the layout a
                // mask is most often written in.
                let mut dict = Dict::new();
                dict.set("Width", Object::Int(w as i64));
                dict.set("Height", Object::Int(h as i64));
                dict.set("BitsPerComponent", Object::Int(1));
                dict.set("ColorSpace", Object::name("DeviceGray"));
                Stream::new(dict, data)
            }
            other => panic!("no fixture writes {other} bits"),
        };
        stream.dict.set("SMask", Object::Stream(mask.clone()));
        stream
    }

    /// The notes of a decode, joined, for a test that asserts *what was said*.
    fn said(notes: &[String]) -> String {
        notes.join("; ")
    }

    /// A string of `0` and `1` as bytes, most significant bit first, which is the order a
    /// fax code is written in.
    fn pack(bits: &str) -> Vec<u8> {
        let mut out = Vec::new();
        for chunk in bits.as_bytes().chunks(8) {
            let mut byte = 0u8;
            for (i, b) in chunk.iter().enumerate() {
                byte |= u8::from(*b == b'1') << (7 - i);
            }
            out.push(byte);
        }
        out
    }

    /// One row of grey as the raster holds it, for a single-column-per-pixel image.
    fn row_of(raster: &Raster) -> Vec<u8> {
        (0..raster.width)
            .map(|x| raster.at(x, 0)[0])
            .collect::<Vec<u8>>()
    }

    /// Decode with a resolver that hands back the object it is given, which is what a
    /// resource table that holds its entries directly looks like. Returning `None` here
    /// would make every indirect lookup — an indexed image's palette among them — come back
    /// empty and the image black.
    fn decode_ok(stream: &Stream) -> (Raster, Vec<String>) {
        let mut notes = Vec::new();
        let raster = decode(stream, &|o| Some(o.clone()), &mut notes).expect("it decodes");
        (raster, notes)
    }

    #[test]
    fn eight_bit_samples_read_one_per_byte() {
        assert_eq!(sample_at(&[10, 20, 30, 255], 0, 0, 4, 1, 8, 0), 10);
        assert_eq!(sample_at(&[10, 20, 30, 255], 3, 0, 4, 1, 8, 0), 255);
    }

    #[test]
    fn sixteen_bit_samples_are_most_significant_first() {
        assert_eq!(
            sample_at(&[0x12, 0x34, 0xff, 0xff], 0, 0, 1, 2, 16, 0),
            0x1234
        );
        assert_eq!(
            sample_at(&[0x12, 0x34, 0xff, 0xff], 0, 0, 1, 2, 16, 1),
            0xffff
        );
    }

    #[test]
    fn one_bit_samples_are_packed_most_significant_first() {
        assert_eq!(sample_at(&[0b1010_0101], 0, 0, 8, 1, 1, 0), 1);
        assert_eq!(sample_at(&[0b1010_0101], 1, 0, 8, 1, 1, 0), 0);
        assert_eq!(sample_at(&[0b1010_0101], 2, 0, 8, 1, 1, 0), 1);
        assert_eq!(sample_at(&[0b1010_0101], 3, 0, 8, 1, 1, 0), 0);
    }

    /// The bug this guards: a row of an odd number of one-bit samples is padded to a byte,
    /// so row one does not start where row zero's arithmetic says it does.
    #[test]
    fn a_row_of_packed_samples_starts_on_its_own_byte() {
        // Three samples per row, so each row is one byte, and row one is `0b1000_0000`.
        let data = [0b1110_0000u8, 0b1000_0000];
        assert_eq!(sample_at(&data, 0, 0, 3, 1, 1, 0), 1);
        assert_eq!(sample_at(&data, 2, 0, 3, 1, 1, 0), 1, "the row is all ones");
        assert_eq!(
            sample_at(&data, 0, 1, 3, 1, 1, 0),
            1,
            "row one starts at byte one, not at bit three of byte zero"
        );
    }

    #[test]
    fn a_row_of_four_bit_samples_starts_on_its_own_byte() {
        // Width three, one component, four bits: one byte and a half per row, so row one
        // starts at byte two.
        let data = [0x0f, 0x00, 0xf0];
        assert_eq!(sample_at(&data, 0, 0, 3, 1, 4, 0), 0);
        assert_eq!(sample_at(&data, 1, 0, 3, 1, 4, 0), 0xf);
        assert_eq!(
            sample_at(&data, 0, 1, 3, 1, 4, 0),
            0xf,
            "row one is the third byte"
        );
    }

    #[test]
    fn four_and_two_bit_samples_read_correctly() {
        assert_eq!(sample_at(&[0x0f, 0xf0], 0, 0, 2, 1, 4, 0), 0);
        assert_eq!(sample_at(&[0x0f, 0xf0], 1, 0, 2, 1, 4, 0), 0xf);
        assert_eq!(sample_at(&[0b00_01_10_11], 0, 0, 4, 1, 2, 0), 0b00);
        assert_eq!(sample_at(&[0b00_01_10_11], 3, 0, 4, 1, 2, 0), 0b11);
    }

    #[test]
    fn reading_past_the_end_is_zero_rather_than_a_panic() {
        assert_eq!(sample_at(&[], 0, 0, 1, 1, 8, 0), 0);
        assert_eq!(sample_at(&[1, 2], 99, 0, 1, 1, 8, 0), 0);
        assert_eq!(sample_at(&[1, 2], 0, 99, 1, 1, 16, 0), 0);
    }

    #[test]
    fn an_unsupported_depth_reads_as_zero() {
        assert_eq!(
            sample_at(&[0xff; 8], 0, 0, 1, 1, 7, 0),
            0,
            "seven bits is not a thing"
        );
    }

    #[test]
    fn decode_maps_the_stored_range_onto_zero_to_one() {
        assert!((decode_component(0.0, [0.0, 1.0]) - 0.0).abs() < 1e-9);
        assert!((decode_component(0.5, [0.0, 1.0]) - 0.5).abs() < 1e-9);
        assert!((decode_component(1.0, [0.0, 1.0]) - 1.0).abs() < 1e-9);
    }

    /// An inverted `/Decode` is how a file writes a negative, and ignoring it gives a
    /// photograph of its own negative.
    #[test]
    fn an_inverted_decode_inverts() {
        assert!((decode_component(0.0, [1.0, 0.0]) - 1.0).abs() < 1e-9);
        assert!((decode_component(1.0, [1.0, 0.0]) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn a_decode_pair_matched_to_the_stored_range_works() {
        assert!((decode_component(1023.0, [0.0, 1023.0]) - 1.0).abs() < 1e-9);
        assert!((decode_component(512.0, [0.0, 1023.0]) - 0.5).abs() < 1e-3);
    }

    #[test]
    fn a_degenerate_decode_range_is_zero_rather_than_infinite() {
        assert_eq!(decode_component(5.0, [1.0, 1.0]), 0.0);
    }

    #[test]
    fn a_value_outside_the_decode_range_is_clamped() {
        assert_eq!(decode_component(-1.0, [0.0, 1.0]), 0.0);
        assert_eq!(decode_component(2.0, [0.0, 1.0]), 1.0);
    }

    #[test]
    fn decode_defaults_to_zero_to_one_per_component() {
        assert_eq!(read_decode(None, 3), vec![0.0, 1.0, 0.0, 1.0, 0.0, 1.0]);
    }

    #[test]
    fn decode_reads_a_short_array_and_ignores_a_long_one() {
        let short = Object::Array(vec![Object::Real(1.0), Object::Real(0.0)]);
        assert_eq!(
            read_decode(Some(&short), 3),
            vec![1.0, 0.0, 0.0, 1.0, 0.0, 1.0]
        );
        let long = Object::Array(vec![
            Object::Real(0.2),
            Object::Real(0.4),
            Object::Real(0.6),
            Object::Real(0.8),
            Object::Real(9.0),
            Object::Real(9.0),
        ]);
        // Two components need four entries, so the last two are surplus and dropped.
        assert_eq!(read_decode(Some(&long), 2), vec![0.2, 0.4, 0.6, 0.8]);
    }

    #[test]
    fn the_component_counts_are_the_specifications() {
        assert_eq!(Space::Stencil.components(), 1);
        assert_eq!(Space::Gray { decode: [0.0; 2] }.components(), 1);
        assert_eq!(Space::Rgb { decode: [0.0; 6] }.components(), 3);
        assert_eq!(Space::Cmyk { decode: [0.0; 8] }.components(), 4);
    }

    #[test]
    fn a_grey_image_decodes_to_three_equal_channels() {
        let (raster, _) = decode_ok(&grey_image(2, 1, vec![0, 255], 8));
        assert_eq!((raster.width, raster.height), (2, 1));
        assert_eq!(raster.pixels, vec![0, 0, 0, 255, 255, 255, 255, 255]);
    }

    #[test]
    fn an_rgb_image_decodes_its_three_components_in_order() {
        let (raster, _) = decode_ok(&rgb_image(1, 1, vec![10, 20, 30]));
        assert_eq!(raster.pixels, vec![10, 20, 30, 255]);
    }

    #[test]
    fn a_cmyk_image_is_subtractive() {
        let mut dict = Dict::new();
        dict.set("Width", Object::Int(1));
        dict.set("Height", Object::Int(1));
        dict.set("BitsPerComponent", Object::Int(8));
        dict.set("ColorSpace", Object::name("DeviceCMYK"));
        let (raster, _) = decode_ok(&Stream::new(dict, vec![255, 0, 0, 0]));
        assert_eq!(
            raster.pixels,
            vec![0, 255, 255, 255],
            "full cyan removes the red and leaves green and blue"
        );
    }

    #[test]
    fn an_image_mask_is_a_stencil_and_not_a_picture() {
        let mut dict = Dict::new();
        dict.set("Width", Object::Int(4));
        dict.set("Height", Object::Int(1));
        dict.set("ImageMask", Object::Bool(true));
        dict.set("BitsPerComponent", Object::Int(1));
        // Bits 7..4 are the four samples, most significant first: two paint and two do not.
        let (raster, _) = decode_ok(&Stream::new(dict, vec![0b0011_0000]));
        assert!(raster.is_stencil);
        assert_eq!(raster.pixels.len(), 16);
        assert_eq!(
            raster.pixels.get(3).copied(),
            Some(255),
            "the first bit paints"
        );
        assert_eq!(raster.pixels.get(11).copied(), Some(0), "the last does not");
    }

    /// The bug this guards, in both halves. A fax decoder hands back one byte per pixel,
    /// and that byte is a *value* as well as a *place*: read as packed bits it is a
    /// barcode, and read as eight bits and normalised by 255 it is a black page, because a
    /// fax scan is mostly white and its white sample is the byte 1.
    #[test]
    fn a_fax_stream_is_read_one_sample_per_byte_at_its_declared_value() {
        // White 3, black 3, white 2, as three Group 3 1D terminating codes: `1000`, `10`
        // and `011`. Every run is written down rather than left to the end of the row,
        // because the decoder's last run of a line is a separate matter from this one.
        let (raster, notes) = decode_ok(&fax_image(8, "100010011"));
        assert_eq!((raster.width, raster.height), (8, 1), "{notes:?}");
        assert_eq!(
            row_of(&raster),
            vec![255, 255, 255, 0, 0, 0, 255, 255],
            "each pixel is the byte the decoder wrote for it, read over the range \
             /BitsPerComponent declares rather than over 255"
        );
    }

    /// A width of eight is the width at which a packed misreading looks least wrong — the
    /// bytes land in the right row and the wrong pixels come out of them — so thirteen is
    /// the width that shows it.
    #[test]
    fn a_fax_stream_whose_width_is_not_a_multiple_of_eight_still_reads_one_per_byte() {
        // White 4, black 5, white 4, out of thirteen: `1011`, `0011`, `1011`. Read as
        // packed bits, thirteen columns would come out of two bytes of a thirteen-byte row,
        // which is the smear this guards.
        let (raster, notes) = decode_ok(&fax_image(13, "101100111011"));
        assert_eq!((raster.width, raster.height), (13, 1), "{notes:?}");
        assert_eq!(
            row_of(&raster),
            vec![255, 255, 255, 255, 0, 0, 0, 0, 0, 255, 255, 255, 255]
        );
    }

    /// The other side of the same decision: a stream the decoder did *not* expand is still
    /// packed, and reading it as one byte per pixel would undo the codec's own packing.
    #[test]
    fn a_packed_one_bit_image_still_unpacks() {
        // Width eight of unfiltered one-bit data: `0b1010_0101` is five samples.
        let (raster, _) = decode_ok(&grey_image(8, 1, vec![0b1010_0101], 1));
        assert_eq!(
            row_of(&raster),
            vec![255, 0, 255, 0, 0, 255, 0, 255],
            "eight one-bit samples out of one byte, most significant first"
        );
    }

    // ── Layout and value are different facts ─────────────────────────────────────────

    /// What a decoder says about layout and what the dictionary says about value, and what
    /// each of them does when the other is wrong. A buffer of one byte per pixel whose
    /// samples are a bilevel 0 and 1 is the shape a fax decoder hands back.
    #[test]
    fn a_one_byte_per_pixel_bilevel_buffer_renders_black_and_white() {
        let gray = Space::Gray { decode: [0.0, 1.0] };
        // Laid out a byte apiece and worth one bit, which is what a fax image is: the
        // samples are zero and one and zero means black.
        let pixels = to_rgba(
            &[0, 1],
            &gray,
            2,
            1,
            SampleRange {
                layout: 8,
                value: 1,
            },
            1,
        );
        assert_eq!(
            &pixels[..4],
            &[0, 0, 0, 255],
            "a zero sample is black, because zero is the bottom of the declared range"
        );
        assert_eq!(
            &pixels[4..],
            &[255, 255, 255, 255],
            "and a sample of one is the top of that range, not one 255th of the way up it"
        );
    }

    /// The same bytes read as eight-bit samples instead, which is what happens when the
    /// decoder's layout signal is ignored: the sample 1 is then worth 1/255 and a white
    /// fax page comes out black.
    #[test]
    fn the_same_bytes_normalised_over_255_are_the_black_page_they_were() {
        let gray = Space::Gray { decode: [0.0, 1.0] };
        let pixels = to_rgba(&[0, 1], &gray, 2, 1, SampleRange::declared(8), 1);
        assert_eq!(&pixels[..4], &[0, 0, 0, 255]);
        assert_eq!(
            &pixels[4..8],
            &[1, 1, 1, 255],
            "one over 255 is black: this is what the value range has to stop"
        );
    }

    /// A sample of 255 in a buffer whose declared range is one bit is still the top of that
    /// range, so a decoder that wrote a full byte rather than a bare bit cannot make a page
    /// black either.
    #[test]
    fn a_full_byte_in_a_one_bit_range_is_still_white() {
        let gray = Space::Gray { decode: [0.0, 1.0] };
        let pixels = to_rgba(
            &[255, 0, 255],
            &gray,
            3,
            1,
            SampleRange {
                layout: 8,
                value: 1,
            },
            1,
        );
        assert_eq!(
            &pixels[..4],
            &[255, 255, 255, 255],
            "255 is past the top, and clamps"
        );
        assert_eq!(&pixels[4..8], &[0, 0, 0, 255]);
        assert_eq!(&pixels[8..], &[255, 255, 255, 255]);
    }

    /// The layout half on its own: the same thirteen bytes read as a packed bit stream
    /// have to come out of it exactly as they always did, or the split has broken the case
    /// it was supposed to leave alone.
    #[test]
    fn the_same_bytes_read_as_a_packed_bit_stream_are_unchanged() {
        let gray = Space::Gray { decode: [0.0, 1.0] };
        let bytes = [0b1010_1010u8, 0b0101_0101];
        let packed = to_rgba(&bytes, &gray, 13, 1, SampleRange::declared(1), 1);
        let grey = row_of(&decode_ok(&grey_image(13, 1, bytes.to_vec(), 1)).0);
        for (x, got) in packed.chunks_exact(4).enumerate() {
            assert_eq!(got[0], grey[x], "column {x} of thirteen");
        }
        assert_eq!(
            packed.chunks_exact(4).map(|p| p[0]).collect::<Vec<u8>>(),
            vec![255, 0, 255, 0, 255, 0, 255, 0, 0, 255, 0, 255, 0],
            "and those are the thirteen bits of the two bytes, most significant first"
        );
    }

    /// A real eight-bit image has to be untouched by any of this: its samples really do
    /// span 0..255, and dividing them by 1 or by 15 would flatten it.
    /// An `[/ICCBased …]` image resolves through the `/Alternate` its profile names, the
    /// same way a colour in the same space does.
    ///
    /// The fixture is the one that matters: a four-component profile whose `/N` says 4 and
    /// whose `/Alternate` says `/DeviceCMYK` is read as CMYK, and one whose `/N` says 3
    /// while the alternate says `/DeviceCMYK` is *still* read as CMYK. A reader cannot
    /// apply the profile, and the alternate is the producer's answer to exactly that, so
    /// following it is not an approximation of the profile — it is what the file says to do
    /// instead. Counting `/N` where the alternate is available would paint a CMYK image
    /// from its components as though they were red, green and blue.
    #[test]
    fn an_icc_based_image_is_read_through_its_alternate() {
        let profile = |n: i64, alternate: Option<&str>| {
            let mut dict = Dict::new();
            dict.set("N", Object::Int(n));
            if let Some(alternate) = alternate {
                dict.set("Alternate", Object::name(alternate));
            }
            Object::Stream(Stream::new(dict, vec![0u8; 4]))
        };
        let icc_image = |n: i64, alternate: Option<&str>, data: Vec<u8>| {
            let mut dict = Dict::new();
            dict.set("Width", Object::Int(1));
            dict.set("Height", Object::Int(1));
            dict.set("BitsPerComponent", Object::Int(8));
            dict.set(
                "ColorSpace",
                Object::Array(vec![Object::name("ICCBased"), profile(n, alternate)]),
            );
            Stream::new(dict, data)
        };
        // 1 0 0 0 is cyan in CMYK and a very dark red read as RGB, so this distinguishes
        // the two readings exactly.
        let (raster, notes) = decode_ok(&icc_image(4, Some("DeviceCMYK"), vec![255, 0, 0, 0]));
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(raster.at(0, 0)[..3], [0, 255, 255], "subtractive: cyan");

        // And with a `/N` that contradicts the alternate, the alternate still wins.
        let (raster, _) = decode_ok(&icc_image(3, Some("DeviceCMYK"), vec![255, 0, 0]));
        assert_eq!(
            raster.at(0, 0)[..3],
            [0, 255, 255],
            "the alternate is what the producer said the data is"
        );

        // A one-component profile is grey whether it says so or is merely counted as one.
        let (raster, _) = decode_ok(&icc_image(1, Some("DeviceGray"), vec![128]));
        let got = raster.at(0, 0);
        assert_eq!(got[0..3], [got[0], got[0], got[0]], "grey is grey: {got:?}");

        // With no alternate the count is still enough to read the samples, which is what
        // an image does where a colour cannot.
        let (raster, _) = decode_ok(&icc_image(4, None, vec![255, 0, 0, 0]));
        assert_eq!(
            raster.at(0, 0)[..3],
            [0, 255, 255],
            "the /N fallback still works"
        );
        let (raster, _) = decode_ok(&icc_image(1, None, vec![128]));
        let got = raster.at(0, 0);
        assert_eq!(
            got[0..3],
            [got[0], got[0], got[0]],
            "and for grey too: {got:?}"
        );
    }

    #[test]
    fn a_genuine_eight_bit_image_is_unaffected() {
        let (raster, notes) = decode_ok(&grey_image(4, 1, vec![0, 1, 128, 255], 8));
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(
            row_of(&raster),
            vec![0, 1, 128, 255],
            "four grey levels, still four grey levels"
        );
        assert_eq!(
            raster.pixels,
            vec![
                0, 0, 0, 255, //
                1, 1, 1, 255, //
                128, 128, 128, 255, //
                255, 255, 255, 255
            ]
        );
    }

    /// And a 2- or 4-bit image, whose declared range is neither 1 nor 255.
    #[test]
    fn a_two_bit_image_still_spans_its_own_range() {
        let (raster, _) = decode_ok(&grey_image(4, 1, vec![0b00_01_10_11], 2));
        assert_eq!(
            row_of(&raster),
            vec![0, 85, 170, 255],
            "four two-bit samples over the range zero to three"
        );
    }

    /// A one-byte-per-pixel `/ImageMask`. The zero bits paint and the one bits do not,
    /// which is the whole of a stencil and neither half of it may be decided by dividing
    /// the byte by a hundred and fifty-five.
    #[test]
    fn a_one_byte_per_pixel_mask_paints_its_zero_bits_and_leaves_the_ones() {
        let (raster, notes) = decode_ok(&fax_mask(8, "1000".to_owned() + "0011"));
        assert!(notes.is_empty(), "{notes:?}");
        assert!(raster.is_stencil);
        assert_eq!((raster.width, raster.height), (8, 1));
        // `/BlackIs1 true` leaves the decoder's zero as zero: three white, then five black.
        // A mask paints its *zeros*, so the left three columns paint and the rest is paper.
        let painted = |x: usize| raster.at(x, 0)[3] == 255;
        assert!((0..3).all(painted), "the zero bits paint");
        assert!(
            (3..8).all(|x| !painted(x)),
            "and the one bits are not painted, which a comparison against one half over \
             255 would have got backwards"
        );
        // And drawn, in a colour of the graphics state's choosing.
        let paper = draw_on_paper(&raster, Some(Rgba::BLACK));
        for x in 0..8 {
            let want = if x < 3 {
                [0, 0, 0, 255]
            } else {
                [255, 255, 255, 255]
            };
            assert_eq!(paper.get(x, 0), Some(want), "column {x}");
        }
    }

    /// Every width a fax mask can be, so the width does not decide which bits paint.
    #[test]
    fn a_one_byte_per_pixel_mask_of_odd_width_still_separates_its_bits() {
        // White four (`1011`) then black nine (`000100`), thirteen columns in all, so the
        // row is thirteen bytes and no two of them share a byte the way packed bits would.
        let (raster, notes) = decode_ok(&fax_mask(13, "1011".to_owned() + "000100"));
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!((raster.width, raster.height), (13, 1), "{notes:?}");
        for x in 0..13 {
            assert_eq!(
                raster.at(x, 0)[3] == 255,
                x < 4,
                "column {x}: four zeros then nine ones, one byte each"
            );
        }
    }

    #[test]
    fn a_packed_one_bit_row_of_odd_width_starts_on_its_own_byte() {
        // Thirteen columns of one bit is thirteen bits, so a row is two bytes and row one
        // starts at byte two rather than at bit thirteen of byte one. Row zero is all ones
        // across its thirteen bits, and row one starts with a one.
        let (raster, _) = decode_ok(&grey_image(
            13,
            2,
            vec![0b1010_1010, 0b1010_1000, 0b1100_0000],
            1,
        ));
        assert_eq!((raster.width, raster.height), (13, 2));
        assert_eq!(
            row_of(&raster),
            vec![255, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255]
        );
        assert_eq!(
            raster.at(0, 1)[0],
            255,
            "row one starts at byte two, where the padding put it"
        );
    }

    #[test]
    fn an_indexed_image_reads_its_palette() {
        let mut dict = Dict::new();
        dict.set("Width", Object::Int(2));
        dict.set("Height", Object::Int(1));
        dict.set("BitsPerComponent", Object::Int(8));
        dict.set(
            "ColorSpace",
            Object::Array(vec![
                Object::name("Indexed"),
                Object::name("DeviceRGB"),
                Object::Int(1),
                Object::string("\x10\x20\x30\x40\x50\x60"),
            ]),
        );
        // Eight bits per component against a two-entry palette: the whole byte range maps
        // onto the two indices, so index 0 is the byte 0 and index 1 is the byte 255.
        let (raster, _) = decode_ok(&Stream::new(dict, vec![0, 255]));
        assert_eq!(
            raster.pixels,
            vec![0x10, 0x20, 0x30, 255, 0x40, 0x50, 0x60, 255]
        );
    }

    #[test]
    fn an_inverted_decode_gives_the_negative_of_the_picture() {
        let mut inverted = Dict::new();
        inverted.set("Width", Object::Int(1));
        inverted.set("Height", Object::Int(1));
        inverted.set("BitsPerComponent", Object::Int(8));
        inverted.set("ColorSpace", Object::name("DeviceGray"));
        inverted.set(
            "Decode",
            Object::Array(vec![Object::Int(1), Object::Int(0)]),
        );
        let (raster, _) = decode_ok(&Stream::new(inverted, vec![64]));
        assert_eq!(raster.pixels.first().copied(), Some(191), "255 - 64");
    }

    #[test]
    fn interpolate_is_read_and_defaults_to_off() {
        let mut dict = Dict::new();
        dict.set("Width", Object::Int(1));
        dict.set("Height", Object::Int(1));
        dict.set("BitsPerComponent", Object::Int(8));
        dict.set("ColorSpace", Object::name("DeviceGray"));
        assert!(
            !decode_ok(&Stream::new(dict.clone(), vec![128]))
                .0
                .interpolate
        );
        dict.set("Interpolate", Object::Bool(true));
        assert!(decode_ok(&Stream::new(dict, vec![128])).0.interpolate);
    }

    #[test]
    fn a_lab_image_is_refused_because_it_has_no_white_point() {
        let mut dict = Dict::new();
        dict.set("Width", Object::Int(1));
        dict.set("Height", Object::Int(1));
        dict.set("BitsPerComponent", Object::Int(8));
        dict.set("ColorSpace", Object::name("Lab"));
        let mut notes = Vec::new();
        assert!(decode(&Stream::new(dict, vec![50, 20, 30]), &|_| None, &mut notes).is_none());
        assert!(!notes.is_empty(), "and the refusal is reported");
    }

    #[test]
    fn an_image_of_impossible_size_is_refused_with_a_note() {
        let mut notes = Vec::new();
        assert!(decode(&Stream::new(Dict::new(), Vec::new()), &|_| None, &mut notes).is_none());
        assert!(notes.iter().any(|n| n.contains("not drawn")), "{notes:?}");

        let mut notes = Vec::new();
        let stream = grey_image(0, 10, Vec::new(), 8);
        assert!(decode(&stream, &|_| None, &mut notes).is_none());
        assert!(notes.iter().any(|n| n.contains("0 by 10")), "{notes:?}");
    }

    #[test]
    fn an_odd_bit_depth_is_refused_with_a_note() {
        let mut notes = Vec::new();
        let stream = grey_image(1, 1, vec![0], 7);
        assert!(decode(&stream, &|_| None, &mut notes).is_none());
        assert!(notes.iter().any(|n| n.contains("7 bits")), "{notes:?}");
    }

    #[test]
    fn a_jpeg_2000_image_is_reported_rather_than_drawn_blank() {
        let mut dict = Dict::new();
        dict.set("Width", Object::Int(4));
        dict.set("Height", Object::Int(4));
        dict.set("BitsPerComponent", Object::Int(8));
        dict.set("ColorSpace", Object::name("DeviceRGB"));
        dict.set("Filter", Object::name("JPXDecode"));
        let mut notes = Vec::new();
        assert!(decode(&Stream::new(dict, vec![0u8; 16]), &|_| None, &mut notes).is_none());
        assert!(notes.iter().any(|n| n.contains("JPEG 2000")), "{notes:?}");
    }

    // ── What a refusal leaves unread ─────────────────────────────────────────────────

    /// An image of `filter` whose own samples cannot be decoded here.
    fn coded_image(filter: &str, w: usize, h: usize) -> Stream {
        let mut dict = Dict::new();
        dict.set("Width", Object::Int(w as i64));
        dict.set("Height", Object::Int(h as i64));
        dict.set("BitsPerComponent", Object::Int(8));
        dict.set("ColorSpace", Object::name("DeviceRGB"));
        dict.set("Filter", Object::name(filter));
        Stream::new(dict, vec![0u8; w * h * 3])
    }

    /// An image we cannot decode, whose `/SMask` is also one we cannot decode, reports
    /// both — and the mask's own codec by name.
    ///
    /// This is the shape of the failure the fix is about. The `/SMask` is read about a
    /// hundred lines below the codec refusal, so the picture was missing and the mask was
    /// never mentioned: one gap reported where there were two. A page that loses both has
    /// to say so, because "a JPEG 2000 image was not decoded" reads as a codec gap when
    /// the transparency is a second, independent one.
    #[test]
    fn an_undecodable_image_reports_the_mask_it_never_looked_at() {
        let mut image = coded_image("JPXDecode", 4, 4);
        image
            .dict
            .set("SMask", Object::Stream(coded_image("JBIG2Decode", 4, 4)));
        let mut notes = Vec::new();
        assert!(decode(&image, &|o| Some(o.clone()), &mut notes).is_none());
        let said = said(&notes);
        assert!(
            said.contains("JPEG 2000"),
            "the image is reported: {said:?}"
        );
        assert!(said.contains("`/SMask`"), "and the mask is: {said:?}");
    }

    /// The same for a `/Mask`, which is the case the corpus found: a JPEG 2000 picture
    /// whose mask is JBIG2, where the mask's codec is named as well as the mask.
    #[test]
    fn an_undecodable_image_reports_the_jbig2_mask_it_never_looked_at() {
        let mut image = coded_image("JPXDecode", 4, 4);
        image
            .dict
            .set("Mask", Object::Stream(coded_image("JBIG2Decode", 4, 4)));
        let mut notes = Vec::new();
        assert!(decode(&image, &|o| Some(o.clone()), &mut notes).is_none());
        let said = said(&notes);
        assert!(said.contains("`/Mask`"), "the mask is reported: {said:?}");
        assert!(
            said.contains("JBIG2"),
            "and so is the codec of the mask, which is a second gap and not the first: {said:?}"
        );
    }

    /// A mask we could have read is still reported as skipped: it was skipped because the
    /// image it belongs to was not decoded, and a note that says only "not decoded" leaves
    /// the reader to guess whether anything else was lost.
    #[test]
    fn an_undecodable_image_reports_a_readable_mask_it_never_looked_at() {
        let mut image = coded_image("JBIG2Decode", 4, 4);
        image
            .dict
            .set("SMask", Object::Stream(soft_mask(4, 4, vec![255; 16], 8)));
        let mut notes = Vec::new();
        assert!(decode(&image, &|o| Some(o.clone()), &mut notes).is_none());
        assert!(said(&notes).contains("`/SMask`"), "{notes:?}");
    }

    /// An image with no mask of either kind reports only its own gap, which is the other
    /// half of the property: a note that lists everything is no use if it lists things
    /// that are not there.
    #[test]
    fn an_undecodable_image_with_no_mask_reports_only_its_own_gap() {
        let mut notes = Vec::new();
        assert!(
            decode(
                &coded_image("JPXDecode", 4, 4),
                &|o| Some(o.clone()),
                &mut notes
            )
            .is_none()
        );
        let said = said(&notes);
        assert!(said.contains("JPEG 2000"), "{said:?}");
        assert!(
            !said.contains("/Mask") && !said.contains("/SMask"),
            "and nothing about a mask the file does not have: {said:?}"
        );
    }

    /// A refusal that is not a codec is reported the same way, which is the reason the
    /// reporting sits outside the codec arms rather than inside them.
    #[test]
    fn any_refusal_reports_what_was_skipped_with_it() {
        let mut image = rgb_image(4, 4, vec![0; 48]);
        // `/BitsPerComponent 7` is not a thing, and the refusal happens well before the
        // mask keys are read.
        image.dict.set("BitsPerComponent", Object::Int(7));
        image
            .dict
            .set("SMask", Object::Stream(soft_mask(4, 4, vec![255; 16], 8)));
        let mut notes = Vec::new();
        assert!(decode(&image, &|o| Some(o.clone()), &mut notes).is_none());
        let said = said(&notes);
        assert!(said.contains("7 bits"), "{said:?}");
        assert!(said.contains("`/SMask`"), "and the skipped mask: {said:?}");
    }

    #[test]
    fn a_jbig2_image_is_reported_rather_than_drawn_blank() {
        let mut dict = Dict::new();
        dict.set("Width", Object::Int(4));
        dict.set("Height", Object::Int(4));
        dict.set("BitsPerComponent", Object::Int(1));
        dict.set("ColorSpace", Object::name("DeviceGray"));
        dict.set("Filter", Object::name("JBIG2Decode"));
        let mut notes = Vec::new();
        assert!(decode(&Stream::new(dict, vec![0u8; 16]), &|_| None, &mut notes).is_none());
        assert!(notes.iter().any(|n| n.contains("JBIG2")), "{notes:?}");
    }

    // ── Stencils on a page ────────────────────────────────────────────────────────────

    /// An eight-by-eight mask whose painting bits are the four-by-four block at its top
    /// left. One byte per row, one bit per pixel, so each row's own byte says everything
    /// about it.
    fn corner_mask() -> Raster {
        let mut dict = Dict::new();
        dict.set("Width", Object::Int(8));
        dict.set("Height", Object::Int(8));
        dict.set("ImageMask", Object::Bool(true));
        dict.set("BitsPerComponent", Object::Int(1));
        let mut data = vec![0b1111_1111u8; 8];
        for row in data.iter_mut().take(4) {
            *row = 0b0000_1111;
        }
        decode_ok(&Stream::new(dict, data)).0
    }

    /// Draw a raster over an eight-by-eight sheet of paper, one raster pixel to one pixel.
    ///
    /// The matrix is the one a page produces — the unit square scaled to the sheet with its
    /// vertical axis turned over, because a canvas counts down and a page counts up — and not
    /// a bare scale. A matrix with no turn is not a placement any page can ask for, and
    /// drawing through one would say an image's first row is its *last* one.
    fn draw_on_paper(raster: &Raster, fill: Option<Rgba>) -> crate::Image {
        let mut device = Device::new(crate::Image::filled(8, 8, [255, 255, 255, 255]));
        let unit_square = Matrix::new(8.0, 0.0, 0.0, -8.0, 0.0, 8.0);
        let fill = fill.map(FillColour::flat);
        draw(&mut device, raster, &unit_square, 1.0, fill.as_ref());
        device.into_image()
    }

    #[test]
    fn a_mask_paints_its_zero_bits_in_the_fill_colour_and_leaves_the_ones_as_paper() {
        let paper = draw_on_paper(&corner_mask(), Some(Rgba::BLACK));
        for y in 0..8 {
            for x in 0..8 {
                let want = if x < 4 && y < 4 {
                    [0, 0, 0, 255]
                } else {
                    [255, 255, 255, 255]
                };
                assert_eq!(paper.get(x, y), Some(want), "at ({x}, {y})");
            }
        }
    }

    #[test]
    fn two_fill_colours_both_take_effect_on_a_mask() {
        let red = Rgba {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        };
        let blue = Rgba {
            r: 0.0,
            g: 0.0,
            b: 1.0,
            a: 1.0,
        };
        let one = draw_on_paper(&corner_mask(), Some(red));
        let other = draw_on_paper(&corner_mask(), Some(blue));
        assert_eq!(one.get(1, 1), Some([255, 0, 0, 255]), "painted red");
        assert_eq!(other.get(1, 1), Some([0, 0, 255, 255]), "painted blue");
        assert_eq!(
            other.get(6, 6),
            Some([255, 255, 255, 255]),
            "and the one bits are still paper in both"
        );
    }

    /// A stencil with no colour to paint in draws nothing, rather than a block in some
    /// arbitrary ink: the caller has said why, and guessing would be the wrong answer.
    #[test]
    fn a_mask_with_no_paint_colour_draws_nothing() {
        let raster = corner_mask();
        let paper = draw_on_paper(&raster, None);
        assert!(
            paper
                .pixels
                .chunks_exact(4)
                .all(|p| p == [255, 255, 255, 255]),
            "the page is untouched"
        );
    }

    #[test]
    fn an_image_that_is_not_a_mask_paints_its_own_samples() {
        // Half black, half white, in DeviceGray, with a red fill colour in force: an image
        // is not a stencil, so the fill colour has no say in what it draws.
        let (raster, _) = decode_ok(&grey_image(2, 1, vec![0, 255], 8));
        let red = FillColour::flat(Rgba {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        });
        let mut device = Device::new(crate::Image::filled(2, 1, [255, 255, 255, 255]));
        let unit_square = Matrix::new(2.0, 0.0, 0.0, 1.0, 0.0, 0.0);
        draw(&mut device, &raster, &unit_square, 1.0, Some(&red));
        let image = device.into_image();
        assert_eq!(image.get(0, 0), Some([0, 0, 0, 255]), "its own black");
        assert_eq!(image.get(1, 0), Some([255, 255, 255, 255]), "its own white");
    }

    #[test]
    fn nearest_sampling_picks_the_pixel_that_contains_the_point() {
        let raster = Raster {
            width: 2,
            height: 2,
            pixels: vec![
                0, 0, 0, 255, 255, 255, 255, 255, 255, 0, 0, 255, 255, 255, 255, 255,
            ],
            interpolate: false,
            soft_mask: None,
            key_range: None,
            is_stencil: false,
        };
        assert_eq!(raster.sample(0.1, 0.1)[0], 0, "the top-left pixel");
        assert_eq!(raster.sample(0.9, 0.1)[0], 255, "the top-right pixel");
    }

    #[test]
    fn bilinear_sampling_blends_between_the_neighbours() {
        let raster = Raster {
            width: 2,
            height: 1,
            pixels: vec![0, 0, 0, 255, 255, 255, 255, 255],
            interpolate: true,
            soft_mask: None,
            key_range: None,
            is_stencil: false,
        };
        // Halfway between the two pixels' centres.
        let mid = raster.sample(0.5, 0.5);
        let r = i32::from(mid[0]);
        assert!(
            (100..=155).contains(&r),
            "a halfway sample is about 128, not one end or the other: {r}"
        );
    }

    #[test]
    fn sampling_outside_the_image_is_clamped_rather_than_wrapping() {
        let raster = Raster {
            width: 2,
            height: 2,
            pixels: vec![0; 16],
            interpolate: false,
            soft_mask: None,
            key_range: None,
            is_stencil: false,
        };
        assert_eq!(raster.sample(-1.0, -1.0)[0], 0);
        assert_eq!(raster.sample(2.0, 2.0)[0], 0);
    }

    #[test]
    fn an_image_with_no_samples_does_not_panic() {
        let raster = Raster {
            width: 0,
            height: 0,
            pixels: Vec::new(),
            interpolate: false,
            soft_mask: None,
            key_range: None,
            is_stencil: false,
        };
        assert_eq!(raster.sample(0.5, 0.5), [0, 0, 0, 0]);
    }

    #[test]
    fn an_image_larger_than_the_bound_is_refused() {
        const {
            assert!(MAX_IMAGE_PIXELS == 64 * 1024 * 1024);
            assert!(MAX_IMAGE_PIXELS > 1_000_000, "room for a real photograph");
        }
    }

    // ── A soft mask, which is a separate image and has to be checked as one ──────────

    /// The bound is a question, and both sides of it are asserted rather than the shape of
    /// the check.
    #[test]
    fn the_bound_is_at_the_bound_and_one_pixel_over_is_not() {
        // 8192 by 8192 is exactly 64 · 1024 · 1024 pixels.
        assert!(!exceeds_bound(8192, 8192), "at the bound is drawn");
        assert!(
            exceeds_bound(8193, 8192) && exceeds_bound(8192, 8193),
            "one pixel over in either axis is refused, and `>` is what says so"
        );
        assert!(
            exceeds_bound(34862, 4332),
            "the corpus file's mask is 151 million samples"
        );
    }

    /// A multiplication that wraps is not a small image.
    #[test]
    fn a_bound_saturated_by_an_absurd_size_is_still_over_it() {
        assert!(
            exceeds_bound(usize::MAX, 2),
            "2^65 pixels is not zero pixels"
        );
        assert!(exceeds_bound(usize::MAX, 1));
        assert!(!exceeds_bound(1, 1));
    }

    /// The common case: a mask the same size as the image, which is what the specification
    /// requires and what nearly every file has. It must keep working.
    #[test]
    fn a_mask_the_same_size_as_its_image_is_used() {
        // Two by two, so one byte per row at one bit per sample. The top row is white and the
        // bottom row is black, which is a mask with a visible edge rather than a constant.
        // The mask's first row is its *top* row, so this is opaque above and transparent
        // below — a mask with a visible edge rather than a constant one.
        let mask = soft_mask(2, 2, vec![0b1111_1111, 0b0000_0000], 1);
        let image = with_mask(
            2,
            2,
            vec![255, 0, 0, 0, 0, 255, 0, 255, 0, 255, 0, 255],
            8,
            &mask,
        );
        let (raster, notes) = decode_ok(&image);
        assert!(
            notes.is_empty(),
            "an ordinary mask is not a complaint: {notes:?}"
        );
        let soft = raster.soft_mask.as_ref().expect("the mask was read");
        assert_eq!((soft.width, soft.height), (2, 2));
        // The mask is a luminance, so its white row is opaque and its black row is not. The
        // alpha reaches the page through `draw`, so that is where it is asserted rather than
        // on the raster's own pixels — the image's alpha is 255 everywhere and the mask is
        // what makes it otherwise.
        assert_eq!(soft.at(0, 0)[0], 255, "the mask's first row is white");
        assert_eq!(soft.at(0, 1)[0], 0, "and its second is black");
        assert!(
            (raster.mask_alpha(0.5, 0.25) - 1.0).abs() < 1e-9,
            "opaque above"
        );
        assert!(
            (raster.mask_alpha(0.5, 0.75) - 0.0).abs() < 1e-9,
            "not drawn below"
        );

        let mut device = Device::new(crate::Image::filled(2, 2, [255, 255, 255, 255]));
        // The canvas counts its rows down and `draw` turns the page's y axis over itself, so the
        // unit square onto rows 0..2 is a negative y scale rather than a positive one.
        let unit_square = Matrix::new(2.0, 0.0, 0.0, -2.0, 0.0, 2.0);
        assert!(draw(&mut device, &raster, &unit_square, 1.0, None));
        let page = device.into_image();
        assert_eq!(
            page.get(0, 0),
            Some([255, 0, 0, 255]),
            "red, where the mask is white"
        );
        assert_eq!(
            page.get(0, 1),
            Some([255, 255, 255, 255]),
            "paper, where the mask is black — the image's colour does not leak through"
        );
    }

    /// A mask's luminance is its alpha, and that is what the drawn picture answers.
    #[test]
    fn a_masks_alpha_comes_from_its_luminance() {
        let raster = Raster {
            width: 1,
            height: 1,
            pixels: vec![10, 20, 30, 255],
            interpolate: false,
            soft_mask: Some(Box::new(Raster {
                width: 1,
                height: 1,
                pixels: vec![128, 128, 128, 255],
                interpolate: false,
                soft_mask: None,
                key_range: None,
                is_stencil: false,
            })),
            key_range: None,
            is_stencil: false,
        };
        let alpha = raster.mask_alpha(0.5, 0.5);
        assert!(
            (alpha - 128.0 / 255.0).abs() < 1e-9,
            "half grey is half opaque, and it is per image sample rather than per device pixel"
        );
    }

    /// A missing mask is fully opaque, not zero: an alpha nobody can check is not a claim of
    /// transparency.
    #[test]
    fn an_image_with_no_mask_is_fully_opaque() {
        let raster = Raster {
            width: 1,
            height: 1,
            pixels: vec![10, 20, 30, 255],
            interpolate: false,
            soft_mask: None,
            key_range: None,
            is_stencil: false,
        };
        assert_eq!(raster.mask_alpha(0.5, 0.5), 1.0);
    }

    /// The defect, as it was found: a 2 by 2 image whose `/SMask` is 34862 by 4332.
    ///
    /// The mask was read with no check at all, and a mask that large sampled at a two-by-two
    /// image's coordinates is a constant, so the picture painted solid. Here the mask is
    /// reported and the picture keeps its own colours at full alpha.
    #[test]
    fn a_mask_the_size_of_another_page_is_reported_and_the_image_keeps_its_colours() {
        // The corpus file's own numbers, and the mask's stream is *not* filled with 151
        // million samples — the point of the test is that it is never decoded, so its
        // contents are not what decides anything.
        let mask = soft_mask(34862, 4332, vec![0; 16], 8);
        let image = with_mask(2, 2, vec![255; 24], 8, &mask);
        let mut notes = Vec::new();
        let raster = decode(&image, &|o| Some(o.clone()), &mut notes).expect("the image decodes");
        assert!(
            raster.soft_mask.is_none(),
            "a mask 17431 times the width of the image it masks is not an alpha channel"
        );
        let said = said(&notes);
        assert!(
            said.contains("soft mask") && said.contains("34862") && said.contains("4332"),
            "the mask is reported by its own size: {said:?}"
        );
        // The image's colours are real and worth showing.
        assert_eq!(raster.at(1, 1), [255, 255, 255, 255], "drawn, and opaque");
    }

    /// The same fact for a mask that is merely the wrong size, which is damaged rather than
    /// hostile.
    #[test]
    fn a_mask_that_does_not_match_its_image_is_reported() {
        let mask = soft_mask(3, 2, vec![0, 0, 0, 0, 0, 0], 8);
        let image = with_mask(2, 2, vec![0; 24], 8, &mask);
        let mut notes = Vec::new();
        let raster = decode(&image, &|o| Some(o.clone()), &mut notes).expect("the image decodes");
        assert!(raster.soft_mask.is_none());
        assert!(
            said(&notes).contains("does not match"),
            "a mask of a different size is reported: {notes:?}"
        );
    }

    /// The bound is a boundary, and one pixel over is on the other side of it.
    ///
    /// What is asserted is **which** refusal happened, not whether one did. A mask of
    /// exactly 64 · 1024 · 1024 samples cannot be decoded in a test — 604 MB of RGBA — so
    /// the only honest way to stand on the boundary is to give the mask at the bound no
    /// samples at all and read the reason: the bound must not be it. An off-by-one makes this
    /// fail at exactly the pixel where it belongs.
    #[test]
    fn a_mask_at_the_bound_is_not_refused_and_one_pixel_over_is() {
        // Exactly at the bound: 8192 by 8192 samples.
        let at_bound = soft_mask(8192, 8192, vec![0; 32], 8);
        let mut notes = Vec::new();
        let _ = decode_soft_mask(&at_bound, 8192, 8192, &|o| Some(o.clone()), &mut notes);
        assert!(
            !said(&notes).contains("above the"),
            "a mask of exactly {MAX_IMAGE_PIXELS} samples is within the bound, so the bound \
             cannot be why this mask was refused: {notes:?}"
        );

        // One pixel over, in the shape the corpus file actually uses.
        let over = soft_mask(34862, 4332, vec![0; 32], 8);
        notes.clear();
        assert!(
            decode_soft_mask(&over, 34862, 4332, &|o| Some(o.clone()), &mut notes).is_none(),
            "151 million samples is over the bound whatever the image is"
        );
        assert!(
            said(&notes).contains("above the"),
            "and it is refused on the bound rather than on its size: {notes:?}"
        );
    }

    /// A mask under a mask is not followed, because a chain of them is a file choosing the depth
    /// of the recursion rather than the renderer discovering one.
    #[test]
    fn a_mask_under_a_mask_is_reported_rather_than_followed() {
        let inner = soft_mask(2, 2, vec![0b1111_1111, 0b1111_1111], 1);
        let mut outer = soft_mask(2, 2, vec![0b1111_1111, 0b1111_1111], 1);
        outer.dict.set("SMask", Object::Stream(inner));
        let image = with_mask(2, 2, vec![255; 24], 8, &outer);
        let mut notes = Vec::new();
        let raster = decode(&image, &|o| Some(o.clone()), &mut notes).expect("the image decodes");
        let soft = raster
            .soft_mask
            .as_ref()
            .expect("the outer mask is still read");
        assert!(
            soft.soft_mask.is_none(),
            "and the mask under it is not: a soft mask's luminance is its alpha, so there is \\
             nothing for a second mask to say"
        );
        assert!(
            said(&notes).contains("soft mask of its own"),
            "reported: {notes:?}"
        );
    }

    /// A mask that decoded short is reported rather than padded, because padding it with
    /// zeroes is drawing the picture as though the missing part were transparent.
    #[test]
    fn a_mask_that_decodes_short_is_reported_rather_than_padded() {
        // Two by two at eight bits a sample is eight bytes; this carries one.
        let mask = soft_mask(2, 2, vec![255], 8);
        let image = with_mask(2, 2, vec![255; 24], 8, &mask);
        let mut notes = Vec::new();
        let raster = decode(&image, &|o| Some(o.clone()), &mut notes).expect("the image decodes");
        assert!(
            raster.soft_mask.is_none(),
            "a mask of one byte is not a mask of four"
        );
        let said = said(&notes);
        assert!(
            said.contains("soft mask") && said.contains("bytes"),
            "reported as a short stream rather than as a refusal of the mask's meaning: {said:?}"
        );
        assert_eq!(
            raster.at(0, 0)[3],
            255,
            "and the image is drawn, not hidden"
        );
    }

    /// A minimal JPEG of this size, as `libjpeg` accepts it: quantisation tables, Huffman
    /// tables, a start-of-frame marker and a start-of-scan, then no scan data at all.
    ///
    /// Written here rather than pasted in as a byte string so that the size under test is a
    /// number in the test rather than two bytes of somebody else's file. Verified against
    /// `djpeg`, which reads an 8 by 8 of these and complains only about the missing data.
    fn tiny_jpeg(width: u16, height: u16) -> Vec<u8> {
        let mut out: Vec<u8> = vec![0xff, 0xd8];
        let marker = |out: &mut Vec<u8>, m: u8, body: &[u8]| {
            out.extend_from_slice(&[0xff, m]);
            out.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
            out.extend_from_slice(body);
        };
        // One 8-bit quantisation table of ones.
        let mut dqt = vec![0u8];
        dqt.extend_from_slice(&[1u8; 64]);
        marker(&mut out, 0xdb, &dqt);
        // The standard luminance Huffman tables, which is the smallest pair that decodes.
        marker(
            &mut out,
            0xc4,
            &[
                0x00, 0x00, 0x01, 0x05, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a,
                0x0b,
            ],
        );
        let mut ac: Vec<u8> = vec![
            0x10, 0x00, 0x02, 0x01, 0x03, 0x03, 0x02, 0x04, 0x03, 0x05, 0x05, 0x04, 0x04, 0x00,
            0x00, 0x01, 0x7d,
        ];
        ac.extend(0..=0xa1u8);
        marker(&mut out, 0xc4, &ac);
        // SOF0: three components, the first sampled 2×2 so that luma gets a full MCU.
        let mut sof = vec![8u8];
        sof.extend_from_slice(&height.to_be_bytes());
        sof.extend_from_slice(&width.to_be_bytes());
        sof.extend_from_slice(&[3, 1, 0x22, 0, 2, 0x11, 0, 3, 0x11, 0]);
        marker(&mut out, 0xc0, &sof);
        marker(
            &mut out,
            0xda,
            &[3, 1, 0x00, 2, 0x11, 3, 0x11, 0x00, 0x3f, 0],
        );
        out.extend_from_slice(&[0u8; 32]);
        out.extend_from_slice(&[0xff, 0xd9]);
        out
    }

    /// A JPEG whose header names a size above the bound, which is all it takes.
    ///
    /// `decode` allocates `width × height × 3` up front from the frame header, so a file is a
    /// request for that much memory with a header and nothing behind it — and 192 MB is
    /// nothing for a file of three hundred bytes to ask for. The bound is asked of the header
    /// alone, before any of it, which is the only place the question can be asked.
    #[test]
    fn a_jpeg_header_above_the_bound_is_refused_before_the_pixels_are_decoded() {
        let mut dict = Dict::new();
        dict.set("Width", Object::Int(10000));
        dict.set("Height", Object::Int(10000));
        dict.set("ColorSpace", Object::name("DeviceRGB"));
        dict.set("BitsPerComponent", Object::Int(8));
        dict.set("Filter", Object::name("DCTDecode"));
        // The header claims 10000 by 10000 and the dictionary agrees, so nothing about this
        // file is a lie: it is simply a request for 300 MB of pixels.
        let stream = Stream::new(dict, tiny_jpeg(10000, 10000));

        let before = std::time::Instant::now();
        let mut notes = Vec::new();
        let decoded = decode(&stream, &|o| Some(o.clone()), &mut notes);
        let elapsed = before.elapsed();
        assert!(decoded.is_none(), "a header that big is refused");
        assert!(
            said(&notes).contains("above the"),
            "and refused on the bound rather than on the file having no scan data: {notes:?}"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "300 MB of RGB is not allocated in the time it took to read a frame header: \
             {elapsed:?}"
        );
    }

    /// The boundary on the codec path too, for the same reason: exactly at the bound is
    /// inside it.
    #[test]
    fn a_jpeg_header_exactly_at_the_bound_is_inside_it() {
        // 8192 by 8192 is 64 · 1024 · 1024 samples and the bytes are 192 MB, so this must not
        // be decoded — the assertion is on the *reason* rather than on the outcome, exactly
        // as it is for a mask.
        let mut dict = Dict::new();
        dict.set("Width", Object::Int(8192));
        dict.set("Height", Object::Int(8192));
        dict.set("Filter", Object::name("DCTDecode"));
        let mut notes = Vec::new();
        let _ = decode(
            &Stream::new(dict, tiny_jpeg(8192, 8192)),
            &|o| Some(o.clone()),
            &mut notes,
        );
        assert!(
            !said(&notes).contains("above the"),
            "8192 by 8192 is at the bound and inside it: {notes:?}"
        );
    }

    /// **The bound is asked before the mask's stream is decoded, and this measures that.**
    ///
    /// The mask claims 34862 by 4332 pixels — 151 million samples — and its stream inflates
    /// to 400 MB, so a renderer that decoded the stream first and refused afterwards grows its
    /// address space by 400 MB doing it. Asserting only that the mask was *refused* would pass
    /// equally well for that renderer, and the allocation is the whole of what is at stake, so
    /// the assertion is on a process's own peak virtual size.
    ///
    /// It runs in a child because the parent has to build the bomb, and a process that has
    /// held 400 MB of zeroes has already reached the peak being measured. The child is this
    /// same test binary with one environment variable set, so there is no second program to
    /// keep in step with this one.
    #[test]
    fn a_mask_above_the_bound_does_not_grow_the_address_space() {
        fn peak() -> u64 {
            std::fs::read_to_string("/proc/self/status")
                .unwrap_or_default()
                .lines()
                .find_map(|l| l.strip_prefix("VmPeak:"))
                .and_then(|v| v.split_whitespace().next())
                .and_then(|v| v.parse().ok())
                .unwrap_or(0)
        }
        let bomb = std::env::temp_dir().join("mangle-mask-bomb.bin");
        if let Ok(path) = std::env::var("MANGLE_MASK_BOMB") {
            // The child: build the mask from the file and decode it, reporting the growth.
            let packed = std::fs::read(&path).expect("the bomb the parent wrote");
            let mut dict = Dict::new();
            dict.set("Width", Object::Int(34862));
            dict.set("Height", Object::Int(4332));
            dict.set("BitsPerComponent", Object::Int(1));
            dict.set("ColorSpace", Object::name("DeviceGray"));
            dict.set("Filter", Object::name("FlateDecode"));
            let mask = Stream::new(dict, packed);
            let image = with_mask(2, 2, vec![255; 24], 8, &mask);
            let before = peak();
            let mut notes = Vec::new();
            let raster =
                decode(&image, &|o| Some(o.clone()), &mut notes).expect("the image decodes");
            let grew = peak().saturating_sub(before);
            println!(
                "CHILD grew {grew} mask {:?} said {:?}",
                raster.soft_mask.map(|m| m.width),
                said(&notes)
            );
            return;
        }
        // The parent: 400 MB of zeroes, deflated, handed over and dropped.
        let zeros = vec![0u8; 400 * 1024 * 1024];
        let packed = mangle_filters::deflate(&zeros, mangle_filters::DeflateLevel::Default);
        assert!(
            packed.len() < 4 * 1024 * 1024,
            "400 MB of zeroes must compress to something a file could carry: {} bytes",
            packed.len()
        );
        drop(zeros);
        std::fs::write(&bomb, &packed).expect("write the bomb");
        let out = std::process::Command::new(std::env::current_exe().expect("this binary"))
            .args([
                "--exact",
                "image::tests::a_mask_above_the_bound_does_not_grow_the_address_space",
                "--nocapture",
            ])
            .env("MANGLE_MASK_BOMB", &bomb)
            .output()
            .expect("the child runs");
        let child = String::from_utf8_lossy(&out.stdout);
        let grew = child
            .lines()
            .find_map(|l| l.strip_prefix("CHILD grew "))
            .and_then(|v| v.split_whitespace().next())
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or_else(|| {
                panic!("the child said nothing about its growth:\n{child}");
            });
        assert!(
            child.contains("soft mask"),
            "the mask is refused, and by name: {child}"
        );
        assert!(
            grew < 64 * 1024,
            "the mask's stream inflates to 400 MB and the child's address space grew {grew} kB, \
             so the stream was decoded before the bound was asked"
        );
        let _ = std::fs::remove_file(&bomb);
    }

    /// How many bytes a picture needs, for the two layouts that differ: packed per row, or
    /// one byte per sample because a codec said so.
    #[test]
    fn a_pictures_byte_count_is_padded_per_row() {
        let eight = SampleRange::declared(8);
        assert_eq!(sample_bytes(2, 2, 3, eight), 12, "two RGB rows of three");
        // Three one-bit samples is one byte per row, so four rows is four bytes rather than
        // the two a whole-image count would give.
        assert_eq!(sample_bytes(3, 4, 1, SampleRange::declared(1)), 4);
        // A fax decoder wrote one byte per pixel whatever the dictionary said.
        assert_eq!(sample_bytes(3, 4, 1, SampleRange::expanded(1)), 12);
        assert_eq!(
            sample_bytes(2, 2, 1, SampleRange::stencil(false)),
            2,
            "a stencil is one bit per sample, so two rows of two is a byte each"
        );
        assert_eq!(
            sample_bytes(2, 2, 1, SampleRange::stencil(true)),
            4,
            "and one byte per sample rather than one bit, when a codec widened the layout"
        );
    }

    /// A two-by-two raster whose four samples are four different colours, so every corner of it
    /// is distinguishable from the other three.
    fn four_colours() -> Raster {
        Raster {
            width: 2,
            height: 2,
            // Row zero is the raster's *first* row, which is the top of the image.
            pixels: vec![
                255, 0, 0, 255, // top left: red
                0, 255, 0, 255, // top right: green
                0, 0, 255, 255, // bottom left: blue
                255, 255, 0, 255, // bottom right: yellow
            ],
            interpolate: false,
            soft_mask: None,
            key_range: None,
            is_stencil: false,
        }
    }

    fn blank(width: usize, height: usize) -> crate::Image {
        crate::Image::filled(width, height, [255, 255, 255, 255])
    }

    /// An image is written at the device position its matrix names, not at the origin of the
    /// area it was clipped to.
    ///
    /// The clipped area of an image that does not start at the page's own corner starts
    /// somewhere else, and subtracting that is what put every translated image at the page
    /// origin. The matrix here names a square in the middle of a larger sheet, and every
    /// pixel of it has to appear on the sheet at the four device positions the square covers.
    #[test]
    fn an_image_is_written_where_its_matrix_says() {
        let mut device = Device::new(blank(6, 6));
        // The unit square onto device rows 2..4 and columns 2..4, with the canvas's y axis
        // counting down: `f` is the row the unit square's *bottom* edge lands on.
        let square = Matrix::new(2.0, 0.0, 0.0, -2.0, 2.0, 4.0);
        assert!(draw(&mut device, &four_colours(), &square, 1.0, None));
        let paper = device.into_image();
        for y in 0..6 {
            for x in 0..6 {
                let inside = (2..=3).contains(&x) && (2..=3).contains(&y);
                let want = if !inside {
                    [255, 255, 255, 255]
                } else if y < 3 {
                    if x < 3 {
                        [255, 0, 0, 255]
                    } else {
                        [0, 255, 0, 255]
                    }
                } else if x < 3 {
                    [0, 0, 255, 255]
                } else {
                    [255, 255, 0, 255]
                };
                assert_eq!(paper.get(x, y), Some(want), "at ({x}, {y})");
            }
        }
    }

    /// A quarter turn carries the raster's first row to the side the matrix names.
    ///
    /// This is the case that a fix which turned the raster buffer over would get wrong: the
    /// buffer and the mapping agree on every image drawn square to the page, and they disagree
    /// here, because the turn belongs to the placement and a flipped buffer is not carried by
    /// one.
    ///
    /// The matrix maps the unit square onto the whole sheet with its x axis along the device's
    /// y and its y axis along the device's x turned over, so `v = 0` — the raster's first row
    /// — is the sheet's leftmost column and `u = 0` is the sheet's topmost row. Every corner
    /// of the raster is therefore in a different quadrant of the sheet, and each of the four
    /// is asserted by name.
    #[test]
    fn a_turned_image_samples_the_right_way_round() {
        const RED: [u8; 4] = [255, 0, 0, 255];
        const GREEN: [u8; 4] = [0, 255, 0, 255];
        const BLUE: [u8; 4] = [0, 0, 255, 255];
        const YELLOW: [u8; 4] = [255, 255, 0, 255];
        let mut device = Device::new(blank(4, 4));
        let turned = Matrix::new(0.0, 4.0, -4.0, 0.0, 4.0, 0.0);
        assert!(draw(&mut device, &four_colours(), &turned, 1.0, None));
        let paper = device.into_image();
        for y in 0..4 {
            for x in 0..4 {
                // `v` is the device's x and `u` the device's y, so the raster's four quadrants
                // are the sheet's four quadrants with the two axes exchanged: red — the
                // raster's top left — is the sheet's upper left, and the raster's first row
                // runs down the sheet's left column from its top.
                let want = match (x < 2, y < 2) {
                    (true, true) => RED,
                    (false, true) => BLUE,
                    (true, false) => GREEN,
                    (false, false) => YELLOW,
                };
                assert_eq!(paper.get(x, y), Some(want), "at ({x}, {y})");
            }
        }
    }

    /// The same image drawn through two translations lands in two places.
    ///
    /// One image at one translation could be put in the wrong place by a defect that displaces
    /// every image by the same amount; two translations cannot, because the displacement would
    /// have to be the same for both and these are four rows apart.
    #[test]
    fn two_translations_land_in_two_places() {
        let red = [255, 0, 0, 255];
        let blue = [0, 0, 255, 255];
        let paper = [255, 255, 255, 255];
        let mut drawn = Vec::new();
        for f in [2.0, 6.0] {
            let mut device = Device::new(blank(8, 8));
            let square = Matrix::new(2.0, 0.0, 0.0, -2.0, 2.0, f);
            assert!(draw(&mut device, &four_colours(), &square, 1.0, None));
            drawn.push(device.into_image());
        }
        let (first, second) = (&drawn[0], &drawn[1]);
        assert_eq!(
            first.get(2, 0),
            Some(red),
            "the upper image's top row is its first"
        );
        assert_eq!(
            first.get(2, 1),
            Some(blue),
            "and its next row is its second"
        );
        assert_eq!(
            second.get(2, 4),
            Some(red),
            "the lower image's top row is its first"
        );
        assert_eq!(
            second.get(2, 5),
            Some(blue),
            "and its next row is its second"
        );
        // And neither one is where the other one is, which is the part a displacement of every
        // image by the same amount cannot produce.
        for x in 0..8 {
            for y in 0..4 {
                assert_eq!(
                    second.get(x, y),
                    Some(paper),
                    "({x}, {y}) is paper in the second"
                );
                assert_eq!(
                    first.get(x, y + 2),
                    Some(paper),
                    "({x}, {y}) is paper in the first"
                );
            }
        }
    }
}
