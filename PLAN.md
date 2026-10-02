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

1. **The font kinds** (F19–F21): CFF and Type 1 charstrings, then the standard fourteen,
   then Type 3, then two-byte codes. TrueType is done and compared against an oracle.
2. **Keep a clip in force for more than the next mark.** The interpreter records the clip on
   every record, but the renderer resets it after each mark drawn, so a clip applies to one
   mark rather than to the rest of the page. The render loop should trust `record.clip`.
3. **Patterns**: the tiling loop and the colour-space converter, for the painting and
   shading pattern types.
4. **JBIG2 and JPEG 2000 decoders** (F17, F18), or an explicit scope statement if they are
   not going to be built.
5. **Fill the Inspector's right-hand region** from the object model, which is the window work
   that has not been started.
6. **Break the symmetry of the remaining fixtures.** The diagonal-clip page is asymmetric now
   and scored 0.51652 when its clip was a box, where every symmetric fixture had scored above
   0.99 through the same bug. The rest still need the same treatment.
7. Tier B with `SOURCES.md`: a real-world corpus, which is the only way to find the encoding
   problems a hand-written fixture cannot express.

Settled and no longer queued: images decode and draw, axial and radial shadings paint, the
clip bounds are transformed by exactly one matrix, a clip is now the path the page set rather
than the box around it — the interpreter records the path, its rule and its box together, and
the renderer rasterises the path into a per-pixel mask it multiplies into every fill, so a
diagonal clip is a diagonal and a circular one is round (0.99639 SSIM against `mutool` at
150 DPI) — glyphs fill from embedded TrueType
outlines through the same filler a path uses, the text matrix and the font size no longer
double-count (a `TJ` kern also displaces the glyph that follows it, not the one at the same
index), and the standard-14 width tables are verified rather than trusted — every one of the
1043 widths was measured against two independent renderers and Adobe's own metrics, so
nobody should re-derive them from a local URW clone.

One loose end on the write side: `StreamCompression::Flate` is defined but not wired into
`encode_stream`, so a stream chosen for writing is not compressed. What the compressor now
emits is a zlib stream, which `/FlateDecode` names and three other programs can read, so the
work left is the call site rather than the format.
