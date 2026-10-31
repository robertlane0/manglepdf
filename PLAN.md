# ManglePDF — Plan

> The authoritative specification is [`GOAL.md`](GOAL.md); the definition of done is
> [`FINISH.md`](FINISH.md). This file is the *working* plan: what is being built right
> now, in what order, and why. Where each milestone stands lives in
> [`docs/STATUS.md`](docs/STATUS.md); architectural decisions live in `docs/decisions/`;
> corpus fidelity notes live in [`docs/known-diffs.md`](docs/known-diffs.md).

## Order of work

The product is one vertical spine plus breadth; [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md#the-spine)
draws the spine. The spine is where the architecture is decided, so it is built first and
deeply. Annotations, forms, pages, protect and export hang off it and follow.

## Key architectural bets

One row per bet. Full records live in [`docs/decisions/`](docs/decisions/).

| # | Bet | Why |
|---|---|---|
| [ADR-0001](docs/decisions/0001-lossless-cos-layer.md) | Lossless COS layer: every object kept verbatim (raw stream bytes, original `/Filter` chain), lazily resolved from file bytes via xref; edits live in an object-level copy-on-write overlay | Enables byte-for-byte preservation of everything we do not touch (Charter §4.1 law 1) and incremental saves (FINISH S1.17) |
| [ADR-0002](docs/decisions/0002-provenance-content-streams.md) | Content streams are parsed into a token/operator stream that **retains byte spans**; the page-object model maps every object back to its provenance ranges | Makes surgical write-back, the Inspector, and "what changed" diffing possible |
| [ADR-0003](docs/decisions/0003-in-house-flate.md) | In-house Flate **inflate** (must return partial output for truncated streams) and in-house deflate/zlib | Standard crates abort on corruption; a truncated stream must return partial data with a diagnostic note |
| [ADR-0004](docs/decisions/0004-gui-toolkit.md) | egui + eframe (winit) for the GUI, everything custom-painted from design tokens | Pure Rust, no toolkit look to fight, custom canvas, HiDPI, IME, and accessibility |
| [ADR-0005](docs/decisions/0005-retype-own-font.md) | Text retyping walks the font resolution ladder as surgical changes: the run's own font, then a same-family font already in the document, then a bundled TrueType face, subsetted and embedded | Charter §4.4 asks for a retyped headline in a file every other viewer opens perfectly; the earlier the rung, the less of the file changes |
| [ADR-0006](docs/decisions/0006-image-codecs.md) | Permitted codecs only for JPEG (DCT) decode/encode; PNG is in-house on top of our own Flate | PNG needs exact control of predictors/bit depths; JPEG is general-purpose |
| [ADR-0007](docs/decisions/0007-shared-rasterizer.md) | Own analytic-coverage scanline rasterizer shared by PDF rendering *and* SVG icon rendering | One rasterizer to get right; dogfoods the renderer (Charter §8.7) |

`ttf-parser` reads TrueType and OpenType tables (see [`docs/DEPENDENCIES.md`](docs/DEPENDENCIES.md)).
The shaper the original plan paired it with, `rustybuzz`, was never called and is no longer a
dependency: nothing in the workspace shapes text.

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

Where each milestone stands: [`docs/STATUS.md`](docs/STATUS.md#milestones).

## What is next, in order

The gap list in [`docs/STATUS.md`](docs/STATUS.md#open-gaps-in-priority-order) is the queue. This is
why it is in that order:

1. **The spine's loose ends first**: the M5 UI, reading order, and the viewer and renderer pieces
   of M1–M3. A gap in the spine costs more later than a gap in breadth.
2. **Then breadth, in the order of the gauntlet scenarios it unlocks**: a surface for annotations
   and flatten (S3–S5), forms (S6), the organizer (S7), protect (S8–S9) and export (S10). The
   engines exist; nothing in the UI or CLI reaches them.
3. **Then the floors**: performance and the fuzzer (M10), which Gates 2 and 7 need.
4. **Then polish and the formal run**: M11, the remaining Tier-A fixtures, the
   `--automation-stdio` line protocol and the gauntlet runner (M12).
