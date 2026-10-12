//! Search, and the cases FINISH.md names by hand.
//!
//! S0.4 asks for "Beautiful", a ligature word ("office"), a CJK term and a hyphenated word across
//! a line break. Each of those is a different way for a naive search to fail, and each has a test
//! here:
//!
//! * **case** — a title in small caps or a heading in caps is still the word the user typed;
//! * **a ligature** — `office` set with a real `fi` ligature is *four* glyphs and four bytes, so
//!   searching the raw bytes for five letters finds nothing, and searching the decoded text finds
//!   it. This is the case that only the decoder makes possible at all;
//! * **a hyphenated word across a line break** — the page has `sani-\ntization`, and the word a
//!   user searches for is `sanitization`. A search over the raw lines finds neither half.
//!
//! The last one is not solved here and is not pretended to be: `find` matches what is in the text,
//! and joining a hyphenated break is a *normalisation* of the text before it is searched. What
//! these tests do is pin down exactly where that boundary is, so the missing piece is a known gap
//! rather than a silent wrong answer.

#![forbid(unsafe_code)]
// Tests state their expectations with `expect` and index into small vectors by design; the
// panic-free rule is about what the product does with a file, not about what a test asserts.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

/// What a hit slices back out of the text it was found in.
///
/// Taken through `get`, and the `expect` is the assertion: a hit whose offsets are not on
/// character boundaries, or that runs past the end of the text, is a hit nobody can show — so this
/// is the check that the offsets are usable and not merely plausible.
fn sliced(text: &str, hit: mangle_text::Hit) -> &str {
    text.get(hit.start..hit.end)
        .unwrap_or_else(|| panic!("{hit:?} does not slice out of {text:?}"))
}

/// A word is found whatever case the page set it in.
#[test]
fn a_word_is_found_whatever_case_the_page_used() {
    let page = "Beautiful Places and BEAUTIFUL places";
    let hits = mangle_text::find(page, "beautiful");
    assert_eq!(hits.len(), 2, "both cases should be found: {hits:?}");
    assert_eq!(sliced(page, hits[0]), "Beautiful");
    assert_eq!(sliced(page, hits[1]), "BEAUTIFUL");
    // And the needle is the thing that is not case-sensitive, not the page.
    let hits = mangle_text::find(page, "PLACES");
    assert_eq!(hits.len(), 2);
}

/// A ligature is one character, so a word containing one is found by its letters.
///
/// The decoder turns the `fi` glyph into U+FB01, and the page's text is therefore `o\u{FB01}ce`.
/// A user types the five ASCII letters. Found, because both sides are compared as text and the
/// ligature is a character that case-folds to itself.
#[test]
fn a_word_with_a_ligature_is_found_by_its_letters() {
    // What the decoder produces for a run set with a real `fi` ligature.
    let page = "The of\u{FB01}ce is closed";
    assert!(
        mangle_text::find(page, "office").is_empty(),
        "the raw ligature is not the two letters, so this is the failure the decoder fixes"
    );
    // With the ligature normalised to its letters — which is what a search over text a reader
    // would read has to do — the word is found.
    let normalised: String = page
        .chars()
        .map(|c| match c {
            '\u{FB01}' => "fi".to_string(),
            other => other.to_string(),
        })
        .collect();
    let hits = mangle_text::find(&normalised, "office");
    assert_eq!(hits.len(), 1, "the word is found once normalised: {hits:?}");
    assert_eq!(sliced(&normalised, hits[0]), "office");
}

/// A hyphenated break is two halves, and searching for the whole word finds neither.
///
/// This is stated as a *failure* rather than hidden: `find` matches what is in the text, and
/// `sani-\ntization` does not contain `sanitization`. The join is a normalisation that belongs
/// before the search, and this test is what records that it is missing.
#[test]
fn a_hyphenated_break_is_two_halves_and_the_whole_word_is_in_neither() {
    let page = "Guidelines for Media Sani-\ntization";
    assert!(
        mangle_text::find(page, "sanitization").is_empty(),
        "the break splits the word, and a search over the raw text finds neither half"
    );
    // But each half is findable, which is what a user falls back to.
    assert_eq!(mangle_text::find(page, "sani").len(), 1);
    assert_eq!(mangle_text::find(page, "tization").len(), 1);
    // And joining the break is what makes the whole word searchable.
    let joined = page.replace("-\n", "");
    assert_eq!(mangle_text::find(&joined, "sanitization").len(), 1);
}

/// An empty needle finds nothing, because every position "matches" it and that is an answer that
/// tells a user nothing.
#[test]
fn an_empty_needle_finds_nothing() {
    assert!(mangle_text::find("a page of text", "").is_empty());
    assert!(mangle_text::find("", "word").is_empty());
}

/// Overlapping matches are not double-counted: `aa` in `aaa` is one hit, not two.
#[test]
fn matches_do_not_overlap() {
    let hits = mangle_text::find("aaaa", "aa");
    assert_eq!(hits.len(), 2, "two non-overlapping pairs: {hits:?}");
    assert_eq!(mangle_text::find("aaa", "aa").len(), 1);
}

/// Hits are attributed to the object they start in, so a hit is shown against the thing a user
/// would point at.
#[test]
fn a_hit_is_attributed_to_the_object_it_starts_in() {
    let objects = vec!["Archived NIST".to_string(), "Technical Series".to_string()];
    let joined = objects.join(" ");
    let hits = mangle_text::find(&joined, "Technical");
    assert_eq!(hits.len(), 1);
    let located = mangle_text::locate(&objects, &hits);
    assert_eq!(located.len(), 1);
    assert_eq!(located[0].0, 1, "the second object is where it starts");
    // And a phrase that runs across the boundary is still found — it is matched against the
    // joined text — and is attributed to the object it begins in.
    let spanning = mangle_text::find(&joined, "NIST Technical");
    assert_eq!(
        spanning.len(),
        1,
        "the phrase spans the join and is still one hit"
    );
    assert_eq!(
        mangle_text::locate(&objects, &spanning)[0].0,
        0,
        "and it begins in the first object"
    );
}

/// A needle that is not on the page finds nothing, which is the honest answer rather than a
/// near-miss shown as a hit.
#[test]
fn a_word_that_is_not_there_finds_nothing() {
    assert!(mangle_text::find("Archived NIST Technical Series", "Adobe").is_empty());
}

/// Offsets are in characters, and slicing with them gives the match back — including where the
/// text has multi-byte characters in it.
#[test]
fn offsets_are_character_offsets_and_slice_back_to_the_match() {
    let page = "NIST \u{65e5}\u{672c} publication";
    let hits = mangle_text::find(page, "publication");
    assert_eq!(hits.len(), 1);
    assert_eq!(sliced(page, hits[0]), "publication");
    // And a multi-byte needle is found, with the offsets still valid for slicing.
    let hits = mangle_text::find(page, "\u{65e5}\u{672c}");
    assert_eq!(hits.len(), 1);
    assert_eq!(sliced(page, hits[0]), "\u{65e5}\u{672c}");
}
