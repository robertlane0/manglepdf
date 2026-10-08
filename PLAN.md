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

## Type 1 charstrings (settled)

A Type 1 font is a PostScript program that happens to be stored in two encrypted halves, so
reading a glyph means three things in order: find `eexec`, undo the outer cipher, undo a
second inner cipher over the charstrings. Each layer is `p = c XOR (r >> 8)` with
`r = ((c + r) * 52845 + 22719) mod 65536` — and the recurrence is fed the **ciphertext**
byte, which is the one place the algorithm can be read two ways and the wrong reading still
produces printable output. The seeds are what separate the layers: 55665 for `eexec`, whose
first four plaintext bytes are a salt to discard, and 4330 for the charstrings, advanced
`lenIV - 4` times, which is why the usual `lenIV` of 4 — and a font that mentions none —
needs no advance at all.

Five things are worth recording, because the wrong version of any of them produces a
*plausible* wrong glyph rather than an error:

- **`hvcurveto` and `vhcurveto` take four operands whose fourth is the endpoint's x for
  one and its y for the other** (`dy1 dx2 dy2 dx3` and `dx1 dx2 dy2 dy3`). Reading the fourth
  and fifth the wrong way round swaps them, which turns every round letter into a shape that
  closes and does not match. Cost 0.15 of SSIM on a page of ordinary text before it was found.
- **`hsbw`'s first operand says where the outline sits relative to the pen**, and the
  charstring's coordinates do not include it. Leaving it out puts every glyph a whole
  sidebearing to the left of where it belongs.
- **There is no subroutine-number bias.** Adding CFF's lands on a different, in-range
  subroutine and runs the wrong program.
- **A hint mask's length is a property of the code it is written in**: a subroutine's mask
  covers the stems *that subroutine* declared, so it is empty unless it declares stems of its
  own. Reading it from the caller's running total skips bytes that belong to the subroutine's
  operators, which turns the standard hint-replacement subroutine into a prefix of nonsense.
- **The absolute-`dy` convention is not applied, and that is measured rather than assumed.**
  The specification records that the last `dy` of the first `rrcurveto` after a mover is an
  absolute y. Every one of the 28 Type 1 faces installed here was converted from a CFF
  outline and is written to the *other* convention, and applying the rule moves the endpoint
  of the first curve in 18 of the 94 printable ASCII glyphs of `NimbusSans-Regular`. The same
  face is installed as OpenType/CFF and can be compared glyph by glyph: 18 mismatches with the
  convention applied, 2 without, and `mutool` — the oracle the acceptance criteria name —
  agrees with the reading without it.

**Refused, each with a reason rather than a wrong shape:** a PostScript `OtherSubrs`
procedure with no conventional number (the four that have one — the flex at 0, 1 and 2, and
hint replacement at 3 — are implemented); Multiple Master `callothersubr` 14–28; `seac`;
operator 15, which is reserved and undocumented; and a PFB container, whose segment headers
are not stripped. **Hint replacement is refused and nothing else is**: it asks for the stem
hints to be recomputed for a rasterizer's pixel grid, this renderer computes exact analytic
coverage and has no grid, so the hint is dropped and the glyph's geometry is untouched.

A page's own `/Encoding` is *not* consulted inside the Type 1 reader — the font's own is, which
is what makes a Type 1 program able to resolve a character code at all where a bare CFF cannot.
The page's encoding is consulted one layer up instead, by `mangle-render`, which turns the
code into a glyph *name* before asking any font program for a glyph (see "Encodings"). Putting
it here would have made this the only font type whose page encoding was honoured, and putting it
in the interpreter would have made it so for no font type at all.

## Encodings (settled)

A character code means nothing on its own. It is the `/Encoding` that turns it into a glyph
*name*, and the name that a font is looked up by — which is why a page that remaps one code
through `/Differences` used to render that code with the wrong glyph for every font type. The
tables are in `mangle-font::encoding`, transcribed from Table D and Annex D.2, and each was
cross-checked against a source already on the machine rather than trusted: Ghostscript's own
decoding table for StandardEncoding, its init file's construction for WinAnsi, and the
machine's `mac_roman` code page together with poppler's `MacRomanEncoding` for MacRoman, the
last two agreeing on every code from 32 to 225. The AGL resolver holds 912 names from Adobe's
Glyph List, which is a machine source — so the Greek and Cyrillic blocks are complete rather
than restricted to Latin.

Three decisions worth recording:

- **The `/Encoding` is read in `mangle-render`, not in the font readers and not in the
  interpreter.** `mangle-font` is below the content layer and cannot see a page; the
  interpreter would have to know about fonts to ask this question. One call site, one place
  the answer can be wrong.
- **A composite font keeps the CID path untouched.** Its code is a CID, not a character code,
  and the specification says `/Differences` does not apply to one. Applying an encoding to a
  number that is not a character would be wrong in the same way a CID looked up through a
  `cmap` would be.
- **A name reaches a glyph by two routes, and a Type 1 font by the direct one.** A Type 1
  program's `CharStrings` are keyed by name, so the name *is* the lookup. A TrueType font is
  found through the `post` table if it carries names and otherwise through the Unicode
  subtables by way of the AGL — the (3,0) subtable stays reserved for the code, because that
  is a symbolic font's author's own choice of code for a glyph and it is already consulted in
  the right order.

Verified at **0.98780 SSIM** against `mutool` on a page remapping eighteen codes, four of them
in the Latin-1 range where WinAnsi and MacRoman disagree completely. The fixture's remaps are
not a permutation of each other and the page is asymmetric, so it catches ignoring the array,
stopping at the end of the base, and transposing two codes; renaming `/Differences` in the
file and re-scoring drops it to 0.895, which is how the test is known to see what it is for.

## Standard-14 substitution (settled)

The standard fourteen are defined by their *metrics* rather than by any one program, so a
document that names one without embedding it — which is most of the LaTeX and office output
in the wild, because those producers assume the reader has the font — is naming a font, not
naming nothing. Refusing it leaves a hole in the page where the text was. Twelve of the
fourteen now draw from bundled metric-compatible faces: Liberation Sans for Helvetica, Serif
for Times, Mono for Courier, unmodified and under the OFL.

- **It was two fixes, and the width one was larger.** The outlines are obvious — look the name
  up in a table and fill from the face. But `Resources::from_dict` only recorded a width run
  when `/Widths` was present, and the standard fourteen have none by definition, so an
  unembedded standard-14 font advanced *every* glyph by the 500-unit default. Drawing the
  glyphs without fixing the widths would have put every character after the first in the
  wrong column, which scores worse against the oracle than not drawing at all. The run is the
  built-in table indexed by **code**, read through the font's own `/Encoding` — a `/WinAnsi`
  font, a `/MacRoman` one and one with a `/Differences` array each get their own answers,
  because the width of 0x92 is a question about the document and not about Helvetica.
- **One name parser, not two.** `mangle_font::substitute` calls the same `split_name` and
  `style_words` the width tables use, so a subset prefix (`ABCDEF+Helvetica-Bold`) and the
  older comma spelling (`Helvetica-Bold,Italic`) resolve for the outline and the width
  together and cannot disagree with each other.
- **The substitution is reported, never silent.** The page carries a note naming the face
  that stands in, because the outlines on the paper are that face's and not the original's,
  and that is the one fact about the picture a user cannot read off it.
- **A composite font gets no stand-in.** Its codes are CIDs the document chose itself, so a
  simple face is not a substitute however well the widths happen to agree.
- **A family that stands in for a standard one by measurement is answered as that family.**
  `Arial`, `TimesNewRoman` and `CourierNew` are not on the list of fourteen and are the names
  a good deal of the wild corpus actually writes, so a table maps each onto the family whose
  metrics it shares. This is a name→table mapping and nothing more — the numbers are still the
  verified Adobe ones — and `split_name` has already stripped the subset prefix and the
  trailing `MT`/`PS`, so one entry per family covers `TimesNewRomanPSMT`,
  `TimesNewRomanPS-BoldItalicMT`, `Arial,Bold` and the rest without a second parser. **What
  this does not do is guess:** refusing `Helv` is refusing to infer a font from a producer's
  abbreviation, and that still happens. What is asserted instead is a property of a named
  font. `HelveticaNeueLTStd-*` is therefore still refused and is meant to be — Helvetica Neue
  is *not* metric-compatible with Helvetica, so mapping it would not be an approximation to be
  tolerated but a number a reader could measure to be wrong. A style word the family does not
  know refuses too, so `Arial-Black` is not quietly answered with `Helvetica-Bold`.

Five things this is *not*, recorded so nobody reads a higher SSIM as more than it is:

1. **`fi` and `fl` are absent from Liberation Sans.** They would have to be composed from
   `f`+`i` and `f`+`l` condensed to fit 500 units, against an `f` of 278 — an invented
   ligature rather than the font's own glyph, and the kind of plausible wrong shape this
   project refuses everywhere else.
2. **`macron` is the wrong macron.** The U+00AF glyph in Liberation is the *spacing* bar, not
   the AFM accent that sits over a base character. A name that resolves to a spacing bar
   where the metrics files say accent is a silently wrong glyph.
3. **`periodcentered` is drawn wider than Adobe's**, so a bullet or a middot in a list comes
   out fatter than the oracle's. The width is right, which is what keeps the line where the
   producer put it.
4. **`Symbol` and `ZapfDingbats` have no substitute and stay refused.** Liberation carries no
   symbol or dingbat face, and inventing a stand-in for a symbol font would put the wrong
   glyphs on the page. They are reported by name.
5. **This does not close every blank page in the corpus.** Measured against `mutool`,
   `pdfbox__data-000001` goes **0.93127 → 0.94703** with ink **448 654 → 477 471** against an
   oracle of 483 043, and `gov__arxiv-1206.5537` **0.87844 → 0.88414**. Both moved, neither is
   close to 0.99, and the rest of the gap is not only fonts. The alias table closed the naming
   half of the remainder but **no corpus page number moved because of it**, and that is a
   finding rather than a null: of the eight corpus files naming `Arial*`, `TimesNewRoman*` or
   `CourierNew*`, four **do not open at all** (`gov__nist-sp800-88`, `gov__nist-fips197`,
   `gov__nist-nistir7657`, `gov__usgs-topo-cnmi-1` — "the page tree root is missing" or "no
   catalogue"), and of the four that do open none draws a glyph in one on any page. So the
   alias is worth having and is worth **waiting to re-measure on** — it should matter a great
   deal to the two NIST reports, which are the corpus's densest users of `TimesNewRomanPSMT`
   (569 spans in `sp800-88`) and `CourierNew` (3069 in `fips197`) — but it cannot be, and is
   not, credited with a number it did not move.

**The cost is deliberate and on the record:** `include_bytes!` puts 4.4 MB of faces into
every binary that links `mangle-font`, with no way to leave them out. That is the price of a
renderer that never has to ask the machine for a font.

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

1. **The font kinds** (F19–F21): **the standard fourteen are done** for a document that does
   not embed them — twelve of the fourteen now draw from bundled metric-compatible faces, with
   their widths from the built-in tables rather than from the 500-unit default, and the page
   says which face is standing in. `Symbol` and `ZapfDingbats` stay refused: Liberation has no
   equivalent and inventing one would put the wrong glyphs on the page. **Type 3 is next**, and
   the corpus scope for it is now measured: **three files** carry a `/Subtype /Type3` font —
   `gov__arxiv-1206.5537.pdf` (23 pages, one Type 3 font) and the two pdfjs regression files
   `ContentStreamCycleType3insideType3.pdf` and `ContentStreamNoCycleType3insideType3.pdf` (three
   fonts each). A Type 3 font's glyphs are content streams rather than outlines, so it is a
   different kind of work from the other five kinds rather than more of the same, and the two
   pdfjs files are specifically about a **content stream that recurses**, which is the case that
   needs a depth bound of its own.

   **Mesh and other exotic shadings (`/ShadingType` 4–7) are smaller than they look**: exactly
   **one** corpus file uses one, `pdfjs__bug1703683_page2_reduced.pdf`, and it uses **type 7**
   (drift), not the mesh types. So the cluster is a single page of a single reduced file, and it
   should be weighed against `/PaintType 2` and the Inspector on that basis rather than on the
   size of the specification.
   Type 1 is done and compared against an oracle at 0.982 SSIM. TrueType is done and
   compared against an oracle, and two-byte codes are done with
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
   closed, plausible, wrong glyph rather than an error. Encodings are done on top of all of
    that: a code now becomes a glyph *name* through the standard tables and `/Differences`
    before it becomes a glyph number, at 0.98780 SSIM against `mutool` (see "Encodings").
2. **Patterns**: the tiling loop. A *shading* pattern as a **fill** colour is done and evaluated
   per pixel where the fill lands, sharing `sh`'s evaluator, and it is done as a **stroke**
   colour too — `SCN` names a pattern resource exactly as `scn` does, and a page whose only ink
   was patterned strokes used to render blank ([D12](docs/known-diffs.md)); `Device::stroke_outline`
   now builds the outline once and both paints fill it, so a patterned stroke cannot come out
   dashed differently from a flat one.
   **Done: the pattern is read and the loop is written** — and it turns out to have **no corpus
   effect at all**, which corrects a claim made here earlier. Nine corpus files *declare* tiling
   patterns (50 each in `comments`, `issue12337`, `highlights` and `bug1992868`, and 301 in
   `bug1795263`), and it was counted from those declarations that the missing loop cost 12 pages.
   **It does not.** Tracing every page of all nine with `mutool trace` finds **zero**
   `colorspace="Pattern"` and **zero** `sh` operators: not one corpus file selects a pattern at
   all. A targeted four-file run confirms it from the other end — after the loop landed, three of
   them score **0.8667** and `comments` **0.8645**, identical to their pre-feature figures to every
   digit. The count came from `mutool show <file> grep PatternType`, which reads the *object
   structure* and cannot see inside a content stream: the same grep for ` re` returns **zero** on a
   PDF full of rectangles, and a substring match for `sh` on a trace hits every line containing
   `glyph`. **A declared resource is not a used one.** The loop is kept as a named M2/M3
   deliverable with tests, not as a fidelity win, and no figure is claimed for it. What is still
   wrong in those four files is **undiagnosed**: `bug1992868` page 3 is at 0.8667 against a file
   median of 0.9633, with 103 marks and **no note of any kind**, so nothing is being refused and
   the page is simply coming out different.
   The loop itself: `tiling_pattern` in `page.rs` returns a `TilingPattern` — the cell's
   `/BBox`, the two steps, the `/Matrix`, the paint type and the
   cell's own content stream — and `tiling_fill` renders the cell **once** into an image and hands
   it to the fill sampler, which repeats it. The cell is drawn by `paint_records`, the same
   marks-to-pixels loop the page uses, so a mark inside a cell is not a second implementation
   that agrees with the page today and drifts tomorrow.
   Three of the decisions are load-bearing and are stated in the code because they are the ones a
   renderer gets wrong *silently*: **a zero `/XStep` or `/YStep` is a value, not an absence** (it
   means the cell is *not* repeated along that axis, drawn once at the pattern-space origin), **a
   negative step is legal** and is kept as written, and **the cell is drawn with pattern space's
   y inverted** to match the image it is read back from — a cell drawn one way and sampled the
   other comes out mirrored, and a mirrored texture looks like a texture.
   Only `/BBox` is refused rather than defaulted — there is no default clip, and one invented
   from zeros is a texture that belongs to nobody — and an unreadable `/XStep` or `/YStep` is
   refused too, each error naming the pattern and the entry at fault.
   **`/PaintType 2` is refused by name and not guessed at.** An uncoloured cell paints in the
   colour in force when the pattern is used, and a `Pattern` colour space has already replaced
   that colour by then, so there is nothing to give it. Painting the cell's own colour would be
   plausible and wrong, which is worse than the refusal.
   Two defects found while building the fill have since been fixed: [D9](docs/known-diffs.md)
   (a filled path was transformed by the content stream's `cm` twice, so a fill under a
   scaled `cm` landed off the page and was drawn nowhere) and [D10](docs/known-diffs.md) (the
   type-2 function added `C1` where the specification adds `C1 - C0`, which is the same answer
   only when `C0` is zero). D9 was the urgent one and its two corpus pages are at 0.98826 and
   0.99366; D10's is at 0.92923, and what is left there is the oracle's dither rather than a
   wrong colour. What the D9 measurement turned up on the way is a defect neither entry
   covered, and it is now fixed too: a stroke's width was scaled by the CTM and by nothing else,
   so it was not multiplied by the page placement and a stroke did not thicken when the page was
   zoomed. The placement is the renderer's business — it is the layer that knows the canvas and
   the scale, and it is the layer that already scales the path's own points — so the width is
   multiplied by it there, in the same place and by the same code, and the two factors multiply
   rather than compose because the geometry has already had the `cm` applied. `gov__irs-f1040`
   went from 0.93970 to 0.97348 and `gov__irs-fw4` from 0.95949 to 0.97742 against `mutool` at
   150 DPI; a `w 0` hairline now draws as the specification's one device pixel instead of not at
   all. What is left is [D11](docs/known-diffs.md): a non-uniform `cm` makes a stroke's width an
   ellipse and this renderer has one number for it, so it takes `sqrt(|det|)` — 45.7 by 25.7
   device pixels where both oracles draw the exact ellipse's 48 by 24. Fixing that means
   stroking in user space and transforming the outline, which is a change to the stroker.
   **The dash pattern has now had the same treatment** ([D11b](docs/known-diffs.md)), and it
   turned out to be two defects rather than one. `stroke_polygon` received every on-run and
   off-run from `walk_dashes` and **stroked both**, so a gap was a thinner stroke rather than
   no stroke and every pattern drew solid — the flag was being thrown away one call away from
   where it was computed. And the lengths themselves were user-space lengths that nothing
   turned into pixels, so a `[6 3] 0 d` line was one solid run at 72, 144 and 288 DPI alike.
   Both halves live in the same layer as the width, for the same reason: the interpreter knows
   the content stream's `cm` and no more, so `page.rs` multiplies the array by
   `mean_scale(ctm) × mean_scale(placement)` beside the geometry rather than the interpreter
   carrying a half-converted pattern. **The phase is scaled by the same factor as the
   lengths** — it is a distance into the pattern measured against the pattern's own total, so
   scaling both leaves the fraction named unchanged, and scaling only the lengths would slide
   every dash along the line as the page is zoomed. A zero-length entry is the one thing
   scaling could break: a factor times a zero is a zero, and `walk_dashes` walks it as a
   hairline, so `[6 0 3 4] 0 d` is nine of ink and four of paper as both oracles draw it.
   `[6 3] 0 d` now gives 6, 12 and 24 pixel on-runs at 72, 144 and 288 DPI — `mutool`'s own
   figures, matched to the pixel, with `pdftoppm` agreeing at 72. Against `mutool` at 150 DPI
   the corpus movement is small, and that is stated rather than dressed: `gov__irs-f1040` page 1
   is 0.97348 → 0.97408 and `gov__arxiv-1512.03385` page 1 does not move, because most corpus
   dash patterns are sub-pixel or on another page. One case is left and recorded: an array that
   sums to zero is drawn solid here, where both oracles draw nothing at all.
   **And the outline itself had a defect the dash work exposed**, now fixed and measured as
   [D13](docs/known-diffs.md): a **two-point** dash run — one `m` and one `l` — drawn in the
   negative x direction came out a **bowtie**, hollow in the middle, while the identical dash
   drawn left to right was solid. `offset_sides` asked `normal_at(path, i - 1, 0)` for the normal
   of the segment arriving at vertex `i`, but `normal_at(_, index, 0)` already means "the segment
   ending at `index`", so the call was one segment too early; at a two-point path's last vertex
   both lookups miss and the normal fell back to a fixed `(0, 1)`, which is right for a segment
   running in the positive x direction and wrong for everything else. It predated D12 and
   reproduced with a plain `0 0 0 RG` stroke. Checking rather than assuming found the same wrong
   argument in three more places — the joins, both caps, and the seam of a ring — and correcting
   the ring's seam is what stopped three existing stroke tests from failing, which is the check
   that says the fix is right rather than merely different. Against `mutool` at 150 DPI,
   `gov__irs-f1040` page 1 is **0.97408 → 0.98250**, RMS 14.58 → 12.75, and 12 909 fewer pixels
   above tolerance; `pdfjs__bug1795263` page 1 **does not move at all**, to the last digit,
   because it carries no stroke that reaches the offset code.
   **What is left on this item is bigger than what was just fixed.**
   [D14](docs/known-diffs.md): `Device::stroke_outline` pushes a subpath's first point onto its
   end whenever the last point is not already the first — which is exactly the condition for the
   subpath being **open** — so every open path of three or more points is stroked as a closed
   ring. It then carries a stroke along its own closing segment and has no caps. `mutool` draws
   `20 20 m 80 20 l 80 80 l S` as an `L`; this draws the `L` plus a diagonal across it. The fix is
   in the path representation rather than in the outline, because `transform_path` throws away
   whether a subpath was closed, and it moves every stroked path with three or more points on
   every page — so it wants its own entry and its own corpus run, and it is what unblocks proper
   cap and join work on a polyline. Two smaller things stand beside it and are left open: a
   bevel's corners are appended to the end of the assembled outline rather than put at the corner
   ([D15](docs/known-diffs.md)), and the fill's coverage depends on where a polygon's edge list
   starts, which moves one boundary pixel by up to eighteen units out of 255
   ([D16](docs/known-diffs.md)).
3. **`Separation`/`DeviceN` tint-transform evaluation: done.** The tint transform is a PDF
   function, and the four kinds already existed in the shading module; they moved **down** into
   `mangle-content`, because the conversion that needs them lives there and the dependency
   direction does not run the other way. `TAMReview`'s median went from 0.7692 to **0.8879** and
   `gov__nist-sp800-88.pdf`'s page 1 from 0.8103 to **0.98501**; no page of either file reports
   `could not be converted` any more. Two things came out of it that were not in the scope and
   are worth keeping: the `Separation` arm was matching on the *resource key* (`Cs8`) rather than
   the space's kind, so it was unreachable for every real file, and `Function::inputs()` returned
   `domain.len() / 2` for a `domain` that is already one pair per input, so it reported zero
   inputs for every function in existence. `Indexed` is the colour space still in the hole — it
   needs its palette, and there is no `/Alternate` to fall through to.
4. **A `TJ` array truncated at 32 elements: done.** `MAX_COLLECTION_DEPTH` in
   `crates/mangle-content/src/tokens.rs` is documented as a bound on how *deep* a bracketed
   collection may nest and was used as a bound on how many *items* it may hold, so every `TJ`
   array longer than 32 elements — which is what kerned justified text looks like — silently
   lost the rest of the line. The bound is split in two, and both are still there because a
   hostile file can open a bracket and never close it: `MAX_COLLECTION_DEPTH` is still 32 for
   nesting, and a new `MAX_COLLECTION_ITEMS` of **8 192** is the item count, with the
   arithmetic in the constant's own comment. `collect` also **recurses** now, so a nested
   bracket is a nested collection rather than an empty one with its items folded into the
   parent — an `/OCMD` membership list is the shape that had been arriving in pieces.
   **Truncation is reported, never silent**: one note per affected collection, naming what was
   cut, where and how far it got, carried on `ContentStream::notes()` and seeded into
   `PageContent::notes` so a renderer's report has it. `pdfjs__TAMReview.pdf`'s median went
   from 0.8879 to **0.92395** and page 17 from 0.8210 to **0.89465**;
   `gov__nist-sp800-88.pdf`'s page 1 is **unchanged at 0.98501**. The file's 22 remaining pages
   below 0.95 are limited by the unembedded `/TiRoARRN~1268702012`, which is item 1's subject
   ([D21](docs/known-diffs.md)). **Depth protection still holds and was measured, not assumed:**
   100 000 unclosed `[` finish holding 31 items and report one note, and raising the depth
   bound to 200 000 instead — the tempting wrong fix — overflows the stack and aborts the
   process.
5. **JBIG2 and JPEG 2000 decoders** (F17, F18). **The scope statement is written: neither is
   built**, and the reasoning is measured rather than assumed — a 4,320-line JPEG 2000 decoder
   was written from ISO/IEC 15444-1, six substantive defects in it were found and fixed against
   `opj_decompress`, and what remains is in the MQ-coded tier-1 pass, which could not be
   certified. Shipping it would mean emitting wrong pixels that the corpus scores as "roughly
   right". Both are wanted by name on the corpus — they are W034 and W038, 67 pages between
   them — and **the reporting half is done**: an image that cannot be decoded now names every
   part of itself it skipped, including a `/Mask` or `/SMask` that was never reached and that
   mask's own codec. So the note says whether closing the codec gap closes the page, which is
   the question that was unanswerable before. Full account, including the three `opj_compress`
   traps that cost time, in [D22](docs/known-diffs.md).
6. **Fill the Inspector's right-hand region** from the object model, which is the window work
   that has not been started.
7. **Break the symmetry of the remaining fixtures.** The diagonal-clip page is asymmetric now
   and scored 0.51652 when its clip was a box, where every symmetric fixture had scored above
   0.99 through the same bug. The rest still need the same treatment.
8. **Form XObjects: done.** A `Do` whose `/Subtype` is `/Form` is executed as a nested content
   stream rather than read as an image, and what it took is written up in
   [STATUS](docs/STATUS.md#a-form-xobject-is-a-nested-content-stream-not-an-image-with-no-samples):
   the form/image decision is made at the XObject dictionary, the `/Matrix` and `/BBox` go into
   the state the form runs in, the form's own `/Resources` are read where the resource dictionary
   is built — the only place that can follow a reference — and every record says which form drew
   it, so a renderer can find the right table for `/F1`. Nesting needed a starting-state entry
   point (`run_with_state`) and a depth bound; `/BBox` clipping needed the corners put through
   the form's own CTM. What it left behind was
   [D17](docs/known-diffs.md), now fixed — see item 9.
9. **A composite font's glyph addressing: done.** A CID is answered by the descendant's
   `/CIDToGIDMap` and by nothing else. Absent, or the name `Identity`, the identifier is the
   glyph number; a stream is one two-byte entry per CID; the font's `cmap` is never consulted,
   because a `cmap` numbers *characters* and a symbolic font's private-use codes are a different
   numbering from the one the file declared. A simple font with a two-byte encoding is a
   different question — its codes are character codes — and now has its own answer, without the
   "the code is the glyph number" step a single-byte code is allowed. `.notdef` is reported as
   nothing rather than drawn. `CidToGid` lives beside `CidWidths` in `metrics.rs`, and
   `mangle-render` reads it from the descendant because it is a property of the font dictionary
   and not of the font program ([D17](docs/known-diffs.md)).

   **The page it was found on does not move, and that is the finding rather than a null
   result.** `pdfjs__issue16263.pdf` was diagnosed here as drawing `.notdef` where the oracle
   draws arrows; it is not — the embedded SymbolMT *does* have a `(3,0)` subtable, it does not
   cover `0x000E`, and the glyph-number fallback was landing on glyph 14, which is the arrow. So
   the fix changes no pixel on that page while removing a wrong answer for every CID in the
   range that subtable does cover. Its 2.6× excess ink is `/Image15`, a 2×2 image whose `/SMask`
   is a 34862×4332 one, drawn as a solid bar where the oracle draws three thin arrows.
10. **The image mask on `pdfjs__issue16263.pdf`: done.** It was **not** a sampling defect, and
   ruling out the other two candidates is most of the finding. `Raster::sample` was not clamping
   out of range — a mask is sampled at the *image's* own `(u, v)`, which is the specification's
   arrangement, and a mask wider than its image is not an out-of-range read. The alpha was not
   read at the wrong scale. **What was there is that `decode` called itself on `/SMask` with no
   check at all**, so `/Image15`'s 34862×4332 mask decoded, sampled to a constant over a 2×2
   image, and painted the picture solid. Four things are now refused in `decode_soft_mask` before
   the mask's stream is touched at all — over the bound, a size that disagrees with the image's,
   a stream that decoded short, and a mask carrying a mask — and each leaves the image drawn
   with its own colours at full alpha. The **memory** half was the urgent one and it is measured:
   `MAX_IMAGE_PIXELS` *was* reached for a mask, but only *after* its stream had been decoded, and
   not at all under a codec. A 408 kB `/FlateDecode` mask inflating to 400 MB grew this process's
   address space by **1 114 624 kB** and said nothing about it; a 354-byte `/DCTDecode` claiming
   16000×16000 grew it by **750 004 kB**, because `zune_jpeg::decode` reserves `w × h × 3` from a
   `/SOF` marker. Both are now **0–400 kB**, refused on the bound before anything is allocated,
   and page 1 went from a 650 MB peak and 38 s to 41 MB and 20 s. A fifth defect fell out: a
   decode's notes were **discarded whenever the decode succeeded**, so a mask that could not be
   read was reported nowhere at all even though the picture drew.

   **The page's numbers do not move, and that is the rule rather than a null result:**
   0.84906 / 73.51 / 237 475 above tolerance / 272 715 ink before and after, against `mutool`'s
   104 890. Skipping the image moves it to 0.94306 and *below* the oracle, which is why the
   difference is visible at all — but an image whose mask cannot be used is **reported and
   drawn**, not dropped and not made invisible, so the bar is still drawn and the page now says
   why the picture has no transparency. See
   [D18](docs/known-diffs.md#d18--an-smask-was-never-checked-against-the-image-it-masks-and-a-hostile-one-cost-a-gigabyte).

Tier B is now standing — see "The wild corpus found" below — and it has overtaken everything
else on this list, because nothing else in the project can see the problems it can see.

## A fax image is two facts, not one — and the two-dimensional codec reads two coordinate systems as one

The wild corpus found four defects in one place, and they are written down together because
each of them was invisible until the one before it was fixed.

**Layout and value are different facts.** How many bits of each byte a sample occupies —
which decides which byte a pixel lives in — is not the same question as what a sample is
worth, which decides what a sample is divided by to become a colour. For every image without
a codec the two answers coincide, which is why collapsing them into one "bits per sample"
number looks harmless. A codec that expands runs makes them disagree: `ccitt_decode` writes
one byte per pixel while `/BitsPerComponent` is still 1, and only the decoder knows the
first while only the dictionary knows the second. Conflating them produced a defect, then its
successor:

| | SSIM vs `mutool` | RMS | above tol | ink ours / oracle |
|---|---|---|---|---|
| read as the declared one bit per sample | 0.38414 | 197.78 | 60.59% | 59.0% / 8.0% |
| read as eight bits and divided by 255 | 0.38414 | 197.78 | 60.59% | 59.0% / 8.0% |
| layout from the decoder, value from `/BitsPerComponent` | 0.91170 | 72.09 | 7.99% | 0.0% / 8.0% |
| and the T.6 codec reading the coding line's coordinates | 0.86317 | 88.84 | 12.14% | 7.98% / 8.0% |
| **and the image placed where its matrix says** | **0.99747** | **4.72** | **0.03%** | **7.99% / 8.0%** |

(`pdfbox__multitiff.pdf` page 1 at 150 DPI.) The first row is a barcode — eight pixels come
out of every byte and the image smears sideways by a factor of eight. The second is a black
page — a fax scan is mostly white, its white sample is the byte 1, and 1/255 is black. Same
one number, two ways of being wrong. `Decoded::one_byte_per_sample` is now how the decoder
says which layout it produced, and `image::decode` carries layout and value apart through a
`SampleRange` rather than as one `bits`. A stencil is the same split again: its sample *is* a
bit however wide the byte a codec wrote it into, so it is tested on the stored bit and never
on a value normalised over 255.

**The fourth row scores lower than the one above it, and the page is much closer to right.**
The third row's 0.91170 was the score for drawing *nothing*: a blank page agrees with a page
that is 92% paper, which is a flattering number for a page with no content on it. The fourth
row draws the whole scan — the right amount of ink (7.98% against the oracle's 7.99%) in the
right horizontal band — and puts it in the wrong rows, because `image::draw` rebased its
writes to the origin of the clipped area and mirrored its samples about the horizontal axis.
That was a placement defect in the image path rather than a codec one, it predated everything
on this page, and it is fixed with its evidence in `docs/known-diffs.md` as D5b. **The codec
was settled before the placement was, and nothing said so until both were measured.**

**The placement, which put a decoded scan in the wrong rows — now done.** A decoded image and
a correctly *placed* image are two different facts, and the page above is where the first was
true while the second was not: the whole page is the scan, so a right codec and a wrong
placement disagree by an amount no codec work could reach.

1. **The write was rebased and the bounds were not.** `area.pixels()` yields absolute device
   rows and columns, and the loop then wrote at `x - x0, y - y0`, which is the *clipped area's*
   origin subtracted from them. Every image not at the page origin was drawn at the page
   origin. `x0`/`y0` are deleted rather than left at zero, because a binding named for an
   origin that is no longer subtracted is the same mistake one refactor away.
2. **The vertical axis was mirrored.** `placement.matrix` inverts y, so `v = 0` is the
   placement's bottom, and `Raster::sample` reads `v = 0` as raster row 0 — the scan's top.
   Every image was upside down. The flip now goes into the **mapping**
   (`matrix.concat(Matrix::new(1.0, 0.0, 0.0, -1.0, 0.0, 1.0))`) rather than into the raster's
   buffer, and that distinction is the whole of the fix: a flipped buffer produces identical
   pixels for every image drawn square to the page — all of the corpus's, all of the old
   fixtures — and different ones as soon as the image is turned, because the turn belongs to the
   placement and a flipped buffer is not carried by one. `mutool draw` settles which is which.

**What the two compose into, and why the SSIM could not see it.** A rebase and a mirror are a
*mirror about the middle of the placement rectangle*, not a shift, so the best pure vertical
translation explained only **0.6270** of the oracle's ink at 650 rows. It is now **0.9972 at no
shift**, and the intermediate figures say why both halves of the check are needed:

| | best shift | translation explains | SSIM |
|---|---|---|---|
| both defects | 650 rows up | 0.6270 | 0.86317 |
| mirror fixed, rebase not | 719 rows up | 0.9972 | — |
| rebase fixed, mirror not | 650 rows up | 0.6270 | — |
| **both fixed** | **none** | **0.9972** | **0.99747** |

A **fraction cannot see a rebase** — with the mirror fixed and the rebase not, the translation
still explains 0.9972 of the oracle's ink with the image 719 rows in the wrong place, because a
shift absorbs a constant displacement exactly. A **shift cannot see a mirror**, because a mirror
is not a shift. SSIM says neither: 0.91170 was this page's score for drawing nothing at all.

**Why nothing caught either, and what is there now.** The five image tests in `pages.rs` drew a
2 × 2 of pure red and `0b0000_1111` repeated on all eight of its rows — neither has vertical
structure, so a mirror is invisible — through `cm`s whose translation is 0, so the rebase
subtracted nothing. Every assertion was on columns. Now, each confirmed to fail on the code
before it: a banded image at a non-zero translate, asserted pixel by pixel at three scales; the
same image at two translations, each leaving the other's rows bare; a quarter-turned
four-quadrant image asserting the *oracle's* orientation, which a buffer-flip fix fails and a
mapping fix passes; a full-page image at the origin, which must not regress; and the corpus page,
where every candidate vertical shift is tried over per-row bitmaps and both the shift and the
fraction it explains are asserted. Three more are unit tests in `image.rs` over a two-by-two
raster of four colours.

**An `/ImageMask` painted nothing.** `Do` names no colour, so the graphics state's fill
colour has to travel with the mark, and the painter had been given black unconditionally —
which is right only where the page was black. The mark now carries the colour it was drawn
with. A pattern colour was reported here first, and is now *evaluated* instead: a `/PatternType 2`
pattern as a mask's colour changes with position across the mask, so it is sampled per pixel
through the same evaluator `sh` uses, which is what draws `pdfjs__issue13372.pdf` — a
portrait whose every pixel is a gradient rather than a sample — at 0.83738 against `mutool`
where it was 0.74347 with no ink at all.

**The CCITT decoder dropped a line's last run.** `Line::samples` fills up to each change
point and stops, and a change point says where a run *ends*, so everything after the last
one was left at the row's opening colour: a row coded `white 3, black 5` came out as eight
white pixels. Every fax fixture in the project ended on a white run, which is the one case
where leaving the tail alone looks right.

**The T.6 two-dimensional path read two coordinate systems as one, and had the mode table
wrong besides.** This was not the PDFBox file being odd. Our decoder recovered only **15 of
287 rows from libtiff's own G.4 encoding of an image libtiff itself round-trips perfectly**,
and Group 3 1D was unaffected, which is why the hand-written fixtures never showed it. Four
defects were in `decode_line_2d`, and every one of them is a way of being wrong that a
specification-derived test cannot see, because each is a perfectly good reading of a rule
nobody had written down wrong:

1. **A run length was added to the reference line's position.** `a0` is the *coding* line's
   current changing element and `b1` and `b2` are positions on the *reference* line; the run
   lengths a horizontal-mode pair carries are counted from `a0`. They were being added to
   `b1`. On the corpus file the reference line is blank at row 4, so `b1` is the line width,
   344, the black run lands outside the row, no change point is recorded, the line comes out
   blank, and every later line is read against a reference that was never written. That is
   272 of a page's 287 rows. The pair also recorded only its *last* changing element, so the
   first run's colour was lost along with it.
2. **`b1_b2` read the reference line's colour at `a0`.** `b1` is the reference's first changing
   element to the right of `a0` whose colour differs from the **coding** line's colour there,
   and the two lines are coded against each other and may disagree there. The search compared
   against the reference's own colour at that position, so it returned an element of the
   coding line's own colour, which is never the useful one. It also located `b1` by walking
   pixels for the first place the two lines differ, which answers `a0 + 1` whenever the coding
   element sits inside a reference run of its own colour.
3. **The elements' polarity was inverted.** `b1_b2` called a changing element black when its
   index in the change list was odd. The first change from a line that opens white *is* black,
   so every element was labelled with the colour it changes *away* from.
4. **Three of the eight `MODE_CODES` entries were mistranscribed and a ninth was missing.**
   `0001` was read as `Vertical(-2)` and is *Pass*; `000011` was read as `Vertical(2)` and is
   *Horizontal*; `0000011` was read as `Vertical(-3)` and is *Vertical(+3)*; `0000001` was read
   as *Pass* and is not a T.4 code at all, while `0000010` — *Vertical(-3)* — was missing, as
   was *Vertical(+3)*. T.4 table 4 has nine mode codes and the table had eight.

The table is now read off libtiff's encoder rather than from memory, which is how a shifted
version of a correct table survives review: a reference line holding one black run and the
same line shifted by `d` can only be a pair of vertical modes, and the bits libtiff writes for
`d = -3` are `0000010`. What settles a rule this size is not a specification-derived test but
one whose expectations were **written by another implementation**: `scripts/make-ccitt-fixtures.py`
has `tiffcp -c g4` encode and `tiffcp -c none` decode, and freezes the stream and the raster
together under `crates/mangle-filters/tests/fixtures/ccitt-g4/`.
`crates/mangle-filters/tests/ccitt_libtiff.rs` reads the pair back.

Against that ground truth the corpus image — object 12 of `pdfbox__multitiff.pdf`, 344 by 287,
`/K -1` — now decodes to **287 of 287 rows, byte for byte, with 0 damaged rows and nothing
truncated**, at `DamagedRowsBeforeError` of both 0 and 1, and **all 15 libtiff cases match
byte for byte** (896 of 896 rows across them). Group 3 2D is the one thing libtiff is not an
oracle for — its decoder rejects every first line built to T.4 and its encoder writes that
line as a vertical zero where T.4 requires the horizontal pair — so that path is still checked
against hand-written bit strings in `src/ccitt.rs`, with every code named in the comment above
it.

## The wild corpus found

`corpus/wild/` holds 77 real files from pdf.js, PDFBox, NIST, the IRS, the USGS and arXiv,
pinned by SHA-256 in `MANIFEST.toml` and fetched by `cargo xtask corpus fetch`. The harness
is `crates/mangle-render/tests/wild_corpus.rs`; it renders every page at 150 DPI, compares it
with `mutool draw` through `compare()`, compares the text with `pdftotext`, and writes a
report per file into `corpus/wild/report/`. It asserts nothing about any individual file,
because a corpus's job is to find things and a threshold on a document nobody has read yet
turns the first surprise into a permanent red build.

The measurements and their evidence are in `docs/known-diffs.md`, which records the latest full
77-file run (**740 pages compared, page median SSIM 0.96020, 164 pages below 0.95**, 73 files
opened, 4 closed, 16642.40s, 0 panicked) and, entry by entry, what each fix since has moved.
That run is the first after D30, so it is the first to measure the ICC-header fix against the
oracle across the whole corpus rather than one file at a time; the corpus-wide figures barely
moved for it, because it closed one page that was not its file's worst. It was also the first to
compare all **352** pages of `pdfjs__freeculture.pdf`, which is where the move from 738 pages to
740 comes from. It was run under a hard 8 GiB cgroup ceiling
(`systemd-run --user --scope -p MemoryMax=8G -p MemorySwapMax=0`) and peaked at 1.6 GiB, so the
bound is headroom rather than a constraint — the run is CPU-bound in an unoptimized build. The
same bound is what makes a run survivable: three earlier ones were lost partway through.
Four findings dominated the run that started all this, and three of them are fixed:

1. **`render_page` renders a blank page for any Flate-compressed content stream.** All 542
   measured pages drew nothing. The interpreter's own note contains the compressed bytes as
   "operator names", and `Page::decoded_contents` decodes the same stream correctly, so the
   decoder works and the renderer does not call it. Every fixture in the project writes its
   content stream unfiltered, which is why the suite was blind to it. **Done** — see above.
2. **A cross-reference stream carrying a PNG predictor is not decoded**, so the reader falls
   back to scanning, cannot reach objects inside object streams, and reports zero pages. 31 of
   the 34 files that would not open have this shape: both IRS forms, NIST FIPS 197, both USGS
   topo sheets, every Word 2013 export in the corpus, every InDesign output. `qpdf
   --object-streams=disable` on the same content turns 0 pages into 5 and clean into clean.
3. **`render_page` gains a spurious row on any page whose size in points times the scale should
   be a whole number** — `792.0 * (150.0/72.0)` is `1650.0000000000002` in f64 and the `.ceil()`
   turns it into 1651, where `mutool` gives 1650. 80 pages across 17 files became uncomparable,
   including every page of both arXiv papers. **Done** — see above.
4. **A stroke painted in a `/Pattern` colour space was dropped entirely**, because the stroke arm
   asked a colour converter for one colour and `Colour::to_rgba` answers `None` for a pattern by
   design — the fill arm already special-cased it and the stroke arm did not. `SCN` naming a
   pattern as the stroke colour is ordinary PDF, so a page whose ink is entirely patterned
   strokes rendered **blank**: 11 marks recorded, nothing composited. **Done** — see above, and
   see [D12](docs/known-diffs.md) for the four other explanations that were ruled out first, and
   for the two further defects the same page exposed ([D13](docs/known-diffs.md), since fixed,
   and the missing `Do` of a form). The third, [D14](docs/known-diffs.md), was found while fixing
   D13 and is larger than the one that led to it.

(1) is fixed, and it turned out to be the smaller of the two render defects. Re-measured with
it fixed, 103 of 542 pages draw and 439 are still blank — 422 of those because of a font, 17
for other reasons. The dominant one is a `Type1` font whose `/FontFile3` is a bare CFF: a
character code reaches a glyph through the charset, and the charset is not read, so the font
is refused rather than guessed at. **Both font clusters are now done** — the CFF charset is
read (ISO 10581 §5.2, three formats and the Standard Strings list) and the standard fourteen
draw from bundled metric-compatible faces. Until they were, no render number in this project
said anything about fidelity and the arXiv page's SSIM sat at 0.7915 whether or not its
content decoded; it is now 0.88414.

**What is left in the corpus is now mostly *not* code, and that is the finding worth acting on.**
The last full run measured 738 pages with an SSIM and **167 below 0.95**. Of those 167:

* **62 are the two JPEG 2000 files** (`nistir7255` 61, `S2` 1), and JPEG 2000 is **formally out of
  scope** — a written decoder was measured against OpenJPEG, found to have six defects, and was not
  shipped ([D22](docs/known-diffs.md)). That is a decision, not an oversight, and it removes 62 pages
  from the queue permanently.
* **38 are this project's own font-substitution policy**, and they are now diagnosed
  ([D24](docs/known-diffs.md)): `fips197`'s other 24 pages and all 14 of `nistir7657` are limited by
  unembedded fonts drawn in a substitute face, with our ink **within 0.2%** of the oracle's on
  `nistir7657`'s worst page (285 305 against 284 710) and every glyph simply a different shape — we
  draw Liberation, `mutool` draws Nimbus, and `B` stands 138 device pixels above the baseline against
  118, which is the ratio between their `OS/2 capHeight` values of 0.659 and 0.565. **Their advances
  are identical**, so widths are not the difference and the mismatch is entirely in the outlines.
  **This is the project's stated policy, not a gap**, so those pages are not addressable by writing
  anything — which is exactly why it is the largest remaining cause of a page below 0.95.
* **12 were the tiling-pattern cluster and are not** (`comments` 4, `issue12337` 3, `highlights` 3,
  `bug1992868` 2). This entry said "the loop over cells is still to be written", which had stopped
  being true when the loop landed and contradicted item 2 of the immediate queue **in this same
  file**. They are in fact the **font-substitution cause above** — the diff images show every line
  doubled and misregistered, and the four files declare tiling patterns without ever selecting one
  ([D30](docs/known-diffs.md)). So they belong with the ~70 font pages, not in their own bucket.
* The rest is `TAMReview`'s same font cause (22), `freeculture` (9), `sp800-88` (7) and a long tail
  of one or two pages across a dozen files.
* **And one page was a genuine missing feature** (`fips197` p1: an `ICCBased` profile carrying no
  `/Alternate`, which D19's route cannot read). **That page is now fixed**
  ([D30](docs/known-diffs.md)): a profile names the space its components are in at bytes 16–20 of
  its own header, so a file that omits `/Alternate` has not declined to say — it has said it
  somewhere else. `fips197` page 1 went **0.7564 → 0.9932** and carries no note, and
  `pdfjs__issue10529.pdf` page 1 went **0.9886 → 0.9973** from the same fix. The compressed case is
  the one to remember: `fips197` stores both profiles behind a `/FlateDecode`, so the header is
  behind the filter.

**So the addressable remainder is about 102 pages, four files hold 70 of them, and — after D30 —
the largest genuinely missing feature left is no longer a colour space.** The re-runs this section
asked for have been done: the page median went 0.95795 → 0.96020 and the pages below 0.95 went
254 (34.4%) → 164 (22.2%), a **35% reduction** — **concentrated in a few files rather than spread
evenly**, with `gov__nist-sp800-88.pdf` alone accounting for 34 of the 90 pages that came back over
the 0.95 line. **The per-file median moved only 0.9698 → 0.97375** and then stopped moving, because
a file is scored on its worst page and D30 closed a page that was not one. All three parts of
Gate 3.1 remain missed, and none is met: per-page ≥ 0.95 (**164 of 740**), median ≥ 0.985
(**0.96020**, 0.025 short), and ≤ 3% below 0.95 (**22.2%** against a 3% bound, a factor of 7.5).

**What is left is dominated by things this project has decided, and by codecs it has declined.**
62 of the 165 are the two JPEG 2000 files ([D22](docs/known-diffs.md)), and ~38 are the
font-substitution policy ([D24](docs/known-diffs.md)). **Both are decisions with evidence behind
them, so neither is a bug to fix — and that is the finding this queue has to report rather than
route around.** Moving the gate number now means changing one of those decisions on its merits, and
that is a question for the human rather than for another corpus run.

**The corpus's largest non-feature gap is closed** — the `TJ` truncation that cost W076 two thirds
of its text on every page was a collection bound written for nesting depth being used as a bound on
item count. The two are separate now, `TAMReview`'s median went from 0.8879 to **0.92395** and the
current run measures it at **0.9240**, and what that file's 22 pages are now limited by is an
unembedded font: D6's subject, and this project's stated policy ([D21](docs/known-diffs.md), and
[D24](docs/known-diffs.md) for what that policy costs on the two files that carry it most heavily).

**The one genuine missing feature anyone looked for in the corpus is closed.**
`gov__nist-fips197.pdf` page 1 was drawn entirely in `[/ICCBased …]` spaces whose profiles carry
**no `/Alternate`** — `/Alternate` occurs zero times in the file — so D19's route had nothing to
read and 387 of the page's 397 marks were refused. **D30 closed it without writing an ICC
transform at all**, which is the useful part: the premise was wrong. A profile states the space its
components are in at bytes 16–20 of its own header, so this file had said what its components are
and the code was looking in the one place a producer may omit. Page 1 went **0.7564 → 0.9932**, the
file's worst page moved 0.7564 → 0.8432, and the file's count below 0.95 fell 25 → 24 — a small
number for a whole page, because 24 of the 25 were font substitution and stay
([D30](docs/known-diffs.md), [D24](docs/known-diffs.md)).

**What this changes about the queue is worth stating plainly: no ICC transform is needed, and
nothing in the corpus is now waiting on one.** The remaining shortfall is the JPX codec
([D22](docs/known-diffs.md)) and the font-substitution policy ([D24](docs/known-diffs.md)), one
decline and one policy, both with evidence recorded.

**A `Separation` or `DeviceN` tint transform is settled** and was the third of those three gaps
before D20: the transform is evaluated, its output is read in the space the file names, and a
transform that is missing is refused **by name** rather than painted black. `TAMReview`'s median
went from 0.7692 to 0.8879 and `gov__nist-sp800-88.pdf` page 1 from 0.8103 to 0.98501
([D20](docs/known-diffs.md)).

Tier B with `SOURCES.md`: a real-world corpus, which is the only way to find the encoding
problems a hand-written fixture cannot express. **Done** — see above.

Settled and no longer queued: **an `ICCBased` colour space**, which used to be refused
outright and took 42 of a 44-page NIST publication with it — `Colour` now carries what the
profile declares from the `cs` operator to the conversion, and reads the colour through the
`/Alternate` the file names, which is the file's own answer for a reader that cannot apply the
profile rather than an approximation of it ([D19](docs/known-diffs.md)). That file's median
SSIM went from 0.7727 to 0.96095 and 41 of its 44 pages below 0.95 came down to 10 — the current run
measures the median at 0.9648 with **7** below 0.95, which is 34 of the 87 pages that came back over
the 0.95 line between the two runs. What is left of the gap is a profile with no `/Alternate` to read
through, which is `fips197` page 1 and the one page named above. And **a
`Separation` or `DeviceN`**, the largest colour-space gap left at the time — a spot colour is a
tint and a tint means whatever the space's `/TintTransform` says it means, so the four function
kinds moved down into `mangle-content` to be evaluated there rather than reimplemented
([D20](docs/known-diffs.md)). Also
settled: images decode and draw, axial and radial shadings paint, the
clip bounds are transformed by exactly one matrix, a clip is now the path the page set rather
than the box around it — the interpreter records the path, its rule and its box together, and
the renderer rasterises the path into a per-pixel mask it multiplies into every fill, so a
diagonal clip is a diagonal and a circular one is round (0.99639 SSIM against `mutool` at
150 DPI) — glyphs fill from embedded TrueType or CFF
outlines through the same filler a path uses, the text matrix and the font size no longer
double-count (a `TJ` kern also displaces the glyph that follows it, not the one at the same
index), and the standard-14 width tables are verified rather than trusted — every one of the
1043 widths was measured against two independent renderers and Adobe's own metrics, so
nobody should re-derive them from a local URW clone. An alias is a different thing from a
number and was not treated as one: the tables are unchanged, and only the names that reach
them grew. A font that declares no `/Widths` now
answers from those tables through its own `/Encoding` rather than from one average advance —
see "Standard-14 substitution".

The clip work is otherwise finished: a clip stays in force until a `W` changes it or a `Q`
restores it, so a mark's rendering depends only on its own record. What is left is the cap —
a page's clip keeps at most 32 paths, past which the outermost are dropped for box-only
culling, and the fix is one shared sweep over the paths rather than a larger number.

One loose end on the write side: `StreamCompression::Flate` is defined but not wired into
`encode_stream`, so a stream chosen for writing is not compressed. What the compressor now
emits is a zlib stream, which `/FlateDecode` names and three other programs can read, so the
work left is the call site rather than the format.

## Type 3 fonts — the design, with the API facts checked

Deferred rather than abandoned: the corpus scope is measured ([D31](docs/known-diffs.md)) and the
mechanism is settled. Every API below was read before writing it down, so this is a design rather
than a sketch. `gov__arxiv-1206.5537.pdf` is the target that justifies it — 23 pages, all of
which score 0.95–0.995 **with no text at all**, because its `/FontMatrix 1 0 0 -1 0 0` CMMI font is
the one thing drawn on them.

**Why the recursion belongs in the renderer, not in `interp.rs`.** `execute_form` is the wrong
model: a form is a sibling of the page, but a glyph procedure runs *inside* a text object, with
the glyph's own matrix already applied and with the enclosing text state's colour, clip and font
set. `Mark::Glyphs` already receives the per-glyph matrix, and `paint_records` is already the
function that turns a `PageContent` into marks — so the renderer has both halves and the
interpreter has neither.

**The transform, which is the part that is easy to get subtly wrong.** `paint_records` computes
`to_device = placement.matrix.concat(record.ctm)`, and a glyph currently draws at
`placement · record.ctm · glyph_matrix`. A glyph procedure therefore has to enter with the state
matrix already equal to `record.ctm · glyph_matrix · font_matrix`, and be handed the **same**
page placement — not a modified one — so the two compose exactly once. Seeding the interpreter is
`GraphicsState::new()` then one `concat(m)`, which reads as `ctm = identity.concat(m) = m`, and
`concat` is `ctm = ctm.concat(m)`, so a second `concat` inside the procedure composes on the right
side. `Resources::from_dict` takes the document as a resolver and is already used that way for a
form's own resources, which is the same shape a font's `/Resources` needs.

**The steps**

1. **Parse.** `/Subtype /Type3` → `/FontMatrix` (identity is the default), `/FontBBox`,
   `/CharProcs`, and `/Resources` read through `Resources::from_dict`. This replaces the refusal
   added in [D31](docs/known-diffs.md), so that entry's reason is what the report says until this
   lands — and it must keep naming the font, because a procedure that cannot be read is still a
   refusal.
2. **Select.** The code becomes a glyph name through the **existing** `encoding` — the same
   `Encoding` the outline path already uses, and `Type3` fonts are simple fonts, so
   `/Differences` applies to them exactly as it does to a Type 1. A code with no entry in
   `/CharProcs` is a space or a code the font does not have, and is skipped as it is now.
3. **Execute.** Decode the procedure with `decode_stream`, and **refuse rather than execute** if
   `encoded` is set or the data is empty — the same rule `execute_form` uses, and for the same
   reason: handing still-compressed bytes to the operator table draws a plausible wrong page.
4. **Bound.** A depth counter on the recursion, and a note when it trips. The pdfjs files are
   named for a **procedure that re-enters itself**, so this is not defensive padding — it is one of
   the two corpus cases. It is a separate counter from `MAX_FORM_DEPTH` on purpose: a glyph inside
   a form inside a glyph is three different things, and one shared budget would let a page spend
   the form depth on glyphs.
5. **`/FontBBox` clips** the procedure. Worth doing while the code is open; it is also the only
   part of this that can make a page *worse* if got backwards, so it is the piece to test last.

**The honest risk, stated before it is taken.** A glyph procedure is arbitrary content, so
supporting it widens what the interpreter runs, and a bug in the depth bound is a stack overflow
rather than a note. The mitigation is the bound being the *first* thing merged and tested, ahead of
any rendering, because it is the only part of this that can crash.

## M4 — surgical write-back

GOAL.md §4 is the quality bar this project is judged on, and §4.3 states its rule plainly: an edit
is a **byte-range replacement**, not a re-serialisation. Re-serialising a content stream from its
token list would reproduce the rendering while silently discarding everything this project does
not model — the author's comments, their whitespace, their number formatting, an operator from a
later revision — and the user finds out years later. So the write-back in `mangle_edit::surgery`
holds to one line: *give back the stream with some ranges replaced and every other byte
identical*, and refuse anything that would make that untrue.

Three decisions are already settled by tests:

- **Patches apply by position, never by the order they were written.** Sorting by range makes the
  same edit written either way produce the same file, which is what makes an edit reproducible at
  all. The order is also the order they are reported in, so a history entry names what happened
  rather than what was asked for.
- **Bounds and overlap are checked before a byte is copied.** A rejected edit costs nothing and
  cannot have half-applied. Overlap is refused rather than resolved, because two patches covering
  the same byte would make the result depend on their order — and the caller who wrote them knows
  which they meant in a way this module does not.
- **`q`/`Q` balance is checked across the result**, because GOAL.md §4.3 asks for it and because an
  edit that opens a wrapper without closing it leaves every operator after it under the wrong
  graphics state, with nothing anywhere to say so.

What the module deliberately does **not** do is make an edit *sane*. A caller that moves a picture
by rewriting its `cm` must also re-materialise the graphics state around the new `cm` (§4.7); that
belongs to the caller plus `PageModel`'s spans, and pretending otherwise here would hide the
hard problem.

**The corpus round trip is the test that matters**, and it is kept for a reason no unit test
gives: `tests/surgery_corpus.rs` takes a real page, scales a real image's `cm`, and checks two
things — that the interpreter sees the placement change, and that putting the original bytes back
over the same span returns the original stream *exactly*. The second is what a re-serialisation
cannot satisfy and what a user would never notice.

## What the edits found on real pages

`tests/edits_corpus.rs` runs move, scale, delete and recolour over three corpus pages. It found
two defects that **every unit test in this module passed**, and they are the reason it exists.

**1. An insertion with no whitespace of its own becomes part of the token it lands in.**
`pdfjs__TAMReview.pdf` writes `0.000 Tc(Working Papers on Information Systems)Tj` — an operator
immediately against its operand, which the lexer handles and which nothing in the file forbids.
Inserting `q 1 0 0 1 12 -3 cm` at the start of that string produced `Tcq 1 0 0 1 …`: `Tcq` is
**one keyword**, so the wrapper this module had just opened was not an operator at all and the
`Q` that was meant to close it closed nothing. The stream was unbalanced and the page drew under a
wrong graphics state. Nothing reported it. The fix is that an insertion carries its own separator,
and the mirror for a deletion, because a delete that leaves `Td` against `Q` writes `TdQ` and
removes an operator.

**2. A colour written operands-after-operator is a different stream.** The recolour emitted
`rg 0.784 0.063 0.18`. A content stream is written operands first: `0.784 0.063 0.18 rg`. What was
written instead reads as an `rg` with *no operands* followed by four stray numbers — so the colour
was never set, the next operator quietly collected the numbers as its own operands, and the page
rendered perfectly with **the old colour**. My first unit test asserted the patch *text*
(`contains("rg 1 1 1")`) and passed; the corpus test asserted the *colour the interpreter reports*
and failed. That is the whole argument for running the interpreter on the result: a string of the
right words in the wrong order is still a string of the right words.

Both are the shape of mistake STATUS.md's preamble describes — a fixture written to be simple is a
fixture that cannot express them.

**3. An inline image's span was one byte long.** `mangle-content`'s tokeniser gave an inline image
the span `start..start + 1` — the `B` of `BI` — instead of the whole `BI … EI` region. Every content
test passed, because a span that is too short does not change what is *read*. It changed what was
*written*: the first edit on `pdfjs__TAMReview.pdf` inserted its wrapper at that byte and split the
keyword into `B` + `I`, so the image vanished from the reopened page with nothing reporting it.
Found by the write-back round trip, and it is a defect in GOAL.md §4.1's **seventh law** — an object
that claims a byte it does not occupy is a provenance lie, and provenance is what makes surgical
editing possible at all. The regression test is in `mangle-content`, because that is where the bug
was; the round trip that found it is in `mangle-edit`.

**A fourth thing the corpus test pins down rather than a bug:** an edit's matrix is in the stream's
**user space**, while an object's bounds are in **device space** with the CTM already applied. They
are the same only under the identity CTM. `map_point` converts, and the test asserts the conversion
on a page that really does scale — `TAMReview`'s first object is drawn inside a `cm` scaled by 106,
so twelve points of user space are over a thousand on the page — because a drag that lands somewhere
the pointer never was is worse than one that does not move. Writing the assertion as "+12" is how an
edit gets "fixed" into being wrong.

## The save, and what it must not do

`writeback::save_page` is the other half of the loop, and it is built around what must *not* happen.
FINISH.md S2 wants an edit, a save, a reopen, and an untouched JPEG stream still bit-identical; the
only way to be sure of that is for a save to add a revision rather than rewrite the file, and to
touch nothing it did not have to.

- **Only the parts that changed are written.** A page whose `/Contents` is an array of four streams
  gets one new object and three untouched ones. Their bytes are not re-encoded, because
  re-encoding is re-compressing and a re-compressed image is a different image.
- **The filter is the file's own.** `encode_stream` re-encodes with the filter the stream declared.
  A filter *chain* is refused by name rather than guessed at — content streams are almost never
  filtered twice, and a chain re-encoded wrongly is a page that does not open.
- **A patch that straddles a boundary is refused.** The offsets a `PageObject` carries are offsets
  into the *concatenation* of the page's content streams, because that is what the interpreter ran.
  A patch covering the end of one stream and the start of the next is two patches about two
  different objects, and writing it as one would put one object's bytes inside another.
- **An encrypted document is refused.** A new revision of an encrypted file has to be encrypted,
  and this does not do that; the alternative is a file that opens with a repair prompt.

## Arrange — the edit that cannot be a wrapper

Every other edit here is a wrapper, because a `q … Q` changes the space an object is drawn **in**.
Arrange changes the *order* operators appear in, and in a content stream that order **is** the
z-order. So an arrange is a **move**: the object's bytes come out of where they were and go in
somewhere else, wrapped in every piece of state the record carries — the CTM, the colours in the
space the file used, the stroke width, the caps, the joins, the dash.

Two things about it are worth writing down:

- **The direction is inverted from what it looks like.** Later operators draw on top, so a move to
  the front is a move to *after* the neighbour and a move to the back is a move to *before* it.
  Getting it the other way round is an arrange that appears to work and moves the object the
  opposite way. The first version of this did exactly that, and the test that caught it was the one
  that read the bytes back.
- **What the record does not carry is where this stops.** A clip cannot be re-established (a record
  carries the *region* the clip resolved to, not the path operators that built it); an alpha or a
  blend mode cannot be re-established (they live in a named `/ExtGState`, and inventing a name
  would either collide with the file's or draw attention to a resource nobody defined). Each is
  refused with a name.
- **Text was in that list and no longer is.** A run's appearance depends on the text state in force
  — the font, the size, `Tc`, `Tw`, `Tz`, `TL`, `Ts` — and `Record` used to carry the font and the
  size and nothing else, so a moved run was refused. It now carries `text` and `text_matrix`, and
  arrange writes them back: `BT /F1 12 Tf 0.5 Tc 0 Tw 90 Tz 14 TL 3 Ts`, then the matrix, then the
  moved bytes inside the `BT … ET` the wrapper closes. Across three corpus pages that took **201
  refusals down to 168** and 582 arrangements up to 615, which is what "most of a page is text"
  actually costs. What is still refused is a clip, an alpha, a blend mode and a shading — and the
  numbers on a real page are what say which of those matter.

## The session, and the shape a save takes

`session::Editor` ties the pieces together: it owns the document, the page, the page's resources and
the history, and every operation goes through it. GOAL.md §4.8's four requirements are what it is
for — a command pattern, deep history, and undo that restores *structurally identical* content.

Two decisions in it are worth writing down:

- **The history holds the page's content streams, not the model.** A model is a *derivation* —
  whatever the interpreter makes of the stream — so restoring the model but not the stream would
  leave a session whose picture and whose bytes disagree. The selection is deliberately not restored
  either: it is a UI concern and this module is headless.
- **A save after an undo takes the target, not the range.** The interesting question a session
  raises is what "the patches from the file to what I now have" means once an undo has moved the
  stream somewhere no single edit produced. Composing byte-range edits across an undo is a merge,
  and a merge that picks a side is a merge that can drop bytes. So `writeback::save_decoded` takes
  what each stream should now contain, and the session hands it exactly that.

One bug the session's own tests found, and it is the reason `part_lengths` exists in `writeback`
rather than in `session`: a stream split back into parts **by the lengths they started with** gives
an edit that inserted bytes into the first content stream to the second one. The save moved bytes
into the wrong object and nothing reported it; the split now goes by where the patches landed,
which is what the save already did.

## A stroked line had no box to click on

GOAL.md §4.2 asks for "exact bounds (including stroke width and glyph bounds)". The stroke half was
missing: `bounds_of` took the path's own geometry and nothing else, so a horizontal rule drawn with
`2 w S` along `y = 0` had a box of **zero height** — and a click a quarter of a unit above the line,
square on the ink, missed. `Record::bounds` now grows a stroked path's box by half the *device* width
on every side, which is where a stroke actually sits, and a filled path's box is still its geometry
alone because a fill has no width to add. The corpus model's object counts did not move — f1040 page
1 is still 589 objects, TAMReview 36 — which is the check that this changed the boxes and not the
grouping.

## Text properties, and the check that had to move

Every control GOAL.md §4.4 lists that is a *text-state* operator is now a `Change`: `Tf`, `Tc`,
`Tw`, `Tz`, `TL`, `Ts`, `Tr`. Each is a `q … Q` around the run's own `Tj` with the one operator
inside it, which is what makes it local.

Writing them found **the operand-order bug for the third time**: the text property was emitted as
`Tc 2` rather than `2 Tc`, which is a `Tc` with no operands followed by a stray number — the property
was never set and the page drew exactly as it did. The unit test that asserted the patch *text*
passed. The corpus test that asserted the *value the interpreter reports* caught it. The third
occurrence is the one that settles the lesson, so it is written on the function that does it.

The verification also had to move, and for an interesting reason. A text property **moves the
glyphs**: character spacing widens the run, a baseline shift lifts it, and on a page whose text
matrix is sheared (`0 7 -6.9999 0 41.598 748.001 Tm` on `irs-f1040`) the run spreads *along the
shear*, so it leaves the line it was in and the model regroups — 589 objects become 656 on one
character-spacing change. That is the file asking for what it asked for. A check that insisted the
object count was unchanged would have failed a correct edit, so the text property is verified by
matching the run **by its string** and reading the value back out of the interpreter, and the
grouping is left to be what it is.

## A stroked line had no box to click on

GOAL.md §4.2 asks for "exact bounds (including stroke width and glyph bounds)". The stroke half was
missing: `bounds_of` took the path's own geometry and nothing else, so a horizontal rule drawn with
`2 w S` along `y = 0` had a box of **zero height** — and a click a quarter of a unit above the line,
square on the ink, missed. `Record::bounds` now grows a stroked path's box by half the *device* width
on every side, which is where a stroke actually sits, and a filled path's box is still its geometry
alone because a fill has no width to add. The corpus model's object counts did not move — f1040 page
1 is still 589 objects, TAMReview 36 — which is the check that this changed the boxes and not the
grouping.

## Text properties, and the check that had to move

Every control GOAL.md §4.4 lists that is a *text-state* operator is now a `Change`: `Tf`, `Tc`,
`Tw`, `Tz`, `TL`, `Ts`, `Tr`. Each is a `q … Q` around the run's own `Tj` with the one operator
inside it, which is what makes it local.

Writing them found **the operand-order bug for the third time**: the text property was emitted as
`Tc 2` rather than `2 Tc`, which is a `Tc` with no operands followed by a stray number — the property
was never set and the page drew exactly as it did. The unit test that asserted the patch *text*
passed. The corpus test that asserted the *value the interpreter reports* caught it. The third
occurrence is the one that settles the lesson, so it is written on the function that does it.

The verification also had to move, and for an interesting reason. A text property **moves the
glyphs**: character spacing widens the run, a baseline shift lifts it, and on a page whose text
matrix is sheared (`0 7 -6.9999 0 41.598 748.001 Tm` on `irs-f1040`) the run spreads *along the
shear*, so it leaves the line it was in and the model regroups — 589 objects become 656 on one
character-spacing change. That is the file asking for what it asked for. A check that insisted the
object count was unchanged would have failed a correct edit, so the text property is verified by
matching the run **by its string** and reading the value back out of the interpreter, and the
grouping is left to be what it is.

What is left in M4 is the UI wiring.
