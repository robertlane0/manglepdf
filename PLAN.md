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

## Composite fonts (settled)

A `/Type0` font's codes are two bytes, so the byte count and the glyph count stop agreeing.
Four things changed together, and each is a place where a wrong answer is invisible rather
than loud:

- **The code width is the font's claim, not an inference.** `Resources` records which
  resource names are composite (`/Subtype /Type0`), and `Tf` puts that on the text state
  beside the widths. Inferring it from the presence of a widths array would be wrong in both
  directions: a composite font may declare no `/W` at all, and a simple font always has one.
- **The string is split into codes, and everything downstream counts codes** — placements,
  advances and `TJ` kerns. A trailing odd byte is dropped rather than padded: half a code
  names no glyph, and a glyph nobody asked for is worse than a character missing from a
  truncated string.
- **The width comes from the run-length `/W`.** `CidWidths` keeps the runs as written
  (`c [w1 w2 …]`, `c_first c_last w`) rather than expanding them, because a range run can
  cover the whole 16-bit code space and expanding it would cost 65 536 entries to answer a
  subtraction. `/DW` is 1000 for a code no run covers — the specification's answer, not a
  fallback invented here. `Declared` is untouched: a Type 1 font is the common case and must
  not pay for a two-byte path.
- **The mark carries the codes.** `Mark::Glyphs` has `codes: Vec<u32>` and `two_byte: bool`
  beside the raw `text`. A mark that said only how many bytes it had would leave every
  renderer to guess, and guessing one byte per code is the bug. `/ToUnicode` is not read —
  it is text extraction's, not the drawer's — but the mark now carries what it will need.

## CFF charstrings (settled)

A `CFF ` table's glyphs are Type 2 programs, not point lists, so reading one means running
it: a stack machine over `f32`, local and global subroutines, the subroutine-number bias,
stem hints, and a width that is either the first operand on the stack or absent. Two
decisions are worth recording because the wrong version of either produces a *plausible*
wrong glyph rather than an error:

- **A curve operator's three points are offsets from the point before each of them**, the
  first from the pen, the second from the first, the third from the second. Reading all
  three from the pen yields a closed, well-formed glyph that is simply too narrow. Every
  glyph of every CFF font on this machine is now cross-checked against `ttf-parser`'s
  independent interpreter, coordinate for coordinate, because one hand-written fixture
  cannot see this class of mistake.
- **Three cases are refused with a reason rather than drawn blank or drawn wrongly.** A bare
  `/Subtype /Type1C` name-keyed font cannot resolve a character code, because that needs the
  charset plus the `/Encoding` — its outlines are readable, so the report says exactly that.
  A CID-keyed CFF is read through `/FDArray` and all three `/FDSelect` forms, but only under
  an identity charset, since a renumbering one would silently substitute letters. `seac` and
  a Top DICT declaring `CharstringType 1` both build one glyph from two others *by name* and
  are refused for the same reason.

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

1. **The font kinds** (F19–F21): Type 1 charstrings next, then the standard fourteen, then
   Type 3. TrueType is done and compared against an oracle, and two-byte codes are done with
   it: a composite font's string is split into two-byte CIDs, the pen advances by each CID's
   own entry in the descendant's run-length `/W`, and the mark carries the codes and their
   width so no consumer has to guess (0.99225 SSIM against `mutool`). CFF is done too, and
   differently — a `CFF ` table's glyphs are programs, so they are executed rather than
   walked: a Type 2 stack machine with local and global subroutines, the subroutine-number
   bias tested at both ends of each of its three ranges, every curve and flex operator, and
   the width either present as the first operand or absent (0.98654 SSIM against `mutool`).
   Every glyph of every CFF font on this machine agrees with `ttf-parser`'s independent
   interpreter, coordinate for coordinate — 28 347 outlines, none differing — which is how
   the defect that mattered most was found: a curve operator's points are offsets from the
   point *before* each of them, not all from the pen, and reading them from the pen gives a
   closed, plausible, wrong glyph rather than an error.
2. **Patterns**: the tiling loop and the colour-space converter, for the painting and
   shading pattern types.
3. **JBIG2 and JPEG 2000 decoders** (F17, F18), or an explicit scope statement if they are
   not going to be built.
4. **Fill the Inspector's right-hand region** from the object model, which is the window work
   that has not been started.
5. **Break the symmetry of the remaining fixtures.** The diagonal-clip page is asymmetric now
   and scored 0.51652 when its clip was a box, where every symmetric fixture had scored above
   0.99 through the same bug. The rest still need the same treatment.
6. Tier B with `SOURCES.md`: a real-world corpus, which is the only way to find the encoding
   problems a hand-written fixture cannot express.

Settled and no longer queued: images decode and draw, axial and radial shadings paint, the
clip bounds are transformed by exactly one matrix, a clip is now the path the page set rather
than the box around it — the interpreter records the path, its rule and its box together, and
the renderer rasterises the path into a per-pixel mask it multiplies into every fill, so a
diagonal clip is a diagonal and a circular one is round (0.99639 SSIM against `mutool` at
150 DPI) — glyphs fill from embedded TrueType or CFF
outlines through the same filler a path uses, the text matrix and the font size no longer
double-count (a `TJ` kern also displaces the glyph that follows it, not the one at the same
index), and the standard-14 width tables are verified rather than trusted — every one of the
1043 widths was measured against two independent renderers and Adobe's own metrics, so
nobody should re-derive them from a local URW clone.

The clip work is otherwise finished: a clip stays in force until a `W` changes it or a `Q`
restores it, so a mark's rendering depends only on its own record. What is left is the cap —
a page's clip keeps at most 32 paths, past which the outermost are dropped for box-only
culling, and the fix is one shared sweep over the paths rather than a larger number.

One loose end on the write side: `StreamCompression::Flate` is defined but not wired into
`encode_stream`, so a stream chosen for writing is not compressed. What the compressor now
emits is a zlib stream, which `/FlateDecode` names and three other programs can read, so the
work left is the call site rather than the format.
