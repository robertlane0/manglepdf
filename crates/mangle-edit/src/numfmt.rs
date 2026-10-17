//! Numbers written into stream text.
//!
//! [`fmt_num`] is for content this crate generates: annotation appearances, flattened pages,
//! form appearances and reflowed text. A surgical edit that has to read like its producer's own
//! bytes writes numbers another way, which is `edits::num`.

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
