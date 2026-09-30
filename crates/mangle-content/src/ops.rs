//! The operator table: what each operator consumes and what it does to the state.
//!
//! A content stream has no syntax for arity, so the only way to know that `cm` wants
//! six numbers and `Tj` wants one string is to say so. Doing it in one table rather than
//! in the interpreter means the interpreter is a loop, and means a caller that wants to
//! know "what could this byte be?" can ask without running anything.
//!
//! The table is from ISO 32000-1, tables 52 to 58. An operator that is not in it is
//! still reported — a file from a later revision, or a damaged one, will contain
//! operators this table has never heard of, and dropping them silently would hide the
//! damage.

use mangle_syntax::object::Object;

/// The type one operand must have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// An integer.
    Int,
    /// A real, or an integer where a real is wanted.
    Real,
    /// Either number.
    Num,
    /// A name.
    Name,
    /// A string, literal or hex.
    Str,
    /// A dictionary, as `BDC` and `DP` take.
    Dict,
    /// An array of names, as `d0`/`d1` take.
    NameArray,
    /// Anything at all, for the operators that ignore their operand.
    Any,
}

/// What an operator does beyond producing marks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Effect {
    /// `q`: pushes the state.
    pub saves: bool,
    /// `Q`: pops the state.
    pub restores: bool,
    /// Paints the current path, which also ends it.
    pub paints: bool,
    /// Ends the current path by closing it, without painting.
    pub closes: bool,
    /// This operator produces a mark *without* painting a path: a run of glyphs, an
    /// image, a shading. Path painting operators are covered by `paints`.
    pub emits: bool,
    /// Moves the text position, which matters for the reading order.
    pub is_text: bool,
    /// `gs`: takes the state from a parameter dictionary.
    pub applies_extgstate: bool,
}

/// One row of the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperatorInfo {
    pub name: &'static [u8],
    pub operands: &'static [Kind],
    pub effect: Effect,
}

const fn op(name: &'static [u8], operands: &'static [Kind], effect: Effect) -> OperatorInfo {
    OperatorInfo {
        name,
        operands,
        effect,
    }
}

const NONE: Effect = Effect {
    saves: false,
    restores: false,
    paints: false,
    closes: false,
    emits: false,
    is_text: false,
    applies_extgstate: false,
};
/// A text operator: some of them move the position, some of them show glyphs, and most
/// do both.
const TEXT: Effect = Effect {
    is_text: true,
    ..NONE
};
const SHOWS: Effect = Effect {
    emits: true,
    is_text: true,
    ..NONE
};
const PAINTS: Effect = Effect {
    paints: true,
    emits: true,
    ..NONE
};
const EMITS: Effect = Effect {
    emits: true,
    ..NONE
};
const CLOSES: Effect = Effect {
    closes: true,
    ..NONE
};
const SAVES: Effect = Effect {
    saves: true,
    ..NONE
};
const RESTORES: Effect = Effect {
    restores: true,
    ..NONE
};
const EXTGSTATE: Effect = Effect {
    applies_extgstate: true,
    ..NONE
};

/// A variable-length run of numbers, as the colour operators take.
const N_NUMS: &[Kind] = &[Kind::Num, Kind::Num];
const SIX_NUMS: &[Kind] = &[Kind::Num; 6];
const FOUR_NUMS: &[Kind] = &[Kind::Num; 4];
const NONE_K: &[Kind] = &[];
const NAME_ONE: &[Kind] = &[Kind::Name];
const STR_ONE: &[Kind] = &[Kind::Str];
const DICT_ONE: &[Kind] = &[Kind::Dict];
const NAME_OR_ARRAY: &[Kind] = &[Kind::Any];
const ONE_NUM: &[Kind] = &[Kind::Num];
const TWO_NUMS: &[Kind] = &[Kind::Num, Kind::Num];
const THREE_NUMS: &[Kind] = &[Kind::Num, Kind::Num, Kind::Num];
const FOUR_NUM_INT: &[Kind] = &[Kind::Num, Kind::Num, Kind::Num, Kind::Num];
const SIX: &[Kind] = &[Kind::Num; 6];

/// Every operator the specification defines, in the order the tables give them.
pub static OPERATORS: &[OperatorInfo] = &[
    // Table 52: general graphics state.
    op(b"w", ONE_NUM, NONE),
    op(b"J", &[Kind::Int], NONE),
    op(b"j", &[Kind::Int], NONE),
    op(b"M", ONE_NUM, NONE),
    op(b"d", &[Kind::Int, Kind::Any], NONE),
    op(
        b"ri",
        &[
            Kind::Int,
            Kind::Int,
            Kind::Int,
            Kind::Int,
            Kind::Int,
            Kind::Int,
        ],
        NONE,
    ),
    op(b"i", ONE_NUM, NONE),
    op(b"gs", NAME_ONE, EXTGSTATE),
    op(b"q", NONE_K, SAVES),
    op(b"Q", NONE_K, RESTORES),
    op(b"cm", SIX_NUMS, NONE),
    // Table 53: special graphics state.
    op(b"CS", NAME_ONE, NONE),
    op(b"cs", NAME_ONE, NONE),
    op(b"SC", N_NUMS, NONE),
    op(b"sc", N_NUMS, NONE),
    op(b"SCN", NAME_OR_ARRAY, NONE),
    op(b"scn", NAME_OR_ARRAY, NONE),
    // Table 54: path construction.
    op(b"m", TWO_NUMS, NONE),
    op(b"l", TWO_NUMS, NONE),
    op(b"c", SIX, NONE),
    op(b"v", &[Kind::Num; 4], NONE),
    op(b"y", &[Kind::Num; 4], NONE),
    op(b"h", NONE_K, CLOSES),
    op(b"re", FOUR_NUMS, NONE),
    // Table 55: path painting.
    op(b"S", NONE_K, PAINTS),
    op(
        b"s",
        NONE_K,
        Effect {
            paints: true,
            closes: true,
            ..NONE
        },
    ),
    op(
        b"f",
        NONE_K,
        Effect {
            paints: true,
            closes: true,
            ..NONE
        },
    ),
    op(
        b"F",
        NONE_K,
        Effect {
            paints: true,
            closes: true,
            ..NONE
        },
    ),
    op(
        b"f*",
        NONE_K,
        Effect {
            paints: true,
            closes: true,
            ..NONE
        },
    ),
    op(
        b"B",
        NONE_K,
        Effect {
            paints: true,
            closes: true,
            ..NONE
        },
    ),
    op(
        b"B*",
        NONE_K,
        Effect {
            paints: true,
            closes: true,
            ..NONE
        },
    ),
    op(b"b", NONE_K, PAINTS),
    op(b"b*", NONE_K, PAINTS),
    op(b"n", NONE_K, EMITS),
    // Table 56: clipping paths.
    op(b"W", NONE_K, NONE),
    op(b"W*", NONE_K, NONE),
    // Table 57: text objects.
    op(b"BT", NONE_K, TEXT),
    op(b"ET", NONE_K, TEXT),
    // Table 58: text state, positioning and painting.
    op(b"Tc", ONE_NUM, TEXT),
    op(b"Tw", ONE_NUM, TEXT),
    op(b"Tz", ONE_NUM, TEXT),
    op(b"TL", ONE_NUM, TEXT),
    op(b"Tf", &[Kind::Name, Kind::Num], TEXT),
    op(b"Tr", &[Kind::Int], TEXT),
    op(b"Ts", ONE_NUM, TEXT),
    op(b"Td", TWO_NUMS, TEXT),
    op(b"TD", TWO_NUMS, TEXT),
    op(b"Tm", SIX_NUMS, TEXT),
    op(b"T*", NONE_K, TEXT),
    op(b"Tj", STR_ONE, SHOWS),
    op(b"TJ", &[Kind::Any], SHOWS),
    op(b"'", STR_ONE, SHOWS),
    op(b"\"", &[Kind::Num, Kind::Num, Kind::Str], SHOWS),
    // Type 3 glyph metrics.
    // `d0` and `d1` take a width vector, whose shape depends on the font's type.
    op(b"d0", &[Kind::NameArray, Kind::Num], TEXT),
    op(b"d1", &[Kind::NameArray, Kind::Num], TEXT),
    // Type 3 glyph procedures.
    op(b"gx", NONE_K, NONE),
    // Colour space components. The operators that select a space, `CS`, `cs`, `SC`,
    // `sc`, `SCN` and `scn`, are in the table above with the other state operators.
    op(b"G", ONE_NUM, NONE),
    op(b"g", ONE_NUM, NONE),
    op(b"RG", THREE_NUMS, NONE),
    op(b"rg", THREE_NUMS, NONE),
    op(b"K", FOUR_NUM_INT, NONE),
    op(b"k", FOUR_NUM_INT, NONE),
    // Shading and XObjects.
    op(b"sh", NAME_ONE, EMITS),
    op(b"Do", NAME_ONE, EMITS),
    // Inline images.
    op(b"BI", NAME_ONE, EMITS),
    op(b"ID", NONE_K, NONE),
    op(b"EI", NONE_K, NONE),
    // Marked content.
    op(b"MP", NAME_ONE, NONE),
    op(b"DP", NAME_OR_ARRAY, NONE),
    op(b"BMC", NAME_ONE, NONE),
    op(b"BDC", DICT_ONE, NONE),
    op(b"EMC", NONE_K, NONE),
    // Compatibility sections.
    op(b"BX", NONE_K, NONE),
    op(b"EX", NONE_K, NONE),
    // Optional content.
    op(b"OC", NAME_ONE, NONE),
];

/// What an operator consumes and does, if this table knows it.
#[must_use]
pub fn lookup(name: &[u8]) -> Option<&'static OperatorInfo> {
    // A linear scan is right for a table this size and keeps the order meaningful for
    // a caller that wants to display it.
    OPERATORS.iter().find(|o| o.name == name)
}

/// How many operands an operator takes, or `None` when it is not in the table.
///
/// An operator outside the table takes everything, because refusing to interpret it
/// would mean dropping the operations around it.
#[must_use]
pub fn arity(name: &[u8]) -> Option<usize> {
    Some(lookup(name)?.operands.len())
}

/// The effect an operator has, or the neutral one for an operator not in the table.
#[must_use]
pub fn effect(name: &[u8]) -> Effect {
    lookup(name).map_or(NONE, |o| o.effect)
}

/// Does this operator paint the current path?
#[must_use]
pub fn paints(name: &[u8]) -> bool {
    effect(name).paints
}

/// Does this operator leave a mark on the page?
#[must_use]
pub fn emits(name: &[u8]) -> bool {
    let e = effect(name);
    e.emits || e.paints
}

/// Does this operator move the text position?
#[must_use]
pub fn is_text(name: &[u8]) -> bool {
    effect(name).is_text
}

/// The operands an operation of this operator should actually use.
///
/// The stream is allowed to leave more operands than the operator consumes — a damaged
/// file does — so the last *n* are taken, which is the direction operands are read in.
#[must_use]
pub fn take_last<'a>(info: &OperatorInfo, operands: &[&'a Object]) -> Vec<&'a Object> {
    let n = info.operands.len();
    let skip = operands.len().saturating_sub(n);
    operands.iter().copied().skip(skip).collect()
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect`, which is what a test is for; the
    // panic-free rule is about what the product does with a file, not about tests.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn the_table_has_no_duplicate_names() {
        let mut names: Vec<&[u8]> = OPERATORS.iter().map(|o| o.name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(
            names.len(),
            before,
            "an operator appears twice in the table"
        );
    }

    #[test]
    fn the_operators_that_every_file_uses_are_present() {
        for name in [
            &b"q"[..],
            b"Q",
            b"cm",
            b"m",
            b"l",
            b"c",
            b"re",
            b"h",
            b"S",
            b"s",
            b"f",
            b"F",
            b"f*",
            b"B",
            b"B*",
            b"b",
            b"b*",
            b"n",
            b"W",
            b"W*",
            b"BT",
            b"ET",
            b"Tc",
            b"Tw",
            b"Tz",
            b"TL",
            b"Tf",
            b"Tr",
            b"Ts",
            b"Td",
            b"TD",
            b"Tm",
            b"T*",
            b"Tj",
            b"TJ",
            b"'",
            b"\"",
            b"gs",
            b"d0",
            b"d1",
            b"sh",
            b"Do",
            b"BI",
            b"ID",
            b"EI",
            b"MP",
            b"DP",
            b"BMC",
            b"BDC",
            b"EMC",
        ] {
            assert!(
                lookup(name).is_some(),
                "{} is missing",
                String::from_utf8_lossy(name)
            );
        }
    }

    #[test]
    fn the_arity_is_what_the_specification_says() {
        assert_eq!(arity(b"cm"), Some(6));
        assert_eq!(arity(b"re"), Some(4));
        assert_eq!(arity(b"Tf"), Some(2));
        assert_eq!(arity(b"Tj"), Some(1));
        assert_eq!(arity(b"Q"), Some(0));
        assert_eq!(arity(b"d0"), Some(2));
    }

    #[test]
    fn painting_and_closing_are_distinguished() {
        assert!(paints(b"S"), "S strokes and ends the path");
        assert!(paints(b"f"), "f fills and ends the path");
        assert!(!paints(b"n"), "n ends the path without painting it");
        assert!(!paints(b"W"), "W sets a clip without painting");
        assert!(effect(b"h").closes, "h closes without painting");
        assert!(!effect(b"h").paints);
        // `s` and `f` both close *and* paint; `b` paints but the close is optional.
        assert!(effect(b"f").closes && paints(b"f"));
        assert!(paints(b"b") && !effect(b"b").closes);
    }

    #[test]
    fn text_operators_are_marked_as_such() {
        for name in [b"BT", b"ET", b"Tj", b"TJ", b"Td", b"Tm", b"Tf", b"T*"] {
            assert!(
                is_text(name),
                "{} moves the text position",
                String::from_utf8_lossy(name)
            );
        }
        assert!(!is_text(b"re"), "a rectangle is not text");
        assert!(!is_text(b"Do"));
    }

    #[test]
    fn an_operator_outside_the_table_is_not_fatal() {
        assert!(lookup(b"ZZ").is_none());
        assert_eq!(arity(b"ZZ"), None);
        assert_eq!(
            effect(b"ZZ"),
            NONE,
            "an unknown operator changes nothing known"
        );
    }

    #[test]
    fn operands_are_taken_from_the_end() {
        let info = lookup(b"cm").expect("cm is in the table");
        let owned = [
            Object::Int(99), // left over from a damaged stream
            Object::Int(1),
            Object::Int(0),
            Object::Int(0),
            Object::Int(1),
            Object::Int(10),
            Object::Int(20),
        ];
        let operands: Vec<&Object> = owned.iter().collect();
        let taken = take_last(info, &operands);
        assert_eq!(taken.len(), 6);
        assert_eq!(taken.first().and_then(|o| o.as_i64()), Some(1));
        assert_eq!(taken.last().and_then(|o| o.as_i64()), Some(20));
    }
}
