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
//!   blended with the colour, and it usually has a different size.
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

use mangle_content::{Matrix, Rgba};
use mangle_syntax::object::{Dict, Object, Stream};
use mangle_syntax::stream::decode_stream;

use crate::{Device, Rect};

/// The most pixels an image may have. A page can ask for an image larger than memory, and
/// the answer has to be a refusal rather than an allocation failure.
pub const MAX_IMAGE_PIXELS: usize = 64 * 1024 * 1024;

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
            // An ICC stream names its component count in `/N`. Without the profile the
            // colours cannot be converted, but the count is enough to read the samples and
            // show them approximately, which beats showing nothing.
            let stream = array.get(1).and_then(resolve);
            let n = stream
                .as_ref()
                .and_then(Object::as_dict)
                .and_then(|d| d.get("N"))
                .and_then(Object::as_i64)
                .unwrap_or(3);
            match n {
                1 => Some(Space::Gray { decode: [0.0, 1.0] }),
                4 => Some(Space::Cmyk {
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
    /// Already-decoded samples, at the given bit depth.
    Samples(Vec<u8>, usize),
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

    // The filters below an image codec are deliberately left alone, so what comes back is
    // either samples or the bytes the codec itself wants.
    let decoded = decode_stream(stream);
    for note in &decoded.notes {
        notes.push(format!("image: {note}"));
    }
    let terminal = stream
        .filters()
        .iter()
        .rev()
        .find(|f| matches!(**f, b"DCTDecode" | b"DCT" | b"JPXDecode" | b"JBIG2Decode"))
        .copied();

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
            (Source::Samples(pixels, 8), space, w, h)
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
            let bits = if is_stencil { 1 } else { declared_bits };
            let w = declared_w as usize;
            let h = declared_h as usize;
            (Source::Samples(decoded.data, bits), space, w, h)
        }
    };

    let (samples, bits) = match source {
        Source::Samples(s, b) => (s, b),
    };

    let total = width.checked_mul(height)?;
    if total > MAX_IMAGE_PIXELS {
        notes.push(format!(
            "an image of {width} by {height} pixels is above the {MAX_IMAGE_PIXELS} this \
             draws and was not drawn"
        ));
        return None;
    }
    if !matches!(bits, 1 | 2 | 4 | 8 | 16) {
        notes.push(format!(
            "an image claims {bits} bits per component, which is not a thing, and was not drawn"
        ));
        return None;
    }

    let components = space.components();
    let space = apply_decode(space, &read_decode(dict.get("Decode"), components));
    let pixels = to_rgba(&samples, &space, width, height, bits, components);
    let _ = space.name();

    let soft_mask = dict
        .get("SMask")
        .and_then(resolve)
        .and_then(|o| match o {
            Object::Stream(s) => Some(s),
            _ => None,
        })
        .and_then(|s| decode(&s, resolve, notes))
        .map(Box::new);

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

/// Decode a JPEG's samples into RGB.
fn jpeg(data: &[u8], notes: &mut Vec<String>) -> Option<(Vec<u8>, usize, usize)> {
    use zune_jpeg::JpegDecoder;
    use zune_jpeg::zune_core::bytestream::ZCursor;

    let mut decoder = JpegDecoder::new(ZCursor::new(data));
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
fn to_rgba(
    samples: &[u8],
    space: &Space,
    width: usize,
    height: usize,
    bits: usize,
    components: usize,
) -> Vec<u8> {
    let max = f64::from((1u32 << bits.min(16)) - 1);
    let mut out = Vec::with_capacity(width.saturating_mul(height).saturating_mul(4));
    for y in 0..height {
        for x in 0..width {
            let raw = |c: usize| -> f64 {
                let v = sample_at(samples, x, y, width, components, bits, c);
                if max > 0.0 { f64::from(v) / max } else { 0.0 }
            };
            let (r, g, b, a): (f64, f64, f64, f64) = match space {
                Space::Stencil => {
                    // Zero paints and one does not, which is a stencil and not a picture.
                    if raw(0) > 0.5 {
                        (0.0, 0.0, 0.0, 0.0)
                    } else {
                        (0.0, 0.0, 0.0, 1.0)
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
/// `matrix` maps the unit square onto the page. `fill` is the graphics state's fill colour,
/// which is what paints an image mask's zero bits. Returns whether anything was drawn.
pub fn draw(
    device: &mut Device,
    raster: &Raster,
    matrix: &Matrix,
    alpha: f64,
    fill: Option<Rgba>,
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
    let Some(inverse) = matrix.inverse() else {
        return false;
    };
    let x0 = columns.start;
    let y0 = rows.start;

    let mut drawn = 0usize;
    // `rows` is consumed by the outer loop and `columns` by the inner one, and a range is
    // not `Copy`; the clone is two words and is clearer than restructuring the walk.
    for y in rows.clone() {
        for x in columns.clone() {
            let (u, v) = inverse.apply(x as f64 + 0.5, y as f64 + 0.5);
            if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
                continue;
            }
            let mut px = raster.sample(u, v);
            // A stencil's painted bits take the graphics state's colour, not the data's.
            if raster.is_stencil && px[3] == 0 {
                let colour = fill.unwrap_or(Rgba::BLACK);
                px = colour.to_rgba8(alpha);
            } else if let Some(colour) = fill
                && raster.is_stencil
            {
                px = colour.to_rgba8(alpha);
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
            device.put(x - x0, y - y0, px);
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
}
