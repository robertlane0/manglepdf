# ADR-0007: Shared analytic scanline rasterizer for PDF pages and SVG icons

**Status:** accepted

## Context

`GOAL.md` §8.7 requires hand-authored SVG icons rendered at runtime through either a small in-house
SVG-subset renderer built on the PDF vector rasterizer (dogfooding) or a justified crate, cached by
`(name, size, scale, tint)` and crisp at 1×/2×/3×.

## Decision

Build one analytic-coverage scanline rasterizer in `mangle-render::fill` and share it between:

1. PDF path filling, stroking (`stroke_outline`), glyph outlines, and per-pixel clipping masks; and
2. `mangle-render::svg`, which parses the constrained SVG icon vocabulary (`path`, `rect`, `circle`,
   `ellipse`, `line`, `polyline`, `polygon`, `g`) and rasterizes strokes and fills through the exact
   same edge builder and coverage accumulator.

## Consequences

- Zero third-party SVG dependency in product code.
- Any improvement or bugfix to stroke joins, caps, cubic/quadratic flattening, or anti-aliased
  scanline coverage immediately benefits both page rendering and UI icon rendering.
