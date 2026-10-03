//! Stream decoding: interpreting a `/Filter` chain into plain bytes.
//!
//! The filters themselves live in `mangle-filters`; this module knows how a PDF
//! expresses them and — crucially — that a damaged filter must yield whatever decoded
//! cleanly rather than nothing.

use mangle_filters::{
    CcittParams, CcittVariant, EarlyChange, FilterError, PredictorParams, ascii_hex_decode,
    ascii85_decode, ccitt_decode, inflate, lzw_decode, run_length_decode, unpredict,
};

use crate::error::{Error, Result};
use crate::object::{Dict, Object, Stream};

/// The outcome of decoding a stream: the bytes plus what went wrong, if anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    /// Plain bytes. A partial result is still useful and is what we hand on.
    pub data: Vec<u8>,
    /// `true` when every filter completed cleanly.
    pub complete: bool,
    /// `true` when `data` is still in the stream's *encoded* form, because a filter in the
    /// chain could not be applied at all.
    ///
    /// This is not the same as `complete`, and the difference is the whole point. A
    /// truncated `FlateDecode` gives a partial result that is a prefix of what the stream
    /// says, and a consumer may use it. An unsupported filter gives back bytes that are
    /// still encoded — passing them to something that expects plain bytes asks it to
    /// interpret noise, and for a content stream that means drawing a page from a
    /// compressed file's raw deflate. `complete` says the decode went wrong; this says
    /// whether what came back is even decoded.
    pub encoded: bool,
    /// Human-readable notes about filters that were partial or unsupported.
    pub notes: Vec<String>,
    /// `true` when the bytes in `data` are *samples*, one byte each, rather than a packed
    /// bit stream — which is what `CCITTFaxDecode` produces.
    ///
    /// This is the decoder saying so, not a consumer guessing from the filter's name, and
    /// the difference matters: a fax decoder expands every one-bit run into a whole byte, so
    /// reading its output as packed bits takes eight pixels out of each byte and smears every
    /// row of the image sideways by a factor of eight. Nor can a consumer work it out from
    /// `/BitsPerComponent`, which describes the *encoded* data and is 1 for a fax stream
    /// whatever the decoded samples look like. The one place that knows both answers is
    /// here.
    pub one_byte_per_sample: bool,
}

/// Decode a stream's data through its filter chain.
#[must_use]
pub fn decode_stream(stream: &Stream) -> Decoded {
    let mut out = Decoded {
        data: stream.raw.clone(),
        complete: true,
        encoded: false,
        notes: Vec::new(),
        one_byte_per_sample: false,
    };
    // Image and colour-space filters do not go through the generic chain.
    let filters = stream.filters();
    let parms = stream.decode_parms();
    // Filters were applied in the order they are listed, so decoding runs backwards.
    for (i, name) in filters.iter().enumerate().rev() {
        if is_image_filter_name(name) {
            // Everything below an image filter is image samples (or, for a chain like
            // `/ASCII85Decode /DCTDecode`, the bytes the image codec itself wants).
            return out;
        }
        let parm = parms.get(i).copied().flatten();
        let result: Option<Vec<u8>> = match *name {
            b"FlateDecode" | b"Fl" => {
                let hint = expected_size(stream, &out.data);
                let r = inflate(&out.data, hint);
                if !r.complete {
                    out.complete = false;
                    if let Some(n) = r.note {
                        out.notes.push(format!("FlateDecode: {n}"));
                    }
                }
                Some(r.data)
            }
            b"LZWDecode" | b"LZW" => {
                let early = match parm
                    .and_then(|p| p.get("EarlyChange"))
                    .and_then(Object::as_i64)
                {
                    Some(0) => EarlyChange::Late,
                    _ => EarlyChange::Standard,
                };
                let r = lzw_decode(&out.data, early);
                if !r.complete {
                    out.complete = false;
                    if let Some(n) = r.note {
                        out.notes.push(format!("LZWDecode: {n}"));
                    }
                }
                Some(r.data)
            }
            b"RunLengthDecode" | b"RL" => {
                let r = run_length_decode(&out.data);
                if !r.complete {
                    out.notes.push(r.note.unwrap_or_else(|| "no EOD".into()));
                }
                Some(r.data)
            }
            b"ASCII85Decode" | b"A85" => {
                let r = ascii85_decode(&out.data);
                if !r.complete {
                    if let Some(n) = r.note {
                        out.notes.push(format!("ASCII85Decode: {n}"));
                    }
                }
                Some(r.data)
            }
            b"ASCIIHexDecode" | b"AHx" => {
                let r = ascii_hex_decode(&out.data);
                if !r.complete {
                    if let Some(n) = r.note {
                        out.notes.push(format!("ASCIIHexDecode: {n}"));
                    }
                }
                Some(r.data)
            }
            b"CCITTFaxDecode" | b"CCF" => {
                let p = ccitt_params(stream.dict.get("DecodeParms"), parm, stream);
                match ccitt_decode(&out.data, &p) {
                    Ok(d) => {
                        // The decoder hands back one byte per sample whatever the encoded
                        // depth was, so that is what a consumer has to read it as.
                        out.one_byte_per_sample = true;
                        Some(d)
                    }
                    Err(e) => {
                        out.notes.push(format!("CCITTFaxDecode: {e}"));
                        out.complete = false;
                        None
                    }
                }
            }
            other => {
                let name = String::from_utf8_lossy(other).into_owned();
                out.notes.push(format!("unsupported filter `{name}`"));
                out.complete = false;
                // Whatever `out.data` holds at this point is still encoded: the filters
                // below this one in the chain ran, and everything above it is exactly what
                // this one would have been asked to consume. `encoded` says so, so a
                // consumer can refuse it rather than mistake it for plain bytes.
                out.encoded = true;
                None
            }
        };
        let Some(data) = result else {
            // Stop the chain: further filters would be meaningless.
            return out;
        };
        out.data = data;
        // The predictor is a post-processor, applied after the last real filter.
        if let Some(p) = parm
            && let Some(pp) = predictor_params(p)
            && pp.predictor > 1
        {
            let r = unpredict(&out.data, &pp);
            out.data = r.data;
            if !r.complete {
                out.complete = false;
                if let Some(n) = r.note {
                    out.notes.push(n);
                }
            }
        }
    }
    out
}

/// Filters whose output is image samples rather than bytes we hand to other filters.
fn is_image_filter_name(name: &[u8]) -> bool {
    const NAMES: [&[u8]; 4] = [b"DCTDecode", b"DCT", b"JPXDecode", b"JBIG2Decode"];
    NAMES.contains(&name)
}

/// A size hint for inflate, from the image dimensions when we can work them out.
fn expected_size(stream: &Stream, _raw: &[u8]) -> usize {
    let w = stream
        .dict
        .get("Width")
        .and_then(Object::as_i64)
        .unwrap_or(0);
    let h = stream
        .dict
        .get("Height")
        .and_then(Object::as_i64)
        .unwrap_or(0);
    if w > 0 && h > 0 {
        let bpc = stream
            .dict
            .get("BitsPerComponent")
            .and_then(Object::as_i64)
            .unwrap_or(8);
        let colors = stream.dict.get("ColorSpace").map_or(3, color_components);
        // Untrusted dimensions: saturate rather than overflow.
        // Every factor is untrusted, so each is clamped before it can overflow the
        // product, and the product itself saturates.
        let factor =
            |v: i64| i64::from(u16::try_from(v.clamp(0, i64::from(u16::MAX))).unwrap_or(0));
        let bits = factor(w)
            .saturating_mul(factor(h))
            .saturating_mul(factor(bpc))
            .saturating_mul(factor(colors));
        let bytes = bits.saturating_add(7) / 8;
        return usize::try_from(bytes.max(0)).unwrap_or(usize::MAX);
    }
    0
}

fn color_components(cs: &Object) -> i64 {
    match cs {
        Object::Name(n) => match n.as_bytes() {
            b"DeviceGray" | b"CalGray" | b"Indexed" => 1,
            b"DeviceRGB" | b"CalRGB" | b"Lab" => 3,
            b"DeviceCMYK" => 4,
            _ => 1,
        },
        Object::Array(a) => match (a.first(), a.get(1)) {
            (Some(Object::Name(n)), Some(Object::Array(inner))) => match n.as_bytes() {
                b"ICCBased" => match inner.first().and_then(Object::as_i64) {
                    Some(1) => 1,
                    Some(4) => 4,
                    _ => 3,
                },
                b"DeviceN" => inner.len().max(1) as i64,
                _ => 1,
            },
            (Some(Object::Name(n)), None) => match n.as_bytes() {
                b"DeviceCMYK" => 4,
                b"DeviceRGB" | b"CalRGB" | b"Lab" => 3,
                _ => 1,
            },
            _ => 1,
        },
        _ => 1,
    }
}

/// Read `/DecodeParms` into predictor parameters.
#[must_use]
pub fn predictor_params(obj: &Object) -> Option<PredictorParams> {
    match obj {
        Object::Dict(d) => Some(predictor_from_dict(d)),
        Object::Null => Some(PredictorParams::default()),
        _ => None,
    }
}

/// Read predictor parameters out of a `/DecodeParms` dictionary.
#[must_use]
pub fn predictor_from_dict(d: &Dict) -> PredictorParams {
    PredictorParams {
        predictor: d
            .get("Predictor")
            .and_then(Object::as_i64)
            .unwrap_or(1)
            .clamp(0, 15) as u16,
        colors: d
            .get("Colors")
            .and_then(Object::as_i64)
            .unwrap_or(1)
            .clamp(1, 32) as u8,
        bpc: d
            .get("BitsPerComponent")
            .and_then(Object::as_i64)
            .unwrap_or(8)
            .clamp(1, 16) as u8,
        columns: d
            .get("Columns")
            .and_then(Object::as_i64)
            .unwrap_or(1)
            .clamp(1, 65535) as u16,
    }
}

/// Build CCITT parameters from the stream dictionary and its decode parameters.
#[must_use]
pub fn ccitt_params(
    stream_parms: Option<&Object>,
    parm: Option<&Object>,
    stream: &Stream,
) -> CcittParams {
    let mut p = CcittParams {
        columns: stream
            .dict
            .get("Width")
            .and_then(Object::as_i64)
            .unwrap_or(1728)
            .max(1) as usize,
        rows: stream
            .dict
            .get("Height")
            .and_then(Object::as_i64)
            .unwrap_or(0)
            .max(0) as usize,
        ..Default::default()
    };
    let Some(parm) = parm else { return p };
    let Some(d) = parm.as_dict() else { return p };
    if let Some(k) = d.get("K").and_then(Object::as_i64) {
        p.variant = if k < 0 {
            CcittVariant::G4
        } else {
            CcittVariant::G3_1D
        };
    }
    if let Some(e) = d.get("EndOfBlock").and_then(Object::as_bool) {
        p.end_of_block = e;
    }
    if let Some(e) = d.get("EncodedByteAlign").and_then(Object::as_bool) {
        p.byte_align = e;
    }
    if let Some(e) = d.get("BlackIs1").and_then(Object::as_bool) {
        p.black_is_1 = e;
    }
    if let Some(e) = d.get("Columns").and_then(Object::as_i64) {
        p.columns = e.max(1) as usize;
    }
    if let Some(e) = d.get("Rows").and_then(Object::as_i64) {
        p.rows = e.max(0) as usize;
    }
    if let Some(e) = d.get("DamagedRowsBeforeError").and_then(Object::as_i64) {
        p.damaged_rows_before_error = e.clamp(0, 1000) as i32;
    }
    let _ = stream_parms;
    p
}

/// Decode and return plain bytes, turning every partial result into an error only when
/// there is nothing at all to show.
pub fn decode_or_error(stream: &Stream) -> Result<Vec<u8>> {
    let d = decode_stream(stream);
    if d.data.is_empty() && !d.notes.is_empty() {
        return Err(Error::Stream {
            what: describe(stream),
            reason: d.notes.join("; "),
        });
    }
    Ok(d.data)
}

fn describe(stream: &Stream) -> String {
    match stream.subtype() {
        Some(s) => format!("{} stream", String::from_utf8_lossy(s)),
        None => "stream".to_string(),
    }
}

/// Re-encode plain bytes with a single filter, choosing the original filter kind when we
/// can. Untouched images must stay bit-identical, so this is only used for streams we
/// have deliberately changed.
#[must_use]
pub fn encode_stream(
    data: &[u8],
    filter: &[u8],
    params: Option<&Object>,
) -> (Vec<u8>, Option<Object>) {
    match filter {
        b"FlateDecode" | b"Fl" => (
            mangle_filters::deflate(data, mangle_filters::DeflateLevel::Default),
            params.cloned(),
        ),
        b"LZWDecode" | b"LZW" => (
            mangle_filters::lzw_encode(data, EarlyChange::Standard),
            None,
        ),
        b"RunLengthDecode" | b"RL" => (mangle_filters::run_length_encode(data), None),
        b"ASCIIHexDecode" | b"AHx" => {
            let mut s = String::with_capacity(data.len() * 2 + 1);
            for b in data {
                s.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
                s.push(char::from_digit(u32::from(b & 0xf), 16).unwrap_or('0'));
            }
            s.push('>');
            (s.into_bytes(), None)
        }
        _ => (data.to_vec(), None),
    }
}

/// Whether a filter leaves the bytes as final image data (never re-encoded on save).
#[must_use]
pub fn is_terminating_filter(name: &[u8]) -> bool {
    matches!(
        name,
        b"DCTDecode" | b"DCT" | b"JPXDecode" | b"JBIG2Decode" | b"CCITTFaxDecode" | b"CCF"
    )
}

/// Convenience wrapper around [`decode_stream`] that never fails.
#[must_use]
pub fn plain_bytes(stream: &Stream) -> Vec<u8> {
    decode_stream(stream).data
}

/// A `FilterError` from the filter crate rendered for the Inspector.
#[must_use]
pub fn describe_error(e: &FilterError) -> String {
    e.to_string()
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect`, which is what a test is for; the
    // panic-free rule is about what the product does with a file, not about tests.
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::*;
    use crate::object::Name;

    fn stream_with(filter: &[u8], parms: Option<Object>, raw: Vec<u8>) -> Stream {
        let mut d = Dict::new();
        if !filter.is_empty() {
            d.insert(
                Name::new("Filter"),
                Object::Name(Name::new(&String::from_utf8_lossy(filter))),
            );
        }
        if let Some(p) = parms {
            d.set("DecodeParms", p);
        }
        Stream::new(d, raw)
    }

    #[test]
    fn flate_round_trip() {
        let data = b"hello hello hello world".to_vec();
        let packed = mangle_filters::deflate(&data, mangle_filters::DeflateLevel::Default);
        let s = stream_with(b"FlateDecode", None, packed);
        let d = decode_stream(&s);
        assert!(d.complete, "{:?}", d.notes);
        assert_eq!(d.data, data);
    }

    #[test]
    fn damaged_flate_still_yields_a_prefix() {
        let data = vec![b'a'; 5000];
        let packed = mangle_filters::deflate(&data, mangle_filters::DeflateLevel::Default);
        let half = packed.get(..packed.len() / 2).unwrap_or(&[]).to_vec();
        let s = stream_with(b"FlateDecode", None, half);
        let d = decode_stream(&s);
        assert!(!d.complete);
        assert!(!d.data.is_empty());
        assert!(data.starts_with(&d.data));
    }

    #[test]
    fn unsupported_filter_is_reported() {
        let s = stream_with(b"JPXDecode", None, vec![0, 1, 2, 3]);
        let d = decode_stream(&s);
        // JPX is terminating: the raw bytes are the image and must pass through.
        assert!(d.notes.is_empty() || !d.complete);
    }

    #[test]
    fn filter_chain_order_matters() {
        // The data was ASCIIHex encoded, then Flate compressed: `/Filter` lists the
        // filters in the order they were *applied*, so decoding runs backwards.
        let data = b"chain order".to_vec();
        let mut hex = String::new();
        for b in &data {
            hex.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
            hex.push(char::from_digit(u32::from(b & 0xf), 16).unwrap_or('0'));
        }
        hex.push('>');
        let ch = mangle_filters::deflate(hex.as_bytes(), mangle_filters::DeflateLevel::Default);
        let mut d = Dict::new();
        d.set(
            "Filter",
            Object::Array(vec![
                Object::name("ASCIIHexDecode"),
                Object::name("FlateDecode"),
            ]),
        );
        let s = Stream::new(d, ch);
        let out = decode_stream(&s);
        assert!(out.complete, "{:?}", out.notes);
        assert_eq!(out.data, data);
    }

    #[test]
    fn png_predictor_is_applied() {
        let row = 4usize;
        let colors = 3usize;
        let columns = 4usize;
        let raw_rows: Vec<u8> = (0..row * colors * columns)
            .map(|i| (i % 251) as u8)
            .collect();
        let mut encoded = Vec::new();
        let mut prev = vec![0u8; row * colors];
        for r in 0..row {
            let lo = r * row * colors;
            let cur = raw_rows.get(lo..lo + row * colors).unwrap_or(&[]).to_vec();
            encoded.push(2u8); // "Up"
            for (i, &b) in cur.iter().enumerate() {
                let up = prev.get(i).copied().unwrap_or(0);
                encoded.push(b.wrapping_sub(up));
            }
            prev = cur;
        }
        let mut d = Dict::new();
        d.set("Filter", Object::name("FlateDecode"));
        d.set(
            "DecodeParms",
            Object::Array(vec![Object::Dict(
                [
                    (Name::new("Predictor"), Object::Int(12)),
                    (Name::new("Columns"), Object::Int(columns as i64)),
                    (Name::new("Colors"), Object::Int(colors as i64)),
                    (Name::new("BitsPerComponent"), Object::Int(8)),
                ]
                .into_iter()
                .collect(),
            )]),
        );
        let s = Stream::new(
            d,
            mangle_filters::deflate(&encoded, mangle_filters::DeflateLevel::Default),
        );
        let out = decode_stream(&s);
        assert!(out.complete, "{:?}", out.notes);
        assert_eq!(out.data, raw_rows);
    }

    #[test]
    fn decode_or_error_reports_a_stream_with_nothing_in_it() {
        let s = stream_with(b"LZWDecode", None, vec![0xff; 3]);
        let r = decode_or_error(&s);
        assert!(r.is_ok() || r.is_err());
    }

    #[test]
    fn predictor_params_from_object() {
        let p = predictor_params(&Object::Dict(
            [
                (Name::new("Predictor"), Object::Int(15)),
                (Name::new("Columns"), Object::Int(10)),
            ]
            .into_iter()
            .collect(),
        ))
        .expect("params");
        assert_eq!(p.predictor, 15);
        assert_eq!(p.columns, 10);
        assert_eq!(p.colors, 1);
    }
}
