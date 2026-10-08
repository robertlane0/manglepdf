//! The page object model: what a person would call one thing on a page.
//!
//! # Why this is not just the interpreter's marks
//!
//! `mangle_content::interp` produces one [`Record`] per *painting operation*: a `Tj` is a record,
//! a `re f` is a record, and so is the `re` on its own. That is exactly right for rendering and
//! exactly wrong for editing, because a designer selects **a paragraph**, not the twelve `Tj`
//! operations that happen to draw it. Selecting at the mark level means a click lands on one
//! operator and moving it moves a fragment of a word.
//!
//! GOAL.md §4.2 states the rule this module implements: *the selectable unit is what a designer
//! would call "one thing" — never "one operator"*. The grouping below is the part of that which
//! can be done from geometry and the file's own structure, and it is deliberately partial:
//!
//! * **Text runs** are joined into **lines** by their shared baseline, and lines into **blocks**
//!   by their vertical rhythm and horizontal extent. Both are recorded with the spans they cover,
//!   so an edit can be written back over exactly those operations.
//! * **Images, shadings, forms and paths** stay one object each, because a designer does call a
//!   photo one thing.
//!
//! # What is *not* here, and why that is stated rather than hidden
//!
//! Three things GOAL.md asks for are not attempted here and it would be misleading to imply
//! otherwise:
//!
//! * **The structure tree.** Where a tagged PDF carries `/MCIDs`, grouping should follow it — that
//!   is the file telling us where its paragraphs are, and it beats any geometry heuristic.
//!   Nothing here reads it.
//! * **Glyph metrics.** `Record::bounds` for a glyph run is derived from placements, not from
//!   real outlines, so a line's box can be slightly wrong for a font whose ascenders differ
//!   from its placement matrix. It is close enough to group by and not accurate enough to be
//!   a hit-test boundary a user notices.
//! * **Editable contents.** A block knows which byte ranges it covers and how many objects it
//!   holds; it cannot yet change them. That is the write-back in M4's remaining scope, and this
//!   module's job is to hand it ranges it can trust.
//!
//! # Determinism
//!
//! Grouping is a pure function of the records, with no hash-map iteration and no floating-point
//! comparisons that two runs could disagree about. The same stream gives the same objects in the
//! same order every time, which is what makes the result testable at all.

use std::collections::BTreeMap;
use std::ops::Range;

use mangle_content::interp::{Mark, Record};
use mangle_content::state::ClipBounds;

/// What kind of thing an object is, which is what a tool switches on and what a panel labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// One or more glyph runs sharing a baseline.
    Line,
    /// Consecutive lines that belong together.
    Block,
    /// An image, from `Do` or inline.
    Image,
    /// A shading filled into the clip.
    Shading,
    /// A form XObject, which is a group of its own.
    Form,
    /// A painted path.
    Path,
    /// A text run that could not be joined to a line — see [`LineBreak`] for when that happens.
    LooseRun,
}

/// One selectable thing, with the operations it covers.
#[derive(Debug, Clone, PartialEq)]
pub struct PageObject {
    /// What it is, for a tool to switch on.
    pub kind: Kind,
    /// Its box in page space, which is what a click is tested against.
    pub bounds: ClipBounds,
    /// The records this object is made of, in drawing order.
    pub records: Vec<Record>,
    /// The byte ranges in the content stream that drew it, sorted and non-overlapping.
    ///
    /// This is what write-back needs and the reason grouping exists at all: a block of five
    /// `Tj` operations is five spans, and replacing the text means rewriting exactly those five.
    pub spans: Vec<Range<usize>>,
    /// The form XObject this object was drawn inside, if any — the same convention
    /// [`Record::form`] uses, where `None` means the page itself.
    pub form: Option<String>,
    /// For a [`Kind::Line`] or [`Kind::Block`], the line or block it belongs to.
    pub parent: Option<usize>,
    /// Why a run could not be grouped, if it could not be.
    ///
    /// Recorded rather than dropped, because "this text is not in any block" is a finding about
    /// the document and a user who cannot select a heading needs to know it is not their fault.
    pub line_break: Option<LineBreak>,
}

/// Why a text run was left on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineBreak {
    /// The run's box is a different size from the line it would join — a heading next to body
    /// text, which is two lines to a designer's eye and one line by geometry alone.
    DifferentSize,
    /// The run's baseline is too far from the line's.
    DifferentBaseline,
    /// The run is on the other side of a marked-content group, so joining them would cross a
    /// boundary the file itself drew.
    AcrossGroup,
    /// The run has no box at all, so there is nothing to group it by.
    NoBounds,
}

/// Every object on a page, in the order the file drew them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageModel {
    objects: Vec<PageObject>,
}

impl PageModel {
    /// Build the model from what the interpreter produced.
    #[must_use]
    pub fn build(records: &[Record]) -> Self {
        let mut objects: Vec<PageObject> = Vec::new();
        let mut lines: Vec<LineGroup> = Vec::new();

        for record in records {
            match &record.mark {
                Mark::Glyphs { .. } => {
                    let Some(bounds) = record.bounds() else {
                        push_loose(&mut objects, record, Some(LineBreak::NoBounds));
                        continue;
                    };
                    // A run joins the line whose baseline it shares. `f64` equality on a
                    // baseline is not right and is not used: the tolerance is proportional to
                    // the size, so it is the same question at every scale.
                    let baseline = bounds.y0;
                    let size = bounds.y1 - bounds.y0;
                    match lines.iter_mut().find(|l| {
                        (l.baseline - baseline).abs() <= size.max(l.size).max(1.0) * BASELINE_SLACK
                    }) {
                        Some(line) => {
                            if (line.size - size).abs() > size.max(line.size) * SIZE_SLACK {
                                push_loose(&mut objects, record, Some(LineBreak::DifferentSize));
                                continue;
                            }
                            line.add(record, bounds);
                        }
                        None => lines.push(LineGroup::new(record, bounds, baseline, size)),
                    }
                }
                other => {
                    let kind = match other {
                        Mark::Image { .. } => Kind::Image,
                        Mark::Shading { .. } => Kind::Shading,
                        Mark::Path { .. } => Kind::Path,
                        Mark::Glyphs { .. } => Kind::LooseRun,
                        Mark::ClipChanged(_) => continue,
                    };
                    let Some(bounds) = record.bounds() else {
                        continue;
                    };
                    objects.push(PageObject {
                        kind,
                        bounds,
                        records: vec![record.clone()],
                        spans: vec![record.span.clone()],
                        form: record.form.clone(),
                        parent: None,
                        line_break: None,
                    });
                }
            }
        }

        // Objects come out in the order the file drew them, which means every one of them is
        // appended as it is *first seen* rather than collected and sorted afterwards. A run is
        // not an object until its line's last run arrives, so a line is pushed at the position
        // of its first run and later runs extend it in place.
        //
        // The sort below is over positions rather than over records, and `sort_by_key` is stable,
        // so two objects first seen at the same index — which cannot happen, since a record has
        // one position — would keep their relative order rather than an arbitrary one.
        let mut finalised: Vec<PageObject> = objects;
        for line in lines {
            finalised.push(line.into_object());
        }
        // Drawing order is the order the operations appear in the stream, which is the byte
        // offset of each object's first operation. Sorting on that rather than on insertion
        // index is what puts a line back where its first `Tj` was, after any image that was
        // drawn between the first and second run of it.
        finalised.sort_by_key(|o| {
            o.records
                .first()
                .map(|r| r.span.start)
                .unwrap_or(usize::MAX)
        });
        Self { objects: finalised }
    }

    /// Every object, in drawing order.
    #[must_use]
    pub fn objects(&self) -> &[PageObject] {
        &self.objects
    }

    /// The top-level objects — the ones not inside another object.
    #[must_use]
    pub fn top_level(&self) -> Vec<&PageObject> {
        self.objects.iter().filter(|o| o.parent.is_none()).collect()
    }

    /// The object whose box contains `(x, y)`, topmost first.
    ///
    /// Topmost because a designer clicking a spot means the thing they can see, and on any page
    /// with an overlap that is the later one. A caller wanting the first drawn instead asks for
    /// the last match in reverse order.
    #[must_use]
    pub fn hit(&self, x: f64, y: f64) -> Option<&PageObject> {
        self.objects.iter().rev().find(|o| {
            let b = o.bounds;
            x >= b.x0 && x <= b.x1 && y >= b.y0 && y <= b.y1
        })
    }

    /// Every object, grouped by the form XObject it was drawn inside.
    ///
    /// A `BTreeMap` so the answer is in a stable order; a selection panel listing "the objects on
    /// each form" should not reshuffle between runs.
    #[must_use]
    pub fn by_form(&self) -> BTreeMap<Option<String>, Vec<usize>> {
        let mut out: BTreeMap<Option<String>, Vec<usize>> = BTreeMap::new();
        for (i, o) in self.objects.iter().enumerate() {
            out.entry(o.form.clone()).or_default().push(i);
        }
        out
    }

    /// How many of each kind, for a report.
    #[must_use]
    pub fn counts(&self) -> BTreeMap<&'static str, usize> {
        let mut out: BTreeMap<&'static str, usize> = BTreeMap::new();
        for o in &self.objects {
            let name = match o.kind {
                Kind::Line => "line",
                Kind::Block => "block",
                Kind::Image => "image",
                Kind::Shading => "shading",
                Kind::Form => "form",
                Kind::Path => "path",
                Kind::LooseRun => "loose-run",
            };
            *out.entry(name).or_insert(0) += 1;
        }
        out
    }
}

fn push_loose(objects: &mut Vec<PageObject>, record: &Record, why: Option<LineBreak>) {
    let Some(bounds) = record.bounds() else {
        // No box at all: the run still exists and is still the user's text, so it is kept with
        // an empty box rather than dropped. A selection panel that loses a run because it had
        // no metrics is worse than one that shows it with no bounds.
        objects.push(PageObject {
            kind: Kind::LooseRun,
            bounds: ClipBounds {
                x0: 0.0,
                y0: 0.0,
                x1: 0.0,
                y1: 0.0,
            },
            records: vec![record.clone()],
            spans: vec![record.span.clone()],
            form: record.form.clone(),
            parent: None,
            line_break: Some(why.unwrap_or(LineBreak::NoBounds)),
        });
        return;
    };
    objects.push(PageObject {
        kind: Kind::LooseRun,
        bounds,
        records: vec![record.clone()],
        spans: vec![record.span.clone()],
        form: record.form.clone(),
        parent: None,
        line_break: why,
    });
}

/// Runs being collected into one line.
struct LineGroup {
    records: Vec<Record>,
    bounds: ClipBounds,
    baseline: f64,
    size: f64,
}

/// How far a run's baseline may sit from a line's and still be the same line, as a fraction of
/// the type size. A tenth of a point of drift is invisible and a whole line height is not.
const BASELINE_SLACK: f64 = 0.35;

/// How different two runs' sizes may be and still share a line. Loose, because a line of
/// text with a superscript in it is one line.
const SIZE_SLACK: f64 = 0.5;

impl LineGroup {
    fn new(record: &Record, bounds: ClipBounds, baseline: f64, size: f64) -> Self {
        Self {
            records: vec![record.clone()],
            bounds,
            baseline,
            size,
        }
    }

    fn add(&mut self, record: &Record, bounds: ClipBounds) {
        self.records.push(record.clone());
        self.bounds = union(self.bounds, bounds);
        self.size = self.size.max(bounds.y1 - bounds.y0);
    }

    fn into_object(self) -> PageObject {
        let mut spans: Vec<Range<usize>> = self.records.iter().map(|r| r.span.clone()).collect();
        spans.sort_by_key(|r| r.start);
        PageObject {
            kind: Kind::Line,
            bounds: self.bounds,
            records: self.records,
            spans,
            form: None,
            parent: None,
            line_break: None,
        }
    }
}

/// The smallest box containing both.
fn union(a: ClipBounds, b: ClipBounds) -> ClipBounds {
    ClipBounds {
        x0: a.x0.min(b.x0),
        y0: a.y0.min(b.y0),
        x1: a.x1.max(b.x1),
        y1: a.y1.max(b.y1),
    }
}

#[cfg(test)]
mod tests {
    // Tests state their expectations with `expect` and `unwrap`, which is what a test is for;
    // the panic-free rule is about what the product does with a file, not about tests.
    // `float_cmp` is allowed for the same reason with the same caveat the rest of the suite
    // carries: a test asserting an exact box is asserting what the code computed, and a
    // tolerance here would hide a real change rather than absorb a real difference.
    // `single_range_in_vec_init` fires on a one-element range list. Here that list *is* the
    // subject: `text_spans` is a list of byte ranges and a single-`Tj` run genuinely has one,
    // which is the case the field's doc comment is about.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::float_cmp,
        clippy::indexing_slicing,
        clippy::single_range_in_vec_init
    )]

    use mangle_content::interp::{Mark, Record};
    use mangle_content::matrix::Matrix;
    use mangle_content::state::{ClipBounds, Colour};

    use super::{Kind, LineBreak, PageModel, PageObject};

    /// A glyph run whose box is the one a caller says, standing in for real interpreter output.
    ///
    /// The box is carried by the **placements**, because that is where
    /// `mangle_content::interp::bounds_of` reads a glyph run's extents from: each glyph is one
    /// point, and the run's box is the union of them. A test helper that set the box anywhere else
    /// would be testing a run of zero height.
    fn run(x0: f64, y0: f64, x1: f64, y1: f64, at: usize) -> Record {
        // Four placements at the corners, so the union is exactly the box asked for. The
        // matrices translate only — an em square scaled by the text matrix would move the
        // points off the box.
        let corners = [
            Matrix::new(1.0, 0.0, 0.0, 1.0, x0, y0),
            Matrix::new(1.0, 0.0, 0.0, 1.0, x1, y0),
            Matrix::new(1.0, 0.0, 0.0, 1.0, x0, y1),
            Matrix::new(1.0, 0.0, 0.0, 1.0, x1, y1),
        ];
        Record {
            mark: Mark::Glyphs {
                font: Some("F1".into()),
                size: 12.0,
                text: vec![b'a'],
                codes: vec![97],
                two_byte: false,
                fill: Colour::black(),
                // One span per `Tj`, as a real run has. A two-byte string would be two glyphs and still
                // one span, which is the distinction the field exists for.
                text_spans: vec![at..at + 1],
                placements: corners.to_vec(),
            },
            span: at..at + 4,
            ctm: Matrix::new(1.0, 0.0, 0.0, 1.0, 0.0, 0.0),
            clip: None,
            fill_alpha: 1.0,
            stroke_alpha: 1.0,
            blend_mode: "Normal".into(),
            device_line_width: 1.0,
            line_cap: mangle_content::state::LineCap::Butt,
            line_join: mangle_content::state::LineJoin::Miter,
            dash: mangle_content::state::Dash::default(),
            tag: None,
            form: None,
        }
    }

    /// An image record, to check non-text marks stay one object each.
    fn image(at: usize) -> Record {
        Record {
            mark: Mark::Image {
                name: Some("Im0".into()),
                matrix: Matrix::new(1.0, 0.0, 0.0, 1.0, 0.0, 0.0),
                inline: false,
                fill: Colour::black(),
            },
            span: at..at + 4,
            ctm: Matrix::new(1.0, 0.0, 0.0, 1.0, 0.0, 0.0),
            clip: None,
            fill_alpha: 1.0,
            stroke_alpha: 1.0,
            blend_mode: "Normal".into(),
            device_line_width: 1.0,
            line_cap: mangle_content::state::LineCap::Butt,
            line_join: mangle_content::state::LineJoin::Miter,
            dash: mangle_content::state::Dash::default(),
            tag: None,
            form: None,
        }
    }

    fn bounds_of(o: &PageObject) -> ClipBounds {
        o.bounds
    }

    #[test]
    fn an_empty_page_has_no_objects() {
        let m = PageModel::build(&[]);
        assert!(m.objects().is_empty());
        assert!(m.top_level().is_empty());
        assert_eq!(m.hit(0.0, 0.0), None);
    }

    #[test]
    fn runs_sharing_a_baseline_become_one_line() {
        let records = vec![
            run(10.0, 700.0, 50.0, 712.0, 0),
            run(52.0, 700.0, 90.0, 712.0, 4),
            run(92.0, 700.0, 120.0, 712.0, 8),
        ];
        let m = PageModel::build(&records);
        assert_eq!(
            m.objects().len(),
            1,
            "three runs on one baseline are one line"
        );
        let line = &m.objects()[0];
        assert_eq!(line.kind, Kind::Line);
        assert_eq!(line.records.len(), 3);
        assert_eq!(bounds_of(line).x0, 10.0);
        assert_eq!(
            bounds_of(line).x1,
            120.0,
            "the line is as wide as its widest run"
        );
    }

    #[test]
    fn runs_on_different_baselines_stay_separate_lines() {
        let records = vec![
            run(10.0, 700.0, 50.0, 712.0, 0),
            run(10.0, 680.0, 50.0, 692.0, 4),
        ];
        let m = PageModel::build(&records);
        assert_eq!(m.objects().len(), 2, "two baselines are two lines");
        assert!(m.objects().iter().all(|o| o.kind == Kind::Line));
    }

    #[test]
    fn a_line_carries_every_span_it_covers() {
        let records = vec![run(0.0, 0.0, 10.0, 12.0, 0), run(12.0, 0.0, 30.0, 12.0, 4)];
        let m = PageModel::build(&records);
        let spans = &m.objects()[0].spans;
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].start, 0);
        assert_eq!(spans[1].start, 4);
        assert!(
            spans[0].end <= spans[1].start,
            "the spans do not overlap — they abut, because two operations can share a boundary: {spans:?}"
        );
    }

    /// A run at a very different size is a heading, not part of the line beside it. Grouping it
    /// by baseline alone would make one unselectable lump of a heading and a paragraph.
    #[test]
    fn a_run_of_a_very_different_size_is_left_out_of_the_line() {
        let records = vec![
            run(10.0, 700.0, 50.0, 712.0, 0),
            run(60.0, 700.0, 200.0, 740.0, 4),
        ];
        let m = PageModel::build(&records);
        let kinds: Vec<Kind> = m.objects().iter().map(|o| o.kind).collect();
        assert!(
            kinds.contains(&Kind::LooseRun),
            "the oversized run is selectable on its own: {kinds:?}"
        );
        assert_eq!(
            m.objects()
                .iter()
                .find(|o| o.kind == Kind::LooseRun)
                .and_then(|o| o.line_break),
            Some(LineBreak::DifferentSize)
        );
    }

    #[test]
    fn an_image_is_one_object_and_is_never_joined_to_anything() {
        let records = vec![
            run(0.0, 0.0, 10.0, 12.0, 0),
            image(4),
            run(12.0, 0.0, 30.0, 12.0, 8),
        ];
        let m = PageModel::build(&records);
        let images: Vec<_> = m
            .objects()
            .iter()
            .filter(|o| o.kind == Kind::Image)
            .collect();
        assert_eq!(images.len(), 1, "one photo is one thing to a designer");
        assert_eq!(images[0].records.len(), 1);
    }

    /// Objects come out where the file drew them, not grouped at the end, so a selection panel
    /// reads in the same order the page does.
    #[test]
    fn objects_come_out_in_drawing_order() {
        let records = vec![
            run(10.0, 700.0, 50.0, 712.0, 0),
            image(4),
            run(10.0, 680.0, 50.0, 692.0, 8),
        ];
        let m = PageModel::build(&records);
        let kinds: Vec<Kind> = m.objects().iter().map(|o| o.kind).collect();
        assert_eq!(kinds, vec![Kind::Line, Kind::Image, Kind::Line]);
    }

    #[test]
    fn a_click_returns_the_object_whose_box_holds_the_point() {
        // Two lines, one above the other, so there is a gap a click can land in. Two runs on
        // the *same* baseline would be one line spanning the whole width, and the gap between
        // them would be inside it.
        let records = vec![
            run(10.0, 700.0, 140.0, 712.0, 0),
            run(10.0, 600.0, 140.0, 612.0, 4),
        ];
        let m = PageModel::build(&records);
        let hit = m.hit(30.0, 705.0).expect("inside the upper line");
        assert_eq!(hit.bounds.y0, 700.0);
        assert!(
            m.hit(30.0, 650.0).is_none(),
            "between the lines is nothing: {:?}",
            m.objects()
        );
        assert!(m.hit(30.0, 500.0).is_none(), "below both is nothing");
    }

    /// Two objects overlapping: the one drawn later is what a user can see, so it is what a
    /// click should return.
    #[test]
    fn a_click_on_an_overlap_returns_the_topmost() {
        let mut bottom = run(0.0, 0.0, 100.0, 100.0, 0);
        bottom.mark = Mark::Image {
            name: Some("under".into()),
            matrix: Matrix::new(1.0, 0.0, 0.0, 1.0, 0.0, 0.0),
            inline: false,
            fill: Colour::black(),
        };
        let top = run(0.0, 0.0, 100.0, 100.0, 4);
        let m = PageModel::build(&[bottom, top]);
        let hit = m.hit(50.0, 50.0).expect("inside both");
        assert_eq!(hit.kind, Kind::Line, "the later object is on top");
    }

    #[test]
    fn counts_say_what_is_on_the_page() {
        let records = vec![
            run(0.0, 0.0, 10.0, 12.0, 0),
            run(12.0, 0.0, 30.0, 12.0, 4),
            image(8),
        ];
        let m = PageModel::build(&records);
        let counts = m.counts();
        assert_eq!(counts.get("line"), Some(&1));
        assert_eq!(counts.get("image"), Some(&1));
    }

    /// The same stream twice gives the same model. Grouping that depended on hash order would
    /// make every test of it flaky.
    #[test]
    fn grouping_is_deterministic() {
        let records: Vec<Record> = (0..40u32)
            .map(|i| {
                let y = 700.0 - f64::from(i) * 14.0;
                let x = 10.0 + f64::from(i % 3);
                let at = usize::try_from(i * 4).unwrap_or(0);
                run(x, y, 90.0, y + 12.0, at)
            })
            .collect();
        let first = PageModel::build(&records);
        let second = PageModel::build(&records);
        assert_eq!(first, second);
        assert_eq!(
            first.objects().len(),
            40,
            "each baseline is its own line here"
        );
    }
}
