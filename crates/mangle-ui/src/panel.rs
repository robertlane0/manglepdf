//! What the contextual panel's rows are, decided without a window.
//!
//! The right-hand panel is where a user learns what they have selected and changes it, and both
//! halves are the same requirement: the numbers have to be the file's. A stepper that nudged a
//! value the window remembered would be a stepper that told the truth once and then drifted, and
//! a read-out that showed a remembered value would be a read-out of the wrong thing after an undo.
//!
//! So every value here is read out of the object's own [`TextState`] — which the interpreter puts
//! on every record because a `Tj` names no font, no size and no spacing — and a stepper asks for
//! an absolute value computed from it. [`PanelRow`] is what a row turns out to be, and
//! [`panel_row`] is the decision, taken as plain values so a test can take it without a mouse.
//!
//! # What is not here, and is not pretended
//!
//! The mockup's Text panel has ten rows. Four of them are numbers the interpreter already
//! records, and those four are steppers here. The rest — family, style, alignment, colour, render
//! mode, rotation — need the font layer (what the resource name resolves to, and what weight its
//! descriptor declares) and the colour, which a record does not carry. Those rows are
//! [`PanelRow::Empty`]: a field with nothing in it. A panel that invented "Regular" for a style
//! it had not read would be a panel that lies, and a blank field is at least honest about it.

use crate::shell::RightPanel;
use mangle_content::state::TextState;
use mangle_edit::TextProperty;

/// What one row of the contextual panel is.
#[derive(Debug, Clone, PartialEq)]
pub enum PanelRow {
    /// A value the panel shows and does not edit from here.
    Value(String),
    /// A number with a minus and a plus either side of it.
    ///
    /// The `property` carries the value the object has now, and a click writes it plus or minus
    /// `step` — see [`TextProperty::with_value`] for why the gesture is relative and the edit is
    /// not.
    Stepper {
        /// The number as the panel shows it.
        shown: String,
        /// How far one click moves it.
        step: f64,
        /// Which property a click sets.
        property: TextProperty,
    },
    /// A row with nothing to say yet: a field with no value in it.
    Empty,
}

/// The names of the rows, in the order the mockup draws them.
///
/// Kept as a list rather than built per panel, because the panel is a fixed composition — a user
/// learns where "Character Spacing" is and it does not move because a page had no text on it.
pub const TEXT_ROWS: [&str; 10] = [
    "Font",
    "Style",
    "Size",
    "Colour",
    "Alignment",
    "Line Spacing",
    "Character Spacing",
    "Baseline Shift",
    "Render Mode",
    "Rotation",
];

/// Which of the Text panel's rows are steppers, and by how much one click moves them.
///
/// The steps are in the units the row *shows*: a size moves a whole point, and a line spacing
/// moves a tenth of a multiple. That is what a designer expects from a stepper — a click of
/// "Character Spacing" that moved it by 0.1 would take a hundred clicks to go from 0 to 10.
const TEXT_STEPS: [(usize, f64); 4] = [
    (2, 1.0),
    (5, LINE_SPACING_STEP),
    (6, 1.0),
    (7, BASELINE_STEP),
];

/// One click of the line-spacing stepper, as a multiple of the size.
const LINE_SPACING_STEP: f64 = 0.1;

/// One click of the baseline-shift stepper, in points.
const BASELINE_STEP: f64 = 1.0;

/// What row `row` of a panel is, for an object drawn in `text`.
///
/// Only the Text Properties panel reads anything out yet, because only its four numbers are on
/// the records the interpreter already carries. Every row of every other panel is
/// [`PanelRow::Empty`] — a field with nothing in it — which is why the matching is on the panel
/// and not just on the row: the Colour panel's third row is a line width, not a size, and a
/// stepper that let a user click it as one would be a stepper that lied.
#[must_use]
pub fn panel_row(panel: RightPanel, row: usize, text: Option<&TextState>) -> PanelRow {
    let Some(state) = text else {
        return PanelRow::Empty;
    };
    if panel != RightPanel::Text {
        return PanelRow::Empty;
    }
    let step = TEXT_STEPS.iter().find(|(r, _)| *r == row).map(|(_, s)| *s);
    let Some(step) = step else {
        // The font's resource name is the one thing a panel can show without the font layer,
        // and it is the name as the stream wrote it — `/F3`, not a family.
        return if row == 0 {
            PanelRow::Value(
                state
                    .font
                    .clone()
                    .unwrap_or_else(|| "Not named".to_string()),
            )
        } else {
            PanelRow::Empty
        };
    };
    // What the row shows and what a click writes are the same value, and both come from the
    // object's own state: the panel shows the multiple because that is what a designer moves,
    // and the property holds the leading because that is what the file stores.
    let (shown, property) = match row {
        2 => (
            format!("{} pt", number(state.size)),
            TextProperty::Size(state.size),
        ),
        // A leading with no size to divide by is a multiple of nothing, so the row has
        // nothing to show and nothing a click could write.
        5 => match mangle_edit::line_spacing(state) {
            Some(multiple) => (number(multiple), TextProperty::Leading(state.leading)),
            None => return PanelRow::Empty,
        },
        6 => (
            number(state.char_spacing),
            TextProperty::CharacterSpacing(state.char_spacing),
        ),
        7 => (number(state.rise), TextProperty::BaselineShift(state.rise)),
        _ => return PanelRow::Empty,
    };
    PanelRow::Stepper {
        shown,
        step,
        property,
    }
}

/// What one click of a stepper asks the worker to write.
///
/// `property` carries the value the object has *now*, and `by` is the click's direction and size —
/// so this is the whole of "nudge the size by one point", read as two plain values rather than as
/// a gesture inside a window. The current value comes from [`mangle_edit::text_value`] over the
/// object's own state, which is the same read-out the panel just drew, so what a click writes is
/// exactly one step from what the user was looking at.
///
/// **Line spacing is the one row whose step is not in the file's own units.** The panel shows a
/// multiple and the file stores a distance, so a tenth of a multiple of 48 pt is 4.8 points of
/// `TL`. That scaling is here and only here, because this is the one place that knows both the
/// step and the object's size — and a second click of the same button moves it another tenth,
/// because the row's own value is recomputed from the object every frame.
///
/// `None` for the property that is not a number — a render mode has no direction to nudge in — or
/// for an object with no size, where a multiple of nothing is not a number.
#[must_use]
pub fn stepped(property: TextProperty, state: &TextState, by: f64) -> Option<TextProperty> {
    let current = mangle_edit::text_value(state, property)?;
    if let TextProperty::Leading(_) = property {
        // The row shows a multiple, so the step is a fraction of a multiple and the leading it
        // becomes is `multiple x size` — the size is what makes the two units agree.
        let size = state.size;
        if size.abs() < f64::EPSILON {
            return None;
        }
        return Some(TextProperty::Leading(size * (state.leading / size + by)));
    }
    property.with_value(current + by)
}
/// Which panel a selection opens.
///
/// The panel is contextual — it is about what is selected — so a click on a run of text has to
/// open a different panel from a click on a photograph. Text opens the text properties, because
/// that is where a size and a spacing live; anything else opens colour, because a picture's
/// fill and stroke are what there is to say about it; nothing selected goes back to the document
/// properties, which is the panel a viewer shows when there is no selection to be about.
#[must_use]
pub fn panel_for(selection: Option<&mangle_edit::Summary>) -> RightPanel {
    let Some(object) = selection else {
        return RightPanel::Properties;
    };
    match object.kind {
        mangle_edit::Kind::Line | mangle_edit::Kind::Block | mangle_edit::Kind::LooseRun => {
            RightPanel::Text
        }
        _ => RightPanel::Colour,
    }
}

/// A number as a panel shows it: two places at most, and never a negative zero.///
/// Two places, because a character spacing of 0.03 is a value a file really has and "0.03" is
/// what the operator says; and never `-0`, because a stepper that has been nudged back to nothing
/// reads as a mistake rather than as a zero.
#[must_use]
pub fn number(value: f64) -> String {
    let rounded = (value * 100.0).round() / 100.0;
    if rounded == 0.0 {
        return "0".to_string();
    }
    format!("{rounded}")
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect` and `unwrap`, which is what a test is for;
    // the panic-free rule is about what the product does with a file, not about tests.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use mangle_edit::Summary;
    fn state() -> TextState {
        TextState {
            font: Some("/F3".to_string()),
            size: 48.0,
            // 1.4 multiples of 48, as the hero document's subtitle is drawn.
            leading: 67.2,
            char_spacing: 4.0,
            rise: 0.0,
            ..TextState::default()
        }
    }

    /// An object with no text state, which is what a picture is.
    fn object() -> Summary {
        Summary {
            kind: mangle_edit::Kind::Image,
            bounds: mangle_content::state::ClipBounds {
                x0: 0.0,
                y0: 0.0,
                x1: 10.0,
                y1: 10.0,
            },
            spans: Vec::new(),
            form: None,
            line_break: None,
            text: None,
        }
    }

    /// A size is read out of the state the object was drawn with, in the file's own units.
    #[test]
    fn the_size_row_reads_the_size_the_file_drew() {
        let row = panel_row(RightPanel::Text, 2, Some(&state()));
        match row {
            PanelRow::Stepper {
                shown,
                step,
                property,
            } => {
                assert_eq!(shown, "48 pt");
                assert!((step - 1.0).abs() < 1e-9);
                assert_eq!(property.name(), "size");
            }
            other => panic!("size should be a stepper, was {other:?}"),
        }
    }

    /// The line-spacing row shows a *multiple*, because that is what a designer moves, while the
    /// edit it writes is a `TL` in points — and the conversion is done in one place.
    #[test]
    fn line_spacing_is_shown_as_a_multiple_and_written_as_points() {
        match panel_row(RightPanel::Text, 5, Some(&state())) {
            PanelRow::Stepper {
                shown,
                step,
                property,
            } => {
                // Shown as a multiple, because that is what a designer moves.
                assert_eq!(shown, "1.4");
                assert!((step - 0.1).abs() < 1e-9);
                // And carried as the leading the file stores -- the value the object has NOW,
                // not the value a click would write. The row is recomputed from the object every
                // frame, so holding the current value is what lets a second click move it again.
                assert_eq!(
                    property,
                    TextProperty::Leading(67.2),
                    "the row carries what the object has"
                );
            }
            other => panic!("line spacing should be a stepper, was {other:?}"),
        }
    }

    /// A row with no state to read is empty, rather than showing a number the file never had.
    #[test]
    fn an_object_that_is_not_text_has_nothing_to_show() {
        assert_eq!(panel_row(RightPanel::Text, 2, None), PanelRow::Empty);
        assert_eq!(panel_row(RightPanel::Text, 5, None), PanelRow::Empty);
    }

    /// The font row shows the resource name, which is the only thing that can be shown without
    /// the font layer — and says "not named" rather than inventing a family.
    #[test]
    fn the_font_row_shows_the_name_the_stream_wrote() {
        assert_eq!(
            panel_row(RightPanel::Text, 0, Some(&state())),
            PanelRow::Value("/F3".to_string())
        );
        assert_eq!(
            panel_row(RightPanel::Text, 0, Some(&TextState::default())),
            PanelRow::Value("Not named".to_string())
        );
    }

    /// One click of a stepper writes an absolute value read out of the state, which is the whole
    /// reason `with_value` exists.
    #[test]
    fn a_stepper_writes_the_value_it_read_plus_one_step() {
        let s = state();
        let PanelRow::Stepper { property, .. } = panel_row(RightPanel::Text, 2, Some(&s)) else {
            panic!("size should be a stepper");
        };
        let nudged = property.with_value(mangle_edit::text_value(&s, property).unwrap() + 1.0);
        assert_eq!(nudged, Some(TextProperty::Size(49.0)));
        // And back down again: a stepper is reversible, or undo is a fiction.
        let back = nudged.unwrap().with_value(48.0);
        assert_eq!(back, Some(TextProperty::Size(48.0)));
    }

    /// A leading with no size to divide by is a multiple of nothing, and the row says so.
    #[test]
    fn a_line_spacing_with_no_size_has_no_multiple() {
        let s = TextState {
            leading: 12.0,
            ..TextState::default()
        };
        assert_eq!(panel_row(RightPanel::Text, 5, Some(&s)), PanelRow::Empty);
    }

    /// One click of a stepper writes a value one step from what the object has.
    ///
    /// The current value is read out of the object's own state, so the direction of the click is
    /// the only thing the caller supplies — which is what makes it testable: a click is a
    /// direction, and the number it lands on comes from the file.
    #[test]
    fn a_click_of_a_stepper_writes_one_step_from_what_the_object_has() {
        let s = state();
        // The size row, which shows 48 and moves by a whole point.
        let PanelRow::Stepper { property, .. } = panel_row(RightPanel::Text, 2, Some(&s)) else {
            panic!("size should be a stepper");
        };
        let up = stepped(property, &s, 1.0);
        assert_eq!(up, Some(TextProperty::Size(49.0)));
        let down = stepped(property, &s, -1.0);
        assert_eq!(down, Some(TextProperty::Size(47.0)));
    }

    /// Line spacing is the one row whose step is not one of the file's own units: the click moves
    /// a *multiple*, and what gets written is the leading that multiple implies.
    #[test]
    fn a_click_of_the_line_spacing_row_moves_a_multiple() {
        let s = state();
        let PanelRow::Stepper { property, .. } = panel_row(RightPanel::Text, 5, Some(&s)) else {
            panic!("line spacing should be a stepper");
        };
        // The row carries the leading the file stores, which is 1.4 multiples of 48.
        assert_eq!(property, TextProperty::Leading(67.2));
        // A click widens the multiple by a tenth, which is 4.8 points of leading.
        let Some(TextProperty::Leading(after)) = stepped(property, &s, 0.1) else {
            panic!("a line spacing writes a leading");
        };
        assert!(
            (after - 48.0 * 1.5).abs() < 1e-6,
            "a tenth of a multiple of 48 is 4.8 points: {after}"
        );
        // And a second click from the NEW state moves it another tenth -- which is the case the
        // old design got wrong, because it had the row carry the next value already.
        let widened = TextState {
            leading: 72.0,
            ..s.clone()
        };
        let Some(TextProperty::Leading(again)) =
            stepped(TextProperty::Leading(72.0), &widened, 0.1)
        else {
            panic!("a line spacing writes a leading");
        };
        assert!(
            (again - 48.0 * 1.6).abs() < 1e-6,
            "the second click adds another tenth of the size: {again}"
        );
    }

    /// The property that is not a number is answered with nothing rather than with a guess: a
    /// render mode has no direction to nudge in, and a window that clicked it as though it did
    /// would write a number the file never meant.
    #[test]
    fn a_property_that_is_not_a_number_is_not_steppable() {
        let s = state();
        let mode = TextProperty::RenderMode(mangle_content::state::RenderMode::Stroke);
        assert_eq!(stepped(mode, &s, 1.0), None);
    }

    /// An object with no text state has nothing to nudge, which is the honest answer for a
    /// picture: the row is empty and a click on it writes nothing. `stepped` takes the state
    /// directly, so the "no text state" case is a caller that never calls it — and what it must
    /// not do is invent a value for an object whose state it does not have.
    #[test]
    fn a_picture_has_no_state_to_step_from() {
        assert_eq!(panel_row(RightPanel::Text, 2, None), PanelRow::Empty);
        // The image kind never reaches `stepped`, because `panel_row` answered Empty first.
        assert_eq!(
            panel_for(Some(&Summary {
                kind: mangle_edit::Kind::Image,
                ..object()
            })),
            RightPanel::Colour
        );
    }

    /// A number that came back to nothing reads as zero, not as a minus sign.
    #[test]
    fn a_number_that_is_nothing_reads_as_zero() {
        assert_eq!(number(0.0), "0");
        assert_eq!(number(-0.0), "0");
        assert_eq!(number(-0.001), "0");
        assert_eq!(number(48.0), "48");
        assert_eq!(number(0.03), "0.03");
        assert_eq!(number(1.444), "1.44");
    }

    /// The panel a selection opens, which is what makes the right-hand side contextual at all.
    ///
    /// Text opens the text panel, a picture opens the colour panel, and nothing selected goes back
    /// to the document properties. The bug this closes is that a box could be drawn around a run
    /// of text while the panel still said "Document Properties": the selection was stored and the
    /// panel was never told, so the numbers were there and unreachable.
    #[test]
    fn the_panel_follows_the_selection() {
        let text = Summary {
            kind: mangle_edit::Kind::Line,
            ..object()
        };
        assert_eq!(panel_for(None), RightPanel::Properties);
        assert_eq!(panel_for(Some(&text)), RightPanel::Text);
        // A block and a loose run are text; a picture and a path are not, and the panel is the
        // whole difference between opening on a size stepper and opening on a fill swatch.
        for kind in [
            mangle_edit::Kind::Block,
            mangle_edit::Kind::LooseRun,
            mangle_edit::Kind::Image,
            mangle_edit::Kind::Path,
        ] {
            let other = Summary { kind, ..object() };
            let expected =
                if kind == mangle_edit::Kind::Block || kind == mangle_edit::Kind::LooseRun {
                    RightPanel::Text
                } else {
                    RightPanel::Colour
                };
            assert_eq!(panel_for(Some(&other)), expected, "{kind:?}");
        }
    }
}
