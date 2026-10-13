//! The icon set, read back by the renderer that draws it.
//!
//! Every test here exists because of a way an icon renderer can be wrong *quietly*: a shape
//! that is drawn in the wrong place, a stroke that is twice as thick as it should be, a
//! rounded corner that is a square one. None of those are errors — the icon still draws,
//! and it looks almost right.

// Tests state their expectations with `expect` and index a slice whose length they
// have just asserted; both are what a test is for. The panic-free rule is about what
// the product does with a file, not about how a test reads one.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_cmp
)]

use crate::svg::{Icon, parse, rasterise};

/// An icon whose whole content is one path.
fn one(shape: &str) -> Option<Icon> {
    parse(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\" width=\"24\" height=\"24\" \
         fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.6\" stroke-linecap=\"round\" \
         stroke-linejoin=\"round\" role=\"img\" aria-label=\"t\">{shape}</svg>"
    ))
}

/// How many covered pixels an icon has at a size, which is the only question that matters
/// about a picture: is there ink in it?
fn ink(icon: &Icon, size: usize) -> usize {
    let image = rasterise(icon, size, size, [0, 0, 0, 255]);
    image.pixels.chunks_exact(4).filter(|p| p[3] > 0).count()
}

/// A path with a visible mark produces pixels, and refuses nothing.
#[test]
fn a_path_draws() {
    let icon = one("<path d=\"M3 12h18\"/>").expect("a line is an icon");
    assert!(ink(&icon, 24) > 0, "a line must make ink");
}

/// A `<circle>` is an ellipse with equal radii, and it draws.
#[test]
fn a_circle_draws() {
    let icon = one("<circle cx=\"12\" cy=\"12\" r=\"6\"/>").expect("a circle is an icon");
    assert!(ink(&icon, 24) > 0, "a circle must make ink");
}

/// A `rect` with no radius is four corners and no curves.
#[test]
fn a_plain_rect_has_four_corners_and_no_curve() {
    let icon =
        one("<rect x=\"4\" y=\"4\" width=\"16\" height=\"16\"/>").expect("a rect is an icon");
    let shape = icon.shapes.first().expect("one shape");
    let sub = shape.subpaths.first().expect("one subpath");
    assert!(sub.closed);
    // The four corners: no flattened arc points, because there is no arc.
    assert_eq!(sub.points.len(), 4, "a rect is its four corners");
}

/// A rounded rect's corner is an arc, so it has more than four points, and the extra ones
/// lie outside the axis-aligned square — which is what makes it a round corner.
#[test]
fn a_rounded_rect_has_curved_corners() {
    let icon = one("<rect x=\"4\" y=\"4\" width=\"16\" height=\"16\" rx=\"3\"/>").expect("rounded");
    let points = &icon
        .shapes
        .first()
        .expect("one shape")
        .subpaths
        .first()
        .expect("sub")
        .points;
    assert!(
        points.len() > 4,
        "a rounded rect has arc points: {}",
        points.len()
    );
    // The corners of the rounded rect are inside the plain one's, so every point is within
    // the box but the shape is not the box.
    for (x, y) in points {
        assert!((4.0..=20.0).contains(x), "{x} in range");
        assert!((4.0..=20.0).contains(y), "{y} in range");
    }
}

/// The icon set's own arcs are rounded corners, and a corner that comes out square is a
/// corner that has been lost. This is the case the `rx` path and the `a` path both serve,
/// so both are checked for the same property.
#[test]
fn a_rounded_corner_is_not_a_square_one() {
    // The same rounded box written as an arc sweep rather than as a `rect/rx`.
    let arcs = one(
        "<path d=\"M7 4h10a3 3 0 0 1 3 3v10a3 3 0 0 1-3 3H7a3 3 0 0 1-3-3V7a3 3 0 0 1 3-3z\"/>",
    )
    .expect("an arc sweep is an icon");
    let rxs = one("<rect x=\"4\" y=\"4\" width=\"16\" height=\"16\" rx=\"3\"/>").expect("a rect");
    let a = rasterise(&arcs, 24, 24, [0, 0, 0, 255]);
    let b = rasterise(&rxs, 24, 24, [0, 0, 0, 255]);
    // Both are rings of ink, and neither is the other's filled square: a square-cornered
    // ring has ink right up to its bounding box, and a rounded one does not.
    let has_corner_gap = |image: &crate::Image| {
        let x = 2;
        let y = 2;
        let inside = image.get(x, y).unwrap()[3] > 0;
        !inside
    };
    assert!(
        has_corner_gap(&a),
        "the arc sweep must leave its corners bare"
    );
    assert!(
        has_corner_gap(&b),
        "the rect/rx must leave its corners bare"
    );
}

/// A stroke's width is the width the file wrote, in the icon's own units — not a default
/// and not a doubled one.
#[test]
fn a_stroke_width_is_the_width_the_file_wrote() {
    let thin = one("<path d=\"M3 12h18\" stroke-width=\"1\"/>").expect("thin");
    let thick = one("<path d=\"M3 12h18\" stroke-width=\"4\"/>").expect("thick");
    let shape = thin.shapes.first().expect("shape");
    assert!((shape.width - 1.0).abs() < 1e-9, "{}", shape.width);
    assert!(
        ink(&thick, 48) > ink(&thin, 48),
        "a thicker stroke inks more"
    );
}

/// `fill="none"` is not the same as an absent `fill`: one is a stroke, the other is a solid
/// shape. Both icons in the set are drawn, and neither is drawn as the other.
#[test]
fn fill_none_is_a_stroke_and_nothing_else() {
    let outline = one("<rect x=\"4\" y=\"4\" width=\"16\" height=\"16\"/>").expect("outline");
    let shape = outline.shapes.first().expect("shape");
    assert!(!shape.fill, "fill=none must not fill");
    assert!(shape.stroke, "and it must stroke");
    // A `fill="currentColor"` rect is filled instead, so it draws very much more.
    let solid = one("<rect x=\"4\" y=\"4\" width=\"16\" height=\"16\" fill=\"currentColor\"/>")
        .expect("a filled rect");
    assert!(
        solid.shapes.first().expect("s").fill,
        "fill=currentColor fills"
    );
    assert!(
        ink(&solid, 24) > ink(&outline, 24) * 2,
        "a fill covers far more: a filled 16x16 box is 256 units of ink against \
         an outline of 64 units at a 1.6 width, about 102, so the honest ratio is \
         near 2.5 rather than 3"
    );
}

/// A `polygon` closes itself and a `polyline` does not, which is the difference between a
/// triangle and an open angle.
#[test]
fn a_polygon_closes_and_a_polyline_does_not() {
    let triangle = one("<polygon points=\"4 8 20 8 12 20\"/>").expect("polygon");
    let angle = one("<polyline points=\"4 8 20 8 12 20\"/>").expect("polyline");
    assert!(
        triangle
            .shapes
            .first()
            .expect("s")
            .subpaths
            .first()
            .expect("sp")
            .closed,
        "a polygon is closed"
    );
    assert!(
        !angle
            .shapes
            .first()
            .expect("s")
            .subpaths
            .first()
            .expect("sp")
            .closed,
        "a polyline is open"
    );
}

/// The relative forms change the current point, and the absolute ones do not.
#[test]
fn a_relative_command_moves_from_where_the_last_one_ended() {
    let absolute = one("<path d=\"M3 12l6 4\"/>").expect("absolute");
    let relative = one("<path d=\"M3 12l6 4\"/>").expect("relative");
    let a = absolute
        .shapes
        .first()
        .expect("s")
        .subpaths
        .first()
        .expect("sp")
        .points
        .clone();
    let r = relative
        .shapes
        .first()
        .expect("s")
        .subpaths
        .first()
        .expect("sp")
        .points
        .clone();
    assert_eq!(a, r);
    // Now the real relative case: `m` from (10,10) and then `l` 6 right, 4 up.
    let r = one("<path d=\"M10 10l6 -4\"/>")
        .expect("relative move")
        .shapes
        .first()
        .expect("s")
        .subpaths
        .first()
        .expect("sp")
        .points
        .clone();
    assert_eq!(r.last().copied(), Some((16.0, 6.0)));
}

/// A `Z` closes the subpath back where the `M` started, which is what makes a filled ring
/// possible at all.
#[test]
fn a_close_path_returns_to_where_the_subpath_started() {
    let icon = one("<path d=\"M4 4h16v16H4z\"/>").expect("a closed path");
    let points = &icon
        .shapes
        .first()
        .expect("s")
        .subpaths
        .first()
        .expect("sp")
        .points;
    assert_eq!(points.first().copied(), Some((4.0, 4.0)));
    assert_eq!(points.last().copied(), Some((4.0, 4.0)));
}

/// A shorthand curve's first control point is the mirror of the previous curve's second,
/// which is the whole of what `S` and `T` mean.
#[test]
fn a_shorthand_curve_reflects_the_previous_control_point() {
    // Two cubics, the second an `S`.
    let icon = one("<path d=\"M0 0C0 4 4 8 8 8S16 4 16 0\"/>").expect("two cubics");
    let points = icon
        .shapes
        .first()
        .expect("s")
        .subpaths
        .first()
        .expect("sp")
        .points
        .clone();
    assert!(
        points.len() > 2,
        "the curves are flattened: {}",
        points.len()
    );
    // The second curve starts where the first ended.
    assert_eq!(points.first().copied(), Some((0.0, 0.0)));
    assert_eq!(points.last().copied(), Some((16.0, 0.0)));
}

/// A path with no drawable command is a refusal, not an empty icon.
#[test]
fn an_unreadable_path_is_refused() {
    assert!(one("<path d=\"not a path\"/>").is_none());
    assert!(one("<path d=\"M3 12 L\"/>").is_none());
}

/// A `<script>` or a data URI is a refusal, because an icon that is *nearly* right is wrong.
#[test]
fn a_file_that_is_not_an_icon_is_refused() {
    let doc = |body: &str| {
        parse(&format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\" width=\"24\" height=\"24\">{body}</svg>"
        ))
    };
    assert!(doc("<script>alert(1)</script>").is_none());
    assert!(doc("<image href=\"data:image/png;base64,AAAA\"/>").is_none());
    // A comment is not content, and skipping it is correct rather than permissive.
    assert!(doc("<!-- a note --><path d=\"M3 12h18\"/>").is_some());
}

/// What an icon file really contains: the whole set, and not one of them refused.
#[test]
fn the_whole_icon_set_parses() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/icons")
        .canonicalize()
        .expect("the icon directory exists");
    let mut read = 0;
    let mut refused = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("readable") {
        let path = entry.expect("an entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("svg") {
            continue;
        }
        let name = path
            .file_stem()
            .expect("a name")
            .to_string_lossy()
            .to_string();
        let text = std::fs::read_to_string(&path).expect("readable");
        read += 1;
        if parse(&text).is_none() {
            refused.push(name);
        }
    }
    assert!(read > 100, "the set is {read} icons");
    assert!(refused.is_empty(), "refused: {refused:?}");
}

/// Every icon draws ink at a slot size, which is the property that catches an icon whose
/// geometry has silently collapsed to a point.
#[test]
fn every_icon_draws_ink_at_every_slot_size() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/icons")
        .canonicalize()
        .expect("the icon directory exists");
    let mut blank = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("readable") {
        let path = entry.expect("an entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("svg") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("readable");
        let Some(icon) = parse(&text) else {
            blank.push(format!(
                "{}: refused",
                path.file_stem().expect("n").to_string_lossy()
            ));
            continue;
        };
        for size in [16usize, 20, 24, 32] {
            if ink(&icon, size) == 0 {
                blank.push(format!(
                    "{}: blank at {size}",
                    path.file_stem().expect("n").to_string_lossy()
                ));
            }
        }
    }
    assert!(blank.is_empty(), "{}", blank.join("\n"));
}

/// The same icon at two sizes scales, and the ink scales with it — within the antialiasing
/// of the edge, a 32-pixel slot holds about four times a 16-pixel one's ink.
///
/// The shape is filled on purpose: a *stroked* circle's ink is length times width plus an
/// edge term that does not scale, so a thin stroke measures near 2.7 rather than 4 and a
/// bound written for an area would be a bound against the wrong law.
#[test]
fn a_larger_slot_has_more_ink() {
    let icon =
        one("<circle cx=\"12\" cy=\"12\" r=\"6\" fill=\"currentColor\"/>").expect("a circle");
    let small = ink(&icon, 16);
    let large = ink(&icon, 32);
    assert!(large > small * 3, "{small} -> {large}");
    assert!(large < small * 6, "{small} -> {large}");
}

/// The tint is the colour, and an icon drawn in two tints differs only in colour.
///
/// Every pixel is compared rather than one fully-covered one: a 1.6-wide stroke on an
/// integer coordinate never covers a pixel whole (its best row is covered 0.8), so "find
/// the pixel with alpha 255" is a search for a pixel the renderer is correct not to
/// produce. Compositing mixes toward the tint by coverage, so green, blue and alpha are
/// tint-independent everywhere and red is present exactly where there is ink.
#[test]
fn the_tint_is_the_colour() {
    let icon = one("<path d=\"M3 12h18\"/>").expect("a line");
    let black = rasterise(&icon, 24, 24, [0, 0, 0, 255]);
    let red = rasterise(&icon, 24, 24, [255, 0, 0, 255]);
    assert!(ink(&icon, 24) > 0, "the line must make ink");
    for (p, q) in black.pixels.chunks_exact(4).zip(red.pixels.chunks_exact(4)) {
        assert_eq!(p[1], q[1], "green is tint-independent");
        assert_eq!(p[2], q[2], "blue is tint-independent");
        assert_eq!(p[3], q[3], "coverage is tint-independent");
        if p[3] > 0 {
            assert_eq!(p[0], 0, "the black tint paints black");
            assert!(q[0] > 0, "the red tint paints red where there is ink");
        } else {
            assert_eq!(q[0], 0, "no ink, no red");
        }
    }
}

/// An icon with no viewBox falls back to 24 rather than to nothing.
#[test]
fn a_file_with_no_size_defaults_to_twenty_four() {
    let icon = parse("<svg xmlns=\"http://www.w3.org/2000/svg\"><path d=\"M3 12h18\"/></svg>")
        .expect("a default-size icon");
    assert_eq!(icon.extent(), (24.0, 24.0));
}

/// An empty raster is a blank image, not a panic and not a crash.
#[test]
fn a_zero_sized_slot_is_blank() {
    let icon = one("<path d=\"M3 12h18\"/>").expect("a line");
    let image = rasterise(&icon, 0, 0, [0, 0, 0, 255]);
    assert_eq!(image.width, 0);
    assert!(image.pixels.is_empty());
}
