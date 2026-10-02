# ManglePDF — Plan

> The authoritative specification is [`GOAL.md`](GOAL.md); the definition of done is
> [`FINISH.md`](FINISH.md). This file is the *working* plan: what is being built right
> now, in what order, and why. Keep it short. Living status lives in
> [`docs/STATUS.md`](docs/STATUS.md); architectural decisions live in `docs/decisions/`.

## Shape of the work

The product is one vertical spine plus breadth:

```
spine:  open → parse (lossless) → render → select → edit → save → reopen elsewhere
```

Everything else (annotations, forms, pages, protect, export) hangs off the spine.
The spine is where the architecture is decided, so it is built first and deeply.

## Key architectural bets (see `docs/decisions/`)

| # | Bet | Why |
|---|---|---|
| ADR-0001 | Lossless COS layer: every object kept verbatim (raw stream bytes, original `/Filter` chain), lazily resolved from file bytes via xref; edits live in an object-level copy-on-write overlay | Enables byte-for-byte preservation of everything we do not touch (Charter §4.1 law 1) and incremental saves (FINISH S1.17) |
| ADR-0002 | Content streams are parsed into a token/operator stream that **retains byte spans**; the page-object model maps every object back to its provenance ranges | Makes surgical write-back, the Inspector, and "what changed" diffing possible |
| ADR-0003 | In-house Flate **inflate** (must return partial output for truncated streams) and in-house deflate | `flate2`/`miniz_oxide` abort on corruption; a bomb must return partial data, not fail |
| ADR-0004 | egui + eframe (winit) for the GUI, everything custom-painted from design tokens | Pure Rust, no toolkit look to fight, easy canvas, HiDPI, IME, and accessibility via AccessKit |
| ADR-0005 | `ttf-parser` + `rustybuzz` for shaping only; all PDF font program handling (CFF, Type 1, subsetting, CMaps, encodings) in-house | Shaping is not PDF-shaped; font *program* handling is (Charter §2.3) |
| ADR-0006 | Permitted codecs only for JPEG (DCT) decode/encode; PNG is in-house on top of our own Flate | PNG needs exact control of predictors/bit depths; JPEG is too large to justify rewriting twice |
| ADR-0007 | Own analytic-coverage scanline rasterizer shared by PDF rendering *and* SVG icon rendering | One rasterizer to get right; dogfoods the renderer (Charter §8.7) |

## Milestone map (detail in GOAL §10)

| M | Deliverable | Gate it unlocks |
|---|---|---|
| **M0** | Workspace, lints, `xtask policy`, docs, fixturegen skeleton, themed empty window at 1536×1024, icon pipeline + gallery + lint | Gate 0 |
| **M1** | Lexer/parser, xref + repair, object streams, decryption, page tree, full + incremental writer, round-trip tests, Inspector object tree | M1 exit: F01–F11 |
| **M2** | Interpreter, paths/clips/text, tiles, viewer shell with thumbnails/zoom/nav | M2 exit: F31 p1 composition |
| **M3** | All fonts + CMaps, colour spaces, patterns, shadings, transparency, CCITT/JBIG2/JPX, OCGs | Gate 3 render fidelity |
| **M4** | Page objects + provenance, select/move/scale/delete/recolour/Arrange, undo/redo, first save→reopen-elsewhere loop | U1/U2/U3 |
| **M5** | Text model, editing, reflow, font resolution/embedding/`ToUnicode`, Text Properties panel | Gauntlet S1 |
| **M6** | AP generators, foreign annotations, comments, add text/image/shape/link, header/footer/watermark, flatten | Gauntlet S3–S5 |
| **M7** | Fill, calculation/format/validate, Prepare Form, FDF/XFDF | Gauntlet S6 |
| **M8** | Organizer, import/merge/split, retargeting | Gauntlet S7 |
| **M9** | Redaction, sanitize, encryption write, signatures | Gauntlet S8–S9 |
| **M10** | Export/optimize, performance, fuzzer, crash safety | Gates 2 and 7 |
| **M11** | UI states, full icon set, dark theme, HiDPI, responsive | Gate 8 ≥ 90 |
| **M12** | Gauntlet dry-run, fixes, formal run, evidence pack, independent verification | All |

## Immediate queue

1. **Shadings** (F15): axial and radial are pure maths and `G3.2` wants analytic
   expectations for their sample points, so they are checkable by formula. Then patterns,
   which need a tiling loop.
2. **Glyph outlines** (F19–F21): TrueType first via `ttf-parser`, then CFF and Type 1.
   Advances for *existing* text come from `/Widths` and need no font program at all, which
   is what the charter asks for.
3. **A real clip region** rather than the clip path's bounding box.
4. **JBIG2 and JPEG 2000 decoders** (F17, F18), or an explicit scope statement if they are
   not going to be built.
5. **The page tree and the marks into the window**: fill the right-hand region, which is
   the Inspector's first half.
6. Tier-A fixtures for the remaining catalogue entries, then Tier B with `SOURCES.md`.
