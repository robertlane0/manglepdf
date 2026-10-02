//! Metric-compatible stand-ins for the Standard 14, bundled so a document that does not
//! embed its fonts still renders at the right width.
//!
//! Liberation Sans is metric-compatible with Helvetica, Liberation Serif with Times and
//! Liberation Mono with Courier, so text laid out against the standard widths keeps its
//! line breaks and column widths. The programs here are read by `from_true_type`; they are
//! TrueType (`glyf`) outlines, not Type 1, so no CFF or Type 1 path is involved.
//!
//! A name this module cannot answer for is reported rather than guessed at. `Symbol` and
//! `ZapfDingbats` return `None`: Liberation has no equivalent face, and inventing a
//! stand-in for a symbol font would silently put the wrong glyphs on the page.

use crate::metrics::{split_name, style_words};

/// The twelve Liberation faces, keyed by the Standard 14 family they stand in for.
const HELVETICA: [(&str, &[u8]); 4] = [
    (
        "Regular",
        include_bytes!("../../../assets/fonts/LiberationSans-Regular.ttf"),
    ),
    (
        "Bold",
        include_bytes!("../../../assets/fonts/LiberationSans-Bold.ttf"),
    ),
    (
        "Italic",
        include_bytes!("../../../assets/fonts/LiberationSans-Italic.ttf"),
    ),
    (
        "BoldItalic",
        include_bytes!("../../../assets/fonts/LiberationSans-BoldItalic.ttf"),
    ),
];

const TIMES: [(&str, &[u8]); 4] = [
    (
        "Regular",
        include_bytes!("../../../assets/fonts/LiberationSerif-Regular.ttf"),
    ),
    (
        "Bold",
        include_bytes!("../../../assets/fonts/LiberationSerif-Bold.ttf"),
    ),
    (
        "Italic",
        include_bytes!("../../../assets/fonts/LiberationSerif-Italic.ttf"),
    ),
    (
        "BoldItalic",
        include_bytes!("../../../assets/fonts/LiberationSerif-BoldItalic.ttf"),
    ),
];

const COURIER: [(&str, &[u8]); 4] = [
    (
        "Regular",
        include_bytes!("../../../assets/fonts/LiberationMono-Regular.ttf"),
    ),
    (
        "Bold",
        include_bytes!("../../../assets/fonts/LiberationMono-Bold.ttf"),
    ),
    (
        "Italic",
        include_bytes!("../../../assets/fonts/LiberationMono-Italic.ttf"),
    ),
    (
        "BoldItalic",
        include_bytes!("../../../assets/fonts/LiberationMono-BoldItalic.ttf"),
    ),
];

/// The bundled font program that matches a `/BaseFont` name, or `None` when none does.
///
/// A subset prefix (`ABCDEF+`) and a style suffix are both handled by the shared
/// name-splitting in [`metrics`], so `ABCDEF+Helvetica-Bold` and `Helvetica-Bold` agree.
#[must_use]
pub fn substitute(name: &str) -> Option<&'static [u8]> {
    let (family, style) = split_name(name)?;
    let (bold, italic) = style_words(&style)?;
    let table = match family {
        "Helvetica" => &HELVETICA,
        "Times" => &TIMES,
        "Courier" => &COURIER,
        // Liberation carries no symbol or dingbat face. There is nothing metric-compatible
        // to substitute, and the project's position is to report the missing font rather
        // than invent one, so these deliberately have no entry above.
        "Symbol" | "ZapfDingbats" => return None,
        _ => return None,
    };
    let want = match (bold, italic) {
        (false, false) => "Regular",
        (true, false) => "Bold",
        (false, true) => "Italic",
        (true, true) => "BoldItalic",
    };
    table.iter().find(|(k, _)| *k == want).map(|(_, b)| *b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A TrueType sfnt version tag: `0x00010000`, or `true` for an Apple-flavoured one.
    fn is_truetype(bytes: &[u8]) -> bool {
        bytes.starts_with(&[0x00, 0x01, 0x00, 0x00]) || bytes.starts_with(b"true")
    }

    #[test]
    fn all_twelve_standard_names_resolve() {
        for name in [
            "Helvetica",
            "Helvetica-Bold",
            "Helvetica-Oblique",
            "Helvetica-BoldOblique",
            "Times-Roman",
            "Times-Bold",
            "Times-Italic",
            "Times-BoldItalic",
            "Courier",
            "Courier-Bold",
            "Courier-Oblique",
            "Courier-BoldOblique",
        ] {
            assert!(substitute(name).is_some(), "{name} should resolve");
        }
    }

    #[test]
    fn a_subset_prefix_resolves() {
        assert!(substitute("ABCDEF+Helvetica-Bold").is_some());
    }

    #[test]
    fn bold_oblique_resolves() {
        assert!(substitute("Helvetica-BoldOblique").is_some());
    }

    #[test]
    fn symbol_has_no_substitute() {
        assert!(substitute("Symbol").is_none());
    }

    #[test]
    fn zapf_dingbats_has_no_substitute() {
        assert!(substitute("ZapfDingbats").is_none());
    }

    #[test]
    fn every_program_is_a_non_empty_truetype() {
        for name in [
            "Helvetica",
            "Helvetica-Bold",
            "Helvetica-Oblique",
            "Helvetica-BoldOblique",
            "Times-Roman",
            "Times-Bold",
            "Times-Italic",
            "Times-BoldItalic",
            "Courier",
            "Courier-Bold",
            "Courier-Oblique",
            "Courier-BoldOblique",
        ] {
            let bytes = substitute(name).unwrap_or_default();
            assert!(!bytes.is_empty(), "{name} returned no bytes");
            assert!(is_truetype(bytes), "{name} is not a TrueType program");
        }
    }

    #[test]
    fn the_comma_spelling_agrees_with_the_hyphen_one() {
        assert_eq!(
            substitute("Helvetica-Bold").map(|b| b.len()),
            substitute("Helvetica,Bold").map(|b| b.len())
        );
    }

    #[test]
    fn an_unknown_family_is_reported_rather_than_guessed() {
        assert!(substitute("NoSuchFont").is_none());
        assert!(substitute("Helvetica-Extrabold").is_none());
    }
}
