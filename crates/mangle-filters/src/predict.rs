//! Predictors (ISO 32000-1 7.4.4.4): PNG predictors 10-15 and the TIFF predictor 2.

// Direct indexing is used throughout this file: every index is either masked to a
// table width or produced by a loop bounded by the length of the same buffer, so a
// checked access would add noise without adding safety. The surrounding code is
// still panic-free: see docs/PDF-QUIRKS.md for the callers' tolerance rules.
#![allow(clippy::indexing_slicing)]

use crate::FilterResult;
use crate::error::FilterError;

/// `/DecodeParms` for a predictor filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PredictorParams {
    /// 1 = none, 2 = TIFF, 10..=15 = PNG.
    pub predictor: u16,
    /// `/Colors`: components per sample, 1..=32.
    pub colors: u8,
    /// `/BitsPerComponent`.
    pub bpc: u8,
    /// `/Columns`: samples per row.
    pub columns: u16,
}

/// Bytes per pixel for a predictor, i.e. `/Colors * ceil(/BitsPerComponent / 8)`.
#[must_use]
pub(crate) fn bytes_per_pixel(p: &PredictorParams) -> usize {
    let colors = usize::from(p.colors.max(1));
    let bpp_bits = usize::from(p.bpc.max(1));
    let bytes = bpp_bits.div_ceil(8);
    colors * bytes.max(1)
}

/// Row length in bytes, rounded up to whole bytes.
#[must_use]
pub(crate) fn row_length(p: &PredictorParams) -> usize {
    let colors = usize::from(p.colors.max(1));
    let bpp = usize::from(p.bpc.max(1));
    let bits = colors * bpp * usize::from(p.columns.max(1));
    bits.div_ceil(8)
}

/// Apply the inverse predictor.
pub fn unpredict(data: &[u8], p: &PredictorParams) -> FilterResult<Vec<u8>> {
    match p.predictor {
        0 | 1 => Ok(data.to_vec()),
        2 => tiff_unpredict(data, p),
        10..=15 => png_unpredict(data, p),
        other => Err(FilterError::BadParameters {
            filter: "Predictor",
            reason: format!("unsupported predictor {other}"),
        }),
    }
}

/// TIFF predictor 2: horizontal differencing, always 8-bit components.
fn tiff_unpredict(data: &[u8], p: &PredictorParams) -> FilterResult<Vec<u8>> {
    if p.bpc != 8 {
        return Err(FilterError::BadParameters {
            filter: "Predictor",
            reason: format!("TIFF predictor 2 needs 8 bits per component, got {}", p.bpc),
        });
    }
    let mut out = data.to_vec();
    let bpp = bytes_per_pixel(p);
    let row = row_length(p);
    if bpp == 0 {
        return Ok(out);
    }
    for r in (0..out.len()).step_by(row.max(1)) {
        let row_end = (r + row).min(out.len());
        if row_end <= r + bpp {
            continue;
        }
        for i in (r + bpp)..row_end {
            let prev = out.get(i - bpp).copied().unwrap_or(0);
            if let Some(cur) = out.get_mut(i) {
                *cur = cur.wrapping_add(prev);
            }
        }
    }
    Ok(out)
}

/// PNG predictors: each row is preceded by a filter-type byte.
// The rows are addressed as `a`, `b`, `c` because that is how the PNG specification
// names the neighbours used by the Paeth filter.
#[allow(clippy::many_single_char_names)]
fn png_unpredict(data: &[u8], p: &PredictorParams) -> FilterResult<Vec<u8>> {
    let bpp = bytes_per_pixel(p);
    let row = row_length(p);
    if row == 0 {
        return Ok(Vec::new());
    }
    let stride = row + 1;
    let rows = data.len().div_ceil(stride);
    let mut out = vec![0u8; rows.saturating_mul(row)];
    let mut prev_row = vec![0u8; row];

    for r in 0..rows {
        let Some(&ft) = data.get(r * stride) else {
            break;
        };
        let src = data.get((r * stride + 1)..(r * stride + stride));
        let Some(src) = src else { break };
        let n = src.len().min(row);
        let mut cur = vec![0u8; row];
        cur[..n].copy_from_slice(&src[..n]);

        match ft {
            0 => {}
            1 => {
                for i in bpp..n {
                    let l = cur.get(i - bpp).copied().unwrap_or(0);
                    if let Some(c) = cur.get_mut(i) {
                        *c = c.wrapping_add(l);
                    }
                }
            }
            2 => {
                for i in 0..n {
                    let u = prev_row.get(i).copied().unwrap_or(0);
                    if let Some(c) = cur.get_mut(i) {
                        *c = c.wrapping_add(u);
                    }
                }
            }
            3 => {
                for i in 0..n {
                    let left = if i >= bpp {
                        *cur.get(i - bpp).unwrap_or(&0)
                    } else {
                        0
                    };
                    let up = *prev_row.get(i).unwrap_or(&0);
                    if let Some(c) = cur.get_mut(i) {
                        *c = c.wrapping_add(u16::midpoint(u16::from(left), u16::from(up)) as u8);
                    }
                }
            }
            4 => {
                for i in 0..n {
                    let a = if i >= bpp {
                        *cur.get(i - bpp).unwrap_or(&0)
                    } else {
                        0
                    };
                    let b = *prev_row.get(i).unwrap_or(&0);
                    let c = if i >= bpp {
                        *prev_row.get(i - bpp).unwrap_or(&0)
                    } else {
                        0
                    };
                    if let Some(slot) = cur.get_mut(i) {
                        *slot = slot.wrapping_add(paeth(a, b, c));
                    }
                }
            }
            other => {
                return Err(FilterError::BadParameters {
                    filter: "Predictor",
                    reason: format!("PNG filter type {other} is not defined"),
                });
            }
        }
        if let Some(slot) = out.get_mut(r * row..(r + 1) * row) {
            slot.copy_from_slice(&cur[..row.min(row)]);
        }
        prev_row.copy_from_slice(&cur);
    }
    Ok(out)
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = i16::from(a) + i16::from(b) - i16::from(c);
    let pa = (p - i16::from(a)).abs();
    let pb = (p - i16::from(b)).abs();
    let pc = (p - i16::from(c)).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Apply the forward predictor. Used by the PNG writer and by round-trip tests.
#[allow(clippy::many_single_char_names)]
pub fn predict(data: &[u8], p: &PredictorParams) -> FilterResult<Vec<u8>> {
    match p.predictor {
        0 | 1 => Ok(data.to_vec()),
        2 => {
            let bpp = bytes_per_pixel(p);
            let row = row_length(p);
            let mut out = data.to_vec();
            for r in (0..out.len()).step_by(row.max(1)) {
                let row_end = (r + row).min(out.len());
                for i in (r + bpp..row_end).rev() {
                    let prev = out.get(i - bpp).copied().unwrap_or(0);
                    if let Some(cur) = out.get_mut(i) {
                        *cur = cur.wrapping_sub(prev);
                    }
                }
            }
            Ok(out)
        }
        10..=15 => {
            // Always emit "Up", which is simple, effective and deterministic.
            let bpp = bytes_per_pixel(p);
            let row = row_length(p);
            if row == 0 {
                return Ok(Vec::new());
            }
            let rows = data.len().div_ceil(row);
            let mut out = Vec::with_capacity(rows * (row + 1));
            let mut prev_row = vec![0u8; row];
            for r in 0..rows {
                let start = r * row;
                let mut cur = vec![0u8; row];
                let n = data.len().saturating_sub(start).min(row);
                if n > 0 {
                    cur[..n].copy_from_slice(&data[start..start + n]);
                }
                out.push(2);
                for i in 0..row {
                    let u = *prev_row.get(i).unwrap_or(&0);
                    let l = if i >= bpp {
                        *cur.get(i - bpp).unwrap_or(&0)
                    } else {
                        0
                    };
                    let v = cur.get(i).copied().unwrap_or(0);
                    let _ = l;
                    out.push(v.wrapping_sub(u));
                }
                prev_row.copy_from_slice(&cur);
            }
            Ok(out)
        }
        other => Err(FilterError::BadParameters {
            filter: "Predictor",
            reason: format!("unsupported predictor {other}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect`, which is what a test is for; the
    // panic-free rule is about what the product does with a file, not about tests.
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::*;

    #[test]
    fn tiff_round_trip() {
        let p = PredictorParams {
            predictor: 2,
            colors: 3,
            bpc: 8,
            columns: 4,
        };
        let data: Vec<u8> = (0..36u8).collect();
        let enc = predict(&data, &p).expect("predict");
        let dec = unpredict(&enc, &p).expect("unpredict");
        assert_eq!(dec, data);
    }

    #[test]
    fn png_round_trip_all_filters() {
        // Build a five-row stream that uses every filter type, then check the result.
        let bpp = 3usize;
        let row = 12usize;
        let base: Vec<u8> = (0..(row * 5) as u8).map(|i| i.wrapping_mul(7)).collect();
        let mut stream = Vec::new();
        for ft in 0..5usize {
            let this = &base[ft * row..(ft + 1) * row];
            let prev: &[u8] = if ft == 0 {
                &[]
            } else {
                &base[(ft - 1) * row..ft * row]
            };
            stream.push(ft as u8);
            for i in 0..row {
                let a = if i >= bpp { this[i - bpp] } else { 0 };
                let b = prev.get(i).copied().unwrap_or(0);
                let c = if i >= bpp {
                    prev.get(i - bpp).copied().unwrap_or(0)
                } else {
                    0
                };
                let v = match ft {
                    0 => this[i],
                    1 => this[i].wrapping_sub(a),
                    2 => this[i].wrapping_sub(b),
                    3 => this[i].wrapping_sub(
                        u8::try_from(u16::from(a).midpoint(u16::from(b))).unwrap_or(0),
                    ),
                    _ => this[i].wrapping_sub(paeth(a, b, c)),
                };
                stream.push(v);
            }
        }
        let p = PredictorParams {
            predictor: 15,
            colors: 3,
            bpc: 8,
            columns: 4,
        };
        let dec = unpredict(&stream, &p).expect("unpredict");
        assert_eq!(dec, base);
    }

    #[test]
    fn png_predict_unpredict_round_trip() {
        let p = PredictorParams {
            predictor: 15,
            colors: 1,
            bpc: 8,
            columns: 16,
        };
        let data: Vec<u8> = (0..100u8).collect();
        let enc = predict(&data, &p).expect("predict");
        let dec = unpredict(&enc, &p).expect("unpredict");
        // The final row is zero-padded out to a whole number of rows.
        assert_eq!(dec.len(), 112);
        assert_eq!(&dec[..100], &data[..]);
    }

    #[test]
    fn unknown_filter_type_is_an_error() {
        let p = PredictorParams {
            predictor: 15,
            colors: 1,
            bpc: 8,
            columns: 4,
        };
        let r = unpredict(&[9, 1, 2, 3, 4], &p);
        assert!(r.is_err());
    }
}
