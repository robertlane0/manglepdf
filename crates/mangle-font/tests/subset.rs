//! A subsetter checked against an independent reader and the original outlines.
//!
//! The unit tests in `subset.rs` prove the `cmap` reader against hand-built bytes. These
//! prove the whole subset against a real font: the output parses in `ttf-parser` (which
//! shares no code with the subsetter), every kept character maps, every kept outline is
//! identical to the original's, and the file checksum validates.

#![forbid(unsafe_code)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use mangle_font::{Program, glyph_for_unicode, subset_truetype};

fn liberation() -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/fonts/LiberationSans-Regular.ttf");
    std::fs::read(&path).expect("the bundled face is checked in")
}

fn chars(text: &str) -> Vec<u32> {
    text.chars().map(u32::from).collect()
}

/// The new glyph number of each character: the subset renumbers, so a test that wants
/// an outline has to ask where it went rather than assume.
fn renumbered(font: &[u8], want: &[u32]) -> Vec<(u32, u32)> {
    want.iter()
        .map(|code| {
            (
                *code,
                glyph_for_unicode(font, *code)
                    .expect("readable")
                    .expect("kept"),
            )
        })
        .collect()
}

#[test]
fn a_subset_parses_and_holds_what_was_asked() {
    let data = liberation();
    let want = chars("Hello, World!");
    let out = subset_truetype(&data, &want).expect("ordinary Latin subsets");
    let face = ttf_parser::Face::parse(&out, 0).expect("a font");
    // Twelve distinct letters and marks, plus .notdef, plus whatever composites pull in.
    let mut distinct = want.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert!(
        u32::from(face.number_of_glyphs()) > distinct.len() as u32,
        "every character kept, and .notdef with them"
    );
    assert!(
        u32::from(face.number_of_glyphs()) < 200,
        "a subset, not the whole font: {} glyphs",
        face.number_of_glyphs()
    );
    // And the same reader agrees with ttf-parser on every kept character: two readers
    // from different hands answering the same question.
    for code in want {
        let mine = glyph_for_unicode(&out, code).expect("readable");
        let theirs = face
            .glyph_index(char::from_u32(code).expect("BMP"))
            .map(|g| u32::from(g.0));
        assert_eq!(mine, theirs, "U+{code:04X}");
    }
}

#[test]
fn kept_outlines_are_identical_to_the_originals() {
    let data = liberation();
    // Accented Latin on purpose: several of these are composite glyphs, whose remapped
    // component numbers are the one thing a subsetter can get quietly wrong.
    let want = chars("Héllo À Ögâché");
    let out = subset_truetype(&data, &want).expect("accented Latin subsets");
    let old_ids = renumbered(&data, &want);
    let new_ids = renumbered(&out, &want);
    let mut original = Program::new(data);
    let mut subset = Program::new(out);
    for ((code, old), (_, new)) in old_ids.iter().zip(new_ids.iter()) {
        let (old_outline, _) = original.outline(*old).expect("an outline");
        let (new_outline, _) = subset.outline(*new).expect("an outline");
        // A space is correctly blank; everything else must be ink, or the equality
        // below is vacuous.
        if !char::from_u32(*code).is_some_and(|c| c.is_whitespace()) {
            assert!(!new_outline.is_empty(), "U+{code:04X} is ink, not a blank");
        }
        assert_eq!(
            new_outline, old_outline,
            "U+{code:04X} draws exactly as it did"
        );
    }
}

#[test]
fn the_file_checksum_validates() {
    let data = liberation();
    let out = subset_truetype(&data, &chars("Checksum")).expect("subsets");
    let sum: u32 = out
        .chunks(4)
        .map(|c| {
            let mut word = [0u8; 4];
            for (slot, byte) in word.iter_mut().zip(c.iter()) {
                *slot = *byte;
            }
            u32::from_be_bytes(word)
        })
        .fold(0u32, |a, b| a.wrapping_add(b));
    assert_eq!(
        sum, 0xB1B0_AFBA,
        "a file that sums wrong is a file no reader trusts"
    );
}

#[test]
fn subsetting_twice_draws_the_same_glyphs() {
    let data = liberation();
    let want = chars("Stable?");
    let once = subset_truetype(&data, &want).expect("subsets");
    let twice = subset_truetype(&once, &want).expect("a subset subsets");
    // The second output renumbers an already-small font, so the bytes may differ while
    // the outlines must not: same characters, same shapes.
    let first_ids = renumbered(&once, &want);
    let second_ids = renumbered(&twice, &want);
    let mut first = Program::new(once);
    let mut second = Program::new(twice);
    for ((code, old), (_, new)) in first_ids.iter().zip(second_ids.iter()) {
        let (before, _) = first.outline(*old).expect("an outline");
        let (after, _) = second.outline(*new).expect("an outline");
        assert_eq!(before, after, "U+{code:04X} survives a second subset");
    }
}
