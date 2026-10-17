//! Numbers written into stream text.
//!
//! There are two formatters because there are two jobs, and they are not interchangeable: a
//! number with more than six decimals comes out differently from each.
//!
//! [`num`] is for a surgical edit, which has to read like its producer's own bytes.
//!
//! [`fmt_num`] is for content this crate generates: annotation appearances, flattened pages,
//! form appearances and reflowed text.

/// One number as a stream would write it.
///
/// Rust's shortest round-trip form is what a producer's own numbers look like: `0.1` stays
/// `0.1`, and a whole number has no `.0` on it. Negative zero is normalised, because no
/// producer writes `-0` and a diff reads it as a change from `0`.
pub(crate) fn num(v: f64) -> String {
    if v == 0.0 {
        return "0".to_string();
    }
    format!("{v}")
}

/// A number as generated content writes it: fixed notation rounded to six decimals, with the
/// trailing zeros and a bare point trimmed.
///
/// Anything that is not finite, or is too small to show, is `0`. The result is never `NaN`,
/// `inf` or `-0`.
pub(crate) fn fmt_num(v: f64) -> String {
    if !v.is_finite() || v.abs() < 1e-9 {
        return "0".to_string();
    }
    let rounded = (v * 1_000_000.0).round() / 1_000_000.0;
    let s = format!("{rounded:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" {
        "0".to_string()
    } else {
        s.to_string()
    }
}
